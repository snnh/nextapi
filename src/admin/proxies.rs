//! 代理配置管理 API（/api/proxies，PLAN.md §5.9）。
//!
//! - 类型 http/https/socks5；端口 1..=65535；name/host 非空；
//! - 密码落库前经 Crypto AES-256-GCM 加密（`password_enc`），响应一律不返回 `password_enc`，
//!   改用 `has_password` 布尔表示；更新时密码语义：
//!     - 未传 / 掩码回传（`"***"`）→ 保持原值；
//!     - 空串 `""` → 清除密码；
//!     - 其他 → 视为新明文，加密更新；
//!   `NEXTAPI_SECRET_KEY` 未设置时禁止设置密码（返回 BadRequest）；
//! - POST /{id}/test：真实连通性测试——经代理请求 `proxy.probe_url` 白名单探测地址，
//!   返回可达性（ok/status/latency_ms/error，对齐 /api/upstreams/{id}/test）；
//! - 所有写操作写 admin_audit_logs（action 如 proxy.create/update/delete）。
//!
//! 全部 SQL 使用运行时校验（sqlx::query/keyword QueryBuilder），不使用 query! 宏。

use axum::{
    extract::{Path, State},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::auth::{self, AdminUsername};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// proxy_configs 行（含加密密码列，仅供内部使用；对外绝不返回）。
#[derive(Debug, FromRow)]
struct ProxyRow {
    id: Uuid,
    name: String,
    kind: String,
    host: String,
    port: i32,
    username: Option<String>,
    password_enc: Option<String>,
    no_proxy: Vec<String>,
    enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// 对外响应结构：不以明文/密文形式暴露密码，仅暴露 `has_password`。
#[derive(Debug, Serialize)]
struct ProxyOut {
    id: Uuid,
    name: String,
    kind: String,
    host: String,
    port: i32,
    username: Option<String>,
    has_password: bool,
    no_proxy: Vec<String>,
    enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn to_out(row: ProxyRow) -> ProxyOut {
    ProxyOut {
        id: row.id,
        name: row.name,
        kind: row.kind,
        host: row.host,
        port: row.port,
        username: row.username,
        has_password: row.password_enc.is_some(),
        no_proxy: row.no_proxy,
        enabled: row.enabled,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// 创建代理请求体。
#[derive(Debug, Deserialize)]
struct CreateProxyReq {
    name: String,
    kind: String,
    host: String,
    #[serde(default)]
    port: u16,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    no_proxy: Option<Vec<String>>,
    #[serde(default)]
    enabled: Option<bool>,
}

/// 更新代理请求体（全部字段 Option：已传才更新）。
#[derive(Debug, Deserialize)]
struct UpdateProxyReq {
    name: Option<String>,
    kind: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    username: Option<String>,
    password: Option<String>,
    no_proxy: Option<Vec<String>>,
    enabled: Option<bool>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_proxies).post(create_proxy))
        .route("/{id}", put(update_proxy).delete(delete_proxy))
        .route("/{id}/test", post(test_proxy))
}

/// GET /：列表，绝不返回 password_enc，改为 has_password。
async fn list_proxies(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = sqlx::query_as::<_, ProxyRow>(
        "SELECT id, name, kind, host, port, username, password_enc, no_proxy, enabled, \
         created_at, updated_at FROM proxy_configs ORDER BY name",
    )
    .fetch_all(&state.db)
    .await?;
    let items: Vec<ProxyOut> = rows.into_iter().map(to_out).collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

/// POST /：校验并创建代理。
async fn create_proxy(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Json(body): Json<CreateProxyReq>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_proxy(&body.name, &body.kind, &body.host, body.port)?;

    // 密码：非空则加密；密钥缺失 → BadRequest
    let password_enc = encrypt_password_option(&state, body.password.as_deref())?;

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO proxy_configs (name, kind, host, port, username, password_enc, no_proxy, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(&body.name)
    .bind(&body.kind)
    .bind(&body.host)
    .bind(body.port as i32)
    .bind(&body.username)
    .bind(&password_enc)
    .bind(body.no_proxy.as_deref().unwrap_or(&[]))
    .bind(body.enabled.unwrap_or(true))
    .fetch_one(&state.db)
    .await?;

    // 刷新内存快照（写路径失效刷新，网关/外联 client 从快照取代理行）
    state.cache.reload(&state.db, &state.crypto).await.map_err(ApiError::internal)?;

    auth::audit(
        &state,
        &admin.0,
        "proxy.create",
        "proxy",
        Some(&id.to_string()),
        serde_json::json!({ "name": body.name, "kind": body.kind, "host": body.host, "port": body.port }),
        None,
    )
    .await?;

    let row = fetch_proxy(&state, id).await?;
    Ok(Json(serde_json::json!(to_out(row))))
}

/// PUT /{id}：部分更新；密码语义见模块注释。
async fn update_proxy(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateProxyReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let row = fetch_proxy(&state, id).await?;

    // 合并出新的字段值（未传则沿用旧值）
    let name = body.name.unwrap_or(row.name.clone());
    let kind = body.kind.unwrap_or(row.kind.clone());
    let host = body.host.unwrap_or(row.host.clone());
    let port = body.port.unwrap_or(row.port as u16);
    validate_proxy(&name, &kind, &host, port)?;

    let username = body.username.or(row.username.clone());
    let no_proxy = body.no_proxy.unwrap_or_else(|| row.no_proxy.clone());
    let enabled = body.enabled.unwrap_or(row.enabled);

    // 密码语义：未传 / "***" 保持、"" 清除、其他加密更新
    let password_enc = match password_semantics(body.password.as_deref()) {
        PasswordSemantics::Keep => row.password_enc,
        PasswordSemantics::Clear => None,
        PasswordSemantics::Set(plain) => {
            if !state.crypto.is_available() {
                return Err(ApiError::bad_request(
                    "未设置 NEXTAPI_SECRET_KEY，无法保存代理密码",
                ));
            }
            Some(state.crypto.encrypt(&plain).map_err(|e| ApiError::bad_request(e.to_string()))?)
        }
    };

    sqlx::query(
        "UPDATE proxy_configs SET name = $1, kind = $2, host = $3, port = $4, username = $5, \
         password_enc = $6, no_proxy = $7, enabled = $8, updated_at = now() WHERE id = $9",
    )
    .bind(&name)
    .bind(&kind)
    .bind(&host)
    .bind(port as i32)
    .bind(&username)
    .bind(&password_enc)
    .bind(&no_proxy)
    .bind(enabled)
    .bind(id)
    .execute(&state.db)
    .await?;

    // 刷新内存快照（写路径失效刷新）
    state.cache.reload(&state.db, &state.crypto).await.map_err(ApiError::internal)?;

    auth::audit(
        &state,
        &admin.0,
        "proxy.update",
        "proxy",
        Some(&id.to_string()),
        serde_json::json!({ "name": name, "kind": kind, "host": host, "port": port, "enabled": enabled }),
        None,
    )
    .await?;

    let row = fetch_proxy(&state, id).await?;
    Ok(Json(serde_json::json!(to_out(row))))
}

/// DELETE /{id}：删除代理并审计。
async fn delete_proxy(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let res = sqlx::query("DELETE FROM proxy_configs WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    // 刷新内存快照（写路径失效刷新，避免已删代理仍被路由使用）
    state.cache.reload(&state.db, &state.crypto).await.map_err(ApiError::internal)?;

    auth::audit(
        &state,
        &admin.0,
        "proxy.delete",
        "proxy",
        Some(&id.to_string()),
        serde_json::json!({}),
        None,
    )
    .await?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /{id}/test：真实连通性测试——经代理请求 `proxy.probe_url` 白名单探测地址。
/// 返回形状与 /api/upstreams/{id}/test 一致（ok/status?/error?/latency_ms）。
async fn test_proxy(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    // 从内存快照取行（含解密后的 password_plain；proxy_url() 已带凭据），不存在 → 404。
    let snap = state.cache.snapshot();
    let row = snap.proxies.get(&id).cloned().ok_or(ApiError::NotFound)?;
    if !row.enabled {
        return Err(ApiError::bad_request("代理已禁用，无法测试"));
    }

    let probe_url = state.hot.load().proxy.probe_url.clone();

    // 代理 URL 解析失败 / Client 构建失败 → ok:false + error（不发探测请求）。
    let proxy = match reqwest::Proxy::all(row.proxy_url()) {
        Ok(p) => p,
        Err(e) => {
            return Ok(Json(serde_json::json!({
                "ok": false,
                "latency_ms": 0u64,
                "error": e.to_string(),
            })));
        }
    };
    let client = match reqwest::Client::builder().proxy(proxy).build() {
        Ok(c) => c,
        Err(e) => {
            return Ok(Json(serde_json::json!({
                "ok": false,
                "latency_ms": 0u64,
                "error": e.to_string(),
            })));
        }
    };

    let started = std::time::Instant::now();
    let resp = client.get(&probe_url).timeout(Duration::from_secs(10)).send().await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            Ok(Json(serde_json::json!({ "ok": status < 500, "status": status, "latency_ms": latency_ms })))
        }
        Err(e) => {
            Ok(Json(serde_json::json!({ "ok": false, "latency_ms": latency_ms, "error": e.to_string() })))
        }
    }
}

/// 校验代理字段：类型、端口、名称/主机。
fn validate_proxy(name: &str, kind: &str, host: &str, port: u16) -> Result<(), ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::bad_request("代理名称不能为空"));
    }
    if host.trim().is_empty() {
        return Err(ApiError::bad_request("代理主机不能为空"));
    }
    if !matches!(kind, "http" | "https" | "socks5") {
        return Err(ApiError::bad_request("代理类型必须为 http/https/socks5"));
    }
    if !(1..=65535).contains(&port) {
        return Err(ApiError::bad_request("端口必须在 1..=65535"));
    }
    Ok(())
}

/// 密码语义：未传或掩码回传 → Keep；空串 → Clear；其他 → Set(新明文)。
#[derive(Debug, PartialEq)]
enum PasswordSemantics {
    Keep,
    Clear,
    Set(String),
}

fn password_semantics(password: Option<&str>) -> PasswordSemantics {
    match password {
        None => PasswordSemantics::Keep,
        Some("***") => PasswordSemantics::Keep,
        Some("") => PasswordSemantics::Clear,
        Some(s) => PasswordSemantics::Set(s.to_string()),
    }
}

/// 创建路径的加密：password 为 None/空 → None；非空但密钥缺失 → BadRequest。
fn encrypt_password_option(state: &AppState, password: Option<&str>) -> Result<Option<String>, ApiError> {
    match password {
        Some(pw) if !pw.is_empty() => {
            if !state.crypto.is_available() {
                return Err(ApiError::bad_request(
                    "未设置 NEXTAPI_SECRET_KEY，无法保存代理密码",
                ));
            }
            let enc = state.crypto.encrypt(pw).map_err(|e| ApiError::bad_request(e.to_string()))?;
            Ok(Some(enc))
        }
        _ => Ok(None),
    }
}

/// 按 id 读取代理行；不存在 → NotFound。
async fn fetch_proxy(state: &AppState, id: Uuid) -> ApiResult<ProxyRow> {
    sqlx::query_as::<_, ProxyRow>(
        "SELECT id, name, kind, host, port, username, password_enc, no_proxy, enabled, \
         created_at, updated_at FROM proxy_configs WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_semantics_cases() {
        assert_eq!(password_semantics(None), PasswordSemantics::Keep);
        assert_eq!(password_semantics(Some("***")), PasswordSemantics::Keep);
        assert_eq!(password_semantics(Some("")), PasswordSemantics::Clear);
        assert_eq!(
            password_semantics(Some("new-pw")),
            PasswordSemantics::Set("new-pw".into())
        );
    }

    #[test]
    fn validate_basic() {
        assert!(validate_proxy("p", "http", "127.0.0.1", 8080).is_ok());
        assert!(validate_proxy("", "http", "127.0.0.1", 8080).is_err());
        assert!(validate_proxy("p", "ftp", "127.0.0.1", 8080).is_err());
        assert!(validate_proxy("p", "socks5", "127.0.0.1", 0).is_err());
        assert!(validate_proxy("p", "https", "", 80).is_err());
    }

    #[test]
    fn proxy_url_builds_with_credentials() {
        // test_proxy 依赖内存快照中 entities::ProxyRow 的 proxy_url()（含解密后凭据）。
        let row = crate::entities::ProxyRow {
            id: uuid::Uuid::new_v4(),
            name: "p".into(),
            kind: "http".into(),
            host: "proxy.example.com".into(),
            port: 8080,
            username: Some("user".into()),
            password_enc: None,
            password_plain: Some("pass".into()),
            no_proxy: Vec::new(),
            enabled: true,
        };
        assert_eq!(row.proxy_url(), "http://user:pass@proxy.example.com:8080");

        // 无凭据 + socks5 类型
        let row2 = crate::entities::ProxyRow {
            id: uuid::Uuid::new_v4(),
            name: "s".into(),
            kind: "socks5".into(),
            host: "10.0.0.1".into(),
            port: 1080,
            username: None,
            password_enc: None,
            password_plain: None,
            no_proxy: Vec::new(),
            enabled: true,
        };
        assert_eq!(row2.proxy_url(), "socks5://10.0.0.1:1080");
    }
}
