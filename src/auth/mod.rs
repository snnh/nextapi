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

/// JWT 载荷：sub（用户名）+ exp + iat。
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
    iat: usize,
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
    pub fn issue(&self, username: &str) -> anyhow::Result<String> {
        let now = Utc::now().timestamp().max(0) as usize;
        let claims = Claims {
            sub: username.to_string(),
            exp: now + self.ttl_secs as usize,
            iat: now,
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )?;
        Ok(token)
    }

    /// 校验 token，返回主体（用户名）。
    pub fn verify(&self, token: &str) -> anyhow::Result<String> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_aud = false; // 我们不用 aud 声明
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &validation,
        )?;
        Ok(data.claims.sub)
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
        .route("/password", put(change_password))
}

/// 登录请求体。
#[derive(Debug, Deserialize)]
struct LoginReq {
    username: String,
    password: String,
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
    Json(body): Json<LoginReq>,
) -> ApiResult<Json<serde_json::Value>> {
    // 1. 防爆破：按 username 的 1 分钟滑动窗口限速
    let limit = state.hot.load().gateway.admin_login_rate_limit_per_min;
    if !login_rate_limiter().check(&body.username, limit) {
        return Err(ApiError::RateLimited);
    }

    // 2. 查用户；查不到与密码错误统一返回 401（不区分）
    let row = sqlx::query("SELECT password_hash FROM admin_users WHERE username = $1")
        .bind(&body.username)
        .fetch_optional(&state.db)
        .await?;
    let Some(row) = row else {
        return Err(ApiError::Unauthorized);
    };
    let hash: String = row.get("password_hash");

    // 3. argon2 校验密码
    if !verify_password(&hash, &body.password) {
        return Err(ApiError::Unauthorized);
    }

    // 4. 签发 JWT
    let token = state
        .jwt
        .issue(&body.username)
        .map_err(|e| ApiError::internal(e))?;
    Ok(Json(
        serde_json::json!({ "token": token, "username": body.username }),
    ))
}

/// GET /me：返回当前登录用户名。
async fn me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers)?;
    Ok(Json(serde_json::json!({ "username": username })))
}

/// PUT /password：校验旧密码后更新为新密码，并写审计。
async fn change_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let username = current_username(&state, &headers)?;

    // 查用户（token 有效但用户可能已被删）
    let row = sqlx::query("SELECT password_hash FROM admin_users WHERE username = $1")
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
    let new_hash = hash_password(&body.new_password).map_err(|e| ApiError::internal(e))?;
    sqlx::query("UPDATE admin_users SET password_hash = $1 WHERE username = $2")
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
        None,
    )
    .await?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// JWT 保护中间件：校验 Authorization: Bearer，失败返回 401。
/// 校验通过后把用户名写入请求扩展，供 `/api/*` 下的 handler 提取。
pub async fn require_admin(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let username = current_username(&state, req.headers())?;
    req.extensions_mut().insert(AdminUsername(username));
    Ok(next.run(req).await)
}

/// 从请求头解析 Bearer 并校验 JWT，返回管理员用户名。
/// `require_admin` 与被保护的 handler 复用同一逻辑。
pub fn current_username(state: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    let token = bearer_from_headers(headers)?;
    state.jwt.verify(token).map_err(|_| ApiError::Unauthorized)
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

/// 内存滑动窗口限速器：`username -> 最近请求时间戳队列`。
struct LoginRateLimiter(Mutex<HashMap<String, VecDeque<Instant>>>);

impl LoginRateLimiter {
    /// 尝试记录一次登录：返回 true 表示放行（并计入窗口），false 表示超限。
    fn check(&self, username: &str, limit: u32) -> bool {
        if limit == 0 {
            return true; // 0 = 关闭限速
        }
        let now = Instant::now();
        let window = Duration::from_secs(RATE_WINDOW_SECS);
        let mut map = self.0.lock().unwrap();
        let queue = map.entry(username.to_string()).or_default();
        window_allow(queue, now, limit, window)
    }
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

    // ip 为 INET 列：项目未启用 sqlx 的 ipnet 特性，先转成文本再 `::inet` 交由 PostgreSQL 解析
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
    fn jwt_roundtrip() {
        let svc = JwtService::new("test-secret".into());
        let token = svc.issue("admin").unwrap();
        assert_eq!(svc.verify(&token).unwrap(), "admin");
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
