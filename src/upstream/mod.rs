//! 上游执行层：reqwest 连接池（按代理/直连分池）、透传改写、SSE 流处理（PLAN.md §3/§5.3）。
//!
//! - 透传默认原样转发；仅做模型名重写、鉴权头替换、用户配置的请求头/体 add/set 覆盖；
//! - SSE 透传边转发边按行解析，改写 chunk 的 model/id；单行解析失败该行原样放行；
//! - 敏感头（Host/Cookie/Proxy-*）不透传；其余头按白名单默认放行（M3：仅转发 content-type/accept）。

pub mod usage;

use bytes::Bytes;
use futures::{stream, Stream, StreamExt};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use std::{net::IpAddr, pin::Pin, sync::Mutex};

use uuid::Uuid;

use crate::cache::Snapshot;
use crate::entities::{Overrides, UpstreamRow};
use crate::protocol::ir::Protocol;
use crate::protocol::sse::{encode_event, SseParser};
use crate::protocol::{chunk_from_ir, chunk_to_ir, stream_end, AnyStreamState, ConvCtx};

/// 上游调用错误。
#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error("上游超时")]
    Timeout,
    #[error("连接失败: {0}")]
    Connect(String),
    /// 上游返回非 2xx（status, body 原文截断 2KB）
    #[error("上游错误 {0}: {1}")]
    Status(u16, String),
    #[error("响应读取失败: {0}")]
    BodyRead(String),
}

impl UpstreamError {
    /// 是否可重试（429/5xx/超时/连接失败；状态码列表来自 model_routes.retry_status_codes）。
    pub fn retryable(&self, codes: &[i32]) -> bool {
        match self {
            UpstreamError::Timeout | UpstreamError::Connect(_) => true,
            UpstreamError::Status(s, _) => codes.contains(&(*s as i32)),
            UpstreamError::BodyRead(_) => false,
        }
    }
}

/// reqwest 连接池：按「代理/直连」组合分池（避免不同代理连接串用，PLAN §5.9）。
pub struct ClientPools {
    inner: Mutex<HashMap<String, reqwest::Client>>,
}

impl ClientPools {
    pub fn new() -> Self {
        Self { inner: Mutex::new(HashMap::new()) }
    }

    /// 取上游对应的客户端：
    /// - use_proxy=false → 直连池；
    /// - use_proxy=true 且 proxy_id 指定 → 该代理池；
    /// - use_proxy=true 且 proxy_id 为空 → 跟随系统默认代理 default_proxy_id（空 = 直连）。
    ///
    /// `no_proxy` 命中判定由调用方（gateway）在决定代理前用 `no_proxy_match` 做，此处仅按代理选择。
    pub fn client_for(&self, up: &UpstreamRow, snap: &Snapshot, default_proxy_id: &str) -> reqwest::Client {
        let key = if !up.use_proxy {
            "direct".to_string()
        } else if let Some(pid) = up.proxy_id {
            pid.to_string()
        } else if !default_proxy_id.is_empty() {
            default_proxy_id.to_string()
        } else {
            "direct".to_string()
        };

        let mut inner = self.inner.lock().unwrap();
        if let Some(c) = inner.get(&key) {
            return c.clone();
        }
        let client = if key == "direct" {
            Self::build_client(None)
        } else {
            match Uuid::parse_str(&key) {
                Ok(pid) => match snap.proxies.get(&pid) {
                    Some(proxy) if proxy.enabled => match reqwest::Proxy::all(proxy.proxy_url()) {
                        Ok(p) => Self::build_client(Some(p)),
                        Err(e) => {
                            tracing::warn!("代理 `{}` URL 解析失败: {e}；回退直连", proxy.name);
                            Self::build_client(None)
                        }
                    },
                    Some(proxy) => {
                        tracing::warn!("代理 `{}` 已禁用；回退直连", proxy.name);
                        Self::build_client(None)
                    }
                    None => {
                        tracing::warn!("代理 `{pid}` 未找到；回退直连");
                        Self::build_client(None)
                    }
                },
                Err(_) => {
                    tracing::warn!("无效代理 id `{key}`；回退直连");
                    Self::build_client(None)
                }
            }
        };
        inner.insert(key, client.clone());
        client
    }

