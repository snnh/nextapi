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
    /// 上游凭证失效守卫（token 失效：内存隔离 + 后台可见，见 upstream::cred）
    pub cred_guard: crate::upstream::cred::AuthGuard,
    /// Key 级粘性路由表（M11.3，gateway.sticky_routing 开启时生效）
    pub sticky: crate::routing::StickyMap,
    /// 上游 HTTP 客户端池（按代理/直连分池）
    pub client_pools: ClientPools,
    /// Codex token 刷新 single-flight 锁表（per-upstream）
    pub codex_locks: crate::upstream::codex::CodexLockTable,
    /// 额度快照写入节流表（per-upstream：最近写入时间 + 内容指纹）
    pub quota_last: crate::upstream::quota::QuotaThrottle,
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

impl AppState {
    /// 记录上游凭证失效（token 失效四项之②③）：内存守卫立即生效（后续请求跳过该上游），
    /// 内容有变化时异步把详情写入 `upstreams.extra.auth_state`（同一次失效期内不重复写库）。
    pub fn mark_auth_failure(&self, id: uuid::Uuid, failure: crate::upstream::cred::AuthFailure) {
        if !self.cred_guard.mark(id, failure.clone()) {
            return;
        }
        let db = self.db.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::upstream::cred::store(&db, id, &failure).await {
                tracing::debug!(upstream = %id, error = %e, "写入凭证失效状态失败（不影响主链路）");
            }
        });
    }

    /// 清除凭证失效标记（凭证更新 / 请求或探测成功）：内存立即清，库里异步清。
    ///
    /// 返回内存中此前是否存在标记；库清理按需触发（无标记则不动 DB）。
    pub fn clear_auth_failure(&self, id: uuid::Uuid) -> bool {
        let had_live = self.cred_guard.clear(&id);
        if !had_live {
            return false;
        }
        let db = self.db.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::upstream::cred::clear_stored(&db, id).await {
                tracing::debug!(upstream = %id, error = %e, "清除凭证失效状态失败");
            }
        });
        true
    }
}
