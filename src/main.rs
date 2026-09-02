use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use arc_swap::ArcSwap;
use axum::{middleware, routing::get, Router};
use tracing::{error, info, warn};

mod admin;
mod auth;
mod billing;
mod cache;
mod config;
mod crypto;
mod db;
mod entities;
mod error;
mod gateway;
mod limit;
mod logging;
mod media;
mod metrics;
mod protocol;
mod routing;
mod seed;
mod settings;
mod state;
mod stats;
mod upstream;

use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nextapi=info,tower_http=info".into()),
        )
        .init();

    // 1. 加载 YAML 配置（NEXTAPI_CONFIG 覆盖默认路径）
    let config_path = std::env::var("NEXTAPI_CONFIG").unwrap_or_else(|_| "config.yaml".into());
    let cfg = config::load_file(&config_path)?;
    info!(path = %config_path, "配置已加载");

    // 2. 敏感字段加密（env-only 密钥）
    let crypto = crypto::Crypto::from_env();
    if !crypto.is_available() {
        warn!("NEXTAPI_SECRET_KEY 未设置：无法保存/读取加密敏感字段（如代理密码）");
    }

    // 3. 数据库：连接（env DATABASE_URL 优先）→ 迁移 → 种子
    let db_url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.database.url.clone());
    if db_url.is_empty() {
        anyhow::bail!("数据库未配置：请设置 DATABASE_URL 或 config.yaml 的 database.url");
    }
    let pool = db::connect(&db_url).await?;
    db::migrate(&pool).await?;
    info!("数据库迁移完成");
    seed::seed_if_needed(&pool, &cfg, &crypto).await?;

    // 4. 设置引擎：hot 参数 UI > YAML 合并，原子替换生效配置
    let hot = Arc::new(ArcSwap::from_pointee(cfg.hot()));
    let engine = settings::SettingsEngine::init(pool.clone(), &cfg, hot.clone()).await?;

    // 5. 启动类参数：env > UI > YAML
    let (listen, listen_src) = engine.effective_startup("server.listen", &cfg.server.listen).await?;
    let (jwt_secret, jwt_src) =
        engine.effective_startup("server.admin_jwt_secret", &cfg.server.admin_jwt_secret).await?;
    if jwt_secret.is_empty() && !cfg.server.debug {
        anyhow::bail!(
            "生产模式必须配置 server.admin_jwt_secret 或环境变量 NEXTAPI_ADMIN_JWT_SECRET（或开启 server.debug）"
        );
    }
    info!(%listen, listen_src, jwt_src, "启动类参数解析完成");

    // 5.5 日志统计接线（M4-C）：WAL / 有界队列 / 分区预热。失败只 warn，不阻塞启动。
    let cfg_hot = cfg.hot();
    let gw = &cfg_hot.gateway;
    let (log_tx, log_rx) = tokio::sync::mpsc::channel(gw.log_queue_capacity.min(1_000_000).max(16));
    let wal = logging::wal::WalWriter::new(gw.log_wal_dir.clone().into(), gw.log_wal_file_max_mb);
    // 启动即重放 WAL（失败只 warn）
    if let Err(e) = logging::wal::replay_and_archive(wal.dir(), &pool, &gw.billing_timezone, gw.fx_stale_max_minutes).await {
        warn!("WAL 启动重放失败（忽略，继续启动）: {e}");
    }
    // 预热 usage_logs 分区（失败只 warn）
    if let Err(e) = logging::partition::ensure_partitions(&pool, gw.log_partition_days, 2).await {
        warn!("usage_logs 分区预热失败（忽略，继续启动）: {e}");
    }
    // 指标与日志出口：Metrics::new() 只建一次，复用进 state；sink 内部共享底层计数/深度句柄。
    let metrics = metrics::Metrics::new();
    let log_depth_gauge = metrics.log_queue_depth.clone();
    let sink = logging::LogSink::new(
        if gw.log_async { Some(log_tx) } else { None },
        pool.clone(),
        wal.clone(),
        metrics.log_overflows.clone(),
        metrics.log_queue_depth.clone(),
        gw.billing_timezone.clone(),
        gw.fx_stale_max_minutes,
    );

    let state = Arc::new(AppState {
        db: pool.clone(),
        hot,
        settings: engine.clone(),
        cache: {
            let c = cache::EntityCache::new();
            c.reload(&pool, &crypto).await?;
            c
        },
        limiter: limit::RateLimiter::new(),
        quota_cache: limit::QuotaCache::new(),
        breaker: routing::Breaker::new(),
        client_pools: upstream::ClientPools::new(),
        file_config: std::sync::RwLock::new(cfg.clone()),
        config_path: config_path.clone().into(),
        crypto,
        jwt: auth::JwtService::new(jwt_secret),
        metrics,
        log_sink: sink.clone(),
        version: env!("CARGO_PKG_VERSION"),
        started_at: chrono::Utc::now(),
    });

    // 5.6 启动日志后台写者与周期任务（失败只 warn）。
    // log_async=false 时 LogSink 无队列（直写），此处立即关闭 rx，writer 直接退出。
    let writer_handle = if gw.log_async {
        let hot_clone = state.hot.clone();
        let wal_clone = wal.clone();
        let pool_clone = pool.clone();
        Some(tokio::spawn(logging::writer::run_writer(
            pool_clone,
            log_rx,
            hot_clone,
            wal_clone,
            log_depth_gauge,
        )))
    } else {
        drop(log_rx);
        None
    };
    // 周期任务：每 60s 重放非活跃 WAL；每 6h 维护分区。
    let wal_dir = wal.dir().to_path_buf();
    let tz = gw.billing_timezone.clone();
    let part_days = gw.log_partition_days;
    let fx_stale = gw.fx_stale_max_minutes;
    let pool_replay = pool.clone();
    let pool_part = pool.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Err(e) = logging::wal::replay_and_archive(&wal_dir, &pool_replay, &tz, fx_stale).await {
                warn!("WAL 周期重放失败: {e}");
            }
        }
    });
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            if let Err(e) = logging::partition::ensure_partitions(&pool_part, part_days, 2).await {
                warn!("usage_logs 分区维护失败: {e}");
            }
        }
    });

    // M5：fx 自动刷新（enabled 且 refresh_interval_minutes>0 才启动；interval=0 仅手动 refresh）。
    // 每 interval 分钟按代理矩阵构建 client → billing::fx::fetch_and_store；失败 warn 且保留旧值（降级链）。
    {
        let hot = state.hot.load();
        let fx_cfg = hot.fx_auto_fetch.clone();
        drop(hot);
        let interval_minutes = fx_cfg.refresh_interval_minutes;
        if fx_cfg.enabled && interval_minutes > 0 {
            let state_fx = state.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(interval_minutes * 60)).await;
                    let cfg = state_fx.hot.load().fx_auto_fetch.clone();
                    let client = build_fx_client(&state_fx, &cfg);
                    if let Err(e) = billing::fx::fetch_and_store(&state_fx.db, &client, &cfg).await {
                        warn!("fx 自动刷新失败（保留旧值）: {e}");
                    }
                }
            });
        }
    }

    // M6：媒体任务轮询（图片异步任务闭环计费）。media_poller.interval_secs=0 时不启动
    // （poller 内部也会在读到 0 时自行退出，此处显式判断避免空转任务）。
    {
        let interval_secs = state.hot.load().media_poller.interval_secs;
        if interval_secs > 0 {
            let state_poller = state.clone();
            tokio::spawn(media::tasks::run_media_poller(state_poller));
        }
    }

    // 6. 配置文件热加载监听（仅 hot 类生效；不覆盖 UI 保存值与 DB 业务实体）
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    match config::watch(&config_path, tx) {
        Ok(_watcher) => {
            // watcher 泄漏至进程生命周期（需保持存活）
            std::mem::forget(_watcher);
            let engine2 = engine;
            let path2 = config_path.clone();
            let state2 = state.clone();
            tokio::spawn(async move {
                while rx.recv().await.is_some() {
                    match config::load_file(&path2) {
                        Ok(new_cfg) => {
                            if let Err(e) = engine2.on_file_reload(&new_cfg).await {
                                error!("配置热加载失败: {e}");
                            } else {
                                *state2.file_config.write().unwrap() = new_cfg;
                                info!("配置热加载完成（hot 参数已生效）");
                            }
                        }
                        Err(e) => error!("配置文件解析失败，保持原配置: {e}"),
                    }
                }
            });
        }
        Err(e) => warn!("配置文件监听不可用，热加载停用: {e}"),
    }

    // 7. 路由（state 之后被 with_state 消费，先保留 log_sink 供优雅关闭）
    let log_sink_for_close = state.log_sink.clone();
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics::handle))
        .nest("/api/auth", auth::router())
        .nest(
            "/api",
            admin::router().route_layer(middleware::from_fn_with_state(
                state.clone(),
                auth::require_admin,
            )),
        )
        .merge(gateway::router())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = listen.parse().map_err(|e| anyhow::anyhow!("监听地址非法 {listen}: {e}"))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "NextAPI 已启动");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    // 优雅关闭：close 后 await writer 退出（超时 10s，容忍后台排空）。
    log_sink_for_close.close();
    if let Some(handle) = writer_handle {
        let _ = tokio::time::timeout(Duration::from_secs(10), handle).await;
    }
    Ok(())
}

