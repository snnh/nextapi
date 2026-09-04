//! 限流与用量上限（PLAN.md §5.2）。
//!
//! - RPM：内存滑动窗口计数（单实例），请求进入即计数，强一致；
//! - TPM：max_tokens/max_completion_tokens 预检（未传则跳过），完成后按实际 usage 回填
//!   （回填在 M4 记账管道实现，本模块提供 record 接口），最终一致；
//! - 用量上限：基于 quota_usage 独立计数器（最终一致），判定结果缓存 1–5s（可配），
//!   超限动作 block（429）/ warn（仅告警）。

use chrono::{DateTime, Datelike, TimeZone, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::entities::ApiKeyRow;
use crate::error::ApiError;

/// RPM 滑动窗口长度（秒）。
const RPM_WINDOW: Duration = Duration::from_secs(60);

/// RPM 滑动窗口限流器（内存，单实例）。
#[derive(Default)]
pub struct RateLimiter {
    windows: Mutex<HashMap<Uuid, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// RPM 检查：key.rpm 为 None 时用 default_rpm；超限返回 ApiError::RateLimited。
    ///
    /// 有效上限 <= 0 表示不限（直接放行，不计数）。
    pub fn check_rpm(&self, key: &ApiKeyRow, default_rpm: u32) -> Result<(), ApiError> {
        let limit = match key.rpm {
            // 显式配置：<=0 为不限；>0 用实际值
            Some(r) if r <= 0 => return Ok(()),
            Some(r) => r as u32,
            // 未配置：用系统默认，0 表示不限
            None => default_rpm,
        };
        if limit == 0 {
            return Ok(());
        }
        let now = Instant::now();
        let mut map = self.windows.lock().expect("RPM 窗口锁中毒");
        let queue = map.entry(key.id).or_default();
        if window_allow(queue, now, limit) {
            Ok(())
        } else {
            Err(ApiError::RateLimited)
        }
    }

    /// 移除某 Key 的滑动窗口（删除 Key 时调用，防残留，review P3）。
    pub fn remove_key(&self, key_id: Uuid) {
        self.windows.lock().expect("RPM 窗口锁中毒").remove(&key_id);
    }
}

/// 滑动窗口纯逻辑（60s）：先剔除过期事件，再判断能否容纳新事件。
/// 放行时把 `now` 压入队列；超限返回 false 且不压入。
/// `limit == 0` 视为不限（直接放行、不记录）。
fn window_allow(queue: &mut VecDeque<Instant>, now: Instant, limit: u32) -> bool {
    if limit == 0 {
        return true;
    }
    // 剔除过期事件（队列按时间递增，从头部开始；严格大于窗口即过期）
    while let Some(&t) = queue.front() {
        if now.duration_since(t) > RPM_WINDOW {
            queue.pop_front();
        } else {
            break;
        }
    }
    if queue.len() as u32 >= limit {
        return false;
    }
    queue.push_back(now);
    true
}

/// TPM 预检：请求未传 max_tokens 时跳过；max_tokens 超上限时直接拒绝。
/// （精确的窗口内累计判定在 M4 记账回填后生效，M3 仅做单次请求上限截断。）
pub fn tpm_precheck(key: &ApiKeyRow, max_tokens: Option<u64>) -> Result<(), ApiError> {
    if let Some(t) = key.tpm {
        // tpm <= 0 视为不限
        if t <= 0 {
            return Ok(());
        }
        if let Some(m) = max_tokens {
            if m > t as u64 {
                return Err(ApiError::RateLimited);
            }
        }
    }
    Ok(())
}

/// 用量上限检查（quota_usage 计数器，最终一致 + 结果缓存）。
///
/// - key 未配置 quota_limit/quota_unit/quota_window → 放行；
/// - 窗口起点按 billing_timezone 计算（daily/monthly；total 固定 epoch）；
/// - 超限：action=block → ApiError::RateLimited；action=warn → tracing::warn 后放行。
pub async fn check_quota(
    pool: &PgPool,
    key: &ApiKeyRow,
    billing_timezone: &str,
    action: &str,
) -> Result<(), ApiError> {
    let (Some(limit), Some(unit), Some(window)) =
        (&key.quota_limit, &key.quota_unit, &key.quota_window)
    else {
        return Ok(());
    };

    let period_start = window_start(window, billing_timezone, Utc::now());

    // 查询该窗口的累计用量；无行 = 0
    let usage: Option<Decimal> = match sqlx::query_scalar::<_, Decimal>(
        "SELECT value FROM quota_usage WHERE key_id = $1 AND unit = $2 AND window = $3 AND period_start = $4",
    )
    .bind(key.id)
    .bind(unit.as_str())
    .bind(window.as_str())
    .bind(period_start)
    .fetch_optional(pool)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            // 查询失败：记 warn 放行（用量上限为最终一致，不阻断主链路）
            tracing::warn!("查询用量上限失败，放行: key={} err={e}", key.id);
            return Ok(());
        }
    };

    let value = usage.unwrap_or_default(); // 无行 = 用量 0
    if value >= *limit {
        if action == "warn" {
            tracing::warn!(
                "用量上限预警: key={} unit={} window={} usage={} limit={}",
                key.id,
                unit,
                window,
                value,
                limit
            );
            return Ok(());
        }
        return Err(ApiError::RateLimited);
    }
    Ok(())
}

