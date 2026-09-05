//! 路由决策：模型通配匹配、优先级/加权轮询、熔断与自动禁用（PLAN.md §5.3）。
//!
//! 重试参数只在 model_routes 目标级维护；熔断连续失败自动禁用 + 冷却 + 半开探活；
//! 手动禁用（disabled_by='manual'）优先于一切自动逻辑。

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

use crate::entities::{ModelRouteRow, UpstreamRow};

/// 冷却期默认 60s（冷却到期后半开探活恢复）。
pub const DEFAULT_COOLDOWN_SECS: i64 = 60;

/// 半开探测冷却的指数退避上限：60s × 2^n 封顶 30 分钟。
pub const MAX_COOLDOWN_SECS: i64 = 30 * 60;

/// 半开状态的死锁防护：探测在飞（HalfOpen）超过该时长仍无结果则重置计时并再放行一个。
pub const HALFOPEN_TIMEOUT_SECS: i64 = 120;

/// 模型通配符匹配（仅支持 `*` 通配任意字符序列）。
///
/// 采用贪心回退算法：遇到 `*` 记录回溯点，失配时让最近的 `*` 多吃一个字符再试。
/// 仅将 `*` 视为元字符，`?`、`[` 等其它 glob 元字符一律按字面量匹配。最坏 O(n·m)。
pub fn pattern_matches(pattern: &str, model: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = model.chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    // 最近遇到的一个 `*` 在 pattern 中的下标，以及它当前在 text 中匹配到的位置
    let mut star: Option<usize> = None;
    let mut star_match: usize = 0;

    while t < txt.len() {
        if p < pat.len() {
            if pat[p] == '*' {
                star = Some(p);
                star_match = t;
                p += 1;
                continue;
            }
            if pat[p] == txt[t] {
                p += 1;
                t += 1;
                continue;
            }
        }
        // 失配：回溯到最近的 `*`，让它多吃一个字符再试
        if let Some(sp) = star {
            star_match += 1;
            t = star_match;
            p = sp + 1;
        } else {
            return false;
        }
    }
    // 消费 pattern 尾部剩余的 `*`
    while p < pat.len() && pat[p] == '*' {
        p += 1;
    }
    p == pat.len()
}

/// 匹配某模型的启用路由，按 (priority 升序, sort_order 升序) 排序返回。
pub fn match_routes(model: &str, routes: &[ModelRouteRow]) -> Vec<ModelRouteRow> {
    let mut matched: Vec<ModelRouteRow> = routes
        .iter()
        .filter(|r| r.enabled && pattern_matches(&r.model_pattern, model))
        .cloned()
        .collect();
    matched.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(a.sort_order.cmp(&b.sort_order))
    });
    matched
}

/// 同优先级组内按权重加权随机选择（weight <= 0 视为 1）。
pub fn weighted_pick(routes: &[ModelRouteRow]) -> Option<ModelRouteRow> {
    if routes.is_empty() {
        return None;
    }
    use rand::Rng;
    let weight_of = |r: &ModelRouteRow| if r.weight <= 0 { 1u64 } else { r.weight as u64 };
    let total: u64 = routes.iter().map(weight_of).sum();
    let mut rng = rand::rng();
    let mut draw: u64 = rng.random_range(0..total);
    for r in routes {
        let w = weight_of(r);
        if draw < w {
            return Some(r.clone());
        }
        draw -= w;
    }
    // 理论上不会走到这里（total >= 1），兜底返回末项。
    routes.last().cloned()
}

/// 熔断器状态。
#[derive(Debug, Clone)]
pub enum BreakerState {
    /// 正常（记录连续失败数）
    Closed { failures: u32 },
    /// 熔断打开（冷却中，until 前拒绝请求；consecutive_opens 用于半开失败指数退避）
    Open {
        until: DateTime<Utc>,
        consecutive_opens: u32,
    },
    /// 半开（冷却到期，放行 1 个探测请求）
    HalfOpen { consecutive_opens: u32 },
}

