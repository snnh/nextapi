//! 日志查询 / 手动清理 / CSV 导出（PLAN §7.2）。契约 contracts/m4-logging.md §10，M4-B 实现。
//!
//! - `GET /api/logs`：usage_logs 分页 + 过滤，附 key/upstream 名称（内存快照 join）。
//! - `GET /api/logs/export.csv`：同过滤，上限 100 000 行，超出截断并尾部追加注释行。
//! - `GET /api/logs/{request_id}`：该 request_id 最新一条（全字段含 usage_raw/debug_payload）。
//! - `POST /api/logs/cleanup`：整分区 DROP（dry_run 预览）。
//!
//! 全部 SQL 使用运行时校验（sqlx::query / QueryBuilder），不使用 query! 宏。
//! 排序 / 过滤参数一律 bind，禁止拼接用户输入。

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, QueryBuilder};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{self, AdminUsername};
use crate::cache::Snapshot;
use crate::entities::UsageLogRow;
use crate::error::{ApiError, ApiResult};
use crate::logging::partition;
use crate::state::AppState;
use rust_decimal::Decimal;

/// 日志查询超时（M11.2 资源治理）：30s 未返回 → 503，防慢查询拖垮连接池。
const LOG_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

async fn with_log_timeout<F, T, E>(fut: F) -> Result<Result<T, E>, ApiError>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    match tokio::time::timeout(LOG_QUERY_TIMEOUT, fut).await {
        Ok(r) => Ok(r),
        Err(_) => Err(ApiError::internal(
            "日志查询超时（30s），请缩小时间范围".to_string(),
        )),
    }
}

/// usage_logs 查询列清单（与 entities::UsageLogRow 字段名一一对应）。
const LOG_COLS: &str = "id, request_id, ts, key_id, model, requested_model, upstream_id, protocol_in, protocol_out, \
    convert_mode, stream, prompt_tokens, completion_tokens, cache_write_tokens, cache_read_tokens, \
    images, image_size, video_seconds, video_resolution, video_task_type, latency_ms, status, error, \
    retry_count, ttfb_ms, degraded, pricing_source, cost_cny, cost_usd, price_used, fx_snapshot, \
    usage_raw, debug_payload, request_headers";

/// 列表返回项：usage_logs 全字段 + 名称（内存快照 join 附加）。
#[derive(Debug, Serialize)]
struct LogItem {
    #[serde(flatten)]
    row: UsageLogRow,
    key_name: Option<String>,
    key_prefix: Option<String>,
    upstream_name: Option<String>,
}

/// `/api/logs` 查询参数（from/to/key_id/upstream_id 以字符串接收后手动解析）。
#[derive(Debug, Default, Deserialize)]
struct LogQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    from_ts: Option<String>,
    to_ts: Option<String>,
    key_id: Option<String>,
    upstream_id: Option<String>,
    model: Option<String>,
    stream: Option<bool>,
    status: Option<i32>,
    status_group: Option<String>,
    request_id: Option<String>,
    degraded: Option<bool>,
}

/// 解析后的日志过滤条件。
#[derive(Debug, Clone)]
struct LogFilter {
    from_ts: DateTime<Utc>,
    to_ts: DateTime<Utc>,
    key_id: Option<Uuid>,
    upstream_id: Option<Uuid>,
    model: Option<String>,
    stream: Option<bool>,
    status: Option<i32>,
    status_group: Option<String>,
    request_id: Option<String>,
    degraded: Option<bool>,
}

/// `POST /api/logs/cleanup` 请求体。
#[derive(Debug, Deserialize)]
struct CleanupReq {
    before: String,
    dry_run: bool,
}

/// dry-run 预览汇总：待删明细规模与用量/成本合计（M14 清理口径）。
#[derive(Debug, Serialize, Default)]
struct CleanupSummary {
    log_rows: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_write_tokens: i64,
    cache_read_tokens: i64,
    cost_cny: Decimal,
    cost_usd: Decimal,
}

