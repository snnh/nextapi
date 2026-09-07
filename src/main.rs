use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use arc_swap::ArcSwap;
use axum::{extract::DefaultBodyLimit, middleware, routing::get, Router};
use tracing::{error, info, warn};

mod admin;
mod auth;
mod billing;
mod cache;
mod config;
mod crypto;
mod db;
mod embed;
mod entities;
mod error;
mod gateway;
mod idempotency;
mod limit;
mod logging;
mod media;
mod metrics;
mod netguard;
mod presets;
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
    // PG 可能尚未就绪（compose 健康门外竞态/升级重启）：指数退避重试约 30s
    let pool = {
        let mut attempt = 0u32;
        loop {
            match db::connect(&db_url).await {
                Ok(p) => break p,
                Err(e) => {
                    attempt += 1;
                    if attempt >= 8 {
                        anyhow::bail!("数据库连接失败（重试 {attempt} 次后放弃）: {e}");
                    }
                    let wait = std::time::Duration::from_millis(500 << attempt.min(6));
                    warn!(attempt, ?wait, "数据库未就绪，重试中: {e}");
                    tokio::time::sleep(wait).await;
                }
            }
        }
    };
    db::migrate(&pool).await?;
    info!("数据库迁移完成");
    seed::seed_if_needed(&pool, &cfg, &crypto).await?;

    // 4. 设置引擎：hot 参数 UI > YAML 合并，原子替换生效配置
    let hot = Arc::new(ArcSwap::from_pointee(cfg.hot()));
    let engine =
        settings::SettingsEngine::init(pool.clone(), &cfg, hot.clone(), crypto.clone()).await?;

    // 5. 启动类参数：env > UI > YAML
    let (listen, listen_src) = engine
        .effective_startup("server.listen", &cfg.server.listen)
        .await?;
    let (jwt_secret, jwt_src) = engine
        .effective_startup("server.admin_jwt_secret", &cfg.server.admin_jwt_secret)
        .await?;
    // 已知占位值（docker-compose.yml / config.example.yaml 中的 CHANGE_ME）一律拒绝——
    // 公开仓库的固定占位串等同公开密钥，不可用于签名/加密。
    const KNOWN_PLACEHOLDERS: &[&str] =
        &["__CHANGE_ME__请替换为随机密钥__", "CHANGE_ME", "changeme"];
    let is_placeholder = |v: &str| KNOWN_PLACEHOLDERS.iter().any(|p| v.contains(p));
    let mut jwt_secret = jwt_secret;
    if is_placeholder(&jwt_secret) {
        anyhow::bail!("JWT 密钥仍是占位值，请替换为随机密钥（openssl rand -base64 48）");
    }
    if jwt_secret.is_empty() {
        if !cfg.server.debug {
            anyhow::bail!(
                "生产模式必须配置 server.admin_jwt_secret 或环境变量 NEXTAPI_ADMIN_JWT_SECRET（或开启 server.debug）"
            );
        }
        // debug 模式空密钥：生成随机临时密钥（重启会话失效，但不可伪造）
        let mut buf = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut buf);
        jwt_secret = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, buf);
        warn!("server.debug 且未配置 JWT 密钥：已生成随机临时密钥（重启后所有会话失效）");
    }
    {
        let sk = std::env::var("NEXTAPI_SECRET_KEY").unwrap_or_default();
        if is_placeholder(&sk) {
            anyhow::bail!(
                "NEXTAPI_SECRET_KEY 仍是占位值，请替换为随机密钥（openssl rand -base64 48）"
            );
        }
    }
    info!(%listen, listen_src, jwt_src, "启动类参数解析完成");

    // 5.5 日志统计接线（M4-C）：WAL / 有界队列 / 分区预热。失败只 warn，不阻塞启动。
    // 注意：这里必须取「引擎合并后」的 hot（UI > YAML），不能读原始 cfg.hot()，
    // 否则 log_wal_dir/log_queue_capacity/log_async 等 UI 覆盖重启后失效（review P1-#5）。
    let gw = hot.load().gateway.clone();
    let (log_tx, log_rx) = tokio::sync::mpsc::channel(gw.log_queue_capacity.clamp(16, 1_000_000));
    let wal = logging::wal::WalWriter::new(gw.log_wal_dir.clone().into(), gw.log_wal_file_max_mb);
    // 启动即重放 WAL（失败只 warn）。本进程尚未 append（WalWriter::new 无 IO），
    // 无活跃文件 → 旧文件重放成功后全部归档。
    if let Err(e) = logging::wal::replay_and_archive(
        wal.dir(),
        &pool,
        &gw.billing_timezone,
        gw.fx_stale_max_minutes,
        &wal,
    )
    .await
    {
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
        sticky: routing::StickyMap::default(),
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
    // 周期任务：每 60s 重放 WAL；每 6h 维护分区。
    // 归档判定由 WalWriter::archive_if_inactive 在写锁内完成（活跃/并发追加文件自动跳过）。
    let wal_dir = wal.dir().to_path_buf();
    let wal_periodic = wal.clone();
    let tz = gw.billing_timezone.clone();
    let part_days = gw.log_partition_days;
    let fx_stale = gw.fx_stale_max_minutes;
    let pool_replay = pool.clone();
    let pool_part = pool.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Err(e) = logging::wal::replay_and_archive(
                &wal_dir,
                &pool_replay,
                &tz,
                fx_stale,
                &wal_periodic,
            )
            .await
            {
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

    // M10.2：幂等键过期清理（每 10 分钟）
    {
        let pool_idem = pool.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(600)).await;
                match idempotency::cleanup_expired(&pool_idem).await {
                    Ok(n) if n > 0 => tracing::info!("幂等键过期清理: {n} 条"),
                    Ok(_) => {}
                    Err(e) => warn!("幂等键过期清理失败: {e}"),
                }
            }
        });
    }

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
                    if let Err(e) = billing::fx::fetch_and_store(&state_fx.db, &client, &cfg).await
                    {
                        warn!("fx 自动刷新失败（保留旧值）: {e}");
                    }
                }
            });
        }
    }

    // 渠道模型自动同步（model_sync='auto' 的上游定时对账托管路由）。
    // 任务内部每轮读 hot 配置，enabled=false/interval=0 时空转（热加载可再打开）。
    {
        let state_ms = state.clone();
        tokio::spawn(admin::model_sync::run_model_sync_task(state_ms));
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

    // M8：定时更新检查（内部按 update_check.enabled/interval_hours/repo 条件执行）。
    admin::system::spawn_scheduled_check(state.clone());

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
                    // 文件被删/短暂不可读（编辑器原子替换、ConfigMap 抖动）时保持
                    // last-good——load_file 对 NotFound 回落默认值，若直接采用会把
                    // 全部文件级 hot 参数静默重置（发布审阅运维 P2-3）。
                    if !std::path::Path::new(&path2).exists() {
                        error!("配置文件不存在，保持当前配置（last-good）: {path2}");
                        continue;
                    }
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
        .route("/readyz", get(readyz))
        // /metrics 纳入管理员鉴权（发布审阅运维 P1-4：公开暴露上游名/Key 前缀
        // 用量属租户级信息泄露）。Prometheus 抓取配置 bearer_token 即可。
        .route(
            "/metrics",
            get(metrics::handle).route_layer(middleware::from_fn_with_state(
                state.clone(),
                auth::require_admin,
            )),
        )
        .nest("/api/auth", auth::router())
        .nest(
            "/api",
            admin::router().route_layer(middleware::from_fn_with_state(
                state.clone(),
                auth::require_admin,
            )),
        )
        .merge(gateway::router())
        // M8：管理后台静态资源（web/dist 内嵌；SPA fallback）
        .fallback(embed::spa_fallback)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        // 单请求体上限 100 MiB（发布审阅 M7：16MiB 会误拒多图 base64 请求）；
        // map_response 把提取器产生的 413（纯文本、无 request_id）塑形为统一
        // JSON 错误体 + x-request-id。
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .layer(middleware::map_response(shape_payload_too_large))
        .with_state(state);

    let addr: SocketAddr = listen
        .parse()
        .map_err(|e| anyhow::anyhow!("监听地址非法 {listen}: {e}"))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "NextAPI 已启动");
    // 优雅关闭（发布审阅运维 P1-2）：HTTP drain 无上限会等所有活动连接
    // （含最长 600s 的上游 SSE），超过编排宽限被 SIGKILL 时日志全丢。
    // 信号到达即先 close 日志队列（此后新事件按「队列关闭」语义处理），
    // drain 设 25s 上限（与 docker/K8s 默认 10-30s 宽限对齐），超时放弃残留连接。
    let drain_notify = Arc::new(tokio::sync::Notify::new());
    // WithGracefulShutdown 实现的是 IntoFuture，需显式转换
    let serve =
        std::future::IntoFuture::into_future(axum::serve(listener, app).with_graceful_shutdown({
            let n = drain_notify.clone();
            async move { n.notified().await }
        }));
    tokio::pin!(serve);
    tokio::select! {
        res = &mut serve => { res?; }
        _ = shutdown_signal() => {
            info!("收到关闭信号：关闭日志队列并开始连接 drain（上限 25s）");
            log_sink_for_close.close();
            drain_notify.notify_one();
            if tokio::time::timeout(Duration::from_secs(25), &mut serve).await.is_err() {
                warn!("连接 drain 超过 25s，放弃残留连接（活动 SSE 被中断）");
            }
        }
    }
    // 等待游离写入任务（溢出 WAL/直写）落地，再排空 writer。
    log_sink_for_close.close();
    log_sink_for_close.wait_pending().await;
    if let Some(handle) = writer_handle {
        let _ = tokio::time::timeout(Duration::from_secs(10), handle).await;
    }
    Ok(())
}

