//! 渠道模型管理：拉取上游模型列表（协议适配）+ 路由同步引擎。
//!
//! - `GET /{id}/models`：实时拉取上游模型列表，标注每个模型的路由状态
//!   （new / routed_manual / routed_auto / routed_other / excluded）；
//! - `POST /{id}/models/sync`：自动模式下的全量对账（拉取 → 排除 → 增删托管路由）。
//!   手动模式拒绝（400），由 `POST /{id}/models/routes` 按需勾选创建手动路由；
//! - 周期任务 `run_model_sync_task`：对 model_sync='auto' 且启用的上游定时对账。
//!
//! 托管边界：同步引擎只增删 `managed_by='auto'` 的路由；同 pattern 的手动路由
//! （含指向其他上游的）一律跳过并在报告中列出，绝不覆盖。

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

use crate::auth::{self, AdminUsername};
use crate::entities::UpstreamRow;
use crate::error::{ApiError, ApiResult};
use crate::protocol::ir::Protocol;
use crate::state::AppState;
use crate::upstream;

/// 挂到 /api/upstreams 下的子路由。
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{id}/models", get(list_models_live))
        .route("/{id}/models/sync", post(sync_now))
        .route("/{id}/models/routes", post(add_manual_routes))
}

// ---------------------------------------------------------------------------
// 拉取上游模型列表（协议适配）
// ---------------------------------------------------------------------------

/// 按上游首个协议拉取模型列表：
/// URL 统一 `{base_url}/models`（Anthropic 为 /v1/models、Gemini 为 /v1beta/models，
/// 均由 base_url 携带版本前缀）；响应形状按协议适配：
/// OpenAI/Anthropic `{data:[{id}]}`；Gemini `{models:[{name:"models/x"}]}`（去前缀）。
pub async fn fetch_upstream_models(
    state: &Arc<AppState>,
    up: &UpstreamRow,
) -> Result<Vec<String>, String> {
    let snap = state.cache.snapshot();
    let client = {
        let hot = state.hot.load();
        state
            .client_pools
            .client_for(up, &snap, &hot.proxy.default_proxy_id)
    };
    let is_codex = up.kind == "codex";
    let proto = up
        .protocol_list()
        .first()
        .copied()
        .unwrap_or(Protocol::OpenaiChat);

    let url = format!("{}/models", up.base_url.trim_end_matches('/'));
    let mut headers = reqwest::header::HeaderMap::new();
    if is_codex {
        // Codex 渠道：OAuth 凭证（临期自动刷新）+ 必需头；覆盖 SSE Accept 为 JSON
        let (token, account) = upstream::codex::ensure_token(state, up).await?;
        upstream::codex::apply_headers(&mut headers, &token, &account);
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
    } else {
        upstream::apply_auth(&mut headers, proto, up.api_key_plain.as_deref());
    }

    let resp = client
        .get(&url)
        .headers(headers)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(200).collect();
        return Err(format!("HTTP {status}: {snippet}"));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("响应解析失败: {e}"))?;

    if is_codex {
        return Ok(parse_codex_models_response(&body));
    }
    Ok(parse_models_response(proto, &body))
}

/// 解析 Codex 模型目录响应：`{models:[{slug, visibility, supported_in_api, ...}]}`。
/// 仅保留可见（visibility 缺失或 == "list"）且支持 API（supported_in_api 缺失或 true）的模型 slug。
fn parse_codex_models_response(body: &serde_json::Value) -> Vec<String> {
    let mut models: Vec<String> = body
        .get("models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|m| {
                    m.get("visibility")
                        .and_then(|v| v.as_str())
                        .map(|v| v == "list")
                        .unwrap_or(true)
                        && m.get("supported_in_api")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true)
                })
                .filter_map(|m| m.get("slug").and_then(|s| s.as_str()))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    models.retain(|m| !m.is_empty());
    models.sort();
    models.dedup();
    models
}

/// 解析各协议 /models 响应为模型 ID 列表（排序去重、去空串）。
/// OpenAI/Anthropic `{data:[{id}]}`；Gemini `{models:[{name:"models/x"}]}`（去前缀）。
fn parse_models_response(proto: Protocol, body: &serde_json::Value) -> Vec<String> {
    let mut models: Vec<String> = match proto {
        Protocol::Gemini => body
            .get("models")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                    .map(|n| n.strip_prefix("models/").unwrap_or(n).to_string())
                    .collect()
            })
            .unwrap_or_default(),
        _ => body
            .get("data")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|n| n.as_str()))
                    .map(|n| n.to_string())
                    .collect()
            })
            .unwrap_or_default(),
    };
    models.retain(|m| !m.is_empty());
    models.sort();
    models.dedup();
    models
}

