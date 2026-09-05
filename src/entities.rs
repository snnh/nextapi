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
            Some(list) => list
                .iter()
                .any(|p| crate::routing::pattern_matches(p, model)),
        }
    }

    /// Key 是否当前可用（启用 + 未过期）。
    pub fn is_usable(&self) -> bool {
        self.enabled && self.expires_at.map(|e| e > Utc::now()).unwrap_or(true)
    }

    /// debug 模式是否生效（开启且未过期）。
    pub fn debug_active(&self) -> bool {
        self.debug_enabled
            && self
                .debug_expires_at
                .map(|e| e > Utc::now())
                .unwrap_or(false)
    }
}

/// 上游渠道。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct UpstreamRow {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub base_url: String,
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
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str())
                    .filter_map(|s| s.parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        if custom.is_empty() {
            vec![
                Protocol::OpenaiChat,
                Protocol::Anthropic,
                Protocol::Gemini,
                Protocol::OpenaiResponses,
            ]
        } else {
            custom
        }
    }

    /// 解析覆盖规则（未配置 = 完全原样转发）。
    pub fn overrides(&self) -> Option<Overrides> {
        self.extra
            .get("overrides")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
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
    /// 用户名/密码做百分号编码（review P4：含 `@ : # /` 或非 ASCII 的凭据此前会拼出
    /// 非法 URL，凭据静默失效甚至被截断）。
    pub fn proxy_url(&self) -> String {
        let scheme = if self.kind == "socks5" {
            "socks5"
        } else {
            "http"
        };
        let host = &self.host;
        let port = self.port;
        match (&self.username, &self.password_plain) {
            (Some(u), Some(p)) => {
                let (ue, pe) = (urlencode_credential(u), urlencode_credential(p));
                format!("{scheme}://{ue}:{pe}@{host}:{port}")
            }
            (Some(u), None) => format!("{scheme}://{}@{host}:{port}", urlencode_credential(u)),
            _ => format!("{scheme}://{host}:{port}"),
        }
    }
}

/// RFC 3986 用户信息（userinfo）百分号编码：保留子集与已编码串原样保留。
fn urlencode_credential(s: &str) -> String {
    // 与 url crate 的 userinfo 编码规则一致的简化实现：仅对非保留字符放行。
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if UNRESERVED.contains(&b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

// ===========================================================================
// M4 日志统计（contracts/m4-logging.md §8）：usage_logs / usage_hourly 行结构。
// 仅追加到文件末尾，不改动任何既有内容。
// ===========================================================================

/// usage_logs 明细行（与 migrations/0003_logging.sql 表结构全字段对齐）。
/// NUMERIC → `rust_decimal::Decimal`，JSONB → `serde_json::Value`；仅 M4-B 查询侧使用。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct UsageLogRow {
    pub id: i64,
    pub request_id: String,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub key_id: Option<Uuid>,
    pub model: String,
    pub upstream_id: Option<Uuid>,
    pub protocol_in: String,
    pub protocol_out: String,
    pub convert_mode: String,
    pub stream: bool,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub images: Option<i32>,
    pub image_size: Option<String>,
    pub video_seconds: Option<Decimal>,
    pub video_resolution: Option<String>,
    pub video_task_type: Option<String>,
    pub latency_ms: Option<i32>,
    pub status: i32,
    pub error: Option<String>,
    pub retry_count: i32,
    pub ttfb_ms: Option<i32>,
    pub degraded: bool,
    pub pricing_source: Option<String>,
    pub cost_cny: Option<Decimal>,
    pub cost_usd: Option<Decimal>,
    pub price_used: Option<serde_json::Value>,
    pub fx_snapshot: Option<serde_json::Value>,
    pub usage_raw: Option<serde_json::Value>,
    pub debug_payload: Option<serde_json::Value>,
}

/// price_rules 行（M5；PLAN §6。dimension_key 为 dimensions 归一化串，参与唯一键）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PriceRuleRow {
    pub id: Uuid,
    pub upstream_id: Uuid,
    pub model_id: String,
    pub unit: String,
    pub currency: String,
    pub base_price: Decimal,
    pub source: String,
    pub dimensions: Option<serde_json::Value>,
    pub dimension_key: String,
    pub segments: Option<serde_json::Value>,
    pub context_basis: String,
    pub effective_from: Option<chrono::DateTime<chrono::Utc>>,
    pub effective_to: Option<chrono::DateTime<chrono::Utc>>,
    pub priority: i32,
    pub sort_order: i32,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// fx_rates 行（manual/auto 分行独立存储，主键含 source）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FxRateRow {
    pub currency_from: String,
    pub currency_to: String,
    pub rate: Decimal,
    pub source: String,
    pub fetched_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// media_tasks 行（M6；PLAN §6。异步图片任务 + 计费幂等）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MediaTaskRow {
    pub id: Uuid,
    pub media_type: String,
    pub gateway_key_id: Uuid,
    pub model: String,
    pub upstream_id: Uuid,
    pub provider_task_id: Option<String>,
    pub status: String,
    pub resolution: Option<String>,
    pub duration_seconds: Option<Decimal>,
    pub task_type: Option<String>,
    pub image_count: Option<i32>,
    pub image_size: Option<String>,
    pub cost_cny: Option<Decimal>,
    pub cost_usd: Option<Decimal>,
    pub billing_key: Option<String>,
    pub error: Option<String>,
    pub raw: Option<serde_json::Value>,
    pub request_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}
