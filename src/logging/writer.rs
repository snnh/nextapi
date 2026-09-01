//! 后台批量写者（契约 contracts/m4-logging.md §7，M4-A 实现）。
//!
//! 从 rx 收事件，按 `batch_insert_interval_ms`（每次 flush 前读 hot 配置）或攒满 BATCH_MAX
//! 触发 flush → insert_batch。失败 → wal.append(整批) + 指数退避 1s→30s。
//! 每轮更新 depth gauge（rx.len()）。rx 关闭后排空做最后 flush 再退出。

use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use prometheus_client::metrics::gauge::Gauge;
use sqlx::PgPool;
use tokio::sync::mpsc;

use crate::config::HotConfig;
use crate::logging::{insert_batch, wal::WalWriter, LogEvent, BATCH_MAX};

/// 下次退避值：指数翻倍，上限 30s。纯函数，供测试。
fn next_backoff(cur: u64) -> u64 {
    cur.saturating_mul(2).min(30)
}

/// 尝试写库一批；失败则 WAL 兜底并指数退避。billing_tz 每次从 hot 热读。
async fn flush(
    pool: &PgPool,
    batch: &mut Vec<LogEvent>,
    hot: &Arc<ArcSwap<HotConfig>>,
    wal: &WalWriter,
    backoff: &mut u64,
) {
    if batch.is_empty() {
        return;
    }
    // billing_tz 与 fx_stale_max_minutes 每次从 hot 热读（契约 §5：run_writer 签名不变）。
    let gw = hot.load().gateway.clone();
    let billing_tz = gw.billing_timezone.clone();
    let fx_stale_max_minutes = gw.fx_stale_max_minutes;
    match insert_batch(pool, batch, &billing_tz, fx_stale_max_minutes).await {
        Ok(_) => {
            batch.clear();
            *backoff = 1;
        }
        Err(e) => {
            tracing::warn!("批量写库失败，写 WAL 兜底: {e}");
            if let Err(we) = wal.append(batch).await {
                tracing::error!("WAL 兜底追加失败，数据丢弃: {we}");
            }
            batch.clear();
            // 指数退避 1s→30s（先休眠，再加大下次退避值）
            tokio::time::sleep(Duration::from_secs((*backoff).min(30))).await;
            *backoff = next_backoff(*backoff);
        }
    }
}

/// 后台批量写者：攒批（batch_insert_interval_ms 或 BATCH_MAX）→ insert_batch；
/// 失败 → wal.append(整批) + 指数退避 1s→30s；rx 关闭后排空做最后 flush 再退出。
pub async fn run_writer(
    pool: PgPool,
    mut rx: mpsc::Receiver<LogEvent>,
    hot: Arc<ArcSwap<HotConfig>>,
    wal: WalWriter,
    depth: Gauge,
) {
    let mut batch: Vec<LogEvent> = Vec::with_capacity(BATCH_MAX);
    let mut batch_started: Option<Instant> = None;
    let mut backoff: u64 = 1;

    loop {
        let interval_ms = hot.load().gateway.batch_insert_interval_ms.max(1);
        let interval = Duration::from_millis(interval_ms);
        // 距本批首个事件的 flush 截止时间；无批时先等待首个事件。
        let wait = match batch_started {
            Some(start) => {
                let elapsed = start.elapsed();
                if elapsed >= interval {
                    Duration::ZERO
                } else {
                    interval - elapsed
                }
            }
            None => interval,
        };

        tokio::select! {
            maybe = rx.recv() => {
                match maybe {
                    Some(ev) => {
                        if batch.is_empty() {
                            batch_started = Some(Instant::now());
                        }
                        batch.push(ev);
                        depth.set(rx.len() as i64);
                        if batch.len() >= BATCH_MAX {
                            flush(&pool, &mut batch, &hot, &wal, &mut backoff).await;
                            batch_started = None;
                            depth.set(rx.len() as i64);
                        }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep(wait) => {
                if !batch.is_empty() {
                    flush(&pool, &mut batch, &hot, &wal, &mut backoff).await;
                    batch_started = None;
                    depth.set(rx.len() as i64);
                }
            }
        }
    }

    // rx 关闭：排空剩余批做最后 flush 再退出（PLAN 关闭流程）。
    if !batch.is_empty() {
        flush(&pool, &mut batch, &hot, &wal, &mut backoff).await;
        depth.set(rx.len() as i64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_ramps_and_caps() {
        assert_eq!(next_backoff(1), 2);
        assert_eq!(next_backoff(2), 4);
        assert_eq!(next_backoff(4), 8);
        assert_eq!(next_backoff(8), 16);
        assert_eq!(next_backoff(16), 30); // 上限 30s
        assert_eq!(next_backoff(30), 30);
    }
}
