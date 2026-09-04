//! 种子初始化（seed 类配置，仅首次初始化写入 DB）。
//!
//! 以 `app_meta.seeded_at` 为标记，不采用"表空才写入"，
//! 避免管理员清空实体后重启被种子复活（PLAN.md §5.1）。
//!
//! 全部 SQL 使用运行时校验（sqlx::query），不使用 query! 宏（无编译期数据库）。

use crate::config::ConfigFile;
use crate::crypto::Crypto;
use rand::Rng;
use sqlx::{PgPool, Postgres, Transaction};

/// 若 app_meta.seeded_at 不存在，则执行种子写入并设置标记；否则直接返回。
/// 种子内容（M1 范围）：
/// - admin_users：admin.username + （password_hash 优先，其次 initial_password /
///   env NEXTAPI_ADMIN_INITIAL_PASSWORD，皆空则生成随机密码并打印一次到日志）；
/// - proxy_configs：cfg.proxies（密码经 Crypto 加密；有密码但无密钥时报错）；
/// - fx_rates：cfg.fx_rates（source=manual 等）。
pub async fn seed_if_needed(
    pool: &PgPool,
    cfg: &ConfigFile,
    crypto: &Crypto,
) -> anyhow::Result<()> {
    // 已有 seeded_at 标记 → 直接返回，防止种子复活
    let seeded = sqlx::query("SELECT key FROM app_meta WHERE key = 'seeded_at'")
        .fetch_optional(pool)
        .await?;
    if seeded.is_some() {
        return Ok(());
    }

    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

    seed_admin(&mut tx, cfg).await?;
    seed_proxies(&mut tx, cfg, crypto).await?;
    seed_fx_rates(&mut tx, cfg).await?;

    sqlx::query(
        "INSERT INTO app_meta (key, value) VALUES ('seeded_at', to_jsonb(now())) \
         ON CONFLICT (key) DO UPDATE SET value = to_jsonb(now()), updated_at = now()",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    tracing::info!("种子初始化完成（app_meta.seeded_at 已标记）");
    Ok(())
}

/// 管理员账号：仅当 username 不存在时插入（避免覆盖管理员后续修改）。
async fn seed_admin(tx: &mut Transaction<'_, Postgres>, cfg: &ConfigFile) -> anyhow::Result<()> {
    let exists = sqlx::query("SELECT username FROM admin_users WHERE username = $1")
        .bind(&cfg.admin.username)
        .fetch_optional(&mut **tx)
        .await?;
    if exists.is_some() {
        return Ok(());
    }
    let password_hash = resolve_admin_password_hash(cfg)?;
    sqlx::query("INSERT INTO admin_users (username, password_hash) VALUES ($1, $2)")
        .bind(&cfg.admin.username)
        .bind(&password_hash)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// 密码哈希优先级：password_hash（已是 argon2）> initial_password >
/// env NEXTAPI_ADMIN_INITIAL_PASSWORD > 生成随机密码（打印一次）。
fn resolve_admin_password_hash(cfg: &ConfigFile) -> anyhow::Result<String> {
    if !cfg.admin.password_hash.is_empty() {
        return Ok(cfg.admin.password_hash.clone());
    }
    if !cfg.admin.initial_password.is_empty() {
        return argon2_hash(&cfg.admin.initial_password);
    }
    if let Ok(p) = std::env::var("NEXTAPI_ADMIN_INITIAL_PASSWORD") {
        if !p.is_empty() {
            return argon2_hash(&p);
        }
    }
    let password = generate_random_password();
    tracing::warn!(
        username = %cfg.admin.username,
        password = %password,
        "初始管理员密码仅显示一次：未配置 admin.password_hash / admin.initial_password / \
         NEXTAPI_ADMIN_INITIAL_PASSWORD，已生成随机密码，请立即保存并修改"
    );
    argon2_hash(&password)
}

/// argon2（默认参数）计算 PHC 字符串。
/// 注意：password-hash 0.5 依赖 rand_core 0.6，与 rand 0.9 不兼容，
/// 因此使用 argon2 自带 re-export 的 OsRng。
fn argon2_hash(password: &str) -> anyhow::Result<String> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    use argon2::Argon2;
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 哈希失败: {e}"))?;
    Ok(hash.to_string())
}

/// 生成 16 位随机字母数字密码。
fn generate_random_password() -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..16)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
        .collect()
}

/// 代理配置：按 name 幂等；password 非空时必须经 Crypto 加密，密钥缺失则报错。
async fn seed_proxies(
    tx: &mut Transaction<'_, Postgres>,
    cfg: &ConfigFile,
    crypto: &Crypto,
) -> anyhow::Result<()> {
    for p in &cfg.proxies {
        let password_enc = match &p.password {
            Some(pw) if !pw.is_empty() => Some(crypto.encrypt(pw).map_err(|_| {
                anyhow::anyhow!(
                    "代理 `{}` 配置了密码，但 {} 未设置，无法加密存储",
                    p.name,
                    crate::crypto::ENV_SECRET_KEY
                )
            })?),
            _ => None,
        };
        sqlx::query(
            "INSERT INTO proxy_configs (name, kind, host, port, username, password_enc, no_proxy, enabled) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (name) DO NOTHING",
        )
        .bind(&p.name)
        .bind(&p.kind)
        .bind(&p.host)
        .bind(p.port as i32)
        .bind(&p.username)
        .bind(&password_enc)
        .bind(&p.no_proxy)
        .bind(p.enabled)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// 汇率：按 (from,to,source) 幂等；manual 的 fetched_at 为 NULL。
async fn seed_fx_rates(tx: &mut Transaction<'_, Postgres>, cfg: &ConfigFile) -> anyhow::Result<()> {
    for f in &cfg.fx_rates {
        let fetched_at: Option<chrono::DateTime<chrono::Utc>> = if f.source == "manual" {
            None
        } else {
            Some(chrono::Utc::now())
        };
        sqlx::query(
            "INSERT INTO fx_rates (currency_from, currency_to, rate, source, fetched_at) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (currency_from, currency_to, source) DO NOTHING",
        )
        .bind(&f.from)
        .bind(&f.to)
        .bind(f.rate)
        .bind(&f.source)
        .bind(fetched_at)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_password_format() {
        let p = generate_random_password();
        assert_eq!(p.len(), 16);
        assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn argon2_hash_verifiable() {
        use argon2::password_hash::{PasswordHash, PasswordVerifier};
        use argon2::Argon2;
        let h = argon2_hash("test-password").unwrap();
        let parsed = PasswordHash::new(&h).unwrap();
        assert!(Argon2::default()
            .verify_password(b"test-password", &parsed)
            .is_ok());
        assert!(Argon2::default()
            .verify_password(b"wrong", &parsed)
            .is_err());
    }

    #[test]
    fn password_priority_password_hash_first() {
        let cfg = ConfigFile {
            admin: crate::config::AdminCfg {
                username: "admin".into(),
                initial_password: "plain".into(),
                password_hash: "$argon2$pre-hashed".into(),
            },
            ..Default::default()
        };
        assert_eq!(
            resolve_admin_password_hash(&cfg).unwrap(),
            "$argon2$pre-hashed"
        );
    }
}
