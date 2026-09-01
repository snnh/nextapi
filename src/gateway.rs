//! 网关入口与请求主链路（PLAN.md §3.1/§3.2）。
//!
//! 请求 → 鉴权（网关 Key）→ 限流/用量上限 → 模型白名单 → 协议解析 → 路由决策
//!      → 透传/转换 → 上游调用（重试/熔断/故障转移）→ 响应转换回入口协议 → 客户端。
//! （旁路 usage 提取与记账在 M4 接入；本模块每请求生成 request_id 并写 X-Request-Id。）

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
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
    PassthroughFallback,
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

/// 把字节以 UTF-8 安全方式追加到收集缓冲（不切断多字节字符；超限截断）。
fn append_chunk(collect: &Mutex<StreamCollect>, bytes: &[u8], start: Instant) {
    let mut c = collect.lock().unwrap_or_else(|p| p.into_inner());
    if c.text.len() < STREAM_TEXT_MAX {
        let remain = STREAM_TEXT_MAX - c.text.len();
        let mut take = bytes.len().min(remain);
        // 裁剪到字符边界，避免把多字节字符截断
        while take > 0 && std::str::from_utf8(&bytes[..take]).is_err() {
            take -= 1;
        }
        c.text.push_str(&String::from_utf8_lossy(&bytes[..take]));
    }
    if c.ttfb_ms.is_none() {
        c.ttfb_ms = Some(start.elapsed().as_millis() as i32);
    }
}

/// 将 axum::Error 映射为 std::io::Error（供 Body::from_stream 的 Err 类型约束）。
fn to_io_err(e: axum::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.into_inner())
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
            let (resp, resp_trunc) =
                crate::logging::redact::redact_and_truncate(text.as_bytes(), crate::logging::DEBUG_MAX_BYTES);
            ev.debug_payload = Some(serde_json::json!({
                "request": req.clone(),
                "response": resp,
                "truncated": *req_trunc || resp_trunc,
            }));
        }
        ctx.state.log_sink.log(ev);

        let total = usage.prompt_tokens.unwrap_or(0).saturating_add(usage.completion_tokens.unwrap_or(0));
        if total > 0 {
            ctx.state.metrics.gateway_tokens.get_or_create(&ctx.labels).inc_by(total.max(0) as u64);
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
        let val = t
            .strip_prefix("Bearer ")
            .or_else(|| t.strip_prefix("bearer "))
            .unwrap_or(t)
            .trim();
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
fn extract_model(entry: Entry, body: &serde_json::Value, gemini_model: Option<&str>) -> Result<String, ApiError> {
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
fn extract_max_tokens(body: &serde_json::Value) -> Option<u64> {
    body.get("max_tokens")
        .or_else(|| body.get("max_completion_tokens"))
        .and_then(|v| v.as_u64())
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
    let mut groups: std::collections::BTreeMap<i32, Vec<Candidate>> = std::collections::BTreeMap::new();
    for c in candidates {
        groups.entry(c.route.priority).or_default().push(c);
    }
    let mut ordered: Vec<Candidate> = Vec::new();
    for (_prio, group) in groups {
        let routes: Vec<ModelRouteRow> = group.iter().map(|c| c.route.clone()).collect();
        if let Some(primary) = routing::weighted_pick(&routes) {
            if let Some(p) = group.iter().find(|c| c.route.id == primary.id).cloned() {
                ordered.push(p.clone());
                let mut rest: Vec<Candidate> =
                    group.iter().filter(|c| c.route.id != p.route.id).cloned().collect();
                rest.sort_by(|a, b| {
                    b.route.weight
                        .cmp(&a.route.weight)
                        .then(a.route.sort_order.cmp(&b.route.sort_order))
                });
                ordered.extend(rest);
            }
        }
    }
    ordered
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
        Mode::Passthrough | Mode::PassthroughFallback => {
            let mut b = body.clone();
            upstream::rewrite_body(&mut b, Some(upstream_model), upstream.overrides().as_ref());
            // PLAN §4.4：OpenAI 系透传流式注入 stream_options.include_usage 用于 usage 采集兜底
            inject_stream_usage_option(&mut b, entry, stream);
            Ok(Outbound { body: b, protocol: entry, mode })
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
            let out = protocol::request_from_ir(target, &ir, &mut ctx)?;
            Ok(Outbound { body: out, protocol: target, mode })
        }
    }
}

/// 若为 OpenAI Chat 流式请求且未带 stream_options.include_usage，则注入该选项。
fn inject_stream_usage_option(body: &mut serde_json::Value, protocol: Protocol, stream: bool) {
    if !stream || protocol != Protocol::OpenaiChat {
        return;
    }
    let Some(obj) = body.as_object_mut() else { return };
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
fn json_rsp(status: StatusCode, request_id: &str, degraded: Option<&str>, body: serde_json::Value) -> Response {
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    h.insert(HDR_REQUEST_ID, request_id.parse::<header::HeaderValue>().unwrap_or_else(|_| header::HeaderValue::from_static("")));
    if let Some(d) = degraded {
        if let Ok(dv) = header::HeaderValue::from_str(d) {
            h.insert(HDR_DEGRADED, dv);
        }
    }
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    (status, h, Body::from(bytes)).into_response()
}

/// 以入口协议（错误结构）构造错误 JSON 响应。
fn error_resp(entry: Protocol, request_id: &str, status: u16, message: &str) -> Response {
    let ir = IrError { status, message: message.to_string(), ..Default::default() };
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    json_rsp(code, request_id, None, error_from_ir(entry, &ir))
}

/// 构造 SSE 流式响应。
fn stream_rsp(request_id: &str, body: Body) -> Response {
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
    h.insert(HDR_REQUEST_ID, request_id.parse::<header::HeaderValue>().unwrap_or_else(|_| header::HeaderValue::from_static("")));
    (StatusCode::OK, h, body).into_response()
}

/// mode 转换后的字符串（contract §12.7）。
fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Passthrough => "passthrough",
        Mode::Convert(_) => "convert",
        Mode::PassthroughFallback => "passthrough_fallback",
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
fn debug_payload(req: &Option<(serde_json::Value, bool)>, resp_bytes: &[u8]) -> Option<serde_json::Value> {
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
                Ok(out) => NonstreamOutcome {
                    response: json_rsp(StatusCode::OK, request_id, ctx.degraded_header().as_deref(), out.clone()),
                    status: 200,
                    degraded: !ctx.degraded.is_empty(),
                    error: None,
                    final_json: Some(out),
                },
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
                    obj.insert("model".into(), serde_json::Value::String(gateway_model.to_string()));
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
                    .unwrap_or_else(|_| IrError { status: *s, message: body.clone(), ..Default::default() });
                (*s, error_from_ir(entry, &ir))
            }
            other => {
                let ir = IrError { status: 502, message: other.to_string(), ..Default::default() };
                (502, error_from_ir(entry, &ir))
            }
        },
        Fail::Convert(e) => {
            let ir = IrError { status: 502, message: e.to_string(), ..Default::default() };
            (502, error_from_ir(entry, &ir))
        }
    }
}

