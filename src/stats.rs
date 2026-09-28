//! 统计聚合查询（PLAN §5.5/§7.2）。契约 contracts/m4-logging.md §9，M4-B 实现。
//!
//! - 写路径（insert_batch）在同一事务内维护 usage_logs（明细）与 usage_hourly（按小时+model 的汇总），
//!   因此查询可直读两张表，始终反映最新数据。
//! - `summary` 恒用明细源（usage_logs），支持全维度过滤 + 百分位（percentile_cont）。
//! - `series` 按 `pick_source` 选择数据源：粒度=day 或区间 > 7 天且维度 ∈ {None, model} → 汇总源，
//!   否则回退明细源；当过滤条件含 key/upstream/protocol 时（usage_hourly 无这些列）强制明细源。
//!
//! 全部 SQL 使用运行时校验（sqlx::query* / QueryBuilder），不使用 query! 宏。
//! 动态 WHERE 只从白名单 `match` 取列名/表达式，参数一律 bind，禁止拼接用户输入。

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

/// 统计过滤条件（含计费时区；from/to 为闭开区间 [from, to)）。
#[derive(Debug, Clone)]
pub struct StatsFilter {
    pub from_ts: DateTime<Utc>,
    pub to_ts: DateTime<Utc>,
    pub key_id: Option<Uuid>,
    pub upstream_id: Option<Uuid>,
    pub model: Option<String>,
    /// protocol 匹配 usage_logs.protocol_in
    pub protocol: Option<String>,
    pub billing_tz: String,
}

/// 聚合汇总结果（source = "detail" | "hourly"）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Summary {
    pub requests: i64,
    pub errors: i64,
    pub success_rate: f64,
    /// 未命中缓存的输入 token（协议层已归一，见 protocol/* 与 upstream/usage.rs）。
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// 缓存命中（读）token。
    pub cache_read_tokens: i64,
    /// 缓存写入 token。
    pub cache_write_tokens: i64,
    /// 总 token = prompt + completion + cache_read + cache_write（含缓存，与配额口径一致）。
    pub total_tokens: i64,
    /// 输入侧缓存命中率 = cache_read / (prompt + cache_read + cache_write)；分母 0 → None。
    pub cache_hit_rate: Option<f64>,
    pub cost_cny: Option<Decimal>,
    pub cost_usd: Option<Decimal>,
    /// 展示币种（display_currency）下的合计成本；全 NULL → None。
    pub cost_display: Option<Decimal>,
    /// 有成本另一侧但展示币种列为 NULL 的行数（未计价混入）。
    pub cost_na_count: i64,
    pub avg_latency_ms: Option<f64>,
    /// 仅 source=detail 时非空（percentile_cont 需要 latency_ms）
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub p99_ms: Option<f64>,
    pub source: String,
}

/// 时序序列点。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SeriesPoint {
    pub bucket: DateTime<Utc>,
    pub dimension: String,
    pub requests: i64,
    pub errors: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// 缓存命中（读）token。
    pub cache_read_tokens: i64,
    /// 缓存写入 token。
    pub cache_write_tokens: i64,
    pub cost_cny: Option<Decimal>,
    pub cost_usd: Option<Decimal>,
    /// 展示币种（display_currency）下的合计成本；全 NULL → None。
    pub cost_display: Option<Decimal>,
    /// 有成本另一侧但展示币种列为 NULL 的行数（未计价混入）。
    pub cost_na_count: i64,
}

/// 模型排行行（P5-a：区间内按模型聚合的 Top-N）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelRankRow {
    pub model: String,
    pub requests: i64,
    pub errors: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// 总 token（含缓存）。
    pub total_tokens: i64,
    /// 输入侧缓存命中率；分母 0 → None。
    pub cache_hit_rate: Option<f64>,
    /// 平均延迟 ms。
    pub avg_latency_ms: Option<f64>,
    pub cost_cny: Option<Decimal>,
    pub cost_usd: Option<Decimal>,
    pub cost_display: Option<Decimal>,
    pub cost_na_count: i64,
}

