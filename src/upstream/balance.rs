//! 上游余额/额度查询（通用适配器）。
//!
//! 设计要点：
//! - 配置放 `upstreams.extra.balance`（非敏感）；独立凭证放 `balance_key_enc`，
//!   缓存加载时解密到 `balance_key_plain`，留空回退推理 Key。
//! - 本模块只负责「配置解析 + 适配器选择 + 请求 + 归一化快照」，落库由 admin quota
//!   handler 统一调用 `quota::store`。
//! - 新快照统一 `kind:"balance"`；codex/devin 仍走既有专用结构。
//! - 余额查询失败不得影响渠道熔断/凭证失效状态。

use std::str::FromStr;
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::Url;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::entities::UpstreamRow;
use crate::state::AppState;

/// 单次余额请求的响应体上限（余额接口都是小 JSON）。
const MAX_BODY_BYTES: usize = 256 * 1024;
const REQUEST_TIMEOUT_SECS: u64 = 10;

/// `extra.balance` 配置。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BalanceConfig {
    /// auto / off / deepseek / kimi / openrouter / zenmux / zhipu / openai_billing / custom
    #[serde(default)]
    pub adapter: Option<String>,
    /// custom：相对路径或公网 HTTPS 绝对 URL
    #[serde(default)]
    pub url: Option<String>,
    /// custom：鉴权 Header 名（空 = 匿名）
    #[serde(default)]
    pub header: Option<String>,
    /// custom：Header 值前缀（默认 "Bearer "）
    #[serde(default)]
    pub prefix: Option<String>,
    /// custom：余额字段点路径（必填）
    #[serde(default)]
    pub amount_path: Option<String>,
    /// custom：币种字面量（与 currency_path 二选一）
    #[serde(default)]
    pub currency: Option<String>,
    /// custom：币种字段点路径
    #[serde(default)]
    pub currency_path: Option<String>,
    #[serde(default)]
    pub total_path: Option<String>,
    #[serde(default)]
    pub used_path: Option<String>,
    #[serde(default)]
    pub reset_at_path: Option<String>,
}

/// 已识别的余额适配器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalanceAdapter {
    DeepSeek,
    Kimi,
    OpenRouter,
    Zenmux,
    Zhipu,
    OpenaiBilling,
    Custom,
}

impl BalanceAdapter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeepSeek => "deepseek",
            Self::Kimi => "kimi",
            Self::OpenRouter => "openrouter",
            Self::Zenmux => "zenmux",
            Self::Zhipu => "zhipu",
            Self::OpenaiBilling => "openai_billing",
            Self::Custom => "custom",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "deepseek" => Some(Self::DeepSeek),
            "kimi" | "moonshot" => Some(Self::Kimi),
            "openrouter" => Some(Self::OpenRouter),
            "zenmux" => Some(Self::Zenmux),
            "zhipu" => Some(Self::Zhipu),
            "openai_billing" => Some(Self::OpenaiBilling),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// 渠道额度能力（列表入口/弹窗支持判断）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceSupport {
    pub supported: bool,
    /// codex / devin / 具体 balance adapter；不支持时为 None
    pub adapter: Option<&'static str>,
    /// 不支持原因（用于接口诊断；前端可不展示）
    pub reason: Option<String>,
}

/// 读取 `extra.balance`；缺失/非对象 = 默认配置。
pub fn config(up: &UpstreamRow) -> BalanceConfig {
    config_from(&up.extra)
}