// ---------------------------------------------------------------------------
// 指标
// ---------------------------------------------------------------------------

fn metrics_labels(key: &ApiKeyRow, model: &str, entry: Entry, upstream: &str) -> RequestLabels {
    RequestLabels {
        key_prefix: key.prefix.clone(),
        model: model.to_string(),
        upstream: upstream.to_string(),
        protocol: entry.protocol().as_str().to_string(),
    }
}

fn record(metrics: &crate::metrics::Metrics, labels: &RequestLabels, start: Instant, is_error: bool, resp: Response) -> Response {
    metrics.gateway_requests.get_or_create(labels).inc();
    if is_error {
        metrics.gateway_errors.get_or_create(labels).inc();
    }
    metrics.gateway_latency.get_or_create(labels).observe(start.elapsed().as_secs_f64());
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

    // 2. Key 鉴权
    let Some(raw_key) = extract_api_key(&headers, entry) else {
        return error_resp(entry_protocol, &request_id, 401, "未授权");
    };
    let snap = state.cache.snapshot();
    let Some(key) = snap.api_keys.get(&hash_key(&raw_key)).cloned() else {
        return error_resp(entry_protocol, &request_id, 401, "未授权");
    };
    if !key.is_usable() {
        return error_resp(entry_protocol, &request_id, 401, "未授权");
    }

    // 鉴权通过后创建记账事件模板（contract §12.1；鉴权前的 401 不记账，避免刷库）。
    // model/stream/params 在解析后填充；上游相关字段（upstream_id/protocol_out/convert_mode）按候选填充。
    let mut tmpl = LogEvent {
        request_id: request_id.clone(),
        ts: chrono::Utc::now(),
        key_id: Some(key.id),
        model: String::new(),
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
    };
    let debug_active = key.debug_active();
    // params_meta 在 body 解析后计算（早退路径不引用，故延迟初始化）。
    let params_meta: serde_json::Value;
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
    // 模型/stream 确定后填充模板与参数元数据
    tmpl.model = model.clone();
    tmpl.stream = is_stream(&body_value, gemini_stream);
    let stream = tmpl.stream;
    let max_tokens = extract_max_tokens(&body_value);
    params_meta = crate::upstream::usage::request_params_meta(entry_protocol, &body_value);
    tmpl.usage_raw = Some(serde_json::json!({ "params": params_meta.clone() }));
    if debug_active {
        debug_req = Some(crate::logging::redact::redact_and_truncate(&body, crate::logging::DEBUG_MAX_BYTES));
    }

    // 4. 模型白名单
    if !key.model_allowed(&model) {
        log_fail(&state, &tmpl, start, 403, "禁止访问", 0);
        return error_resp(entry_protocol, &request_id, 403, "禁止访问");
    }

    // 5. 限流 + TPM 预检 + 配额
    if let Err(e) = state.limiter.check_rpm(&key, gateway_cfg.default_rate_limit_rpm) {
        let msg = e.to_string();
        log_fail(&state, &tmpl, start, e.status().as_u16(), &msg, 0);
        return error_resp(entry_protocol, &request_id, e.status().as_u16(), &msg);
    }
    if let Err(e) = limit::tpm_precheck(&key, max_tokens) {
        let msg = e.to_string();
        log_fail(&state, &tmpl, start, e.status().as_u16(), &msg, 0);
        return error_resp(entry_protocol, &request_id, e.status().as_u16(), &msg);
    }
    {
        let exceeded = match state.quota_cache.get(key.id, gateway_cfg.quota_check_cache_secs) {
            Some(v) => v,
            None => {
                let v = matches!(
                    limit::check_quota(&state.db, &key, &gateway_cfg.billing_timezone, &gateway_cfg.quota_exceed_action).await,
                    Err(ApiError::RateLimited)
                );
                state.quota_cache.put(key.id, v);
                v
            }
        };
        if exceeded {
            log_fail(&state, &tmpl, start, 429, "请求过于频繁，请稍后再试", 0);
            return error_resp(entry_protocol, &request_id, 429, "请求过于频繁，请稍后再试");
        }
    }

    // 6. 路由决策
    let matched = routing::match_routes(&model, &snap.routes);
    if matched.is_empty() {
        let lbl = metrics_labels(&key, &model, entry, "");
        log_fail(&state, &tmpl, start, 404, "模型未配置路由", 0);
        return record(&state.metrics, &lbl, start, true, error_resp(entry_protocol, &request_id, 404, "模型未配置路由"));
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    for route in matched {
        if let Some(up) = snap.upstreams.get(&route.upstream_id).cloned() {
            if up.enabled && state.breaker.allow(&up) {
                candidates.push(Candidate { route, upstream: up });
            }
        }
    }
    if candidates.is_empty() {
        let lbl = metrics_labels(&key, &model, entry, "");
        log_fail(&state, &tmpl, start, 503, "无可用上游", 0);
        return record(&state.metrics, &lbl, start, true, error_resp(entry_protocol, &request_id, 503, "无可用上游"));
    }
    let attempts: Vec<Attempt> = order_candidates(candidates)
        .into_iter()
        .filter_map(|c| decide_mode(entry_protocol, &c.upstream).map(|mode| Attempt { candidate: c, mode }))
        .collect();
    if attempts.is_empty() {
        let lbl = metrics_labels(&key, &model, entry, "");
        log_fail(&state, &tmpl, start, 503, "无可用上游", 0);
        return record(&state.metrics, &lbl, start, true, error_resp(entry_protocol, &request_id, 503, "无可用上游"));
    }

    // 7-10. 协议决策 → 请求构建 → 执行/重试/故障转移 → 响应
    let mut last_err: Option<Fail> = None;
    let mut last_upstream: Option<String> = None;
    // retry_count 定义：本候选内重试次数 idx + 之前彻底失败的候选数。
    let mut failed_candidates: u32 = 0;
    let mut last_idx: u32 = 0;
    let mut last_protocol_out: Option<String> = None;
    let mut last_convert_mode: Option<String> = None;
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

        let outbound = match build_outbound(entry_protocol, mode, &body_value, stream, &up_model, upstream) {
            Ok(o) => o,
            Err(cvt) => {
                // 转换失败且入口协议受支持 → PassthroughFallback（PLAN §3.2）
                if matches!(mode, Mode::Convert(_)) && upstream.protocol_list().contains(&entry_protocol) {
                    let mut b = body_value.clone();
                    upstream::rewrite_body(&mut b, Some(&up_model), overrides.as_ref());
                    inject_stream_usage_option(&mut b, entry_protocol, stream);
                    Outbound { body: b, protocol: entry_protocol, mode: Mode::PassthroughFallback }
                } else {
                    // 该候选无法承接（转换失败且入口不受支持），转移到下一候选
                    last_upstream = Some(upstream.name.clone());
                    last_err = Some(Fail::Convert(cvt));
                    failed_candidates += 1;
                    last_idx = 0;
                    continue 'outer;
                }
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
        req_headers.insert(reqwest::header::CONTENT_TYPE, "application/json".parse().unwrap());
        upstream::apply_auth(&mut req_headers, outbound.protocol, upstream.api_key_plain.as_deref());
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
                state.client_pools.client_for(upstream, &snap, &default_proxy_id)
            }
        };

        let lock = route.lock_upstream;
        let retry_limit = if lock { 0 } else { route.retries.max(0) as u32 };
        let mut idx: u32 = 0;
        loop {
            let exec_res: Result<ExecOutcome, UpstreamError> = if stream {
                upstream::execute_stream(&client, &url, req_headers.clone(), &outbound.body, upstream.timeout_ms)
                    .await
                    .map(ExecOutcome::Stream)
            } else {
                upstream::execute_nonstream(&client, &url, req_headers.clone(), &outbound.body, upstream.timeout_ms)
                    .await
                    .map(ExecOutcome::Json)
            };

            match exec_res {
                Ok(ExecOutcome::Json(json)) => {
                    state.breaker.on_success(&state.db, upstream).await;
                    let lbl = metrics_labels(&key, &model, entry, &upstream.name);
                    // 用上游原始 JSON 提取 usage（§12.2）；转换前后 usage 语义等价，取原始值最准。
                    let usage = crate::upstream::usage::extract_json_usage(outbound.protocol, &json);
                    let outcome = nonstream_success(entry_protocol, outbound.mode, json, &model, &request_id);
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
                        let total = usage.prompt_tokens.unwrap_or(0).saturating_add(usage.completion_tokens.unwrap_or(0));
                        if total > 0 {
                            state.metrics.gateway_tokens.get_or_create(&lbl).inc_by(total.max(0) as u64);
                        }
                        if let Some(payload) = debug_payload(&debug_req, &serde_json::to_vec(
                            outcome.final_json.as_ref().unwrap_or(&serde_json::Value::Null),
                        ).unwrap_or_default()) {
                            ev.debug_payload = Some(payload);
                        }
                        state.log_sink.log(ev);
                        return record(&state.metrics, &lbl, start, false, outcome.response);
                    } else {
                        // 转换失败（502）：记为失败事件（token 恒 None）。
                        ev.status = outcome.status as i32;
                        ev.error = outcome.error.clone();
                        if let Some(payload) = debug_payload(&debug_req, &serde_json::to_vec(
                            outcome.final_json.as_ref().unwrap_or(&serde_json::Value::Null),
                        ).unwrap_or_default()) {
                            ev.debug_payload = Some(payload);
                        }
                        state.log_sink.log(ev);
                        return record(&state.metrics, &lbl, start, true, outcome.response);
                    }
                }
                Ok(ExecOutcome::Stream(resp)) => {
                    state.breaker.on_success(&state.db, upstream).await;
                    let lbl = metrics_labels(&key, &model, entry, &upstream.name);
                    let sbody = if matches!(outbound.mode, Mode::Convert(_)) {
                        Body::from_stream(upstream::sse_convert_stream(resp, outbound.protocol, entry_protocol))
                    } else {
                        Body::from_stream(upstream::sse_passthrough_stream(resp, model.clone(), request_id.clone()))
                    };
                    let mut ev = tmpl.clone();
                    ev.upstream_id = Some(upstream.id);
                    ev.protocol_out = outbound.protocol.as_str().to_string();
                    ev.convert_mode = mode_str(outbound.mode).to_string();
                    ev.degraded = false; // 流式转换的降级项在流内部处理，无法在此捕获，记为 false
                    let collect = Arc::new(Mutex::new(StreamCollect { text: String::new(), ttfb_ms: None }));
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
                    let resp = stream_rsp(&request_id, teed);
                    return record(&state.metrics, &lbl, start, false, resp);
                }
                Err(e) => {
                    let retryable = e.retryable(&route.retry_status_codes);
                    if retryable {
                        state.breaker.on_failure(&state.db, upstream).await;
                    }
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
                    if lock {
                        break 'outer;
                    }
                    break; // 转移到下一候选
                }
            }
        }
    }

    // 全部候选失败
    let lbl = metrics_labels(&key, &model, entry, last_upstream.as_deref().unwrap_or(""));
    let (status, msg, jbody) = match &last_err {
        Some(f) => {
            let (s, body) = entry_error(entry_protocol, f);
            (s, fail_message(f), body)
        }
        None => (
            502,
            "无可用上游".to_string(),
            error_from_ir(entry_protocol, &IrError { status: 502, message: "无可用上游".into(), ..Default::default() }),
        ),
    };
    // 失败终点：protocol_out 已知则填，convert_mode 已决策则填，否则兜底（log_fail 内完成）。
    // retry_count = 末候选内重试次数 last_idx + 之前彻底失败的候选数（failed_candidates 包含末候选，故减 1）。
    let mut ev = tmpl.clone();
    ev.protocol_out = last_protocol_out.unwrap_or_else(|| entry_protocol.as_str().to_string());
    ev.convert_mode = last_convert_mode.unwrap_or_else(|| "none".into());
    let retry_count = (last_idx + failed_candidates.saturating_sub(1)) as i32;
    log_fail(&state, &ev, start, status, &msg, retry_count);
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let resp = json_rsp(code, &request_id, None, jbody);
    record(&state.metrics, &lbl, start, true, resp)
}