/// 实际生效截止时间：被删分区的最大 to_ts（分区边界对齐）；无可删分区 → None。
fn effective_cut(dropped: &[partition::PartitionInfo]) -> Option<DateTime<Utc>> {
    dropped.iter().map(|p| p.to_ts).max()
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_logs))
        // 静态段优先于 {request_id} 动态段；仍先注册以防匹配歧义。
        .route("/export.csv", get(export_csv))
        .route("/cleanup", post(cleanup_logs))
        .route("/{request_id}", get(get_log))
}

/// GET /api/logs：分页 + 过滤返回日志列表。
async fn list_logs(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Query(q): Query<LogQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let tz = state.hot.load().gateway.billing_timezone.clone();
    let (page, page_size) = validate_pagination(q.page, q.page_size)?;
    let f = build_log_filter(&q, &tz)?;
    let offset = ((page as i64) - 1) * (page_size as i64);

    // 事务 + SET LOCAL：服务端 statement_timeout 与客户端 30s 超时对齐，
    // 超时后服务端查询被取消而非继续占用连接（发布审阅数据批 #7）。
    let mut tx = state.db.begin().await?;
    sqlx::query("SET LOCAL statement_timeout = '31s'")
        .execute(&mut *tx)
        .await?;

    // 总数（与列表共用同一过滤条件；M11.2：30s 查询超时治理）
    let mut count_qb = QueryBuilder::<Postgres>::new("SELECT count(*) FROM usage_logs WHERE");
    push_log_where(&mut count_qb, &f);
    let total: i64 = with_log_timeout(count_qb.build_query_scalar().fetch_one(&mut *tx)).await??;

    // 数据页
    let mut qb = QueryBuilder::<Postgres>::new(format!("SELECT {LOG_COLS} FROM usage_logs WHERE"));
    push_log_where(&mut qb, &f);
    qb.push(" ORDER BY ts DESC, id DESC LIMIT ")
        .push_bind(page_size as i64);
    qb.push(" OFFSET ").push_bind(offset);
    let rows: Vec<UsageLogRow> =
        with_log_timeout(qb.build_query_as().fetch_all(&mut *tx)).await??;
    let _ = tx.rollback().await;

    let snap = state.cache.snapshot();
    let items: Vec<LogItem> = rows
        .into_iter()
        .map(|row| attach_names(row, &snap))
        .collect();

    Ok(Json(serde_json::json!({
        "items": items,
        "total": total,
        "page": page,
        "page_size": page_size,
    })))
}

/// GET /api/logs/export.csv：同过滤，上限 100 000 行，超出截断并追加注释行。
async fn export_csv(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Query(q): Query<LogQuery>,
) -> ApiResult<Response> {
    const LIMIT: usize = 100_000;

    let tz = state.hot.load().gateway.billing_timezone.clone();
    let f = build_log_filter(&q, &tz)?;

    let mut tx = state.db.begin().await?;
    sqlx::query("SET LOCAL statement_timeout = '31s'")
        .execute(&mut *tx)
        .await?;
    // 多查一行以判断是否截断
    let mut qb = QueryBuilder::<Postgres>::new(format!("SELECT {LOG_COLS} FROM usage_logs WHERE"));
    push_log_where(&mut qb, &f);
    qb.push(" ORDER BY ts DESC, id DESC LIMIT ")
        .push_bind(LIMIT as i64 + 1);
    let rows: Vec<UsageLogRow> =
        with_log_timeout(qb.build_query_as().fetch_all(&mut *tx)).await??;
    let _ = tx.rollback().await;

    let truncated = rows.len() > LIMIT;
    let slice = if truncated { &rows[..LIMIT] } else { &rows[..] };

    let snap = state.cache.snapshot();
    let mut out = String::new();
    out.push_str(
        "request_id,ts,key_name,model,upstream_name,protocol_in,protocol_out,convert_mode,stream,\
         status,prompt_tokens,completion_tokens,cache_read_tokens,cache_write_tokens,latency_ms,\
         ttfb_ms,retry_count,degraded,cost_cny,cost_usd,error",
    );
    for row in slice {
        out.push('\n');
        out.push_str(&csv_record(row, &snap));
    }
    if truncated {
        out.push_str("\n# truncated at 100000");
    }

    Ok(csv_response(out))
}

