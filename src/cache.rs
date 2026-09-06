//! 业务实体内存快照（DB 为事实源 + RwLock<Arc<Snapshot>> + 写路径失效刷新，PLAN.md §5.1）。
//!
//! 请求路径只读快照（零 DB 查询）；管理 API 写成功后先提交 DB 再调 `reload` 刷新快照。
//! 极端竞态下最多读到一次旧值，可接受。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

use crate::crypto::Crypto;
use crate::entities::{ApiKeyRow, ModelAliasRow, ModelRouteRow, ProxyRow, UpstreamRow};

#[derive(Debug, Default)]
pub struct Snapshot {
    /// key_hash(sha256 hex) → ApiKeyRow
    pub api_keys: HashMap<String, ApiKeyRow>,
    pub upstreams: HashMap<Uuid, UpstreamRow>,
    /// name → id 索引
    pub upstream_ids: HashMap<String, Uuid>,
    pub routes: Vec<ModelRouteRow>,
    pub proxies: HashMap<Uuid, ProxyRow>,
    /// alias → ModelAliasRow（含禁用项；使用处自行判 enabled，M10.1）
    pub aliases: HashMap<String, ModelAliasRow>,
}

pub struct EntityCache {
    inner: RwLock<Arc<Snapshot>>,
}

impl EntityCache {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Arc::new(Snapshot::default())),
        }
    }

    /// 读取当前快照（请求路径用，零锁读）。
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.inner.read().unwrap().clone()
    }

    /// 从 DB 全量加载并原子替换快照（api_key/代理密码在此解密进内存）。
    pub async fn reload(&self, pool: &PgPool, crypto: &Crypto) -> anyhow::Result<()> {
        // 1. 全量读取（sqlx::query_as 运行时校验；与 sqlx::query! 宏区分，无编译期 DB）
        //    —— api_keys / model_routes 的行结构与 entities 完全一致，可直接反序列化；
        //    —— upstreams / proxy_configs 行中存在加密列，entities 中多了内存用的明文列，
        //       故使用局部 Db 行结构，加载时再解密密文补进内存字段。
        let api_keys: Vec<ApiKeyRow> = sqlx::query_as::<_, ApiKeyRow>(
            "SELECT id, name, key_hash, prefix, enabled, models, rpm, tpm, quota_limit, \
             quota_unit, quota_window, allow_upstream_passthrough, passthrough_upstreams, \
             debug_enabled, debug_expires_at, expires_at, created_at, last_used_at \
             FROM api_keys",
        )
        .fetch_all(pool)
        .await?;

        let upstream_rows: Vec<UpstreamDbRow> = sqlx::query_as::<_, UpstreamDbRow>(
            "SELECT id, name, kind, base_url, api_key_enc, protocols, enabled, timeout_ms, \
             breaker_threshold, probe_model, consecutive_failures, disabled_by, cooldown_until, \
             use_proxy, proxy_id, extra, created_at, updated_at FROM upstreams",
        )
        .fetch_all(pool)
        .await?;

        let routes: Vec<ModelRouteRow> = sqlx::query_as::<_, ModelRouteRow>(
            "SELECT id, model_pattern, upstream_id, override_model, priority, weight, enabled, \
             retries, retry_status_codes, lock_upstream, sort_order, created_at, updated_at \
             FROM model_routes",
        )
        .fetch_all(pool)
        .await?;

        let aliases: Vec<ModelAliasRow> = sqlx::query_as::<_, ModelAliasRow>(
            "SELECT id, alias, model, enabled, created_at, updated_at FROM model_aliases",
        )
        .fetch_all(pool)
        .await?;

        let proxy_rows: Vec<ProxyDbRow> = sqlx::query_as::<_, ProxyDbRow>(
            "SELECT id, name, kind, host, port, username, password_enc, no_proxy, enabled \
             FROM proxy_configs",
        )
        .fetch_all(pool)
        .await?;

        // 2. 构建快照
        let mut snap = Snapshot::default();

        for k in api_keys {
            snap.api_keys.insert(k.key_hash.clone(), k);
        }

        for u in upstream_rows {
            // api_key 密文解密进内存（仅内存、不出 API）；解密失败记 warn 并置 None
            let api_key_plain = match u.api_key_enc.as_deref() {
                Some(enc) if !enc.is_empty() => match crypto.decrypt(enc) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        tracing::warn!("上游 `{}` 的 api_key 解密失败: {e}", u.name);
                        None
                    }
                },
                _ => None,
            };
            let row = UpstreamRow {
                id: u.id,
                name: u.name,
                kind: u.kind,
                base_url: u.base_url,
                api_key_plain,
                protocols: u.protocols,
                enabled: u.enabled,
                timeout_ms: u.timeout_ms,
                breaker_threshold: u.breaker_threshold,
                probe_model: u.probe_model,
                consecutive_failures: u.consecutive_failures,
                disabled_by: u.disabled_by,
                cooldown_until: u.cooldown_until,
                use_proxy: u.use_proxy,
                proxy_id: u.proxy_id,
                extra: u.extra,
                created_at: u.created_at,
                updated_at: u.updated_at,
            };
            snap.upstream_ids.insert(row.name.clone(), row.id);
            snap.upstreams.insert(row.id, row);
        }

        snap.routes = routes;

        for a in aliases {
            snap.aliases.insert(a.alias.clone(), a);
        }

        for p in proxy_rows {
            // 代理密码密文解密进内存（仅内存、不出 API）；解密失败记 warn 并置 None
            let password_plain = match p.password_enc.as_deref() {
                Some(enc) if !enc.is_empty() => match crypto.decrypt(enc) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        tracing::warn!("代理 `{}` 的密码解密失败: {e}", p.name);
                        None
                    }
                },
                _ => None,
            };
            let row = ProxyRow {
                id: p.id,
                name: p.name,
                kind: p.kind,
                host: p.host,
                port: p.port,
                username: p.username,
                password_enc: p.password_enc,
                password_plain,
                no_proxy: p.no_proxy,
                enabled: p.enabled,
            };
            snap.proxies.insert(row.id, row);
        }

        // 3. 原子替换（写锁），替换为全新 Arc 快照
        *self.inner.write().unwrap() = Arc::new(snap);
        Ok(())
    }
}

impl Default for EntityCache {
    fn default() -> Self {
        Self::new()
    }
}

/// upstreams 表行（加密列保持密文；内存明文列由加载时解密填充）。
#[derive(sqlx::FromRow)]
struct UpstreamDbRow {
    id: Uuid,
    name: String,
    kind: String,
    base_url: String,
    api_key_enc: Option<String>,
    protocols: Vec<String>,
    enabled: bool,
    timeout_ms: i32,
    breaker_threshold: i32,
    probe_model: Option<String>,
    consecutive_failures: i32,
    disabled_by: Option<String>,
    cooldown_until: Option<DateTime<Utc>>,
    use_proxy: bool,
    proxy_id: Option<Uuid>,
    extra: serde_json::Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// proxy_configs 表行（加密密码保持密文；内存明文列由加载时解密填充）。
#[derive(sqlx::FromRow)]
struct ProxyDbRow {
    id: Uuid,
    name: String,
    kind: String,
    host: String,
    port: i32,
    username: Option<String>,
    password_enc: Option<String>,
    no_proxy: Vec<String>,
    enabled: bool,
}