fn config_from(extra: &serde_json::Value) -> BalanceConfig {
    extra
        .get("balance")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// 解析渠道支持的额度适配器。codex/devin 返回 None（由既有专用逻辑处理）。
pub fn resolve_adapter(up: &UpstreamRow) -> Option<BalanceAdapter> {
    resolve_adapter_from(&up.kind, &up.extra)
}

pub fn resolve_adapter_from(kind: &str, extra: &serde_json::Value) -> Option<BalanceAdapter> {
    let cfg = config_from(extra);
    match cfg.adapter.as_deref().map(str::trim) {
        Some("off") => None,
        Some("custom") => Some(BalanceAdapter::Custom),
        Some("openai_billing") => Some(BalanceAdapter::OpenaiBilling),
        Some(s) if !s.is_empty() && s != "auto" => BalanceAdapter::parse(s),
        _ => match kind {
            "deepseek" => Some(BalanceAdapter::DeepSeek),
            "kimi" | "moonshot" => Some(BalanceAdapter::Kimi),
            "openrouter" => Some(BalanceAdapter::OpenRouter),
            "zenmux" => Some(BalanceAdapter::Zenmux),
            "zhipu" => Some(BalanceAdapter::Zhipu),
            _ => None,
        },
    }
}

/// 渠道是否支持额度查询；codex/devin 继续由既有 `/quota` 逻辑支持。
pub fn support(up: &UpstreamRow) -> BalanceSupport {
    support_from(&up.kind, &up.extra)
}

pub fn support_from(kind: &str, extra: &serde_json::Value) -> BalanceSupport {
    if kind == "codex" || kind == "devin" {
        return BalanceSupport {
            supported: true,
            adapter: Some(if kind == "codex" { "codex" } else { "devin" }),
            reason: None,
        };
    }
    let cfg = config_from(extra);
    if cfg.adapter.as_deref().map(str::trim) == Some("off") {
        return BalanceSupport {
            supported: false,
            adapter: None,
            reason: Some("余额查询已关闭".into()),
        };
    }
    if let Some(err) = validate_config_from(extra) {
        return BalanceSupport {
            supported: false,
            adapter: None,
            reason: Some(err),
        };
    }
    match resolve_adapter_from(kind, extra) {
        Some(a) => BalanceSupport {
            supported: true,
            adapter: Some(a.as_str()),
            reason: None,
        },
        None => BalanceSupport {
            supported: false,
            adapter: None,
            reason: Some("未匹配到余额适配器".into()),
        },
    }
}

/// 校验 `extra.balance`；返回 Some(error) 表示配置非法。
pub fn validate_config(up: &UpstreamRow) -> Option<String> {
    validate_config_from(&up.extra)
}

pub fn validate_extra(extra: &serde_json::Value) -> Option<String> {
    validate_config_from(extra)
}

fn validate_config_from(extra: &serde_json::Value) -> Option<String> {
    let cfg = config_from(extra);
    let adapter = cfg.adapter.as_deref().map(str::trim).unwrap_or("auto");
    if adapter != "auto" && adapter != "off" && BalanceAdapter::parse(adapter).is_none() {
        return Some(format!("未知余额适配器 `{adapter}`"));
    }
    if adapter != "custom" {
        return None;
    }
    let url = cfg.url.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if url.is_none() {
        return Some("自定义余额端点缺少 URL".into());
    }
    if let Some(url) = url {
        if let Err(e) = validate_custom_url(url) {
            return Some(e);
        }
    }
    let amount = cfg
        .amount_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if amount.is_none() {
        return Some("自定义余额端点缺少 amount_path".into());
    }
    let has_currency = cfg
        .currency
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty())
        || cfg
            .currency_path
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty());
    if !has_currency {
        return Some("自定义余额端点需配置 currency 或 currency_path".into());
    }
    let header = cfg.header.as_deref().unwrap_or("Authorization").trim();
    if !header.is_empty() && HeaderName::from_bytes(header.as_bytes()).is_err() {
        return Some(format!("非法 Header 名 `{header}`"));
    }
    if cfg.prefix.as_deref().unwrap_or("").len() > 64 {
        return Some("Header 前缀过长".into());
    }
    None
}

fn validate_custom_url(raw: &str) -> Result<(), String> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        let url = Url::parse(raw).map_err(|e| format!("URL 非法: {e}"))?;
        if url.scheme() != "https" {
            return Err("自定义余额绝对 URL 仅允许 https".into());
        }
        if url.host_str().is_none() {
            return Err("URL 缺少主机名".into());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("URL 不允许携带用户名/密码".into());
        }
        return Ok(());
    }
    if raw.starts_with("//") {
        return Err("URL 不允许以 // 开头（会解析为跨域）".into());
    }
    Ok(())
}

/// 余额查询凭证：独立 Key 优先，留空回退推理 Key。
pub fn credential(up: &UpstreamRow) -> Option<&str> {
    up.balance_key_plain
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            up.api_key_plain
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
}

