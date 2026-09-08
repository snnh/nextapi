//! 单管理员认证：用户名/密码（argon2）→ JWT 短期 access token；登录防爆破限速。
//!
//! 路由：
//! - `POST /login`：公开，按 username 做滑动窗口限速，失败统一返回 401；
//! - `GET /me`、`PUT /password`：需登录（从 Authorization: Bearer 解析并校验 JWT）。
//!
//! `require_admin` 由 `main.rs` 通过 `middleware::from_fn_with_state` 挂到 `/api/*`，
//! 校验通过后把用户名写入请求扩展（`AdminUsername`），供下方 handler 提取。
//! 明确不做：用户注册、多用户、角色与分组（PLAN.md §1.3）。
//!
//! 全部 SQL 使用运行时校验（sqlx::query），不使用 query! 宏（无编译期数据库）。

pub mod totp;

use argon2::password_hash::{PasswordHash, PasswordVerifier};
use argon2::Argon2;
use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::request::Parts,
    http::HeaderMap,
    middleware::Next,
    response::Response,
    routing::{get, post, put},
    Extension, Json, Router,
};
use chrono::Utc;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// 短期 access token 有效期（秒）：1 小时。
pub const TOKEN_TTL_SECS: u64 = 3600;

/// 登录防爆破滑动窗口（秒）：1 分钟。
const RATE_WINDOW_SECS: u64 = 60;

/// 限速器最大用户名条目数：超过先淘汰闲置条目，仍满则拒绝新用户名（防随机用户名内存 DoS）。
const MAX_RATE_ENTRIES: usize = 10_000;

/// 限速条目闲置淘汰时长：超过该时长未再尝试登录即删除（窗口仅 60s，10 分钟绰绰有余）。
const IDLE_EVICT_SECS: u64 = 600;

// 当前请求客户端 IP：由 require_admin 中间件在 task 作用域内设置，
// 供审计自动携带；登录端点不走中间件，显式传参。
tokio::task_local! {
    static CURRENT_CLIENT_IP: Option<std::net::IpAddr>;
}

/// JWT 载荷：sub（用户名）+ exp + iat。
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
    iat: usize,
    #[serde(default)]
    session_version: i32,
}

#[derive(Clone)]
pub struct JwtService {
    secret: String,
    ttl_secs: u64,
}

impl JwtService {
    pub fn new(secret: String) -> Self {
        Self {
            secret,
            ttl_secs: TOKEN_TTL_SECS,
        }
    }

    /// 签发管理员 token（HS256）。
    pub fn issue(&self, username: &str, session_version: i32) -> anyhow::Result<String> {
        let now = Utc::now().timestamp().max(0) as usize;
        let claims = Claims {
            sub: username.to_string(),
            exp: now + self.ttl_secs as usize,
            iat: now,
            session_version,
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )?;
        Ok(token)
    }

    /// 校验 token，返回主体（用户名）。
    #[cfg(test)]
    pub fn verify(&self, token: &str) -> anyhow::Result<String> {
        Ok(self.verify_claims(token)?.sub)
    }

    fn verify_claims(&self, token: &str) -> anyhow::Result<Claims> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_aud = false; // 我们不用 aud 声明
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &validation,
        )?;
        Ok(data.claims)
    }
}

/// 已认证管理员用户名，由 `require_admin` 中间件注入请求扩展。
#[derive(Clone, Debug)]
pub struct AdminUsername(pub String);

impl<S> FromRequestParts<S> for AdminUsername
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AdminUsername>()
            .cloned()
            .ok_or(ApiError::Unauthorized)
    }
}

/// 认证路由：POST /login（公开，限速）、GET /me、PUT /password（受保护）。
///
/// 注意：`router()` 无参且每次调用新建，无法在此处持有 State 实例，
/// 因此 `/me`、`/password` 改为在 handler 内部自行校验 Bearer（见 `current_username`）。
/// 登录防爆破由模块级 `static` + `OnceLock` 持有滑窗状态。
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/login", post(login))
        .route("/me", get(me))
        .route("/totp", get(totp_status))
        .route("/totp/setup", post(totp_setup))
        .route("/totp/enable", post(totp_enable))
        .route("/totp/disable", post(totp_disable))
        .route("/password", put(change_password))
}

/// 登录请求体。
#[derive(Debug, Deserialize)]
struct LoginReq {
    username: String,
    password: String,
    /// 已启用 TOTP 时必填的二次验证码（6 位数字）。
    #[serde(default)]
    totp_code: Option<String>,
}

/// 修改密码请求体。
#[derive(Debug, Deserialize)]
struct ChangePasswordReq {
    old_password: String,
    new_password: String,
}

