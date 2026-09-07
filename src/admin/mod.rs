//! 管理 API（/api/*，JWT 保护）。明确不存在任何运营接口。

use axum::Router;
use std::sync::Arc;

use crate::state::AppState;

pub mod aliases;
pub mod audit;
pub mod fx;
pub mod keys;
pub mod logs;
pub mod model_sync;
pub mod presets;
pub mod pricing;
pub mod proxies;
pub mod routes;
pub mod settings_api;
pub mod stats_api;
pub mod system;
pub mod upstreams;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .nest("/keys", keys::router())
        .nest(
            "/upstreams",
            upstreams::router().merge(model_sync::router()),
        )
        .nest("/model-routes", routes::router())
        .nest("/aliases", aliases::router())
        .nest("/proxies", proxies::router())
        .nest("/settings", settings_api::settings_router())
        .nest("/config", settings_api::config_router())
        .nest("/audit", audit::router())
        .nest("/logs", logs::router())
        .nest("/stats", stats_api::router())
        .nest("/pricing", pricing::router())
        .nest("/fx", fx::router())
        .nest("/presets", presets::router())
        .nest("/system", system::router())
}