/// 探测余额并返回归一化快照；失败信息直接给管理员看（不含凭证）。
pub async fn probe(
    state: &AppState,
    up: &UpstreamRow,
    adapter: BalanceAdapter,
) -> Result<serde_json::Value, String> {
    let key = credential(up)
        .ok_or_else(|| "未配置余额查询凭证（可填独立 Key，或保留推理 Key 作为回退）".to_string())?;
    let cfg = config(up);

    if let Some(err) = validate_config(up) {
        return Err(err);
    }

    let request = build_request(up, adapter, &cfg, key)?;
    let client = client_for_url(state, up, &request.url).await?;
    match adapter {
        BalanceAdapter::DeepSeek => {
            let body = fetch_ok(&client, &request.url, &request.headers).await?;
            parse_deepseek(&body)
        }
        BalanceAdapter::Kimi => {
            let body = fetch_ok(&client, &request.url, &request.headers).await?;
            parse_kimi(&body)
        }
        BalanceAdapter::OpenRouter => {
            let body = fetch_ok(&client, &request.url, &request.headers).await?;
            parse_openrouter(&body)
        }
        BalanceAdapter::Zenmux => {
            let body = fetch_ok(&client, &request.url, &request.headers).await?;
            parse_zenmux(&body)
        }
        BalanceAdapter::Zhipu => {
            let body = fetch_ok(&client, &request.url, &request.headers).await?;
            parse_zhipu(&body)
        }
        BalanceAdapter::OpenaiBilling => probe_openai_billing(&client, up, &request).await,
        BalanceAdapter::Custom => {
            let body = fetch_ok(&client, &request.url, &request.headers).await?;
            parse_custom(&body, &cfg)
        }
    }
}

struct RequestSpec {
    url: String,
    headers: reqwest::header::HeaderMap,
}

fn build_request(
    up: &UpstreamRow,
    adapter: BalanceAdapter,
    cfg: &BalanceConfig,
    key: &str,
) -> Result<RequestSpec, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        HeaderValue::from_static("application/json"),
    );
    let bearer = || format!("Bearer {key}");
    let url = match adapter {
        BalanceAdapter::DeepSeek => {
            insert_auth(&mut headers, "Authorization", &bearer())?;
            format!("{}/user/balance", origin(&up.base_url)?)
        }
        BalanceAdapter::Kimi => {
            insert_auth(&mut headers, "Authorization", &bearer())?;
            join_base(&up.base_url, "users/me/balance")?
        }
        BalanceAdapter::OpenRouter => {
            insert_auth(&mut headers, "Authorization", &bearer())?;
            join_base(&up.base_url, "credits")?
        }
        BalanceAdapter::Zenmux => {
            insert_auth(&mut headers, "Authorization", &bearer())?;
            "https://zenmux.ai/api/v1/management/payg/balance".to_string()
        }
        BalanceAdapter::Zhipu => {
            insert_auth(&mut headers, "Authorization", key)?;
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            headers.insert(
                HeaderName::from_static("accept-language"),
                HeaderValue::from_static("en-US,en"),
            );
            format!("{}/api/monitor/usage/quota/limit", origin(&up.base_url)?)
        }
        BalanceAdapter::OpenaiBilling => {
            insert_auth(&mut headers, "Authorization", &bearer())?;
            // URL 在 probe_openai_billing 内按候选根逐个尝试；这里只放首个。
            billing_roots(&up.base_url)
                .into_iter()
                .next()
                .map(|root| format!("{root}/dashboard/billing/subscription"))
                .ok_or_else(|| "OpenAI billing 基址为空".to_string())?
        }
        BalanceAdapter::Custom => {
            let raw = cfg.url.as_deref().unwrap_or_default();
            let url = resolve_custom_url(&up.base_url, raw)?;
            let header = cfg.header.as_deref().unwrap_or("Authorization").trim();
            if !header.is_empty() {
                let prefix = cfg.prefix.as_deref().unwrap_or("Bearer ");
                insert_auth(&mut headers, header, &format!("{prefix}{key}"))?;
            }
            url
        }
    };
    Ok(RequestSpec { url, headers })
}

fn insert_auth(
    headers: &mut reqwest::header::HeaderMap,
    name: &str,
    value: &str,
) -> Result<(), String> {
    let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| "非法鉴权 Header 名")?;
    let value = HeaderValue::from_str(value).map_err(|_| "鉴权 Header 值非法")?;
    headers.insert(name, value);
    Ok(())
}

