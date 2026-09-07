//! Codex（ChatGPT 账户 OAuth）渠道支持。
//!
//! - 凭证来源：管理员粘贴 codex CLI 的 `~/.codex/auth.json`（或内置 PKCE 授权流换发），
//!   解析出 access_token / refresh_token / expires_at / account_id，AES-256-GCM 加密存
//!   `upstreams.oauth_enc`，内存快照持明文（同 api_key_plain 语义）；
//! - 调用形态：上游只走 OpenAI Responses（`{base}/responses`），强制 `stream=true`
//!   （Codex 后端仅流式）、`store=false`、`instructions` 缺省必填；请求头注入
//!   `Authorization: Bearer`、`ChatGPT-Account-Id`、`OpenAI-Beta: responses=experimental`、
//!   `originator`；非流式入口由网关聚合 SSE 的 `response.completed` 帧还原完整响应；
//! - 刷新：access_token 到期前 60s 触发刷新（per-upstream single-flight），成功即回写
//!   DB（refresh_token 轮换语义）并刷新快照；失败抛错走网关统一熔断/故障转移。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::UpstreamRow;
use crate::state::AppState;

/// codex CLI 的公开 OAuth client_id（社区通行用法）。
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// PKCE 授权端点（Phase 2 内置授权流使用）
#[allow(dead_code)]
pub const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const ORIGINATOR: &str = "codex_cli_rs";
/// Codex 后端要求 instructions 必填（缺失 400 Instructions are required）。
pub const DEFAULT_INSTRUCTIONS: &str = "You are a helpful coding assistant.";
/// access_token 距过期不足该秒数即刷新。
const REFRESH_SKEW_SECS: i64 = 60;

/// Codex OAuth 凭证（加密存 oauth_enc；内存快照持明文）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexOAuth {
    pub access_token: String,
    pub refresh_token: String,
    /// access_token 过期时间（unix 秒）
    pub expires_at: i64,
    pub account_id: String,
}

// ---------------------------------------------------------------------------
// auth.json 解析与 JWT claim 提取
// ---------------------------------------------------------------------------

/// base64url（无填充）解码。
fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).ok()
}

/// 解 JWT payload（不验签——仅提取 claim，凭证本身来自用户粘贴/官方端点）。
fn jwt_payload(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = b64url_decode(payload)?;
    serde_json::from_slice(&bytes).ok()
}

/// 从 id_token 提取 chatgpt account id。
fn account_id_from_jwt(id_token: &str) -> Option<String> {
    let p = jwt_payload(id_token)?;
    p.get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// 从 access_token JWT 提取 exp（unix 秒）。
fn exp_from_jwt(access_token: &str) -> Option<i64> {
    jwt_payload(access_token)?.get("exp")?.as_i64()
}

/// 解析 codex CLI `~/.codex/auth.json` 原文为内部凭证。
/// 结构：`{tokens:{id_token, access_token, refresh_token, account_id?}}`；
/// account_id 缺省时从 id_token 的 auth claim 提取；过期时间取 access_token 的 exp，
/// 非 JWT（少见）时保守按 1 小时计。
pub fn parse_auth_json(raw: &str) -> Result<CodexOAuth, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("auth.json 不是合法 JSON: {e}"))?;
    let tokens = v.get("tokens").ok_or("auth.json 缺少 tokens 字段")?;
    let get = |k: &str| tokens.get(k).and_then(|x| x.as_str()).filter(|s| !s.is_empty());

    let access_token = get("access_token")
        .ok_or("tokens.access_token 缺失")?
        .to_string();
    let refresh_token = get("refresh_token")
        .ok_or("tokens.refresh_token 缺失")?
        .to_string();
    let account_id = get("account_id")
        .map(str::to_string)
        .or_else(|| get("id_token").and_then(account_id_from_jwt))
        .ok_or("无法确定 account_id（tokens.account_id 与 id_token claim 均缺失）")?;
    let expires_at = exp_from_jwt(&access_token)
        .unwrap_or_else(|| Utc::now().timestamp() + 3600);

    Ok(CodexOAuth {
        access_token,
        refresh_token,
        expires_at,
        account_id,
    })
}

// ---------------------------------------------------------------------------
// 请求整形
// ---------------------------------------------------------------------------

