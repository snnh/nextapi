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
    /// Codex 渠道的 OAuth 凭证（仅 kind='codex'；同 api_key_plain 的内存语义）
    #[serde(skip)]
    pub oauth_plain: Option<crate::upstream::codex::CodexOAuth>,
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
    /// 模型同步策略：manual=仅手动 / auto=跟随上游自动更新路由
    pub model_sync: String,
    /// 自动同步排除名单（命中即不生成/移除托管路由）
    pub model_exclude: Vec<String>,
    /// 最近一次成功拉取的模型列表缓存（JSONB 数组，元素为字符串）
    pub models_cache: serde_json::Value,
    pub models_fetched_at: Option<DateTime<Utc>>,
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
    /// 按协议取 base_url：extra.protocol_base_urls 覆盖优先（DeepSeek 等多根供应商），
    /// 否则回落 base_url。
    pub fn base_url_for(&self, p: crate::protocol::ir::Protocol) -> String {
        if let Some(v) = self
            .extra
            .get("protocol_base_urls")
            .and_then(|m| m.get(p.as_str()))
            .and_then(|u| u.as_str())
        {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
        self.base_url.clone()
    }

    /// 模型列表路径：extra.models_path 覆盖（如千帆 Token Plan 为 "/v1/models"），默认 "/models"。
    pub fn models_path(&self) -> &str {
        self.extra
            .get("models_path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("/models")
    }

    /// 模型列表 URL：{base_url}{models_path}（缺前导斜杠自动补；连通性探测与模型拉取共用）。
    pub fn models_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = self.models_path();
        if path.starts_with('/') {
            format!("{base}{path}")
        } else {
            format!("{base}/{path}")
        }
    }

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

    /// 能力矩阵（M10.3）：extra.capabilities 子对象，未配置 = 假设支持（向后兼容）。
    pub fn capabilities(&self) -> Capabilities {
        let c = &self.extra["capabilities"];
        Capabilities {
            stream: c["stream"].as_bool(),
            tools: c["tools"].as_bool(),
            vision: c["vision"].as_bool(),
            max_context: c["max_context"].as_u64(),
        }
    }
}

/// 上游能力矩阵（M10.3）。三态：None = 未配置（假设支持）；Some(false) = 明确不支持
/// → 路由候选过滤；Some(true) = 显式支持（与 None 行为相同，仅作管理标记）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub stream: Option<bool>,
    pub tools: Option<bool>,
    pub vision: Option<bool>,
    /// 最大上下文 tokens（仅记录/展示；v1 不参与过滤——请求侧无廉价 token 计量）
    pub max_context: Option<u64>,
}

/// 请求所需能力（M10.3），由 gateway 从请求体检测。
#[derive(Debug, Clone, Copy, Default)]
pub struct RequiredCaps {
    pub stream: bool,
    pub tools: bool,
    pub vision: bool,
}

impl Capabilities {
    /// 返回缺失能力名列表（空 = 满足），供路由过滤与错误提示。
    pub fn missing(&self, req: RequiredCaps) -> Vec<&'static str> {
        let mut m = Vec::new();
        if req.stream && self.stream == Some(false) {
            m.push("stream");
        }
        if req.tools && self.tools == Some(false) {
            m.push("tools");
        }
        if req.vision && self.vision == Some(false) {
            m.push("vision");
        }
        m
    }
}