/// 冷却时长计算（纯函数）。
///
/// `opens` 为「进入本次打开之前」的连续打开次数（Closed→Open 时为 0，HalfOpen{n}→Open 时为 n）。
/// 返回 `DEFAULT_COOLDOWN_SECS × 2^opens`，封顶 `MAX_COOLDOWN_SECS`（30 分钟）。
pub fn cooldown_secs(opens: u32) -> i64 {
    let secs = (DEFAULT_COOLDOWN_SECS as i64).saturating_mul(1i64 << opens.min(62));
    secs.min(MAX_COOLDOWN_SECS)
}

/// 熔断状态迁移（纯函数）。
///
/// 入参 `state` 为当前状态、`threshold` 为连续失败阈值、`now` 为当前时间。
/// 返回 `(新状态, 是否需回写 DB)`。回写内容由调用方基于新旧状态推导：
/// - Closed 连续失败未达阈值 → 仅内存计数，不回写；
/// - Closed 达到阈值 → Open（默认冷却 60s，consecutive_opens=1），需回写；
/// - HalfOpen 探测失败 → Open（冷却指数退避 60s×2^n，consecutive_opens+1），需回写；
/// - Open 中再次失败 → 保持 Open 不变，仅需回写 DB consecutive_failures。
pub fn next_on_failure(
    state: &BreakerState,
    threshold: u32,
    now: DateTime<Utc>,
) -> (BreakerState, bool) {
    match state {
        BreakerState::Closed { failures } => {
            let f = failures + 1;
            if f >= threshold {
                let until = now + chrono::Duration::seconds(cooldown_secs(0));
                (
                    BreakerState::Open {
                        until,
                        consecutive_opens: 1,
                    },
                    true,
                )
            } else {
                (BreakerState::Closed { failures: f }, false)
            }
        }
        BreakerState::Open { .. } => (state.clone(), true),
        BreakerState::HalfOpen { consecutive_opens } => {
            let opens = *consecutive_opens;
            let until = now + chrono::Duration::seconds(cooldown_secs(opens));
            (
                BreakerState::Open {
                    until,
                    consecutive_opens: opens + 1,
                },
                true,
            )
        }
    }
}

/// DB 回写动作（仅 on_failure 内部使用）。
enum DbUpdate {
    /// 打开冷却：写 disabled_by='auto' + cooldown_until + consecutive_failures
    Open { until: DateTime<Utc>, failures: i32 },
    /// 已是 Open 再失败：只更新 consecutive_failures
    FailuresOnly { failures: i32 },
}

/// 熔断器：内存状态机；自动禁用时回写 DB（disabled_by='auto' + cooldown_until）。
/// 手动禁用（DB 行 disabled_by='manual'）优先于一切自动逻辑。
#[derive(Default)]
pub struct Breaker {
    states: Mutex<HashMap<Uuid, BreakerState>>,
    /// 进入 HalfOpen 的时间，用于探测超时未收敛时的死锁防护重置。
    halfopen_since: Mutex<HashMap<Uuid, DateTime<Utc>>>,
}

