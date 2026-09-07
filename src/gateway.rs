//! 网关入口与请求主链路（PLAN.md §3.1/§3.2）。
//!
//! 请求 → 鉴权（网关 Key）→ 限流/用量上限 → 模型白名单 → 协议解析 → 路由决策
//!      → 透传/转换 → 上游调用（重试/熔断/故障转移）→ 响应转换回入口协议 → 客户端。
//! （旁路 usage 提取与记账在 M4 接入；本模块每请求生成 request_id 并写 X-Request-Id。）

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use futures::Stream;
use sha2::{Digest, Sha256};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::logging::LogEvent;

use crate::entities::{ApiKeyRow, ModelRouteRow, UpstreamRow};
use crate::error::ApiError;
use crate::limit;
use crate::media::{self, ImageApi, ImageOutcome};
use crate::metrics::RequestLabels;
use crate::protocol;
use crate::protocol::ir::Protocol;
use crate::protocol::{error_from_ir, error_to_ir, ConvCtx, ConvertError, IrError};
use crate::routing;
use crate::state::AppState;
use crate::upstream::{self, UpstreamError};

/// 网关注入的响应头。
pub const HDR_REQUEST_ID: &str = "x-request-id";
pub const HDR_DEGRADED: &str = "x-nextapi-degraded";
/// 服务版本头（M10.5 标准客户端兼容）
pub const HDR_VERSION: &str = "x-nextapi-version";
/// 限流上限/剩余头（M10.5，可选）
pub const HDR_RL_LIMIT: &str = "x-ratelimit-limit";
pub const HDR_RL_REMAINING: &str = "x-ratelimit-remaining";

/// 网关版本（编译期确定）。
pub const GATEWAY_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 网关入口协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    OpenaiChat,
    OpenaiResponses,
    Anthropic,
    Gemini,
}

impl Entry {
    /// 入口对应的 IR 协议。
    fn protocol(self) -> Protocol {
        match self {
            Entry::OpenaiChat => Protocol::OpenaiChat,
            Entry::OpenaiResponses => Protocol::OpenaiResponses,
            Entry::Anthropic => Protocol::Anthropic,
            Entry::Gemini => Protocol::Gemini,
        }
    }

    /// 入口协议的首选鉴权头名。
    fn primary_auth_header(self) -> &'static str {
        match self {
            Entry::OpenaiChat | Entry::OpenaiResponses => "authorization",
            Entry::Anthropic => "x-api-key",
            Entry::Gemini => "x-goog-api-key",
        }
    }
}

/// 协议决策结果（透传 / 转换 / 转换失败回退透传）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Passthrough,
    Convert(Protocol),
}

/// 候选 = 路由 × 上游。
#[derive(Debug, Clone)]
struct Candidate {
    route: ModelRouteRow,
    upstream: UpstreamRow,
}

/// 已决策的候选：候选 + 本次请求的协议模式。
#[derive(Debug, Clone)]
struct Attempt {
    candidate: Candidate,
    mode: Mode,
}

/// 待发送上游的请求。
struct Outbound {
    body: serde_json::Value,
    /// 实际请求协议（透传 = 入口协议；转换 = 目标协议）。
    protocol: Protocol,
    mode: Mode,
}

/// 执行结果。
enum ExecOutcome {
    Json(serde_json::Value),
    Stream(reqwest::Response),
}

/// 候选失败原因。
enum Fail {
    Upstream(UpstreamError),
    Convert(ConvertError),
}

// ---------------------------------------------------------------------------
// 流式 tee 记账（合同 §12.3）：在输出 Body 外再包一层收集流，
// 逐 chunk 收集文本（上限 256KB）+ 记录 ttfb，流结束（含 Err）时组装 LogEvent 并 sink.log。
// ---------------------------------------------------------------------------

/// 流式收集状态：text 为 UTF-8 安全累积的 SSE 文本；ttfb_ms 为首 chunk 相对请求起点的毫秒。
struct StreamCollect {
    text: String,
    ttfb_ms: Option<i32>,
}

/// 单次流式响应的收集上限（字节）。
const STREAM_TEXT_MAX: usize = 256 * 1024;

/// 把字节以 UTF-8 安全方式追加到收集缓冲，超限时丢弃**头部**保留**尾部**。
///
/// 设计依据（review P1-#4）：usage 提取只关心流尾的 usage 帧
/// （OpenAI include_usage 尾帧 / Anthropic message_delta / Gemini usageMetadata），
/// 原「头部截断」会在长流（>256KB）时把这些帧全部丢弃导致计量漏记；
/// 改为尾部滑动保留后，长流的 usage/quota/成本计量不再失真。
fn append_chunk(collect: &Mutex<StreamCollect>, bytes: &[u8], start: Instant) {
    let mut c = collect.lock().unwrap_or_else(|p| p.into_inner());
    if bytes.is_empty() {
        if c.ttfb_ms.is_none() {
            c.ttfb_ms = Some(start.elapsed().as_millis() as i32);
        }
        return;
    }
    // lossy 追加：跨 chunk 的多字节字符边界以 U+FFFD 替换（只影响个别文本字符，
    // usage 帧为 ASCII 结构不受影响；不切断 JSON 结构字符）。
    c.text.push_str(&String::from_utf8_lossy(bytes));
    if c.text.len() > STREAM_TEXT_MAX {
        let excess = c.text.len() - STREAM_TEXT_MAX;
        // 从头部按字符边界移除（drain 需在 char 边界，向前找到第一个边界）
        let mut cut = excess;
        while cut < c.text.len() && !c.text.is_char_boundary(cut) {
            cut += 1;
        }
        c.text.drain(..cut);
    }
    if c.ttfb_ms.is_none() {
        c.ttfb_ms = Some(start.elapsed().as_millis() as i32);
    }
}

/// 将 axum::Error 映射为 std::io::Error（供 Body::from_stream 的 Err 类型约束）。
fn to_io_err(e: axum::Error) -> std::io::Error {
    std::io::Error::other(e.into_inner())
}

/// 打包一次流式记账上下文，供 TeeStream 在流结束时 spawn 处理。
struct TeeLogCtx {
    state: Arc<AppState>,
    tmpl: LogEvent,
    labels: RequestLabels,
    params_meta: serde_json::Value,
    debug_req: Option<(serde_json::Value, bool)>,
    retry_count: i32,
    entry_protocol: Protocol,
}

/// 包装 Body 的收集流：透传 chunk，结束时（含 Err）按入口协议提取 usage 并记账。
/// Ok 项为 Bytes；Err 项为 io::Error（以匹配 Body::from_stream 的 Into<BoxError> 约束）。
struct TeeStream {
    inner: axum::body::BodyDataStream,
    collect: Arc<Mutex<StreamCollect>>,
    start: Instant,
    ctx: Option<TeeLogCtx>,
    done: bool,
    finalized: bool,
}

