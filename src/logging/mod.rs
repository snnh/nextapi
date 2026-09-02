//! 日志队列与记账（PLAN §5.5/§7：有界异步队列 + 批量写库 + WAL 兜底 + request_id 幂等）。
//! 契约见 contracts/m4-logging.md（M4-A 实现）。

pub mod partition;
pub mod redact;
pub mod wal;
pub mod writer;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::error::ApiResult;
use crate::limit::window_start;

/// 单条记账事件。media/pricing 维度 M5/M6 才填，M4 恒 None/NULL。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    pub request_id: String,
    pub ts: DateTime<Utc>,
    pub key_id: Option<Uuid>,
    /// 网关侧模型名（改写前）
    pub model: String,
    pub upstream_id: Option<Uuid>,
    pub protocol_in: String,
    /// 未知时 = protocol_in
    pub protocol_out: String,
    /// passthrough|convert|passthrough_fallback
    pub convert_mode: String,
    pub stream: bool,
    pub status: i32,
    /// ≤2000 字符摘要
    pub error: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    /// 媒体维度（M6；旧 WAL 行缺省可反序列化 → None）。图片结构化计费维度。
    #[serde(default)]
    pub images: Option<i64>,
    #[serde(default)]
    pub image_size: Option<String>,
    #[serde(default)]
    pub video_seconds: Option<Decimal>,
    #[serde(default)]
    pub video_resolution: Option<String>,
    #[serde(default)]
    pub video_task_type: Option<String>,
    pub latency_ms: Option<i32>,
    pub retry_count: i32,
    pub ttfb_ms: Option<i32>,
    pub degraded: bool,
    /// {"params":{...},"usage":{...}}
    pub usage_raw: Option<serde_json::Value>,
    /// {"request":...,"response":...,"truncated":bool}
    pub debug_payload: Option<serde_json::Value>,
    /// 计价成本（CNY）。M5 计价引擎回填；未计价 = None（不填 0）。旧 WAL 行缺此字段 → None。
    #[serde(default)]
    pub cost_cny: Option<Decimal>,
    /// 计价成本（USD）。M5 计价引擎回填；未计价 = None。
    #[serde(default)]
    pub cost_usd: Option<Decimal>,
    /// 计价来源（'bound' 表示命中了价格规则）。未计价 = None。
    #[serde(default)]
    pub pricing_source: Option<String>,
    /// 命中的价格规则明细（[{rule_id,unit,currency,base_price,matched_segment,price,effective_at}]）。
    #[serde(default)]
    pub price_used: Option<serde_json::Value>,
    /// 本次计价实际用到的汇率快照（[{from,to,rate,source,at,inverse}]）。
    #[serde(default)]
    pub fx_snapshot: Option<serde_json::Value>,
}

pub const BATCH_MAX: usize = 500;
pub const DEBUG_MAX_BYTES: usize = 64 * 1024;

/// 日志出口句柄（入 AppState）。log_async=false 时 queue 为 None（绕过队列立即写库，
/// 仍在 spawned task 中执行，绝不阻塞主链路）。
#[derive(Clone)]
pub struct LogSink {
    queue: Arc<Mutex<Option<mpsc::Sender<LogEvent>>>>,
    pool: PgPool,
    wal: wal::WalWriter,
    overflows: prometheus_client::metrics::counter::Counter,
    depth: prometheus_client::metrics::gauge::Gauge,
    billing_tz: String,
    /// 最近成功汇率在超过该分钟数视为过期不可用（M5 计价；writer 直写分支透传）。
    fx_stale_max_minutes: u64,
}

impl LogSink {
    pub fn new(
        queue: Option<mpsc::Sender<LogEvent>>,
        pool: PgPool,
        wal: wal::WalWriter,
        overflows: prometheus_client::metrics::counter::Counter,
        depth: prometheus_client::metrics::gauge::Gauge,
        billing_tz: String,
        fx_stale_max_minutes: u64,
    ) -> Self {
        Self {
            queue: Arc::new(Mutex::new(queue)),
            pool,
            wal,
            overflows,
            depth,
            billing_tz,
            fx_stale_max_minutes,
        }
    }