/// 输入侧缓存命中率 = cache_read / (prompt + cache_read + cache_write)。
/// 分母为 0（无任何输入 token）→ None（展示 `-`，不算 0%）。纯函数，可测。
///
/// 三个分量各自 `max(0)` 钳位：协议层归一后不应出现负值，但脏数据（如 prompt=-5、
/// cache_read=10）会让分母小于分子从而算出 >1（前端显示 200%），故在此统一钳位。
pub fn cache_hit_rate(cache_read: i64, prompt: i64, cache_write: i64) -> Option<f64> {
    let cr = cache_read.max(0);
    let input_total = prompt
        .max(0)
        .saturating_add(cr)
        .saturating_add(cache_write.max(0));
    if input_total <= 0 {
        return None;
    }
    // 钳位后分子 ≤ 分母，比率恒在 [0, 1]。
    Some(cr as f64 / input_total as f64)
}

/// 数据源选择（纯函数，可测）：
/// granularity=="day" 或区间 > 7 天，且 dimension ∈ {None, Some("model")} → "hourly"，
/// 否则 → "detail"（usage_hourly 只有 model 维度，PLAN §6）。
pub fn pick_source(
    granularity: &str,
    from_ts: DateTime<Utc>,
    to_ts: DateTime<Utc>,
    dimension: Option<&str>,
) -> &'static str {
    let dim_ok = matches!(dimension, None | Some("model"));
    let span_days = (to_ts - from_ts).num_seconds() as f64 / 86_400.0;
    if dim_ok && (granularity == "day" || span_days > 7.0) {
        "hourly"
    } else {
        "detail"
    }
}

/// 明细源汇总行（内部反序列化用）。
#[derive(sqlx::FromRow)]
struct SummaryRow {
    requests: i64,
    errors: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    total_tokens: i64,
    cost_cny: Option<Decimal>,
    cost_usd: Option<Decimal>,
    cost_display: Option<Decimal>,
    cost_na_count: i64,
    avg_latency_ms: Option<f64>,
    p50_ms: Option<f64>,
    p95_ms: Option<f64>,
    p99_ms: Option<f64>,
}

/// 序列点行（内部反序列化用）。
#[derive(sqlx::FromRow)]
struct SeriesRow {
    bucket: DateTime<Utc>,
    dimension: String,
    requests: i64,
    errors: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost_cny: Option<Decimal>,
    cost_usd: Option<Decimal>,
    cost_display: Option<Decimal>,
    cost_na_count: i64,
}

impl From<SeriesRow> for SeriesPoint {
    fn from(r: SeriesRow) -> Self {
        SeriesPoint {
            bucket: r.bucket,
            dimension: r.dimension,
            requests: r.requests,
            errors: r.errors,
            prompt_tokens: r.prompt_tokens,
            completion_tokens: r.completion_tokens,
            cache_read_tokens: r.cache_read_tokens,
            cache_write_tokens: r.cache_write_tokens,
            cost_cny: r.cost_cny,
            cost_usd: r.cost_usd,
            cost_display: r.cost_display,
            cost_na_count: r.cost_na_count,
        }
    }
}

/// 模型排行行（内部反序列化用；命中率/总 token 在 Rust 侧派生）。
#[derive(sqlx::FromRow)]
struct RankRow {
    model: String,
    requests: i64,
    errors: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    avg_latency_ms: Option<f64>,
    cost_cny: Option<Decimal>,
    cost_usd: Option<Decimal>,
    cost_display: Option<Decimal>,
    cost_na_count: i64,
}

/// 展示币种对应的成本列名（"CNY" → "cost_cny"，"USD" → "cost_usd"；调用方已校验白名单）。
fn cost_col(display_currency: &str) -> &'static str {
    if display_currency == "USD" {
        "cost_usd"
    } else {
        "cost_cny"
    }
}

