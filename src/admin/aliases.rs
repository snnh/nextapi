//! 模型别名管理 API（/api/aliases，M10.1 / PLAN.md §4.1）。
//!
//! - GET /：全量别名列表；
//! - POST /：新建（alias 唯一；与非通配路由 model_pattern 冲突 → 409；禁止链式：model
//!   不得等于另一别名）；写成功后刷新缓存快照并写审计；
//! - PUT /{id}：更新（同上新校验）；
//! - DELETE /{id}：删除。
//!
//! 运行时语义见 gateway.rs `resolve_alias`（单跳、仅 enabled、白名单按实际模型判定）。
//! 全部 SQL 使用运行时校验（sqlx::query / query_as），不使用 query! 宏。

use axum::{
    extract::{Path, State},
    routing::{get, put},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{self, AdminUsername};
use crate::entities::ModelAliasRow;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_aliases).post(create_alias))
        .route("/{id}", put(update_alias).delete(delete_alias))
}

/// 新建/更新请求体。
#[derive(Debug, Deserialize)]
struct AliasIn {
    alias: String,
    model: String,
    #[serde(default)]
    enabled: Option<bool>,
}

/// 校验别名/模型字段边界与命名规则。返回规整后的 (alias, model)。
fn validate_fields(alias: &str, model: &str) -> ApiResult<(String, String)> {
    let alias = alias.trim();
    let model = model.trim();
    if alias.is_empty() || alias.chars().count() > 255 {
        return Err(ApiError::bad_request("alias 不能为空且不能超过 255 字符"));
    }
    if model.is_empty() || model.chars().count() > 255 {
        return Err(ApiError::bad_request("model 不能为空且不能超过 255 字符"));
    }
    // 别名即客户端可见的模型名：字母数字开头，允许 . _ : / -（覆盖常见模型命名）
    let valid = alias
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'));
    if !valid {
        return Err(ApiError::bad_request(
            "alias 须以字母或数字开头，只能包含字母、数字与 . _ : / -",
        ));
    }
    if alias.contains('*') {
        return Err(ApiError::bad_request("alias 不能包含通配符 *"));
    }
    if alias == model {
        return Err(ApiError::bad_request("alias 与 model 相同则别名无意义"));
    }
    Ok((alias.to_string(), model.to_string()))
}

/// 冲突检测（M10.1）：别名不得与非通配路由 pattern 冲突、不得指向另一别名（禁链式）。
/// exclude_id 用于更新时排除自身。
async fn check_conflicts(
    state: &AppState,
    alias: &str,
    model: &str,
    exclude_id: Option<Uuid>,
) -> ApiResult<()> {
    // 1. 与非通配路由 pattern 冲突（同名同时是别名和路由 → 请求语义歧义）
    let route_hit: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM model_routes WHERE model_pattern = $1 AND position('*' in model_pattern) = 0 LIMIT 1",
    )
    .bind(alias)
    .fetch_optional(&state.db)
    .await?;
    if route_hit.is_some() {
        return Err(ApiError::Conflict(format!(
            "alias `{alias}` 与现有路由 model_pattern 冲突"
        )));
    }
    // 2. 禁止链式：model 不得等于（其它）别名
    let chain_hit: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM model_aliases WHERE alias = $1 AND ($2::uuid IS NULL OR id <> $2) LIMIT 1",
    )
    .bind(model)
    .bind(exclude_id)
    .fetch_optional(&state.db)
    .await?;
    if chain_hit.is_some() {
        return Err(ApiError::Conflict(format!(
            "model `{model}` 是另一别名，不支持链式别名"
        )));
    }
    Ok(())
}

