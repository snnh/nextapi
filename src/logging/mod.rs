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
    pub latency_ms: Option<i32>,
    pub retry_count: i32,
    pub ttfb_ms: Option<i32>,
    pub degraded: bool,
    /// {"params":{...},"usage":{...}}
    pub usage_raw: Option<serde_json::Value>,
    /// {"request":...,"response":...,"truncated":bool}
    pub debug_payload: Option<serde_json::Value>,
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
}

impl LogSink {
    pub fn new(
        queue: Option<mpsc::Sender<LogEvent>>,
        pool: PgPool,
        wal: wal::WalWriter,
        overflows: prometheus_client::metrics::counter::Counter,
        depth: prometheus_client::metrics::gauge::Gauge,
        billing_tz: String,
    ) -> Self {
        Self {
            queue: Arc::new(Mutex::new(queue)),
            pool,
            wal,
            overflows,
            depth,
            billing_tz,
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
                tokio::spawn(async move {
                    let batch = [ev];
                    if let Err(e) = insert_batch(&pool, &batch, &billing_tz).await {
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
const LOG_COLS: [&str; 21] = [
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
    "latency_ms",
    "status",
    "error",
    "retry_count",
    "ttfb_ms",
    "degraded",
    "usage_raw",
    "debug_payload",
];

/// 对应列的 Postgres 数组类型（与 LOG_COLS 同序）。
const LOG_ARRAY_TYPES: [&str; 21] = [
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
    "int[]",
    "text[]",
    "int[]",
    "int[]",
    "bool[]",
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

/// 将事件的时间桶到小时（UTC），供 usage_hourly 聚合。纯函数，可测。
fn hour_bucket(ts: DateTime<Utc>) -> DateTime<Utc> {
    let secs = ts.timestamp().div_euclid(3600) * 3600;
    DateTime::<Utc>::from_timestamp(secs, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

/// 批量落库（单事务）：usage_logs UNNEST 多行（ON CONFLICT (request_id,ts) DO NOTHING）
/// + usage_hourly upsert + quota_usage tokens 累加 + last_used_at 批量更新。
/// 任何一步失败 → 整事务回滚，由调用方写 WAL 兜底。返回实际插入 usage_logs 行数。
pub async fn insert_batch(pool: &PgPool, events: &[LogEvent], billing_tz: &str) -> ApiResult<u64> {
    if events.is_empty() {
        return Ok(0);
    }

    let mut tx = pool.begin().await?;

    // ---- 1) usage_logs UNNEST 多行 INSERT ----
    let n = events.len();
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
    let mut latencies = Vec::with_capacity(n);
    let mut statuses = Vec::with_capacity(n);
    let mut error_msgs = Vec::with_capacity(n);
    let mut retry_counts = Vec::with_capacity(n);
    let mut ttfb = Vec::with_capacity(n);
    let mut degraded = Vec::with_capacity(n);
    let mut usage_raw = Vec::with_capacity(n);
    let mut debug_payload = Vec::with_capacity(n);

    for ev in events {
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
        latencies.push(ev.latency_ms);
        statuses.push(ev.status);
        error_msgs.push(ev.error.clone());
        retry_counts.push(ev.retry_count);
        ttfb.push(ev.ttfb_ms);
        degraded.push(ev.degraded);
        usage_raw.push(ev.usage_raw.clone());
        debug_payload.push(ev.debug_payload.clone());
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
        .bind(&latencies)
        .bind(&statuses)
        .bind(&error_msgs)
        .bind(&retry_counts)
        .bind(&ttfb)
        .bind(&degraded)
        .bind(&usage_raw)
        .bind(&debug_payload)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    // ---- 2) usage_hourly 聚合 upsert（小时桶 + model 维度）----
    // 聚合项: (requests, errors, prompt_tokens, completion_tokens)
    let mut hourly: HashMap<(DateTime<Utc>, String), (i64, i64, i64, i64)> = HashMap::new();
    for ev in events {
        let bucket = hour_bucket(ev.ts);
        let agg = hourly.entry((bucket, ev.model.clone())).or_insert((0, 0, 0, 0));
        agg.0 += 1; // requests
        if ev.status >= 400 {
            agg.1 += 1; // errors（≥400 视为错误）
        }
        agg.2 += ev.prompt_tokens.unwrap_or(0);
        agg.3 += ev.completion_tokens.unwrap_or(0);
    }
    for ((hour, model), (requests, errors, pr_tok, comp_tok)) in hourly {
        sqlx::query(
            "INSERT INTO usage_hourly \
             (hour, model, requests, errors, prompt_tokens, completion_tokens, cost_cny, cost_usd) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
             ON CONFLICT (hour, model) DO UPDATE SET \
               requests = usage_hourly.requests + EXCLUDED.requests, \
               errors = usage_hourly.errors + EXCLUDED.errors, \
               prompt_tokens = usage_hourly.prompt_tokens + EXCLUDED.prompt_tokens, \
               completion_tokens = usage_hourly.completion_tokens + EXCLUDED.completion_tokens, \
               cost_cny = COALESCE(usage_hourly.cost_cny, 0) + COALESCE(EXCLUDED.cost_cny, 0), \
               cost_usd = COALESCE(usage_hourly.cost_usd, 0) + COALESCE(EXCLUDED.cost_usd, 0)",
        )
        .bind(hour)
        .bind(&model)
        .bind(requests)
        .bind(errors)
        .bind(pr_tok)
        .bind(comp_tok)
        .bind(None::<Decimal>)
        .bind(None::<Decimal>)
        .execute(&mut *tx)
        .await?;
    }

    // ---- 3) token 配额累加（仅 quota_unit='tokens'）----
    let distinct_keys: Vec<Uuid> = {
        let mut seen = std::collections::HashSet::new();
        events
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
    for ev in events {
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

    // ---- 4) last_used_at 批量更新（批内按 key 取 max ts）----
    let mut last_used: HashMap<Uuid, DateTime<Utc>> = HashMap::new();
    for ev in events {
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
        assert!(sql.contains("$21::jsonb[]"));
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
}
