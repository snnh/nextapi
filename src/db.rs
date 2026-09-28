//! 数据库连接池与迁移。
//!
//! 注意：不使用 sqlx 编译期校验宏（query!），所有查询走运行时校验，
//! 以便无数据库环境下也能构建（CI/本地）。

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// 建连接池。`max_connections` 由调用方从 `database.max_connections` /
/// `NEXTAPI_DB_MAX_CONNECTIONS` 解析，并经 [`crate::config::pool_max_connections`] 钳制
/// （1–256，默认 16）——内存受限的部署可调低，高并发场景可调高。
pub async fn connect(url: &str, max_connections: u32) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(crate::config::clamp_pool_max_connections(max_connections))
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(url)
        .await?;
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