impl Stream for TeeStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if self.done {
                if !self.finalized {
                    self.finalized = true;
                    finalize_stream(self.get_mut());
                }
                return Poll::Ready(None);
            }
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    append_chunk(&self.collect, &bytes, self.start);
                    return Poll::Ready(Some(Ok(bytes)));
                }
                Poll::Ready(Some(Err(e))) => {
                    self.done = true;
                    // 转发错误；下次 poll 会 finalize 一次性记账
                    return Poll::Ready(Some(Err(to_io_err(e))));
                }
                Poll::Ready(None) => {
                    self.done = true;
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// 客户端提前断开（Body 被 drop 而流未到尾）时补记账（review P3）：
/// 正常走完的流在 poll 中已 finalize（finalized=true 或 ctx 已取走），此处仅补漏。
impl Drop for TeeStream {
    fn drop(&mut self) {
        if self.finalized || self.ctx.is_none() {
            return;
        }
        self.finalized = true;
        if tokio::runtime::Handle::try_current().is_ok() {
            finalize_stream(self);
        }
    }
}

/// 流结束（含 Err）：取收集文本，按入口协议提取 usage，组装 LogEvent 并 sink.log
/// （status=200 —— HTTP 响应头已发，即使中途断流，客户端看到的仍是 200）。
fn finalize_stream(stream: &mut TeeStream) {
    let Some(ctx) = stream.ctx.take() else { return };
    let collect = stream.collect.clone();
    let start = stream.start;
    tokio::spawn(async move {
        let c = collect.lock().unwrap_or_else(|p| p.into_inner());
        let text = c.text.clone();
        let ttfb = c.ttfb_ms;
        drop(c);

        let usage = crate::upstream::usage::extract_sse_usage(ctx.entry_protocol, &text);
        let mut ev = ctx.tmpl.clone();
        ev.status = 200;
        ev.latency_ms = Some(start.elapsed().as_millis() as i32);
        ev.ttfb_ms = ttfb;
        ev.retry_count = ctx.retry_count;
        ev.prompt_tokens = usage.prompt_tokens;
        ev.completion_tokens = usage.completion_tokens;
        ev.cache_write_tokens = usage.cache_write_tokens;
        ev.cache_read_tokens = usage.cache_read_tokens;
        ev.usage_raw = Some(serde_json::json!({
            "params": ctx.params_meta.clone(),
            "usage": usage.raw.clone().unwrap_or(serde_json::Value::Null),
        }));
        if let Some((req, req_trunc)) = &ctx.debug_req {
            let (resp, resp_trunc) = crate::logging::redact::redact_and_truncate(
                text.as_bytes(),
                crate::logging::DEBUG_MAX_BYTES,
            );
            ev.debug_payload = Some(serde_json::json!({
                "request": req.clone(),
                "response": resp,
                "truncated": *req_trunc || resp_trunc,
            }));
        }
        ctx.state.log_sink.log(ev);

        let total = usage
            .prompt_tokens
            .unwrap_or(0)
            .saturating_add(usage.completion_tokens.unwrap_or(0));
        if total > 0 {
            ctx.state
                .metrics
                .gateway_tokens
                .get_or_create(&ctx.labels)
                .inc_by(total.max(0) as u64);
        }
    });
}

/// 在输出 Body 外再包一层 tee 收集流。
#[allow(clippy::too_many_arguments)]
fn tee_log_stream(
    inner: Body,
    state: Arc<AppState>,
    tmpl: LogEvent,
    collect: Arc<Mutex<StreamCollect>>,
    start: Instant,
    entry_protocol: Protocol,
    labels: RequestLabels,
    params_meta: serde_json::Value,
    debug_req: Option<(serde_json::Value, bool)>,
    retry_count: i32,
) -> Body {
    let stream = TeeStream {
        inner: inner.into_data_stream(),
        collect,
        start,
        ctx: Some(TeeLogCtx {
            state,
            tmpl,
            labels,
            params_meta,
            debug_req,
            retry_count,
            entry_protocol,
        }),
        done: false,
        finalized: false,
    };
    Body::from_stream(stream)
}

// ---------------------------------------------------------------------------
// 纯逻辑函数（便于单测）
// ---------------------------------------------------------------------------

/// 按入口协议提取网关 Key：优先入口首选鉴权头，否则回退其它兼容头（三者任一存在即可）。
fn extract_api_key(headers: &HeaderMap, entry: Entry) -> Option<String> {
    let mut order: Vec<&'static str> = vec![entry.primary_auth_header()];
    for h in ["authorization", "x-api-key", "x-goog-api-key"] {
        if !order.contains(&h) {
            order.push(h);
        }
    }
    for name in order {
        if let Some(v) = headers.get(name) {
            if let Ok(s) = v.to_str() {
                if let Some(k) = clean_auth_value(name, s) {
                    return Some(k);
                }
            }
        }
    }
    None
}

fn clean_auth_value(name: &str, raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    if name.eq_ignore_ascii_case("authorization") {
        // scheme 大小写不敏感（Bearer/bearer/BEARER 均接受，review P3）
        let val = match t.split_once(char::is_whitespace) {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("bearer") => rest.trim(),
            _ => t,
        };
        if !val.is_empty() {
            Some(val.to_string())
        } else {
            None
        }
    } else {
        Some(t.to_string())
    }
}

/// sha256 hex 化（网关 Key 鉴权）。
fn hash_key(key: &str) -> String {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    format!("{:x}", h.finalize())
}

/// 网关 Key 鉴权（媒体端点与主链路复用；契约 m6 §4）。
///
/// 以 OpenAI Chat 头序提取网关 Key → sha256 → 内存快照命中 → is_usable()。
/// 任一环节失败返回 401（OpenAI 协议错误体）。request_id 在函数内自产（契约签名
/// 冻结为 (state, headers)；主链路/媒体端点各自已有独立 request_id，鉴权失败的
/// 401 仅在 X-Request-Id 头中出现，无需与请求日志一致）。
///
/// 媒体端点（images/videos）统一走此函数；run_gateway 步骤 2 亦改用它，行为不变。
pub(crate) fn authenticate_gateway_key(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<ApiKeyRow, Box<Response>> {
    let request_id = Uuid::new_v4().to_string();
    let Some(raw_key) = extract_api_key(headers, Entry::OpenaiChat) else {
        return Err(Box::new(error_resp(
            Protocol::OpenaiChat,
            &request_id,
            401,
            "未授权",
        )));
    };
    let snap = state.cache.snapshot();
    let Some(key) = snap.api_keys.get(&hash_key(&raw_key)).cloned() else {
        return Err(Box::new(error_resp(
            Protocol::OpenaiChat,
            &request_id,
            401,
            "未授权",
        )));
    };
    if !key.is_usable() {
        return Err(Box::new(error_resp(
            Protocol::OpenaiChat,
            &request_id,
            401,
            "未授权",
        )));
    }
    Ok(key)
}

/// 解析 Gemini action 路径：`model:generateContent` / `model:streamGenerateContent`。
/// 返回 `(model, stream)`；非法 action 返回 None。
fn parse_gemini_action(action: &str) -> Option<(String, bool)> {
    let action = action.trim().trim_start_matches('/');
    let (model, method) = action.split_once(':')?;
    let model = model.trim();
    if model.is_empty() {
        return None;
    }
    match method {
        "generateContent" => Some((model.to_string(), false)),
        "streamGenerateContent" => Some((model.to_string(), true)),
        _ => None,
    }
}

/// 提取请求模型名：Gemini 取路径 model，其余取 body["model"]；缺失返回 BadRequest。
fn extract_model(
    entry: Entry,
    body: &serde_json::Value,
    gemini_model: Option<&str>,
) -> Result<String, ApiError> {
    match entry {
        Entry::Gemini => match gemini_model {
            Some(m) if !m.is_empty() => Ok(m.to_string()),
            _ => Err(ApiError::BadRequest("Gemini 模型缺失".into())),
        },
        _ => match body["model"].as_str() {
            Some(m) if !m.is_empty() => Ok(m.to_string()),
            _ => Err(ApiError::BadRequest("model 缺失".into())),
        },
    }
}

/// 提取 max_tokens / max_completion_tokens（用于 TPM 预检）。
/// TPM 预检的 max_tokens 提取：按入口协议分派字段名（发布审阅 M4）——
/// Chat: max_tokens/max_completion_tokens；Responses: max_output_tokens；
/// Anthropic: max_tokens；Gemini: generationConfig.maxOutputTokens。
fn extract_max_tokens(protocol: Protocol, body: &serde_json::Value) -> Option<u64> {
    match protocol {
        Protocol::OpenaiChat => body
            .get("max_tokens")
            .or_else(|| body.get("max_completion_tokens"))
            .and_then(|v| v.as_u64()),
        Protocol::OpenaiResponses => body.get("max_output_tokens").and_then(|v| v.as_u64()),
        Protocol::Anthropic => body.get("max_tokens").and_then(|v| v.as_u64()),
        Protocol::Gemini => body
            .get("generationConfig")
            .and_then(|g| g.get("maxOutputTokens"))
            .and_then(|v| v.as_u64()),
    }
}

/// 别名解析（M10.1）：入口模型命中**启用**的别名 → 返回 (实际模型, Some(入口模型))；
/// 否则原样返回 (model, None)。单跳解析，不链式（管理 API 禁止别名指向别名）。
fn resolve_alias(snap: &crate::cache::Snapshot, model: &str) -> (String, Option<String>) {
    match snap.aliases.get(model) {
        Some(a) if a.enabled && !a.model.trim().is_empty() => {
            (a.model.clone(), Some(model.to_string()))
        }
        _ => (model.to_string(), None),
    }
}

/// 从请求体检测能力需求（M10.3）：tools 参数 / 图片 part / stream。
/// 覆盖四协议形态：chat messages[].content[]（image_url）、responses input[].content[]
/// （input_image）、anthropic messages[].content[]（image）、gemini contents[].parts[]
/// （inline_data/file_data）。
fn required_caps(body: &serde_json::Value, stream: bool) -> crate::entities::RequiredCaps {
    let mut req = crate::entities::RequiredCaps {
        stream,
        ..Default::default()
    };
    // tools：chat tools[] 与 legacy functions[]；responses/anthropic/gemini 均 tools[]
    if body["tools"].as_array().is_some_and(|t| !t.is_empty())
        || body["functions"].as_array().is_some_and(|f| !f.is_empty())
    {
        req.tools = true;
    }
    // vision：消息内容 part 中的图片形态
    let is_image_part = |p: &serde_json::Value| {
        matches!(
            p["type"].as_str(),
            Some("image_url") | Some("input_image") | Some("image")
        ) || p.get("inline_data").is_some()
            || p.get("file_data").is_some()
    };
    'outer: for key in ["messages", "input", "contents"] {
        if let Some(items) = body[key].as_array() {
            for it in items {
                for sub in ["content", "parts"] {
                    if it[sub]
                        .as_array()
                        .is_some_and(|ps| ps.iter().any(is_image_part))
                    {
                        req.vision = true;
                        break 'outer;
                    }
                }
            }
        }
    }
    req
}
/// 是否流式请求：Gemini 以路径 action 判定，其余以 body["stream"] 判定。
fn is_stream(body: &serde_json::Value, gemini_stream: Option<bool>) -> bool {
    match gemini_stream {
        Some(s) => s,
        None => body["stream"].as_bool().unwrap_or(false),
    }
}

/// 协议决策（PLAN.md §3.2）：entry ∈ 上游支持 → 透传；否则按优先级取第一个支持的协议转换；
/// 无任何可服务协议 → None。
fn decide_mode(entry: Protocol, up: &UpstreamRow) -> Option<Mode> {
    let supported = up.protocol_list();
    if supported.contains(&entry) {
        return Some(Mode::Passthrough);
    }
    for target in up.protocol_priority() {
        if supported.contains(&target) {
            return Some(Mode::Convert(target));
        }
    }
    None
}

/// 候选排序构建：按 priority 升序分组，每组 weighted_pick 得一个主候选，同组其余作为
/// 故障转移后续候选（按权重降序排列）；每组均出现在有序尝试列表中。
fn order_candidates(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut groups: std::collections::BTreeMap<i32, Vec<Candidate>> =
        std::collections::BTreeMap::new();
    for c in candidates {
        groups.entry(c.route.priority).or_default().push(c);
    }
    let mut ordered: Vec<Candidate> = Vec::new();
    for (_prio, group) in groups {
        let routes: Vec<ModelRouteRow> = group.iter().map(|c| c.route.clone()).collect();
        if let Some(primary) = routing::weighted_pick(&routes) {
            if let Some(p) = group.iter().find(|c| c.route.id == primary.id).cloned() {
                ordered.push(p.clone());
                let mut rest: Vec<Candidate> = group
                    .iter()
                    .filter(|c| c.route.id != p.route.id)
                    .cloned()
                    .collect();
                rest.sort_by(|a, b| {
                    b.route
                        .weight
                        .cmp(&a.route.weight)
                        .then(a.route.sort_order.cmp(&b.route.sort_order))
                });
                ordered.extend(rest);
            }
        }
    }
    ordered
}

/// 异步版请求构建：转换目标为 Gemini 时先受控下载 URL 图片转 inlineData
/// （参考 new-api：URL 一律下载转 inlineData，不用 fileData.fileUri 直传，
/// 兼容旧模型与 Gemini 中转）。其余情况委托同步 build_outbound。
/// img_cache 由调用方在重试循环外持有，同 URL 跨候选只下载一次。
async fn build_outbound_any(
    state: &AppState,
    entry: Protocol,
    mode: Mode,
    body: &serde_json::Value,
    stream: bool,
    upstream_model: &str,
    upstream: &UpstreamRow,
    img_cache: &mut crate::media::resolve::ResolveCache,
) -> Result<Outbound, ConvertError> {
    if let Mode::Convert(Protocol::Gemini) = mode {
        let mut ctx = ConvCtx::new();
        let mut ir = protocol::request_to_ir(entry, body, &mut ctx)?;
        ir.model = upstream_model.to_string();
        ir.stream = stream;
        crate::media::resolve::resolve_url_images(state, &mut ir, img_cache, &mut ctx).await;
        let mut out = protocol::request_from_ir(Protocol::Gemini, &ir, &mut ctx)?;
        upstream::rewrite_body(&mut out, None, upstream.overrides().as_ref());
        return Ok(Outbound {
            body: out,
            protocol: Protocol::Gemini,
            mode,
        });
    }
    build_outbound(entry, mode, body, stream, upstream_model, upstream)
}

/// 构建待发送上游的请求体（透传改写 / 协议转换），并处理流式 usage 采集兜底。
fn build_outbound(
    entry: Protocol,
    mode: Mode,
    body: &serde_json::Value,
    stream: bool,
    upstream_model: &str,
    upstream: &UpstreamRow,
) -> Result<Outbound, ConvertError> {
    match mode {
        Mode::Passthrough => {
            let mut b = body.clone();
            upstream::rewrite_body(&mut b, Some(upstream_model), upstream.overrides().as_ref());
            // PLAN §4.4：OpenAI 系透传流式注入 stream_options.include_usage 用于 usage 采集兜底
            inject_stream_usage_option(&mut b, entry, stream);
            Ok(Outbound {
                body: b,
                protocol: entry,
                mode,
            })
        }
        Mode::Convert(target) => {
            let mut ctx = ConvCtx::new();
            let mut ir = protocol::request_to_ir(entry, body, &mut ctx)?;
            ir.model = upstream_model.to_string();
            ir.stream = stream;
            // 流式且目标为 openai 系时注入 stream_options.include_usage（PLAN §4.4）
            if stream && matches!(target, Protocol::OpenaiChat | Protocol::OpenaiResponses) {
                ir.stream_include_usage = true;
            }
            let mut out = protocol::request_from_ir(target, &ir, &mut ctx)?;
            // 转换路径同样应用用户配置的 body 覆盖（发布审阅 L8：此前仅透传生效）
            upstream::rewrite_body(&mut out, None, upstream.overrides().as_ref());
            Ok(Outbound {
                body: out,
                protocol: target,
                mode,
            })
        }
    }
}

/// 若为 OpenAI Chat 流式请求且未带 stream_options.include_usage，则注入该选项。
fn inject_stream_usage_option(body: &mut serde_json::Value, protocol: Protocol, stream: bool) {
    if !stream || protocol != Protocol::OpenaiChat {
        return;
    }
    let Some(obj) = body.as_object_mut() else {
        return;
    };
    let so = obj
        .entry("stream_options")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(m) = so.as_object_mut() {
        m.insert("include_usage".into(), serde_json::Value::Bool(true));
    }
}

// ---------------------------------------------------------------------------
// 响应构建
// ---------------------------------------------------------------------------

/// 构造 JSON 响应（自动写 content-type / X-Request-Id / 可选 X-NextAPI-Degraded）。
fn json_rsp(
    status: StatusCode,
    request_id: &str,
    degraded: Option<&str>,
    body: serde_json::Value,
) -> Response {
    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h.insert(
        HDR_REQUEST_ID,
        request_id
            .parse::<header::HeaderValue>()
            .unwrap_or_else(|_| header::HeaderValue::from_static("")),
    );
    if let Some(d) = degraded {
        if let Ok(dv) = header::HeaderValue::from_str(d) {
            h.insert(HDR_DEGRADED, dv);
        }
    }
    h.insert(
        HDR_VERSION,
        header::HeaderValue::from_static(GATEWAY_VERSION),
    );
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    (status, h, Body::from(bytes)).into_response()
}

/// 以入口协议（错误结构）构造错误 JSON 响应。
fn error_resp(entry: Protocol, request_id: &str, status: u16, message: &str) -> Response {
    let ir = IrError {
        status,
        message: message.to_string(),
        ..Default::default()
    };
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    json_rsp(code, request_id, None, error_from_ir(entry, &ir))
}

/// 构造 SSE 流式响应。
fn stream_rsp(request_id: &str, body: Body) -> Response {
    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    h.insert(
        HDR_REQUEST_ID,
        request_id
            .parse::<header::HeaderValue>()
            .unwrap_or_else(|_| header::HeaderValue::from_static("")),
    );
    h.insert(
        HDR_VERSION,
        header::HeaderValue::from_static(GATEWAY_VERSION),
    );
    (StatusCode::OK, h, body).into_response()
}

/// 附加速率限制头（M10.5）：Some((limit, remaining)) 时写入；None（不限流）不写。
fn add_ratelimit_headers(resp: &mut Response, rl: Option<(u32, u32)>) {
    if let Some((limit, remaining)) = rl {
        let h = resp.headers_mut();
        if let Ok(v) = header::HeaderValue::from_str(&limit.to_string()) {
            h.insert(HDR_RL_LIMIT, v);
        }
        if let Ok(v) = header::HeaderValue::from_str(&remaining.to_string()) {
            h.insert(HDR_RL_REMAINING, v);
        }
    }
}

/// 粘性重排（M11.3）：pinned 上游位于**最优优先级组**内才提到首位，否则忽略。
/// 纯函数，供测试。
fn reorder_pinned_first(attempts: &mut Vec<Attempt>, pin: Uuid) {
    let Some(best) = attempts.first().map(|a| a.candidate.route.priority) else {
        return;
    };
    if let Some(pos) = attempts
        .iter()
        .position(|a| a.candidate.upstream.id == pin && a.candidate.route.priority == best)
    {
        let a = attempts.remove(pos);
        attempts.insert(0, a);
    }
}

/// mode 转换后的字符串（contract §12.7）。
fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Passthrough => "passthrough",
        Mode::Convert(_) => "convert",
    }
}