/// 汇总：恒用明细源（usage_logs），需全维度过滤 + percentile_cont。
///
/// Token 口径（P1）：`prompt_tokens` 为未缓存输入，总 token 含 cache_read/cache_write。
/// 明细表缓存列自 M4 起就在写（计价用），无需回填；汇总源缺列问题见 0017 迁移。
pub async fn summary(
    conn: &mut sqlx::PgConnection,
    f: &StatsFilter,
    display_currency: &str,
) -> ApiResult<Summary> {
    let cc = cost_col(display_currency);
    // cost_display = 指定币种成本合计；cost_na_count = 有成本另一侧但指定币种列为 NULL 的行数。
    let mut qb = QueryBuilder::<Postgres>::new(format!(
        "SELECT count(*) AS requests, \
         count(*) FILTER (WHERE status >= 400) AS errors, \
         COALESCE(sum(prompt_tokens), 0)::bigint AS prompt_tokens, \
         COALESCE(sum(completion_tokens), 0)::bigint AS completion_tokens, \
         COALESCE(sum(cache_read_tokens), 0)::bigint AS cache_read_tokens, \
         COALESCE(sum(cache_write_tokens), 0)::bigint AS cache_write_tokens, \
         (COALESCE(sum(prompt_tokens), 0) + COALESCE(sum(completion_tokens), 0) \
          + COALESCE(sum(cache_read_tokens), 0) + COALESCE(sum(cache_write_tokens), 0))::bigint AS total_tokens, \
         sum(cost_cny) AS cost_cny, \
         sum(cost_usd) AS cost_usd, \
         sum({cc}) AS cost_display, \
         count(*) FILTER (WHERE {cc} IS NULL AND (cost_cny IS NOT NULL OR cost_usd IS NOT NULL)) AS cost_na_count, \
         avg(latency_ms)::float8 AS avg_latency_ms, \
         percentile_cont(0.5) WITHIN GROUP (ORDER BY latency_ms) AS p50_ms, \
         percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms) AS p95_ms, \
         percentile_cont(0.99) WITHIN GROUP (ORDER BY latency_ms) AS p99_ms \
         FROM usage_logs"
    ));
    push_stats_where(&mut qb, f);

    let row: SummaryRow = qb.build_query_as().fetch_one(&mut *conn).await?;
    let requests = row.requests;
    let errors = row.errors;
    // 无请求时视为 100% 成功（无失败即成功）。
    let success_rate = if requests == 0 {
        1.0
    } else {
        (requests - errors) as f64 / requests as f64
    };

    Ok(Summary {
        requests,
        errors,
        success_rate,
        prompt_tokens: row.prompt_tokens,
        completion_tokens: row.completion_tokens,
        cache_read_tokens: row.cache_read_tokens,
        cache_write_tokens: row.cache_write_tokens,
        total_tokens: row.total_tokens,
        cache_hit_rate: cache_hit_rate(
            row.cache_read_tokens,
            row.prompt_tokens,
            row.cache_write_tokens,
        ),
        cost_cny: row.cost_cny,
        cost_usd: row.cost_usd,
        cost_display: row.cost_display,
        cost_na_count: row.cost_na_count,
        avg_latency_ms: row.avg_latency_ms,
        p50_ms: row.p50_ms,
        p95_ms: row.p95_ms,
        p99_ms: row.p99_ms,
        source: "detail".into(),
    })
}

/// 序列：granularity 为 "hour"|"day"，dimension 为 None|"model"|"key"|"upstream"|"protocol"。
/// 数据源由 `pick_source` 决定，但 usage_hourly 无法表达 key/upstream/protocol 过滤，故强制回退明细源。
pub async fn series(
    conn: &mut sqlx::PgConnection,
    f: &StatsFilter,
    granularity: &str,
    dimension: Option<&str>,
    display_currency: &str,
) -> ApiResult<Vec<SeriesPoint>> {
    let g = match granularity {
        "hour" => "hour",
        "day" => "day",
        other => {
            return Err(ApiError::bad_request(format!(
                "granularity 必须为 hour 或 day: {other}"
            )))
        }
    };

    let mut src = pick_source(g, f.from_ts, f.to_ts, dimension);
    if f.key_id.is_some() || f.upstream_id.is_some() || non_empty(f.protocol.as_deref()) {
        src = "detail";
    }

    if src == "hourly" {
        series_hourly(
            &mut *conn,
            f,
            g,
            display_currency,
            matches!(dimension, Some("model")),
        )
        .await
    } else {
        series_detail(&mut *conn, f, g, dimension, display_currency).await
    }
}

