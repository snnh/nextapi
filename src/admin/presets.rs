//! 供应商预设 API（/api/presets，PLAN §7.2）。契约 contracts/m7-presets.md。
//!
//! - `GET /api/presets`：返回全部预设（含 media_base_url）；
//! - `POST /api/presets/{name}/provision`：body `{api_key, name?}`，api_key 空 → 400，
//!   name 缺省 = preset.name，调 presets::provision 一键接入（创建上游 + 加密 + 审计 + 探测）。
//!
//! 全部 SQL 使用运行时校验（sqlx::query / query_as），不使用 query! 宏。

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::auth::AdminUsername;
use crate::error::{ApiError, ApiResult};
use crate::presets;
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_presets))
        .route("/{name}/provision", post(provision_preset))
}

/// GET /api/presets：返回全部预设（含 media_base_url）。
async fn list_presets(
    State(_state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    let items: Vec<serde_json::Value> = presets::presets()
        .iter()
        .map(|p| {
            // protocol_base_urls 以对象（协议→URL）输出，前端直接展示/编辑
            let mut v = serde_json::json!({
                "name": p.name,
                "display_name": p.display_name,
                "kind": p.kind,
                "base_url": p.base_url,
                "protocols": p.protocols,
                "media_base_url": p.media_base_url,
                "description": p.description,
            });
            if let Some(pbu) = p.protocol_base_urls {
                let map: serde_json::Map<String, serde_json::Value> = pbu
                    .iter()
                    .map(|(k, u)| (k.to_string(), serde_json::Value::String(u.to_string())))
                    .collect();
                v["protocol_base_urls"] = serde_json::Value::Object(map);
            }
            v
        })
        .collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

/// POST /api/presets/{name}/provision 请求体。
#[derive(Debug, Deserialize)]
struct ProvisionReq {
    api_key: String,
    #[serde(default)]
    name: Option<String>,
}

/// POST /api/presets/{name}/provision：一键接入（预设不存在 → 404）。
async fn provision_preset(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(name): Path<String>,
    Json(body): Json<ProvisionReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let preset = presets::find(&name).ok_or(ApiError::NotFound)?;
    let result = presets::provision(
        &state,
        preset,
        &body.api_key,
        body.name.as_deref(),
        &admin.0,
    )
    .await?;
    Ok(Json(result))
}
