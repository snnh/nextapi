//! 配置中心：YAML 文件加载 + hot/seed/derived 分类 + 热加载。
//!
//! 约定（PLAN.md §5.1）：
//! - `hot`（热加载项）：gateway / fx_auto_fetch / price_import / media_download /
//!   proxy / update_check / media_poller —— 文件变化即生效；
//! - `seed`（种子项）：admin / upstreams / model_routes / price_rules / fx_rates /
//!   proxies —— 仅首次初始化写入 DB；
//! - 启动类参数：server.listen / database.url / server.admin_jwt_secret，
//!   优先级 env > UI > YAML（见 settings.rs）；
//! - `NEXTAPI_SECRET_KEY` 为唯一 env-only 例外，不落 DB/YAML。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 需重启才生效的启动类配置键（UI 标注用）。
pub const RESTART_REQUIRED_KEYS: &[&str] =
    &["server.listen", "database.url", "server.admin_jwt_secret"];

/// 启动类配置键 → 环境变量映射（env 仅启动时读取，UI 只读展示）。
pub const STARTUP_ENV_MAP: &[(&str, &str)] = &[
    ("server.listen", "NEXTAPI_LISTEN"),
    ("database.url", "DATABASE_URL"),
    ("server.admin_jwt_secret", "NEXTAPI_ADMIN_JWT_SECRET"),
];

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("配置文件读取失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("配置文件解析失败: {0}")]
    Parse(#[from] serde_yaml::Error),
    #[error("配置键不存在或不可写: {0}")]
    UnknownKey(String),
    #[error("配置值类型不匹配: {key} = {value}")]
    BadValue { key: String, value: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    pub server: ServerCfg,
    pub database: DatabaseCfg,
    pub gateway: GatewayCfg,
    pub admin: AdminCfg,
    /// seed 类：上游（M3 起结构化，M1 透传保留）
    pub upstreams: Vec<serde_yaml::Value>,
    /// seed 类：模型路由
    pub model_routes: Vec<serde_yaml::Value>,
    /// seed 类：价格规则
    pub price_rules: Vec<serde_yaml::Value>,
    /// seed 类：手动汇率
    pub fx_rates: Vec<FxRateSeed>,
    pub fx_auto_fetch: FxAutoFetchCfg,
    pub price_import: PriceImportCfg,
    pub media_download: MediaDownloadCfg,
    pub proxy: ProxyCfg,
    /// seed 类：初始代理配置
    pub proxies: Vec<ProxySeed>,
    pub update_check: UpdateCheckCfg,
    pub media_poller: MediaPollerCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerCfg {
    pub listen: String,
    /// 生产必填；为空且非 debug 模式则拒绝启动
    pub admin_jwt_secret: String,
    /// 调试模式：放宽启动检查。仅本地开发使用。
    pub debug: bool,
    /// 可信反向代理 CIDR；为空时不信任 X-Forwarded-For/X-Real-IP。
    pub trusted_proxies: Vec<String>,
}

impl Default for ServerCfg {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8080".into(),
            admin_jwt_secret: String::new(),
            debug: false,
            trusted_proxies: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseCfg {
    pub url: String,
}

/// hot 类：网关运行参数（全部 UI 可配、热生效）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayCfg {
    pub default_rate_limit_rpm: u32,
    pub default_timeout_secs: u64,
    pub stream_timeout_secs: u64,
    pub billing_timezone: String,
    /// 混合统计默认展示币种（CNY|USD）
    pub display_currency: String,
    /// 金额展示舍入小数位（DB 仍 NUMERIC(20,10)）
    pub display_precision: u32,
    /// 最近成功汇率超过该时长视为过期不可用
    pub fx_stale_max_minutes: u64,
    pub log_async: bool,
    /// 有界队列容量；溢出先写 WAL 再丢弃内存条目
    pub log_queue_capacity: usize,
    pub log_wal_dir: String,
    pub log_wal_file_max_mb: u64,
    /// 尾部 N MB 全部写入并重放完成即归档
    pub log_wal_archive_tail_mb: u64,
    /// usage_logs 每分区覆盖天数
    pub log_partition_days: u32,
    /// 明细不自动清理；仅作手动清理默认 before（0=永久）
    pub log_detail_retention_days: u32,
    /// 汇总表保留建议值，不自动删除（0=永久）
    pub log_aggregate_retention_days: u32,
    /// 按 Key debug 模式最长开启时长（分钟），到时自动关闭
    pub log_debug_ttl_minutes: u64,
    /// 登录接口防爆破限速（次/分钟）
    pub admin_login_rate_limit_per_min: u32,
    /// 转 Anthropic 缺省 max_tokens
    pub anthropic_default_max_tokens: u64,
    pub batch_insert_interval_ms: u64,
    /// 用量上限聚合结果缓存秒数
    pub quota_check_cache_secs: u64,
    /// block=429 硬阻断 | warn=仅日志告警
    pub quota_exceed_action: String,
}

impl Default for GatewayCfg {
    fn default() -> Self {
        Self {
            default_rate_limit_rpm: 600,
            default_timeout_secs: 300,
            stream_timeout_secs: 600,
            billing_timezone: "Asia/Shanghai".into(),
            display_currency: "CNY".into(),
            display_precision: 6,
            fx_stale_max_minutes: 1440,
            log_async: true,
            log_queue_capacity: 10_000,
            log_wal_dir: "/data/wal".into(),
            log_wal_file_max_mb: 128,
            log_wal_archive_tail_mb: 64,
            log_partition_days: 30,
            log_detail_retention_days: 0,
            log_aggregate_retention_days: 365,
            log_debug_ttl_minutes: 60,
            admin_login_rate_limit_per_min: 10,
            anthropic_default_max_tokens: 65536,
            batch_insert_interval_ms: 500,
            quota_check_cache_secs: 3,
            quota_exceed_action: "block".into(),
        }
    }
}

/// seed 类：管理员账号（仅首次初始化写入 DB）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdminCfg {
    pub username: String,
    /// 首次启动若为空则生成随机密码并打印一次到日志
    pub initial_password: String,
    /// 已存在则忽略 initial_password（argon2 哈希）
    pub password_hash: String,
}

impl Default for AdminCfg {
    fn default() -> Self {
        Self {
            username: "admin".into(),
            initial_password: String::new(),
            password_hash: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FxRateSeed {
    pub from: String,
    pub to: String,
    pub rate: rust_decimal::Decimal,
    #[serde(default = "default_manual")]
    pub source: String,
}

fn default_manual() -> String {
    "manual".into()
}

/// hot 类：联网拉取汇率。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FxAutoFetchCfg {
    pub enabled: bool,
    /// frankfurter | ecb | custom
    pub provider: String,
    pub base: String,
    pub symbols: Vec<String>,
    pub refresh_interval_minutes: u64,
    pub cache_ttl_minutes: u64,
    pub timeout_secs: u64,
    pub use_proxy: bool,
    /// 空 + use_proxy=true → 跟随 proxy.default_proxy_id
    pub proxy_id: String,
}

impl Default for FxAutoFetchCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: "frankfurter".into(),
            base: "USD".into(),
            symbols: vec!["CNY".into()],
            refresh_interval_minutes: 60,
            cache_ttl_minutes: 60,
            timeout_secs: 10,
            use_proxy: false,
            proxy_id: String::new(),
        }
    }
}

/// hot 类：价格表 URL 导入。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PriceImportCfg {
    pub allow_url: bool,
    pub max_size_mb: u64,
    pub timeout_secs: u64,
    pub use_proxy: bool,
    pub proxy_id: String,
}

