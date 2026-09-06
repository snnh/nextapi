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
    extract::{FromRequestParts, Request, State},
    http::request::Parts,
    http::HeaderMap,
    middleware::Next,
    response::Response,
    routing::{get, post, put},
    Json, Router,
};
use chrono::Utc;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::{HashMap, VecDeque};
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
    headers: HeaderMap,
    Json(body): Json<LoginReq>,
) -> ApiResult<Json<serde_json::Value>> {
    // 1. 防爆破：按 username 的 1 分钟滑动窗口限速
    let trusted = state
        .file_config
        .read()
        .unwrap()
        .server
        .trusted_proxies
        .clone();
    let ip = client_ip(&headers, &trusted);
    let limit = state.hot.load().gateway.admin_login_rate_limit_per_min;
    let rate_key = format!(
        "{}:{}",
        body.username.trim().to_ascii_lowercase(),
        ip.map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );
    if !login_rate_limiter().check(&rate_key, limit) {
        return Err(ApiError::RateLimited);
    }

    // 2. 查用户；查不到与密码错误统一返回 401（不区分）。
    //    用户不存在不写审计（防随机用户名刷爆审计表）；仅密码错误写失败审计，
    //    写入速率受 per-username 限速约束。
    let row = sqlx::query(
        "SELECT password_hash, session_version, totp_enabled, totp_secret_enc FROM admin_users WHERE username = $1",
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
        let ok = secret
            .as_deref()
            .map(|s| totp::verify(s, &code, now))
            .unwrap_or(false);
        if !ok {
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
    headers: HeaderMap,
    Json(body): Json<ChangePasswordReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers).await?;
    let trusted = state
        .file_config
        .read()
        .unwrap()
        .server
        .trusted_proxies
        .clone();
    let ip = client_ip(&headers, &trusted);

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
async fn totp_setup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers).await?;
    let ip = current_client_ip(&state, &headers);
    let row = sqlx::query("SELECT totp_enabled FROM admin_users WHERE username = $1")
        .bind(&username)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::Unauthorized)?;
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
    headers: HeaderMap,
    Json(body): Json<TotpCodeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers).await?;
    let ip = current_client_ip(&state, &headers);
    let row =
        sqlx::query("SELECT totp_enabled, totp_secret_enc FROM admin_users WHERE username = $1")
            .bind(&username)
            .fetch_optional(&state.db)
            .await?
            .ok_or(ApiError::Unauthorized)?;
    if row.try_get::<bool, _>("totp_enabled").unwrap_or(false) {
        return Err(ApiError::bad_request("TOTP 已启用"));
    }
    let secret = row
        .try_get::<Option<String>, _>("totp_secret_enc")
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::bad_request("请先生成 TOTP 机密（setup）"))?;
    let secret =
        totp::secret_from_base32(&state.crypto.decrypt(&secret).map_err(ApiError::internal)?)
            .ok_or_else(|| ApiError::internal("TOTP 机密损坏"))?;
    let now = Utc::now().timestamp().max(0) as u64;
    if !totp::verify(&secret, &body.code, now) {
        return Err(ApiError::TotpInvalid);
    }
    sqlx::query("UPDATE admin_users SET totp_enabled = true WHERE username = $1")
        .bind(&username)
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
/// 认证器丢失的恢复途径：直接操作数据库清空这两列（README 记载）。
async fn totp_disable(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<TotpDisableReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers).await?;
    let ip = current_client_ip(&state, &headers);
    let row = sqlx::query(
        "SELECT password_hash, totp_enabled, totp_secret_enc FROM admin_users WHERE username = $1",
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
    let enc = row
        .try_get::<Option<String>, _>("totp_secret_enc")
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::internal("TOTP 机密缺失"))?;
    let secret = totp::secret_from_base32(&state.crypto.decrypt(&enc).map_err(ApiError::internal)?)
        .ok_or_else(|| ApiError::internal("TOTP 机密损坏"))?;
    let now = Utc::now().timestamp().max(0) as u64;
    if !totp::verify(&secret, &body.code, now) {
        return Err(ApiError::TotpInvalid);
    }
    sqlx::query(
        "UPDATE admin_users SET totp_enabled = false, totp_secret_enc = NULL WHERE username = $1",
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

/// 审计用客户端 IP（复用 trusted_proxies 配置的解析逻辑）。
fn current_client_ip(state: &AppState, headers: &HeaderMap) -> Option<std::net::IpAddr> {
    let trusted = state
        .file_config
        .read()
        .unwrap()
        .server
        .trusted_proxies
        .clone();
    client_ip(headers, &trusted)
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
    let trusted = state
        .file_config
        .read()
        .unwrap()
        .server
        .trusted_proxies
        .clone();
    let ip = client_ip(req.headers(), &trusted);
    req.extensions_mut().insert(AdminUsername(username));
    Ok(CURRENT_CLIENT_IP.scope(ip, next.run(req)).await)
}

/// 客户端 IP（最佳努力，审计展示用途）：X-Forwarded-For 首值 → X-Real-IP。
/// 说明：XFF 可由客户端伪造，故不将其作为任何安全边界（限速仍以 username 为准）。
fn client_ip(headers: &HeaderMap, trusted_proxies: &[String]) -> Option<std::net::IpAddr> {
    if trusted_proxies.is_empty() {
        return None;
    }
    // 仅接受可解析的地址；XFF 存在但首值非法时继续尝试 X-Real-IP，
    // 避免恶意/异常头让审计 IP 无故丢失。
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next().map(str::trim))
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse().ok())
        })
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

    #[test]
    fn client_ip_parses_proxy_headers() {
        let mut h = axum::http::HeaderMap::new();
        assert_eq!(client_ip(&h, &[]), None);
        // XFF 取首个
        h.insert("x-forwarded-for", "1.2.3.4, 10.0.0.1".parse().unwrap());
        assert_eq!(
            client_ip(&h, &["0.0.0.0/0".into()]),
            Some("1.2.3.4".parse().unwrap())
        );
        // 非法值继续回退到 X-Real-IP
        h.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        h.insert("x-real-ip", "10.0.0.8".parse().unwrap());
        assert_eq!(
            client_ip(&h, &["0.0.0.0/0".into()]),
            Some("10.0.0.8".parse().unwrap())
        );
        // x-real-ip 兜底
        h.remove("x-forwarded-for");
        h.insert("x-real-ip", "2001:db8::1".parse().unwrap());
        assert_eq!(
            client_ip(&h, &["::/0".into()]),
            Some("2001:db8::1".parse().unwrap())
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