    fn build_client(proxy: Option<reqwest::Proxy>) -> reqwest::Client {
        let mut b = reqwest::Client::builder()
            .pool_max_idle_per_host(32)
            .tcp_keepalive(Duration::from_secs(60));
        if let Some(p) = proxy {
            b = b.proxy(p);
        }
        b.build().expect("构建 reqwest Client 失败")
    }
}

impl Default for ClientPools {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientPools {
    /// 直连池客户端（no_proxy 命中时使用）。
    pub fn direct_client(&self) -> reqwest::Client {
        let mut g = self.inner.lock().unwrap();
        // 与 client_for("direct") 同配置（pool_max_idle_per_host/keepalive），避免池参数分裂（review P3）
        g.entry("direct".to_string())
            .or_insert_with(|| Self::build_client(None))
            .clone()
    }
}

/// `no_proxy` 匹配：命中 `lists` 中任一列表的任一规则（CIDR/域名/`*` 后缀通配）→ 返回 true（应直连）。
pub fn no_proxy_match(lists: &[Vec<String>], url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or_default();
    if host.is_empty() {
        return false;
    }
    lists.iter().any(|list| list.iter().any(|pat| no_proxy_pattern_match(pat, host)))
}

fn no_proxy_pattern_match(pattern: &str, host: &str) -> bool {
    let pat = pattern.trim();
    if pat.is_empty() {
        return false;
    }
    if pat.contains('/') {
        return cidr_match(host, pat);
    }
    if pat.contains('*') {
        return crate::routing::pattern_matches(pat, host);
    }
    // 域名：精确或子域
    let domain = pat.strip_prefix('.').unwrap_or(pat);
    let h = host.to_ascii_lowercase();
    let d = domain.to_ascii_lowercase();
    h == d || h.ends_with(&format!(".{d}"))
}

fn cidr_match(host: &str, cidr: &str) -> bool {
    let Ok(host_ip) = host.parse::<IpAddr>() else {
        return false;
    };
    let Some((net_str, prefix_str)) = cidr.split_once('/') else {
        return false;
    };
    let Ok(prefix) = prefix_str.parse::<u32>() else {
        return false;
    };
    let Ok(net_ip) = net_str.parse::<IpAddr>() else {
        return false;
    };
    match (host_ip, net_ip) {
        (IpAddr::V4(h), IpAddr::V4(n)) => {
            if prefix > 32 {
                return false;
            }
            let mask = if prefix == 0 { 0u32 } else { u32::MAX << (32 - prefix) };
            (u32::from(h) & mask) == (u32::from(n) & mask)
        }
        (IpAddr::V6(h), IpAddr::V6(n)) => {
            if prefix > 128 {
                return false;
            }
            ipv6_prefix_match(&h.octets(), &n.octets(), prefix)
        }
        _ => false,
    }
}

fn ipv6_prefix_match(h: &[u8; 16], n: &[u8; 16], prefix: u32) -> bool {
    let full_bytes = (prefix / 8) as usize;
    let rem_bits = (prefix % 8) as u32;
    if h[..full_bytes] != n[..full_bytes] {
        return false;
    }
    if rem_bits > 0 {
        let mask = (0xffu8) << (8 - rem_bits);
        if h[full_bytes] & mask != n[full_bytes] & mask {
            return false;
        }
    }
    true
}

/// 协议 → 请求路径（相对 base_url，base_url 末尾斜杠已去除）。
/// gemini：/models/{model}:generateContent 或 :streamGenerateContent?alt=sse（model 用上游模型名）。
pub fn endpoint_path(p: Protocol, model: &str, stream: bool) -> String {
    match p {
        Protocol::OpenaiChat => "/chat/completions".to_string(),
        Protocol::OpenaiResponses => "/responses".to_string(),
        Protocol::Anthropic => "/messages".to_string(),
        Protocol::Gemini => {
            // 模型名作路径段需 URL 编码，防自定义模型名含 ?/&/# 污染查询串（review P3）
            let enc = encode_path_segment(model);
            if stream {
                format!("/models/{enc}:streamGenerateContent?alt=sse")
            } else {
                format!("/models/{enc}:generateContent")
            }
        }
    }
}

/// URL 路径段百分号编码：保留 unreserved 与安全字符，其余 %XX。
fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b':' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

/// 应用协议鉴权头：
/// - openai_chat/openai_responses：Authorization: Bearer {key}
/// - anthropic：x-api-key + anthropic-version: 2023-06-01
/// - gemini：x-goog-api-key
/// api_key 为 None 时不设置（部分自建上游无需鉴权）。
pub fn apply_auth(headers: &mut reqwest::header::HeaderMap, p: Protocol, api_key: Option<&str>) {
    let Some(key) = api_key else { return };
    match p {
        Protocol::OpenaiChat | Protocol::OpenaiResponses => {
            if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")) {
                headers.insert(reqwest::header::AUTHORIZATION, v);
            }
        }
        Protocol::Anthropic => {
            if let Ok(v) = reqwest::header::HeaderValue::from_str(key) {
                headers.insert("x-api-key", v);
            }
            if let Ok(v) = reqwest::header::HeaderValue::from_str("2023-06-01") {
                headers.insert("anthropic-version", v);
            }
        }
        Protocol::Gemini => {
            if let Ok(v) = reqwest::header::HeaderValue::from_str(key) {
                headers.insert("x-goog-api-key", v);
            }
        }
    }
}