/// GET /api/logs/{request_id}：该 request_id ts 最新一条（全字段）。
async fn get_log(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Path(request_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // 与列表/导出一致（M14.1）：事务内 SET LOCAL statement_timeout，
    // 客户端 30s 超时触发时服务端查询同步取消，不再占用连接。
    let mut tx = state.db.begin().await?;
    sqlx::query("SET LOCAL statement_timeout = '31s'")
        .execute(&mut *tx)
        .await?;

    let row = with_log_timeout(
        sqlx::query_as::<_, UsageLogRow>(&format!(
            "SELECT {LOG_COLS} FROM usage_logs WHERE request_id = $1 \
             ORDER BY ts DESC, id DESC LIMIT 1"
        ))
        .bind(&request_id)
        .fetch_optional(&mut *tx),
    )
    .await??;
    let _ = tx.rollback().await;
    let row = row.ok_or(ApiError::NotFound)?;

    let snap = state.cache.snapshot();
    let item = attach_names(row, &snap);
    Ok(Json(serde_json::json!(item)))
}

/// POST /api/logs/cleanup：整分区 DROP（dry_run 只返回待删列表与汇总）。
async fn cleanup_logs(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Json(req): Json<CleanupReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let before = parse_rfc3339(&req.before)?;
    // 防误删当前/未来分区：before 晚于现在时仅按时间也必然覆盖当前分区（review P3）
    if before > chrono::Utc::now() {
        return Err(ApiError::bad_request("before 不能晚于当前时间"));
    }
    let days = state.hot.load().gateway.log_partition_days;
    let dropped = partition::drop_covered(&state.db, before, days, req.dry_run).await?;

    // 边界归一化（M14 §5.4）：明细按整分区 DROP，聚合表按同一截止时间删除。
    // effective_before = 被删分区的最大 to_ts（分区边界对齐），两者口径一致；
    // 无可删分区时不清理 usage_hourly（避免明细还在、聚合已被删的背离）。
    let effective_before = effective_cut(&dropped);

    let hourly_rows: u64 = match effective_before {
        None => 0,
        Some(cut) => {
            if req.dry_run {
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM usage_hourly WHERE hour < $1")
                    .bind(cut)
                    .fetch_one(&state.db)
                    .await? as u64
            } else {
                sqlx::query("DELETE FROM usage_hourly WHERE hour < $1")
                    .bind(cut)
                    .execute(&state.db)
                    .await?
                    .rows_affected()
            }
        }
    };

    // dry-run 汇总：待删明细的行数 / Token / 成本合计（仅预览时执行，避免执行路径多一次大范围扫描）。
    let summary: Option<CleanupSummary> = if req.dry_run {
        match effective_before {
            None => Some(CleanupSummary::default()),
            Some(cut) => {
                let row = with_log_timeout(
                    sqlx::query_as::<_, (i64, i64, i64, i64, i64, Decimal, Decimal)>(
                        "SELECT count(*), \
                         COALESCE(sum(prompt_tokens),0)::int8, \
                         COALESCE(sum(completion_tokens),0)::int8, \
                         COALESCE(sum(cache_write_tokens),0)::int8, \
                         COALESCE(sum(cache_read_tokens),0)::int8, \
                         COALESCE(sum(cost_cny),0), COALESCE(sum(cost_usd),0) \
                         FROM usage_logs WHERE ts < $1",
                    )
                    .bind(cut)
                    .fetch_one(&state.db),
                )
                .await??;
                Some(CleanupSummary {
                    log_rows: row.0,
                    prompt_tokens: row.1,
                    completion_tokens: row.2,
                    cache_write_tokens: row.3,
                    cache_read_tokens: row.4,
                    cost_cny: row.5,
                    cost_usd: row.6,
                })
            }
        }
    } else {
        None
    };

    // 非 dry_run 且有删除时写审计
    if !req.dry_run && !dropped.is_empty() {
        let summaries: Vec<serde_json::Value> = dropped
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name, "from": p.from_ts, "to": p.to_ts,
                    "size_bytes": p.size_bytes, "row_estimate": p.row_estimate,
                })
            })
            .collect();
        auth::audit(
            &state,
            &admin.0,
            "logs.cleanup",
            "usage_logs",
            None,
            serde_json::json!({
                "before": before,
                "effective_before": effective_before,
                "partitions": summaries,
            }),
            None,
        )
        .await?;
    }

    Ok(Json(serde_json::json!({
        "dry_run": req.dry_run,
        "dropped": dropped,
        // dry_run 时为待删行数预估；实际执行时为已删行数
        "hourly_rows": hourly_rows,
        // 实际生效截止时间（分区边界对齐）；null = 没有可清理的分区
        "effective_before": effective_before,
        "summary": summary,
    })))
}

