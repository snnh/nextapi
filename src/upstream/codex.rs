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
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .ok()
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
    let get = |k: &str| {
        tokens
            .get(k)
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
    };

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
    let expires_at = exp_from_jwt(&access_token).unwrap_or_else(|| Utc::now().timestamp() + 3600);

    Ok(CodexOAuth {
        access_token,
        refresh_token,
        expires_at,
        account_id,
    })
}

// ---------------------------------------------------------------------------
// 官方客户端指纹模拟（预设默认开启；extra.codex_fingerprint 可覆盖/关闭）
// ---------------------------------------------------------------------------

/// 预设客户端标识（与 docs/codex-fingerprint.md 采集样本的 codex-tui 一致）。
pub const SIM_ORIGINATOR: &str = "codex-tui";
/// 预设 User-Agent：官方 codex-tui/Windows 形态
/// （`{originator}/{版本} ({OS} {OS版本}; {架构}) {终端}[ ({客户端名}; {版本})]`）。
pub const SIM_USER_AGENT: &str =
    "codex-tui/0.153.4 (Windows 10.0.26200; x86_64) WindowsTerminal (codex-tui; 0.153.4)";
/// HTTP 入口的 Responses 实验头值（老版本官方客户端使用，保留向后兼容）。
pub const OPENAI_BETA_HTTP: &str = "responses=experimental";
/// WS 握手头值（官方 responses_websockets beta）。
pub const OPENAI_BETA_WS: &str = "responses_websockets=2026-02-06";

/// Codex 上游传输方式（extra.codex_transport）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// 预设：优先 WebSocket，握手/首个事件失败回落 HTTP（与官方客户端一致）。
    Auto,
    /// 强制 WebSocket（失败按上游错误处理，不回落）。
    Ws,
    /// 只用 HTTP SSE。
    Http,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::Auto => "auto",
            Transport::Ws => "ws",
            Transport::Http => "http",
        }
    }
}

/// 解析上游传输方式：extra.codex_transport ∈ {auto, ws, http}，缺省/非法 = auto。
pub fn transport(extra: &serde_json::Value) -> Transport {
    match extra
        .get("codex_transport")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "ws" | "websocket" => Transport::Ws,
        "http" | "sse" => Transport::Http,
        _ => Transport::Auto,
    }
}

/// 指纹模拟配置（extra.codex_fingerprint：false 关闭；对象覆盖单项）。
#[derive(Debug, Clone)]
pub struct SimConfig {
    pub enabled: bool,
    pub user_agent: String,
    pub originator: String,
    /// 固定 installation id（缺省按上游 ID 确定性派生）。
    pub installation_id: Option<String>,
    /// 是否补齐 body 侧 Codex 专属字段（client_metadata/include/prompt_cache_key/…）。
    pub simulate_body: bool,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            user_agent: SIM_USER_AGENT.to_string(),
            originator: SIM_ORIGINATOR.to_string(),
            installation_id: None,
            simulate_body: true,
        }
    }
}

/// 读取上游指纹模拟配置（extra.codex_fingerprint）。
pub fn sim_config(extra: &serde_json::Value) -> SimConfig {
    let mut cfg = SimConfig::default();
    let Some(v) = extra.get("codex_fingerprint") else {
        return cfg;
    };
    if v.as_bool() == Some(false) {
        cfg.enabled = false;
        return cfg;
    }
    let Some(o) = v.as_object() else {
        return cfg;
    };
    if o.get("enabled").and_then(|x| x.as_bool()) == Some(false) {
        cfg.enabled = false;
    }
    if let Some(ua) = o
        .get("user_agent")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cfg.user_agent = ua.to_string();
    }
    if let Some(orig) = o
        .get("originator")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cfg.originator = orig.to_string();
    }
    if let Some(id) = o
        .get("installation_id")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| safe_id(s))
    {
        cfg.installation_id = Some(id.to_string());
    }
    if let Some(b) = o.get("simulate_body").and_then(|x| x.as_bool()) {
        cfg.simulate_body = b;
    }
    cfg
}

/// 幂等标识长度上限（与官方 uuid/短 label 量级一致，防止超长值污染请求头）。
const ID_MAX_LEN: usize = 64;