/// 注入 Codex 必需请求头（覆盖 apply_auth 可能写入的 Authorization）。
pub fn apply_headers(
    headers: &mut reqwest::header::HeaderMap,
    access_token: &str,
    account_id: &str,
) {
    use reqwest::header::{HeaderValue, AUTHORIZATION};
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {access_token}")) {
        headers.insert(AUTHORIZATION, v);
    }
    if let Ok(v) = HeaderValue::from_str(account_id) {
        headers.insert("chatgpt-account-id", v);
    }
    headers.insert(
        "OpenAI-Beta",
        HeaderValue::from_static("responses=experimental"),
    );
    headers.insert("originator", HeaderValue::from_static(ORIGINATOR));
    headers.insert(
        reqwest::header::ACCEPT,
        HeaderValue::from_static("text/event-stream"),
    );
}

/// 请求体整形：Codex 后端仅支持流式、无状态。
/// 强制 stream=true / store=false；instructions 缺失时补默认（后端必填校验）。
pub fn prepare_body(body: &serde_json::Value) -> serde_json::Value {
    let mut b = body.clone();
    let obj = b.as_object_mut();
    if let Some(o) = obj {
        o.insert("stream".into(), serde_json::Value::Bool(true));
        o.insert("store".into(), serde_json::Value::Bool(false));
        if o.get("instructions").and_then(|v| v.as_str()).is_none() {
            o.insert(
                "instructions".into(),
                serde_json::Value::String(DEFAULT_INSTRUCTIONS.into()),
            );
        }
    }
    b
}

/// 非流式入口聚合：读尽 SSE，取 `response.completed` 帧内的完整 response 对象。
pub async fn aggregate_stream_to_json(
    resp: reqwest::Response,
) -> Result<serde_json::Value, super::UpstreamError> {
    use futures::StreamExt;
    let mut parser = crate::protocol::sse::SseParser::new();
    let mut stream = resp.bytes_stream();
    let mut completed: Option<serde_json::Value> = None;

    let handle = |data: String, completed: &mut Option<serde_json::Value>| {
        if completed.is_some() {
            return;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
            if v.get("type").and_then(|t| t.as_str()) == Some("response.completed") {
                if let Some(r) = v.get("response") {
                    *completed = Some(r.clone());
                }
            }
        }
    };

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| super::UpstreamError::BodyRead(e.to_string()))?;
        for ev in parser.feed(&bytes) {
            handle(ev, &mut completed);
        }
        if completed.is_some() {
            break;
        }
    }
    if completed.is_none() {
        for ev in parser.finish() {
            handle(ev, &mut completed);
        }
    }
    completed.ok_or_else(|| {
        super::UpstreamError::BodyRead("codex 流缺少 response.completed 帧".into())
    })
}

// ---------------------------------------------------------------------------
// Token 刷新（single-flight + DB 回写 + 快照刷新）
// ---------------------------------------------------------------------------