impl Default for PriceImportCfg {
    fn default() -> Self {
        Self {
            allow_url: true,
            max_size_mb: 1,
            timeout_secs: 10,
            use_proxy: false,
            proxy_id: String::new(),
        }
    }
}

/// hot 类：媒体 URL 下载转内联（默认关闭，SSRF 防护）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaDownloadCfg {
    pub enabled: bool,
    pub max_size_mb: u64,
    pub timeout_secs: u64,
    pub use_proxy: bool,
    pub proxy_id: String,
}

impl Default for MediaDownloadCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            max_size_mb: 10,
            timeout_secs: 30,
            use_proxy: false,
            proxy_id: String::new(),
        }
    }
}

/// hot 类：系统默认代理与全局直连名单。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyCfg {
    /// 空 = 直连；各外联场景 use_proxy=true 且未指定 proxy_id 时跟随此值
    pub default_proxy_id: String,
    /// 全局直连名单（与所选代理自身 no_proxy 取并集；CIDR/域名，支持 * 后缀通配）
    pub no_proxy: Vec<String>,
    /// 代理连通性测试固定探测地址（白名单，防 SSRF）
    pub probe_url: String,
}

impl Default for ProxyCfg {
    fn default() -> Self {
        Self {
            default_proxy_id: String::new(),
            no_proxy: Vec::new(),
            probe_url: "https://api.github.com".into(),
        }
    }
}

