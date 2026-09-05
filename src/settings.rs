//! system_settings 持久化与配置优先级引擎（PLAN.md §5.1 / §5.9）。
//!
//! 优先级：
//! - hot 运行参数：**UI 保存值 > YAML 热加载值**（文件变化只更新 source=file 的键）；
//! - 启动类参数（RESTART_REQUIRED_KEYS）：**env > UI > YAML**（env 仅启动时读取，
//!   UI 显示 source=env 且只读提示）；
//! - `NEXTAPI_SECRET_KEY` 为唯一 env-only 例外，不进入本模块。
//!
//! 全部 SQL 使用运行时校验（sqlx::query），不使用 query! 宏（无编译期数据库）。

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::config::{ConfigFile, HotConfig};
use crate::crypto::Crypto;
use crate::error::ApiResult;

/// GET /api/settings 的单项视图。
#[derive(Debug, Clone, Serialize)]
pub struct SettingItem {
    /// 点分键，如 "gateway.display_currency"
    pub key: String,
    /// 生效值（已按优先级合并；secret 项掩码）
    pub value: serde_json::Value,
    /// file | ui | env
    pub source: String,
    /// 是否需重启才生效（启动类参数）
    pub restart_required: bool,
    /// 是否敏感字段（掩码展示，回传掩码值 = 保持原值）
    pub secret: bool,
}

/// PUT /api/settings 的请求体。
#[derive(Debug, Deserialize)]
pub struct PutSettingsReq {
    pub settings: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone)]
pub struct SettingsEngine {
    pool: PgPool,
    /// 最近一次加载的 YAML hot 配置（热加载时更新）
    yaml_hot: Arc<RwLock<HotConfig>>,
    /// 最近一次 YAML 中的启动类键值（server.listen / database.url / server.admin_jwt_secret）
    yaml_startup: Arc<RwLock<HashMap<String, String>>>,
    /// 生效中的 hot 配置（与 AppState.hot 共享同一 Arc）
    effective: Arc<ArcSwap<HotConfig>>,
    /// 敏感字段（server.admin_jwt_secret）落库前 AES-256-GCM 加密（review P4：DB 明文
    /// 泄露即可伪造管理员 JWT，与上游 api_key / 代理密码的落库加密约定保持一致）
    crypto: Crypto,
}

/// 敏感键落库前加密（返回带 `enc1:` 前缀的密文；非敏感/空值/加密不可用 → 原样返回）。
fn seal_secret_value(crypto: &Crypto, key: &str, value: &str) -> String {
    if key_is_secret(key) && !value.is_empty() {
        if let Ok(enc) = crypto.encrypt(value) {
            return format!("enc1:{enc}");
        }
    }
    value.to_string()
}

/// 敏感键读取时解密（兼容旧版明文行：无 `enc1:` 前缀原样返回）。解密失败返回 Err，
/// 由调用方降级（跳过该 UI 覆盖并告警），不中断启动。
fn open_secret_value(crypto: &Crypto, key: &str, value: &str) -> Result<String, String> {
    if key_is_secret(key) {
        if let Some(enc) = value.strip_prefix("enc1:") {
            return crypto.decrypt(enc).map_err(|e| e.to_string());
        }
    }
    Ok(value.to_string())
}