/// 透传改写（就地修改 body）：
/// 1. 模型名重写（Gemini 在 URL 不在 body，跳过）；
/// 2. 请求体 add/set 覆盖（支持嵌套路径 "a.b.c"；set 覆盖已有、add 仅缺失时写入）。
pub fn rewrite_body(body: &mut serde_json::Value, upstream_model: Option<&str>, overrides: Option<&Overrides>) {
    if let Some(m) = upstream_model {
        body["model"] = serde_json::Value::String(m.to_string());
    }
    let Some(o) = overrides else { return };
    for (k, v) in &o.body.set {
        set_nested(body, k, v.clone());
    }
    for (k, v) in &o.body.add {
        add_nested(body, k, v.clone());
    }
}

fn set_nested(root: &mut serde_json::Value, path: &str, value: serde_json::Value) {
    let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return;
    }
    set_nested_segs(root, &segs, value);
}

fn set_nested_segs(cur: &mut serde_json::Value, segs: &[&str], value: serde_json::Value) {
    if segs.is_empty() {
        return;
    }
    // 数字路径段 = 数组索引（review P2-4：原实现把数组强转对象破坏请求结构）
    let next_is_index = segs.get(1).map_or(false, |s| s.parse::<usize>().is_ok());

    if segs.len() == 1 {
        match cur {
            serde_json::Value::Object(obj) => {
                obj.insert(segs[0].to_string(), value);
            }
            serde_json::Value::Array(arr) => {
                if let Ok(i) = segs[0].parse::<usize>() {
                    if i < arr.len() {
                        arr[i] = value;
                    } else {
                        tracing::warn!("body 覆盖路径 {segs:?} 数组越界，忽略该键");
                    }
                }
            }
            _ => {}
        }
        return;
    }

    match cur {
        serde_json::Value::Object(obj) => {
            let child = match obj.entry(segs[0].to_string()) {
                serde_json::map::Entry::Occupied(o) => o.into_mut(),
                serde_json::map::Entry::Vacant(v) => {
                    v.insert(if next_is_index {
                        serde_json::Value::Array(Default::default())
                    } else {
                        serde_json::Value::Object(Default::default())
                    })
                }
            };
            if child.is_array() && !next_is_index {
                // 现存数组上无法用非索引键写入 → 忽略，绝不把数组转成对象（review P2-4）
                tracing::warn!("body 覆盖路径 `{}` 落在数组上且非索引段，忽略该键", segs[0]);
                return;
            }
            // 容器类型与下一段语义对齐（标量 → 容器只在确有路径时替换）
            if next_is_index && !child.is_array() {
                *child = serde_json::Value::Array(Default::default());
            } else if !next_is_index && !child.is_object() && !child.is_array() {
                *child = serde_json::Value::Object(Default::default());
            }
            set_nested_segs(child, &segs[1..], value);
        }
        serde_json::Value::Array(arr) => {
            // 路径落在数组上：首段必须是索引，否则语义不明 → 忽略不破坏
            if let Ok(i) = segs[0].parse::<usize>() {
                if i >= arr.len() {
                    // 扩容到索引位；新元素用与下一段语义一致的默认容器（保证数组元素同构）
                    let fill = if next_is_index {
                        serde_json::Value::Array(Default::default())
                    } else {
                        serde_json::Value::Object(Default::default())
                    };
                    arr.resize_with(i + 1, || fill.clone());
                }
                set_nested_segs(&mut arr[i], &segs[1..], value);
            } else {
                tracing::warn!("body 覆盖路径 `{}` 落在数组上且非索引段，忽略该键", segs[0]);
            }
        }
        _ => {}
    }
}