/// 解析查询参数为过滤条件（from_ts/to_ts 缺省 = billing_timezone 当天 00:00 → 现在）。
fn build_log_filter(q: &LogQuery, tz: &str) -> ApiResult<LogFilter> {
    let now = Utc::now();
    let from_ts = match q.from_ts.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => parse_rfc3339(s)?,
        None => crate::limit::window_start("daily", tz, now),
    };
    let to_ts = match q.to_ts.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => parse_rfc3339(s)?,
        None => now,
    };
    let key_id = q
        .key_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(parse_uuid)
        .transpose()?;
    let upstream_id = q
        .upstream_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(parse_uuid)
        .transpose()?;

    if to_ts <= from_ts {
        return Err(ApiError::bad_request("to_ts 必须晚于 from_ts"));
    }
    // 限制单次查询跨度，避免日志/统计接口消耗无限数据库资源。
    if to_ts.signed_duration_since(from_ts) > chrono::Duration::days(366) {
        return Err(ApiError::bad_request("日志查询时间跨度不能超过 366 天"));
    }

    Ok(LogFilter {
        from_ts,
        to_ts,
        key_id,
        upstream_id,
        model: q.model.clone().filter(|s| !s.is_empty()),
        stream: q.stream,
        status: q.status,
        status_group: q
            .status_group
            .clone()
            .filter(|s| matches!(s.as_str(), "success" | "4xx" | "5xx" | "429" | "degraded")),
        request_id: q.request_id.clone().filter(|s| !s.is_empty()),
        degraded: q.degraded,
    })
}

/// 追加日志过滤 WHERE（与计数/数据查询共用）。参数一律 bind。
fn push_log_where(qb: &mut QueryBuilder<'_, Postgres>, f: &LogFilter) {
    qb.push(" ts >= ").push_bind(f.from_ts);
    qb.push(" AND ts < ").push_bind(f.to_ts);
    if let Some(k) = f.key_id {
        qb.push(" AND key_id = ").push_bind(k);
    }
    if let Some(u) = f.upstream_id {
        qb.push(" AND upstream_id = ").push_bind(u);
    }
    if let Some(m) = f.model.as_deref() {
        qb.push(" AND model = ").push_bind(m.to_string());
    }
    if let Some(s) = f.stream {
        qb.push(" AND stream = ").push_bind(s);
    }
    if let Some(s) = f.status {
        qb.push(" AND status = ").push_bind(s);
    }
    if let Some(group) = f.status_group.as_deref() {
        match group {
            "success" => qb.push(" AND status < 400"),
            "4xx" => qb.push(" AND status >= 400 AND status < 500"),
            "5xx" => qb.push(" AND status >= 500 AND status < 600"),
            "429" => qb.push(" AND status = 429"),
            "degraded" => qb.push(" AND degraded = TRUE"),
            _ => qb,
        };
    }
    if let Some(r) = f.request_id.as_deref() {
        qb.push(" AND request_id = ").push_bind(r.to_string());
    }
    if let Some(d) = f.degraded {
        qb.push(" AND degraded = ").push_bind(d);
    }
}

/// 分页参数校验：page 缺省 1（<1 钳制为 1）；page_size 缺省 20，须在 1..=200 内。
fn validate_pagination(page: Option<u32>, page_size: Option<u32>) -> ApiResult<(u32, u32)> {
    // page 上限防深 OFFSET 慢查询（review P2-12）：200/页 × 10 万页封顶
    const MAX_PAGE: u32 = 100_000;
    let page = page.unwrap_or(1).max(1);
    let page_size = page_size.unwrap_or(20);
    if page > MAX_PAGE {
        return Err(ApiError::bad_request(format!("page 不能超过 {MAX_PAGE}")));
    }
    if !(1..=200).contains(&page_size) {
        return Err(ApiError::bad_request("page_size 必须在 1..=200 之间"));
    }
    Ok((page, page_size))
}