/// 可安全写入请求头/元数据的标识：短 ASCII 且不含分隔/控制字符。
fn safe_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= ID_MAX_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// 确定性 UUIDv7：前 48bit = seed_ms，其余位 = SHA-256(scope) 派生。
/// 用于会话/安装标识——同一 (上游, Key, 模型) 每次得到同一个值，
/// 保证上游粘性路由与提示缓存亲和。
fn derived_id(scope: &str, seed_ms: i64) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(scope.as_bytes());
    let mut b = [0u8; 16];
    let ms = (seed_ms.max(0) as u64) & 0xFFFF_FFFF_FFFF;
    b[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    b[6] = 0x70 | (h[0] & 0x0F);
    b[7] = h[1];
    b[8] = 0x80 | (h[2] & 0x3F);
    b[9..16].copy_from_slice(&h[3..10]);
    uuid::Uuid::from_bytes(b).to_string()
}

fn md_str(md: Option<&serde_json::Value>, key: &str) -> Option<String> {
    md?.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| safe_id(s))
        .map(str::to_string)
}

/// 请求身份：对齐官方 client_metadata（body 事实源）与兼容请求头。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub installation_id: String,
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub window_id: String,
    pub window_number: u32,
    pub context_window_id: String,
    pub turn_started_at_unix_ms: i64,
}