fn origin(base_url: &str) -> Result<String, String> {
    let url = Url::parse(base_url).map_err(|e| format!("上游 base_url 非法: {e}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| "上游 base_url 缺少主机名".to_string())?;
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    Ok(format!("{}://{host}{port}", url.scheme()))
}

fn join_base(base_url: &str, path: &str) -> Result<String, String> {
    let base = base_url.trim_end_matches('/');
    if base.is_empty() {
        return Err("上游 base_url 为空".into());
    }
    Ok(format!("{base}/{}", path.trim_start_matches('/')))
}

/// 自定义 URL：相对路径拼到 base_url；以 `/` 开头拼到 origin。
pub fn resolve_custom_url(base_url: &str, raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("自定义余额端点 URL 为空".into());
    }
    validate_custom_url(raw)?;
    if raw.starts_with("https://") {
        return Ok(raw.to_string());
    }
    if raw.starts_with('/') {
        return Ok(format!("{}{}", origin(base_url)?, raw));
    }
    join_base(base_url, raw)
}

fn billing_roots(base_url: &str) -> Vec<String> {
    let mut out = Vec::new();
    let base = base_url.trim_end_matches('/');
    if !base.is_empty() {
        out.push(base.to_string());
    }
    if let Ok(o) = origin(base_url) {
        out.push(format!("{o}/v1"));
        out.push(o);
    }
    out.dedup();
    out
}

async fn client_for_url(
    state: &AppState,
    up: &UpstreamRow,
    url: &str,
) -> Result<reqwest::Client, String> {
    let absolute = url.starts_with("https://");
    let pinned = if absolute {
        crate::netguard::check_outbound_url(url)
            .await
            .map_err(|e| e.to_string())?
    } else {
        Vec::new()
    };
    let proxy_id = up.proxy_id.map(|p| p.to_string()).unwrap_or_default();
    Ok(crate::netguard::onetime_client_via_matrix(
        state,
        up.use_proxy,
        &proxy_id,
        url,
        &pinned,
    ))
}

struct HttpJson {
    status: u16,
    body: Option<serde_json::Value>,
    text: String,
}

async fn fetch_json(
    client: &reqwest::Client,
    url: &str,
    headers: &reqwest::header::HeaderMap,
) -> Result<HttpJson, String> {
    let resp = client
        .get(url)
        .headers(headers.clone())
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status().as_u16();
    let bytes = read_limited(resp).await?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    let body = serde_json::from_slice(&bytes).ok();
    Ok(HttpJson { status, body, text })
}

async fn fetch_ok(
    client: &reqwest::Client,
    url: &str,
    headers: &reqwest::header::HeaderMap,
) -> Result<serde_json::Value, String> {
    let resp = fetch_json(client, url, headers).await?;
    if !(200..300).contains(&resp.status) {
        let msg = summarize_body(&resp.text);
        return Err(format!("上游 HTTP {}: {msg}", resp.status));
    }
    resp.body
        .ok_or_else(|| format!("上游返回非 JSON: {}", summarize_body(&resp.text)))
}

async fn read_limited(resp: reqwest::Response) -> Result<Vec<u8>, String> {
    if let Some(len) = resp.content_length() {
        if len > MAX_BODY_BYTES as u64 {
            return Err(format!("响应体超过 {}KB", MAX_BODY_BYTES / 1024));
        }
    }
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("读取响应失败: {e}"))?;
        if buf.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(format!("响应体超过 {}KB", MAX_BODY_BYTES / 1024));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

fn summarize_body(text: &str) -> String {
    let s = text.trim();
    if s.is_empty() {
        return "空响应".into();
    }
    s.chars().take(300).collect()
}

async fn probe_openai_billing(
    client: &reqwest::Client,
    up: &UpstreamRow,
    first: &RequestSpec,
) -> Result<serde_json::Value, String> {
    let headers = &first.headers;
    let mut last_error = String::from("未找到可用的 OpenAI billing 端点");
    for root in billing_roots(&up.base_url) {
        let sub_url = format!("{root}/dashboard/billing/subscription");
        let usage_url = format!("{root}/dashboard/billing/usage");
        let sub = match fetch_ok(client, &sub_url, headers).await {
            Ok(v) => v,
            Err(e) => {
                last_error = e;
                continue;
            }
        };
        let usage = match fetch_ok(client, &usage_url, headers).await {
            Ok(v) => v,
            Err(e) => {
                last_error = e;
                continue;
            }
        };
        match parse_openai_billing(&sub, &usage) {
            Ok(v) => return Ok(v),
            Err(e) => last_error = e,
        }
    }
    Err(last_error)
}

fn decimal_from(v: &serde_json::Value) -> Option<Decimal> {
    match v {
        serde_json::Value::Number(n) => Decimal::from_str(&n.to_string()).ok(),
        serde_json::Value::String(s) => Decimal::from_str(s.trim()).ok(),
        _ => None,
    }
}

fn decimal_path(v: &serde_json::Value, path: &str) -> Option<Decimal> {
    path_value(v, path).and_then(decimal_from)
}

fn path_value<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    for part in path.split('.') {
        if part.is_empty() {
            return None;
        }
        if let Ok(idx) = part.parse::<usize>() {
            cur = cur.get(idx)?;
        } else {
            cur = cur.get(part)?;
        }
    }
    Some(cur)
}