fn add_nested(root: &mut serde_json::Value, path: &str, value: serde_json::Value) {
    let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() || path_exists(root, &segs) {
        return;
    }
    set_nested_segs(root, &segs, value);
}

fn path_exists(cur: &serde_json::Value, segs: &[&str]) -> bool {
    if segs.is_empty() {
        return true;
    }
    let Some(next) = cur.get(segs[0]) else {
        return false;
    };
    path_exists(next, &segs[1..])
}

/// 请求头 add/set 覆盖（headers 为即将发往上游的头）。
pub fn apply_header_overrides(headers: &mut reqwest::header::HeaderMap, overrides: Option<&Overrides>) {
    let Some(o) = overrides else { return };
    for (k, v) in &o.headers.set {
        apply_header_one(headers, k, v, true);
    }
    for (k, v) in &o.headers.add {
        apply_header_one(headers, k, v, false);
    }
}

fn apply_header_one(headers: &mut reqwest::header::HeaderMap, name: &str, value: &serde_json::Value, overwrite: bool) {
    let Ok(hname) = reqwest::header::HeaderName::from_bytes(name.as_bytes()) else {
        tracing::warn!("无效请求头名 `{name}` 跳过覆盖");
        return;
    };
    let value_str = header_value_str(value);
    let Ok(hval) = reqwest::header::HeaderValue::from_str(&value_str) else {
        tracing::warn!("无效请求头值 `{name}` 跳过覆盖");
        return;
    };
    if overwrite || !headers.contains_key(&hname) {
        headers.insert(hname, hval);
    }
}

fn header_value_str(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 非流式上游调用：返回 (status, body JSON)。非 2xx → Err(Status)。
pub async fn execute_nonstream(
    client: &reqwest::Client,
    url: &str,
    headers: reqwest::header::HeaderMap,
    body: &serde_json::Value,
    timeout_ms: i32,
) -> Result<serde_json::Value, UpstreamError> {
    let resp = post_json(client, url, headers, body, timeout_ms).await?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| UpstreamError::BodyRead(e.to_string()))?;
    if !status.is_success() {
        return Err(UpstreamError::Status(status.as_u16(), truncate_text(&bytes, 2048)));
    }
    serde_json::from_slice(&bytes).map_err(|e| UpstreamError::BodyRead(e.to_string()))
}

/// 流式上游调用：建立 SSE 连接，返回响应（调用方随后消费 body bytes_stream）。
/// 首字节前的失败表现为本函数返回 Err（可重试/故障转移）；
/// 首字节后的失败由流自然断流表达（PLAN §4.4 流式错误边界）。
pub async fn execute_stream(
    client: &reqwest::Client,
    url: &str,
    headers: reqwest::header::HeaderMap,
    body: &serde_json::Value,
    timeout_ms: i32,
) -> Result<reqwest::Response, UpstreamError> {
    let resp = post_json(client, url, headers, body, timeout_ms).await?;
    let status = resp.status();
    if !status.is_success() {
        let bytes = resp.bytes().await.map_err(|e| UpstreamError::BodyRead(e.to_string()))?;
        return Err(UpstreamError::Status(status.as_u16(), truncate_text(&bytes, 2048)));
    }
    Ok(resp)
}

async fn post_json(
    client: &reqwest::Client,
    url: &str,
    headers: reqwest::header::HeaderMap,
    body: &serde_json::Value,
    timeout_ms: i32,
) -> Result<reqwest::Response, UpstreamError> {
    let mut req = client.post(url).headers(headers).json(body);
    if timeout_ms > 0 {
        req = req.timeout(Duration::from_millis(timeout_ms as u64));
    }
    match req.send().await {
        Ok(resp) => Ok(resp),
        Err(e) if e.is_timeout() => Err(UpstreamError::Timeout),
        Err(e) if e.is_connect() => Err(UpstreamError::Connect(e.to_string())),
        Err(e) => Err(UpstreamError::Connect(e.to_string())),
    }
}