/// POST /login：校验用户名/密码，签发 JWT。
async fn login(
    State(state): State<Arc<AppState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<LoginReq>,
) -> ApiResult<Json<serde_json::Value>> {
    // 1. 防爆破：按 username 的 1 分钟滑动窗口限速
    // M15：审计 IP 结合 TCP 对端（ConnectInfo）判定——仅对端命中 trusted_proxies
    // 时才解析转发头，否则一律使用对端 IP（转发头视为可伪造，直接忽略）。
    let ip = client_ip(&state, &headers, peer.map(|Extension(c)| c.0.ip()));
    let limit = state.hot.load().gateway.admin_login_rate_limit_per_min;
    // 限速键只取 username：即使 M15 已校验对端可信性，XFF 链中不可信段的内容
    // 仍由客户端控制，掺入 IP 会使限速可被随机 XFF 绕过（发布审阅 H1）。
    // 单管理员场景下 username 维度已足够防爆破；IP 仍记录进审计。
    let rate_key = body.username.trim().to_ascii_lowercase();
    if !login_rate_limiter().check(&rate_key, limit) {
        return Err(ApiError::RateLimited);
    }

    // 2. 查用户；查不到与密码错误统一返回 401（不区分）。
    //    用户不存在不写审计（防随机用户名刷爆审计表）；仅密码错误写失败审计，
    //    写入速率受 per-username 限速约束。
    let row = sqlx::query(
        "SELECT password_hash, session_version, totp_enabled, totp_secret_enc, last_totp_step FROM admin_users WHERE username = $1",
    )
    .bind(&body.username)
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::Unauthorized);
    };
    let hash: String = row.get("password_hash");
    let session_version: i32 = row.try_get("session_version").unwrap_or(0);

    // 3. argon2 校验密码
    if !verify_password(&hash, &body.password) {
        let _ = audit(
            &state,
            &body.username,
            "auth.login_failed",
            "admin",
            Some(&body.username),
            serde_json::json!({ "reason": "bad_password" }),
            ip,
        )
        .await;
        return Err(ApiError::Unauthorized);
    }

    // 3.5 TOTP 二次验证（已启用时）：缺码返回 totp_required（前端据此弹出验证码输入），
    //     错码返回 totp_invalid 并写审计；尝试均受 per-username+IP 登录限速约束。
    let totp_enabled: bool = row.try_get("totp_enabled").unwrap_or(false);
    if totp_enabled {
        let code = body.totp_code.as_deref().unwrap_or("").trim().to_string();
        if code.is_empty() {
            return Err(ApiError::TotpRequired);
        }
        let secret = row
            .try_get::<Option<String>, _>("totp_secret_enc")
            .ok()
            .flatten()
            .and_then(|enc| state.crypto.decrypt(&enc).ok())
            .and_then(|b32| totp::secret_from_base32(&b32));
        let now = Utc::now().timestamp().max(0) as u64;
        let step = secret.as_deref().and_then(|s| totp::verify(s, &code, now));
        // 防重放（RFC 6238 §5.2）：命中步必须大于上次消费步，且原子占用——
        // 并发/重放同码只会有一个请求把 last_totp_step 推进成功。
        let consumed = match step {
            Some(st) => {
                let r = sqlx::query(
                    "UPDATE admin_users SET last_totp_step = $2                      WHERE username = $1 AND (last_totp_step IS NULL OR last_totp_step < $2)",
                )
                .bind(&body.username)
                .bind(st)
                .execute(&state.db)
                .await?;
                r.rows_affected() == 1
            }
            None => false,
        };
        if !consumed {
            let _ = audit(
                &state,
                &body.username,
                "auth.login_failed",
                "admin",
                Some(&body.username),
                serde_json::json!({ "reason": "bad_totp" }),
                ip,
            )
            .await;
            return Err(ApiError::TotpInvalid);
        }
    }

    // 4. 签发 JWT，写登录成功审计
    let token = state
        .jwt
        .issue(&body.username, session_version)
        .map_err(ApiError::internal)?;
    audit(
        &state,
        &body.username,
        "auth.login",
        "admin",
        Some(&body.username),
        serde_json::json!({}),
        ip,
    )
    .await?;
    Ok(Json(
        serde_json::json!({ "token": token, "username": body.username }),
    ))
}

/// GET /me：返回当前登录用户名。
async fn me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers).await?;
    Ok(Json(serde_json::json!({ "username": username })))
}

/// PUT /password：校验旧密码后更新为新密码，并写审计。
async fn change_password(
    State(state): State<Arc<AppState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers).await?;
    sensitive_rate_check(&state, &username)?;
    let ip = client_ip(&state, &headers, peer.map(|Extension(c)| c.0.ip()));

    // 强度校验（review P4）：≥8 字符且不得与旧密码相同（避免误操作/弱口令）
    if body.new_password.chars().count() < 8 {
        return Err(ApiError::bad_request("新密码至少 8 个字符"));
    }
    if body.new_password == body.old_password {
        return Err(ApiError::bad_request("新密码不能与原密码相同"));
    }

    // 查用户（token 有效但用户可能已被删）
    let row =
        sqlx::query("SELECT password_hash, session_version FROM admin_users WHERE username = $1")
            .bind(&username)
            .fetch_optional(&state.db)
            .await?;
    let Some(row) = row else {
        return Err(ApiError::Unauthorized);
    };
    let hash: String = row.get("password_hash");

    // 校验旧密码
    if !verify_password(&hash, &body.old_password) {
        return Err(ApiError::bad_request("原密码不正确"));
    }

    // argon2 哈希新密码并更新
    let new_hash = hash_password(&body.new_password).map_err(ApiError::internal)?;
    sqlx::query("UPDATE admin_users SET password_hash = $1, session_version = session_version + 1 WHERE username = $2")
        .bind(&new_hash)
        .bind(&username)
        .execute(&state.db)
        .await?;

    audit(
        &state,
        &username,
        "password.change",
        "admin",
        Some(&username),
        serde_json::json!({}),
        ip,
    )
    .await?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// TOTP 二次验证管理（均需登录；handler 内自验 JWT）
// ---------------------------------------------------------------------------