impl SettingsEngine {
    /// 初始化：
    /// 1. 将 YAML hot 键初始化到 system_settings（缺失才插入，source=file）；
    /// 2. 确保 3 个启动类键也存在（source=file、restart_required=true；
    ///    server.admin_jwt_secret 另标 secret=true，落库前加密）；
    /// 3. 读取全部 UI 保存值（source=ui），按 UI > YAML 计算生效 HotConfig；
    /// 4. 原子替换 effective。
    pub async fn init(
        pool: PgPool,
        cfg: &ConfigFile,
        effective: Arc<ArcSwap<HotConfig>>,
        crypto: Crypto,
    ) -> anyhow::Result<Self> {
        let yaml_hot = cfg.hot();

        // 1. 初始化全部 hot 键（缺失才插入，source=file；保留已有 ui 覆盖）
        for (key, value) in crate::config::hot_flat(&yaml_hot) {
            sqlx::query(
                "INSERT INTO system_settings (key, value, source) VALUES ($1, $2, 'file') \
                 ON CONFLICT (key) DO NOTHING",
            )
            .bind(&key)
            .bind(&value)
            .execute(&pool)
            .await?;
        }

        // 2. 确保 3 个启动类键存在（source=file、需重启；admin_jwt_secret 略敏感 → 落库前加密）
        for key in crate::config::RESTART_REQUIRED_KEYS {
            let value = startup_yaml_value(cfg, key).unwrap_or_default();
            let secret = key_is_secret(key);
            let stored = seal_secret_value(&crypto, key, &value);
            if stored != value {
                tracing::debug!(%key, "启动类敏感键已加密落库");
            } else if secret && !value.is_empty() && !crypto.is_available() {
                tracing::warn!(
                    %key,
                    "未配置 {}，{key} 将以明文形式存入 system_settings（建议生产环境配置后重新保存）",
                    crate::crypto::ENV_SECRET_KEY
                );
            }
            sqlx::query(
                "INSERT INTO system_settings (key, value, source, secret, restart_required, updated_at) \
                 VALUES ($1, $2, 'file', $3, TRUE, now()) \
                 ON CONFLICT (key) DO NOTHING",
            )
            .bind(key)
            .bind(serde_json::Value::String(stored))
            .bind(secret)
            .execute(&pool)
            .await?;
        }

        let engine = Self {
            pool,
            yaml_hot: Arc::new(RwLock::new(yaml_hot.clone())),
            yaml_startup: Arc::new(RwLock::new(startup_yaml_map(cfg))),
            effective,
            crypto,
        };

        // 3. 计算生效配置：YAML hot + 全部 UI 覆盖（单行失败警告并跳过）
        let merged = engine.merge_effective(&yaml_hot).await?;
        engine.effective.store(Arc::new(merged));
        Ok(engine)
    }

    /// YAML 文件热加载：仅更新 source=file 的键，重算生效配置。
    pub async fn on_file_reload(&self, cfg: &ConfigFile) -> ApiResult<()> {
        let new_hot = cfg.hot();
        for (key, value) in crate::config::hot_flat(&new_hot) {
            // 仅覆盖 source=file 的行；存在 ui 覆盖的行保持不变
            let res = sqlx::query(
                "UPDATE system_settings SET value = $1, updated_at = now() \
                 WHERE key = $2 AND source = 'file'",
            )
            .bind(&value)
            .bind(&key)
            .execute(&self.pool)
            .await?;
            if res.rows_affected() == 0 {
                // 无 file 行：视作新出现的键插入（若已存在 ui 行则保留）
                sqlx::query(
                    "INSERT INTO system_settings (key, value, source) VALUES ($1, $2, 'file') \
                     ON CONFLICT (key) DO NOTHING",
                )
                .bind(&key)
                .bind(&value)
                .execute(&self.pool)
                .await?;
            }
        }

        // 更新 YAML 快照（hot 与启动类）
        *self.yaml_hot.write().unwrap() = new_hot.clone();
        *self.yaml_startup.write().unwrap() = startup_yaml_map(cfg);

        // 重算生效配置并原子替换
        let merged = self.merge_effective(&self.yaml_hot()).await?;
        self.effective.store(Arc::new(merged));
        Ok(())
    }

    /// GET /api/settings：返回全量键（含生效值与来源；env 覆盖项 source=env 只读）。
    pub async fn view(&self) -> ApiResult<Vec<SettingItem>> {
        // 生效中的 hot 配置 → 全部 hot 键与生效值
        let effective = self.effective.load_full();
        let hot_flat = crate::config::hot_flat(&effective);

        // DB 行信息（source / secret / restart_required 取自存储；ui 优先于 file）
        let rows =
            sqlx::query("SELECT key, value, source, secret, restart_required FROM system_settings")
                .fetch_all(&self.pool)
                .await?;
        let mut db: HashMap<String, (serde_json::Value, String, bool, bool)> = HashMap::new();
        for row in rows {
            let key: String = row.get("key");
            let value: serde_json::Value = row.get("value");
            let source: String = row.get("source");
            let secret: bool = row.get("secret");
            let restart_required: bool = row.get("restart_required");
            db.insert(key, (value, source, secret, restart_required));
        }

        let mut items = Vec::new();

        // hot 键：生效值取 effective，来源取 DB 行（ui 优先）
        for (key, value) in hot_flat {
            let (_, source, secret, restart_required) =
                db.get(&key)
                    .cloned()
                    .unwrap_or((value.clone(), "file".into(), false, false));
            let source = if source == "ui" {
                "ui".to_string()
            } else {
                "file".to_string()
            };
            items.push(SettingItem {
                key,
                value,
                source,
                restart_required,
                secret,
            });
        }

        // 启动类键：env > UI > YAML，secret 项掩码
        for &key in crate::config::RESTART_REQUIRED_KEYS {
            let secret = key_is_secret(key);
            // database.url 特判：UI 覆盖永不生效（自举矛盾，见 is_writable_key），
            // 只读展示 env DATABASE_URL 优先、否则 YAML 值，来源如实标注。
            let (value, source) = if key == "database.url" {
                let env_val = std::env::var("DATABASE_URL").ok().filter(|s| !s.is_empty());
                match env_val {
                    Some(v) => (v, "env"),
                    None => (self.startup_yaml_value(key).unwrap_or_default(), "file"),
                }
            } else {
                let yaml_value = self.startup_yaml_value(key).unwrap_or_default();
                self.effective_startup(key, &yaml_value).await?
            };
            let non_empty = !value.is_empty();
            let mut value = serde_json::Value::String(value);
            if secret && non_empty {
                value = serde_json::Value::String("***".into());
            }
            items.push(SettingItem {
                key: key.into(),
                value,
                source: source.into(),
                restart_required: true,
                secret,
            });
        }

        // 排序输出，保证确定性
        items.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(items)
    }