fn str_path(v: &serde_json::Value, path: &str) -> Option<String> {
    let x = path_value(v, path)?;
    match x {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn pct(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn reset_seconds(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => {
            n.as_i64()
                .map(|x| if x > 1_000_000_000_000 { x / 1000 } else { x })
        }
        serde_json::Value::String(s) => {
            if let Ok(x) = s.parse::<i64>() {
                return Some(if x > 1_000_000_000_000 { x / 1000 } else { x });
            }
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|d| d.timestamp())
        }
        _ => None,
    }
}

fn usd_currency(s: Option<&str>) -> String {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_uppercase())
        .unwrap_or_else(|| "USD".to_string())
}

fn snapshot(adapter: BalanceAdapter, body: serde_json::Value) -> serde_json::Value {
    let mut obj = match body {
        serde_json::Value::Object(o) => o,
        _ => serde_json::Map::new(),
    };
    obj.insert("kind".into(), serde_json::json!("balance"));
    obj.insert("adapter".into(), serde_json::json!(adapter.as_str()));
    obj.insert("source".into(), serde_json::json!("probe"));
    obj.insert(
        "captured_at".into(),
        serde_json::json!(chrono::Utc::now().timestamp()),
    );
    serde_json::Value::Object(obj)
}

fn push_balance(
    balances: &mut Vec<serde_json::Value>,
    currency: &str,
    available: Decimal,
    total: Option<Decimal>,
    used: Option<Decimal>,
) {
    let mut b = serde_json::Map::new();
    b.insert("currency".into(), serde_json::json!(currency));
    b.insert("available".into(), serde_json::json!(available.to_string()));
    if let Some(v) = total {
        b.insert("total".into(), serde_json::json!(v.to_string()));
    }
    if let Some(v) = used {
        b.insert("used".into(), serde_json::json!(v.to_string()));
    }
    balances.push(serde_json::Value::Object(b));
}

fn push_detail(details: &mut Vec<serde_json::Value>, label: &str, value: Decimal, currency: &str) {
    details.push(serde_json::json!({
        "label": label,
        "value": value.to_string(),
        "currency": currency,
    }));
}

/// DeepSeek `/user/balance`。
pub fn parse_deepseek(body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let infos = body
        .get("balance_infos")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "DeepSeek 响应缺少 balance_infos".to_string())?;
    let mut balances = Vec::new();
    let mut details = Vec::new();
    for info in infos {
        let Some(total) = info.get("total_balance").and_then(decimal_from) else {
            continue;
        };
        let currency = usd_currency(info.get("currency").and_then(|v| v.as_str()));
        push_balance(&mut balances, &currency, total, Some(total), None);
        if let Some(v) = info.get("granted_balance").and_then(decimal_from) {
            push_detail(&mut details, "赠送余额", v, &currency);
        }
        if let Some(v) = info.get("topped_up_balance").and_then(decimal_from) {
            push_detail(&mut details, "充值余额", v, &currency);
        }
    }
    if balances.is_empty() {
        return Err("DeepSeek 未返回任何余额".into());
    }
    let available = body
        .get("is_available")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    Ok(snapshot(
        BalanceAdapter::DeepSeek,
        serde_json::json!({
            "balances": balances,
            "details": details,
            "available": available,
        }),
    ))
}

/// Kimi `/v1/users/me/balance`。
pub fn parse_kimi(body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let data = body
        .get("data")
        .ok_or_else(|| "Kimi 响应缺少 data".to_string())?;
    let available = data
        .get("available_balance")
        .and_then(decimal_from)
        .ok_or_else(|| "Kimi 响应缺少 available_balance".to_string())?;
    let mut details = Vec::new();
    if let Some(v) = data.get("voucher_balance").and_then(decimal_from) {
        push_detail(&mut details, "代金券余额", v, "CNY");
    }
    if let Some(v) = data.get("cash_balance").and_then(decimal_from) {
        push_detail(&mut details, "现金余额", v, "CNY");
    }
    let mut balances = Vec::new();
    push_balance(&mut balances, "CNY", available, None, None);
    Ok(snapshot(
        BalanceAdapter::Kimi,
        serde_json::json!({ "balances": balances, "details": details }),
    ))
}

/// OpenRouter `/api/v1/credits`。
pub fn parse_openrouter(body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let data = body
        .get("data")
        .ok_or_else(|| "OpenRouter 响应缺少 data".to_string())?;
    let total = data
        .get("total_credits")
        .and_then(decimal_from)
        .ok_or_else(|| "OpenRouter 响应缺少 total_credits".to_string())?;
    let used = data
        .get("total_usage")
        .and_then(decimal_from)
        .unwrap_or(Decimal::ZERO);
    let available = total - used;
    let mut balances = Vec::new();
    push_balance(&mut balances, "USD", available, Some(total), Some(used));
    Ok(snapshot(
        BalanceAdapter::OpenRouter,
        serde_json::json!({ "balances": balances }),
    ))
}