fn truncate_text(bytes: &[u8], max_bytes: usize) -> String {
    let len = bytes.len();
    if len <= max_bytes {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut end = max_bytes;
    // 避免切开 UTF-8 码点
    while end > 0 && (bytes[end - 1] & 0xC0) == 0x80 {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// 改写单条 SSE data 载荷并重新编码为 SSE 事件。
/// - "[DONE]" 原样放行；JSON 解析成功则改写顶层 `model`（存在才改）与 `id`（存在才改）；
/// - 解析失败原样放行。
pub fn rewrite_sse_payload(data: &str, model: &str, request_id: &str) -> String {
    if data.trim() == "[DONE]" {
        return encode_event(data);
    }
    match serde_json::from_str::<serde_json::Value>(data) {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                if obj.contains_key("model") {
                    obj.insert("model".to_string(), serde_json::Value::String(model.to_string()));
                }
                if obj.contains_key("id") {
                    obj.insert("id".to_string(), serde_json::Value::String(request_id.to_string()));
                }
            }
            encode_event(&v.to_string())
        }
        Err(_) => encode_event(data),
    }
}

/// 透传 SSE 改写流：把上游字节流解析为 SSE 事件，改写每个 chunk JSON 的
/// `model`（回写网关模型名）与 `id`（回写 request_id），其余字段原样透传；
/// 单行解析失败该行原样放行，不中断流。
pub fn sse_passthrough_stream(
    resp: reqwest::Response,
    gateway_model: String,
    request_id: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    let state = PassthroughState {
        stream: Box::pin(resp.bytes_stream()),
        parser: SseParser::new(),
        queue: VecDeque::new(),
        done: false,
        gateway_model,
        request_id,
    };
    stream::unfold(state, |mut st| async move {
        while st.queue.is_empty() && !st.done {
            match st.stream.next().await {
                Some(Ok(bytes)) => {
                    for ev in st.parser.feed(&bytes) {
                        let out = rewrite_sse_payload(&ev, &st.gateway_model, &st.request_id);
                        st.queue.push_back(Ok(Bytes::from(out.into_bytes())));
                    }
                }
                Some(Err(e)) => {
                    st.done = true;
                    for ev in st.parser.finish() {
                        let out = rewrite_sse_payload(&ev, &st.gateway_model, &st.request_id);
                        st.queue.push_back(Ok(Bytes::from(out.into_bytes())));
                    }
                    st.queue.push_back(Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())));
                }
                None => {
                    st.done = true;
                    for ev in st.parser.finish() {
                        let out = rewrite_sse_payload(&ev, &st.gateway_model, &st.request_id);
                        st.queue.push_back(Ok(Bytes::from(out.into_bytes())));
                    }
                }
            }
        }
        st.queue.pop_front().map(|item| (item, st))
    })
}

struct PassthroughState {
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>>,
    parser: SseParser,
    queue: VecDeque<Result<Bytes, std::io::Error>>,
    done: bool,
    gateway_model: String,
    request_id: String,
}

/// 转换 SSE 流：上游协议事件 → IR chunk → 入口协议事件编码。
/// 降级项无法跨流回传响应头（首字节已发），只记 warning 日志。
pub fn sse_convert_stream(
    resp: reqwest::Response,
    from: Protocol,
    to: Protocol,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    let state = ConvertState {
        stream: Box::pin(resp.bytes_stream()),
        parser: SseParser::new(),
        queue: VecDeque::new(),
        done: false,
        from,
        to,
        from_state: AnyStreamState::new(from),
        to_state: AnyStreamState::new(to),
    };
    stream::unfold(state, |mut st| async move {
        while st.queue.is_empty() && !st.done {
            match st.stream.next().await {
                Some(Ok(bytes)) => {
                    for ev in st.parser.feed(&bytes) {
                        process_convert_frame(&mut st, &ev);
                    }
                }
                Some(Err(e)) => {
                    st.done = true;
                    for ev in st.parser.finish() {
                        process_convert_frame(&mut st, &ev);
                    }
                    st.queue.push_back(Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())));
                }
                None => {
                    st.done = true;
                    for ev in st.parser.finish() {
                        process_convert_frame(&mut st, &ev);
                    }
                    // 终止事件（OpenAI [DONE] / Anthropic message_stop / Responses completed / Gemini 无）
                    let mut ctx = ConvCtx::new();
                    match stream_end(st.to, &mut st.to_state, &mut ctx) {
                        Ok(events) => {
                            for ev in events {
                                st.queue.push_back(Ok(Bytes::from(encode_event(&ev).into_bytes())));
                            }
                        }
                        Err(e) => tracing::warn!("转换终止事件失败: {e}"),
                    }
                    log_degraded(&ctx);
                }
            }
        }
        st.queue.pop_front().map(|item| (item, st))
    })
}