/// 取有效 access_token：未临期直接返回；否则 per-upstream 加锁刷新。
/// 返回 (access_token, account_id)。失败返回错误描述（调用方走熔断/故障转移）。
pub async fn ensure_token(state: &Arc<AppState>, up: &UpstreamRow) -> Result<(String, String), String> {
    let now = Utc::now().timestamp();
    if let Some(o) = up.oauth_plain.as_ref() {
        if o.expires_at - now > REFRESH_SKEW_SECS {
            return Ok((o.access_token.clone(), o.account_id.clone()));
        }
    } else {
        return Err("Codex 渠道未配置 OAuth 凭证（请粘贴 auth.json）".into());
    }

    // per-upstream 锁：并发请求只刷新一次
    let lock = {
        let mut locks = state
            .codex_locks
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        locks
            .entry(up.id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let _guard = lock.lock().await;

    // 双重检查：等锁期间可能已被其他请求刷新
    let cur = {
        let snap = state.cache.snapshot();
        snap.upstreams.get(&up.id).cloned()
    };
    let oauth = cur
        .as_ref()
        .and_then(|u| u.oauth_plain.clone())
        .ok_or("Codex 渠道未配置 OAuth 凭证")?;
    let now = Utc::now().timestamp();
    if oauth.expires_at - now > REFRESH_SKEW_SECS {
        return Ok((oauth.access_token, oauth.account_id));
    }

    refresh_token(state, up, &oauth).await
}

/// 强制刷新（管理员手动触发 / 临期自动调用共用）。
pub async fn refresh_token(
    state: &Arc<AppState>,
    up: &UpstreamRow,
    oauth: &CodexOAuth,
) -> Result<(String, String), String> {
    let client = {
        let snap = state.cache.snapshot();
        let hot = state.hot.load();
        state
            .client_pools
            .client_for(up, &snap, &hot.proxy.default_proxy_id)
    };
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", CLIENT_ID),
            ("refresh_token", oauth.refresh_token.as_str()),
        ])
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("token 刷新请求失败: {e}"))?;
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("token 刷新响应解析失败: {e}"))?;
    if !(200..300).contains(&status) {
        let snippet = body.to_string();
        return Err(format!(
            "token 刷新被拒 HTTP {status}: {}",
            snippet.chars().take(200).collect::<String>()
        ));
    }

    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("刷新响应缺少 access_token")?
        .to_string();
    // refresh_token 轮换：响应携带即更新，否则沿用旧的
    let refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| oauth.refresh_token.clone());
    let account_id = body
        .get("id_token")
        .and_then(|v| v.as_str())
        .and_then(account_id_from_jwt)
        .unwrap_or_else(|| oauth.account_id.clone());
    let expires_at = body
        .get("expires_in")
        .and_then(|v| v.as_i64())
        .map(|secs| Utc::now().timestamp() + secs)
        .or_else(|| exp_from_jwt(&access_token))
        .unwrap_or_else(|| Utc::now().timestamp() + 3600);

    let next = CodexOAuth {
        access_token,
        refresh_token,
        expires_at,
        account_id,
    };
    let enc = state
        .crypto
        .encrypt(&serde_json::to_string(&next).map_err(|e| e.to_string())?)
        .map_err(|e| format!("凭证加密失败: {e}"))?;
    sqlx::query("UPDATE upstreams SET oauth_enc=$1, updated_at=now() WHERE id=$2")
        .bind(&enc)
        .bind(up.id)
        .execute(&state.db)
        .await
        .map_err(|e| format!("凭证回写失败: {e}"))?;
    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(|e| format!("快照刷新失败: {e}"))?;

    Ok((next.access_token, next.account_id))
}

/// AppState 用的锁表类型。
pub type CodexLockTable = std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_jwt(payload: &serde_json::Value) -> String {
        use base64::Engine;
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!(
            "header.{}.sig",
            enc.encode(serde_json::to_string(payload).unwrap())
        )
    }

    #[test]
    fn parse_auth_json_full() {
        let id_token = fake_jwt(&serde_json::json!({
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct-123"}
        }));
        let access = fake_jwt(&serde_json::json!({"exp": 2_000_000_000}));
        let raw = serde_json::json!({
            "tokens": {
                "id_token": id_token,
                "access_token": access,
                "refresh_token": "rt-abc"
            }
        })
        .to_string();
        let o = parse_auth_json(&raw).unwrap();
        assert_eq!(o.account_id, "acct-123");
        assert_eq!(o.refresh_token, "rt-abc");
        assert_eq!(o.expires_at, 2_000_000_000);
    }

    #[test]
    fn parse_auth_json_explicit_account() {
        let raw = serde_json::json!({
            "tokens": {
                "access_token": "opaque-token",
                "refresh_token": "rt",
                "account_id": "acct-x"
            }
        })
        .to_string();
        let o = parse_auth_json(&raw).unwrap();
        assert_eq!(o.account_id, "acct-x");
        // 非 JWT access_token：保守 1 小时
        assert!(o.expires_at > Utc::now().timestamp());
    }

    #[test]
    fn parse_auth_json_missing_fields() {
        assert!(parse_auth_json("{}").is_err());
        assert!(parse_auth_json("{\"tokens\":{}}").is_err());
        assert!(
            parse_auth_json("{\"tokens\":{\"access_token\":\"a\",\"refresh_token\":\"r\"}}").is_err()
        );
    }

    #[test]
    fn prepare_body_forces_stream_store_instructions() {
        let out = prepare_body(&serde_json::json!({"model":"gpt-5-codex","input":"hi"}));
        assert_eq!(out["stream"], true);
        assert_eq!(out["store"], false);
        assert_eq!(out["instructions"], DEFAULT_INSTRUCTIONS);
        // 已有 instructions 不覆盖
        let out2 = prepare_body(&serde_json::json!({"instructions":"custom","stream":false}));
        assert_eq!(out2["instructions"], "custom");
        assert_eq!(out2["stream"], true);
    }
}