impl Breaker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否放行该上游的请求。手动禁用/自动冷却中的上游返回 false；
    /// 冷却到期自动进入半开，放行一个探测请求。
    pub fn allow(&self, up: &UpstreamRow) -> bool {
        if !up.enabled || up.disabled_by.as_deref() == Some("manual") {
            return false;
        }
        let id = up.id;
        let now = Utc::now();
        // 重启恢复（review P4）：熔断自动禁用会回写 DB（disabled_by='auto' + cooldown_until），
        // 而内存状态机重启即空。冷启动后若 DB 行仍处于冷却期，把状态导入 Open 并拒绝放行，
        // 避免重启/滚动更新绕过冷却直接把流量打向已知故障上游；冷却已过期的 auto 行不拦截
        // （走下方 None → Closed 正常放行，成功后续 on_success 会清 DB 状态）。
        if up.disabled_by.as_deref() == Some("auto") {
            if let Some(until) = up.cooldown_until {
                if until > now {
                    let mut states = self.states.lock().unwrap();
                    if !states.contains_key(&id) {
                        states.insert(
                            id,
                            BreakerState::Open {
                                until,
                                consecutive_opens: up.consecutive_failures.max(1) as u32,
                            },
                        );
                    }
                    return false;
                }
            }
        }
        let mut states = self.states.lock().unwrap();
        match states.get(&id).cloned() {
            None => {
                states.insert(id, BreakerState::Closed { failures: 0 });
                true
            }
            Some(BreakerState::Closed { .. }) => true,
            Some(BreakerState::Open {
                until,
                consecutive_opens,
            }) => {
                if now < until {
                    false
                } else {
                    states.insert(id, BreakerState::HalfOpen { consecutive_opens });
                    self.halfopen_since.lock().unwrap().insert(id, now);
                    true
                }
            }
            Some(BreakerState::HalfOpen { .. }) => {
                // 已有探测在飞，默认拒绝；仅当半开状态持续超时未收敛时重置放行，防死锁。
                let stuck = self
                    .halfopen_since
                    .lock()
                    .unwrap()
                    .get(&id)
                    .cloned()
                    .map(|since| (now - since).num_seconds() >= HALFOPEN_TIMEOUT_SECS)
                    .unwrap_or(false);
                if stuck {
                    self.halfopen_since.lock().unwrap().insert(id, now);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// 记录成功：清零失败计数；半开 → 关闭；若 DB 行为 auto 禁用则恢复
    /// （清除 disabled_by/cooldown_until，写回 consecutive_failures=0）。
    pub async fn on_success(&self, pool: &sqlx::PgPool, up: &UpstreamRow) {
        let id = up.id;
        {
            let mut states = self.states.lock().unwrap();
            states.insert(id, BreakerState::Closed { failures: 0 });
        }
        self.halfopen_since.lock().unwrap().remove(&id);

        if up.disabled_by.as_deref() == Some("auto") {
            let res = sqlx::query(
                "UPDATE upstreams SET disabled_by=NULL, cooldown_until=NULL, consecutive_failures=0 WHERE id=$1",
            )
            .bind(id)
            .execute(pool)
            .await;
            if let Err(e) = res {
                tracing::warn!(upstream = %up.name, error = %e, "熔断恢复写库失败");
            }
        }
    }

    /// 记录失败：连续失败 ≥ breaker_threshold → 自动禁用 + 冷却（默认 60s）
    /// 并回写 DB（disabled_by='auto', cooldown_until, consecutive_failures）；
    /// 半开探测失败 → 重新冷却，连续打开次数 +1，冷却时长指数退避（×2，上限 30min）。
    pub async fn on_failure(&self, pool: &sqlx::PgPool, up: &UpstreamRow) {
        if up.disabled_by.as_deref() == Some("manual") {
            return;
        }
        let id = up.id;
        let now = Utc::now();
        let threshold = up.breaker_threshold.max(1) as u32;

        // 在锁内完成状态迁移（读→计算→**立即写回**，杜绝并发失败计数丢失）与 DB
        // 回写内容的推导；`await` 放在锁外，避免持有 std Mutex 跨 await。
        // DB 回写基于旧状态推导：并发下 FailuresOnly 可能略滞后于内存计数，
        // 但 Open 边界回写准确（重启恢复用，允许近似）。
        let (next, db) = {
            let mut states = self.states.lock().unwrap();
            let state = states
                .get(&id)
                .cloned()
                .unwrap_or(BreakerState::Closed { failures: 0 });
            let (next, _write_back) = next_on_failure(&state, threshold, now);
            states.insert(id, next.clone());

            // 离开 HalfOpen 时清理探测计时，避免残留。
            if matches!(state, BreakerState::HalfOpen { .. })
                && !matches!(next, BreakerState::HalfOpen { .. })
            {
                self.halfopen_since.lock().unwrap().remove(&id);
            }

            let db = match (&state, &next) {
                (BreakerState::Open { .. }, BreakerState::Open { .. }) => {
                    Some(DbUpdate::FailuresOnly {
                        failures: up.consecutive_failures.saturating_add(1) as i32,
                    })
                }
                (_, BreakerState::Open { until, .. }) => Some(DbUpdate::Open {
                    until: *until,
                    failures: match &state {
                        BreakerState::Closed { failures } => *failures as i32,
                        _ => threshold as i32,
                    },
                }),
                _ => None,
            };
            (next, db)
        };
        debug_assert_eq!(
            db.is_some(),
            matches!(next, BreakerState::Open { .. }),
            "DB 回写与开闸状态应一致"
        );
        let _ = &next;

        if let Some(db) = db {
            let res = match db {
                DbUpdate::Open { until, failures } => {
                    sqlx::query(
                        "UPDATE upstreams SET disabled_by='auto', cooldown_until=$1, consecutive_failures=$2 WHERE id=$3",
                    )
                    .bind(until)
                    .bind(failures)
                    .bind(id)
                    .execute(pool)
                    .await
                }
                DbUpdate::FailuresOnly { failures } => {
                    sqlx::query("UPDATE upstreams SET consecutive_failures=$1 WHERE id=$2")
                        .bind(failures)
                        .bind(id)
                        .execute(pool)
                        .await
                }
            };
            if let Err(e) = res {
                tracing::warn!(upstream = %up.name, error = %e, "熔断失败回写 DB 失败");
            }
        }
        // 内存状态已在锁内推进，无需此处回写（见上）。
    }

    /// 手动启停后清除内存状态（由管理 API 调用）。
    pub fn reset(&self, upstream_id: Uuid) {
        self.states.lock().unwrap().remove(&upstream_id);
        self.halfopen_since.lock().unwrap().remove(&upstream_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn route(
        tag: &str,
        pattern: &str,
        priority: i32,
        weight: i32,
        enabled: bool,
        sort_order: i32,
    ) -> ModelRouteRow {
        let now = Utc::now();
        ModelRouteRow {
            id: Uuid::new_v4(),
            model_pattern: pattern.into(),
            upstream_id: Uuid::new_v4(),
            override_model: Some(tag.into()),
            priority,
            weight,
            enabled,
            retries: 2,
            retry_status_codes: vec![429, 500],
            lock_upstream: false,
            sort_order,
            created_at: now,
            updated_at: now,
        }
    }

    fn upstream(threshold: i32, disabled_by: Option<&str>) -> UpstreamRow {
        let now = Utc::now();
        UpstreamRow {
            id: Uuid::new_v4(),
            name: "up".into(),
            kind: "openai".into(),
            base_url: "http://localhost".into(),
            api_key_plain: None,
            protocols: vec!["openai_chat".into()],
            enabled: true,
            timeout_ms: 300_000,
            breaker_threshold: threshold,
            probe_model: None,
            consecutive_failures: 0,
            disabled_by: disabled_by.map(String::from),
            cooldown_until: None,
            use_proxy: false,
            proxy_id: None,
            extra: serde_json::json!({}),
            created_at: now,
            updated_at: now,
        }
    }

    /// 用于测试的懒连接池：仅用于 on_success/on_failure 的 DB 写（连不上时仅记 warn，不影响状态机）。
    /// 使用极短 acquire_timeout 让连接失败快速返回，避免测试被默认 30s 超时拖慢。
    fn lazy_pool() -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(300))
            .connect_lazy("postgres://u:p@127.0.0.1:1/db")
            .unwrap()
    }

    #[test]
    fn pattern_matches_forms() {
        // "*" 匹配一切（含空串）
        assert!(pattern_matches("*", ""));
        assert!(pattern_matches("*", "gpt-4o"));
        assert!(pattern_matches("*", "anything"));

        // 前缀
        assert!(pattern_matches("gpt-*", "gpt-4o"));
        assert!(pattern_matches("gpt-*", "gpt-"));
        assert!(!pattern_matches("gpt-*", "gpt"));
        assert!(!pattern_matches("gpt-*", "claude-4o"));

        // 后缀
        assert!(pattern_matches("*-preview", "gpt-4o-preview"));
        assert!(pattern_matches("*-preview", "-preview"));
        assert!(!pattern_matches("*-preview", "gpt-4o"));

        // 多段
        assert!(pattern_matches("a*b*c", "axxbyyc"));
        assert!(pattern_matches("a*b*c", "abc"));
        assert!(pattern_matches("a*b*c", "aXbXc"));
        assert!(!pattern_matches("a*b*c", "aXb"));
        assert!(!pattern_matches("a*b*c", "bc"));

        // 无通配符的字面匹配
        assert!(pattern_matches("gpt-4o", "gpt-4o"));
        assert!(!pattern_matches("gpt-4o", "gpt-4o-mini"));

        // 空 pattern 只匹配空串
        assert!(pattern_matches("", ""));
        assert!(!pattern_matches("", "x"));

        // 其它 glob 元字符按字面量处理（不支持 ? / [）
        assert!(pattern_matches("a?c", "a?c"));
        assert!(!pattern_matches("a?c", "abc"));

        // 前后/星号与字面量交错
        assert!(pattern_matches("*x*", "axb"));
        assert!(!pattern_matches("*x*", "abc"));
        assert!(pattern_matches("x*", "xyz"));
        assert!(pattern_matches("*x", "zyx"));
        assert!(pattern_matches("*", "*"));
    }

    #[test]
    fn match_routes_filters_and_sorts() {
        let routes = vec![
            route("late-p10-s2", "gpt-*", 10, 1, true, 2),
            route("late-p20", "gpt-*", 20, 1, true, 0),
            route("disabled", "gpt-*", 5, 1, false, 0),
            route("nomatch", "claude-*", 5, 1, true, 0),
            route("early-p10-s0", "gpt-*", 10, 1, true, 0),
            route("early-p10-s1", "*", 10, 1, true, 1),
        ];
        let out = match_routes("gpt-4o", &routes);
        let tags: Vec<&str> = out
            .iter()
            .map(|r| r.override_model.as_deref().unwrap())
            .collect();
        assert_eq!(
            tags,
            vec!["early-p10-s0", "early-p10-s1", "late-p10-s2", "late-p20"]
        );
        assert!(out.iter().all(|r| r.enabled));
    }

    #[test]
    fn weighted_pick_distribution() {
        let routes = vec![
            route("w1", "a", 1, 1, true, 0),
            route("w3", "b", 1, 3, true, 0),
        ];
        let first_id = routes[0].id;
        let n = 1000;
        let mut hits = 0;
        for _ in 0..n {
            if let Some(r) = weighted_pick(&routes) {
                if r.id == first_id {
                    hits += 1;
                }
            }
        }
        let frac = hits as f64 / n as f64;
        // 期望约 0.25；容忍一定统计波动
        assert!(
            frac > 0.18 && frac < 0.32,
            "first-weight fraction observed {frac}"
        );
    }

    #[test]
    fn weighted_pick_edge_cases() {
        assert!(weighted_pick(&[]).is_none());
        let single = vec![route("s", "a", 1, 0, true, 0)]; // weight 0 → 视为 1
        assert_eq!(weighted_pick(&single).unwrap().id, single[0].id);
    }

    #[test]
    fn cooldown_secs_values() {
        assert_eq!(cooldown_secs(0), 60);
        assert_eq!(cooldown_secs(1), 120);
        assert_eq!(cooldown_secs(2), 240);
        assert_eq!(cooldown_secs(3), 480);
        assert_eq!(cooldown_secs(10), MAX_COOLDOWN_SECS);
        assert_eq!(cooldown_secs(1000), MAX_COOLDOWN_SECS);
    }

    #[test]
    fn next_on_failure_closed_state() {
        let now = Utc::now();
        // 未达阈值（failures=0 → 1）：仅内存计数，不回写
        let (s, wb) = next_on_failure(&BreakerState::Closed { failures: 0 }, 2, now);
        assert!(!wb);
        assert!(matches!(s, BreakerState::Closed { failures: 1 }));

        // 达到阈值（failures=1 → 2）：打开，默认冷却 60s，consecutive_opens=1
        let (s, wb) = next_on_failure(&BreakerState::Closed { failures: 1 }, 2, now);
        assert!(wb);
        match s {
            BreakerState::Open {
                until,
                consecutive_opens,
            } => {
                assert_eq!(consecutive_opens, 1);
                assert_eq!(until, now + Duration::seconds(60));
            }
            _ => panic!("期望 Open"),
        }
    }

    #[test]
    fn next_on_failure_halfopen_backoff() {
        let now = Utc::now();
        let (s, wb) = next_on_failure(
            &BreakerState::HalfOpen {
                consecutive_opens: 1,
            },
            5,
            now,
        );
        assert!(wb);
        match s {
            BreakerState::Open {
                until,
                consecutive_opens,
            } => {
                assert_eq!(consecutive_opens, 2);
                // 60s × 2^1 = 120s
                assert_eq!(until, now + Duration::seconds(120));
            }
            _ => panic!("期望 Open"),
        }
        // 连续打开次数再 +1 → 240s
        let (s2, _) = next_on_failure(
            &BreakerState::HalfOpen {
                consecutive_opens: 2,
            },
            5,
            now,
        );
        match s2 {
            BreakerState::Open {
                until,
                consecutive_opens,
            } => {
                assert_eq!(consecutive_opens, 3);
                assert_eq!(until, now + Duration::seconds(240));
            }
            _ => panic!("期望 Open"),
        }
    }

    #[test]
    fn next_on_failure_open_stays_open() {
        let now = Utc::now();
        let until = now + Duration::seconds(500);
        let (s, wb) = next_on_failure(
            &BreakerState::Open {
                until,
                consecutive_opens: 3,
            },
            5,
            now,
        );
        assert!(wb); // 仅需回写 DB consecutive_failures
        match s {
            BreakerState::Open {
                until: u,
                consecutive_opens,
            } => {
                assert_eq!(consecutive_opens, 3);
                assert_eq!(u, until); // until 不变
            }
            _ => panic!("期望仍为 Open"),
        }
    }

    #[tokio::test]
    async fn breaker_full_lifecycle() {
        let pool = lazy_pool();
        let br = Breaker::new();
        let up = upstream(2, None);
        let id = up.id;

        // 初始（无记录）→ 初始化 Closed 并放行
        assert!(br.allow(&up));

        // 一次失败（threshold=2，未达阈值）→ 仍在 Closed，放行
        br.on_failure(&pool, &up).await;
        assert!(br.allow(&up));
        assert!(matches!(
            &*br.states.lock().unwrap().get(&id).unwrap(),
            BreakerState::Closed { failures: 1 }
        ));

        // 第二次失败达到阈值 → Open，拒绝
        br.on_failure(&pool, &up).await;
        assert!(!br.allow(&up));
        assert!(matches!(
            &*br.states.lock().unwrap().get(&id).unwrap(),
            BreakerState::Open { .. }
        ));

        // 冷却到期：注入已过期的 Open → allow 转 HalfOpen 并放行一个探测
        br.states.lock().unwrap().insert(
            id,
            BreakerState::Open {
                until: Utc::now() - Duration::seconds(1),
                consecutive_opens: 1,
            },
        );
        assert!(br.allow(&up));
        // 探测在飞，再次 allow 应被拒绝
        assert!(!br.allow(&up));
        assert!(matches!(
            &*br.states.lock().unwrap().get(&id).unwrap(),
            BreakerState::HalfOpen { .. }
        ));

        // 半开探测成功 → 关闭，计数清零
        br.on_success(&pool, &up).await;
        assert!(matches!(
            &*br.states.lock().unwrap().get(&id).unwrap(),
            BreakerState::Closed { failures: 0 }
        ));
        assert!(br.allow(&up));
    }

    #[tokio::test]
    async fn breaker_halfopen_probe_failure_backoff() {
        let pool = lazy_pool();
        let br = Breaker::new();
        let up = upstream(2, None);
        let id = up.id;

        // 半开探测失败：consecutive_opens=1 → Open{opens:2, until≈now+120s}
        br.states.lock().unwrap().insert(
            id,
            BreakerState::HalfOpen {
                consecutive_opens: 1,
            },
        );
        let before = Utc::now();
        br.on_failure(&pool, &up).await;
        let after = Utc::now();
        let guard = br.states.lock().unwrap();
        match guard.get(&id).unwrap() {
            BreakerState::Open {
                until,
                consecutive_opens,
            } => {
                assert_eq!(*consecutive_opens, 2);
                let secs = (*until - before).num_seconds();
                assert!(
                    secs >= 115 && secs <= 125,
                    "cooldown should be ~120s, got {secs}s"
                );
                assert!(*until > after);
            }
            other => panic!("期望 Open，得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn breaker_halfopen_timeout_reset() {
        let br = Breaker::new();
        let up = upstream(2, None);
        let id = up.id;

        // 刚进入 HalfOpen（<120s）→ 拒绝
        br.states.lock().unwrap().insert(
            id,
            BreakerState::HalfOpen {
                consecutive_opens: 1,
            },
        );
        br.halfopen_since.lock().unwrap().insert(id, Utc::now());
        assert!(!br.allow(&up));

        // 持续超 120s 未收敛 → 重置计时并再放行一个
        br.halfopen_since
            .lock()
            .unwrap()
            .insert(id, Utc::now() - Duration::seconds(121));
        assert!(br.allow(&up));
    }

    #[tokio::test]
    async fn breaker_manual_disable_shortcircuit() {
        let pool = lazy_pool();
        let br = Breaker::new();
        let mut up = upstream(2, None);
        up.disabled_by = Some("manual".into());

        // manual 禁用：allow 恒为 false
        assert!(!br.allow(&up));

        // on_failure 遇到 manual 直接返回，不改变内存状态
        br.states
            .lock()
            .unwrap()
            .insert(up.id, BreakerState::Closed { failures: 0 });
        br.on_failure(&pool, &up).await;
        assert!(matches!(
            &*br.states.lock().unwrap().get(&up.id).unwrap(),
            BreakerState::Closed { failures: 0 }
        ));

        // reset 清除内存状态
        br.reset(up.id);
        assert!(br.states.lock().unwrap().get(&up.id).is_none());
    }

    #[tokio::test]
    async fn breaker_success_recovers_auto_disabled() {
        let pool = lazy_pool();
        let br = Breaker::new();
        let mut up = upstream(2, Some("auto"));
        up.consecutive_failures = 5;
        // DB 行为 auto 禁用：on_success 应清零计数并恢复
        br.on_success(&pool, &up).await;
        assert!(matches!(
            &*br.states.lock().unwrap().get(&up.id).unwrap(),
            BreakerState::Closed { failures: 0 }
        ));
    }

    #[tokio::test]
    async fn breaker_db_cooldown_blocks_after_restart() {
        // review P4：进程重启后内存状态为空，但 DB 行 disabled_by='auto' 且冷却未到期 →
        // allow() 应拒绝放行并导入 Open 状态（冷却期内的流量不得直击故障上游）。
        let pool = lazy_pool();
        let br = Breaker::new();
        let mut up = upstream(2, Some("auto"));
        up.consecutive_failures = 4;
        up.cooldown_until = Some(Utc::now() + Duration::seconds(120));
        assert!(!br.allow(&up));
        assert!(matches!(
            &*br.states.lock().unwrap().get(&up.id).unwrap(),
            BreakerState::Open {
                consecutive_opens: 4,
                ..
            }
        ));
        // 冷却已过期（如重放刚过期的半开窗口）：放行探测，且不导入 Open
        br.reset(up.id);
        up.cooldown_until = Some(Utc::now() - Duration::seconds(1));
        assert!(br.allow(&up));
        assert!(matches!(
            &*br.states.lock().unwrap().get(&up.id).unwrap(),
            BreakerState::Closed { failures: 0 }
        ));
        // 内存已有状态时 DB 冷却不覆盖内存判定（仍在 Open 窗口内 → 拒绝）
        br.states.lock().unwrap().insert(
            up.id,
            BreakerState::Open {
                until: Utc::now() + Duration::seconds(30),
                consecutive_opens: 1,
            },
        );
        assert!(!br.allow(&up));
    }
}