/// 模型别名（model_aliases 表，M10.1）：alias → model 单跳映射。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ModelAliasRow {
    pub id: Uuid,
    pub alias: String,
    /// 实际模型 ID（路由匹配/白名单/计价均以此为准）
    pub model: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
    /// NULL=手动路由；'auto'=渠道模型同步托管（同步引擎只增删托管路由）
    pub managed_by: Option<String>,
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
        let scheme = match self.kind.as_str() {
            "socks5" => "socks5",
            "https" => "https",
            _ => "http",
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
    /// 客户端原始入口模型（M10.1；经别名解析时与 model 不同，未走别名为 NULL）
    pub requested_model: Option<String>,
    /// 上游实际使用的模型名（override_model 生效且与 model 不同时记录；NULL=与 model 相同。计价修复）
    pub upstream_model: Option<String>,
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
    /// 白名单请求头摘要（M14.3；仅 user-agent 等调试头，绝不含敏感头；旧行为 NULL）
    pub request_headers: Option<serde_json::Value>,
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
    /// 上游实际模型名（override_model 生效时；NULL=与 model 相同。计价修复）
    pub upstream_model: Option<String>,
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

#[cfg(test)]
mod cap_tests {
    use super::*;

    #[test]
    fn capabilities_missing_rules() {
        // 未配置 = 假设支持（任何需求都不缺失）
        let none = Capabilities::default();
        assert!(none
            .missing(RequiredCaps {
                stream: true,
                tools: true,
                vision: true
            })
            .is_empty());
        // 显式支持同上
        let on = Capabilities {
            stream: Some(true),
            tools: Some(true),
            vision: Some(true),
            max_context: None,
        };
        assert!(on
            .missing(RequiredCaps {
                stream: true,
                tools: true,
                vision: true
            })
            .is_empty());
        // 显式不支持 → 按需缺失
        let off = Capabilities {
            stream: Some(false),
            tools: Some(false),
            vision: Some(false),
            max_context: None,
        };
        assert_eq!(
            off.missing(RequiredCaps {
                stream: true,
                tools: false,
                vision: true
            }),
            vec!["stream", "vision"]
        );
        // 无需求不缺失
        assert!(off.missing(RequiredCaps::default()).is_empty());
        // max_context 不参与过滤（v1）
        assert!(
            off.missing(RequiredCaps {
                tools: true,
                ..Default::default()
            }) == vec!["tools"]
        );
    }
}

#[cfg(test)]
mod base_url_tests {
    use super::*;

    fn up(extra: serde_json::Value) -> UpstreamRow {
        UpstreamRow {
            id: uuid::Uuid::new_v4(),
            name: "up".into(),
            kind: "deepseek".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            api_key_plain: None,
            oauth_plain: None,
            protocols: vec!["openai_chat".into()],
            enabled: true,
            timeout_ms: 300_000,
            breaker_threshold: 5,
            probe_model: None,
            consecutive_failures: 0,
            disabled_by: None,
            cooldown_until: None,
            use_proxy: false,
            proxy_id: None,
            extra,
            model_sync: "manual".into(),
            model_exclude: vec![],
            models_cache: serde_json::json!([]),
            models_fetched_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn base_url_for_override_and_fallback() {
        let u = up(serde_json::json!({
            "protocol_base_urls": {"anthropic": "https://api.deepseek.com/anthropic/v1"}
        }));
        assert_eq!(
            u.base_url_for(crate::protocol::ir::Protocol::Anthropic),
            "https://api.deepseek.com/anthropic/v1"
        );
        assert_eq!(
            u.base_url_for(crate::protocol::ir::Protocol::OpenaiChat),
            "https://api.deepseek.com/v1"
        );
        let u2 = up(serde_json::json!({}));
        assert_eq!(
            u2.base_url_for(crate::protocol::ir::Protocol::Anthropic),
            "https://api.deepseek.com/v1"
        );
        let u3 = up(serde_json::json!({"protocol_base_urls": {"anthropic": "  "}}));
        assert_eq!(
            u3.base_url_for(crate::protocol::ir::Protocol::Anthropic),
            "https://api.deepseek.com/v1"
        );
    }

    #[test]
    fn models_url_override_and_fallback() {
        // 默认 /models
        let u = up(serde_json::json!({}));
        assert_eq!(u.models_path(), "/models");
        assert_eq!(u.models_url(), "https://api.deepseek.com/v1/models");

        // 覆盖（千帆 Token Plan 场景）
        let u2 = up(serde_json::json!({"models_path": "/v1/models"}));
        assert_eq!(u2.models_url(), "https://api.deepseek.com/v1/v1/models");

        // 无前导斜杠 → 自动补
        let u3 = up(serde_json::json!({"models_path": "v1/models"}));
        assert_eq!(u3.models_url(), "https://api.deepseek.com/v1/v1/models");

        // 空白 / 非字符串 → 回落默认
        let u4 = up(serde_json::json!({"models_path": "  "}));
        assert_eq!(u4.models_url(), "https://api.deepseek.com/v1/models");
        let u5 = up(serde_json::json!({"models_path": 7}));
        assert_eq!(u5.models_url(), "https://api.deepseek.com/v1/models");

        // base_url 末尾斜杠不产生双斜杠
        let mut u6 = up(serde_json::json!({}));
        u6.base_url = "https://up.example.com/base/".into();
        assert_eq!(u6.models_url(), "https://up.example.com/base/models");
    }
}