/// 错误摘要 ≤2000 字符（不切断 UTF-8 码点）。
fn truncate_msg(msg: &str) -> String {
    if msg.len() <= 2000 {
        return msg.to_string();
    }
    let mut end = 2000;
    while end > 0 && !msg.is_char_boundary(end) {
        end -= 1;
    }
    msg[..end].to_string()
}

/// 从 Fail 中抽取人类可读的消息摘要。
fn fail_message(fail: &Fail) -> String {
    match fail {
        Fail::Upstream(e) => match e {
            UpstreamError::Status(_, body) => body.clone(),
            other => other.to_string(),
        },
        Fail::Convert(e) => e.to_string(),
    }
}

/// 构建 debug_payload（req 已脱敏；resp 按字节脱敏 + 截断）。
fn debug_payload(
    req: &Option<(serde_json::Value, bool)>,
    resp_bytes: &[u8],
) -> Option<serde_json::Value> {
    let (req, req_trunc) = req.as_ref()?;
    let (resp, resp_trunc) =
        crate::logging::redact::redact_and_truncate(resp_bytes, crate::logging::DEBUG_MAX_BYTES);
    Some(serde_json::json!({
        "request": req.clone(),
        "response": resp,
        "truncated": *req_trunc || resp_trunc,
    }))
}

/// 鉴权后的失败统一记账（contract §12.5）。token 恒 None，protocol_out 未知 = protocol_in，
/// convert_mode 未决策 = "none"（已在模板初始化时置 "none"，此处兜底）。
fn log_fail(
    state: &AppState,
    tmpl: &LogEvent,
    start: Instant,
    status: u16,
    msg: &str,
    retry_count: i32,
) {
    let mut ev = tmpl.clone();
    ev.status = status as i32;
    ev.error = Some(truncate_msg(msg));
    ev.latency_ms = Some(start.elapsed().as_millis() as i32);
    ev.retry_count = retry_count;
    if ev.protocol_out.is_empty() {
        ev.protocol_out = ev.protocol_in.clone();
    }
    if ev.convert_mode.is_empty() {
        ev.convert_mode = "none".into();
    }
    state.log_sink.log(ev);
}

/// 非流式成功/转换失败结果（供打点使用）。
struct NonstreamOutcome {
    response: Response,
    /// 200 或 502
    status: u16,
    degraded: bool,
    error: Option<String>,
    /// 返回给客户端的最终 JSON（用于 debug_payload）
    final_json: Option<serde_json::Value>,
}

/// 非流式响应：透传改写回网关模型名；转换走 response_to_ir → response_from_ir。
/// 返回响应与打点所需的元数据（status/degraded/error/最终 JSON）。
fn nonstream_success(
    entry: Protocol,
    mode: Mode,
    json: serde_json::Value,
    gateway_model: &str,
    request_id: &str,
) -> NonstreamOutcome {
    match mode {
        Mode::Convert(t) => {
            let mut ctx = ConvCtx::new();
            match protocol::response_to_ir(t, &json, &mut ctx)
                .and_then(|ir| protocol::response_from_ir(entry, &ir, &mut ctx))
            {
                Ok(out) => {
                    // 回写网关模型名（与透传路径一致；转换响应会保留上游 model，review P2-6）
                    let mut out = out;
                    if let Some(obj) = out.as_object_mut() {
                        if obj.contains_key("model") {
                            obj.insert(
                                "model".into(),
                                serde_json::Value::String(gateway_model.to_string()),
                            );
                        }
                    }
                    NonstreamOutcome {
                        response: json_rsp(
                            StatusCode::OK,
                            request_id,
                            ctx.degraded_header().as_deref(),
                            out.clone(),
                        ),
                        status: 200,
                        degraded: !ctx.degraded.is_empty(),
                        error: None,
                        final_json: Some(out),
                    }
                }
                Err(e) => {
                    let body = error_from_ir(
                        entry,
                        &IrError {
                            status: 502,
                            message: format!("响应转换失败: {e}"),
                            ..Default::default()
                        },
                    );
                    NonstreamOutcome {
                        response: json_rsp(StatusCode::BAD_GATEWAY, request_id, None, body.clone()),
                        status: 502,
                        degraded: false,
                        error: Some(truncate_msg(&format!("响应转换失败: {e}"))),
                        final_json: Some(body),
                    }
                }
            }
        }
        _ => {
            let mut v = json;
            if let Some(obj) = v.as_object_mut() {
                if obj.contains_key("model") {
                    obj.insert(
                        "model".into(),
                        serde_json::Value::String(gateway_model.to_string()),
                    );
                }
            }
            NonstreamOutcome {
                response: json_rsp(StatusCode::OK, request_id, None, v.clone()),
                status: 200,
                degraded: false,
                error: None,
                final_json: Some(v),
            }
        }
    }
}

/// 全部候选失败时，把最后的错误翻译为入口协议错误体。
fn entry_error(entry: Protocol, fail: &Fail) -> (u16, serde_json::Value) {
    match fail {
        Fail::Upstream(e) => match e {
            UpstreamError::Status(s, body) => {
                let ir = serde_json::from_str::<serde_json::Value>(body)
                    .map(|j| error_to_ir(entry, *s, &j))
                    .unwrap_or_else(|_| IrError {
                        status: *s,
                        message: body.clone(),
                        ..Default::default()
                    });
                (*s, error_from_ir(entry, &ir))
            }
            other => {
                let ir = IrError {
                    status: 502,
                    message: other.to_string(),
                    ..Default::default()
                };
                (502, error_from_ir(entry, &ir))
            }
        },
        Fail::Convert(e) => {
            let ir = IrError {
                status: 502,
                message: e.to_string(),
                ..Default::default()
            };
            (502, error_from_ir(entry, &ir))
        }
    }
}

// ---------------------------------------------------------------------------
// 指标
// ---------------------------------------------------------------------------

fn metrics_labels(key: &ApiKeyRow, entry: Entry, upstream: &str) -> RequestLabels {
    RequestLabels {
        key_prefix: key.prefix.clone(),
        upstream: upstream.to_string(),
        protocol: entry.protocol().as_str().to_string(),
    }
}

fn record(
    metrics: &crate::metrics::Metrics,
    labels: &RequestLabels,
    start: Instant,
    is_error: bool,
    resp: Response,
) -> Response {
    metrics.gateway_requests.get_or_create(labels).inc();
    if is_error {
        metrics.gateway_errors.get_or_create(labels).inc();
    }
    metrics
        .gateway_latency
        .get_or_create(labels)
        .observe(start.elapsed().as_secs_f64());
    resp
}

// ---------------------------------------------------------------------------
// 主链路
// ---------------------------------------------------------------------------

