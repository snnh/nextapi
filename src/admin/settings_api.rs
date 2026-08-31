//! 系统设置 / 配置视图 API（PLAN.md §5.9 / §7.2）。
//!
//! - `/api/settings`：运行参数键值视图（UI 全配置读写通道，持久化 system_settings）；
//! - `/api/config`：整文件级 YAML 视图（导出掩码；导入/热加载），
//!   PUT 对掩码值语义 = 保持原值；`/api/config/reload` 只重载 hot 运行参数，
//!   永不覆盖 DB 中的业务实体。
//! 两者共享同一配置引擎，禁止各自维护一套默认值。

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;

use crate::auth;
use crate::config::{self, ConfigFile};
use crate::error::{ApiError, ApiResult};
use crate::settings::PutSettingsReq;
use crate::state::AppState;

/// GET / PUT /api/settings
pub fn settings_router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(get_settings).put(put_settings))
}

/// GET / PUT /api/config，POST /api/config/reload
pub fn config_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(get_config).put(put_config))
        .route("/reload", post(reload_config))
}

/// GET /api/settings：返回全量键视图（配置值 + 生效值 + 来源）。
async fn get_settings(State(state): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let items = state.settings.view().await?;
    Ok(Json(serde_json::json!({ "settings": items })))
}

/// PUT /api/settings：按键保存并热生效，返回更新后的视图。
async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PutSettingsReq>,
) -> ApiResult<Json<serde_json::Value>> {
    state.settings.put(req.settings).await?;
    let items = state.settings.view().await?;
    Ok(Json(serde_json::json!({ "settings": items })))
}

/// GET /api/config：整文件级 YAML 视图，敏感字段掩码后返回。
async fn get_config(State(state): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let cfg = state.file_config.read().unwrap().clone();
    let mut value = serde_json::to_value(&cfg).map_err(|e| ApiError::internal(e))?;
    config::mask_config_json(&mut value);
    Ok(Json(serde_json::json!({ "config": value })))
}

/// PUT /api/config：写入完整配置；掩码值（"***"）= 保持原值；写文件后热加载并审计。
async fn put_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    // 兼容两种形态：GET 返回的 {config: {...}} 或直接完整配置 JSON
    let mut value = body.get("config").cloned().unwrap_or(body);

    // 掩码字段按原值恢复（导出可安全回传）
    restore_masked_fields(&mut value, &state.file_config.read().unwrap());

    let new_cfg: ConfigFile = serde_json::from_value(value.clone())
        .map_err(|e| ApiError::bad_request(format!("配置解析失败: {e}")))?;

    // 写回配置文件（YAML）
    let yaml_str = serde_yaml::to_string(&new_cfg).map_err(|e| ApiError::internal(e))?;
    std::fs::write(&state.config_path, yaml_str).map_err(|e| ApiError::internal(e))?;

    // 热载 hot 参数 + 更新内存中的 file_config + 审计
    state.settings.on_file_reload(&new_cfg).await?;
    *state.file_config.write().unwrap() = new_cfg;
    auth::audit(&state, "admin", "config.put", "config", None, serde_json::json!({}), None).await?;

    // 返回新的 GET 结果
    let cfg = state.file_config.read().unwrap().clone();
    let mut value = serde_json::to_value(&cfg).map_err(|e| ApiError::internal(e))?;
    config::mask_config_json(&mut value);
    Ok(Json(serde_json::json!({ "config": value })))
}

/// POST /api/config/reload：仅重载 hot 运行参数，不覆盖 DB 业务实体。
async fn reload_config(State(state): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let cfg = config::load_file(&state.config_path.to_string_lossy())
        .map_err(|e| ApiError::bad_request(format!("配置加载失败: {e}")))?;
    state.settings.on_file_reload(&cfg).await?;
    *state.file_config.write().unwrap() = cfg;
    auth::audit(&state, "admin", "config.reload", "config", None, serde_json::json!({}), None).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 将 yaml 值转成 json 值（用于恢复 masked 字段）。
fn yaml_value_to_json(v: &serde_yaml::Value) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

/// 恢复掩码字段：值为 "***" 时从旧 file_config 恢复原值。
/// 覆盖：server.admin_jwt_secret、admin.password_hash、admin.initial_password、
///       upstreams[*].api_key、proxies[*].password（数组按下标对齐）。
fn restore_masked_fields(value: &mut serde_json::Value, old: &ConfigFile) {
    // server.admin_jwt_secret
    if let Some(v) = value.pointer_mut("/server/admin_jwt_secret") {
        if v.as_str() == Some("***") {
            *v = serde_json::Value::String(old.server.admin_jwt_secret.clone());
        }
    }
    // admin.password_hash / admin.initial_password
    for field in ["password_hash", "initial_password"] {
        let path = format!("/admin/{field}");
        if let Some(v) = value.pointer_mut(&path) {
            if v.as_str() == Some("***") {
                let old_val = if field == "password_hash" {
                    old.admin.password_hash.clone()
                } else {
                    old.admin.initial_password.clone()
                };
                *v = serde_json::Value::String(old_val);
            }
        }
    }
    // upstreams[*].api_key（按下标对齐，缺省保持 "***" 不还原）
    if let Some(ups) = value.get_mut("upstreams").and_then(|u| u.as_array_mut()) {
        for (i, item) in ups.iter_mut().enumerate() {
            if let Some(api) = item.get_mut("api_key") {
                if api.as_str() == Some("***") {
                    if let Some(o) = old.upstreams.get(i).and_then(|it| it.get("api_key")) {
                        *api = yaml_value_to_json(o);
                    }
                }
            }
        }
    }
    // proxies[*].password（按下标对齐）
    if let Some(proxies) = value.get_mut("proxies").and_then(|p| p.as_array_mut()) {
        for (i, item) in proxies.iter_mut().enumerate() {
            if let Some(pw) = item.get_mut("password") {
                if pw.as_str() == Some("***") {
                    let old_pw = old.proxies.get(i).and_then(|p| p.password.clone()).unwrap_or_default();
                    *pw = serde_json::Value::String(old_pw);
                }
            }
        }
    }
}