    /// PUT /api/settings：校验键合法性与类型，持久化 source=ui，热生效。
    /// 掩码值（"***"）回传语义 = 保持原值。启动类键允许保存但标注需重启。
    pub async fn put(&self, updates: serde_json::Map<String, serde_json::Value>) -> ApiResult<()> {
        // hot 键空间（来自当前 YAML hot 快照）
        let hot_keys: std::collections::HashSet<String> = crate::config::hot_flat(&self.yaml_hot())
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        let base = self.yaml_hot();

        // 逐键校验，收集合法更新
        let mut validated: Vec<(String, serde_json::Value, bool, bool)> = Vec::new();
        for (key, value) in updates {
            // 掩码回传值：保持原值，跳过该键
            if is_mask_sentinel(&value) {
                continue;
            }
            let is_startup = key_is_restart(&key);
            if !is_writable_key(&key, &hot_keys) {
                return Err(crate::error::ApiError::bad_request(format!(
                    "配置键不存在或不可写: {key}"
                )));
            }
            if is_startup {
                // 启动类键：值为字符串（监听地址/URL/secret）
                if !value.is_string() {
                    return Err(crate::error::ApiError::bad_request(format!(
                        "启动类配置键 {key} 的值必须是字符串"
                    )));
                }
            } else {
                // hot 键：先用 apply_overrides 试算做类型校验
                crate::config::apply_overrides(&base, &[(key.clone(), value.clone())]).map_err(
                    |e| {
                        crate::error::ApiError::bad_request(format!("配置值类型不匹配: {key}: {e}"))
                    },
                )?;
            }
            let secret = key_is_secret(&key);
            let restart = is_startup;
            // 敏感键保存：要求落库前可加密（与上游 api_key 等语义一致）；
            // 若 NEXTAPI_SECRET_KEY 未配置则拒绝，避免 JWT secret 明文落库（review P4）。
            if secret && value.is_string() && !value.as_str().unwrap_or_default().is_empty() {
                if !self.crypto.is_available() {
                    return Err(crate::error::ApiError::bad_request(format!(
                        "保存 {key} 需要先配置环境变量 {}（敏感字段落库前强制加密）",
                        crate::crypto::ENV_SECRET_KEY
                    )));
                }
            }
            validated.push((key.clone(), value.clone(), secret, restart));
        }

        // 持久化 source='ui'（覆盖即更新；source 转为 ui；敏感键加密存储）
        for (key, value, secret, restart) in &validated {
            let stored = match value {
                serde_json::Value::String(s) => {
                    serde_json::Value::String(seal_secret_value(&self.crypto, key, s))
                }
                other => other.clone(),
            };
            sqlx::query(
                "INSERT INTO system_settings (key, value, source, secret, restart_required, updated_at) \
                 VALUES ($1, $2, 'ui', $3, $4, now()) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, source = 'ui', \
                 secret = EXCLUDED.secret, restart_required = EXCLUDED.restart_required, \
                 updated_at = now()",
            )
            .bind(key)
            .bind(stored)
            .bind(secret)
            .bind(restart)
            .execute(&self.pool)
            .await?;
        }

        // 全部写入成功后重算并热生效
        let merged = self.merge_effective(&self.yaml_hot()).await?;
        self.effective.store(Arc::new(merged));
        Ok(())
    }