/// 统一请求主链路。`gemini_action` 仅 Gemini 入口携带（原始路径片段）。
async fn run_gateway(
    state: Arc<AppState>,
    entry: Entry,
    headers: HeaderMap,
    body: Bytes,
    gemini_action: Option<String>,
) -> Response {
    // 1. request_id + 计时
    let request_id = Uuid::new_v4().to_string();
    let start = Instant::now();
    let entry_protocol = entry.protocol();

    // 2. Key 鉴权（复用 authenticate_gateway_key；媒体端点亦走此函数）
    let key = match authenticate_gateway_key(&state, &headers) {
        Ok(k) => k,
        Err(resp) => return *resp,
    };
    let snap = state.cache.snapshot();

    // 鉴权通过后创建记账事件模板（contract §12.1；鉴权前的 401 不记账，避免刷库）。
    // model/stream/params 在解析后填充；上游相关字段（upstream_id/protocol_out/convert_mode）按候选填充。
    let mut tmpl = LogEvent {
        request_id: request_id.clone(),
        ts: chrono::Utc::now(),
        key_id: Some(key.id),
        model: String::new(),
        requested_model: None,
        upstream_id: None,
        protocol_in: entry_protocol.as_str().to_string(),
        protocol_out: String::new(),
        convert_mode: "none".into(),
        stream: false,
        status: 0,
        error: None,
        prompt_tokens: None,
        completion_tokens: None,
        cache_write_tokens: None,
        cache_read_tokens: None,
        latency_ms: None,
        retry_count: 0,
        ttfb_ms: None,
        degraded: false,
        usage_raw: None,
        debug_payload: None,
        // M5 计价字段：网关主链路恒 None，由 writer 侧 insert_batch 计价回填。
        cost_cny: None,
        cost_usd: None,
        pricing_source: None,
        price_used: None,
        fx_snapshot: None,
        // M6 媒体字段：网关主链路（chat/responses/messages/gemini）恒 None，图片端点由 B 填充。
        images: None,
        image_size: None,
        video_seconds: None,
        video_resolution: None,
        video_task_type: None,
    };
    let debug_active = key.debug_active();
    // params_meta 在 body 解析后计算（早退路径不引用，故延迟初始化）。

    let mut debug_req: Option<(serde_json::Value, bool)> = None;

    // 预取热配置（避免跨 await 持有 ArcSwap guard）
    let gateway_cfg = state.hot.load().gateway.clone();
    let default_proxy_id = state.hot.load().proxy.default_proxy_id.clone();

    // 3. Gemini action 解析 + body 解析 + 模型提取
    let (gemini_model, gemini_stream) = match entry {
        Entry::Gemini => {
            let action = gemini_action.unwrap_or_default();
            match parse_gemini_action(&action) {
                Some((m, s)) => (Some(m), Some(s)),
                None => {
                    log_fail(&state, &tmpl, start, 400, "无效 Gemini action（应为 model:generateContent 或 model:streamGenerateContent）", 0);
                    return error_resp(entry_protocol, &request_id, 400, "无效 Gemini action（应为 model:generateContent 或 model:streamGenerateContent）");
                }
            }
        }
        _ => (None, None),
    };

    let body_value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            log_fail(&state, &tmpl, start, 400, "请求体不是合法 JSON", 0);
            return error_resp(entry_protocol, &request_id, 400, "请求体不是合法 JSON");
        }
    };

    let model = match extract_model(entry, &body_value, gemini_model.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            let msg = e.to_string();
            log_fail(&state, &tmpl, start, e.status().as_u16(), &msg, 0);
            return error_resp(entry_protocol, &request_id, e.status().as_u16(), &msg);
        }
    };
    // 别名解析（M10.1）：入口模型命中启用的别名 → 替换为实际模型（单跳，不链式）。
    // 白名单/路由匹配/计价均按解析后的实际模型；requested_model 保留客户端原始值入账。
    let (model, requested) = resolve_alias(&snap, &model);
    tmpl.requested_model = requested;
    // 模型/stream 确定后填充模板与参数元数据
    tmpl.model = model.clone();
    tmpl.stream = is_stream(&body_value, gemini_stream);
    let stream = tmpl.stream;
    let max_tokens = extract_max_tokens(entry_protocol, &body_value);
    let params_meta: serde_json::Value =
        crate::upstream::usage::request_params_meta(entry_protocol, &body_value);
    tmpl.usage_raw = Some(serde_json::json!({ "params": params_meta.clone() }));
    if debug_active {
        debug_req = Some(crate::logging::redact::redact_and_truncate(
            &body,
            crate::logging::DEBUG_MAX_BYTES,
        ));
    }

    // 4. 模型白名单
    if !key.model_allowed(&model) {
        log_fail(&state, &tmpl, start, 403, "禁止访问", 0);
        return error_resp(entry_protocol, &request_id, 403, "禁止访问");
    }

    // 5. 限流 + TPM 预检 + 配额
    if let Err(e) = state
        .limiter
        .check_rpm(&key, gateway_cfg.default_rate_limit_rpm)
    {
        let msg = e.to_string();
        log_fail(&state, &tmpl, start, e.status().as_u16(), &msg, 0);
        // RPM 429 附 Retry-After（滑窗 60s；M10.5 标准客户端兼容）
        let mut resp = error_resp(entry_protocol, &request_id, e.status().as_u16(), &msg);
        if e.status().as_u16() == 429 {
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
        }
        return resp;
    }
    if let Err(e) = limit::tpm_precheck(&key, max_tokens) {
        let msg = e.to_string();
        log_fail(&state, &tmpl, start, e.status().as_u16(), &msg, 0);
        return error_resp(entry_protocol, &request_id, e.status().as_u16(), &msg);
    }
    // M10.5：限流通过后读取窗口余量，成功响应附 X-RateLimit-* 头
    let rl_status = state
        .limiter
        .rpm_status(&key, gateway_cfg.default_rate_limit_rpm);
    {
        let exceeded = match state
            .quota_cache
            .get(key.id, gateway_cfg.quota_check_cache_secs)
        {
            Some(v) => v,
            None => match limit::check_quota(
                &state.db,
                &key,
                &gateway_cfg.billing_timezone,
                &gateway_cfg.quota_exceed_action,
            )
            .await
            {
                Ok(()) => {
                    state.quota_cache.put(key.id, false);
                    false
                }
                Err(ApiError::RateLimited) => {
                    state.quota_cache.put(key.id, true);
                    true
                }
                // 查询失败（DB 抖动等）：block 模式要求 fail-closed（发布审阅 M5），
                // 不缓存否定结果，直接 503。
                Err(e) => {
                    log_fail(&state, &tmpl, start, 503, &format!("配额检查失败: {e}"), 0);
                    return error_resp(
                        entry_protocol,
                        &request_id,
                        503,
                        "配额检查失败，请稍后重试",
                    );
                }
            },
        };
        if exceeded {
            log_fail(&state, &tmpl, start, 429, "请求过于频繁，请稍后再试", 0);
            return error_resp(entry_protocol, &request_id, 429, "请求过于频繁，请稍后再试");
        }
    }

    // 6. 路由决策
    let matched = routing::match_routes(&model, &snap.routes);
    if matched.is_empty() {
        let lbl = metrics_labels(&key, entry, "");
        log_fail(&state, &tmpl, start, 404, "模型未配置路由", 0);
        return record(
            &state.metrics,
            &lbl,
            start,
            true,
            error_resp(entry_protocol, &request_id, 404, "模型未配置路由"),
        );
    }
    // M10.3 能力过滤：显式标记不支持所需能力（stream/tools/vision）的上游跳过，
    // 全部被滤掉时错误信息附原因（后台可在日志看到 detail）。
    let req_caps = required_caps(&body_value, stream);
    let mut cap_filtered: Vec<String> = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    for route in matched {
        if let Some(up) = snap.upstreams.get(&route.upstream_id).cloned() {
            let missing = up.capabilities().missing(req_caps);
            if !missing.is_empty() {
                cap_filtered.push(format!("{} 缺 {}", up.name, missing.join("/")));
                continue;
            }
            if up.enabled && state.breaker.allow(&up) {
                candidates.push(Candidate {
                    route,
                    upstream: up,
                });
            }
        }
    }
    if candidates.is_empty() {
        let msg = if cap_filtered.is_empty() {
            "无可用上游".to_string()
        } else {
            format!("无可用上游（能力不足：{}）", cap_filtered.join("；"))
        };
        let lbl = metrics_labels(&key, entry, "");
        log_fail(&state, &tmpl, start, 503, &msg, 0);
        return record(
            &state.metrics,
            &lbl,
            start,
            true,
            error_resp(entry_protocol, &request_id, 503, &msg),
        );
    }
    let attempts: Vec<Attempt> = order_candidates(candidates)
        .into_iter()
        .filter_map(|c| {
            decide_mode(entry_protocol, &c.upstream).map(|mode| Attempt { candidate: c, mode })
        })
        .collect();
    if attempts.is_empty() {
        let lbl = metrics_labels(&key, entry, "");
        log_fail(&state, &tmpl, start, 503, "无可用上游", 0);
        return record(
            &state.metrics,
            &lbl,
            start,
            true,
            error_resp(entry_protocol, &request_id, 503, "无可用上游"),
        );
    }

    // M11.3 Key 级粘性路由：pinned 上游在最优优先级组内则提到首位；
    // 不在（禁用/熔断/缺能力/组外）则忽略，成功路径会更新 pin。
    let mut attempts = attempts;
    if gateway_cfg.sticky_routing {
        if let Some(pin) = state.sticky.get(key.id, &model) {
            reorder_pinned_first(&mut attempts, pin);
        }
    }

    // 6b. 幂等键（M10.2）：仅非流式；登记点在路由决策之后——限流/无路由等未触达上游的
    // 失败不占键。重放直接返回缓存响应（不重复计费、不写日志、不计 metrics）。
    let mut idem_rec: Option<Uuid> = None;
    if let Some(ik_raw) = headers
        .get(crate::idempotency::IDEM_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        let ik_trim = ik_raw.trim();
        if !ik_trim.is_empty() {
            if stream {
                log_fail(
                    &state,
                    &tmpl,
                    start,
                    400,
                    "流式请求不支持 Idempotency-Key",
                    0,
                );
                return error_resp(
                    entry_protocol,
                    &request_id,
                    400,
                    "流式请求不支持 Idempotency-Key",
                );
            }
            let ik = match crate::idempotency::validate_key(ik_trim) {
                Ok(k) => k,
                Err(msg) => {
                    log_fail(&state, &tmpl, start, 400, msg, 0);
                    return error_resp(entry_protocol, &request_id, 400, msg);
                }
            };
            let fp = crate::idempotency::fingerprint(&body_value);
            match crate::idempotency::begin(&state.db, key.id, ik, &fp).await {
                Ok(crate::idempotency::Begin::Proceed(rid)) => idem_rec = Some(rid),
                Ok(crate::idempotency::Begin::Replay { status, response }) => {
                    let code = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK);
                    let mut resp = json_rsp(code, &request_id, None, response);
                    resp.headers_mut().insert(
                        crate::idempotency::REPLAYED_HEADER,
                        HeaderValue::from_static("true"),
                    );
                    return resp;
                }
                Ok(crate::idempotency::Begin::Conflict(msg)) => {
                    log_fail(&state, &tmpl, start, 409, msg, 0);
                    return error_resp(entry_protocol, &request_id, 409, msg);
                }
                Err(e) => {
                    let msg = format!("幂等键存储失败: {e}");
                    log_fail(&state, &tmpl, start, 500, &msg, 0);
                    return error_resp(entry_protocol, &request_id, 500, "幂等键存储失败");
                }
            }
        }
    }

    // 7-10. 协议决策 → 请求构建 → 执行/重试/故障转移 → 响应
    let mut last_err: Option<Fail> = None;
    let mut last_upstream: Option<String> = None;
    // retry_count 定义：本候选内重试次数 idx + 之前彻底失败的候选数。
    let mut failed_candidates: u32 = 0;
    let mut last_idx: u32 = 0;
    let mut last_protocol_out: Option<String> = None;
    let mut last_convert_mode: Option<String> = None;
    // URL 图片下载缓存：跨候选共享，同 URL 只下载一次（含失败结果）。
    let mut img_cache = crate::media::resolve::ResolveCache::new();
    'outer: for attempt in &attempts {
        let route = &attempt.candidate.route;
        let upstream = &attempt.candidate.upstream;
        let mode = attempt.mode;
        let up_model: String = route
            .override_model
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| model.clone());
        let overrides = upstream.overrides();

        let outbound = match build_outbound_any(
            &state,
            entry_protocol,
            mode,
            &body_value,
            stream,
            &up_model,
            upstream,
            &mut img_cache,
        )
        .await
        {
            Ok(o) => o,
            Err(cvt) => {
                // 注：PLAN §3.2 的 PassthroughFallback 不可达已移除（发布审阅 M3）——
                // Mode::Convert 蕴含入口协议不在该上游支持列表，「转换失败回退透传」
                // 的前提不成立；转换失败只能转移到下一候选。
                last_upstream = Some(upstream.name.clone());
                last_err = Some(Fail::Convert(cvt));
                failed_candidates += 1;
                last_idx = 0;
                continue 'outer;
            }
        };
        // 已决策出上游协议：记账时回填 protocol_out / convert_mode。
        last_protocol_out = Some(outbound.protocol.as_str().to_string());
        last_convert_mode = Some(mode_str(outbound.mode).to_string());

        let url = format!(
            "{}{}",
            upstream.base_url.trim_end_matches('/'),
            upstream::endpoint_path(outbound.protocol, &up_model, stream)
        );
        let mut req_headers = reqwest::header::HeaderMap::new();
        req_headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        upstream::apply_auth(
            &mut req_headers,
            outbound.protocol,
            upstream.api_key_plain.as_deref(),
        );
        upstream::apply_header_overrides(&mut req_headers, overrides.as_ref());
        // no_proxy 合并规则（PLAN §5.9）：目标 URL 命中全局 proxy.no_proxy 或所选代理
        // 自身 no_proxy 任一列表 → 直连池。
        let client = {
            let mut lists: Vec<Vec<String>> = vec![state.hot.load().proxy.no_proxy.clone()];
            let eff_proxy_id = if upstream.use_proxy {
                upstream
                    .proxy_id
                    .or_else(|| uuid::Uuid::parse_str(&default_proxy_id).ok())
            } else {
                None
            };
            if let Some(pid) = eff_proxy_id {
                if let Some(p) = snap.proxies.get(&pid) {
                    lists.push(p.no_proxy.clone());
                }
            }
            if upstream.use_proxy && upstream::no_proxy_match(&lists, &url) {
                state.client_pools.direct_client()
            } else {
                state
                    .client_pools
                    .client_for(upstream, &snap, &default_proxy_id)
            }
        };

        let lock = route.lock_upstream;
        let retry_limit = if lock { 0 } else { route.retries.max(0) as u32 };
        // 流式总超时取 gateway.stream_timeout_secs（默认 600s，0=不设）；
        // 非流式仍用 per-upstream timeout_ms（review P1-#7：此前流式误用 timeout_ms 且该配置无人引用）。
        let stream_timeout_ms: i32 = {
            let secs = state.hot.load().gateway.stream_timeout_secs;
            if secs == 0 {
                0
            } else {
                (secs.saturating_mul(1000)).min(i32::MAX as u64) as i32
            }
        };
        let mut idx: u32 = 0;
        loop {
            let exec_res: Result<ExecOutcome, UpstreamError> = if stream {
                upstream::execute_stream(
                    &client,
                    &url,
                    req_headers.clone(),
                    &outbound.body,
                    stream_timeout_ms,
                )
                .await
                .map(ExecOutcome::Stream)
            } else {
                upstream::execute_nonstream(
                    &client,
                    &url,
                    req_headers.clone(),
                    &outbound.body,
                    upstream.timeout_ms,
                )
                .await
                .map(ExecOutcome::Json)
            };

            match exec_res {
                Ok(ExecOutcome::Json(json)) => {
                    state.breaker.on_success(&state.db, upstream).await;
                    // M11.3：成功即更新粘性 pin（仅开启时；pin 只在成功路径写入）
                    if gateway_cfg.sticky_routing {
                        state.sticky.pin(key.id, &model, upstream.id);
                    }
                    let lbl = metrics_labels(&key, entry, &upstream.name);
                    // 用上游原始 JSON 提取 usage（§12.2）；转换前后 usage 语义等价，取原始值最准。
                    let usage =
                        crate::upstream::usage::extract_json_usage(outbound.protocol, &json);
                    let outcome =
                        nonstream_success(entry_protocol, outbound.mode, json, &model, &request_id);
                    let retry_count = (idx + failed_candidates) as i32;
                    let mut ev = tmpl.clone();
                    ev.upstream_id = Some(upstream.id);
                    ev.protocol_out = outbound.protocol.as_str().to_string();
                    ev.convert_mode = mode_str(outbound.mode).to_string();
                    ev.latency_ms = Some(start.elapsed().as_millis() as i32);
                    ev.retry_count = retry_count;
                    ev.degraded = outcome.degraded;
                    if outcome.status == 200 {
                        ev.status = 200;
                        ev.prompt_tokens = usage.prompt_tokens;
                        ev.completion_tokens = usage.completion_tokens;
                        ev.cache_write_tokens = usage.cache_write_tokens;
                        ev.cache_read_tokens = usage.cache_read_tokens;
                        ev.usage_raw = Some(serde_json::json!({
                            "params": params_meta.clone(),
                            "usage": usage.raw.clone().unwrap_or(serde_json::Value::Null),
                        }));
                        let total = usage
                            .prompt_tokens
                            .unwrap_or(0)
                            .saturating_add(usage.completion_tokens.unwrap_or(0));
                        if total > 0 {
                            state
                                .metrics
                                .gateway_tokens
                                .get_or_create(&lbl)
                                .inc_by(total.max(0) as u64);
                        }
                        if let Some(payload) = debug_payload(
                            &debug_req,
                            &serde_json::to_vec(
                                outcome
                                    .final_json
                                    .as_ref()
                                    .unwrap_or(&serde_json::Value::Null),
                            )
                            .unwrap_or_default(),
                        ) {
                            ev.debug_payload = Some(payload);
                        }
                        // 幂等登记完成（M10.2）：缓存响应体；失败仅告警不阻断响应
                        if let (Some(rid), Some(fj)) = (idem_rec, outcome.final_json.as_ref()) {
                            if let Err(e) =
                                crate::idempotency::complete(&state.db, rid, 200, fj, None).await
                            {
                                tracing::warn!("幂等键结果写入失败: {e}");
                            }
                        }
                        state.log_sink.log(ev);
                        let mut resp = outcome.response;
                        add_ratelimit_headers(&mut resp, rl_status);
                        return record(&state.metrics, &lbl, start, false, resp);
                    } else {
                        // 转换失败（502）：记为失败事件（token 恒 None）。
                        ev.status = outcome.status as i32;
                        ev.error = outcome.error.clone();
                        if let Some(rid) = idem_rec {
                            let _ = crate::idempotency::fail(&state.db, rid).await;
                        }
                        if let Some(payload) = debug_payload(
                            &debug_req,
                            &serde_json::to_vec(
                                outcome
                                    .final_json
                                    .as_ref()
                                    .unwrap_or(&serde_json::Value::Null),
                            )
                            .unwrap_or_default(),
                        ) {
                            ev.debug_payload = Some(payload);
                        }
                        state.log_sink.log(ev);
                        return record(&state.metrics, &lbl, start, true, outcome.response);
                    }
                }
                Ok(ExecOutcome::Stream(resp)) => {
                    state.breaker.on_success(&state.db, upstream).await;
                    // M11.3：成功即更新粘性 pin（仅开启时；pin 只在成功路径写入）
                    if gateway_cfg.sticky_routing {
                        state.sticky.pin(key.id, &model, upstream.id);
                    }
                    let lbl = metrics_labels(&key, entry, &upstream.name);
                    let sbody = if matches!(outbound.mode, Mode::Convert(_)) {
                        Body::from_stream(upstream::sse_convert_stream(
                            resp,
                            outbound.protocol,
                            entry_protocol,
                            model.clone(),
                        ))
                    } else {
                        Body::from_stream(upstream::sse_passthrough_stream(resp, model.clone()))
                    };
                    let mut ev = tmpl.clone();
                    ev.upstream_id = Some(upstream.id);
                    ev.protocol_out = outbound.protocol.as_str().to_string();
                    ev.convert_mode = mode_str(outbound.mode).to_string();
                    ev.degraded = false; // 流式转换的降级项在流内部处理，无法在此捕获，记为 false
                    let collect = Arc::new(Mutex::new(StreamCollect {
                        text: String::new(),
                        ttfb_ms: None,
                    }));
                    let retry_count = (idx + failed_candidates) as i32;
                    let teed = tee_log_stream(
                        sbody,
                        state.clone(),
                        ev,
                        collect,
                        start,
                        entry_protocol,
                        lbl.clone(),
                        params_meta.clone(),
                        debug_req.clone(),
                        retry_count,
                    );
                    let mut resp = stream_rsp(&request_id, teed);
                    add_ratelimit_headers(&mut resp, rl_status);
                    return record(&state.metrics, &lbl, start, false, resp);
                }
                Err(e) => {
                    let retryable = e.retryable(&route.retry_status_codes);
                    if retryable {
                        state.breaker.on_failure(&state.db, upstream).await;
                    }
                    // 客户端成因的 4xx（400/404/422 等）：同一 body 换候选必然同样失败
                    // 且每个候选都可能计费——不故障转移，直接返回当前错误（发布审阅 L12）。
                    let no_failover = e.client_causal();
                    last_upstream = Some(upstream.name.clone());
                    last_err = Some(Fail::Upstream(e));
                    let can_retry = retryable && !lock && idx < retry_limit;
                    if can_retry {
                        // 指数退避 100ms×2^n（n = 已尝试次数）
                        tokio::time::sleep(Duration::from_millis(100u64 << idx.min(30))).await;
                        idx += 1;
                        continue;
                    }
                    failed_candidates += 1;
                    last_idx = idx;
                    if lock || no_failover {
                        break 'outer;
                    }
                    break; // 转移到下一候选
                }
            }
        }
    }

    // 全部候选失败
    let lbl = metrics_labels(&key, entry, last_upstream.as_deref().unwrap_or(""));
    let (status, msg, jbody) = match &last_err {
        Some(f) => {
            let (s, body) = entry_error(entry_protocol, f);
            (s, fail_message(f), body)
        }
        None => (
            502,
            "无可用上游".to_string(),
            error_from_ir(
                entry_protocol,
                &IrError {
                    status: 502,
                    message: "无可用上游".into(),
                    ..Default::default()
                },
            ),
        ),
    };
    // 失败终点：protocol_out 已知则填，convert_mode 已决策则填，否则兜底（log_fail 内完成）。
    // retry_count = 末候选内重试次数 last_idx + 之前彻底失败的候选数（failed_candidates 包含末候选，故减 1）。
    let mut ev = tmpl.clone();
    ev.protocol_out = last_protocol_out.unwrap_or_else(|| entry_protocol.as_str().to_string());
    ev.convert_mode = last_convert_mode.unwrap_or_else(|| "none".into());
    let retry_count = (last_idx + failed_candidates.saturating_sub(1)) as i32;
    log_fail(&state, &ev, start, status, &msg, retry_count);
    if let Some(rid) = idem_rec {
        let _ = crate::idempotency::fail(&state.db, rid).await;
    }
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let resp = json_rsp(code, &request_id, None, jbody);
    record(&state.metrics, &lbl, start, true, resp)
}