/// RFC3339 解析 → Utc；失败 → BadRequest。
fn parse_rfc3339(s: &str) -> ApiResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| ApiError::bad_request(format!("时间戳解析失败（需 RFC3339）: {e}")))
}

/// UUID 解析 → 失败 → BadRequest。
fn parse_uuid(s: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(s).map_err(|e| ApiError::bad_request(format!("UUID 解析失败: {e}")))
}

/// 内存快照 join：key 按 id 遍历 values 查找（量小可接受）；upstream 按 id 直查。
fn attach_names(row: UsageLogRow, snap: &Snapshot) -> LogItem {
    let key = row
        .key_id
        .and_then(|id| snap.api_keys.values().find(|k| k.id == id));
    let upstream = row.upstream_id.and_then(|id| snap.upstreams.get(&id));
    LogItem {
        row,
        key_name: key.map(|k| k.name.clone()),
        key_prefix: key.map(|k| k.prefix.clone()),
        upstream_name: upstream.map(|u| u.name.clone()),
    }
}

/// 组装 CSV 单行（列序固定，按契约 §10）。
fn csv_record(row: &UsageLogRow, snap: &Snapshot) -> String {
    let key = row
        .key_id
        .and_then(|id| snap.api_keys.values().find(|k| k.id == id));
    let upstream = row.upstream_id.and_then(|id| snap.upstreams.get(&id));
    let fields = [
        row.request_id.clone(),
        row.ts.to_rfc3339(),
        key.map(|k| k.name.clone()).unwrap_or_default(),
        row.model.clone(),
        upstream.map(|u| u.name.clone()).unwrap_or_default(),
        row.protocol_in.clone(),
        row.protocol_out.clone(),
        row.convert_mode.clone(),
        row.stream.to_string(),
        row.status.to_string(),
        opt_i64(row.prompt_tokens),
        opt_i64(row.completion_tokens),
        opt_i64(row.cache_read_tokens),
        opt_i64(row.cache_write_tokens),
        opt_i32(row.latency_ms),
        opt_i32(row.ttfb_ms),
        row.retry_count.to_string(),
        row.degraded.to_string(),
        row.cost_cny.map(|d| d.to_string()).unwrap_or_default(),
        row.cost_usd.map(|d| d.to_string()).unwrap_or_default(),
        row.error.clone().unwrap_or_default(),
    ];
    fields
        .iter()
        .map(|s| csv_escape(s))
        .collect::<Vec<_>>()
        .join(",")
}

fn opt_i64(v: Option<i64>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}
fn opt_i32(v: Option<i32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}

/// 手写 CSV 转义：`"` → `""`；含 `,` / `"` / 换行时整体加引号。
/// CSV 单元格转义：引号/逗号/换行 + 公式注入前缀中和（= + - @ 开头前置单引号，CWE-1236）。
fn csv_escape(s: &str) -> String {
    let s = match s.chars().next() {
        Some('=' | '+' | '-' | '@' | '\t' | '\r') => {
            let mut t = String::with_capacity(s.len() + 1);
            t.push('\'');
            t.push_str(s);
            t
        }
        _ => s.to_string(),
    };
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// 构造 CSV 下载响应（Content-Type / Content-Disposition）。
fn csv_response(body: String) -> Response {
    let mut resp = Response::new(Body::from(body));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"usage_logs.csv\""),
    );
    resp
}

#[cfg(test)]
mod cleanup_tests {
    use super::{effective_cut, partition, DateTime};

    fn part(to: &str) -> partition::PartitionInfo {
        partition::PartitionInfo {
            name: "usage_logs_p0".to_string(),
            from_ts: DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .to_utc(),
            to_ts: DateTime::parse_from_rfc3339(to).unwrap().to_utc(),
            size_bytes: 0,
            row_estimate: 0,
        }
    }