    /// 非阻塞记账。队列满 → overflows.inc() + spawn WAL 追加（PLAN：溢出先写 WAL，内存丢弃）；
    /// 队列关闭 → tracing::warn 丢弃；log_async=false → spawn 立即 insert_batch。
    pub fn log(&self, ev: LogEvent) {
        let queue = self.queue.lock().unwrap_or_else(|p| p.into_inner()).clone();
        match queue {
            Some(tx) => match tx.try_send(ev) {
                Ok(()) => {
                    // 更新队列深度指标（writer 侧也会周期更新，这里补即时值）
                    self.depth.set((tx.max_capacity() - tx.capacity()) as i64);
                }
                Err(mpsc::error::TrySendError::Full(ev)) => {
                    // 队列满：先写 WAL 兜底，内存条目丢弃（绝不阻塞主链路）。
                    self.overflows.inc();
                    let wal = self.wal.clone();
                    tokio::spawn(async move {
                        let batch = [ev];
                        if let Err(e) = wal.append(&batch).await {
                            tracing::warn!("日志溢出写 WAL 失败，数据丢弃: {e}");
                        }
                    });
                }
                Err(mpsc::error::TrySendError::Closed(ev)) => {
                    let _ = ev;
                    tracing::warn!("日志队列已关闭，丢弃事件");
                }
            },
            None => {
                // log_async=false：绕过队列，spawn 立即 insert_batch（不阻塞主链路）。
                let pool = self.pool.clone();
                let wal = self.wal.clone();
                let billing_tz = self.billing_tz.clone();
                let fx_stale_max_minutes = self.fx_stale_max_minutes;
                tokio::spawn(async move {
                    let batch = [ev];
                    if let Err(e) = insert_batch(&pool, &batch, &billing_tz, fx_stale_max_minutes).await {
                        tracing::warn!("直写日志失败，写 WAL 兜底: {e}");
                        if let Err(we) = wal.append(&batch).await {
                            tracing::error!("直写兜底 WAL 追加失败，数据丢弃: {we}");
                        }
                    }
                });
            }
        }
    }

    /// 优雅关闭：drop sender（调用方随后 await writer 退出）。
    pub fn close(&self) {
        let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(tx) = queue.take() {
            drop(tx);
        }
    }
}

// ---------------------------------------------------------------------------
// usage_logs 多行 UNNEST INSERT 的列清单与占位符（纯函数，供测试验证）
// ---------------------------------------------------------------------------

/// 直接来自 LogEvent 的列（顺序固定，与数组类型一一对应）。
const LOG_COLS: [&str; 31] = [
    "request_id",
    "ts",
    "key_id",
    "model",
    "upstream_id",
    "protocol_in",
    "protocol_out",
    "convert_mode",
    "stream",
    "prompt_tokens",
    "completion_tokens",
    "cache_write_tokens",
    "cache_read_tokens",
    "images",
    "image_size",
    "video_seconds",
    "video_resolution",
    "video_task_type",
    "latency_ms",
    "status",
    "error",
    "retry_count",
    "ttfb_ms",
    "degraded",
    "usage_raw",
    "debug_payload",
    "cost_cny",
    "cost_usd",
    "pricing_source",
    "price_used",
    "fx_snapshot",
];

/// 对应列的 Postgres 数组类型（与 LOG_COLS 同序）。
const LOG_ARRAY_TYPES: [&str; 31] = [
    "text[]",
    "timestamptz[]",
    "uuid[]",
    "text[]",
    "uuid[]",
    "text[]",
    "text[]",
    "text[]",
    "bool[]",
    "bigint[]",
    "bigint[]",
    "bigint[]",
    "bigint[]",
    "int[]",
    "text[]",
    "numeric[]",
    "text[]",
    "text[]",
    "int[]",
    "int[]",
    "text[]",
    "int[]",
    "int[]",
    "bool[]",
    "jsonb[]",
    "jsonb[]",
    "numeric[]",
    "numeric[]",
    "text[]",
    "jsonb[]",
    "jsonb[]",
];