/// Zenmux management payg balance。
pub fn parse_zenmux(body: &serde_json::Value) -> Result<serde_json::Value, String> {
    if body.get("success").and_then(|v| v.as_bool()) == Some(false) {
        let msg = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("Zenmux: {msg}"));
    }
    let data = body
        .get("data")
        .ok_or_else(|| "Zenmux 响应缺少 data".to_string())?;
    let total = data
        .get("total_credits")
        .and_then(decimal_from)
        .ok_or_else(|| "Zenmux 响应缺少 total_credits".to_string())?;
    let currency = usd_currency(data.get("currency").and_then(|v| v.as_str()));
    let mut balances = Vec::new();
    push_balance(&mut balances, &currency, total, Some(total), None);
    let mut details = Vec::new();
    if let Some(v) = data.get("top_up_credits").and_then(decimal_from) {
        push_detail(&mut details, "充值余额", v, &currency);
    }
    if let Some(v) = data.get("bonus_credits").and_then(decimal_from) {
        push_detail(&mut details, "赠送余额", v, &currency);
    }
    Ok(snapshot(
        BalanceAdapter::Zenmux,
        serde_json::json!({ "balances": balances, "details": details }),
    ))
}

/// 智谱 GLM Coding Plan `/api/monitor/usage/quota/limit`（实验性）。
pub fn parse_zhipu(body: &serde_json::Value) -> Result<serde_json::Value, String> {
    if body.get("success").and_then(|v| v.as_bool()) == Some(false) {
        let msg = body
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("智谱: {msg}"));
    }
    let data = body
        .get("data")
        .ok_or_else(|| "智谱响应缺少 data".to_string())?;
    let mut windows = Vec::new();
    for item in data
        .get("limits")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let ty = item
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if !ty.eq_ignore_ascii_case("TOKENS_LIMIT") && !ty.eq_ignore_ascii_case("CREDIT_LIMIT") {
            continue;
        }
        let Some(used) = item.get("percentage").and_then(pct) else {
            continue;
        };
        let unit = item.get("unit").and_then(|v| v.as_i64());
        let (name, label) = match unit {
            Some(3) => ("5h", "5 小时"),
            Some(6) => ("weekly", "每周"),
            _ => (ty, ty),
        };
        let reset_at = item.get("nextResetTime").and_then(reset_seconds);
        windows.push(serde_json::json!({
            "name": name,
            "label": label,
            "used_percent": used,
            "remaining_percent": (100.0 - used).max(0.0),
            "reset_at": reset_at,
        }));
    }
    if windows.is_empty() {
        return Err("智谱未返回套餐额度（可能非 Coding Plan 账号）".into());
    }
    let plan = data.get("level").and_then(|v| v.as_str());
    Ok(snapshot(
        BalanceAdapter::Zhipu,
        serde_json::json!({ "windows": windows, "plan": plan }),
    ))
}

/// OpenAI billing 兼容：`hard_limit_usd`（美元） - `total_usage`（分）。
pub fn parse_openai_billing(
    subscription: &serde_json::Value,
    usage: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let total = subscription
        .get("hard_limit_usd")
        .and_then(decimal_from)
        .or_else(|| subscription.get("soft_limit_usd").and_then(decimal_from))
        .ok_or_else(|| "OpenAI billing 响应缺少 hard_limit_usd".to_string())?;
    let used_cents = usage
        .get("total_usage")
        .and_then(decimal_from)
        .unwrap_or(Decimal::ZERO);
    let used = used_cents / Decimal::from(100);
    let available = total - used;
    let mut balances = Vec::new();
    push_balance(&mut balances, "USD", available, Some(total), Some(used));
    let mut details = Vec::new();
    if let Some(v) = subscription.get("soft_limit_usd").and_then(decimal_from) {
        push_detail(&mut details, "软限额", v, "USD");
    }
    Ok(snapshot(
        BalanceAdapter::OpenaiBilling,
        serde_json::json!({ "balances": balances, "details": details }),
    ))
}