/// 计算窗口起点（daily/monthly 按计费时区；total 固定 1970-01-01）。纯函数，供测试。
/// 非法 window 按 total 处理；非法 tz 回退 UTC。
pub fn window_start(window: &str, tz: &str, now: DateTime<Utc>) -> DateTime<Utc> {
    // total 与未知 window：固定 epoch
    if window != "daily" && window != "monthly" {
        return Utc
            .with_ymd_and_hms(1970, 1, 1, 0, 0, 0)
            .single()
            .expect("epoch");
    }

    // 非法时区回退 UTC
    let tz: chrono_tz::Tz = tz.parse().unwrap_or_else(|_| "UTC".parse().expect("UTC"));

    let local = now.with_timezone(&tz);
    let day = if window == "monthly" {
        local.date_naive().with_day(1).expect("月份首日有效")
    } else {
        local.date_naive()
    };
    let naive = day.and_hms_opt(0, 0, 0).expect("当地 00:00 有效");

    // 当地 00:00 → 回 UTC。DST 边缘取最早候选；极端无映射直接按 UTC 解释。
    match naive.and_local_timezone(tz) {
        chrono::LocalResult::Single(dt) => dt.with_timezone(&Utc),
        chrono::LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
        chrono::LocalResult::None => naive.and_utc(),
    }
}

/// 用量上限判定缓存（key_id → (结果, 缓存时间)），避免每请求聚合查询。
#[derive(Default)]
pub struct QuotaCache {
    inner: Mutex<HashMap<Uuid, (bool, Instant)>>, // (是否超限, 判定时间)
}

