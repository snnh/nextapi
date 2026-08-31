use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::{middleware, routing::get, Router};
use tracing::{error, info, warn};

mod admin;
mod auth;
mod config;
mod crypto;
mod db;
mod error;
mod metrics;
mod protocol;
mod seed;
mod settings;
mod state;

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

    let state = Arc::new(AppState {
        db: pool,
        hot,
        settings: engine.clone(),
        file_config: std::sync::RwLock::new(cfg.clone()),
        config_path: config_path.clone().into(),
        crypto,
        jwt: auth::JwtService::new(jwt_secret),
        metrics: metrics::Metrics::new(),
        version: env!("CARGO_PKG_VERSION"),
        started_at: chrono::Utc::now(),
    });

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

    // 7. 路由
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
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = listen.parse().map_err(|e| anyhow::anyhow!("监听地址非法 {listen}: {e}"))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "NextAPI 已启动");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn healthz() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok" }))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("收到退出信号，正在优雅关闭");
}