impl Identity {
    /// 解析请求身份：优先复用入站 body 的 Codex client_metadata（真实 CLI 透传），
    /// 缺失时按 (上游, scope) 确定性合成；`seed_ms` 为派生 UUIDv7 的时间部分。
    pub fn resolve(
        body: Option<&serde_json::Value>,
        upstream_id: Uuid,
        scope: &str,
        seed_ms: i64,
        cfg: &SimConfig,
    ) -> Self {
        let md = body.and_then(|b| b.get("client_metadata"));
        let installation_id = md_str(md, "x-codex-installation-id")
            .or_else(|| cfg.installation_id.clone())
            .unwrap_or_else(|| derived_id(&format!("install:{upstream_id}"), seed_ms));
        let session_id = md_str(md, "session_id")
            .or_else(|| md_str(md, "thread_id"))
            .or_else(|| {
                body.and_then(|b| b.get("prompt_cache_key"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| safe_id(s))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| derived_id(&format!("session:{scope}:{upstream_id}"), seed_ms));
        let thread_id = md_str(md, "thread_id").unwrap_or_else(|| session_id.clone());
        let turn_id = md_str(md, "turn_id").unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let window_id =
            md_str(md, "x-codex-window-id").unwrap_or_else(|| format!("{session_id}:0"));
        let context_window_id = md_str(md, "context_window_id")
            .unwrap_or_else(|| derived_id(&format!("ctx:{session_id}"), seed_ms));
        Self {
            installation_id,
            session_id,
            thread_id,
            turn_id,
            window_id,
            window_number: 0,
            context_window_id,
            turn_started_at_unix_ms: Utc::now().timestamp_millis(),
        }
    }

    /// x-codex-turn-metadata（JSON 字符串；官方客户端把它作为 client_metadata 的 canonical 值）。
    pub fn turn_metadata_json(&self) -> String {
        serde_json::json!({
            "installation_id": self.installation_id,
            "session_id": self.session_id,
            "thread_id": self.thread_id,
            "turn_id": self.turn_id,
            "window_id": self.window_id,
            "window_number": self.window_number,
            "context_window_id": self.context_window_id,
            "request_kind": "turn",
            "thread_source": "cli",
            "turn_started_at_unix_ms": self.turn_started_at_unix_ms,
        })
        .to_string()
    }

    /// body.client_metadata（扁平键 + canonical turn metadata）。
    pub fn client_metadata(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        let mut put = |k: &str, v: &str| {
            m.insert(k.to_string(), serde_json::Value::String(v.to_string()));
        };
        put("x-codex-installation-id", &self.installation_id);
        put("session_id", &self.session_id);
        put("thread_id", &self.thread_id);
        put("turn_id", &self.turn_id);
        put("x-codex-window-id", &self.window_id);
        put("context_window_id", &self.context_window_id);
        put("x-codex-turn-metadata", &self.turn_metadata_json());
        m
    }
}

// ---------------------------------------------------------------------------
// 请求整形
// ---------------------------------------------------------------------------

/// 注入 Codex 鉴权头（覆盖 apply_auth 可能写入的 Authorization）。
pub fn apply_auth_headers(
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
}

/// 指纹模拟头：entry 语义（不覆盖用户通过 overrides 已显式设置的头）。
pub fn apply_sim_headers(
    headers: &mut reqwest::header::HeaderMap,
    cfg: &SimConfig,
    ident: &Identity,
) {
    use reqwest::header::HeaderValue;
    if !cfg.enabled {
        return;
    }
    let mut put = |name: &'static str, value: String| {
        if let Ok(v) = HeaderValue::from_str(&value) {
            headers.entry(name).or_insert(v);
        }
    };
    put("user-agent", cfg.user_agent.clone());
    put("originator", cfg.originator.clone());
    put("session-id", ident.session_id.clone());
    put("thread-id", ident.thread_id.clone());
    put("x-client-request-id", ident.thread_id.clone());
    put("x-codex-window-id", ident.window_id.clone());
    put("x-codex-turn-metadata", ident.turn_metadata_json());
}

/// 注入 Codex HTTP 请求头：鉴权 + 实验头 + Accept + 指纹模拟。
pub fn apply_headers(
    headers: &mut reqwest::header::HeaderMap,
    access_token: &str,
    account_id: &str,
    cfg: &SimConfig,
    ident: &Identity,
) {
    use reqwest::header::{HeaderValue, ACCEPT};
    apply_auth_headers(headers, access_token, account_id);
    headers.insert("OpenAI-Beta", HeaderValue::from_static(OPENAI_BETA_HTTP));
    if !cfg.enabled {
        // 关闭模拟时保持历史行为：仅注入固定 originator。
        headers.insert("originator", HeaderValue::from_static(ORIGINATOR));
    }
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    apply_sim_headers(headers, cfg, ident);
}

/// 注入 Codex WebSocket 握手头：鉴权 + responses_websockets beta + 指纹模拟。
pub fn apply_ws_headers(
    headers: &mut reqwest::header::HeaderMap,
    access_token: &str,
    account_id: &str,
    cfg: &SimConfig,
    ident: &Identity,
) {
    use reqwest::header::HeaderValue;
    apply_auth_headers(headers, access_token, account_id);
    headers.insert("OpenAI-Beta", HeaderValue::from_static(OPENAI_BETA_WS));
    apply_sim_headers(headers, cfg, ident);
}

/// 请求体整形：Codex 后端仅支持流式、无状态。
/// - 强制 stream=true / store=false；instructions 缺失时补默认（后端必填校验）；
/// - 指纹模拟开启时补齐官方客户端字段（已有值一律保留）：
///   client_metadata / include / prompt_cache_key / reasoning / parallel_tool_calls / tool_choice。
pub fn prepare_body(
    body: &serde_json::Value,
    ident: &Identity,
    cfg: &SimConfig,
) -> serde_json::Value {
    let mut b = body.clone();
    let Some(o) = b.as_object_mut() else {
        return b;
    };
    o.insert("stream".into(), serde_json::Value::Bool(true));
    o.insert("store".into(), serde_json::Value::Bool(false));
    if o.get("instructions").and_then(|v| v.as_str()).is_none() {
        o.insert(
            "instructions".into(),
            serde_json::Value::String(DEFAULT_INSTRUCTIONS.into()),
        );
    }
    if !cfg.enabled || !cfg.simulate_body {
        return b;
    }
    let has_client_metadata = o
        .get("client_metadata")
        .and_then(|v| v.as_object())
        .map(|m| !m.is_empty())
        .unwrap_or(false);
    if !has_client_metadata {
        o.insert(
            "client_metadata".into(),
            serde_json::Value::Object(ident.client_metadata()),
        );
    }
    if !o.contains_key("include") {
        o.insert(
            "include".into(),
            serde_json::json!(["reasoning.encrypted_content"]),
        );
    }
    if o.get("prompt_cache_key").and_then(|v| v.as_str()).is_none() {
        o.insert(
            "prompt_cache_key".into(),
            serde_json::Value::String(ident.session_id.clone()),
        );
    }
    if !o.contains_key("reasoning") {
        o.insert(
            "reasoning".into(),
            serde_json::json!({"effort": "medium", "summary": "auto"}),
        );
    }
    if !o.contains_key("parallel_tool_calls") {
        o.insert("parallel_tool_calls".into(), serde_json::Value::Bool(true));
    }
    if !o.contains_key("tool_choice") {
        o.insert(
            "tool_choice".into(),
            serde_json::Value::String("auto".into()),
        );
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
    completed
        .ok_or_else(|| super::UpstreamError::BodyRead("codex 流缺少 response.completed 帧".into()))
}

// ---------------------------------------------------------------------------
// Token 刷新（single-flight + DB 回写 + 快照刷新）
// ---------------------------------------------------------------------------

/// 取有效 access_token：未临期直接返回；否则 per-upstream 加锁刷新。
/// 返回 (access_token, account_id)。失败返回错误描述（调用方走熔断/故障转移）。
pub async fn ensure_token(
    state: &Arc<AppState>,
    up: &UpstreamRow,
) -> Result<(String, String), String> {
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
        let mut locks = state.codex_locks.lock().unwrap_or_else(|p| p.into_inner());
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
            parse_auth_json("{\"tokens\":{\"access_token\":\"a\",\"refresh_token\":\"r\"}}")
                .is_err()
        );
    }