fn non_empty(s: Option<&str>) -> bool {
    s.map(|v| !v.is_empty()).unwrap_or(false)
}

/// 汇总源序列：usage_hourly 聚合，dimension 恒 model。
///
/// bucket 与明细源**同式**按 billing_tz 截断：`date_trunc(g, hour AT TIME ZONE $tz) AT TIME ZONE $tz`。
/// 此前用裸 `date_trunc(g, hour)`（按 DB 会话时区），与 series_detail 的 billing_tz 截断不一致，
/// 同一条请求在两种源里会落到不同的「天」——区间跨过 7 天阈值（触发 pick_source 换源）时
/// 图表分桶会跳变。
async fn series_hourly(
    conn: &mut sqlx::PgConnection,
    f: &StatsFilter,
    g: &str,
    display_currency: &str,
    dimension_model: bool,
) -> ApiResult<Vec<SeriesPoint>> {
    let cc = cost_col(display_currency);
    // dimension=None（总量趋势）：只按时间桶聚合，输出恒 'all'——
    // 此前恒按 model 分组，总量趋势被拆成多行（发布审阅数据批 #2）。
    let dim_sql = if dimension_model {
        "model AS dimension,"
    } else {
        "'all' AS dimension,"
    };
    let mut qb =
        QueryBuilder::<Postgres>::new(format!("SELECT date_trunc('{g}', hour AT TIME ZONE "));
    qb.push_bind(&f.billing_tz); // $1
    qb.push(format!(
        ") AT TIME ZONE $1 AS bucket, \
         {dim_sql} \
         sum(requests)::bigint AS requests, \
         sum(errors)::bigint AS errors, \
         sum(prompt_tokens)::bigint AS prompt_tokens, \
         sum(completion_tokens)::bigint AS completion_tokens, \
         sum(cache_read_tokens)::bigint AS cache_read_tokens, \
         sum(cache_write_tokens)::bigint AS cache_write_tokens, \
         sum(cost_cny) AS cost_cny, \
         sum(cost_usd) AS cost_usd, \
         sum({cc}) AS cost_display, \
         count(*) FILTER (WHERE {cc} IS NULL AND (cost_cny IS NOT NULL OR cost_usd IS NOT NULL)) AS cost_na_count \
         FROM usage_hourly"
    ));
    qb.push(" WHERE hour >= ").push_bind(f.from_ts);
    qb.push(" AND hour < ").push_bind(f.to_ts);
    if let Some(m) = f.model.as_ref().filter(|s| !s.is_empty()) {
        qb.push(" AND model = ").push_bind(m.clone());
    }
    // GROUP BY / ORDER BY 引用输出列别名（PostgreSQL 允许），与 series_detail 写法一致。
    if dimension_model {
        qb.push(" GROUP BY bucket, model ORDER BY bucket, dimension");
    } else {
        qb.push(" GROUP BY bucket ORDER BY bucket");
    }

    let rows: Vec<SeriesRow> = qb.build_query_as().fetch_all(&mut *conn).await?;
    Ok(rows.into_iter().map(SeriesPoint::from).collect())
}