// ---------------------------------------------------------------------------
// 入口 handlers（薄封装）
// ---------------------------------------------------------------------------

async fn openai_chat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    run_gateway(state, Entry::OpenaiChat, headers, body, None).await
}

async fn openai_responses(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    run_gateway(state, Entry::OpenaiResponses, headers, body, None).await
}

async fn anthropic(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    run_gateway(state, Entry::Anthropic, headers, body, None).await
}

async fn gemini(
    State(state): State<Arc<AppState>>,
    Path(action): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    run_gateway(state, Entry::Gemini, headers, body, Some(action)).await
}

// ---------------------------------------------------------------------------
// 图片通道（契约 m6-media §4）：图片生成 + 视频占位 + 图片任务查询转发
// ---------------------------------------------------------------------------

/// 图片 API 形状 → 上游协议名（用于 protocol_out / 日志）。
fn image_api_name(api: ImageApi) -> &'static str {
    match api {
        ImageApi::Openai => "images_openai",
        ImageApi::Gemini => "images_gemini",
        ImageApi::DashscopeSync => "images_dashscope_sync",
        ImageApi::DashscopeAsync => "images_dashscope_async",
    }
}

/// 图片请求的指标标签：protocol 维度用 "openai_image"（无 token 语义，不打 gateway_tokens）。
fn image_metrics_labels(key: &ApiKeyRow, upstream: &str) -> RequestLabels {
    RequestLabels {
        key_prefix: key.prefix.clone(),
        upstream: upstream.to_string(),
        protocol: "openai_image".to_string(),
    }
}