/// GET /totp：{enabled, pending}（pending = 已生成机密但未确认启用）。
async fn totp_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers).await?;
    let row =
        sqlx::query("SELECT totp_enabled, totp_secret_enc FROM admin_users WHERE username = $1")
            .bind(&username)
            .fetch_optional(&state.db)
            .await?
            .ok_or(ApiError::Unauthorized)?;
    let enabled: bool = row.try_get("totp_enabled").unwrap_or(false);
    let has_secret = row
        .try_get::<Option<String>, _>("totp_secret_enc")
        .ok()
        .flatten()
        .is_some();
    Ok(Json(serde_json::json!({
        "enabled": enabled,
        "pending": has_secret && !enabled,
    })))
}

/// POST /totp/setup：生成新机密（pending 态），返回 base32 机密与 otpauth URL。
/// 机密仅本次展示；已启用时须先禁用才能重新设置。
#[derive(Debug, Deserialize)]
struct TotpSetupReq {
    /// 当前密码——setup 可覆盖 pending 机密，必须验密防持会话者锁死管理员（发布审阅 M5）。
    password: String,
}

async fn totp_setup(
    State(state): State<Arc<AppState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<TotpSetupReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers).await?;
    sensitive_rate_check(&state, &username)?;
    let ip = client_ip(&state, &headers, peer.map(|Extension(c)| c.0.ip()));
    let row =
        sqlx::query("SELECT password_hash, totp_enabled FROM admin_users WHERE username = $1")
            .bind(&username)
            .fetch_optional(&state.db)
            .await?
            .ok_or(ApiError::Unauthorized)?;
    let hash: String = row.get("password_hash");
    if !verify_password(&hash, &body.password) {
        return Err(ApiError::bad_request("密码不正确"));
    }
    if row.try_get::<bool, _>("totp_enabled").unwrap_or(false) {
        return Err(ApiError::Conflict(
            "TOTP 已启用，请先禁用后再重新设置".into(),
        ));
    }
    let secret = totp::generate_secret();
    let b32 = totp::secret_to_base32(&secret);
    let enc = state.crypto.encrypt(&b32).map_err(ApiError::internal)?;
    sqlx::query(
        "UPDATE admin_users SET totp_secret_enc = $1, totp_enabled = false WHERE username = $2",
    )
    .bind(&enc)
    .bind(&username)
    .execute(&state.db)
    .await?;
    audit(
        &state,
        &username,
        "auth.totp_setup",
        "admin",
        Some(&username),
        serde_json::json!({}),
        ip,
    )
    .await?;
    Ok(Json(serde_json::json!({
        "secret": b32,
        "otpauth_url": totp::otpauth_url("NextAPI", &username, &b32),
    })))
}

#[derive(Debug, Deserialize)]
struct TotpCodeReq {
    code: String,
}

/// POST /totp/enable {code}：验证 pending 机密的验证码后启用。
async fn totp_enable(
    State(state): State<Arc<AppState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<TotpCodeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers).await?;
    sensitive_rate_check(&state, &username)?;
    let ip = client_ip(&state, &headers, peer.map(|Extension(c)| c.0.ip()));
    let row = sqlx::query(
        "SELECT totp_enabled, totp_secret_enc, last_totp_step FROM admin_users WHERE username = $1",
    )
    .bind(&username)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    if row.try_get::<bool, _>("totp_enabled").unwrap_or(false) {
        return Err(ApiError::bad_request("TOTP 已启用"));
    }
    let secret = decode_totp_secret(
        &state,
        row.try_get::<Option<String>, _>("totp_secret_enc")
            .ok()
            .flatten(),
    )?;
    let now = Utc::now().timestamp().max(0) as u64;
    let step = totp::verify(&secret, &body.code, now).ok_or(ApiError::TotpInvalid)?;
    // 启用即吊销全部会话（session_version+1，与改密一致，发布审阅 M4）——
    // 当前会话的旧 token 也失效，前端启用成功后跳回登录页重新登录。
    sqlx::query(
        "UPDATE admin_users SET totp_enabled = true, last_totp_step = $2,          session_version = session_version + 1 WHERE username = $1",
    )
    .bind(&username)
    .bind(step)
    .execute(&state.db)
    .await?;
    audit(
        &state,
        &username,
        "auth.totp_enable",
        "admin",
        Some(&username),
        serde_json::json!({}),
        ip,
    )
    .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
struct TotpDisableReq {
    password: String,
    code: String,
}

/// POST /totp/disable {password, code}：密码 + 验证码双重校验后禁用并清除机密。
/// 认证器丢失的恢复途径：直接操作数据库清空
/// `totp_enabled` / `totp_secret_enc` / `last_totp_step` 三列（README 记载）。
async fn totp_disable(
    State(state): State<Arc<AppState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<TotpDisableReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers).await?;
    sensitive_rate_check(&state, &username)?;
    let ip = client_ip(&state, &headers, peer.map(|Extension(c)| c.0.ip()));
    let row = sqlx::query(
        "SELECT password_hash, totp_enabled, totp_secret_enc, last_totp_step FROM admin_users WHERE username = $1",
    )
    .bind(&username)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    if !row.try_get::<bool, _>("totp_enabled").unwrap_or(false) {
        return Err(ApiError::bad_request("TOTP 未启用"));
    }
    let hash: String = row.get("password_hash");
    if !verify_password(&hash, &body.password) {
        return Err(ApiError::bad_request("密码不正确"));
    }
    let secret = decode_totp_secret(
        &state,
        row.try_get::<Option<String>, _>("totp_secret_enc")
            .ok()
            .flatten(),
    )?;
    let now = Utc::now().timestamp().max(0) as u64;
    // 防重放：命中步必须大于上次消费步（登录/enable 已消费的码不可再用于 disable）
    let last: Option<i64> = row.try_get("last_totp_step").ok().flatten();
    if totp::verify(&secret, &body.code, now)
        .filter(|st| last.is_none_or(|l| *st > l))
        .is_none()
    {
        return Err(ApiError::TotpInvalid);
    }
    // 禁用即吊销全部会话（含当前）；机密与防重放游标一并清除。
    sqlx::query(
        "UPDATE admin_users SET totp_enabled = false, totp_secret_enc = NULL,          last_totp_step = NULL, session_version = session_version + 1 WHERE username = $1",
    )
    .bind(&username)
    .execute(&state.db)
    .await?;
    audit(
        &state,
        &username,
        "auth.totp_disable",
        "admin",
        Some(&username),
        serde_json::json!({}),
        ip,
    )
    .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 敏感端点（改密/TOTP 管理）限速：与登录共用滑窗限速器，