impl QuotaCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 缓存未过期（< ttl）返回 Some(超限判定)；过期或未命中返回 None。
    pub fn get(&self, key_id: Uuid, ttl_secs: u64) -> Option<bool> {
        let map = self.inner.lock().expect("QuotaCache 锁中毒");
        match map.get(&key_id) {
            Some(&(exceeded, at)) if at.elapsed() < Duration::from_secs(ttl_secs) => Some(exceeded),
            _ => None,
        }
    }

    pub fn put(&self, key_id: Uuid, exceeded: bool) {
        self.inner
            .lock()
            .expect("QuotaCache 锁中毒")
            .insert(key_id, (exceeded, Instant::now()));
    }

    /// 移除某 Key 的判定缓存（删除 Key 时调用，防残留，review P3）。
    pub fn remove(&self, key_id: Uuid) {
        self.inner
            .lock()
            .expect("QuotaCache 锁中毒")
            .remove(&key_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // check_quota 依赖 PgPool（quota_usage 表），无 DB 环境无法单测；其窗口起点逻辑
    // 已抽成 window_start 纯函数并在下方覆盖。DB 交互部分留给集成测试。

    /// 构造测试 Key。
    fn mk_key(rpm: Option<i32>, tpm: Option<i32>) -> ApiKeyRow {
        ApiKeyRow {
            id: Uuid::new_v4(),
            name: "test".into(),
            key_hash: "hash".into(),
            prefix: "sk-nx-".into(),
            enabled: true,
            models: None,
            rpm,
            tpm,
            quota_limit: None,
            quota_unit: None,
            quota_window: None,
            allow_upstream_passthrough: false,
            passthrough_upstreams: None,
            debug_enabled: false,
            debug_expires_at: None,
            expires_at: None,
            created_at: Utc::now(),
            last_used_at: None,
        }
    }

    /// 构造 UTC 时刻（单值无歧义）。
    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("构建 UTC")
    }

    // ---- 滑窗 window_allow ----

    #[test]
    fn window_allow_allow_reject_expire() {
        let mut q = VecDeque::new();
        let start = Instant::now();
        // 放行前 3 个
        assert!(window_allow(&mut q, start, 3));
        assert!(window_allow(&mut q, start + Duration::from_secs(1), 3));
        assert!(window_allow(&mut q, start + Duration::from_secs(2), 3));
        assert_eq!(q.len(), 3);
        // 第 4 个超限（不压入、不增长）
        assert!(!window_allow(&mut q, start + Duration::from_secs(3), 3));
        assert_eq!(q.len(), 3);
        // 跳至 far future：全部过期，重新放行且只剩新压入的 1 个
        assert!(window_allow(&mut q, start + Duration::from_secs(100), 3));
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn window_allow_boundary_count() {
        let mut q = VecDeque::new();
        let start = Instant::now();
        let limit = 1;
        // 恰在 60s 边界的旧事件仍算在窗口内 → 拒绝
        assert!(window_allow(&mut q, start, limit));
        assert!(!window_allow(
            &mut q,
            start + Duration::from_secs(60),
            limit
        ));
        assert_eq!(q.len(), 1);
        // 61s：过期 → 放行
        assert!(window_allow(&mut q, start + Duration::from_secs(61), limit));
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn window_allow_limit_zero_unlimited() {
        let mut q = VecDeque::new();
        // limit=0 视为不限：直接放行且不记录
        assert!(window_allow(&mut q, Instant::now(), 0));
        assert!(q.is_empty());
    }

    // ---- check_rpm（上限解析 + 计数） ----

    #[test]
    fn check_rpm_at_limit_rate_limited() {
        let limiter = RateLimiter::new();
        let key = mk_key(Some(2), None);
        assert!(matches!(limiter.check_rpm(&key, 0), Ok(())));
        assert!(matches!(limiter.check_rpm(&key, 0), Ok(())));
        // 第 3 个超限
        assert!(matches!(
            limiter.check_rpm(&key, 0),
            Err(ApiError::RateLimited)
        ));
    }

    #[test]
    fn check_rpm_unlimited_when_zero_or_negative() {
        let limiter = RateLimiter::new();
        // rpm None + default 0 → 不限
        let key_none = mk_key(None, None);
        for _ in 0..5 {
            assert!(matches!(limiter.check_rpm(&key_none, 0), Ok(())));
        }
        // rpm Some(<=0)（key.tpm 无关，此处设 None）
        let key_neg = mk_key(Some(-1), None);
        for _ in 0..5 {
            assert!(matches!(limiter.check_rpm(&key_neg, 0), Ok(())));
        }
    }

    #[test]
    fn check_rpm_uses_default_rpm() {
        let limiter = RateLimiter::new();
        let key = mk_key(None, None); // rpm 未配置
                                      // default_rpm = 1：第 1 个放行，第 2 个超限
        assert!(matches!(limiter.check_rpm(&key, 1), Ok(())));
        assert!(matches!(
            limiter.check_rpm(&key, 1),
            Err(ApiError::RateLimited)
        ));
    }

    // ---- tpm_precheck ----

    #[test]
    fn tpm_precheck_branches() {
        // tpm Some(100)，max Some(200) → 拒
        assert!(matches!(
            tpm_precheck(&mk_key(Some(1), Some(100)), Some(200)),
            Err(ApiError::RateLimited)
        ));
        // max == tpm → 放行（边界，m > t 才拒）
        assert!(matches!(
            tpm_precheck(&mk_key(Some(1), Some(100)), Some(100)),
            Ok(())
        ));
        // max < tpm → 放行
        assert!(matches!(
            tpm_precheck(&mk_key(Some(1), Some(100)), Some(99)),
            Ok(())
        ));
        // max 未传 → 放行
        assert!(matches!(
            tpm_precheck(&mk_key(Some(1), Some(100)), None),
            Ok(())
        ));
        // tpm 未配置 → 放行
        assert!(matches!(
            tpm_precheck(&mk_key(Some(1), None), Some(9999)),
            Ok(())
        ));
        // tpm Some(0) → 不限放行
        assert!(matches!(
            tpm_precheck(&mk_key(Some(1), Some(0)), Some(9999)),
            Ok(())
        ));
        // tpm Some(负) → 不限放行
        assert!(matches!(
            tpm_precheck(&mk_key(Some(1), Some(-5)), Some(9999)),
            Ok(())
        ));
    }

    // ---- window_start ----

    /// 固定 now：2024-03-14 18:00:00 UTC（Asia/Shanghai 此刻为 2024-03-15 02:00）。
    fn fixed_now() -> DateTime<Utc> {
        utc(2024, 3, 14, 18, 0, 0)
    }

    #[test]
    fn window_start_daily_utc_offset() {
        // Shanghai 当日 00:00 = UTC 前一日 16:00
        let start = window_start("daily", "Asia/Shanghai", fixed_now());
        assert_eq!(start, utc(2024, 3, 14, 16, 0, 0));
    }

    #[test]
    fn window_start_monthly_utc_offset() {
        // Shanghai 当月 1 日 00:00 = UTC 前月最后一日 16:00
        let start = window_start("monthly", "Asia/Shanghai", fixed_now());
        assert_eq!(start, utc(2024, 2, 29, 16, 0, 0));
    }

    #[test]
    fn window_start_total_fixed_epoch() {
        let start = window_start("total", "Asia/Shanghai", fixed_now());
        assert_eq!(start, utc(1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn window_start_unknown_window_total() {
        let start = window_start("hourly", "Asia/Shanghai", fixed_now());
        assert_eq!(start, utc(1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn window_start_invalid_tz_fallback_utc() {
        // 回退后按 UTC 计算当日/当月起点
        let daily = window_start("daily", "Not/AZone", fixed_now());
        assert_eq!(daily, utc(2024, 3, 14, 0, 0, 0));
        let monthly = window_start("monthly", "Not/AZone", fixed_now());
        assert_eq!(monthly, utc(2024, 3, 1, 0, 0, 0));
    }

    // ---- QuotaCache ----

    #[test]
    fn quota_cache_ttl() {
        let cache = QuotaCache::new();
        let id = Uuid::new_v4();
        // 未命中
        assert_eq!(cache.get(id, 10), None);
        // put 后未过期命中
        cache.put(id, true);
        assert_eq!(cache.get(id, 10), Some(true));
        // ttl=0 立即过期（永不缓存）
        assert_eq!(cache.get(id, 0), None);
        // 过期：ttl=1，睡 1.1s 后未命中
        cache.put(id, false);
        std::thread::sleep(Duration::from_millis(1100));
        assert_eq!(cache.get(id, 1), None);
    }
}