/// 413 塑形（map_response）：提取器拒绝产生的纯文本 413 → 统一 JSON 错误体
/// + x-request-id（发布审阅 M7）。其余状态原样放行。
async fn shape_payload_too_large(resp: axum::response::Response) -> axum::response::Response {
    use axum::response::IntoResponse;
    if resp.status() != axum::http::StatusCode::PAYLOAD_TOO_LARGE {
        return resp;
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut out = (
        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        axum::Json(serde_json::json!({
            "error": {
                "message": "请求体超过大小限制 100MiB",
                "type": "nextapi_error",
                "code": null
            }
        })),
    )
        .into_response();
    if let Ok(v) = axum::http::HeaderValue::from_str(&request_id) {
        out.headers_mut().insert("x-request-id", v);
    }
    out
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

/// 就绪探针：数据库连接可用时才允许接收流量。
async fn readyz(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<axum::Json<serde_json::Value>, axum::http::StatusCode> {
    sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .map(|_| axum::Json(serde_json::json!({ "status": "ready" })))
        .map_err(|_| axum::http::StatusCode::SERVICE_UNAVAILABLE)
}

/// 优雅关闭：监听 SIGINT（Ctrl+C）与 SIGTERM（容器/K8s 默认停止信号），任一触发即关闭。
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!("SIGTERM 监听不可用: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    info!("收到退出信号，正在优雅关闭");
}
