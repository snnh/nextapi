//! 全局应用状态。

use arc_swap::ArcSwap;
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;

use crate::auth::JwtService;
use crate::config::{ConfigFile, HotConfig};
use crate::crypto::Crypto;
use crate::metrics::Metrics;
use crate::settings::SettingsEngine;

pub struct AppState {
    pub db: PgPool,
    /// 热配置（ArcSwap 无锁读；YAML 热加载或 UI 保存后原子替换）
    pub hot: Arc<ArcSwap<HotConfig>>,
    /// system_settings 持久化与优先级引擎
    pub settings: SettingsEngine,
    /// 最近一次加载的 YAML 配置（供 /api/config 导出）
    pub file_config: std::sync::RwLock<ConfigFile>,
    pub config_path: PathBuf,
    pub crypto: Crypto,
    pub jwt: JwtService,
    pub metrics: Metrics,
    pub version: &'static str,
    pub started_at: chrono::DateTime<chrono::Utc>,
}
