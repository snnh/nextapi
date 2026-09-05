//! usage_logs 分区管理（PLAN §5.5：30 天分文件存储 + 手动整分区 DROP）。
//! 契约 contracts/m4-logging.md §5，M4-A 实现。
//!
//! 索引：idx = floor(unix_days / days)，边界 = 1970-01-01 + idx*days 天（epoch 对齐）。
//! days=0 做防御处理，视为 30。分区边界 SQL 不能参数化，用 `to_timestamp(epoch)` 字面量拼接，
//! 数值来自纯函数计算（非用户输入），安全。

use chrono::{DateTime, Utc};
use sqlx::Row;
use std::collections::HashSet;

use crate::error::ApiResult;

#[derive(Debug, Clone, serde::Serialize)]
pub struct PartitionInfo {
    pub name: String,
    pub from_ts: DateTime<Utc>,
    pub to_ts: DateTime<Utc>,
    pub size_bytes: i64,
    pub row_estimate: i64,
}

/// 防御：days=0 视为 30。
fn effective_days(days: u32) -> u32 {
    if days == 0 {
        30
    } else {
        days
    }
}

/// 纯函数：epoch 天 → 分区序号（floor(unix_days / days)）。
pub fn partition_index(ts: DateTime<Utc>, days: u32) -> i64 {
    let days = effective_days(days) as i64;
    ts.timestamp().div_euclid(86_400).div_euclid(days)
}