/// 键加 `sens:` 前缀隔离——防持会话者爆破旧密码/验证码（发布审阅 M5）。
fn sensitive_rate_check(state: &AppState, username: &str) -> ApiResult<()> {
    let limit = state.hot.load().gateway.admin_login_rate_limit_per_min;
    if !login_rate_limiter().check(&format!("sens:{username}"), limit) {
        return Err(ApiError::RateLimited);
    }
    Ok(())
}

/// 解密并解析 TOTP 机密（enc 为 DB 中的 AES-GCM 密文）。
fn decode_totp_secret(state: &AppState, enc: Option<String>) -> ApiResult<Vec<u8>> {
    let enc = enc.ok_or_else(|| ApiError::bad_request("请先生成 TOTP 机密（setup）"))?;
    let b32 = state.crypto.decrypt(&enc).map_err(ApiError::internal)?;
    totp::secret_from_base32(&b32).ok_or_else(|| ApiError::internal("TOTP 机密损坏"))
}

/// 敏感操作二次验证（如查看上游明文 API Key）：校验当前管理员密码；
/// 若已启用 TOTP 则同时校验动态码（含防重放步消费，语义与登录一致）。
///
/// - 失败统一返回 400（不吊销会话），错误与上游资源状态无关，防未授权探测；
/// - 每次调用都受 per-username 敏感限速（与登录共用滑窗，键前缀 `sens:`）约束；
/// - 调用方在验证通过后自行完成目标操作并写审计。
pub(crate) async fn sensitive_verify(
    state: &AppState,
    username: &str,
    password: &str,
    totp_code: Option<&str>,
) -> ApiResult<()> {
    sensitive_rate_check(state, username)?;

    let row = sqlx::query(
        "SELECT password_hash, totp_enabled, totp_secret_enc, last_totp_step \
         FROM admin_users WHERE username = $1",
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    let hash: String = row.get("password_hash");
    if !verify_password(&hash, password) {
        return Err(ApiError::bad_request("密码不正确"));
    }

    // TOTP 未启用：密码通过即放行
    if !row.try_get::<bool, _>("totp_enabled").unwrap_or(false) {
        return Ok(());
    }
    // 已启用：动态码必填，校验并防重放（命中步须大于上次消费步，原子推进）
    let code = totp_code.unwrap_or("").trim().to_string();
    if code.is_empty() {
        return Err(ApiError::bad_request("已启用 TOTP，需要二次验证码"));
    }
    let secret = decode_totp_secret(
        state,
        row.try_get::<Option<String>, _>("totp_secret_enc")
            .ok()
            .flatten(),
    )?;
    let now = Utc::now().timestamp().max(0) as u64;
    let step =
        totp::verify(&secret, &code, now).ok_or_else(|| ApiError::bad_request("二次验证码错误"))?;
    let consumed = sqlx::query(
        "UPDATE admin_users SET last_totp_step = $2 \
         WHERE username = $1 AND (last_totp_step IS NULL OR last_totp_step < $2)",
    )
    .bind(username)
    .bind(step)
    .execute(&state.db)
    .await?;
    if consumed.rows_affected() != 1 {
        return Err(ApiError::bad_request("二次验证码已使用，请稍后重新获取"));
    }
    Ok(())
}

/// 审计用客户端 IP：读取 trusted_proxies 配置并结合 TCP 对端解析（M15）。
/// `peer` 来自 `ConnectInfo<SocketAddr>`（由 main.rs 的
/// `into_make_service_with_connect_info` 注入每个请求的扩展）。
fn client_ip(state: &AppState, headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
    let trusted = state
        .file_config
        .read()
        .unwrap()
        .server
        .trusted_proxies
        .clone();
    resolve_client_ip(headers, peer, &trusted)
}

/// JWT 保护中间件：校验 Authorization: Bearer，失败返回 401。
/// 校验通过后把用户名写入请求扩展，供 `/api/*` 下的 handler 提取；
/// 客户端 IP 写入 task 作用域，供 audit() 自动携带（无需改动各 handler）。
pub async fn require_admin(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let username = current_username(&state, req.headers()).await?;
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let ip = client_ip(&state, req.headers(), peer);
    req.extensions_mut().insert(AdminUsername(username));
    Ok(CURRENT_CLIENT_IP.scope(ip, next.run(req)).await)
}

// ---------------------------------------------------------------------------
// M15：可信代理核心修复——真实客户端 IP 解析
// ---------------------------------------------------------------------------

/// 解析后的可信代理网段。裸 IP 视为 /32（IPv4）或 /128（IPv6）。
#[derive(Clone, Copy, Debug)]
enum TrustedNet {
    V4(Ipv4Addr, u8),
    V6(Ipv6Addr, u8),
}

/// 解析 trusted_proxies 配置：支持 CIDR（如 "10.0.0.0/8"）与裸 IP；非法项忽略并告警
/// （宁可不信任，也不因配置笔误把半个互联网当可信代理）。
fn parse_trusted_nets(entries: &[String]) -> Vec<TrustedNet> {
    let mut nets = Vec::with_capacity(entries.len());
    for raw in entries {
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        let (ip_str, prefix_str) = match s.split_once('/') {
            Some((ip, p)) => (ip.trim(), Some(p.trim())),
            None => (s, None),
        };
        let parsed = ip_str.parse::<IpAddr>().ok().and_then(|ip| {
            let max: u8 = if ip.is_ipv4() { 32 } else { 128 };
            let prefix = match prefix_str {
                Some(p) => p.parse::<u8>().ok().filter(|p| *p <= max)?,
                None => max,
            };
            Some(match ip {
                IpAddr::V4(a) => TrustedNet::V4(a, prefix),
                IpAddr::V6(a) => TrustedNet::V6(a, prefix),
            })
        });
        match parsed {
            Some(net) => nets.push(net),
            None => tracing::warn!("server.trusted_proxies 条目非法，已忽略: {s}"),
        }
    }
    nets
}

/// 判断 IP 是否落在可信代理网段内（IPv4/IPv6 之间互不匹配）。
fn ip_in_trusted(ip: &IpAddr, nets: &[TrustedNet]) -> bool {
    nets.iter().any(|net| match (net, ip) {
        (TrustedNet::V4(base, prefix), IpAddr::V4(a)) => {
            *prefix == 0 || (a.to_bits() ^ base.to_bits()) >> (32 - *prefix as u32) == 0
        }
        (TrustedNet::V6(base, prefix), IpAddr::V6(a)) => {
            *prefix == 0 || (a.to_bits() ^ base.to_bits()) >> (128 - *prefix as u32) == 0
        }
        _ => false,
    })
}

/// M15 核心：结合 TCP 对端（ConnectInfo）与 trusted_proxies 判定真实客户端 IP。
///
/// 安全语义（X-Forwarded-For/X-Real-IP 均可由直连客户端伪造）：
/// - 无对端信息（不应发生：ConnectInfo 由 serve 注入）→ 不信任转发头，返回 None；
/// - trusted_proxies 为空，或对端不在可信网段 → 转发头一律忽略，返回对端 IP；
/// - 对端可信 → 沿 XFF 链**从右向左**跳过可信代理（最右条目由最外层可信代理追加），
///   第一个不可信条目即真实客户端；全链可信则取最左条目（最佳努力）；
///   链中出现无法解析的条目视为边界，不再信任其左侧任何内容（返回 None）；
/// - 无 XFF（或全为空白）时回退 X-Real-IP（紧邻代理设置），仍无则返回对端 IP。
fn resolve_client_ip(
    headers: &HeaderMap,
    peer: Option<IpAddr>,
    trusted_proxies: &[String],
) -> Option<IpAddr> {
    let peer = peer?;
    let nets = parse_trusted_nets(trusted_proxies);
    if nets.is_empty() || !ip_in_trusted(&peer, &nets) {
        // 直连客户端或不可信代理：转发头可伪造，只用真实 TCP 对端。
        return Some(peer);
    }
    // XFF 格式：「client, proxy1, proxy2」，对端 peer 隐含在链尾（已确认可信）。
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        let mut saw_entry = false;
        for part in xff.split(',').rev() {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            saw_entry = true;
            match part.parse::<IpAddr>() {
                // 可信代理追加/透传的地址，继续向左追溯
                Ok(ip) if ip_in_trusted(&ip, &nets) => continue,
                // 第一个不可信者 = 真实客户端（其右侧全为可信代理）
                Ok(ip) => return Some(ip),
                // 非法条目：链已不可信，不得继续消费其左侧内容
                Err(_) => return None,
            }
        }
        if saw_entry {
            // 全链可信：取最左条目（最早代理声称的来源，最佳努力）。
            // 循环中已确认每个条目可解析，此处 parse 必然成功。
            if let Some(Ok(ip)) = xff
                .split(',')
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.parse::<IpAddr>())
            {
                return Some(ip);
            }
        }
    }
    // 无 XFF：回退 X-Real-IP（由紧邻的可信代理设置）
    if let Some(ip) = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<IpAddr>().ok())
    {
        return Some(ip);
    }
    Some(peer)
}

