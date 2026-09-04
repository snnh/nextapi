//! 管理操作审计查询（/api/audit）：独立表 admin_audit_logs，分页 + 过滤。
//!
//! 查询参数：page（默认 1）、page_size（默认 50，上限 200）、action?、object_type?；
//! 按 created_at DESC 分页，返回 { items, total, page, page_size }；
//! 返回行通过 LEFT JOIN admin_users 附带 admin 用户名（admin_name）。
//!
//! 全部 SQL 使用运行时校验（sqlx::query / QueryBuilder），不使用 query! 宏。

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::AdminUsername;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// 审计日志行（含 LEFT JOIN 出的 admin 用户名）。
#[derive(Debug, FromRow)]
struct AuditRow {
    id: i64,
    admin_id: Option<Uuid>,
    admin_name: Option<String>,
    action: String,
    object_type: Option<String>,
    object_id: Option<String>,
    summary: Option<serde_json::Value>,
    ip: Option<String>,
    created_at: DateTime<Utc>,
}

/// 对外返回的审计条目。
#[derive(Debug, Serialize)]
struct AuditItem {
    id: i64,
    admin_id: Option<Uuid>,
    admin_name: Option<String>,
    action: String,
    object_type: Option<String>,
    object_id: Option<String>,
    summary: Option<serde_json::Value>,
    ip: Option<String>,
    created_at: DateTime<Utc>,
}

/// `/api/audit` 查询参数。
#[derive(Debug, Deserialize)]
struct AuditQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    action: Option<String>,
    object_type: Option<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(list_audit))
}

/// GET /api/audit：分页 + 过滤返回审计日志。
async fn list_audit(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Query(params): Query<AuditQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(50).clamp(1, 200);
    // page 上限防深 OFFSET 慢查询（review P2-12）
    if page > 100_000 {
        return Err(ApiError::bad_request("page 不能超过 100000"));
    }
    let offset = ((page as i64) - 1) * (page_size as i64);

    // 总数（与列表共用同一过滤条件）
    let mut count_qb =
        QueryBuilder::<Postgres>::new("SELECT count(*) FROM admin_audit_logs l WHERE 1 = 1");
    push_filters(&mut count_qb, &params);
    let total: i64 = count_qb.build_query_scalar().fetch_one(&state.db).await?;

    // 数据页（LEFT JOIN 附带 admin 用户名）
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT l.id, l.admin_id, l.action, l.object_type, l.object_id, l.summary, \
         l.ip::text AS ip, l.created_at, u.username AS admin_name \
         FROM admin_audit_logs l \
         LEFT JOIN admin_users u ON u.id = l.admin_id \
         WHERE 1 = 1",
    );
    push_filters(&mut qb, &params);
    qb.push(" ORDER BY l.created_at DESC LIMIT ")
        .push_bind(page_size as i64);
    qb.push(" OFFSET ").push_bind(offset);
    let rows = qb.build_query_as::<AuditRow>().fetch_all(&state.db).await?;

    let items: Vec<AuditItem> = rows
        .into_iter()
        .map(|r| AuditItem {
            id: r.id,
            admin_id: r.admin_id,
            admin_name: r.admin_name,
            action: r.action,
            object_type: r.object_type,
            object_id: r.object_id,
            summary: r.summary,
            ip: r.ip,
            created_at: r.created_at,
        })
        .collect();

    Ok(Json(serde_json::json!({
        "items": items,
        "total": total,
        "page": page,
        "page_size": page_size,
    })))
}

/// 追加公共过滤条件：可选 action / object_type（空串视为未提供）。
/// 绑定前 clone 为属主值，避免借用生命周期与 QueryBuilder 的生命周期纠缠。
fn push_filters(qb: &mut QueryBuilder<'_, Postgres>, params: &AuditQuery) {
    if let Some(action) = params.action.as_ref().filter(|s| !s.is_empty()) {
        qb.push(" AND l.action = ").push_bind(action.clone());
    }
    if let Some(object_type) = params.object_type.as_ref().filter(|s| !s.is_empty()) {
        qb.push(" AND l.object_type = ")
            .push_bind(object_type.clone());
    }
}