/// POST /v1/images/generations：图片生成入口（统一 OpenAI 语义，契约 m6 §4）。
///
/// 与 run_gateway 同款鉴权/限流/候选循环/打点，但差异点：
/// - 无 token、无流式（恒非流式）；
/// - 候选须 `media::image_api_of(up).is_some()`；
/// - tpm_precheck 跳过（图片无 max_tokens 语义）；
/// - 响应/记账含媒体维度（images/image_size）。
async fn images_generations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = Uuid::new_v4().to_string();
    let start = Instant::now();

    // 1. Key 鉴权（媒体端点走网关 Key，同主链路）
    let key = match authenticate_gateway_key(&state, &headers) {
        Ok(k) => k,
        Err(resp) => return *resp,
    };
    let snap = state.cache.snapshot();
    let debug_active = key.debug_active();

    // 2. 记账事件模板（鉴权通过后建立；媒体字段在成功路径回填）
    let mut tmpl = LogEvent {
        request_id: request_id.clone(),
        ts: chrono::Utc::now(),
        key_id: Some(key.id),
        model: String::new(),
        requested_model: None,
        upstream_id: None,
        protocol_in: "openai_image".to_string(),
        protocol_out: String::new(),
        convert_mode: "none".into(),
        stream: false,
        status: 0,
        error: None,
        prompt_tokens: None,
        completion_tokens: None,
        cache_write_tokens: None,
        cache_read_tokens: None,
        latency_ms: None,
        retry_count: 0,
        ttfb_ms: None,
        degraded: false,
        usage_raw: None,
        debug_payload: None,
        cost_cny: None,
        cost_usd: None,
        pricing_source: None,
        price_used: None,
        fx_snapshot: None,
        images: None,
        image_size: None,
        video_seconds: None,
        video_resolution: None,
        video_task_type: None,
    };
    let mut debug_req: Option<(serde_json::Value, bool)> = None;

    // 预取热配置（避免跨 await 持有 ArcSwap guard）
    let gateway_cfg = state.hot.load().gateway.clone();
    let default_proxy_id = state.hot.load().proxy.default_proxy_id.clone();

    // 3. body 解析 + 统一图片请求 + 模型提取
    let body_value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            log_fail(&state, &tmpl, start, 400, "请求体不是合法 JSON", 0);
            return error_resp(
                Protocol::OpenaiChat,
                &request_id,
                400,
                "请求体不是合法 JSON",
            );
        }
    };
    let ir = match media::parse_image_request(&body_value) {
        Ok(r) => r,
        Err(e) => {
            let msg = e.to_string();
            log_fail(&state, &tmpl, start, e.status().as_u16(), &msg, 0);
            return error_resp(Protocol::OpenaiChat, &request_id, e.status().as_u16(), &msg);
        }
    };
    let model = match body_value["model"].as_str() {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => {
            log_fail(&state, &tmpl, start, 400, "model 缺失", 0);
            return error_resp(Protocol::OpenaiChat, &request_id, 400, "model 缺失");
        }
    };
    // 别名解析（M10.1）：同主链路——白名单/路由/计价按解析后的实际模型
    let (model, requested) = resolve_alias(&snap, &model);
    tmpl.requested_model = requested;
    tmpl.model = model.clone();
    if debug_active {
        debug_req = Some(crate::logging::redact::redact_and_truncate(
            &body,
            crate::logging::DEBUG_MAX_BYTES,
        ));
    }

    // 4. 模型白名单 + RPM + 配额（跳过 tpm_precheck——图片无 max_tokens 语义）
    if !key.model_allowed(&model) {
        log_fail(&state, &tmpl, start, 403, "禁止访问", 0);
        return error_resp(Protocol::OpenaiChat, &request_id, 403, "禁止访问");
    }
    if let Err(e) = state
        .limiter
        .check_rpm(&key, gateway_cfg.default_rate_limit_rpm)
    {
        let msg = e.to_string();
        log_fail(&state, &tmpl, start, e.status().as_u16(), &msg, 0);
        // RPM 429 附 Retry-After（滑窗 60s；M10.5）
        let mut resp = error_resp(Protocol::OpenaiChat, &request_id, e.status().as_u16(), &msg);
        if e.status().as_u16() == 429 {
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
        }
        return resp;
    }
    // M10.5：限流通过后读取窗口余量，成功响应附 X-RateLimit-* 头
    let rl_status = state
        .limiter
        .rpm_status(&key, gateway_cfg.default_rate_limit_rpm);
    {
        let exceeded = match state
            .quota_cache
            .get(key.id, gateway_cfg.quota_check_cache_secs)
        {
            Some(v) => v,
            None => match limit::check_quota(
                &state.db,
                &key,
                &gateway_cfg.billing_timezone,
                &gateway_cfg.quota_exceed_action,
            )
            .await
            {
                Ok(()) => {
                    state.quota_cache.put(key.id, false);
                    false
                }
                Err(ApiError::RateLimited) => {
                    state.quota_cache.put(key.id, true);
                    true
                }
                // 查询失败（DB 抖动等）：block 模式要求 fail-closed（发布审阅 M5），
                // 不缓存否定结果，直接 503。
                Err(e) => {
                    log_fail(&state, &tmpl, start, 503, &format!("配额检查失败: {e}"), 0);
                    return error_resp(
                        Protocol::OpenaiChat,
                        &request_id,
                        503,
                        "配额检查失败，请稍后重试",
                    );
                }
            },
        };
        if exceeded {
            log_fail(&state, &tmpl, start, 429, "请求过于频繁，请稍后再试", 0);
            return error_resp(
                Protocol::OpenaiChat,
                &request_id,
                429,
                "请求过于频繁，请稍后再试",
            );
        }
    }

    // 5. 路由匹配 + 候选收集（候选须具备 image_api 形状，否则跳过）
    let matched = routing::match_routes(&model, &snap.routes);
    if matched.is_empty() {
        let lbl = image_metrics_labels(&key, "");
        log_fail(&state, &tmpl, start, 404, "模型未配置路由", 0);
        return record(
            &state.metrics,
            &lbl,
            start,
            true,
            error_resp(Protocol::OpenaiChat, &request_id, 404, "模型未配置路由"),
        );
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    for route in matched {
        if let Some(up) = snap.upstreams.get(&route.upstream_id).cloned() {
            if up.enabled && media::image_api_of(&up).is_some() && state.breaker.allow(&up) {
                candidates.push(Candidate {
                    route,
                    upstream: up,
                });
            }
        }
    }
    if candidates.is_empty() {
        let lbl = image_metrics_labels(&key, "");
        log_fail(&state, &tmpl, start, 503, "无可用上游", 0);
        return record(
            &state.metrics,
            &lbl,
            start,
            true,
            error_resp(Protocol::OpenaiChat, &request_id, 503, "无可用上游"),
        );
    }
    let ordered = order_candidates(candidates);

    // 5b. 幂等键（M10.2）：图片恒非流式；登记点在路由决策之后（无路由/限流不占键）。
    // 异步任务完成时缓存 {"task_id":...}，重放返回同一任务。
    let mut idem_rec: Option<Uuid> = None;
    if let Some(ik_raw) = headers
        .get(crate::idempotency::IDEM_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        let ik_trim = ik_raw.trim();
        if !ik_trim.is_empty() {
            let ik = match crate::idempotency::validate_key(ik_trim) {
                Ok(k) => k,
                Err(msg) => {
                    log_fail(&state, &tmpl, start, 400, msg, 0);
                    return error_resp(Protocol::OpenaiChat, &request_id, 400, msg);
                }
            };
            let fp = crate::idempotency::fingerprint(&body_value);
            match crate::idempotency::begin(&state.db, key.id, ik, &fp).await {
                Ok(crate::idempotency::Begin::Proceed(rid)) => idem_rec = Some(rid),
                Ok(crate::idempotency::Begin::Replay { status, response }) => {
                    let code = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK);
                    let mut resp = json_rsp(code, &request_id, None, response);
                    resp.headers_mut().insert(
                        crate::idempotency::REPLAYED_HEADER,
                        HeaderValue::from_static("true"),
                    );
                    return resp;
                }
                Ok(crate::idempotency::Begin::Conflict(msg)) => {
                    log_fail(&state, &tmpl, start, 409, msg, 0);
                    return error_resp(Protocol::OpenaiChat, &request_id, 409, msg);
                }
                Err(e) => {
                    let msg = format!("幂等键存储失败: {e}");
                    log_fail(&state, &tmpl, start, 500, &msg, 0);
                    return error_resp(Protocol::OpenaiChat, &request_id, 500, "幂等键存储失败");
                }
            }
        }
    }

    // 6-7. 执行/重试/故障转移 → 响应（图片恒非流式）
    let mut last_err: Option<UpstreamError> = None;
    let mut last_upstream: Option<String> = None;
    let mut last_idx: u32 = 0;
    let mut failed_candidates: u32 = 0;
    let mut last_protocol_out: Option<String> = None;
    let mut last_convert_mode: Option<String> = None;
    'outer: for cand in &ordered {
        let route = &cand.route;
        let upstream = &cand.upstream;
        // 候选收集时已核实 image_api_of 为 Some，此处再取避免 unwrap
        let Some(image_api) = media::image_api_of(upstream) else {
            continue 'outer;
        };
        let up_model = route
            .override_model
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| model.clone());
        let overrides = upstream.overrides();

        // 构建上游请求：url_path + body（model 已用 override_model 替换）
        let (path, mut req_body) = media::build_image_request(image_api, &up_model, &ir);
        // 请求体 add/set 覆盖（不重写 model——Gemini 的 model 在 URL 中，OpenAI 已内建于 body）
        upstream::rewrite_body(&mut req_body, None, overrides.as_ref());
        let url = format!(
            "{}{}",
            crate::presets::media_base_url(upstream).trim_end_matches('/'),
            path
        );

        // 请求头：content-type + 鉴权 + 异步头 + 覆盖
        let mut req_headers = reqwest::header::HeaderMap::new();
        req_headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        match image_api {
            ImageApi::Openai | ImageApi::DashscopeSync | ImageApi::DashscopeAsync => {
                // OpenAI / 阿里系鉴权：Authorization Bearer（apply_auth 的 OpenaiChat 形态即 Bearer）
                upstream::apply_auth(
                    &mut req_headers,
                    Protocol::OpenaiChat,
                    upstream.api_key_plain.as_deref(),
                );
            }
            ImageApi::Gemini => {
                // Gemini 鉴权：x-goog-api-key
                upstream::apply_auth(
                    &mut req_headers,
                    Protocol::Gemini,
                    upstream.api_key_plain.as_deref(),
                );
            }
        }
        if media::needs_async_header(image_api) {
            req_headers.insert("x-dashscope-async", "enable".parse().unwrap());
        }
        upstream::apply_header_overrides(&mut req_headers, overrides.as_ref());

        // 客户端：no_proxy 并集（全局 + 所选代理）+ client_for（同 run_gateway 写法）
        let client = {
            let mut lists: Vec<Vec<String>> = vec![state.hot.load().proxy.no_proxy.clone()];
            let eff_proxy_id = if upstream.use_proxy {
                upstream
                    .proxy_id
                    .or_else(|| uuid::Uuid::parse_str(&default_proxy_id).ok())
            } else {
                None
            };
            if let Some(pid) = eff_proxy_id {
                if let Some(p) = snap.proxies.get(&pid) {
                    lists.push(p.no_proxy.clone());
                }
            }
            if upstream.use_proxy && upstream::no_proxy_match(&lists, &url) {
                state.client_pools.direct_client()
            } else {
                state
                    .client_pools
                    .client_for(upstream, &snap, &default_proxy_id)
            }
        };

        // 记录候选对应的协议/转换模式（供失败兜底记账回填）
        last_protocol_out = Some(image_api_name(image_api).to_string());
        last_convert_mode = Some(
            if image_api == ImageApi::Openai {
                "media_passthrough"
            } else {
                "media_adapt"
            }
            .into(),
        );

        let lock = route.lock_upstream;
        let retry_limit = if lock { 0 } else { route.retries.max(0) as u32 };
        let mut idx: u32 = 0;
        loop {
            let exec = upstream::execute_nonstream(
                &client,
                &url,
                req_headers.clone(),
                &req_body,
                upstream.timeout_ms,
            )
            .await;
            match exec {
                Ok(json) => {
                    state.breaker.on_success(&state.db, upstream).await;
                    match media::parse_image_response(image_api, ir.size.as_deref(), &json) {
                        Ok(ImageOutcome::Created {
                            images,
                            image_count,
                            image_size,
                            ..
                        }) => {
                            // M11.1 受控下载：media_download.enabled 时把 URL 图片转 b64_json
                            // 回填（防 CDN URL 过期）；失败保留原 URL 不阻断响应。
                            let mut images = images;
                            if state.hot.load().media_download.enabled {
                                for img in images.iter_mut() {
                                    if img.b64_json.is_none() {
                                        if let Some(u) = img.url.clone() {
                                            match media::download::download_b64(&state, &u).await {
                                                Ok((b64, _mime)) => img.b64_json = Some(b64),
                                                Err(e) => {
                                                    tracing::warn!(
                                                        "图片受控下载失败（保留 URL）: {e}"
                                                    )
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // 同步创建 → 200 {created, data, usage:{image_count}}
                            let data: Vec<serde_json::Value> = images
                                .iter()
                                .map(|img| {
                                    let mut m = serde_json::Map::new();
                                    if let Some(u) = &img.url {
                                        m.insert(
                                            "url".to_string(),
                                            serde_json::Value::String(u.clone()),
                                        );
                                    }
                                    if let Some(b) = &img.b64_json {
                                        m.insert(
                                            "b64_json".to_string(),
                                            serde_json::Value::String(b.clone()),
                                        );
                                    }
                                    serde_json::Value::Object(m)
                                })
                                .collect();
                            let body_json = serde_json::json!({
                                "created": chrono::Utc::now().timestamp(),
                                "data": data,
                                "usage": { "image_count": image_count },
                            });
                            let mut ev = tmpl.clone();
                            ev.upstream_id = Some(upstream.id);
                            ev.protocol_out = image_api_name(image_api).to_string();
                            ev.convert_mode = if image_api == ImageApi::Openai {
                                "media_passthrough"
                            } else {
                                "media_adapt"
                            }
                            .into();
                            ev.latency_ms = Some(start.elapsed().as_millis() as i32);
                            ev.retry_count = (idx + failed_candidates) as i32;
                            ev.status = 200;
                            ev.images = Some(image_count);
                            ev.image_size = image_size.clone();
                            if let Some(payload) = debug_payload(
                                &debug_req,
                                &serde_json::to_vec(&body_json).unwrap_or_default(),
                            ) {
                                ev.debug_payload = Some(payload);
                            }
                            // 幂等登记完成（M10.2）：缓存同步图片响应体
                            if let Some(rid) = idem_rec {
                                if let Err(e) = crate::idempotency::complete(
                                    &state.db, rid, 200, &body_json, None,
                                )
                                .await
                                {
                                    tracing::warn!("幂等键结果写入失败: {e}");
                                }
                            }
                            state.log_sink.log(ev);
                            let lbl = image_metrics_labels(&key, &upstream.name);
                            let mut resp = json_rsp(StatusCode::OK, &request_id, None, body_json);
                            add_ratelimit_headers(&mut resp, rl_status);
                            return record(&state.metrics, &lbl, start, false, resp);
                        }
                        Ok(ImageOutcome::Task { provider_task_id }) => {
                            // 异步任务 → 202，插入 media_tasks 供 M6-C 闭环
                            let billing_key = format!("{}:{}", upstream.id, provider_task_id);
                            // billing_key 唯一约束兜底防重：冲突则返回已存在任务行 id
                            let task_id: Uuid =
                                match sqlx::query_scalar(
                                    "INSERT INTO media_tasks \
                                     (id, media_type, gateway_key_id, model, upstream_id, provider_task_id, status, billing_key, raw, request_id) \
                                     VALUES ($1, 'image', $2, $3, $4, $5, 'pending', $6, $7, $8) \
                                     ON CONFLICT (billing_key) DO NOTHING RETURNING id",
                                )
                                .bind(Uuid::new_v4())
                                .bind(key.id)
                                .bind(&model)
                                .bind(upstream.id)
                                .bind(&provider_task_id)
                                .bind(&billing_key)
                                .bind(json.clone())
                                .bind(&request_id)
                                .fetch_optional(&state.db)
                                .await
                                {
                                    Ok(Some(id)) => id,
                                    Ok(None) => match sqlx::query_scalar::<_, Uuid>(
                                        "SELECT id FROM media_tasks WHERE billing_key = $1",
                                    )
                                    .bind(&billing_key)
                                    .fetch_optional(&state.db)
                                    .await
                                    {
                                        Ok(Some(id)) => id,
                                        Ok(None) => {
                                            let msg = "写入媒体任务失败：billing_key 冲突但未找到任务行".to_string();
                                            log_fail(&state, &tmpl, start, 500, &msg, (idx + failed_candidates) as i32);
                                            if let Some(rid) = idem_rec {
                                                let _ = crate::idempotency::fail(&state.db, rid).await;
                                            }
                                            return error_resp(Protocol::OpenaiChat, &request_id, 500, &msg);
                                        }
                                        Err(e) => {
                                            log_fail(&state, &tmpl, start, 500, &format!("写入媒体任务失败: {e}"), (idx + failed_candidates) as i32);
                                            if let Some(rid) = idem_rec {
                                                let _ = crate::idempotency::fail(&state.db, rid).await;
                                            }
                                            return error_resp(Protocol::OpenaiChat, &request_id, 500, "写入媒体任务失败");
                                        }
                                    },
                                    Err(e) => {
                                        log_fail(&state, &tmpl, start, 500, &format!("写入媒体任务失败: {e}"), (idx + failed_candidates) as i32);
                                        if let Some(rid) = idem_rec {
                                            let _ = crate::idempotency::fail(&state.db, rid).await;
                                        }
                                        return error_resp(Protocol::OpenaiChat, &request_id, 500, "写入媒体任务失败");
                                    }
                                };
                            let body_json = serde_json::json!({ "task_id": task_id });
                            let mut ev = tmpl.clone();
                            ev.upstream_id = Some(upstream.id);
                            ev.protocol_out = image_api_name(image_api).to_string();
                            ev.convert_mode = "media_adapt".into();
                            ev.latency_ms = Some(start.elapsed().as_millis() as i32);
                            ev.retry_count = (idx + failed_candidates) as i32;
                            ev.status = 202;
                            // 异步任务无即时图片：images/image_size 保持 NULL
                            if let Some(payload) = debug_payload(
                                &debug_req,
                                &serde_json::to_vec(&body_json).unwrap_or_default(),
                            ) {
                                ev.debug_payload = Some(payload);
                            }
                            // 幂等登记完成（M10.2）：异步任务缓存 {"task_id":...}，重放返回同一任务
                            if let Some(rid) = idem_rec {
                                if let Err(e) = crate::idempotency::complete(
                                    &state.db,
                                    rid,
                                    202,
                                    &body_json,
                                    Some(task_id),
                                )
                                .await
                                {
                                    tracing::warn!("幂等键结果写入失败: {e}");
                                }
                            }
                            state.log_sink.log(ev);
                            let lbl = image_metrics_labels(&key, &upstream.name);
                            let mut resp =
                                json_rsp(StatusCode::ACCEPTED, &request_id, None, body_json);
                            add_ratelimit_headers(&mut resp, rl_status);
                            return record(&state.metrics, &lbl, start, false, resp);
                        }
                        Err(e) => {
                            // 上游响应虽 2xx 但结构不符合预期：视为该候选失败（BodyRead 不可重试）
                            last_upstream = Some(upstream.name.clone());
                            last_err = Some(e);
                            failed_candidates += 1;
                            last_idx = idx;
                            if lock {
                                break 'outer;
                            }
                            break;
                        }
                    }
                }
                Err(e) => {
                    let retryable = e.retryable(&route.retry_status_codes);
                    if retryable {
                        state.breaker.on_failure(&state.db, upstream).await;
                    }
                    let no_failover = e.client_causal();
                    last_upstream = Some(upstream.name.clone());
                    last_err = Some(e);
                    let can_retry = retryable && !lock && idx < retry_limit;
                    if can_retry {
                        tokio::time::sleep(Duration::from_millis(100u64 << idx.min(30))).await;
                        idx += 1;
                        continue;
                    }
                    failed_candidates += 1;
                    last_idx = idx;
                    if lock || no_failover {
                        break 'outer;
                    }
                    break;
                }
            }
        }
    }

    // 全部候选失败：log_fail 记账 + OpenAI 错误体
    let lbl = image_metrics_labels(&key, last_upstream.as_deref().unwrap_or(""));
    let (status, msg): (u16, String) = match &last_err {
        Some(UpstreamError::Status(s, body)) => (*s, body.clone()),
        Some(other) => (502, other.to_string()),
        None => (502, "无可用上游".to_string()),
    };
    let mut ev = tmpl.clone();
    ev.protocol_out = last_protocol_out.unwrap_or_else(|| "openai_image".into());
    ev.convert_mode = last_convert_mode.unwrap_or_else(|| "none".into());
    let retry_count = (last_idx + failed_candidates.saturating_sub(1)) as i32;
    log_fail(&state, &ev, start, status, &msg, retry_count);
    if let Some(rid) = idem_rec {
        let _ = crate::idempotency::fail(&state.db, rid).await;
    }
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let body = error_from_ir(
        Protocol::OpenaiChat,
        &IrError {
            status,
            message: msg,
            ..Default::default()
        },
    );
    let resp = json_rsp(code, &request_id, None, body);
    record(&state.metrics, &lbl, start, true, resp)
}

/// POST /v1/videos/generations：视频占位（v1.18 未接入）。不做鉴权/记账，恒 501。
async fn video_generations_placeholder() -> Response {
    let request_id = Uuid::new_v4().to_string();
    json_rsp(
        StatusCode::NOT_IMPLEMENTED,
        &request_id,
        None,
        serde_json::json!({
            "error": { "message": "视频生成暂未接入（占位）", "type": "not_implemented" }
        }),
    )
}

/// GET /v1/videos/tasks/{*task_id}：视频任务占位，恒 501。
async fn video_tasks_placeholder(_task_id: Path<String>) -> Response {
    let request_id = Uuid::new_v4().to_string();
    json_rsp(
        StatusCode::NOT_IMPLEMENTED,
        &request_id,
        None,
        serde_json::json!({
            "error": { "message": "视频生成暂未接入（占位）", "type": "not_implemented" }
        }),
    )
}

/// GET /v1/models：网关可用模型列表（OpenAI 格式）。与 OpenAI 语义一致，需网关 Key 鉴权
/// （review P4：此前公开可枚举模型清单与上游拓扑）。
/// 仅暴露当前 Key 白名单允许、且存在可用路由（非通配 pattern + 上游 enabled）的模型
/// （PLAN P0：避免向 Key 泄露其无权访问的模型清单）。通配路由无法枚举具体模型，不列出。
async fn list_models(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let key = match authenticate_gateway_key(&state, &headers) {
        Ok(k) => k,
        Err(resp) => return *resp,
    };
    let request_id = Uuid::new_v4().to_string();
    let snap = state.cache.snapshot();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut data: Vec<serde_json::Value> = Vec::new();
    for route in &snap.routes {
        // 只收集非通配 model_pattern + Key 白名单允许 + 对应上游 enabled
        let pattern = route.model_pattern.as_str();
        if pattern.is_empty() || pattern.contains('*') {
            continue;
        }
        if !key.model_allowed(pattern) {
            continue;
        }
        let Some(up) = snap.upstreams.get(&route.upstream_id) else {
            continue;
        };
        if !up.enabled {
            continue;
        }
        if seen.insert(pattern.to_string()) {
            data.push(serde_json::json!({
                "id": pattern,
                "object": "model",
                "created": route.created_at.timestamp(),
                "owned_by": "nextapi",
            }));
        }
    }
    // M10.1：启用的别名同样对外暴露——要求白名单允许解析后的实际模型，
    // 且实际模型存在可用路由（含通配命中）+ 上游 enabled。
    for alias in snap.aliases.values() {
        if !alias.enabled || !key.model_allowed(&alias.model) {
            continue;
        }
        let routable = snap.routes.iter().any(|r| {
            r.enabled
                && crate::routing::pattern_matches(&r.model_pattern, &alias.model)
                && snap
                    .upstreams
                    .get(&r.upstream_id)
                    .is_some_and(|u| u.enabled)
        });
        if routable && seen.insert(alias.alias.clone()) {
            data.push(serde_json::json!({
                "id": alias.alias,
                "object": "model",
                "created": alias.created_at.timestamp(),
                "owned_by": "nextapi",
            }));
        }
    }
    let body = serde_json::json!({ "object": "list", "data": data });
    json_rsp(StatusCode::OK, &request_id, None, body)
}

/// 网关入口路由（无鉴权中间件；Key 鉴权在各 handler 内完成）。
///
/// 路由清单（PLAN.md §7.1）：
/// - POST /v1/chat/completions        → OpenAI Chat
/// - POST /v1/responses               → OpenAI Responses
/// - POST /v1/messages                → Anthropic Messages
/// - GET  /v1/models                  → 网关可用模型列表（OpenAI 格式）
/// - POST /v1beta/models/{*action}    → Gemini（action 形如 "model:generateContent"
///   或 "model:streamGenerateContent"，手动解析冒号）
/// - POST /v1/images/generations      → 图片生成（统一 OpenAI 语义，契约 m6 §4）
/// - GET  /v1/images/tasks/{task_id}  → 异步图片任务查询（M6-C）
/// - POST /v1/videos/generations      → 视频占位（501，v1.18 未接入）
/// - GET  /v1/videos/tasks/{*task_id} → 视频任务占位（501）
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/chat/completions", post(openai_chat))
        .route("/v1/responses", post(openai_responses))
        .route("/v1/messages", post(anthropic))
        .route("/v1/models", get(list_models))
        .route("/v1beta/models/{*action}", post(gemini))
        // 图片通道（契约 m6 §4）
        .route("/v1/images/generations", post(images_generations))
        .route(
            "/v1/images/tasks/{task_id}",
            get(media::tasks::get_image_task),
        )
        // 视频占位（v1.18 未接入，恒 501）
        .route(
            "/v1/videos/generations",
            post(video_generations_placeholder),
        )
        .route("/v1/videos/tasks/{*task_id}", get(video_tasks_placeholder))
}

// ---------------------------------------------------------------------------
// 纯逻辑单测
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use chrono::Utc;
    use serde_json::json;

    /// 粘性重排（M11.3）：pinned 在最优组 → 提首；跨组/不存在 → 不动。
    #[test]
    fn sticky_reorder_rules() {
        let mk_route = |priority: i32| crate::entities::ModelRouteRow {
            id: Uuid::new_v4(),
            model_pattern: "m".into(),
            upstream_id: Uuid::new_v4(),
            override_model: None,
            priority,
            weight: 1,
            enabled: true,
            retries: 2,
            retry_status_codes: vec![429],
            lock_upstream: false,
            sort_order: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let mk_up = |name: &str| UpstreamRow {
            id: Uuid::new_v4(),
            name: name.into(),
            kind: "openai".into(),
            base_url: "http://localhost".into(),
            api_key_plain: None,
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
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let mk_attempt = |name: &str, priority: i32| Attempt {
            candidate: Candidate {
                route: mk_route(priority),
                upstream: mk_up(name),
            },
            mode: Mode::Passthrough,
        };
        let a = mk_attempt("a", 10);
        let b = mk_attempt("b", 10);
        let c = mk_attempt("c", 20); // 次优组
        let (id_a, id_b, id_c) = (
            a.candidate.upstream.id,
            b.candidate.upstream.id,
            c.candidate.upstream.id,
        );

        // pinned b 在最优组 → 提到首位
        let mut v = vec![a.clone(), b.clone(), c.clone()];
        reorder_pinned_first(&mut v, id_b);
        assert_eq!(v[0].candidate.upstream.id, id_b);
        assert_eq!(v.len(), 3);
        // pinned c 在次优组 → 不动
        let mut v = vec![a.clone(), b.clone(), c.clone()];
        reorder_pinned_first(&mut v, id_c);
        assert_eq!(v[0].candidate.upstream.id, id_a);
        // pinned 不存在 → 不动
        let mut v = vec![a.clone(), b.clone(), c.clone()];
        reorder_pinned_first(&mut v, Uuid::new_v4());
        assert_eq!(v[0].candidate.upstream.id, id_a);
        // 空表不动
        let mut v: Vec<Attempt> = vec![];
        reorder_pinned_first(&mut v, id_a);
        assert!(v.is_empty());
    }

    /// 能力需求检测（M10.3）：tools 参数、四协议图片 part、stream 标志。
    #[test]
    fn required_caps_detection() {
        // 纯文本非流式 → 无需求
        let body = json!({"model":"m","messages":[{"role":"user","content":"hi"}]});
        let r = required_caps(&body, false);
        assert!(!r.stream && !r.tools && !r.vision);
        // chat tools + image_url part + stream
        let body = json!({
            "model":"m","stream":true,
            "tools":[{"type":"function","function":{"name":"f"}}],
            "messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"u"}}]}]
        });
        let r = required_caps(&body, true);
        assert!(r.stream && r.tools && r.vision);
        // responses input_image
        let body = json!({"model":"m","input":[{"role":"user","content":[{"type":"input_image","image_url":"u"}]}]});
        assert!(required_caps(&body, false).vision);
        // anthropic image part
        let body = json!({"model":"m","messages":[{"role":"user","content":[{"type":"image","source":{}}]}]});
        assert!(required_caps(&body, false).vision);
        // gemini inline_data
        let body = json!({"contents":[{"role":"user","parts":[{"inline_data":{"mime_type":"image/png","data":"x"}}]}]});
        assert!(required_caps(&body, false).vision);
        // legacy functions 也视为 tools
        let body = json!({"model":"m","functions":[{"name":"f"}]});
        assert!(required_caps(&body, false).tools);
        // 空 tools 数组不算
        let body = json!({"model":"m","tools":[]});
        assert!(!required_caps(&body, false).tools);
    }

    /// 别名解析（M10.1）：启用别名单跳替换并保留入口名；禁用/未命中原样返回。
    #[test]
    fn resolve_alias_rules() {
        let mk = |alias: &str, model: &str, enabled: bool| crate::entities::ModelAliasRow {
            id: Uuid::new_v4(),
            alias: alias.into(),
            model: model.into(),
            enabled,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let mut snap = crate::cache::Snapshot::default();
        let on = mk("fast-gpt", "gpt-4o-mini", true);
        snap.aliases.insert(on.alias.clone(), on);
        let off = mk("off-alias", "gpt-4o", false);
        snap.aliases.insert(off.alias.clone(), off);

        // 命中启用别名 → 解析 + 保留入口名
        let (m, req) = resolve_alias(&snap, "fast-gpt");
        assert_eq!(m, "gpt-4o-mini");
        assert_eq!(req.as_deref(), Some("fast-gpt"));
        // 禁用别名不解析
        let (m, req) = resolve_alias(&snap, "off-alias");
        assert_eq!(m, "off-alias");
        assert!(req.is_none());
        // 未命中原样
        let (m, req) = resolve_alias(&snap, "gpt-4o");
        assert_eq!(m, "gpt-4o");
        assert!(req.is_none());
    }

    /// tee 收集：验证逐 chunk 文本累积与 ttfb 记录（ctx=None 避免触发 finalize 的 DB 记账）。
    #[tokio::test]
    async fn tee_stream_collects_text_and_ttfb() {
        use futures::StreamExt;
        let inner = Body::from_stream(futures::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from_static(b"data: {\"a\":1}\n\n")),
            Ok::<Bytes, std::io::Error>(Bytes::from_static(b"data: [DONE]\n\n")),
        ]));
        let start = Instant::now();
        let collect = Arc::new(Mutex::new(StreamCollect {
            text: String::new(),
            ttfb_ms: None,
        }));
        let mut tee = TeeStream {
            inner: inner.into_data_stream(),
            collect: collect.clone(),
            start,
            ctx: None,
            done: false,
            finalized: false,
        };
        let mut n = 0;
        while let Some(Ok(_)) = Pin::new(&mut tee).next().await {
            n += 1;
        }
        assert_eq!(n, 2);
        let c = collect.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(c.text, "data: {\"a\":1}\n\ndata: [DONE]\n\n");
        assert!(c.ttfb_ms.is_some());
    }

    fn make_upstream() -> UpstreamRow {
        UpstreamRow {
            id: Uuid::new_v4(),
            name: "up".into(),
            kind: "openai".into(),
            base_url: "http://localhost".into(),
            api_key_plain: Some("sk-up".into()),
            protocols: vec!["openai_chat".into()],
            enabled: true,
            timeout_ms: 30_000,
            breaker_threshold: 5,
            probe_model: None,
            consecutive_failures: 0,
            disabled_by: None,
            cooldown_until: None,
            use_proxy: false,
            proxy_id: None,
            extra: json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn mk_route(
        tag: &str,
        pattern: &str,
        priority: i32,
        weight: i32,
        enabled: bool,
        sort_order: i32,
    ) -> ModelRouteRow {
        let now = Utc::now();
        ModelRouteRow {
            id: Uuid::new_v4(),
            model_pattern: pattern.into(),
            upstream_id: Uuid::new_v4(),
            override_model: Some(tag.into()),
            priority,
            weight,
            enabled,
            retries: 2,
            retry_status_codes: vec![429, 500],
            lock_upstream: false,
            sort_order,
            created_at: now,
            updated_at: now,
        }
    }

    fn cand(up: UpstreamRow, route: ModelRouteRow) -> Candidate {
        Candidate {
            route,
            upstream: up,
        }
    }

    #[test]
    fn extract_api_key_forms() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer sk-a".parse().unwrap());
        assert_eq!(extract_api_key(&h, Entry::OpenaiChat), Some("sk-a".into()));

        let mut h2 = HeaderMap::new();
        h2.insert("x-api-key", "sk-b".parse().unwrap());
        assert_eq!(extract_api_key(&h2, Entry::Anthropic), Some("sk-b".into()));

        let mut h3 = HeaderMap::new();
        h3.insert("x-goog-api-key", "sk-c".parse().unwrap());
        assert_eq!(extract_api_key(&h3, Entry::Gemini), Some("sk-c".into()));

        // 兼容：openai 入口也接受 x-api-key
        let mut h4 = HeaderMap::new();
        h4.insert("x-api-key", "sk-d".parse().unwrap());
        assert_eq!(extract_api_key(&h4, Entry::OpenaiChat), Some("sk-d".into()));

        // 无 header
        assert_eq!(extract_api_key(&HeaderMap::new(), Entry::Anthropic), None);
    }

    #[test]
    fn clean_auth_value_bearer_and_plain() {
        assert_eq!(
            clean_auth_value("authorization", "Bearer xyz"),
            Some("xyz".into())
        );
        assert_eq!(clean_auth_value("authorization", "xyz"), Some("xyz".into()));
        assert_eq!(
            clean_auth_value("x-goog-api-key", "gkey"),
            Some("gkey".into())
        );
        assert_eq!(clean_auth_value("authorization", "   "), None);
    }

    #[test]
    fn hash_key_is_sha256_hex() {
        assert_eq!(
            hash_key("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn gemini_action_parse() {
        assert_eq!(
            parse_gemini_action("gemini-pro:generateContent"),
            Some(("gemini-pro".into(), false))
        );
        assert_eq!(
            parse_gemini_action("gemini-pro:streamGenerateContent"),
            Some(("gemini-pro".into(), true))
        );
        assert_eq!(parse_gemini_action("gemini-pro"), None);
        assert_eq!(parse_gemini_action("gemini-pro:foo"), None);
        assert_eq!(parse_gemini_action(":generateContent"), None);
        // 防御：容忍前导斜杠/空白
        assert_eq!(
            parse_gemini_action("/gemini-pro:generateContent"),
            Some(("gemini-pro".into(), false))
        );
    }

    #[test]
    fn router_builds_without_panic() {
        // 验证通配符路由模式合法（matchit 于构建时校验）
        let _ = router();
    }

    #[tokio::test]
    async fn video_placeholder_returns_501() {
        // 契约 m6 §7-B：视频占位恒 501（不做鉴权/记账）
        let resp = video_generations_placeholder().await;
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["type"], "not_implemented");
        assert_eq!(v["error"]["message"], "视频生成暂未接入（占位）");

        // 视频任务占位同样 501
        let tasks_resp = video_tasks_placeholder(Path("task-abc".to_string())).await;
        assert_eq!(tasks_resp.status(), StatusCode::NOT_IMPLEMENTED);
        let tbytes = axum::body::to_bytes(tasks_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let tv: serde_json::Value = serde_json::from_slice(&tbytes).unwrap();
        assert_eq!(tv["error"]["type"], "not_implemented");
    }

    #[test]
    fn image_api_name_maps_variants() {
        assert_eq!(image_api_name(ImageApi::Openai), "images_openai");
        assert_eq!(image_api_name(ImageApi::Gemini), "images_gemini");
        assert_eq!(
            image_api_name(ImageApi::DashscopeSync),
            "images_dashscope_sync"
        );
        assert_eq!(
            image_api_name(ImageApi::DashscopeAsync),
            "images_dashscope_async"
        );
    }

    #[test]
    fn extract_model_forms() {
        let body = json!({ "model": "gpt-4o" });
        assert_eq!(
            extract_model(Entry::OpenaiChat, &body, None).unwrap(),
            "gpt-4o"
        );
        // Gemini 模型取自路径
        assert_eq!(
            extract_model(Entry::Gemini, &body, Some("gemini-pro")).unwrap(),
            "gemini-pro"
        );
        // 缺失 → 400
        assert!(extract_model(Entry::Anthropic, &json!({}), None).is_err());
        assert!(extract_model(Entry::Gemini, &body, None).is_err());
    }

    #[test]
    fn extract_max_tokens_forms() {
        let chat = Protocol::OpenaiChat;
        assert_eq!(
            extract_max_tokens(chat, &json!({ "max_tokens": 100 })),
            Some(100)
        );
        assert_eq!(
            extract_max_tokens(chat, &json!({ "max_completion_tokens": 200 })),
            Some(200)
        );
        // max_tokens 优先
        assert_eq!(
            extract_max_tokens(
                chat,
                &json!({ "max_tokens": 1, "max_completion_tokens": 2 })
            ),
            Some(1)
        );
        assert_eq!(
            extract_max_tokens(chat, &json!({ "max_tokens": "x" })),
            None
        );
        assert_eq!(extract_max_tokens(chat, &json!({})), None);
        // 各协议字段分派
        assert_eq!(
            extract_max_tokens(
                Protocol::OpenaiResponses,
                &json!({ "max_output_tokens": 300 })
            ),
            Some(300)
        );
        assert_eq!(
            extract_max_tokens(Protocol::Anthropic, &json!({ "max_tokens": 400 })),
            Some(400)
        );
        assert_eq!(
            extract_max_tokens(
                Protocol::Gemini,
                &json!({ "generationConfig": { "maxOutputTokens": 500 } })
            ),
            Some(500)
        );
        assert_eq!(
            extract_max_tokens(Protocol::Gemini, &json!({ "max_tokens": 1 })),
            None
        );
    }

    #[test]
    fn is_stream_forms() {
        assert!(is_stream(&json!({ "stream": true }), None));
        assert!(!is_stream(&json!({ "stream": false }), None));
        assert!(!is_stream(&json!({}), None));
        // Gemini 以路径为准
        assert!(is_stream(&json!({}), Some(true)));
        assert!(!is_stream(&json!({ "stream": true }), Some(false)));
    }

    #[test]
    fn decide_mode_prefers_passthrough() {
        let mut up = make_upstream();
        up.protocols = vec!["openai_chat".into(), "anthropic".into()];
        up.extra = json!({});
        assert_eq!(
            decide_mode(Protocol::OpenaiChat, &up),
            Some(Mode::Passthrough)
        );
        // 入口不受支持 → 按优先级取第一个支持协议
        assert_eq!(
            decide_mode(Protocol::Gemini, &up),
            Some(Mode::Convert(Protocol::OpenaiChat))
        );
        // 只支持 gemini 时，openai 入口应转换为 gemini
        let mut up1 = make_upstream();
        up1.protocols = vec!["gemini".into()];
        assert_eq!(
            decide_mode(Protocol::OpenaiChat, &up1),
            Some(Mode::Convert(Protocol::Gemini))
        );
        // 完全无支持协议 → None
        let mut up2 = make_upstream();
        up2.protocols = vec![];
        assert_eq!(decide_mode(Protocol::OpenaiChat, &up2), None);
    }

    #[test]
    fn decide_mode_honors_priority_override() {
        let mut up = make_upstream();
        up.protocols = vec!["gemini".into(), "anthropic".into()];
        up.extra = json!({ "protocol_priority": ["anthropic", "gemini"] });
        assert_eq!(
            decide_mode(Protocol::OpenaiChat, &up),
            Some(Mode::Convert(Protocol::Anthropic))
        );
    }

    #[test]
    fn order_candidates_grouped_and_once() {
        let up = make_upstream();
        let a = cand(up.clone(), mk_route("a", "gpt-*", 10, 1, true, 0));
        let b = cand(up.clone(), mk_route("b", "gpt-*", 10, 9, true, 1));
        let c = cand(up.clone(), mk_route("c", "gpt-*", 20, 1, true, 0));
        let out = order_candidates(vec![a.clone(), b.clone(), c.clone()]);
        assert_eq!(out.len(), 3);
        let ids: Vec<Uuid> = out.iter().map(|x| x.route.id).collect();
        for x in [&a, &b, &c] {
            assert_eq!(ids.iter().filter(|&&id| id == x.route.id).count(), 1);
        }
        // 优先级升序分组：两个 priority=10 在前，priority=20 在后
        assert!(
            out[0].route.priority <= out[1].route.priority
                && out[1].route.priority <= out[2].route.priority
        );
    }

    #[test]
    fn build_outbound_passthrough_writes_model_and_usage_option() {
        let up = make_upstream();
        let body = json!({ "model": "orig", "stream": true });
        let ob = build_outbound(
            Protocol::OpenaiChat,
            Mode::Passthrough,
            &body,
            true,
            "up-model",
            &up,
        )
        .unwrap();
        assert_eq!(ob.body["model"], json!("up-model"));
        assert_eq!(ob.body["stream_options"]["include_usage"], json!(true));
        assert_eq!(ob.protocol, Protocol::OpenaiChat);
    }

    #[test]
    fn build_outbound_convert_uses_ir_model_and_stream() {
        let up = make_upstream();
        let body = json!({ "messages": [{ "role": "user", "content": "hi" }], "stream": true });
        let ob = build_outbound(
            Protocol::Anthropic,
            Mode::Convert(Protocol::OpenaiChat),
            &body,
            true,
            "up-model",
            &up,
        )
        .unwrap();
        assert_eq!(ob.protocol, Protocol::OpenaiChat);
        assert_eq!(ob.body["model"], json!("up-model"));
        assert_eq!(ob.body["stream"], json!(true));
        // 目标 openai 系 → 注入 stream_options.include_usage
        assert_eq!(ob.body["stream_options"]["include_usage"], json!(true));
    }
}