/// 明细源序列：bucket = date_trunc(granularity, ts AT TIME ZONE $tz) AT TIME ZONE $tz
/// （tz 参数化传 billing_tz；回转为 timestamptz 以映射 DateTime<Utc>）。
/// 维度从白名单取列名/表达式：key→key_id::text，upstream→upstream_id::text，protocol→protocol_in。
async fn series_detail(
    conn: &mut sqlx::PgConnection,
    f: &StatsFilter,
    g: &str,
    dimension: Option<&str>,
    display_currency: &str,
) -> ApiResult<Vec<SeriesPoint>> {
    let cc = cost_col(display_currency);
    let dim_expr = match dimension {
        None => "",
        Some("model") => "model",
        Some("key") => "COALESCE(key_id::text, '')",
        Some("upstream") => "COALESCE(upstream_id::text, '')",
        Some("protocol") => "protocol_in",
        Some(other) => {
            return Err(ApiError::bad_request(format!(
                "dimension 必须为 model/key/upstream/protocol: {other}"
            )))
        }
    };

    let mut qb =
        QueryBuilder::<Postgres>::new(format!("SELECT date_trunc('{g}', ts AT TIME ZONE "));
    qb.push_bind(&f.billing_tz); // $1
    qb.push(") AT TIME ZONE $1 AS bucket, ");
    if dim_expr.is_empty() {
        qb.push("'all' AS dimension, ");
    } else {
        qb.push(dim_expr);
        qb.push(" AS dimension, ");
    }
    qb.push(format!(
        "count(*) AS requests, \
         count(*) FILTER (WHERE status >= 400) AS errors, \
         COALESCE(sum(prompt_tokens), 0)::bigint AS prompt_tokens, \
         COALESCE(sum(completion_tokens), 0)::bigint AS completion_tokens, \
         COALESCE(sum(cache_read_tokens), 0)::bigint AS cache_read_tokens, \
         COALESCE(sum(cache_write_tokens), 0)::bigint AS cache_write_tokens, \
         sum(cost_cny) AS cost_cny, \
         sum(cost_usd) AS cost_usd, \
         sum({cc}) AS cost_display, \
         count(*) FILTER (WHERE {cc} IS NULL AND (cost_cny IS NOT NULL OR cost_usd IS NOT NULL)) AS cost_na_count \
         FROM usage_logs"
    ));
    push_stats_where(&mut qb, f);
    qb.push(" GROUP BY bucket");
    if !dim_expr.is_empty() {
        qb.push(", ");
        qb.push(dim_expr);
    }
    qb.push(" ORDER BY bucket, dimension");

    let rows: Vec<SeriesRow> = qb.build_query_as().fetch_all(&mut *conn).await?;
    Ok(rows.into_iter().map(SeriesPoint::from).collect())
}