    /// 启动类参数解析：env > UI > YAML。
    pub async fn effective_startup(
        &self,
        key: &str,
        yaml_value: &str,
    ) -> ApiResult<(String, &'static str)> {
        // env（按 STARTUP_ENV_MAP 查表；仅启动时读取）
        if let Some((_, env_name)) = crate::config::STARTUP_ENV_MAP
            .iter()
            .find(|(k, _)| *k == key)
        {
            if let Ok(v) = std::env::var(env_name) {
                if !v.is_empty() {
                    return Ok((v, "env"));
                }
            }
        }
        // system_settings 中 source='ui' 的行（敏感键为密文，读时解密；解密失败视为
        // 无效覆盖降级回退 YAML，避免 NEXTAPI_SECRET_KEY 轮换后启动/查询被卡死）
        let row = sqlx::query("SELECT value FROM system_settings WHERE key = $1 AND source = 'ui'")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        if let Some(row) = row {
            let v: serde_json::Value = row.get("value");
            if let Some(s) = v.as_str() {
                match open_secret_value(&self.crypto, key, s) {
                    Ok(plain) => return Ok((plain, "ui")),
                    Err(e) => {
                        tracing::warn!(%key, error = %e, "UI 覆盖的敏感配置解密失败，回退 YAML");
                    }
                }
            }
        }
        // 回退 YAML
        Ok((yaml_value.to_string(), "file"))
    }

    /// 读取当前 YAML hot 配置的快照。
    pub fn yaml_hot(&self) -> HotConfig {
        self.yaml_hot.read().unwrap().clone()
    }

    /// 读取当前 YAML 启动类键值（env > UI > YAML 中的 YAML 基准值）。
    fn startup_yaml_value(&self, key: &str) -> Option<String> {
        self.yaml_startup.read().unwrap().get(key).cloned()
    }

    /// 计算生效 hot 配置：以 YAML hot 为基准，叠加上全部属于 hot 键空间的 UI 覆盖。
    /// 单行失败记录 warning 并跳过该行，不影响其它覆盖。
    async fn merge_effective(&self, yaml: &HotConfig) -> ApiResult<HotConfig> {
        let hot_keys: std::collections::HashSet<String> = crate::config::hot_flat(yaml)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        let rows = sqlx::query("SELECT key, value FROM system_settings WHERE source = 'ui'")
            .fetch_all(&self.pool)
            .await?;
        let mut merged = yaml.clone();
        for row in rows {
            let key: String = row.get("key");
            let value: serde_json::Value = row.get("value");
            if !hot_keys.contains(&key) {
                continue;
            }
            match crate::config::apply_overrides(&merged, &[(key.clone(), value.clone())]) {
                Ok(next) => merged = next,
                Err(e) => {
                    tracing::warn!(%key, error = %e, "跳过无效的 UI 配置覆盖");
                }
            }
        }
        Ok(merged)
    }
}

/// 提取配置文件中某个启动类键对应的 YAML 值（字符串）。
fn startup_yaml_value(cfg: &ConfigFile, key: &str) -> Option<String> {
    match key {
        "server.listen" => Some(cfg.server.listen.clone()),
        "database.url" => Some(cfg.database.url.clone()),
        "server.admin_jwt_secret" => Some(cfg.server.admin_jwt_secret.clone()),
        _ => None,
    }
}

/// 生成启动类键值快照（用于 view 在 env/UI 覆盖后回退到 YAML 基准值）。
fn startup_yaml_map(cfg: &ConfigFile) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for key in crate::config::RESTART_REQUIRED_KEYS {
        if let Some(v) = startup_yaml_value(cfg, key) {
            m.insert((*key).to_string(), v);
        }
    }
    m
}

/// 判断键是否可写：属于 hot 键空间或启动类键集合。
fn is_writable_key(key: &str, hot_keys: &std::collections::HashSet<String>) -> bool {
    if key == "database.url" {
        // 自举矛盾：DB 连接串必须在设置引擎（落库）之前读取，
        // UI 保存值永远不会生效——禁止写入（review P1-#6）。
        return false;
    }
    hot_keys.contains(key) || crate::config::RESTART_REQUIRED_KEYS.contains(&key)
}

/// 掩码回传值（"***"）语义：保持原值不变，跳过该键。
fn is_mask_sentinel(value: &serde_json::Value) -> bool {
    matches!(value, serde_json::Value::String(s) if s == "***")
}