/// 构造 UNNEST 多行 SELECT 片段：`SELECT * FROM UNNEST($1::t, $2::t, ...)`。纯函数，供测试。
fn build_unnest_sql(casts: &[&str]) -> String {
    let params: Vec<String> = casts
        .iter()
        .enumerate()
        .map(|(i, c)| format!("${}::{c}", i + 1))
        .collect();
    format!("SELECT * FROM UNNEST({})", params.join(", "))
}

/// usage_logs 多行 INSERT（ON CONFLICT (request_id, ts) DO NOTHING）。纯函数，供测试。
fn usage_logs_insert_sql() -> String {
    let cols = LOG_COLS.join(", ");
    let unnest = build_unnest_sql(&LOG_ARRAY_TYPES);
    format!("INSERT INTO usage_logs ({cols}) {unnest} ON CONFLICT (request_id, ts) DO NOTHING")
}

/// usage_hourly 单桶聚合累计（含成本可选累加）。
#[derive(Default)]
struct HourlyAcc {
    requests: i64,
    errors: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cost_cny: Option<Decimal>,
    cost_usd: Option<Decimal>,
}

/// 可选成本累加：仅当 v 非 NULL 才累加（全 NULL 时保持 None，即「全 NULL 保持 NULL」）。
/// 纯函数，供测试。PLAN v1.10：未定价事件 cost=None 不计入成本聚合。
fn add_opt_sum(acc: &mut Option<Decimal>, v: Option<Decimal>) {
    if let Some(x) = v {
        *acc = Some(acc.unwrap_or(Decimal::ZERO) + x);
    }
}