/// seed 类：初始代理配置（密码导入即加密，导出仅掩码）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxySeed {
    pub name: String,
    /// http | https | socks5
    pub kind: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub no_proxy: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ProxySeed {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: "http".into(),
            host: String::new(),
            port: 8080,
            username: None,
            password: None,
            no_proxy: Vec::new(),
            enabled: true,
        }
    }
}

/// hot 类：GitHub 更新检查。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateCheckCfg {
    pub enabled: bool,
    /// 如 owner/nextapi
    pub repo: String,
    /// 0 = 关闭定时，仅手动检查
    pub interval_hours: u64,
    pub use_proxy: bool,
    pub proxy_id: String,
}

impl Default for UpdateCheckCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            repo: String::new(),
            interval_hours: 24,
            use_proxy: false,
            proxy_id: String::new(),
        }
    }
}

/// hot 类：图片/视频异步任务轮询。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaPollerCfg {
    pub interval_secs: u64,
    pub max_age_hours: u64,
}

impl Default for MediaPollerCfg {
    fn default() -> Self {
        Self {
            interval_secs: 5,
            max_age_hours: 24,
        }
    }
}

/// hot 类运行参数全集（热加载原子替换的单位）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HotConfig {
    pub gateway: GatewayCfg,
    pub fx_auto_fetch: FxAutoFetchCfg,
    pub price_import: PriceImportCfg,
    pub media_download: MediaDownloadCfg,
    pub proxy: ProxyCfg,
    pub update_check: UpdateCheckCfg,
    pub media_poller: MediaPollerCfg,
}

impl ConfigFile {
    /// 从完整配置中提取 hot 部分。
    pub fn hot(&self) -> HotConfig {
        HotConfig {
            gateway: self.gateway.clone(),
            fx_auto_fetch: self.fx_auto_fetch.clone(),
            price_import: self.price_import.clone(),
            media_download: self.media_download.clone(),
            proxy: self.proxy.clone(),
            update_check: self.update_check.clone(),
            media_poller: self.media_poller.clone(),
        }
    }
}

/// 加载 YAML 配置文件；文件不存在时返回默认值并记 warning（允许纯 env/DB 启动）。
pub fn load_file(path: &str) -> Result<ConfigFile, ConfigError> {
    match std::fs::read_to_string(Path::new(path)) {
        Ok(content) => Ok(serde_yaml::from_str(&content)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(path, "配置文件不存在，使用内置默认值");
            Ok(ConfigFile::default())
        }
        Err(e) => Err(e.into()),
    }
}

/// 将 JSON 值递归扁平化为点分键列表（对象展开，数组/标量为叶子）。
fn flatten_json(
    value: &serde_json::Value,
    prefix: &str,
    out: &mut Vec<(String, serde_json::Value)>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_json(v, &key, out);
            }
        }
        _ => out.push((prefix.to_string(), value.clone())),
    }
}

