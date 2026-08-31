//! 业务实体行结构（对应 migrations/0002_gateway.sql，PLAN.md §6）。

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// 网关 Key（完整 Key 仅创建时展示一次；DB 只存哈希 + 前缀）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ApiKeyRow {
    pub id: Uuid,
    pub name: String,
    #[serde(skip)]
    pub key_hash: String,
    pub prefix: String,
    pub enabled: bool,
    pub models: Option<Vec<String>>,
    pub rpm: Option<i32>,
    pub tpm: Option<i32>,
    pub quota_limit: Option<Decimal>,
    pub quota_unit: Option<String>,
    pub quota_window: Option<String>,
    pub allow_upstream_passthrough: bool,
    pub passthrough_upstreams: Option<Vec<String>>,
    pub debug_enabled: bool,
    pub debug_expires_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl ApiKeyRow {
    /// 模型白名单匹配（NULL/空 = 全部；支持 * 通配）。
    pub fn model_allowed(&self, model: &str) -> bool {
        match &self.models {
            None => true,
            Some(list) if list.is_empty() => true,
            Some(list) => list.iter().any(|p| crate::routing::pattern_matches(p, model)),
        }
    }

    /// Key 是否当前可用（启用 + 未过期）。
    pub fn is_usable(&self) -> bool {
        self.enabled && self.expires_at.map(|e| e > Utc::now()).unwrap_or(true)
    }

    /// debug 模式是否生效（开启且未过期）。
    pub fn debug_active(&self) -> bool {
        self.debug_enabled && self.debug_expires_at.map(|e| e > Utc::now()).unwrap_or(false)
    }
}

/// 上游渠道。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct UpstreamRow {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    #[serde(skip)]
    pub api_key_enc: Option<String>,
    /// 内存快照中持有的解密后 api_key（不落库、不出 API；由 cache 加载时解密填充）
    #[serde(skip)]
    pub api_key_plain: Option<String>,
    pub protocols: Vec<String>,
    pub enabled: bool,
    pub timeout_ms: i32,
    pub breaker_threshold: i32,
    pub probe_model: Option<String>,
    pub consecutive_failures: i32,
    pub disabled_by: Option<String>,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub use_proxy: bool,
    pub proxy_id: Option<Uuid>,
    pub extra: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 上游 extra 中的请求头/请求体覆盖规则（v1.12：配置了才生效，覆盖优先级最高）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Overrides {
    #[serde(default)]
    pub headers: OverrideMap,
    #[serde(default)]
    pub body: OverrideMap,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OverrideMap {
    #[serde(default)]
    pub add: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub set: serde_json::Map<String, serde_json::Value>,
}

impl UpstreamRow {
    /// 上游声明支持的协议（解析为 IR Protocol）。
    pub fn protocol_list(&self) -> Vec<crate::protocol::ir::Protocol> {
        self.protocols
            .iter()
            .filter_map(|p| p.parse().ok())
            .collect()
    }

    /// 目标协议优先级：extra.protocol_priority 覆盖默认 openai_chat > anthropic > gemini > responses。
    pub fn protocol_priority(&self) -> Vec<crate::protocol::ir::Protocol> {
        use crate::protocol::ir::Protocol;
        let custom: Vec<Protocol> = self
            .extra
            .get("protocol_priority")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str()).filter_map(|s| s.parse().ok()).collect())
            .unwrap_or_default();
        if custom.is_empty() {
            vec![Protocol::OpenaiChat, Protocol::Anthropic, Protocol::Gemini, Protocol::OpenaiResponses]
        } else {
            custom
        }
    }

    /// 解析覆盖规则（未配置 = 完全原样转发）。
    pub fn overrides(&self) -> Option<Overrides> {
        self.extra.get("overrides").and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}

/// 模型路由目标。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ModelRouteRow {
    pub id: Uuid,
    pub model_pattern: String,
    pub upstream_id: Uuid,
    pub override_model: Option<String>,
    pub priority: i32,
    pub weight: i32,
    pub enabled: bool,
    pub retries: i32,
    pub retry_status_codes: Vec<i32>,
    pub lock_upstream: bool,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 代理配置（proxy_configs 表；密码解密后仅存内存快照）。
#[derive(Debug, Clone, FromRow)]
pub struct ProxyRow {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub host: String,
    pub port: i32,
    pub username: Option<String>,
    #[allow(dead_code)]
    pub password_enc: Option<String>,
    pub password_plain: Option<String>,
    pub no_proxy: Vec<String>,
    pub enabled: bool,
}

impl ProxyRow {
    /// 生成代理 URL（http://user:pass@host:port / socks5://...）。
    pub fn proxy_url(&self) -> String {
        let scheme = if self.kind == "socks5" { "socks5" } else { "http" };
        match (&self.username, &self.password_plain) {
            (Some(u), Some(p)) => format!("{scheme}://{u}:{p}@{}:{}", self.host, self.port),
            (Some(u), None) => format!("{scheme}://{u}@{}:{}", self.host, self.port),
            _ => format!("{scheme}://{}:{}", self.host, self.port),
        }
    }
}