/// 从请求头解析 Bearer 并校验 JWT，返回管理员用户名。
/// `require_admin` 与被保护的 handler 复用同一逻辑。
pub async fn current_username(state: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    let token = bearer_from_headers(headers)?;
    let claims = state
        .jwt
        .verify_claims(token)
        .map_err(|_| ApiError::Unauthorized)?;
    // session_version 在改密时递增，使旧 JWT 立即失效。
    let row = sqlx::query("SELECT session_version FROM admin_users WHERE username = $1")
        .bind(&claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| ApiError::Unauthorized)?
        .ok_or(ApiError::Unauthorized)?;
    let current: i32 = row.try_get("session_version").unwrap_or(0);
    if current != claims.session_version {
        return Err(ApiError::Unauthorized);
    }
    Ok(claims.sub)
}

/// 从 Authorization: Bearer 头提取 token。
fn bearer_from_headers(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)
}

/// argon2 校验密码：无法解析的 PHC 串一律视为校验失败。
fn verify_password(hash: &str, password: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(ph) => Argon2::default()
            .verify_password(password.as_bytes(), &ph)
            .is_ok(),
        Err(_) => false,
    }
}

/// argon2（默认参数）计算 PHC 哈希。
fn hash_password(password: &str) -> anyhow::Result<String> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 哈希失败: {e}"))?;
    Ok(hash.to_string())
}