/// 自定义点路径映射。
pub fn parse_custom(
    body: &serde_json::Value,
    cfg: &BalanceConfig,
) -> Result<serde_json::Value, String> {
    let amount_path = cfg
        .amount_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "缺少 amount_path".to_string())?;
    let available =
        decimal_path(body, amount_path).ok_or_else(|| format!("未找到余额字段 `{amount_path}`"))?;
    let currency = if let Some(path) = cfg
        .currency_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        str_path(body, path).ok_or_else(|| format!("未找到币种字段 `{path}`"))?
    } else {
        cfg.currency
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "未配置币种".to_string())?
            .to_string()
    };
    let currency = usd_currency(Some(&currency));
    let total = cfg
        .total_path
        .as_deref()
        .and_then(|p| decimal_path(body, p));
    let used = cfg.used_path.as_deref().and_then(|p| decimal_path(body, p));
    let reset_at = cfg
        .reset_at_path
        .as_deref()
        .and_then(|p| path_value(body, p))
        .and_then(reset_seconds);
    let mut balances = Vec::new();
    push_balance(&mut balances, &currency, available, total, used);
    let mut details = Vec::new();
    if let Some(v) = total {
        push_detail(&mut details, "总额", v, &currency);
    }
    if let Some(v) = used {
        push_detail(&mut details, "已用", v, &currency);
    }
    Ok(snapshot(
        BalanceAdapter::Custom,
        serde_json::json!({
            "balances": balances,
            "details": details,
            "reset_at": reset_at,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(kind: &str) -> UpstreamRow {
        UpstreamRow {
            id: uuid::Uuid::new_v4(),
            name: "up".into(),
            kind: kind.into(),
            base_url: "https://api.deepseek.com/v1".into(),
            api_key_plain: Some("sk-test".into()),
            oauth_plain: None,
            balance_key_plain: None,
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
            extra: serde_json::json!({}),
            model_sync: "manual".into(),
            model_exclude: vec![],
            models_cache: serde_json::json!([]),
            models_fetched_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn adapter_auto_by_kind_and_override() {
        assert_eq!(
            resolve_adapter(&up("deepseek")),
            Some(BalanceAdapter::DeepSeek)
        );
        assert_eq!(resolve_adapter(&up("kimi")), Some(BalanceAdapter::Kimi));
        assert_eq!(
            resolve_adapter(&up("openrouter")),
            Some(BalanceAdapter::OpenRouter)
        );
        assert_eq!(resolve_adapter(&up("zenmux")), Some(BalanceAdapter::Zenmux));
        assert_eq!(resolve_adapter(&up("zhipu")), Some(BalanceAdapter::Zhipu));
        assert_eq!(resolve_adapter(&up("openai")), None);
        let mut u = up("openai");
        u.extra = serde_json::json!({"balance":{"adapter":"openai_billing"}});
        assert_eq!(resolve_adapter(&u), Some(BalanceAdapter::OpenaiBilling));
        u.extra = serde_json::json!({"balance":{"adapter":"off"}});
        assert_eq!(resolve_adapter(&u), None);
    }

    #[test]
    fn deepseek_parses_multi_currency() {
        let body = serde_json::json!({
            "is_available": true,
            "balance_infos": [
                {"currency":"CNY","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"},
                {"currency":"USD","total_balance":"5.50","granted_balance":"0.50","topped_up_balance":"5.00"}
            ]
        });
        let s = parse_deepseek(&body).unwrap();
        assert_eq!(s["kind"], "balance");
        assert_eq!(s["adapter"], "deepseek");
        assert_eq!(s["balances"].as_array().unwrap().len(), 2);
        assert_eq!(s["balances"][0]["available"], "110.00");
    }

    #[test]
    fn kimi_parses_balance_and_details() {
        let body = serde_json::json!({
            "code":0,
            "data":{"available_balance":49.5,"voucher_balance":46.5,"cash_balance":3.0}
        });
        let s = parse_kimi(&body).unwrap();
        assert_eq!(s["balances"][0]["currency"], "CNY");
        assert_eq!(s["balances"][0]["available"], "49.5");
        assert_eq!(s["details"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn openrouter_subs_usage() {
        let body = serde_json::json!({"data":{"total_credits":100.5,"total_usage":25.75}});
        let s = parse_openrouter(&body).unwrap();
        assert_eq!(s["balances"][0]["available"], "74.75");
        assert_eq!(s["balances"][0]["used"], "25.75");
    }

    #[test]
    fn zenmux_parses_balance() {
        let body = serde_json::json!({
            "success":true,
            "data":{"currency":"usd","total_credits":482.74,"top_up_credits":35.0,"bonus_credits":447.74}
        });
        let s = parse_zenmux(&body).unwrap();
        assert_eq!(s["balances"][0]["currency"], "USD");
        assert_eq!(s["balances"][0]["available"], "482.74");
    }

    #[test]
    fn zhipu_classifies_windows() {
        let body = serde_json::json!({
            "success":true,
            "data":{"level":"pro","limits":[
                {"type":"TOKENS_LIMIT","unit":3,"percentage":23,"nextResetTime":1712345678000i64},
                {"type":"TOKENS_LIMIT","unit":6,"percentage":41,"nextResetTime":1712345678000i64}
            ]}
        });
        let s = parse_zhipu(&body).unwrap();
        assert_eq!(s["plan"], "pro");
        assert_eq!(s["windows"][0]["name"], "5h");
        assert_eq!(s["windows"][1]["name"], "weekly");
        assert_eq!(s["windows"][0]["remaining_percent"], 77.0);
    }

    #[test]
    fn openai_billing_cents_to_usd() {
        let sub = serde_json::json!({"hard_limit_usd":120.0,"soft_limit_usd":100.0});
        let usage = serde_json::json!({"total_usage":1234});
        let s = parse_openai_billing(&sub, &usage).unwrap();
        assert_eq!(s["balances"][0]["total"], "120.0");
        assert_eq!(s["balances"][0]["used"], "12.34");
        assert_eq!(s["balances"][0]["available"], "107.66");
    }

    #[test]
    fn custom_paths_and_currency() {
        let cfg = BalanceConfig {
            adapter: Some("custom".into()),
            url: Some("/api/user/self".into()),
            amount_path: Some("data.balance".into()),
            currency_path: Some("data.currency".into()),
            total_path: Some("data.total".into()),
            used_path: Some("data.used".into()),
            ..Default::default()
        };
        let body = serde_json::json!({"data":{"balance":"12.5","currency":"cny","total":"20","used":"7.5"}});
        let s = parse_custom(&body, &cfg).unwrap();
        assert_eq!(s["balances"][0]["currency"], "CNY");
        assert_eq!(s["balances"][0]["available"], "12.5");
    }

    #[test]
    fn custom_array_paths_and_reset() {
        let cfg = BalanceConfig {
            amount_path: Some("data.items.0.balance".into()),
            currency_path: Some("data.items.0.currency".into()),
            reset_at_path: Some("data.items.0.reset_at".into()),
            ..Default::default()
        };
        let body = serde_json::json!({"data":{"items":[{"balance":"8","currency":"usd","reset_at":1712345678}]}});
        let s = parse_custom(&body, &cfg).unwrap();
        assert_eq!(s["balances"][0]["currency"], "USD");
        assert_eq!(s["balances"][0]["available"], "8");
        assert_eq!(s["reset_at"], 1712345678i64);
    }

    #[test]
    fn custom_missing_amount_and_currency() {
        let cfg = BalanceConfig {
            amount_path: Some("data.balance".into()),
            currency: Some("CNY".into()),
            ..Default::default()
        };
        let body = serde_json::json!({"data":{"amount":"1"}});
        assert!(parse_custom(&body, &cfg).is_err());
    }

    #[test]
    fn custom_url_resolution() {
        assert_eq!(
            resolve_custom_url("https://api.example.com/v1", "/user/balance").unwrap(),
            "https://api.example.com/user/balance"
        );
        assert_eq!(
            resolve_custom_url("https://api.example.com/v1", "users/me/balance").unwrap(),
            "https://api.example.com/v1/users/me/balance"
        );
        assert!(resolve_custom_url("https://api.example.com/v1", "http://x.com/a").is_err());
        assert!(resolve_custom_url("https://api.example.com/v1", "//evil.com/a").is_err());
    }

    #[test]
    fn support_off_and_invalid_custom() {
        let mut u = up("deepseek");
        u.extra = serde_json::json!({"balance":{"adapter":"off"}});
        let s = support(&u);
        assert!(!s.supported);
        assert!(s.reason.unwrap().contains("关闭"));

        u.kind = "custom".into();
        u.extra = serde_json::json!({"balance":{"adapter":"custom"}});
        let s = support(&u);
        assert!(!s.supported);
        assert!(s.reason.unwrap().contains("URL"));
    }

    #[test]
    fn custom_config_validation() {
        let mut u = up("custom");
        u.extra = serde_json::json!({"balance":{"adapter":"custom"}});
        assert!(validate_config(&u).is_some());
        u.extra = serde_json::json!({"balance":{
            "adapter":"custom","url":"/x","amount_path":"data.balance","currency":"CNY"
        }});
        assert!(validate_config(&u).is_none());
    }

    #[test]
    fn credential_falls_back_to_api_key() {
        let mut u = up("deepseek");
        assert_eq!(credential(&u), Some("sk-test"));
        u.balance_key_plain = Some("mgmt-key".into());
        assert_eq!(credential(&u), Some("mgmt-key"));
    }
}