/// GET /：全量别名（按创建时间升序）。
async fn list_aliases(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    let items: Vec<ModelAliasRow> = sqlx::query_as::<_, ModelAliasRow>(
        "SELECT id, alias, model, enabled, created_at, updated_at FROM model_aliases \
         ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

/// POST /：新建别名。
async fn create_alias(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Json(input): Json<AliasIn>,
) -> ApiResult<Json<serde_json::Value>> {
    let (alias, model) = validate_fields(&input.alias, &input.model)?;
    check_conflicts(&state, &alias, &model, None).await?;

    let row = sqlx::query_as::<_, ModelAliasRow>(
        "INSERT INTO model_aliases (alias, model, enabled) VALUES ($1, $2, $3) \
         RETURNING id, alias, model, enabled, created_at, updated_at",
    )
    .bind(&alias)
    .bind(&model)
    .bind(input.enabled.unwrap_or(true))
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        // alias UNIQUE 冲突 → 友好 409
        if let sqlx::Error::Database(db) = &e {
            if db.is_unique_violation() {
                return ApiError::Conflict(format!("alias `{alias}` 已存在"));
            }
        }
        ApiError::internal(e)
    })?;

    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth::audit(
        &state,
        &admin.0,
        "alias.create",
        "model_alias",
        Some(&row.id.to_string()),
        serde_json::json!({ "alias": row.alias, "model": row.model, "enabled": row.enabled }),
        None,
    )
    .await?;
    Ok(Json(serde_json::json!(row)))
}

/// PUT /{id}：更新别名。
async fn update_alias(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
    Json(input): Json<AliasIn>,
) -> ApiResult<Json<serde_json::Value>> {
    let (alias, model) = validate_fields(&input.alias, &input.model)?;
    check_conflicts(&state, &alias, &model, Some(id)).await?;

    let row = sqlx::query_as::<_, ModelAliasRow>(
        "UPDATE model_aliases SET alias = $2, model = $3, enabled = $4, updated_at = now() \
         WHERE id = $1 \
         RETURNING id, alias, model, enabled, created_at, updated_at",
    )
    .bind(id)
    .bind(&alias)
    .bind(&model)
    .bind(input.enabled.unwrap_or(true))
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(db) = &e {
            if db.is_unique_violation() {
                return ApiError::Conflict(format!("alias `{alias}` 已存在"));
            }
        }
        ApiError::internal(e)
    })?
    .ok_or(ApiError::NotFound)?;

    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth::audit(
        &state,
        &admin.0,
        "alias.update",
        "model_alias",
        Some(&id.to_string()),
        serde_json::json!({ "alias": row.alias, "model": row.model, "enabled": row.enabled }),
        None,
    )
    .await?;
    Ok(Json(serde_json::json!(row)))
}

/// DELETE /{id}：删除别名。
async fn delete_alias(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let row: Option<(String,)> =
        sqlx::query_as("DELETE FROM model_aliases WHERE id = $1 RETURNING alias")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let Some((alias,)) = row else {
        return Err(ApiError::NotFound);
    };

    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth::audit(
        &state,
        &admin.0,
        "alias.delete",
        "model_alias",
        Some(&id.to_string()),
        serde_json::json!({ "alias": alias }),
        None,
    )
    .await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_fields_rules() {
        // 正常
        let (a, m) = validate_fields("fast-gpt", "gpt-4o-mini").unwrap();
        assert_eq!(a, "fast-gpt");
        assert_eq!(m, "gpt-4o-mini");
        // 首尾空白规整
        assert!(validate_fields("  a1  ", " m2 ").is_ok());
        // 空/超长
        assert!(validate_fields("", "m").is_err());
        assert!(validate_fields("a", "").is_err());
        assert!(validate_fields(&"a".repeat(256), "m").is_err());
        // 非法字符/开头
        assert!(validate_fields("-abc", "m").is_err());
        assert!(validate_fields("a b", "m").is_err());
        // 通配符禁止；别名=模型无意义
        assert!(validate_fields("a*", "m").is_err());
        assert!(validate_fields("same", "same").is_err());
        // 常见命名形态允许
        assert!(validate_fields("a.b:c/d_e-1", "gpt-4o").is_ok());
    }
}