// ---------------------------------------------------------------------------
// 入口 handlers（薄封装）
// ---------------------------------------------------------------------------

async fn openai_chat(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    run_gateway(state, Entry::OpenaiChat, headers, body, None).await
}

async fn openai_responses(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    run_gateway(state, Entry::OpenaiResponses, headers, body, None).await
}

async fn anthropic(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    run_gateway(state, Entry::Anthropic, headers, body, None).await
}

async fn gemini(State(state): State<Arc<AppState>>, Path(action): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    run_gateway(state, Entry::Gemini, headers, body, Some(action)).await
}

/// GET /v1/models：网关可用模型列表（OpenAI 格式）。
async fn list_models(State(state): State<Arc<AppState>>) -> Response {
    let request_id = Uuid::new_v4().to_string();
    let snap = state.cache.snapshot();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut data: Vec<serde_json::Value> = Vec::new();
    for route in &snap.routes {
        // 只收集非通配 model_pattern + 对应上游 enabled
        let pattern = route.model_pattern.as_str();
        if pattern.is_empty() || pattern.contains('*') {
            continue;
        }
        let Some(up) = snap.upstreams.get(&route.upstream_id) else { continue };
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
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/chat/completions", post(openai_chat))
        .route("/v1/responses", post(openai_responses))
        .route("/v1/messages", post(anthropic))
        .route("/v1/models", get(list_models))
        .route("/v1beta/models/{*action}", post(gemini))
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

    /// tee 收集：验证逐 chunk 文本累积与 ttfb 记录（ctx=None 避免触发 finalize 的 DB 记账）。
    #[tokio::test]
    async fn tee_stream_collects_text_and_ttfb() {
        use futures::StreamExt;
        let inner = Body::from_stream(futures::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from_static(b"data: {\"a\":1}\n\n")),
            Ok::<Bytes, std::io::Error>(Bytes::from_static(b"data: [DONE]\n\n")),
        ]));
        let start = Instant::now();
        let collect = Arc::new(Mutex::new(StreamCollect { text: String::new(), ttfb_ms: None }));
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
            api_key_enc: None,
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

    fn mk_route(tag: &str, pattern: &str, priority: i32, weight: i32, enabled: bool, sort_order: i32) -> ModelRouteRow {
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
        Candidate { route, upstream: up }
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
        assert_eq!(clean_auth_value("authorization", "Bearer xyz"), Some("xyz".into()));
        assert_eq!(clean_auth_value("authorization", "xyz"), Some("xyz".into()));
        assert_eq!(clean_auth_value("x-goog-api-key", "gkey"), Some("gkey".into()));
        assert_eq!(clean_auth_value("authorization", "   "), None);
    }

    #[test]
    fn hash_key_is_sha256_hex() {
        assert_eq!(hash_key("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn gemini_action_parse() {
        assert_eq!(parse_gemini_action("gemini-pro:generateContent"), Some(("gemini-pro".into(), false)));
        assert_eq!(parse_gemini_action("gemini-pro:streamGenerateContent"), Some(("gemini-pro".into(), true)));
        assert_eq!(parse_gemini_action("gemini-pro"), None);
        assert_eq!(parse_gemini_action("gemini-pro:foo"), None);
        assert_eq!(parse_gemini_action(":generateContent"), None);
        // 防御：容忍前导斜杠/空白
        assert_eq!(parse_gemini_action("/gemini-pro:generateContent"), Some(("gemini-pro".into(), false)));
    }

    #[test]
    fn router_builds_without_panic() {
        // 验证通配符路由模式合法（matchit 于构建时校验）
        let _ = router();
    }

    #[test]
    fn extract_model_forms() {
        let body = json!({ "model": "gpt-4o" });
        assert_eq!(extract_model(Entry::OpenaiChat, &body, None).unwrap(), "gpt-4o");
        // Gemini 模型取自路径
        assert_eq!(extract_model(Entry::Gemini, &body, Some("gemini-pro")).unwrap(), "gemini-pro");
        // 缺失 → 400
        assert!(extract_model(Entry::Anthropic, &json!({}), None).is_err());
        assert!(extract_model(Entry::Gemini, &body, None).is_err());
    }

    #[test]
    fn extract_max_tokens_forms() {
        assert_eq!(extract_max_tokens(&json!({ "max_tokens": 100 })), Some(100));
        assert_eq!(extract_max_tokens(&json!({ "max_completion_tokens": 200 })), Some(200));
        // max_tokens 优先
        assert_eq!(extract_max_tokens(&json!({ "max_tokens": 1, "max_completion_tokens": 2 })), Some(1));
        assert_eq!(extract_max_tokens(&json!({ "max_tokens": "x" })), None);
        assert_eq!(extract_max_tokens(&json!({})), None);
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
        assert_eq!(decide_mode(Protocol::OpenaiChat, &up), Some(Mode::Passthrough));
        // 入口不受支持 → 按优先级取第一个支持协议
        assert_eq!(decide_mode(Protocol::Gemini, &up), Some(Mode::Convert(Protocol::OpenaiChat)));
        // 只支持 gemini 时，openai 入口应转换为 gemini
        let mut up1 = make_upstream();
        up1.protocols = vec!["gemini".into()];
        assert_eq!(decide_mode(Protocol::OpenaiChat, &up1), Some(Mode::Convert(Protocol::Gemini)));
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
        assert_eq!(decide_mode(Protocol::OpenaiChat, &up), Some(Mode::Convert(Protocol::Anthropic)));
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
        assert!(out[0].route.priority <= out[1].route.priority && out[1].route.priority <= out[2].route.priority);
    }

    #[test]
    fn build_outbound_passthrough_writes_model_and_usage_option() {
        let up = make_upstream();
        let body = json!({ "model": "orig", "stream": true });
        let ob = build_outbound(Protocol::OpenaiChat, Mode::Passthrough, &body, true, "up-model", &up).unwrap();
        assert_eq!(ob.body["model"], json!("up-model"));
        assert_eq!(ob.body["stream_options"]["include_usage"], json!(true));
        assert_eq!(ob.protocol, Protocol::OpenaiChat);
    }

    #[test]
    fn build_outbound_convert_uses_ir_model_and_stream() {
        let up = make_upstream();
        let body = json!({ "messages": [{ "role": "user", "content": "hi" }], "stream": true });
        let ob = build_outbound(Protocol::Anthropic, Mode::Convert(Protocol::OpenaiChat), &body, true, "up-model", &up).unwrap();
        assert_eq!(ob.protocol, Protocol::OpenaiChat);
        assert_eq!(ob.body["model"], json!("up-model"));
        assert_eq!(ob.body["stream"], json!(true));
        // 目标 openai 系 → 注入 stream_options.include_usage
        assert_eq!(ob.body["stream_options"]["include_usage"], json!(true));
    }
}