    /// 边界归一化：取被删分区的最大 to_ts；无候选 → None（不清理聚合表）。
    #[test]
    fn effective_cut_uses_latest_partition_end() {
        assert_eq!(effective_cut(&[]), None);
        let cut = effective_cut(&[
            part("2026-01-31T00:00:00Z"),
            part("2026-03-02T00:00:00Z"),
            part("2026-02-15T00:00:00Z"),
        ])
        .unwrap();
        assert_eq!(cut.to_rfc3339(), "2026-03-02T00:00:00+00:00");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn csv_escape_no_special() {
        assert_eq!(csv_escape("abc"), "abc");
        assert_eq!(csv_escape(""), "");
    }

    #[test]
    fn csv_escape_doubles_quotes() {
        assert_eq!(csv_escape("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn csv_escape_quotes_on_comma() {
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
    }

    #[test]
    fn csv_escape_quotes_on_newline() {
        assert_eq!(csv_escape("a\nb"), "\"a\nb\"");
    }

    #[test]
    fn csv_escape_neutralizes_formula_prefix() {
        // 公式注入（CWE-1236）：= + - @ 及制表符/回车开头前置单引号
        assert_eq!(csv_escape("=cmd|' /C calc'!A0"), "'=cmd|' /C calc'!A0");
        assert_eq!(csv_escape("+SUM(A1)"), "'+SUM(A1)");
        assert_eq!(csv_escape("-1+2"), "'-1+2");
        assert_eq!(csv_escape("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(csv_escape("\t=1"), "'\t=1");
        // 常规字段不受影响
        assert_eq!(csv_escape("gpt-4o"), "gpt-4o");
        assert_eq!(csv_escape("abc"), "abc");
    }

    #[test]
    fn validate_pagination_defaults() {
        assert_eq!(validate_pagination(None, None).unwrap(), (1, 20));
        assert_eq!(validate_pagination(Some(3), Some(10)).unwrap(), (3, 10));
    }

    #[test]
    fn validate_pagination_page_zero_clamped() {
        // page=0 钳制为 1；page_size=10 合法
        assert_eq!(validate_pagination(Some(0), Some(10)).unwrap(), (1, 10));
    }

    #[test]
    fn validate_pagination_page_size_out_of_range() {
        assert!(matches!(
            validate_pagination(Some(1), Some(0)),
            Err(ApiError::BadRequest(_))
        ));
        assert!(matches!(
            validate_pagination(Some(1), Some(201)),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn parse_rfc3339_invalid_is_bad_request() {
        assert!(matches!(
            parse_rfc3339("not-a-time"),
            Err(ApiError::BadRequest(_))
        ));
        assert!(parse_rfc3339("2024-01-01T00:00:00Z").is_ok());
    }

    #[test]
    fn parse_uuid_invalid_is_bad_request() {
        assert!(matches!(parse_uuid("nope"), Err(ApiError::BadRequest(_))));
        assert!(parse_uuid("00000000-0000-0000-0000-000000000000").is_ok());
    }

    #[test]
    fn build_log_filter_parses_uuid_and_dates() {
        let q = LogQuery {
            from_ts: Some("2024-01-01T00:00:00Z".into()),
            to_ts: Some("2024-01-02T00:00:00Z".into()),
            key_id: Some("00000000-0000-0000-0000-000000000000".into()),
            ..Default::default()
        };
        let f = build_log_filter(&q, "Asia/Shanghai").unwrap();
        assert_eq!(f.from_ts, utc("2024-01-01T00:00:00Z"));
        assert_eq!(f.to_ts, utc("2024-01-02T00:00:00Z"));
        assert!(f.key_id.is_some());
    }

    #[test]
    fn build_log_filter_invalid_uuid_bad_request() {
        let q = LogQuery {
            key_id: Some("bad".into()),
            ..Default::default()
        };
        assert!(matches!(
            build_log_filter(&q, "Asia/Shanghai"),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn build_log_filter_empty_ts_defaults_to_window_start() {
        // 空串视为缺省 → 走 daily 窗口起点，不应报错
        let q = LogQuery {
            from_ts: Some(String::new()),
            to_ts: Some(String::new()),
            ..Default::default()
        };
        let f = build_log_filter(&q, "Asia/Shanghai").unwrap();
        assert!(f.from_ts <= f.to_ts);
    }
}
