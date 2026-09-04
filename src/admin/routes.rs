//! 模型路由管理 API（/api/model-routes，PLAN.md §5.3 / §7.2）。
//!
//! - GET /：全量路由（附 `upstream_name`，JOIN upstreams 或内存合并）；
//! - PUT /：整体批量替换 —— body 为路由数组，事务内 DELETE 全表 + 逐条 INSERT；
//!   校验每个 `upstream_id` 存在、`weight > 0`；提交后 `state.cache.reload` 刷新快照并写审计
//!   （action: route.replace）。
//!
//! 全部 SQL 使用运行时校验（sqlx::query / query_as），不使用 query! 宏。

use axum::{extract::State, routing::get, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{self, AdminUsername};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(list_routes).put(replace_routes))
}

/// 路由输出行（含上游名称）。
#[derive(Debug, FromRow, Serialize)]
struct RouteOut {
    id: Uuid,
    model_pattern: String,
    upstream_id: Uuid,
    upstream_name: String,
    override_model: Option<String>,
    priority: i32,
    weight: i32,
    enabled: bool,
    retries: i32,
    retry_status_codes: Vec<i32>,
    lock_upstream: bool,
    sort_order: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// 批量替换请求中的单个路由配置。
#[derive(Debug, Deserialize)]
struct RouteItem {
    model_pattern: String,
    upstream_id: Uuid,
    #[serde(default)]
    override_model: Option<String>,
    #[serde(default)]
    priority: Option<i32>,
    #[serde(default)]
    weight: Option<i32>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    retries: Option<i32>,
    #[serde(default)]
    retry_status_codes: Option<Vec<i32>>,
    #[serde(default)]
    lock_upstream: Option<bool>,
    #[serde(default)]
    sort_order: Option<i32>,
}

/// GET /：全量路由（JOIN upstreams 取上游名）。
async fn list_routes(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    let items: Vec<RouteOut> = sqlx::query_as::<_, RouteOut>(
        "SELECT r.id, r.model_pattern, r.upstream_id, u.name AS upstream_name, r.override_model, \
         r.priority, r.weight, r.enabled, r.retries, r.retry_status_codes, r.lock_upstream, \
         r.sort_order, r.created_at, r.updated_at \
         FROM model_routes r JOIN upstreams u ON u.id = r.upstream_id \
         ORDER BY r.sort_order, r.priority, r.created_at",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

/// PUT /：整体批量替换（事务：DELETE 全表 + 逐条 INSERT）。
async fn replace_routes(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Json(routes): Json<Vec<RouteItem>>,
) -> ApiResult<Json<serde_json::Value>> {
    // 1. 校验 weight > 0 与字段边界（长度/数量上限，review P2）
    if routes.len() > 5000 {
        return Err(ApiError::bad_request("路由总数不能超过 5000"));
    }
    for r in &routes {
        if r.weight.unwrap_or(1) <= 0 {
            return Err(ApiError::bad_request(format!(
                "model_pattern `{}` 的 weight 必须 > 0",
                r.model_pattern
            )));
        }
        if r.model_pattern.trim().is_empty() {
            return Err(ApiError::bad_request("model_pattern 不能为空"));
        }
        if r.model_pattern.chars().count() > 255 {
            return Err(ApiError::bad_request("model_pattern 不能超过 255 字符"));
        }
        if r.override_model
            .as_deref()
            .map_or(false, |m| m.chars().count() > 255)
        {
            return Err(ApiError::bad_request("override_model 不能超过 255 字符"));
        }
        if r.retry_status_codes.as_ref().map_or(0, Vec::len) > 20 {
            return Err(ApiError::bad_request("retry_status_codes 最多 20 个状态码"));
        }
    }

    // 2. 校验所有 upstream_id 存在（多条路由可指向同一上游，故先去重再比对）
    let ids: Vec<Uuid> = routes.iter().map(|r| r.upstream_id).collect();
    let unique_count = ids.iter().collect::<std::collections::HashSet<_>>().len();
    let found: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM upstreams WHERE id = ANY($1)")
        .bind(&ids)
        .fetch_all(&state.db)
        .await?;
    if found.len() != unique_count {
        return Err(ApiError::bad_request("存在无效的 upstream_id"));
    }

    // 3. 事务内替换
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM model_routes")
        .execute(&mut *tx)
        .await?;
    for r in &routes {
        let retry_codes: &[i32] = r
            .retry_status_codes
            .as_deref()
            .unwrap_or(&[429, 500, 502, 503, 504]);
        sqlx::query(
            "INSERT INTO model_routes \
             (model_pattern, upstream_id, override_model, priority, weight, enabled, retries, \
              retry_status_codes, lock_upstream, sort_order) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(&r.model_pattern)
        .bind(r.upstream_id)
        .bind(&r.override_model)
        .bind(r.priority.unwrap_or(10))
        .bind(r.weight.unwrap_or(1))
        .bind(r.enabled.unwrap_or(true))
        .bind(r.retries.unwrap_or(2))
        .bind(retry_codes)
        .bind(r.lock_upstream.unwrap_or(false))
        .bind(r.sort_order.unwrap_or(0))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    // 4. 刷新快照 + 审计 + 返回新列表
    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth::audit(
        &state,
        &admin.0,
        "route.replace",
        "route",
        None,
        serde_json::json!({ "count": routes.len() }),
        None,
    )
    .await?;

    let items: Vec<RouteOut> = sqlx::query_as::<_, RouteOut>(
        "SELECT r.id, r.model_pattern, r.upstream_id, u.name AS upstream_name, r.override_model, \
         r.priority, r.weight, r.enabled, r.retries, r.retry_status_codes, r.lock_upstream, \
         r.sort_order, r.created_at, r.updated_at \
         FROM model_routes r JOIN upstreams u ON u.id = r.upstream_id \
         ORDER BY r.sort_order, r.priority, r.created_at",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "items": items })))
}