/// 将 HotConfig 扁平化为点分键值列表，如 ("gateway.display_currency", "CNY")。
/// 用于 system_settings 的键空间（UI 全配置）。输出按键名排序，保证确定性。
pub fn hot_flat(hot: &HotConfig) -> Vec<(String, serde_json::Value)> {
    let value = serde_json::to_value(hot).expect("HotConfig 序列化失败");
    let mut out = Vec::new();
    flatten_json(&value, "", &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// 沿点分路径写入新值；路径段为空、中间键缺失或中间值非对象 → UnknownKey。
fn set_json_path(
    value: &mut serde_json::Value,
    path: &str,
    new_value: &serde_json::Value,
) -> Result<(), ConfigError> {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.iter().any(|p| p.is_empty()) {
        return Err(ConfigError::UnknownKey(path.to_string()));
    }
    let mut cur = value;
    for (i, part) in parts.iter().enumerate() {
        let obj = cur
            .as_object_mut()
            .ok_or_else(|| ConfigError::UnknownKey(path.to_string()))?;
        if i == parts.len() - 1 {
            if !obj.contains_key(*part) {
                return Err(ConfigError::UnknownKey(path.to_string()));
            }
            obj.insert(part.to_string(), new_value.clone());
            return Ok(());
        }
        cur = obj
            .get_mut(*part)
            .ok_or_else(|| ConfigError::UnknownKey(path.to_string()))?;
    }
    unreachable!()
}

/// 在 base 上应用点分键覆盖，返回新的 HotConfig。
/// 键不存在或值类型不匹配时返回 ConfigError。
pub fn apply_overrides(
    base: &HotConfig,
    overrides: &[(String, serde_json::Value)],
) -> Result<HotConfig, ConfigError> {
    let mut value = serde_json::to_value(base).expect("HotConfig 序列化失败");
    for (key, val) in overrides {
        set_json_path(&mut value, key, val)?;
        // 每写一个键立即反序列化校验，精确定位类型不匹配的键
        serde_json::from_value::<HotConfig>(value.clone()).map_err(|_| ConfigError::BadValue {
            key: key.clone(),
            value: val.to_string(),
        })?;
    }
    serde_json::from_value(value).map_err(|_| ConfigError::BadValue {
        key: "<整体>".into(),
        value: String::new(),
    })
}

/// 仅当值为非空字符串时替换为 "***"；空串、null、非字符串保持原样。
fn mask_string_field(v: &mut serde_json::Value) {
    if let serde_json::Value::String(s) = v {
        if !s.is_empty() {
            *v = serde_json::Value::String("***".into());
        }
    }
}

/// 导出配置时对敏感字段掩码（上游 api_key、admin_jwt_secret、代理密码等）。
/// 输入为 ConfigFile 序列化后的 JSON，原地修改。
pub fn mask_config_json(v: &mut serde_json::Value) {
    if let Some(server) = v.get_mut("server").and_then(|s| s.as_object_mut()) {
        if let Some(sec) = server.get_mut("admin_jwt_secret") {
            mask_string_field(sec);
        }
    }
    if let Some(admin) = v.get_mut("admin").and_then(|a| a.as_object_mut()) {
        for k in ["password_hash", "initial_password"] {
            if let Some(p) = admin.get_mut(k) {
                mask_string_field(p);
            }
        }
    }
    if let Some(ups) = v.get_mut("upstreams").and_then(|u| u.as_array_mut()) {
        for item in ups.iter_mut() {
            if let Some(o) = item.as_object_mut() {
                if let Some(api) = o.get_mut("api_key") {
                    mask_string_field(api);
                }
            }
        }
    }
    if let Some(proxies) = v.get_mut("proxies").and_then(|p| p.as_array_mut()) {
        for item in proxies.iter_mut() {
            if let Some(o) = item.as_object_mut() {
                if let Some(pw) = o.get_mut("password") {
                    mask_string_field(pw);
                }
            }
        }
    }
}

/// 监听配置文件变化（debounce 后通过 tx 通知）。返回的 watcher 必须保持存活。
pub fn watch(
    path: &str,
    tx: tokio::sync::mpsc::UnboundedSender<()>,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::{RecommendedWatcher, Watcher};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    // 监听父目录而非文件本身，避免编辑器原子重命名导致监听丢失
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    let dir = target
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let (event_tx, event_rx) = mpsc::channel::<Result<notify::Event, notify::Error>>();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            let _ = event_tx.send(res);
        },
        notify::Config::default(),
    )?;
    watcher.watch(&dir, notify::RecursiveMode::NonRecursive)?;

    // 后台线程做 500ms 尾沿 debounce：仅当事件涉及目标文件时通知一次
    std::thread::spawn(move || {
        const DEBOUNCE: Duration = Duration::from_millis(500);
        let is_target =
            |p: &PathBuf| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == target;
        let mut pending = false;
        let mut last: Option<Instant> = None;
        loop {
            let wait = match last {
                Some(t) => DEBOUNCE.saturating_sub(t.elapsed()),
                None => DEBOUNCE,
            };
            match event_rx.recv_timeout(wait) {
                Ok(Ok(event)) => {
                    if event.paths.iter().any(is_target) {
                        pending = true;
                        last = Some(Instant::now());
                    }
                }
                Ok(Err(_)) => {} // 忽略单个事件错误
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if pending && last.map(|t| t.elapsed() >= DEBOUNCE).unwrap_or(false) {
                        let _ = tx.send(());
                        pending = false;
                        last = None;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if pending {
                        let _ = tx.send(());
                    }
                    break;
                }
            }
        }
    });

    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_flat_keys_and_values() {
        let mut hot = HotConfig::default();
        hot.gateway.display_currency = "USD".into();
        hot.proxy.no_proxy = vec!["a.com".into(), "b.com".into()];

        let flat = hot_flat(&hot);
        let map: std::collections::BTreeMap<_, _> = flat.clone().into_iter().collect();

        assert_eq!(
            map.get("gateway.display_currency").unwrap(),
            &serde_json::json!("USD")
        );
        assert_eq!(
            map.get("gateway.default_rate_limit_rpm").unwrap(),
            &serde_json::json!(600)
        );
        assert_eq!(
            map.get("proxy.no_proxy").unwrap(),
            &serde_json::json!(["a.com", "b.com"])
        );

        // 输出按键名确定性排序
        let keys: Vec<&String> = flat.iter().map(|(k, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn apply_overrides_normal_and_immutable_base() {
        let base = HotConfig::default();
        let out = apply_overrides(
            &base,
            &[
                ("gateway.display_currency".into(), serde_json::json!("EUR")),
                ("proxy.default_proxy_id".into(), serde_json::json!("p1")),
            ],
        )
        .unwrap();
        assert_eq!(out.gateway.display_currency, "EUR");
        assert_eq!(out.proxy.default_proxy_id, "p1");
        assert_eq!(base.gateway.display_currency, "CNY"); // 不修改 base
    }

    #[test]
    fn apply_overrides_unknown_key_errors() {
        let base = HotConfig::default();
        let res = apply_overrides(&base, &[("unknown.path".into(), serde_json::json!(1))]);
        assert!(matches!(res, Err(ConfigError::UnknownKey(ref k)) if k == "unknown.path"));
    }

    #[test]
    fn apply_overrides_bad_value_errors() {
        let base = HotConfig::default();
        let res = apply_overrides(
            &base,
            &[("gateway.display_currency".into(), serde_json::json!(123))],
        );
        match res {
            Err(ConfigError::BadValue { key, value }) => {
                assert_eq!(key, "gateway.display_currency");
                assert_eq!(value, "123");
            }
            other => panic!("期望 BadValue，得到 {other:?}"),
        }
    }

    #[test]
    fn mask_sensitive_fields() {
        let mut v = serde_json::json!({
            "server": { "listen": "0.0.0.0:8080", "admin_jwt_secret": "s3cret", "debug": false },
            "admin": { "username": "admin", "initial_password": "init123", "password_hash": "$argon2$..." },
            "upstreams": [{ "name": "u1", "api_key": "sk-aaa" }],
            "proxies": [
                { "name": "p1", "password": "pw1" },
                { "name": "p2", "password": "" }
            ]
        });
        mask_config_json(&mut v);

        assert_eq!(v["server"]["admin_jwt_secret"], "***");
        assert_eq!(v["admin"]["initial_password"], "***");
        assert_eq!(v["admin"]["password_hash"], "***");
        assert_eq!(v["upstreams"][0]["api_key"], "***");
        assert_eq!(v["proxies"][0]["password"], "***");
        assert_eq!(v["proxies"][1]["password"], ""); // 空字符串保持原样
        assert_eq!(v["server"]["listen"], "0.0.0.0:8080"); // 非敏感字段不受影响
    }

    #[tokio::test]
    async fn watch_notifies_on_change() {
        let dir = std::env::temp_dir().join(format!(
            "nextapi_cfg_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.yaml");
        std::fs::write(&file, "a: 1\n").unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let _watcher = watch(file.to_str().unwrap(), tx).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(200));
        std::fs::write(&file, "a: 2\n").unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await;
        assert!(got.is_ok(), "应在超时前收到配置变更通知");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
