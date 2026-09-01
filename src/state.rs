//! 全局应用状态。

use arc_swap::ArcSwap;
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;

use crate::auth::JwtService;
use crate::cache::EntityCache;
use crate::config::{ConfigFile, HotConfig};
use crate::crypto::Crypto;
use crate::limit::{QuotaCache, RateLimiter};
use crate::metrics::Metrics;
use crate::routing::Breaker;
use crate::settings::SettingsEngine;
use crate::upstream::ClientPools;

pub struct AppState {
    pub db: PgPool,
    /// 热配置（ArcSwap 无锁读；YAML 热加载或 UI 保存后原子替换）
    pub hot: Arc<ArcSwap<HotConfig>>,
    /// system_settings 持久化与优先级引擎
    pub settings: SettingsEngine,
    /// 业务实体内存快照（api_keys/upstreams/routes/proxies；请求路径零 DB 查询）
    pub cache: EntityCache,
    /// RPM 滑窗限流器
    pub limiter: RateLimiter,
    /// 用量上限判定缓存
    pub quota_cache: QuotaCache,
    /// 上游熔断器
    pub breaker: Breaker,
    /// 上游 HTTP 客户端池（按代理/直连分池）
    pub client_pools: ClientPools,
    /// 最近一次加载的 YAML 配置（供 /api/config 导出）
    pub file_config: std::sync::RwLock<ConfigFile>,
    pub config_path: PathBuf,
    pub crypto: Crypto,
    pub jwt: JwtService,
    pub metrics: Metrics,
    /// M4 日志统计出口句柄（非阻塞记账）。
    pub log_sink: crate::logging::LogSink,
    pub version: &'static str,
    pub started_at: chrono::DateTime<chrono::Utc>,
}