struct ConvertState {
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>>,
    parser: SseParser,
    queue: VecDeque<Result<Bytes, std::io::Error>>,
    done: bool,
    from: Protocol,
    to: Protocol,
    from_state: AnyStreamState,
    to_state: AnyStreamState,
}

fn process_convert_frame(st: &mut ConvertState, data: &str) {
    let mut ctx = ConvCtx::new();
    match chunk_to_ir(st.from, data, &mut st.from_state, &mut ctx) {
        Ok(Some(chunk)) => match chunk_from_ir(st.to, &chunk, &mut st.to_state, &mut ctx) {
            Ok(events) => {
                for ev in events {
                    st.queue.push_back(Ok(Bytes::from(encode_event(&ev).into_bytes())));
                }
            }
            Err(e) => tracing::warn!("转换帧写 IR→输出失败: {e}"),
        },
        Ok(None) => {}
        Err(e) => tracing::warn!("转换帧解析失败: {e}"),
    }
    log_degraded(&ctx);
}

fn log_degraded(ctx: &ConvCtx) {
    if !ctx.degraded.is_empty() {
        tracing::warn!("转换降级字段: {}", ctx.degraded_header().unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{Overrides, ProxyRow};
    use crate::protocol::ir::Protocol;
    use reqwest::header::HeaderMap;
    use serde_json::json;

    #[test]
    fn retryable_timeout_connect_always_true() {
        assert!(UpstreamError::Timeout.retryable(&[]));
        assert!(UpstreamError::Connect("x".into()).retryable(&[]));
    }

    #[test]
    fn retryable_status_uses_codes() {
        let codes = [429, 500, 502, 503];
        assert!(UpstreamError::Status(429, "".into()).retryable(&codes));
        assert!(UpstreamError::Status(503, "".into()).retryable(&codes));
        assert!(!UpstreamError::Status(400, "".into()).retryable(&codes));
        assert!(!UpstreamError::BodyRead("x".into()).retryable(&codes));
    }

    #[test]
    fn endpoint_path_variant() {
        assert_eq!(endpoint_path(Protocol::OpenaiChat, "m", false), "/chat/completions");
        assert_eq!(endpoint_path(Protocol::OpenaiResponses, "m", false), "/responses");
        assert_eq!(endpoint_path(Protocol::Anthropic, "m", false), "/messages");
        assert_eq!(endpoint_path(Protocol::Gemini, "m", false), "/models/m:generateContent");
        assert_eq!(endpoint_path(Protocol::Gemini, "m", true), "/models/m:streamGenerateContent?alt=sse");
    }

    #[test]
    fn set_nested_preserves_arrays_by_index() {
        // review P2-4：数组索引路径不再把数组强转对象
        let mut body = serde_json::json!({
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "ok"}
            ],
            "temperature": 1.0
        });
        set_nested(&mut body, "messages.0.content", serde_json::json!("hello"));
        assert_eq!(body["messages"][0]["content"], "hello");
        assert!(body["messages"].is_array());
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);

        // 数组不存在时按索引创建（元素用对象容器）
        let mut body2 = serde_json::json!({});
        set_nested(&mut body2, "items.2.name", serde_json::json!("x"));
        assert_eq!(body2["items"].as_array().unwrap().len(), 3);
        assert_eq!(body2["items"][2]["name"], "x");
        assert!(body2["items"][0].is_object());

        // 现存数组上使用非索引键 → 忽略且不破坏
        let mut body3 = serde_json::json!({ "messages": [{ "role": "user" }] });
        set_nested(&mut body3, "messages.role", serde_json::json!("system"));
        assert!(body3["messages"].is_array());
        assert_eq!(body3["messages"][0]["role"], "user");

        // 深层：数组中元素内嵌数组
        let mut body4 = serde_json::json!({ "list": [[1, 2], [3]] });
        set_nested(&mut body4, "list.1.0", serde_json::json!(9));
        assert_eq!(body4["list"][1][0], 9);
    }

    #[test]
    fn apply_auth_openai() {
        let mut h = HeaderMap::new();
        apply_auth(&mut h, Protocol::OpenaiChat, Some("sk-1"));
        assert_eq!(h.get("authorization").unwrap().to_str().unwrap(), "Bearer sk-1");
        apply_auth(&mut h, Protocol::OpenaiResponses, Some("sk-2"));
        assert_eq!(h.get("authorization").unwrap().to_str().unwrap(), "Bearer sk-2");
    }

    #[test]
    fn apply_auth_anthropic() {
        let mut h = HeaderMap::new();
        apply_auth(&mut h, Protocol::Anthropic, Some("key"));
        assert_eq!(h.get("x-api-key").unwrap().to_str().unwrap(), "key");
        assert_eq!(h.get("anthropic-version").unwrap().to_str().unwrap(), "2023-06-01");
    }

    #[test]
    fn apply_auth_gemini() {
        let mut h = HeaderMap::new();
        apply_auth(&mut h, Protocol::Gemini, Some("gkey"));
        assert_eq!(h.get("x-goog-api-key").unwrap().to_str().unwrap(), "gkey");
    }

    #[test]
    fn apply_auth_none_key_skips() {
        let mut h = HeaderMap::new();
        apply_auth(&mut h, Protocol::OpenaiChat, None);
        apply_auth(&mut h, Protocol::Gemini, None);
        assert!(h.is_empty());
    }

    #[test]
    fn rewrite_body_model_and_set() {
        let mut b = json!({ "model": "old", "temperature": 0 });
        let overrides: Overrides = serde_json::from_value(json!({
            "body": { "set": { "a": 1, "b.c": "x", "d.e.f": [1, 2] } }
        })).unwrap();
        rewrite_body(&mut b, Some("new-model"), Some(&overrides));
        assert_eq!(b["model"], json!("new-model"));
        assert_eq!(b["a"], json!(1));
        assert_eq!(b["b"]["c"], json!("x"));
        assert_eq!(b["d"]["e"]["f"], json!([1, 2]));
    }

    #[test]
    fn rewrite_body_set_overwrites_nested() {
        let mut b = json!({ "b": { "c": "old" } });
        let overrides: Overrides = serde_json::from_value(json!({
            "body": { "set": { "b.c": "new" } }
        })).unwrap();
        rewrite_body(&mut b, None, Some(&overrides));
        assert_eq!(b["b"]["c"], json!("new"));
    }

    #[test]
    fn rewrite_body_add_only_when_missing() {
        let mut b = json!({ "a": 1, "b": { "c": 2 } });
        let overrides: Overrides = serde_json::from_value(json!({
            "body": { "add": { "a": 99, "b.c": 99, "x.y": 5 } }
        })).unwrap();
        rewrite_body(&mut b, None, Some(&overrides));
        // 已存在 → 不改
        assert_eq!(b["a"], json!(1));
        assert_eq!(b["b"]["c"], json!(2));
        // 缺失 → 写
        assert_eq!(b["x"]["y"], json!(5));
    }

    #[test]
    fn apply_header_overrides_set_and_add() {
        let mut h = HeaderMap::new();
        h.insert("x-existing", "v".parse().unwrap());
        let overrides: Overrides = serde_json::from_value(json!({
            "headers": {
                "set": { "x-existing": "n", "x-new": "y" },
                "add": { "x-existing": "ignored", "x-only-add": "z" }
            }
        })).unwrap();
        apply_header_overrides(&mut h, Some(&overrides));
        assert_eq!(h.get("x-existing").unwrap().to_str().unwrap(), "n");
        assert_eq!(h.get("x-new").unwrap().to_str().unwrap(), "y");
        assert_eq!(h.get("x-only-add").unwrap().to_str().unwrap(), "z");
    }

    #[test]
    fn apply_header_overrides_bad_name_skipped() {
        let mut h = HeaderMap::new();
        let overrides: Overrides = serde_json::from_value(json!({
            "headers": { "set": { "bad name": "x", "ok": "1" } }
        })).unwrap();
        apply_header_overrides(&mut h, Some(&overrides));
        assert!(h.get("bad name").is_none());
        assert_eq!(h.get("ok").unwrap().to_str().unwrap(), "1");
    }

    #[test]
    fn no_proxy_match_forms() {
        // CIDR
        assert!(no_proxy_match(&[vec!["10.0.0.0/8".into()]], "http://10.1.2.3/x"));
        assert!(!no_proxy_match(&[vec!["10.0.0.0/8".into()]], "http://192.168.1.1/x"));
        // 域名（含子域）
        assert!(no_proxy_match(&[vec!["example.com".into()]], "https://api.example.com/x"));
        assert!(no_proxy_match(&[vec!["example.com".into()]], "https://example.com/x"));
        assert!(!no_proxy_match(&[vec!["example.com".into()]], "https://badexample.com/x"));
        // * 后缀通配
        assert!(no_proxy_match(&[vec!["*.example.com".into()]], "https://a.example.com/x"));
        assert!(!no_proxy_match(&[vec!["*.example.com".into()]], "https://example.com/x"));
        // 多列表取并集
        assert!(no_proxy_match(&[vec!["a.com".into()], vec!["b.com".into()]], "https://sub.b.com/x"));
    }

    #[test]
    fn rewrite_sse_payload_core() {
        // 模型存在 → 改写；id 存在 → 改写
        let out = rewrite_sse_payload(r#"{"model":"m1","id":"r1","choices":[]}"#, "gm", "rid");
        assert!(out.starts_with("data: "));
        let v: serde_json::Value = serde_json::from_str(out.trim_start_matches("data: ").trim_end()).unwrap();
        assert_eq!(v["model"], json!("gm"));
        assert_eq!(v["id"], json!("rid"));
        // 无 model/id → 不改
        let out2 = rewrite_sse_payload(r#"{"choices":[]}"#, "gm", "rid");
        let v2: serde_json::Value = serde_json::from_str(out2.trim_start_matches("data: ").trim_end()).unwrap();
        assert!(v2.get("model").is_none());
        assert!(v2.get("id").is_none());
        // [DONE] 原样
        assert_eq!(rewrite_sse_payload("[DONE]", "gm", "rid"), "data: [DONE]\n\n");
        // 解析失败原样放行
        assert_eq!(rewrite_sse_payload("not-json", "gm", "rid"), "data: not-json\n\n");
    }

    #[test]
    fn client_for_direct_without_proxy() {
        let pools = ClientPools::new();
        let mut up = make_upstream();
        up.use_proxy = false;
        let snap = Snapshot::default();
        let c = pools.client_for(&up, &snap, "");
        // 直连：无代理也能构建
        let _ = c.post("http://127.0.0.1:1").send();
    }

    #[test]
    fn client_for_proxy_by_id() {
        let pools = ClientPools::new();
        let pid = Uuid::new_v4();
        let proxy = ProxyRow {
            id: pid,
            name: "p".into(),
            kind: "http".into(),
            host: "127.0.0.1".into(),
            port: 8080,
            username: None,
            password_enc: None,
            password_plain: None,
            no_proxy: vec![],
            enabled: true,
        };
        let mut snap = Snapshot::default();
        snap.proxies.insert(pid, proxy);
        let mut up = make_upstream();
        up.use_proxy = true;
        up.proxy_id = Some(pid);
        let c = pools.client_for(&up, &snap, "");
        let _ = c.post("http://127.0.0.1:1").send();
    }

    #[test]
    fn client_for_proxy_fallback_direct() {
        let pools = ClientPools::new();
        let mut up = make_upstream();
        up.use_proxy = true;
        up.proxy_id = Some(Uuid::new_v4()); // 不存在
        let snap = Snapshot::default();
        let c = pools.client_for(&up, &snap, "");
        // 构建直连客户端（关键是不 panic）
        let _ = c.post("http://127.0.0.1:1").send();
    }

    fn make_upstream() -> UpstreamRow {
        UpstreamRow {
            id: Uuid::new_v4(),
            name: "up".into(),
            kind: "openai".into(),
            base_url: "http://127.0.0.1".into(),
            api_key_plain: None,
            protocols: vec!["openai_chat".into()],
            enabled: true,
            timeout_ms: 30000,
            breaker_threshold: 5,
            probe_model: None,
            consecutive_failures: 0,
            disabled_by: None,
            cooldown_until: None,
            use_proxy: false,
            proxy_id: None,
            extra: json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }
}
