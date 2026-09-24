//! 上游凭证失效（token 失效）的识别与守护。
//!
//! 四层处理（顺序即请求路径上的处理顺序）：
//!
//! 1. **识别**：[`classify`] 从上游错误文本判定「鉴权失效」，上游层据此产出
//!    [`UpstreamError::AuthFailed`](crate::upstream::UpstreamError::AuthFailed)，
//!    网关对外统一回 `upstream_auth_failed`（而不是原样透出上游 401）；
//! 2. **熔断**：[`AuthGuard`] 把命中失效的上游**临时**摘出候选（[`AUTH_FAIL_TTL_SECS`]
//!    冷却，到期自动放行重试），避免每个请求都白打一次上游；与熔断器（连续失败计数、
//!    自动禁用 + 人工恢复）语义不同，凭证据失效是「换个凭证就好」的可自愈状态；
//! 3. **可见**：失效详情写入 `upstreams.extra.auth_state`（[`auth_state_from_extra`]
//!    读回），后台列表 / 详情直接展示「凭证失效 + 时间 + 原因」；
//! 4. **恢复**：换凭证（后台重新授权 / 编辑上游）或任一请求成功后清除。
//!
//! token 自动刷新本期不做：本模块只保证「失效被识别、被隔离、看得见、好恢复」。

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 凭证失效后的跳过时长（秒）：期内该上游不参与候选，到期自动放行一次重试。
pub const AUTH_FAIL_TTL_SECS: i64 = 300;

/// extra 中凭证状态的键名。
pub const AUTH_STATE_KEY: &str = "auth_state";

/// 判定为「凭证失效」的错误码（小写匹配子串；覆盖 codeium/devin、OpenAI 系、Anthropic）。
const AUTH_CODES: &[&str] = &[
    "unauthenticated",
    "invalid_token",
    "invalid token",
    "invalid api key",
    "invalid_api_key",
    "invalid api-key",
    "incorrect api key",
    "token expired",
    "token has expired",
    "access token expired",
    "expired token",
    "authentication_error",
    "invalid credentials",
];

/// 从上游错误文本识别凭证失效，返回归一化错误码（命中首个匹配）。
///
/// 覆盖形态（实测见 `.owc/devin-ref/RESEARCH.md` §13）：
/// - devin 流式：HTTP 200 + end 帧 `{"error":{"code":"unauthenticated",…}}`
/// - devin unary：HTTP 401 + `{"code":"unauthenticated","message":…}`
/// - OpenAI 系 / Anthropic：401 `invalid_api_key`、`invalid_token` 等
pub fn classify(text: &str) -> Option<&'static str> {
    let low = text.to_ascii_lowercase();
    AUTH_CODES.iter().copied().find(|c| low.contains(c))
}

/// 单次凭证失效率事件（内存守卫与 `extra.auth_state` 共用结构）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthFailure {
    /// 首次发现时刻（RFC3339，落库后可读）
    pub at: DateTime<Utc>,
    /// 上游返回的 HTTP 状态码（devin 流式形态为 200）
    pub status: u16,
    /// 归一化错误码（如 `unauthenticated`）
    pub code: String,
    /// 上游原文（截断后），供后台展示与排障
    pub message: String,
}

impl AuthFailure {
    /// 构造（message 截断到 500 字符，避免把大响应体写进 extra）。
    pub fn new(status: u16, code: impl Into<String>, message: &str) -> Self {
        Self {
            at: Utc::now(),
            status,
            code: code.into(),
            message: crate::text::truncate_chars(message, 500),
        }
    }

    /// 该失效事件在 `now` 时刻是否仍在冷却期内（即上游仍应被跳过）。
    pub fn blocked_at(&self, now: DateTime<Utc>) -> bool {
        now.signed_duration_since(self.at).num_seconds() < AUTH_FAIL_TTL_SECS
    }
}

/// 读 `upstreams.extra.auth_state`（写入由 [`crate::state::AppState::mark_auth_failure`]
/// 以 JSONB 原子更新完成，避免读改写覆盖其它 extra 字段）。
pub fn auth_state_from_extra(extra: &serde_json::Value) -> Option<AuthFailure> {
    let v = extra.get(AUTH_STATE_KEY)?;
    serde_json::from_value(v.clone()).ok()
}

/// 凭证失效守卫：内存标记 + 冷却判定（请求路径零 DB 查询，无锁读快照）。
#[derive(Default)]
pub struct AuthGuard {
    entries: Mutex<HashMap<Uuid, AuthFailure>>,
}
impl AuthGuard {
    /// 记录一次失效。返回 `true` 表示状态发生变化（首次 / 错误码不同 / 上轮冷却已过），
    /// 调用方据此决定是否落库——同一失效期内重复命中不重复写库。
    pub fn mark(&self, id: Uuid, failure: AuthFailure) -> bool {
        let now = failure.at;
        let mut map = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match map.get(&id) {
            Some(prev) if prev.code == failure.code && prev.blocked_at(now) => false,
            _ => {
                map.insert(id, failure);
                true
            }
        }
    }