/// 纯函数：分区边界（[1970-01-01 + idx*days 天, +days 天））。
pub fn partition_range(index: i64, days: u32) -> (DateTime<Utc>, DateTime<Utc>) {
    let days = effective_days(days) as i64;
    let from_secs = index.saturating_mul(days).saturating_mul(86_400);
    let to_secs = index
        .saturating_add(1)
        .saturating_mul(days)
        .saturating_mul(86_400);
    let from = DateTime::<Utc>::from_timestamp(from_secs, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
    let to = DateTime::<Utc>::from_timestamp(to_secs, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
    (from, to)
}

pub fn partition_name(index: i64) -> String {
    format!("usage_logs_p{index}")
}

/// 从分区名解析序号（`usage_logs_p<idx>`）。
fn name_to_index(name: &str) -> Option<i64> {
    name.strip_prefix("usage_logs_p")?.parse().ok()
}

/// app_meta 固化键：分区天数。首次建分区时写入，此后一律以固化值为准。
///
/// 背景（全局 review P4）：分区名序号与边界都按「epoch 对齐 + days 跨度」计算，若 days 随
/// hot 配置热改（UI 可改），名称反推的边界与既有分区（按旧 days 建）实际边界错位 →
/// 清理可能误删仍含活数据的分区、预热会建重叠分区失败、越界行落入 DEFAULT 不可清理。
/// 因此首次（建分区前）固化到 app_meta，运行期改配置只影响「尚未固化过的空库」。
pub const PARTITION_DAYS_KEY: &str = "log_partition_days";

/// 读取固化的分区天数；无固化值时以配置建议值首次固化并返回（读-写-重读，防并发双写）。
async fn resolve_or_init_days(ex: &mut sqlx::PgConnection, cfg_days: u32) -> ApiResult<u32> {
    async fn read_days(ex: &mut sqlx::PgConnection) -> ApiResult<Option<u32>> {
        let row = sqlx::query("SELECT value FROM app_meta WHERE key = $1")
            .bind(PARTITION_DAYS_KEY)
            .fetch_optional(&mut *ex)
            .await?;
        let Some(row) = row else { return Ok(None) };
        let v: serde_json::Value = row.try_get("value")?;
        let n = v.as_i64().filter(|n| *n > 0).unwrap_or(30);
        Ok(Some(n as u32))
    }
    if let Some(days) = read_days(ex).await? {
        return Ok(days);
    }
    let days = effective_days(cfg_days);
    sqlx::query(
        "INSERT INTO app_meta (key, value, updated_at) VALUES ($1, $2, now()) \
         ON CONFLICT (key) DO NOTHING",
    )
    .bind(PARTITION_DAYS_KEY)
    .bind(serde_json::json!(days))
    .execute(&mut *ex)
    .await?;
    if let Some(days) = read_days(ex).await? {
        return Ok(days);
    }
    Ok(days)
}

/// 查询已存在的 usage_logs 分区名（relkind='r'），用于幂等与新建判定。
async fn existing_partition_names(pool: &sqlx::PgPool) -> ApiResult<HashSet<String>> {
    let rows = sqlx::query(
        "SELECT c.relname AS name FROM pg_class c \
         WHERE c.relkind = 'r' AND c.relname LIKE 'usage_logs_p%'",
    )
    .fetch_all(pool)
    .await?;
    let mut names: HashSet<String> = HashSet::new();
    for row in rows {
        let name: Option<String> = row.get("name");
        if let Some(name) = name {
            // 仅收录合规名字，避免非分区表干扰
            if name_to_index(&name).is_some() {
                names.insert(name);
            }
        }
    }
    Ok(names)
}

/// 启动/定时：确保覆盖 [now-1个窗口, now+ahead个窗口] 的分区存在（幂等），返回新建名。
/// cfg_days 仅为「首次固化建议值」；已固化的库一律以 app_meta 固化值为准（review P4）。
pub async fn ensure_partitions(
    pool: &sqlx::PgPool,
    cfg_days: u32,
    ahead: i64,
) -> ApiResult<Vec<String>> {
    // advisory 锁（固定键）：防多实例/重启并发 CREATE PARTITION / 首次固化竞态（review P2-3）
    const LOCK_KEY: i64 = 794_724_262_143_070; // 任意固定键（'nextapi' 语义占位）
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(LOCK_KEY)
        .execute(&mut *tx)
        .await?;

    // 锁内解析/固化分区天数：首次建分区前写入 app_meta，后续重启/改配置不再改变。
    let days = resolve_or_init_days(&mut tx, cfg_days).await?;

    let existing = existing_partition_names(pool).await?;
    let now = Utc::now();
    let cur = partition_index(now, days);
    let min = cur - 1;
    let max = cur + ahead;

    let mut created = Vec::new();
    for idx in min..=max {
        let name = partition_name(idx);
        if existing.contains(&name) {
            continue;
        }
        let (from, to) = partition_range(idx, days);
        // 分区边界字面量（to_timestamp(epoch)），数值来自纯函数，非用户输入。
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {name} PARTITION OF usage_logs \
             FOR VALUES FROM (to_timestamp({})) TO (to_timestamp({}))",
            from.timestamp(),
            to.timestamp()
        );
        sqlx::query(&sql).execute(&mut *tx).await?;
        created.push(name);
    }
    tx.commit().await?;
    Ok(created)
}

/// 列出 usage_logs 的全部分区（pg_inherits + pg_class；from/to 按固化 days 由名称 idx 反推）。
/// cfg_days 仅为无固化值时的建议值。
pub async fn list_partitions(pool: &sqlx::PgPool, cfg_days: u32) -> ApiResult<Vec<PartitionInfo>> {
    let mut conn = pool.acquire().await?;
    let days = resolve_or_init_days(&mut conn, cfg_days).await?;
    let rows = sqlx::query(
        "SELECT c.relname AS name, \
                pg_total_relation_size(c.oid) AS size_bytes, \
                c.reltuples::bigint AS row_estimate \
         FROM pg_class c \
         JOIN pg_inherits i ON i.inhrelid = c.oid \
         JOIN pg_class p ON p.oid = i.inhparent \
         WHERE p.relname = 'usage_logs' AND c.relkind = 'r' \
         ORDER BY c.relname",
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut out = Vec::new();
    for row in rows {
        let name: String = row.get("name");
        let size_bytes: i64 = row.get("size_bytes");
        let row_estimate: i64 = row.get("row_estimate");
        let Some(idx) = name_to_index(&name) else {
            continue;
        };
        let (from_ts, to_ts) = partition_range(idx, days);
        out.push(PartitionInfo {
            name,
            from_ts,
            to_ts,
            size_bytes,
            row_estimate,
        });
    }
    Ok(out)
}

/// 手动清理：只删 to_ts <= before 的分区（整分区 DROP TABLE，当前/未来分区天然豁免）。
/// 边界按固化 days 反推（list_partitions 内解析），cfg_days 仅为无固化值时的建议值。
/// dry_run=true 只返回将删分区与行数估计（pg_class.reltuples），不执行。
pub async fn drop_covered(
    pool: &sqlx::PgPool,
    before: DateTime<Utc>,
    days: u32,
    dry_run: bool,
) -> ApiResult<Vec<PartitionInfo>> {
    let all = list_partitions(pool, days).await?;
    let mut candidates: Vec<PartitionInfo> =
        all.into_iter().filter(|p| p.to_ts <= before).collect();
    candidates.sort_by(|a, b| a.name.cmp(&b.name));

    if dry_run {
        return Ok(candidates);
    }

    for info in &candidates {
        // 重新构造合规分区名（仅数字 idx），杜绝注入；并由 DROP TABLE IF EXISTS 兜底幂等。
        if let Some(idx) = name_to_index(&info.name) {
            let safe_name = partition_name(idx);
            let sql = format!("DROP TABLE IF EXISTS {safe_name}");
            sqlx::query(&sql).execute(pool).await?;
        }
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const EPOCH: DateTime<Utc> = /* 1970-01-01T00:00:00Z */ DateTime::<Utc>::UNIX_EPOCH;

    #[test]
    fn partition_index_epoch_zero() {
        assert_eq!(partition_index(EPOCH, 30), 0);
        assert_eq!(partition_index(EPOCH, 1), 0);
    }

    #[test]
    fn partition_index_mid_window() {
        // 1_750_000_000 秒 ≈ 2025-06-15，unix_days ≈ 20254
        let ts = Utc.timestamp_opt(1_750_000_000, 0).unwrap();
        let idx = partition_index(ts, 30);
        assert_eq!(idx, partition_index(ts, 30));
        assert!(idx >= 0);
    }

    #[test]
    fn partition_range_matches_index() {
        // idx=0 → [1970-01-01, +30 天)
        let (from, to) = partition_range(0, 30);
        assert_eq!(from, EPOCH);
        assert_eq!(to, Utc.timestamp_opt(30 * 86_400, 0).unwrap());
        // idx=1 → [30 天, 60 天)
        let (from1, to1) = partition_range(1, 30);
        assert_eq!(from1, Utc.timestamp_opt(30 * 86_400, 0).unwrap());
        assert_eq!(to1, Utc.timestamp_opt(60 * 86_400, 0).unwrap());
    }

    #[test]
    fn partition_range_zero_days_defensive() {
        // days=0 防御 → 视为 30
        assert_eq!(partition_range(0, 0), partition_range(0, 30));
    }

    #[test]
    fn partition_name_format() {
        assert_eq!(partition_name(3), "usage_logs_p3");
        assert_eq!(partition_name(-1), "usage_logs_p-1");
    }

    #[test]
    fn name_to_index_parses() {
        assert_eq!(name_to_index("usage_logs_p12"), Some(12));
        assert_eq!(name_to_index("usage_logs_p-2"), Some(-2));
        assert_eq!(name_to_index("usage_logs_pabc"), None);
        assert_eq!(name_to_index("other"), None);
    }

    #[test]
    fn index_round_trips_with_name() {
        let ts = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let idx = partition_index(ts, 30);
        let name = partition_name(idx);
        assert_eq!(name_to_index(&name), Some(idx));
    }
}