/// 内存滑动窗口限速器：`username -> (最近请求时间戳队列, 最近活动时间)`。
/// 条目设上限并闲置淘汰，防随机用户名无界增长（review P4）。
struct LoginRateLimiter(Mutex<HashMap<String, RateEntry>>);

struct RateEntry {
    events: VecDeque<Instant>,
    last_seen: Instant,
}

impl LoginRateLimiter {
    /// 尝试记录一次登录：返回 true 表示放行（并计入窗口），false 表示超限。
    fn check(&self, username: &str, limit: u32) -> bool {
        if limit == 0 {
            return true; // 0 = 关闭限速
        }
        let now = Instant::now();
        let window = Duration::from_secs(RATE_WINDOW_SECS);
        let idle = Duration::from_secs(IDLE_EVICT_SECS);
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // 新用户名且已达条目上限：先淘汰闲置条目；仍满则拒绝（保守：限速而非放行）
        if !map.contains_key(username) && map.len() >= MAX_RATE_ENTRIES {
            evict_idle_entries(&mut map, now, idle);
            if !map.contains_key(username) && map.len() >= MAX_RATE_ENTRIES {
                tracing::warn!("登录限速器条目超限，拒绝新用户名 {username} 的尝试");
                return false;
            }
        }
        let entry = map
            .entry(username.to_string())
            .or_insert_with(|| RateEntry {
                events: VecDeque::new(),
                last_seen: now,
            });
        entry.last_seen = now;
        window_allow(&mut entry.events, now, limit, window)
    }
}

/// 纯函数：淘汰闲置（超过 idle 未活动）的限速条目。供测试。
fn evict_idle_entries(map: &mut HashMap<String, RateEntry>, now: Instant, idle: Duration) {
    map.retain(|_, e| now.duration_since(e.last_seen) <= idle);
}

static LOGIN_RATE_LIMITER: OnceLock<LoginRateLimiter> = OnceLock::new();

fn login_rate_limiter() -> &'static LoginRateLimiter {
    LOGIN_RATE_LIMITER.get_or_init(|| LoginRateLimiter(Mutex::new(HashMap::new())))
}