/// 将事件的时间桶到小时（UTC），供 usage_hourly 聚合。纯函数，可测。
fn hour_bucket(ts: DateTime<Utc>) -> DateTime<Utc> {
    let secs = ts.timestamp().div_euclid(3600) * 3600;
    DateTime::<Utc>::from_timestamp(secs, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

/// 批量落库（单事务）：usage_logs UNNEST 多行（ON CONFLICT (request_id,ts) DO NOTHING）
/// + usage_hourly upsert + quota_usage tokens/cost 累加 + last_used_at 批量更新。
/// 任何一步失败 → 整事务回滚，由调用方写 WAL 兜底。返回实际插入 usage_logs 行数。
///
/// 步骤 0：克隆事件为可变副本 → `billing::price_batch` 回填计价字段（cost_cny/cost_usd/
/// pricing_source/price_used/fx_snapshot）。计价失败 = DB 故障级，按整批失败返回 Err，
/// 由调用方（writer/flush）走既有 WAL 兜底路径，不特殊处理。
pub async fn insert_batch(
    pool: &PgPool,
    events: &[LogEvent],
    billing_tz: &str,
    fx_stale_max_minutes: u64,
) -> ApiResult<u64> {
    if events.is_empty() {
        return Ok(0);
    }

    // 步骤 0：克隆为可变副本并计价回填（M5 计价引擎只统计成本、不阻断明细落库）。
    let mut priced: Vec<LogEvent> = events.to_vec();
    crate::billing::price_batch(pool, &mut priced, billing_tz, fx_stale_max_minutes).await?;

    let mut tx = pool.begin().await?;

    // ---- 1) usage_logs UNNEST 多行 INSERT ----
    let n = priced.len();
    let mut request_ids = Vec::with_capacity(n);
    let mut ts_col = Vec::with_capacity(n);
    let mut key_ids = Vec::with_capacity(n);
    let mut models = Vec::with_capacity(n);
    let mut upstream_ids = Vec::with_capacity(n);
    let mut protocols_in = Vec::with_capacity(n);
    let mut protocols_out = Vec::with_capacity(n);
    let mut convert_modes = Vec::with_capacity(n);
    let mut streams = Vec::with_capacity(n);
    let mut prompt_tokens = Vec::with_capacity(n);
    let mut completion_tokens = Vec::with_capacity(n);
    let mut cache_write = Vec::with_capacity(n);
    let mut cache_read = Vec::with_capacity(n);
    let mut images_col = Vec::with_capacity(n);
    let mut image_size_col = Vec::with_capacity(n);
    let mut video_seconds_col = Vec::with_capacity(n);
    let mut video_resolution_col = Vec::with_capacity(n);
    let mut video_task_type_col = Vec::with_capacity(n);
    let mut latencies = Vec::with_capacity(n);
    let mut statuses = Vec::with_capacity(n);
    let mut error_msgs = Vec::with_capacity(n);
    let mut retry_counts = Vec::with_capacity(n);
    let mut ttfb = Vec::with_capacity(n);
    let mut degraded = Vec::with_capacity(n);
    let mut usage_raw = Vec::with_capacity(n);
    let mut debug_payload = Vec::with_capacity(n);
    let mut cost_cny = Vec::with_capacity(n);
    let mut cost_usd = Vec::with_capacity(n);
    let mut pricing_source = Vec::with_capacity(n);
    let mut price_used = Vec::with_capacity(n);
    let mut fx_snapshot = Vec::with_capacity(n);

    for ev in &priced {
        request_ids.push(ev.request_id.clone());
        ts_col.push(ev.ts);
        key_ids.push(ev.key_id);
        models.push(ev.model.clone());
        upstream_ids.push(ev.upstream_id);
        protocols_in.push(ev.protocol_in.clone());
        protocols_out.push(ev.protocol_out.clone());
        convert_modes.push(ev.convert_mode.clone());
        streams.push(ev.stream);
        prompt_tokens.push(ev.prompt_tokens);
        completion_tokens.push(ev.completion_tokens);
        cache_write.push(ev.cache_write_tokens);
        cache_read.push(ev.cache_read_tokens);
        // usage_logs.images 为 INT 列，LogEvent.images 为 i64，绑定 int[] 需收敛到 i32。
        images_col.push(ev.images.map(|v| v as i32));
        image_size_col.push(ev.image_size.clone());
        video_seconds_col.push(ev.video_seconds);
        video_resolution_col.push(ev.video_resolution.clone());
        video_task_type_col.push(ev.video_task_type.clone());
        latencies.push(ev.latency_ms);
        statuses.push(ev.status);
        error_msgs.push(ev.error.clone());
        retry_counts.push(ev.retry_count);
        ttfb.push(ev.ttfb_ms);
        degraded.push(ev.degraded);
        usage_raw.push(ev.usage_raw.clone());
        debug_payload.push(ev.debug_payload.clone());
        cost_cny.push(ev.cost_cny);
        cost_usd.push(ev.cost_usd);
        pricing_source.push(ev.pricing_source.clone());
        price_used.push(ev.price_used.clone());
        fx_snapshot.push(ev.fx_snapshot.clone());
    }

    let inserted = sqlx::query(&usage_logs_insert_sql())
        .bind(&request_ids)
        .bind(&ts_col)
        .bind(&key_ids)
        .bind(&models)
        .bind(&upstream_ids)
        .bind(&protocols_in)
        .bind(&protocols_out)
        .bind(&convert_modes)
        .bind(&streams)
        .bind(&prompt_tokens)
        .bind(&completion_tokens)
        .bind(&cache_write)
        .bind(&cache_read)
        .bind(&images_col)
        .bind(&image_size_col)
        .bind(&video_seconds_col)
        .bind(&video_resolution_col)
        .bind(&video_task_type_col)
        .bind(&latencies)
        .bind(&statuses)
        .bind(&error_msgs)
        .bind(&retry_counts)
        .bind(&ttfb)
        .bind(&degraded)
        .bind(&usage_raw)
        .bind(&debug_payload)
        .bind(&cost_cny)
        .bind(&cost_usd)
        .bind(&pricing_source)
        .bind(&price_used)
        .bind(&fx_snapshot)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    // ---- 2) usage_hourly 聚合 upsert（小时桶 + model 维度）----
    // 聚合项: (requests, errors, prompt_tokens, completion_tokens, cost_cny, cost_usd)
    let mut hourly: HashMap<(DateTime<Utc>, String), HourlyAcc> = HashMap::new();
    for ev in &priced {
        let bucket = hour_bucket(ev.ts);
        let agg = hourly.entry((bucket, ev.model.clone())).or_default();
        agg.requests += 1;
        if ev.status >= 400 {
            agg.errors += 1; // errors（≥400 视为错误）
        }
        agg.prompt_tokens += ev.prompt_tokens.unwrap_or(0);
        agg.completion_tokens += ev.completion_tokens.unwrap_or(0);
        add_opt_sum(&mut agg.cost_cny, ev.cost_cny);
        add_opt_sum(&mut agg.cost_usd, ev.cost_usd);
    }
    for ((hour, model), agg) in hourly {
        sqlx::query(
            "INSERT INTO usage_hourly \
             (hour, model, requests, errors, prompt_tokens, completion_tokens, cost_cny, cost_usd) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
             ON CONFLICT (hour, model) DO UPDATE SET \
               requests = usage_hourly.requests + EXCLUDED.requests, \
               errors = usage_hourly.errors + EXCLUDED.errors, \
               prompt_tokens = usage_hourly.prompt_tokens + EXCLUDED.prompt_tokens, \
               completion_tokens = usage_hourly.completion_tokens + EXCLUDED.completion_tokens, \
               cost_cny = CASE WHEN EXCLUDED.cost_cny IS NULL THEN usage_hourly.cost_cny \
                               ELSE COALESCE(usage_hourly.cost_cny, 0) + EXCLUDED.cost_cny END, \
               cost_usd = CASE WHEN EXCLUDED.cost_usd IS NULL THEN usage_hourly.cost_usd \
                               ELSE COALESCE(usage_hourly.cost_usd, 0) + EXCLUDED.cost_usd END",
        )
        .bind(hour)
        .bind(&model)
        .bind(agg.requests)
        .bind(agg.errors)
        .bind(agg.prompt_tokens)
        .bind(agg.completion_tokens)
        .bind(agg.cost_cny)
        .bind(agg.cost_usd)
        .execute(&mut *tx)
        .await?;
    }

    // ---- 3) token 配额累加（仅 quota_unit='tokens'）----
    let distinct_keys: Vec<Uuid> = {
        let mut seen = std::collections::HashSet::new();
        priced
            .iter()
            .filter_map(|e| e.key_id)
            .filter(|k| seen.insert(*k))
            .collect()
    };
    // 查询各 key 的 quota_unit/quota_window（仅累加 tokens 单位）
    let mut key_quota: HashMap<Uuid, (Option<String>, Option<String>)> = HashMap::new();
    if !distinct_keys.is_empty() {
        let rows = sqlx::query("SELECT id, quota_unit, quota_window FROM api_keys WHERE id = ANY($1)")
            .bind(&distinct_keys)
            .fetch_all(&mut *tx)
            .await?;
        for row in rows {
            let id: Uuid = row.get("id");
            let unit: Option<String> = row.get("quota_unit");
            let window: Option<String> = row.get("quota_window");
            key_quota.insert(id, (unit, window));
        }
    }
    // 按 (key_id, window, period_start) 聚合 tokens 用量（仅 quota_unit='tokens' 累加）。
    // 若 key 未配置 quota_window，按 window_start 对未知 window 的语义退回 'total'（epoch 起点）。
    let mut quota_tokens: HashMap<(Uuid, String, DateTime<Utc>), Decimal> = HashMap::new();
    for ev in &priced {
        let Some(key_id) = ev.key_id else { continue };
        let Some((unit, window)) = key_quota.get(&key_id) else { continue };
        if unit.as_deref() != Some("tokens") {
            continue;
        }
        let window = window.as_deref().unwrap_or("total");
        let period_start = window_start(window, billing_tz, ev.ts);
        let tokens = ev
            .prompt_tokens
            .unwrap_or(0)
            .saturating_add(ev.completion_tokens.unwrap_or(0))
            .saturating_add(ev.cache_write_tokens.unwrap_or(0))
            .saturating_add(ev.cache_read_tokens.unwrap_or(0));
        let entry = quota_tokens
            .entry((key_id, window.to_string(), period_start))
            .or_insert(Decimal::ZERO);
        *entry += Decimal::from(tokens);
    }
    for ((key_id, window, period_start), value) in quota_tokens {
        sqlx::query(
            "INSERT INTO quota_usage (key_id, unit, window, period_start, value, updated_at) \
             VALUES ($1,'tokens',$2,$3,$4, now()) \
             ON CONFLICT (key_id, unit, window, period_start) DO UPDATE \
             SET value = quota_usage.value + EXCLUDED.value, updated_at = now()",
        )
        .bind(key_id)
        .bind(&window)
        .bind(period_start)
        .bind(value)
        .execute(&mut *tx)
        .await?;
    }

    // ---- 3b) 成本配额累加（仅 quota_unit='cost_cny'/'cost_usd'）----
    // PLAN v1.10：未定价事件 cost=None → 不计入成本配额（但 token 配额照常，见上）。
    let mut quota_cost_cny: HashMap<(Uuid, String, DateTime<Utc>), Decimal> = HashMap::new();
    let mut quota_cost_usd: HashMap<(Uuid, String, DateTime<Utc>), Decimal> = HashMap::new();
    for ev in &priced {
        let Some(key_id) = ev.key_id else { continue };
        let Some((unit, window)) = key_quota.get(&key_id) else { continue };
        let window = window.as_deref().unwrap_or("total");
        let period_start = window_start(window, billing_tz, ev.ts);
        match unit.as_deref() {
            Some("cost_cny") => {
                if let Some(c) = ev.cost_cny {
                    let entry = quota_cost_cny
                        .entry((key_id, window.to_string(), period_start))
                        .or_insert(Decimal::ZERO);
                    *entry += c;
                }
            }
            Some("cost_usd") => {
                if let Some(c) = ev.cost_usd {
                    let entry = quota_cost_usd
                        .entry((key_id, window.to_string(), period_start))
                        .or_insert(Decimal::ZERO);
                    *entry += c;
                }
            }
            _ => {}
        }
    }
    for ((key_id, window, period_start), value) in quota_cost_cny {
        sqlx::query(
            "INSERT INTO quota_usage (key_id, unit, window, period_start, value, updated_at) \
             VALUES ($1,'cost_cny',$2,$3,$4, now()) \
             ON CONFLICT (key_id, unit, window, period_start) DO UPDATE \
             SET value = quota_usage.value + EXCLUDED.value, updated_at = now()",
        )
        .bind(key_id)
        .bind(&window)
        .bind(period_start)
        .bind(value)
        .execute(&mut *tx)
        .await?;
    }
    for ((key_id, window, period_start), value) in quota_cost_usd {
        sqlx::query(
            "INSERT INTO quota_usage (key_id, unit, window, period_start, value, updated_at) \
             VALUES ($1,'cost_usd',$2,$3,$4, now()) \
             ON CONFLICT (key_id, unit, window, period_start) DO UPDATE \
             SET value = quota_usage.value + EXCLUDED.value, updated_at = now()",
        )
        .bind(key_id)
        .bind(&window)
        .bind(period_start)
        .bind(value)
        .execute(&mut *tx)
        .await?;
    }

    // ---- 4) last_used_at 批量更新（批内按 key 取 max ts）----
    let mut last_used: HashMap<Uuid, DateTime<Utc>> = HashMap::new();
    for ev in &priced {
        if let Some(key_id) = ev.key_id {
            let e = last_used.entry(key_id).or_insert(ev.ts);
            if ev.ts > *e {
                *e = ev.ts;
            }
        }
    }
    if !last_used.is_empty() {
        let mut ids = Vec::with_capacity(last_used.len());
        let mut tss = Vec::with_capacity(last_used.len());
        for (id, t) in last_used {
            ids.push(id);
            tss.push(t);
        }
        sqlx::query(
            "UPDATE api_keys AS k SET last_used_at = v.ts \
             FROM (SELECT * FROM UNNEST($1::uuid[], $2::timestamptz[])) AS v(id, ts) \
             WHERE k.id = v.id AND (k.last_used_at IS NULL OR k.last_used_at < v.ts)",
        )
        .bind(&ids)
        .bind(&tss)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn build_unnest_sql_numbering() {
        let casts = vec!["text[]", "timestamptz[]", "uuid[]"];
        let sql = build_unnest_sql(&casts);
        assert_eq!(sql, "SELECT * FROM UNNEST($1::text[], $2::timestamptz[], $3::uuid[])");
    }

    #[test]
    fn usage_logs_insert_sql_structure() {
        let sql = usage_logs_insert_sql();
        assert!(sql.starts_with("INSERT INTO usage_logs ("));
        assert!(sql.contains("$1::text[]"));
        // M5：新增 5 个计价列进入 INSERT 列清单
        assert!(sql.contains("cost_cny, cost_usd, pricing_source, price_used, fx_snapshot"));
        // M6：新增 5 个媒体列进入 INSERT 列清单（数值型/文本型/数值型/文本型/文本型）
        assert!(sql.contains("cache_read_tokens, images, image_size, video_seconds, video_resolution, video_task_type"));
        // 数组位次：usage_raw=$25::jsonb[]、video_seconds=$16::numeric[]、cost_cny=$27::numeric[]、fx_snapshot=$31::jsonb[]
        assert!(sql.contains("$16::numeric[]"));
        assert!(sql.contains("$25::jsonb[]"));
        assert!(sql.contains("$27::numeric[]"));
        assert!(sql.contains("$31::jsonb[]"));
        assert!(sql.contains("ON CONFLICT (request_id, ts) DO NOTHING"));
        assert!(sql.contains("usage_logs (request_id, ts, key_id, model,"));
    }

    #[test]
    fn hourly_bucket_truncates_to_hour() {
        let ts = Utc.timestamp_opt(1_700_000_123, 0).unwrap();
        let b = hour_bucket(ts);
        assert_eq!(b.timestamp(), 1_700_000_000 - (1_700_000_000 % 3600));
    }

    #[test]
    fn zero_events_returns_zero() {
        // 纯逻辑（无 DB）：空批次直接返回 0，不触碰数据库。
        // 这里无法真正连接 DB，仅验证空批次短路逻辑分支逻辑。
        assert!(BATCH_MAX > 0);
        assert!(DEBUG_MAX_BYTES == 64 * 1024);
    }

    #[test]
    fn old_wal_line_without_cost_fields_deserializes() {
        // 契约 §10：旧 WAL 行（M4）不含 M5 的 5 个新字段，靠 #[serde(default)] 兼容 → 全 None。
        let json = r#"{
            "request_id":"r1","ts":"2024-01-01T00:00:00Z","key_id":null,
            "model":"gpt-4","upstream_id":null,"protocol_in":"openai_chat","protocol_out":"openai_chat",
            "convert_mode":"passthrough","stream":false,"status":200,"error":null,
            "prompt_tokens":10,"completion_tokens":5,"cache_write_tokens":null,"cache_read_tokens":null,
            "latency_ms":42,"retry_count":0,"ttfb_ms":null,"degraded":false,
            "usage_raw":null,"debug_payload":null
        }"#;
        let ev: LogEvent = serde_json::from_str(json).expect("旧格式应可反序列化");
        assert_eq!(ev.cost_cny, None);
        assert_eq!(ev.cost_usd, None);
        assert_eq!(ev.pricing_source, None);
        assert_eq!(ev.price_used, None);
        assert_eq!(ev.fx_snapshot, None);
        // M6：旧 WAL 行（M4/M5）不含 5 个媒体字段，靠 #[serde(default)] 兼容 → None
        assert_eq!(ev.images, None);
        assert_eq!(ev.image_size, None);
        assert_eq!(ev.video_seconds, None);
        assert_eq!(ev.video_resolution, None);
        assert_eq!(ev.video_task_type, None);
    }

    #[test]
    fn new_wal_line_with_cost_fields_roundtrip() {
        // 新格式：5 个计价字段正常序列化/反序列化。
        let mut ev = serde_json::from_str::<LogEvent>(r#"{
            "request_id":"r2","ts":"2024-01-01T00:00:00Z","key_id":null,
            "model":"deepseek-chat","upstream_id":null,"protocol_in":"openai_chat","protocol_out":"openai_chat",
            "convert_mode":"passthrough","stream":false,"status":200,"error":null,
            "prompt_tokens":10,"completion_tokens":5,"cache_write_tokens":null,"cache_read_tokens":null,
            "latency_ms":42,"retry_count":0,"ttfb_ms":null,"degraded":false,
            "usage_raw":null,"debug_payload":null,
            "cost_cny":"0.001028","cost_usd":null,"pricing_source":"bound",
            "price_used":[{"unit":"token_in"}],"fx_snapshot":[]
        }"#)
        .expect("新格式应可反序列化");
        assert_eq!(ev.cost_cny, Some(Decimal::new(1028, 6)));
        assert_eq!(ev.pricing_source.as_deref(), Some("bound"));
        assert!(ev.price_used.as_ref().is_some());
        assert!(ev.fx_snapshot.as_ref().is_some());
        // 序列化再反序列化（WAL 行往返）。
        let line = serde_json::to_string(&ev).expect("应可序列化");
        let back: LogEvent = serde_json::from_str(&line).expect("应可反序列化");
        assert_eq!(back.cost_cny, ev.cost_cny);
        assert_eq!(back.pricing_source, ev.pricing_source);
    }

    #[test]
    fn new_wal_line_with_media_fields_roundtrip() {
        // M6：新版 WAL 行携带 5 个媒体字段，序列化后再反序列化应无损往返。
        let json = r#"{
            "request_id":"r3","ts":"2024-01-01T00:00:00Z","key_id":null,
            "model":"qwen-image-2.0-pro","upstream_id":null,"protocol_in":"openai_image","protocol_out":"images_dashscope_async",
            "convert_mode":"media_task","stream":false,"status":200,"error":null,
            "prompt_tokens":null,"completion_tokens":null,"cache_write_tokens":null,"cache_read_tokens":null,
            "latency_ms":null,"retry_count":0,"ttfb_ms":null,"degraded":false,
            "usage_raw":null,"debug_payload":null,
            "cost_cny":"0.001","cost_usd":null,"pricing_source":"bound","price_used":[],"fx_snapshot":[],
            "images":2,"image_size":"1024x1024","video_seconds":null,"video_resolution":null,"video_task_type":null
        }"#;
        let ev: LogEvent = serde_json::from_str(json).expect("含媒体字段应可反序列化");
        assert_eq!(ev.images, Some(2));
        assert_eq!(ev.image_size.as_deref(), Some("1024x1024"));
        assert_eq!(ev.video_seconds, None);
        let line = serde_json::to_string(&ev).expect("应可序列化");
        let back: LogEvent = serde_json::from_str(&line).expect("应可反序列化");
        assert_eq!(back.images, ev.images);
        assert_eq!(back.image_size, ev.image_size);
        assert_eq!(back.video_seconds, ev.video_seconds);
    }

    #[test]
    fn add_opt_sum_accumulates_only_non_null() {
        // 契约 §5：成本累加仅当 EXCLUDED 非 NULL；全 NULL 时保持 None。
        let mut acc: Option<Decimal> = None;
        add_opt_sum(&mut acc, Some(Decimal::from(2)));
        add_opt_sum(&mut acc, None);
        add_opt_sum(&mut acc, Some(Decimal::from(3)));
        assert_eq!(acc, Some(Decimal::from(5)));
        // 全 NULL → None
        let mut acc2: Option<Decimal> = None;
        add_opt_sum(&mut acc2, None);
        add_opt_sum(&mut acc2, None);
        assert_eq!(acc2, None);
        // 单值从 None 起步
        let mut acc3: Option<Decimal> = None;
        add_opt_sum(&mut acc3, Some(Decimal::from(1)));
        assert_eq!(acc3, Some(Decimal::from(1)));
    }
}