    /// 清除标记（换凭证 / 请求成功 / 后台探测成功）。返回是否存在过标记。
    pub fn clear(&self, id: &Uuid) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(id)
            .is_some()
    }

    /// 当前标记（含已过期但尚未清除的，供后台展示「最近一次失效」）。
    pub fn get(&self, id: &Uuid) -> Option<AuthFailure> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .cloned()
    }

    /// 冷却期内返回失效详情（供「跳过原因」展示与后台状态）；不在冷却期返回 None。
    pub fn blocking(&self, id: &Uuid) -> Option<AuthFailure> {
        let now = Utc::now();
        self.get(id).filter(|f| f.blocked_at(now))
    }
}

/// 把失效详情写入 `upstreams.extra.auth_state`（JSONB 原子合并，不触碰 extra 其它字段）。
/// 失败不抛错：主链路的隔离已由内存守卫保证，落库只服务于后台可见性。
pub async fn store(db: &sqlx::PgPool, id: Uuid, failure: &AuthFailure) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE upstreams SET extra = jsonb_set(coalesce(extra, '{}'::jsonb), '{auth_state}', \
         $2::jsonb, true), updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(serde_json::to_value(failure).unwrap_or(serde_json::Value::Null))
    .execute(db)
    .await
    .map(|_| ())
}

/// 清除 `upstreams.extra.auth_state`（凭证已更新 / 请求或探测成功）。
pub async fn clear_stored(db: &sqlx::PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE upstreams SET extra = coalesce(extra, '{}'::jsonb) - 'auth_state', updated_at = now() \
         WHERE id = $1 AND extra ? 'auth_state'",
    )
    .bind(id)
    .execute(db)
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_hits_auth_codes_only() {
        // 实测形态（devin）：200+end 帧 与 401 JSON 两种都带 unauthenticated
        assert_eq!(
            classify("unauthenticated: failed to validate Devin token: Invalid token"),
            Some("unauthenticated")
        );
        assert_eq!(
            classify(r#"{"code":"unauthenticated","message":"Invalid or expired code"}"#),
            Some("unauthenticated")
        );
        assert_eq!(
            classify("invalid_api_key: Incorrect API key provided"),
            Some("invalid_api_key")
        );
        assert_eq!(
            classify("Incorrect API key provided: sk-***"),
            Some("incorrect api key")
        );
        assert_eq!(
            classify("AuthenticationError: token has expired"),
            Some("token has expired")
        );
        // 模型门控 / 普通上游错误不得误判为凭证失效
        assert_eq!(
            classify("failed_precondition: an internal error occurred"),
            None
        );
        assert_eq!(classify("permission_denied: model requires Pro"), None);
        assert_eq!(classify("upstream timeout"), None);
    }

    #[test]
    fn auth_failure_message_truncated() {
        let f = AuthFailure::new(401, "unauthenticated", &"x".repeat(900));
        assert_eq!(f.message.chars().count(), 500);
        assert!(f.blocked_at(Utc::now()));
    }

    #[test]
    fn guard_mark_semantics_and_ttl() {
        let g = AuthGuard::default();
        let id = Uuid::new_v4();
        // 首次命中 → 状态变化（需落库）
        assert!(g.mark(id, AuthFailure::new(401, "unauthenticated", "bad token")));
        assert!(g.blocking(&id).is_some());
        // 同一失效期内重复命中 → 不再写库
        assert!(!g.mark(id, AuthFailure::new(401, "unauthenticated", "bad token")));
        // 错误码变化 → 视为新状态
        assert!(g.mark(id, AuthFailure::new(403, "permission_denied", "no access")));
        // 冷却到期（把 at 拨回 TTL 之前）→ 不再跳过，且下次命中重新落库
        let old = AuthFailure {
            at: Utc::now() - chrono::Duration::seconds(AUTH_FAIL_TTL_SECS + 1),
            ..AuthFailure::new(401, "unauthenticated", "stale")
        };
        g.mark(id, old.clone());
        assert!(g.blocking(&id).is_none());
        assert!(g.mark(id, AuthFailure::new(401, "unauthenticated", "again")));
        // 清除后不再命中
        assert!(g.clear(&id));
        assert!(!g.clear(&id));
        assert!(g.get(&id).is_none());
    }

    #[test]
    fn auth_state_read_from_extra() {
        let f = AuthFailure::new(401, "unauthenticated", "bad token");
        let extra = serde_json::json!({
            AUTH_STATE_KEY: serde_json::to_value(&f).unwrap(),
            "cli_emulation": false,
        });
        assert_eq!(auth_state_from_extra(&extra), Some(f.clone()));
        // 无该键 / 结构损坏 → None（不 panic）
        assert!(auth_state_from_extra(&serde_json::json!({"cli_emulation": true})).is_none());
        assert!(auth_state_from_extra(&serde_json::json!({AUTH_STATE_KEY: 1})).is_none());
    }
}