    #[test]
    fn prepare_body_forces_stream_store_instructions() {
        let cfg = SimConfig::default();
        let ident = test_identity();
        let out = prepare_body(
            &serde_json::json!({"model":"gpt-5-codex","input":"hi"}),
            &ident,
            &cfg,
        );
        assert_eq!(out["stream"], true);
        assert_eq!(out["store"], false);
        assert_eq!(out["instructions"], DEFAULT_INSTRUCTIONS);
        // 已有 instructions 不覆盖
        let out2 = prepare_body(
            &serde_json::json!({"instructions":"custom","stream":false}),
            &ident,
            &cfg,
        );
        assert_eq!(out2["instructions"], "custom");
        assert_eq!(out2["stream"], true);
    }

    // -----------------------------------------------------------------------
    // 指纹模拟
    // -----------------------------------------------------------------------

    fn test_identity() -> Identity {
        Identity::resolve(
            None,
            Uuid::nil(),
            "key:model",
            1_700_000_000_000,
            &SimConfig::default(),
        )
    }

    #[test]
    fn transport_config_parsing() {
        assert_eq!(transport(&serde_json::json!({})), Transport::Auto);
        assert_eq!(
            transport(&serde_json::json!({"codex_transport":"ws"})),
            Transport::Ws
        );
        assert_eq!(
            transport(&serde_json::json!({"codex_transport":"HTTP"})),
            Transport::Http
        );
        assert_eq!(
            transport(&serde_json::json!({"codex_transport":"bogus"})),
            Transport::Auto
        );
        assert_eq!(Transport::Http.as_str(), "http");
    }

    #[test]
    fn sim_config_defaults_and_overrides() {
        let d = sim_config(&serde_json::json!({}));
        assert!(d.enabled && d.simulate_body);
        assert_eq!(d.originator, SIM_ORIGINATOR);
        assert!(d.user_agent.starts_with("codex-tui/0.153.4 ("));

        let off = sim_config(&serde_json::json!({"codex_fingerprint": false}));
        assert!(!off.enabled);

        let custom = sim_config(&serde_json::json!({
            "codex_fingerprint": {
                "enabled": false,
                "user_agent": "codex_cli_rs/1.2.3 (Linux 6.1; x86_64) xterm",
                "originator": "codex_vscode",
                "installation_id": "install-abc",
                "simulate_body": false
            }
        }));
        assert!(!custom.enabled && !custom.simulate_body);
        assert_eq!(custom.originator, "codex_vscode");
        assert_eq!(
            custom.user_agent,
            "codex_cli_rs/1.2.3 (Linux 6.1; x86_64) xterm"
        );
        assert_eq!(custom.installation_id.as_deref(), Some("install-abc"));
    }

    #[test]
    fn identity_synthesizes_stable_uuidv7() {
        let cfg = SimConfig::default();
        let up = Uuid::from_u128(42);
        let a = Identity::resolve(None, up, "key1:gpt-5-codex", 1_700_000_000_000, &cfg);
        let b = Identity::resolve(None, up, "key1:gpt-5-codex", 1_700_000_000_000, &cfg);
        let c = Identity::resolve(None, up, "key2:gpt-5-codex", 1_700_000_000_000, &cfg);
        assert_eq!(a.session_id, b.session_id, "同 scope 派生稳定");
        assert_ne!(a.session_id, c.session_id, "不同 scope 派生不同");
        assert_eq!(a.thread_id, a.session_id);
        assert_eq!(a.window_id, format!("{}:0", a.session_id));
        for id in [
            &a.session_id,
            &a.installation_id,
            &a.context_window_id,
            &a.turn_id,
        ] {
            let parsed = Uuid::parse_str(id).expect("合法 UUID");
            assert_eq!(parsed.get_version_num(), 7, "{id} 应为 UUIDv7");
        }
        // turn_id 为请求级随机（v7 时间有序，不与其他请求共享）
        assert_ne!(a.turn_id, b.turn_id);
    }