/// fx 拉取端点 URL（与 billing::fx::fetch_and_store 内部一致，用于 no_proxy 命中判定）。
fn fx_fetch_url(cfg: &crate::config::FxAutoFetchCfg) -> String {
    match cfg.provider.as_str() {
        "ecb" => "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml".into(),
        "custom" => format!("https://{}", cfg.base),
        _ => format!(
            "https://api.frankfurter.app/latest?base={}&symbols={}",
            cfg.base,
            cfg.symbols.join(",")
        ),
    }
}

/// 构建 fx 拉取 client：按 fx_auto_fetch.use_proxy/proxy_id（空则跟随 proxy.default_proxy_id）
/// + 全局 no_proxy 并集命中直连（参照 gateway.rs 的 client 选择写法）。
fn build_fx_client(state: &Arc<AppState>, cfg: &crate::config::FxAutoFetchCfg) -> reqwest::Client {
    let hot = state.hot.load();
    let snap = state.cache.snapshot();
    let url = fx_fetch_url(cfg);
    let mut lists = vec![hot.proxy.no_proxy.clone()];
    let eff_proxy_id = if cfg.use_proxy {
        Uuid::parse_str(&cfg.proxy_id)
            .ok()
            .or_else(|| Uuid::parse_str(&hot.proxy.default_proxy_id).ok())
    } else {
        None
    };
    if let Some(pid) = eff_proxy_id {
        if let Some(p) = snap.proxies.get(&pid) {
            lists.push(p.no_proxy.clone());
        }
    }
    let use_proxy = cfg.use_proxy && !crate::upstream::no_proxy_match(&lists, &url);
    if use_proxy {
        if let Some(pid) = eff_proxy_id {
            if let Some(p) = snap.proxies.get(&pid) {
                if let Some(client) = build_proxy_client(p) {
                    return client;
                }
            }
        }
    }
    state.client_pools.direct_client()
}

/// 由代理行构建 reqwest client；不可用/禁用 → None（回退直连）。
fn build_proxy_client(p: &crate::entities::ProxyRow) -> Option<reqwest::Client> {
    if !p.enabled {
        return None;
    }
    let proxy = reqwest::Proxy::all(p.proxy_url()).ok()?;
    reqwest::Client::builder().proxy(proxy).build().ok()
}

async fn healthz() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok" }))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("收到退出信号，正在优雅关闭");
}