// ---------------------------------------------------------------------------
// 同步引擎（托管路由对账）
// ---------------------------------------------------------------------------

/// 同步报告（自动对账 / 手动添加共用返回形状）。
#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub fetched: usize,
    pub desired: usize,
    pub excluded: usize,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// 目标模型已存在路由（手动或其他上游），未覆盖
    pub skipped: Vec<String>,
    pub kept: usize,
}

/// 自动模式全量对账：desired = fetched − exclude；
/// 托管路由不在 desired → 删；desired 无任何路由 → 建（managed_by='auto'）；
/// desired 已有手动/他渠道路由 → 跳过不覆盖。
/// 同时回写 models_cache / models_fetched_at。
pub async fn sync_auto(
    state: &AppState,
    up_id: Uuid,
    actor: &str,
    fetched: Vec<String>,
) -> ApiResult<SyncReport> {
    #[derive(sqlx::FromRow)]
    struct UpSyncCols {
        name: String,
        model_exclude: Vec<String>,
    }
    let up =
        sqlx::query_as::<_, UpSyncCols>("SELECT name, model_exclude FROM upstreams WHERE id=$1")
            .bind(up_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(ApiError::NotFound)?;

    let exclude: std::collections::HashSet<&str> =
        up.model_exclude.iter().map(|s| s.as_str()).collect();
    let desired: Vec<&String> = fetched
        .iter()
        .filter(|m| !exclude.contains(m.as_str()))
        .collect();
    let excluded = fetched.len() - desired.len();

    #[derive(sqlx::FromRow)]
    struct RouteRow {
        id: Uuid,
        model_pattern: String,
        managed_by: Option<String>,
    }
    // 本上游的路由 + 其他上游同 pattern 的路由（用于冲突跳过）
    let own: Vec<RouteRow> = sqlx::query_as::<_, RouteRow>(
        "SELECT id, model_pattern, managed_by FROM model_routes WHERE upstream_id=$1",
    )
    .bind(up_id)
    .fetch_all(&state.db)
    .await?;
    let others: Vec<String> =
        sqlx::query_scalar("SELECT model_pattern FROM model_routes WHERE upstream_id<>$1")
            .bind(up_id)
            .fetch_all(&state.db)
            .await?;
    let others: std::collections::HashSet<String> = others.into_iter().collect();

    let desired_set: std::collections::HashSet<&str> = desired.iter().map(|s| s.as_str()).collect();

    let mut removed: Vec<String> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut kept = 0usize;

    let mut tx = state.db.begin().await?;
    // 1) 删除不再需要的托管路由
    for r in own
        .iter()
        .filter(|r| r.managed_by.as_deref() == Some("auto"))
    {
        if !desired_set.contains(r.model_pattern.as_str()) {
            sqlx::query("DELETE FROM model_routes WHERE id=$1")
                .bind(r.id)
                .execute(&mut *tx)
                .await?;
            removed.push(r.model_pattern.clone());
        } else {
            kept += 1;
        }
    }
    // 2) 新建缺失的托管路由（冲突跳过）
    let own_patterns: std::collections::HashSet<&str> =
        own.iter().map(|r| r.model_pattern.as_str()).collect();
    for m in &desired {
        if own_patterns.contains(m.as_str()) {
            continue; // 本上游已有（含托管保留项）
        }
        if others.contains(m.as_str()) {
            skipped.push((*m).clone());
            continue;
        }
        sqlx::query(
            "INSERT INTO model_routes \
             (model_pattern, upstream_id, priority, weight, enabled, retries, retry_status_codes, \
              lock_upstream, sort_order, managed_by) \
             VALUES ($1,$2,10,1,TRUE,2,'{429,500,502,503,504}',FALSE,0,'auto')",
        )
        .bind(*m)
        .bind(up_id)
        .execute(&mut *tx)
        .await?;
        added.push((*m).clone());
    }
    // 3) 回写模型缓存
    let cache_json = serde_json::to_value(&fetched).unwrap_or_default();
    sqlx::query("UPDATE upstreams SET models_cache=$1, models_fetched_at=now() WHERE id=$2")
        .bind(&cache_json)
        .bind(up_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth::audit(
        state,
        actor,
        "upstream.models.sync",
        "upstream",
        Some(&up_id.to_string()),
        serde_json::json!({
            "name": up.name, "fetched": fetched.len(), "added": added.len(),
            "removed": removed.len(), "skipped": skipped.len(),
        }),
        None,
    )
    .await?;

    Ok(SyncReport {
        fetched: fetched.len(),
        desired: desired.len(),
        excluded,
        added,
        removed,
        skipped,
        kept,
    })
}

// ---------------------------------------------------------------------------
// HTTP 处理器
// ---------------------------------------------------------------------------

/// GET /{id}/models：实时拉取 + 路由状态标注。拉取失败返回 502（前端回落 models_cache）。
async fn list_models_live(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let snap = state.cache.snapshot();
    let up = snap.upstreams.get(&id).cloned().ok_or(ApiError::NotFound)?;
    drop(snap);

    let models = fetch_upstream_models(&state, &up)
        .await
        .map_err(|e| ApiError::bad_gateway(format!("拉取上游模型列表失败: {e}")))?;

    // 状态标注：本上游托管/手动、他上游占用、排除名单
    let exclude: std::collections::HashSet<&str> =
        up.model_exclude.iter().map(|s| s.as_str()).collect();
    #[derive(sqlx::FromRow)]
    struct RouteMark {
        model_pattern: String,
        upstream_id: Uuid,
        managed_by: Option<String>,
    }
    let routes: Vec<RouteMark> = sqlx::query_as::<_, RouteMark>(
        "SELECT model_pattern, upstream_id, managed_by FROM model_routes",
    )
    .fetch_all(&state.db)
    .await?;
    let mark: std::collections::HashMap<String, (Uuid, Option<String>)> = routes
        .into_iter()
        .map(|r| (r.model_pattern, (r.upstream_id, r.managed_by)))
        .collect();

    let items: Vec<serde_json::Value> = models
        .iter()
        .map(|m| {
            let status = if exclude.contains(m.as_str()) {
                "excluded"
            } else {
                match mark.get(m) {
                    Some((uid, mb)) if *uid == id => {
                        if mb.as_deref() == Some("auto") {
                            "routed_auto"
                        } else {
                            "routed_manual"
                        }
                    }
                    Some(_) => "routed_other",
                    None => "new",
                }
            };
            serde_json::json!({ "name": m, "status": status })
        })
        .collect();

    // 拉取成功即回写缓存（供失败时回落展示）
    let cache_json = serde_json::to_value(&models).unwrap_or_default();
    let _ =
        sqlx::query("UPDATE upstreams SET models_cache=$1, models_fetched_at=now() WHERE id=$2")
            .bind(&cache_json)
            .bind(id)
            .execute(&state.db)
            .await;

    Ok(Json(serde_json::json!({
        "items": items,
        "fetched_at": Utc::now(),
        "model_sync": up.model_sync,
        "model_exclude": up.model_exclude,
    })))
}

/// POST /{id}/models/sync：自动模式立即对账（手动模式 400 引导用 routes 端点）。
async fn sync_now(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let snap = state.cache.snapshot();
    let up = snap.upstreams.get(&id).cloned().ok_or(ApiError::NotFound)?;
    drop(snap);
    if up.model_sync != "auto" {
        return Err(ApiError::bad_request(
            "该渠道为手动模式：请勾选模型后调 /models/routes 创建路由",
        ));
    }
    let models = fetch_upstream_models(&state, &up)
        .await
        .map_err(|e| ApiError::bad_gateway(format!("拉取上游模型列表失败: {e}")))?;
    let report = sync_auto(&state, id, &admin.0, models).await?;
    Ok(Json(serde_json::json!(report)))
}

/// 手动添加路由请求体。
#[derive(Debug, Deserialize)]
struct AddRoutesReq {
    models: Vec<String>,
}

/// POST /{id}/models/routes：为勾选模型创建手动路由（已存在同 pattern 路由则跳过）。
async fn add_manual_routes(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
    Json(body): Json<AddRoutesReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let snap = state.cache.snapshot();
    let up = snap.upstreams.get(&id).cloned().ok_or(ApiError::NotFound)?;
    drop(snap);
    if body.models.is_empty() || body.models.len() > 1000 {
        return Err(ApiError::bad_request("models 数量须为 1..=1000"));
    }
    for m in &body.models {
        if m.is_empty() || m.len() > 200 {
            return Err(ApiError::bad_request("模型名长度须为 1..=200"));
        }
    }

    let existing: Vec<String> =
        sqlx::query_scalar("SELECT model_pattern FROM model_routes WHERE model_pattern = ANY($1)")
            .bind(&body.models)
            .fetch_all(&state.db)
            .await?;
    let existing: std::collections::HashSet<String> = existing.into_iter().collect();

    let mut added: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut tx = state.db.begin().await?;
    for m in &body.models {
        if existing.contains(m) {
            skipped.push(m.clone());
            continue;
        }
        sqlx::query(
            "INSERT INTO model_routes \
             (model_pattern, upstream_id, priority, weight, enabled, retries, retry_status_codes, \
              lock_upstream, sort_order) \
             VALUES ($1,$2,10,1,TRUE,2,'{429,500,502,503,504}',FALSE,0)",
        )
        .bind(m)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        added.push(m.clone());
    }
    tx.commit().await?;

    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth::audit(
        &state,
        &admin.0,
        "upstream.models.add_routes",
        "upstream",
        Some(&id.to_string()),
        serde_json::json!({ "name": up.name, "added": added.len(), "skipped": skipped.len() }),
        None,
    )
    .await?;

    Ok(Json(serde_json::json!({
        "fetched": body.models.len(),
        "desired": body.models.len(),
        "excluded": 0,
        "added": added,
        "removed": Vec::<String>::new(),
        "skipped": skipped,
        "kept": 0,
    })))
}

// ---------------------------------------------------------------------------
// 周期任务：auto 渠道定时对账
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_codex_catalog_shape() {
        let body = serde_json::json!({"models":[
            {"slug":"gpt-5.1-codex","display_name":"GPT-5.1 Codex","visibility":"list","supported_in_api":true},
            {"slug":"gpt-5.1-codex-mini","visibility":"list","supported_in_api":true},
            {"slug":"gpt-5.1-codex-hidden","visibility":"hide","supported_in_api":true},
            {"slug":"gpt-5.1-codex-legacy","visibility":"list","supported_in_api":false},
            {"slug":"gpt-5.1-codex"},
            {"display_name":"无 slug 项"}
        ]});
        assert_eq!(
            parse_codex_models_response(&body),
            vec!["gpt-5.1-codex", "gpt-5.1-codex-mini"]
        );
        // 非目录形状（无 models 键）→ 空
        assert!(parse_codex_models_response(&serde_json::json!({"data":[]})).is_empty());
    }

    #[test]
    fn parse_openai_shape() {
        let body = serde_json::json!({"object":"list","data":[{"id":"gpt-4o"},{"id":"gpt-4o-mini"},{"id":""},{"id":"gpt-4o"}]});
        assert_eq!(
            parse_models_response(Protocol::OpenaiChat, &body),
            vec!["gpt-4o", "gpt-4o-mini"]
        );
    }

    #[test]
    fn parse_anthropic_shape() {
        let body = serde_json::json!({"data":[{"id":"claude-sonnet-4","type":"model"}]});
        assert_eq!(
            parse_models_response(Protocol::Anthropic, &body),
            vec!["claude-sonnet-4"]
        );
    }

    #[test]
    fn parse_gemini_shape_strips_prefix() {
        let body = serde_json::json!({"models":[{"name":"models/gemini-2.5-pro"},{"name":"models/gemini-2.5-flash"}]});
        assert_eq!(
            parse_models_response(Protocol::Gemini, &body),
            vec!["gemini-2.5-flash", "gemini-2.5-pro"]
        );
    }

    #[test]
    fn parse_unexpected_shape_empty() {
        let body = serde_json::json!({"unexpected": true});
        assert!(parse_models_response(Protocol::OpenaiChat, &body).is_empty());
        assert!(parse_models_response(Protocol::Gemini, &body).is_empty());
    }
}

/// 模型同步周期任务：每 interval_minutes 遍历 model_sync='auto' 且启用的上游，
/// 拉取模型列表并对账托管路由；单上游失败仅 warn，不影响其他上游。
/// interval=0 或 enabled=false 时空转退出（读 hot 配置，热生效于下一轮）。
pub async fn run_model_sync_task(state: Arc<AppState>) {
    loop {
        let cfg = state.hot.load().model_sync.clone();
        if !cfg.enabled || cfg.interval_minutes == 0 {
            // 配置关闭：长睡后轮询配置（热加载可重新打开）
            tokio::time::sleep(Duration::from_secs(60)).await;
            continue;
        }
        tokio::time::sleep(Duration::from_secs(cfg.interval_minutes * 60)).await;

        let targets: Vec<UpstreamRow> = {
            let snap = state.cache.snapshot();
            snap.upstreams
                .values()
                .filter(|u| u.model_sync == "auto" && u.enabled)
                .cloned()
                .collect()
        };
        for up in targets {
            match fetch_upstream_models(&state, &up).await {
                Ok(models) => {
                    if let Err(e) = sync_auto(&state, up.id, "system.model-sync", models).await {
                        warn!("渠道 `{}` 模型自动同步失败: {e}", up.name);
                    }
                }
                Err(e) => warn!("渠道 `{}` 模型列表拉取失败（跳过本轮同步）: {e}", up.name),
            }
        }
    }
}