/// 键是否敏感（仅 server.admin_jwt_secret 标记 secret）。
fn key_is_secret(key: &str) -> bool {
    key == "server.admin_jwt_secret"
}

/// 键是否为需重启的启动类键。
fn key_is_restart(key: &str) -> bool {
    crate::config::RESTART_REQUIRED_KEYS.contains(&key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_key_check() {
        let flat = crate::config::hot_flat(&HotConfig::default());
        let hot_keys: std::collections::HashSet<String> =
            flat.into_iter().map(|(k, _)| k).collect();

        // 合法：hot 键与启动类键（database.url 除外——自举矛盾禁写）
        assert!(is_writable_key("gateway.display_currency", &hot_keys));
        assert!(is_writable_key("proxy.no_proxy", &hot_keys));
        assert!(is_writable_key("server.listen", &hot_keys));
        assert!(!is_writable_key("database.url", &hot_keys));
        assert!(is_writable_key("server.admin_jwt_secret", &hot_keys));

        // 非法：seed 类（admin.*）、业务实体字段、未知键
        assert!(!is_writable_key("admin.username", &hot_keys));
        assert!(!is_writable_key("upstreams", &hot_keys));
        assert!(!is_writable_key("foo.bar", &hot_keys));
        assert!(!is_writable_key("server.debug", &hot_keys));
    }

    #[test]
    fn mask_sentinel_detected() {
        assert!(is_mask_sentinel(&serde_json::json!("***")));
        assert!(!is_mask_sentinel(&serde_json::json!("real")));
        assert!(!is_mask_sentinel(&serde_json::json!(42)));
        assert!(!is_mask_sentinel(&serde_json::json!({ "a": "***" })));
        assert!(!is_mask_sentinel(&serde_json::json!(null)));
    }

    #[test]
    fn secret_and_restart_flags() {
        assert!(key_is_secret("server.admin_jwt_secret"));
        assert!(!key_is_secret("database.url"));
        assert!(!key_is_secret("server.listen"));

        assert!(key_is_restart("server.listen"));
        assert!(key_is_restart("database.url"));
        assert!(key_is_restart("server.admin_jwt_secret"));
        assert!(!key_is_restart("gateway.display_currency"));
    }

    #[test]
    fn secret_seal_open_roundtrip() {
        // review P4：JWT secret 落库前加密、读取解密；旧明文行兼容
        let c = Crypto::from_secret("test-key");
        let sealed = seal_secret_value(&c, "server.admin_jwt_secret", "jwt-abc");
        assert!(sealed.starts_with("enc1:"));
        assert!(!sealed.contains("jwt-abc"));
        assert_eq!(
            open_secret_value(&c, "server.admin_jwt_secret", &sealed).unwrap(),
            "jwt-abc"
        );
        // 非敏感键不加密
        assert_eq!(
            seal_secret_value(&c, "server.listen", "0.0.0.0:8080"),
            "0.0.0.0:8080"
        );
        // 旧版明文行兼容直读
        assert_eq!(
            open_secret_value(&c, "server.admin_jwt_secret", "legacy-plain").unwrap(),
            "legacy-plain"
        );
        // 空值不加密
        assert_eq!(seal_secret_value(&c, "server.admin_jwt_secret", ""), "");
        // 密钥不匹配 → 解密失败（调用方降级回退 YAML）
        let c2 = Crypto::from_secret("other-key");
        assert!(open_secret_value(&c2, "server.admin_jwt_secret", &sealed).is_err());
    }

    #[test]
    fn startup_yaml_value_extraction() {
        let cfg = ConfigFile {
            server: crate::config::ServerCfg {
                listen: "127.0.0.1:9000".into(),
                admin_jwt_secret: "s3cret".into(),
                debug: false,
            },
            database: crate::config::DatabaseCfg {
                url: "postgres://u:p@h/db".into(),
            },
            ..Default::default()
        };
        assert_eq!(
            startup_yaml_value(&cfg, "server.listen").unwrap(),
            "127.0.0.1:9000"
        );
        assert_eq!(
            startup_yaml_value(&cfg, "database.url").unwrap(),
            "postgres://u:p@h/db"
        );
        assert_eq!(
            startup_yaml_value(&cfg, "server.admin_jwt_secret").unwrap(),
            "s3cret"
        );
        assert_eq!(startup_yaml_value(&cfg, "nonexistent").is_none(), true);
    }
}
