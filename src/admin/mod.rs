//! 管理 API（/api/*，JWT 保护）。明确不存在任何运营接口。

use axum::Router;
use std::sync::Arc;

use crate::state::AppState;

pub mod audit;
pub mod proxies;
pub mod settings_api;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .nest("/proxies", proxies::router())
        .nest("/settings", settings_api::settings_router())
        .nest("/config", settings_api::config_router())
        .nest("/audit", audit::router())
}