/// 模型排行（P5-a）：区间内按模型聚合的 Top-N（明细源，含延迟与全维度过滤）。
///
/// `order_by` 白名单：total_tokens | requests | cost | cache_read | errors；默认 total_tokens 降序。
/// `limit` clamp 到 1..=100（前端 Top 10 为主，放宽上限便于导出）。
pub async fn model_rank(
    conn: &mut sqlx::PgConnection,
    f: &StatsFilter,
    display_currency: &str,
    order_by: &str,
    limit: i64,
) -> ApiResult<Vec<ModelRankRow>> {
    let cc = cost_col(display_currency);
    // ORDER BY 用聚合表达式（白名单映射，防拼接注入）；派生总量在 SQL 内直算。
    let total_expr = "(COALESCE(sum(prompt_tokens),0) + COALESCE(sum(completion_tokens),0) \
                      + COALESCE(sum(cache_read_tokens),0) + COALESCE(sum(cache_write_tokens),0))";
    let order = match order_by {
        "requests" => "count(*) DESC".to_string(),
        "cost" => format!("COALESCE(sum({cc}), 0) DESC"),
        "cache_read" => "COALESCE(sum(cache_read_tokens), 0) DESC".to_string(),
        "errors" => "count(*) FILTER (WHERE status >= 400) DESC".to_string(),
        // 默认：总 token（含缓存）降序。
        _ => format!("{total_expr} DESC"),
    };
    let lim = limit.clamp(1, 100);

    let mut qb = QueryBuilder::<Postgres>::new(format!(
        "SELECT model, \
         count(*) AS requests, \
         count(*) FILTER (WHERE status >= 400) AS errors, \
         COALESCE(sum(prompt_tokens), 0)::bigint AS prompt_tokens, \
         COALESCE(sum(completion_tokens), 0)::bigint AS completion_tokens, \
         COALESCE(sum(cache_read_tokens), 0)::bigint AS cache_read_tokens, \
         COALESCE(sum(cache_write_tokens), 0)::bigint AS cache_write_tokens, \
         avg(latency_ms)::float8 AS avg_latency_ms, \
         sum(cost_cny) AS cost_cny, \
         sum(cost_usd) AS cost_usd, \
         sum({cc}) AS cost_display, \
         count(*) FILTER (WHERE {cc} IS NULL AND (cost_cny IS NOT NULL OR cost_usd IS NOT NULL)) AS cost_na_count \
         FROM usage_logs"
    ));
    push_stats_where(&mut qb, f);
    // GROUP BY 后按白名单聚合表达式排序（稳定次级键 model，避免同值抖动）。
    qb.push(format!(" GROUP BY model ORDER BY {order}, model ASC"));
    qb.push(" LIMIT ").push_bind(lim);

    let rows: Vec<RankRow> = qb.build_query_as().fetch_all(&mut *conn).await?;
    Ok(rows
        .into_iter()
        .map(|r| ModelRankRow {
            total_tokens: r
                .prompt_tokens
                .saturating_add(r.completion_tokens)
                .saturating_add(r.cache_read_tokens)
                .saturating_add(r.cache_write_tokens),
            cache_hit_rate: cache_hit_rate(
                r.cache_read_tokens,
                r.prompt_tokens,
                r.cache_write_tokens,
            ),
            model: r.model,
            requests: r.requests,
            errors: r.errors,
            prompt_tokens: r.prompt_tokens,
            completion_tokens: r.completion_tokens,
            cache_read_tokens: r.cache_read_tokens,
            cache_write_tokens: r.cache_write_tokens,
            avg_latency_ms: r.avg_latency_ms,
            cost_cny: r.cost_cny,
            cost_usd: r.cost_usd,
            cost_display: r.cost_display,
            cost_na_count: r.cost_na_count,
        })
        .collect())
}