    #[test]
    fn identity_reuses_inbound_metadata() {
        let body = serde_json::json!({
            "prompt_cache_key": "ignored-when-md-present",
            "client_metadata": {
                "session_id": "01a09e1e-c452-7220-8fca-831f1242644e",
                "thread_id": "01a09e1e-c452-7220-8fca-831f1242644e",
                "turn_id": "01a09e1e-c4a2-7491-bcbd-d9784771f686",
                "x-codex-installation-id": "e4f07884-bdee-4322-9d85-3c497393d7c0",
                "x-codex-window-id": "01a09e1e-c452-7220-8fca-831f1242644e:0"
            }
        });
        let ident = Identity::resolve(
            Some(&body),
            Uuid::nil(),
            "key:model",
            0,
            &SimConfig::default(),
        );
        assert_eq!(ident.session_id, "01a09e1e-c452-7220-8fca-831f1242644e");
        assert_eq!(
            ident.installation_id,
            "e4f07884-bdee-4322-9d85-3c497393d7c0"
        );
        assert_eq!(ident.turn_id, "01a09e1e-c4a2-7491-bcbd-d9784771f686");
        // 非法/含换行的元数据不得进入请求头
        let bad = serde_json::json!({
            "client_metadata": {"session_id": "bad\nvalue"}
        });
        let ident2 = Identity::resolve(Some(&bad), Uuid::nil(), "k:m", 0, &SimConfig::default());
        assert!(Uuid::parse_str(&ident2.session_id).is_ok());
    }

    #[test]
    fn prepare_body_simulates_codex_fields() {
        let cfg = SimConfig::default();
        let ident = test_identity();
        let out = prepare_body(
            &serde_json::json!({"model":"gpt-5-codex","input":"hi"}),
            &ident,
            &cfg,
        );
        assert_eq!(out["include"][0], "reasoning.encrypted_content");
        assert_eq!(out["prompt_cache_key"], ident.session_id);
        assert_eq!(out["reasoning"]["effort"], "medium");
        assert_eq!(out["parallel_tool_calls"], true);
        assert_eq!(out["tool_choice"], "auto");
        assert_eq!(
            out["client_metadata"]["session_id"],
            serde_json::Value::String(ident.session_id.clone())
        );
        // x-codex-turn-metadata 是 JSON 字符串（双重编码），与 session 一致
        let tm: serde_json::Value = serde_json::from_str(
            out["client_metadata"]["x-codex-turn-metadata"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(tm["session_id"], ident.session_id);
        assert_eq!(tm["request_kind"], "turn");

        // 已有值一律保留（透传优先）
        let given = serde_json::json!({
            "include": ["custom.include"],
            "reasoning": {"effort":"high","summary":"detailed"},
            "tool_choice": "required",
            "client_metadata": {"session_id":"keep-me"},
            "prompt_cache_key": "keep-key",
            "parallel_tool_calls": false
        });
        let kept = prepare_body(&given, &ident, &cfg);
        assert_eq!(kept["include"][0], "custom.include");
        assert_eq!(kept["reasoning"]["effort"], "high");
        assert_eq!(kept["tool_choice"], "required");
        assert_eq!(kept["prompt_cache_key"], "keep-key");
        assert_eq!(kept["parallel_tool_calls"], false);
        assert_eq!(kept["client_metadata"]["session_id"], "keep-me");

        // 关闭模拟：只做 stream/store/instructions 整形
        let off = SimConfig {
            enabled: false,
            ..SimConfig::default()
        };
        let plain = prepare_body(&serde_json::json!({"input":"hi"}), &ident, &off);
        assert!(plain.get("client_metadata").is_none());
        assert!(plain.get("include").is_none());
    }

    #[test]
    fn sim_headers_do_not_clobber_user_overrides() {
        use reqwest::header::{HeaderMap, HeaderValue};
        let cfg = SimConfig::default();
        let ident = test_identity();
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("my-custom-ua"));
        apply_headers(&mut headers, "tok", "acct", &cfg, &ident);
        assert_eq!(headers["user-agent"], "my-custom-ua");
        assert_eq!(headers["originator"], SIM_ORIGINATOR);
        assert_eq!(headers["authorization"], "Bearer tok");
        assert_eq!(headers["chatgpt-account-id"], "acct");
        assert_eq!(headers["openai-beta"], OPENAI_BETA_HTTP);
        assert_eq!(headers["session-id"], ident.session_id);
        assert_eq!(headers["thread-id"], ident.thread_id);
        assert_eq!(headers["x-client-request-id"], ident.thread_id);
        assert_eq!(headers["x-codex-window-id"], ident.window_id);
        // WS 握手头的 beta 值不同
        let mut ws = HeaderMap::new();
        apply_ws_headers(&mut ws, "tok", "acct", &cfg, &ident);
        assert_eq!(ws["openai-beta"], OPENAI_BETA_WS);
        assert!(ws.get("accept").is_none());
    }
}