/// 纯函数：滑窗判定。先剔除小于 `window` 的过期事件，再判断是否还能容纳一个新事件。
/// 放行时把 `now` 压入队列；超限返回 false 且不压入。
fn window_allow(queue: &mut VecDeque<Instant>, now: Instant, limit: u32, window: Duration) -> bool {
    // 剔除过期事件（队列按时间递增，从头部开始）
    while let Some(&t) = queue.front() {
        if now.duration_since(t) > window {
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

/// 管理操作审计：写 admin_audit_logs（独立表，不混入 usage_logs）。
/// admin_id 由 username 查 admin_users 得到，查不到则记 warn 日志并把 admin_id 置 NULL；
/// 审计写入失败只记 error 日志，绝不阻断主操作（返回 Ok）。
pub async fn audit(
    state: &AppState,
    admin: &str,
    action: &str,
    object_type: &str,
    object_id: Option<&str>,
    summary: serde_json::Value,
    ip: Option<std::net::IpAddr>,
) -> ApiResult<()> {
    // 解析 admin 用户名 → UUID
    let admin_id: Option<Uuid> =
        match sqlx::query_scalar::<_, Uuid>("SELECT id FROM admin_users WHERE username = $1")
            .bind(admin)
            .fetch_optional(&state.db)
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("审计: 解析 admin `{admin}` 的 id 失败: {e}");
                None
            }
        };

    // ip 为 INET 列：项目未启用 sqlx 的 ipnet 特性，先转成文本再 `::inet` 交由 PostgreSQL 解析。
    // 显式 ip 优先；未传时回退到 task 作用域（require_admin 中间件注入的当前请求 IP）。
    let ip = ip.or_else(|| CURRENT_CLIENT_IP.try_with(|v| *v).unwrap_or(None));
    let ip_text: Option<String> = ip.map(|i| i.to_string());

    if let Err(e) = sqlx::query(
        "INSERT INTO admin_audit_logs (admin_id, action, object_type, object_id, summary, ip) \
         VALUES ($1, $2, $3, $4, $5, $6::inet)",
    )
    .bind(admin_id)
    .bind(action)
    .bind(object_type)
    .bind(object_id)
    .bind(&summary)
    .bind(ip_text)
    .execute(&state.db)
    .await
    {
        tracing::error!("审计写入失败: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_allows_within_limit() {
        let mut q = VecDeque::new();
        let start = Instant::now();
        let window = Duration::from_secs(RATE_WINDOW_SECS);
        // 允许 limit=3 个事件
        assert!(window_allow(&mut q, start, 3, window));
        assert!(window_allow(
            &mut q,
            start + Duration::from_secs(1),
            3,
            window
        ));
        assert!(window_allow(
            &mut q,
            start + Duration::from_secs(2),
            3,
            window
        ));
        // 第 4 个超限
        assert!(!window_allow(
            &mut q,
            start + Duration::from_secs(3),
            3,
            window
        ));
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn window_expires_old_events() {
        let mut q = VecDeque::new();
        let start = Instant::now();
        let window = Duration::from_secs(RATE_WINDOW_SECS);
        q.push_back(start + Duration::from_secs(1));
        q.push_back(start + Duration::from_secs(2));
        q.push_back(start + Duration::from_secs(3));
        // 此刻 3 个事件都在窗口内，达到上限 → 第 4 个超限
        assert!(!window_allow(
            &mut q,
            start + Duration::from_secs(4),
            3,
            window
        ));
        assert_eq!(q.len(), 3);
        // 跳至 far future：全部事件过期，窗口重新放行且只剩新压入的 1 个
        assert!(window_allow(
            &mut q,
            start + Duration::from_secs(100),
            3,
            window
        ));
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn window_zero_limit_rejects_all() {
        let start = Instant::now();
        // limit=0 时纯函数任何事件都超限（不放行、不记录）
        assert!(!window_allow(
            &mut VecDeque::new(),
            start,
            0,
            Duration::from_secs(1)
        ));
    }

    #[test]
    fn evicts_idle_rate_entries() {
        // review P4：限速器条目闲置淘汰（防随机用户名无界增长）
        let now = Instant::now();
        let idle = Duration::from_secs(IDLE_EVICT_SECS);
        let mut map = HashMap::new();
        let active = RateEntry {
            events: VecDeque::new(),
            last_seen: now,
        };
        let stale = RateEntry {
            events: VecDeque::new(),
            last_seen: now - idle - Duration::from_secs(1),
        };
        map.insert("active-user".into(), active);
        map.insert("stale-user".into(), stale);
        evict_idle_entries(&mut map, now, idle);
        assert!(map.contains_key("active-user"));
        assert!(!map.contains_key("stale-user"));
    }

    // ------------------------------------------------------------------
    // M15：可信代理核心修复——客户端 IP 解析单测
    // ------------------------------------------------------------------

    /// 便捷构造：字符串切片 → Vec<String> 配置。
    fn cfgs(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|s| s.to_string()).collect()
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn trusted_nets_parse_cidr_and_bare_ip() {
        // CIDR、裸 IP（视为 /32、/128）、空白与大小写混合
        let nets = parse_trusted_nets(&cfgs(&[
            "10.0.0.0/8",
            "192.168.1.7",
            "2001:db8::/32",
            "  ::1  ",
            "",
        ]));
        assert_eq!(nets.len(), 4);
        assert!(ip_in_trusted(&ip("10.9.8.7"), &nets));
        assert!(ip_in_trusted(&ip("192.168.1.7"), &nets));
        assert!(!ip_in_trusted(&ip("192.168.1.8"), &nets));
        assert!(ip_in_trusted(&ip("2001:db8::dead:beef"), &nets));
        assert!(ip_in_trusted(&ip("::1"), &nets));
        // v4/v6 互不匹配
        assert!(!ip_in_trusted(&ip("8.8.8.8"), &nets));
        assert!(!ip_in_trusted(&ip("2001:db9::1"), &nets));
    }

    #[test]
    fn trusted_nets_ignore_invalid_entries() {
        // 非法前缀、越界前缀、非 IP：全部忽略（宁可不信任）
        let nets = parse_trusted_nets(&cfgs(&["10.0.0.0/33", "not-an-ip", "10.0.0.0/x", "/"]));
        assert!(nets.is_empty());
        assert!(!ip_in_trusted(&ip("10.0.0.1"), &nets));
        // /0 合法：匹配全部 v4
        let nets = parse_trusted_nets(&cfgs(&["0.0.0.0/0"]));
        assert!(ip_in_trusted(&ip("1.2.3.4"), &nets));
        assert!(!ip_in_trusted(&ip("::1"), &nets));
    }

    #[test]
    fn resolve_ip_empty_trusted_uses_peer() {
        // M15：trusted_proxies 为空 → 不信任转发头，审计 IP = TCP 对端
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        h.insert("x-real-ip", "5.6.7.8".parse().unwrap());
        assert_eq!(
            resolve_client_ip(&h, Some(ip("203.0.113.9")), &[]),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn resolve_ip_untrusted_peer_ignores_forward_headers() {
        // M15：对端不在可信网段（如客户端直连伪造 XFF）→ 一律用对端 IP
        let trusted = cfgs(&["10.0.0.0/8"]);
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        h.insert("x-real-ip", "5.6.7.8".parse().unwrap());
        assert_eq!(
            resolve_client_ip(&h, Some(ip("203.0.113.9")), &trusted),
            Some(ip("203.0.113.9"))
        );
        // 无转发头同样返回对端
        let h2 = HeaderMap::new();
        assert_eq!(
            resolve_client_ip(&h2, Some(ip("203.0.113.9")), &trusted),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn resolve_ip_no_peer_returns_none() {
        // 无 ConnectInfo（不应发生）：不信任任何转发头
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        assert_eq!(resolve_client_ip(&h, None, &cfgs(&["0.0.0.0/0"])), None);
    }

    #[test]
    fn resolve_ip_trusted_peer_single_hop() {
        // 单级代理：XFF 只有一个客户端地址
        let trusted = cfgs(&["10.0.0.0/8"]);
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        assert_eq!(
            resolve_client_ip(&h, Some(ip("10.0.0.2")), &trusted),
            Some(ip("1.2.3.4"))
        );
    }

    #[test]
    fn resolve_ip_multihop_xff_right_to_left() {
        // 多级代理链 client(1.2.3.4) → proxyA(10.0.0.1) → proxyB(10.0.0.2) → NextAPI
        // XFF: "1.2.3.4, 10.0.0.1"，对端 10.0.0.2（可信）
        let trusted = cfgs(&["10.0.0.0/8"]);
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4, 10.0.0.1".parse().unwrap());
        assert_eq!(
            resolve_client_ip(&h, Some(ip("10.0.0.2")), &trusted),
            Some(ip("1.2.3.4"))
        );
    }

    #[test]
    fn resolve_ip_multihop_stops_at_first_untrusted() {
        // 客户端伪造 XFF 前缀：client 发送 "XFF: 9.9.9.9"，经可信代理追加后
        // 链为 "9.9.9.9, 8.8.8.8, 10.0.0.1"（8.8.8.8 是外层 LB 看到的客户端）。
        // 从右向左跳过可信的 10.0.0.1，停在第一个不可信者 8.8.8.8——
        // 伪造的 9.9.9.9 在其左侧，不予采信。
        let trusted = cfgs(&["10.0.0.0/8"]);
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            "9.9.9.9, 8.8.8.8, 10.0.0.1".parse().unwrap(),
        );
        assert_eq!(
            resolve_client_ip(&h, Some(ip("10.0.0.2")), &trusted),
            Some(ip("8.8.8.8"))
        );
    }

    #[test]
    fn resolve_ip_all_trusted_takes_leftmost() {
        // 全链可信（如多级内网代理）：取最左条目（最佳努力）
        let trusted = cfgs(&["10.0.0.0/8"]);
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "10.0.0.5, 10.0.0.1".parse().unwrap());
        assert_eq!(
            resolve_client_ip(&h, Some(ip("10.0.0.2")), &trusted),
            Some(ip("10.0.0.5"))
        );
    }

    #[test]
    fn resolve_ip_invalid_xff_entry_is_boundary() {
        // XFF 中混入无法解析的条目：视为链边界，不再信任其左侧
        let trusted = cfgs(&["10.0.0.0/8"]);
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4, garbage".parse().unwrap());
        assert_eq!(resolve_client_ip(&h, Some(ip("10.0.0.2")), &trusted), None);
    }

    #[test]
    fn resolve_ip_real_ip_fallback() {
        // 无 XFF 时回退 X-Real-IP；两者皆无/非法时回退对端
        let trusted = cfgs(&["10.0.0.0/8"]);
        let mut h = HeaderMap::new();
        h.insert("x-real-ip", "2001:db8::1".parse().unwrap());
        assert_eq!(
            resolve_client_ip(&h, Some(ip("10.0.0.2")), &trusted),
            Some(ip("2001:db8::1"))
        );
        h.insert("x-real-ip", "not-an-ip".parse().unwrap());
        assert_eq!(
            resolve_client_ip(&h, Some(ip("10.0.0.2")), &trusted),
            Some(ip("10.0.0.2"))
        );
        // 空白 XFF 视为缺失，走 X-Real-IP
        let mut h2 = HeaderMap::new();
        h2.insert("x-forwarded-for", "  ".parse().unwrap());
        h2.insert("x-real-ip", "1.2.3.4".parse().unwrap());
        assert_eq!(
            resolve_client_ip(&h2, Some(ip("10.0.0.2")), &trusted),
            Some(ip("1.2.3.4"))
        );
    }

    #[test]
    fn resolve_ip_wildcard_trusts_any_proxy() {
        // 0.0.0.0/0：任意 IPv4 对端都视为可信代理（含 X-Real-IP 兜底路径）
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4, 10.0.0.1".parse().unwrap());
        assert_eq!(
            resolve_client_ip(&h, Some(ip("192.0.2.1")), &cfgs(&["0.0.0.0/0"])),
            Some(ip("1.2.3.4"))
        );
        h.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        h.insert("x-real-ip", "10.0.0.8".parse().unwrap());
        assert_eq!(
            resolve_client_ip(&h, Some(ip("192.0.2.1")), &cfgs(&["0.0.0.0/0"])),
            None // 非法 XFF 条目是链边界，不回退 X-Real-IP
        );
        h.remove("x-forwarded-for");
        assert_eq!(
            resolve_client_ip(&h, Some(ip("192.0.2.1")), &cfgs(&["0.0.0.0/0"])),
            Some(ip("10.0.0.8"))
        );
    }

    #[test]
    fn jwt_roundtrip() {
        let svc = JwtService::new("test-secret".into());
        let token = svc.issue("admin", 0).unwrap();
        assert_eq!(svc.verify(&token).unwrap(), "admin");
        assert_eq!(svc.verify_claims(&token).unwrap().session_version, 0);
        let newer = svc.issue("admin", 1).unwrap();
        assert_eq!(svc.verify_claims(&newer).unwrap().session_version, 1);
        assert_ne!(token, newer);
        // 错误密钥校验失败
        let bad = JwtService::new("other-secret".into());
        assert!(bad.verify(&token).is_err());
        // 非法 token
        assert!(svc.verify("not-a-jwt").is_err());
    }

    #[test]
    fn argon2_verify_logic() {
        let hash = hash_password("test-password").unwrap();
        assert!(verify_password(&hash, "test-password"));
        assert!(!verify_password(&hash, "wrong"));
        assert!(!verify_password("not-a-hash", "x"));
    }
}