/// 追加明细源公共 WHERE（ts 区间 + 可选维度），参数一律 bind。占位符由 QueryBuilder 按调用顺序编号。
fn push_stats_where(qb: &mut QueryBuilder<'_, Postgres>, f: &StatsFilter) {
    qb.push(" WHERE ts >= ").push_bind(f.from_ts);
    qb.push(" AND ts < ").push_bind(f.to_ts);
    if let Some(k) = f.key_id {
        qb.push(" AND key_id = ").push_bind(k);
    }
    if let Some(u) = f.upstream_id {
        qb.push(" AND upstream_id = ").push_bind(u);
    }
    if let Some(m) = f.model.as_ref().filter(|s| !s.is_empty()) {
        qb.push(" AND model = ").push_bind(m.clone());
    }
    if let Some(p) = f.protocol.as_ref().filter(|s| !s.is_empty()) {
        qb.push(" AND protocol_in = ").push_bind(p.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("构建 UTC 时间")
    }

    // 短区间：1 天（≤ 7 天）；长区间：14 天（> 7 天）
    fn short() -> (DateTime<Utc>, DateTime<Utc>) {
        (utc(2024, 3, 1, 0, 0, 0), utc(2024, 3, 2, 0, 0, 0))
    }
    fn long() -> (DateTime<Utc>, DateTime<Utc>) {
        (utc(2024, 3, 1, 0, 0, 0), utc(2024, 3, 15, 0, 0, 0))
    }

    #[test]
    fn pick_source_hour_short_detail() {
        let (f, t) = short();
        assert_eq!(pick_source("hour", f, t, None), "detail");
        assert_eq!(pick_source("hour", f, t, Some("model")), "detail");
    }

    #[test]
    fn pick_source_hour_long_hourly() {
        let (f, t) = long();
        assert_eq!(pick_source("hour", f, t, None), "hourly");
        assert_eq!(pick_source("hour", f, t, Some("model")), "hourly");
    }

    #[test]
    fn pick_source_day_hourly() {
        let (f, t) = short();
        assert_eq!(pick_source("day", f, t, None), "hourly");
        assert_eq!(pick_source("day", f, t, Some("model")), "hourly");
    }

    #[test]
    fn pick_source_non_model_dim_falls_back_detail() {
        let (f, t) = short();
        let (lf, lt) = long();
        assert_eq!(pick_source("hour", f, t, Some("key")), "detail");
        assert_eq!(pick_source("day", f, t, Some("key")), "detail");
        assert_eq!(pick_source("hour", lf, lt, Some("upstream")), "detail");
        assert_eq!(pick_source("day", lf, lt, Some("protocol")), "detail");
    }

    #[test]
    fn pick_source_invalid_granularity_falls_back_detail() {
        let (f, t) = short();
        // 未知粒度走 detail（与"day"不同）
        assert_eq!(pick_source("minute", f, t, None), "detail");
    }

    #[test]
    fn pick_source_threshold_seven_days() {
        // 恰好 7 天不视为 > 7 天 → hour 粒度 + None → detail
        let from = utc(2024, 3, 1, 0, 0, 0);
        let to = utc(2024, 3, 8, 0, 0, 0);
        assert_eq!(pick_source("hour", from, to, None), "detail");
    }

    #[test]
    fn non_empty_helper() {
        assert!(!non_empty(None));
        assert!(!non_empty(Some("")));
        assert!(non_empty(Some("x")));
    }

    #[test]
    fn cost_col_maps_display_currency() {
        // 展示币种白名单已由调用方校验（CNY|USD）；此处仅验证列名映射。
        assert_eq!(cost_col("CNY"), "cost_cny");
        assert_eq!(cost_col("USD"), "cost_usd");
        // 非白名单值默认回退 cost_cny（防御性，正常路径不会出现）。
        assert_eq!(cost_col("EUR"), "cost_cny");
    }

    #[test]
    fn cache_hit_rate_input_side() {
        // 输入侧口径：cache_read / (prompt + cache_read + cache_write)
        // prompt=5（未缓存）、cache_read=9、cache_write=6 → 9/20 = 0.45
        let r = cache_hit_rate(9, 5, 6).expect("有输入");
        assert!((r - 0.45).abs() < 1e-9);
        // 全命中：prompt=0、cache_read=10、cache_write=0 → 1.0
        assert_eq!(cache_hit_rate(10, 0, 0), Some(1.0));
        // 无命中但有输入 → 0.0（不是 None）
        assert_eq!(cache_hit_rate(0, 10, 0), Some(0.0));
    }

    #[test]
    fn cache_hit_rate_zero_denominator_is_none() {
        // 分母 0（无任何输入 token）→ None，展示 `-` 而非 0%。
        assert_eq!(cache_hit_rate(0, 0, 0), None);
        // 负值防御（不应出现）：不 panic、不产生 >1 的比率。
        assert_eq!(cache_hit_rate(-3, 0, 0), None);
    }

    #[test]
    fn cache_hit_rate_negative_dirty_data_clamped_to_unit_interval() {
        // 脏数据：prompt 为负会让分母小于分子 → 此前算出 2.0（前端显示 200%）。
        // 各分量钳 0 后：cr=10、分母=0+10+0 → 1.0（上界）。
        assert_eq!(cache_hit_rate(10, -5, 0), Some(1.0));
        // cache_write 为负同理不影响分母下界。
        let r = cache_hit_rate(4, 6, -100).expect("有输入");
        assert!((r - 0.4).abs() < 1e-9);
    }
}
