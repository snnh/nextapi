//! Codex Responses-over-WebSocket 传输（官方 CLI 首选传输，HTTP SSE 为其回落）。
//!
//! 协议口径（对齐 openai/codex `codex-rs/codex-api`）：
//! - 握手：`{ws|wss}://{base}/responses`，头 = 鉴权 + `openai-beta: responses_websockets=…`
//!   + 指纹模拟头（见 `codex.rs`）；服务端返回头 `x-codex-turn-state` / `openai-model`；
//! - 请求：单帧文本 `{"type":"response.create", …Responses 请求体…}`；网关每请求新建连接、
//!   发送完整 input（无 `previous_response_id` 增量复用，保持无状态）；
//! - 响应：文本帧为 Responses 事件 JSON（与 HTTP SSE 的 data 载荷同构）与 `codex.*`
//!   元事件；`{"type":"error",…}` 为带内错误帧。本模块把事件重编码为 SSE 字节流，
//!   复用网关既有的透传/转换/打点链路；
//! - 失败边界：握手/首个事件前失败 → `Err`（网关可重试/故障转移，auto 模式回落 HTTP）；
//!   首事件后失败 → SSE 带内 `error` 事件 + 断流（与 HTTP 流式错误边界一致）。
//!
//! 限制：v1 不走代理（上游绑定代理时由网关直接选择 HTTP 传输）。

use std::collections::VecDeque;
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async_tls_with_config, MaybeTlsStream, WebSocketStream};

use super::{ByteStream, UpstreamError};

/// 首个事件等待窗口：窗口内收到错误帧即提前失败（回落/故障转移）；超时按握手成功处理。
pub const FIRST_EVENT_TIMEOUT: Duration = Duration::from_millis(1500);
/// 未配置上游超时时的连接/握手超时。
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// WS 请求体里记录请求开始时间的 client_metadata 键（官方客户端同名）。
pub const WS_START_MS_KEY: &str = "x-codex-ws-stream-request-start-ms";

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// WS 请求参数。
#[derive(Debug, Clone)]
pub struct WsRequest {
    /// `{ws|wss}://…/responses`
    pub url: String,
    pub headers: reqwest::header::HeaderMap,
    /// `response.create` 载荷（已过 `codex::prepare_body`）。
    pub body: Value,
    pub connect_timeout: Option<Duration>,
    /// 读空闲超时（None = 不设）。
    pub idle_timeout: Option<Duration>,
    /// 首个事件窗口（仅流式路径使用）。
    pub first_event_timeout: Duration,
}

impl WsRequest {
    pub fn new(url: String, headers: reqwest::header::HeaderMap, body: Value) -> Self {
        Self {
            url,
            headers,
            body,
            connect_timeout: Some(DEFAULT_CONNECT_TIMEOUT),
            idle_timeout: None,
            first_event_timeout: FIRST_EVENT_TIMEOUT,
        }
    }
}

/// base_url → WS URL（http→ws / https→wss；已是 ws/wss 原样；其他 scheme 不支持）。
pub fn ws_url(base_url: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(base_url).ok()?;
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        "ws" | "wss" => return Some(url.to_string()),
        _ => return None,
    };
    url.set_scheme(scheme).ok()?;
    Some(url.to_string())
}

/// WS 请求体：补 `type=response.create` 与请求开始时间（官方客户端同字段）。
pub fn prepare_ws_body(body: &Value, start_ms: i64) -> Value {
    let mut b = body.clone();
    let Some(o) = b.as_object_mut() else {
        return b;
    };
    o.insert("type".into(), Value::String("response.create".to_string()));
    let md = o
        .entry("client_metadata")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Some(m) = md.as_object_mut() {
        m.entry(WS_START_MS_KEY.to_string())
            .or_insert_with(|| Value::String(start_ms.to_string()));
    }
    b
}

/// 带内错误帧判定：`{"type":"error", …}`。
fn is_error_frame(v: &Value) -> bool {
    v.get("type").and_then(|t| t.as_str()) == Some("error")
}

/// 错误帧 HTTP 状态（官方形态 `{"status":429,…}`）；缺失按 502（上游错误）处理，
/// 仅连接类问题才用 Connect，避免把带内错误错记为可无脑重试的连接失败。
fn error_frame_status(v: &Value) -> u16 {
    v.get("status")
        .and_then(|s| s.as_u64())
        .filter(|s| (100..=599).contains(s))
        .map(|s| s as u16)
        .unwrap_or(502)
}

/// 错误帧错误码：`error.code` → `error.type` → `code`。
fn error_frame_code(v: &Value) -> Option<String> {
    let inner = v.get("error");
    inner
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_str())
        .or_else(|| inner.and_then(|e| e.get("type")).and_then(|t| t.as_str()))
        .or_else(|| v.get("code").and_then(|c| c.as_str()))
        .map(str::to_string)
}

/// 错误帧可读信息：`error.message` → `message` → 原始载荷截断。
fn error_frame_message(v: &Value, raw: &str) -> String {
    let msg = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .or_else(|| v.get("message").and_then(|m| m.as_str()))
        .unwrap_or(raw);
    msg.chars().take(2048).collect()
}

/// 错误帧 → UpstreamError（首个事件窗口内使用，供重试/回落判定）。
fn error_frame_to_upstream(v: &Value, raw: &str) -> UpstreamError {
    UpstreamError::Status(error_frame_status(v), error_frame_message(v, raw))
}

/// 事件帧 →（SSE 字节, 是否终止帧）：普通事件按 `type` 补 `event:` 行；
/// 错误帧转 SSE `error` 事件；返回的 bool 供流结束判定（`response.completed`）。
pub fn frame_to_sse(text: &str) -> Option<(Bytes, bool)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let completed = v.get("type").and_then(|t| t.as_str()) == Some("response.completed");
    let data = if is_error_frame(&v) {
        let mut payload = serde_json::Map::new();
        payload.insert("type".into(), Value::String("error".into()));
        if let Some(code) = error_frame_code(&v) {
            payload.insert("code".into(), Value::String(code));
        }
        payload.insert(
            "message".into(),
            Value::String(error_frame_message(&v, text)),
        );
        Value::Object(payload).to_string()
    } else {
        text.to_string()
    };
    Some((
        Bytes::from(crate::protocol::sse::encode_typed_event(&data).into_bytes()),
        completed,
    ))
}

fn map_ws_error(err: tokio_tungstenite::tungstenite::Error) -> UpstreamError {
    use tokio_tungstenite::tungstenite::Error as WsError;
    match err {
        // 握手被拒（HTTP 状态 + body）→ 与 HTTP 上游同语义的状态错误
        WsError::Http(resp) => {
            let status = resp.status().as_u16();
            let body = resp
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default();
            let body: String = body.chars().take(2048).collect();
            UpstreamError::Status(status, body)
        }
        other => UpstreamError::Connect(other.to_string()),
    }
}

/// 连接 + 发送 `response.create`（握手失败/发送失败 → Err）。
async fn connect_and_send(req: &WsRequest) -> Result<Ws, UpstreamError> {
    let mut request = req
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| UpstreamError::Connect(format!("WS 请求构造失败: {e}")))?;
    request
        .headers_mut()
        .extend(req.headers.iter().map(|(k, v)| (k.clone(), v.clone())));
    let connect = connect_async_tls_with_config(request, None, false, None);
    let (mut ws, _resp) = match req.connect_timeout {
        Some(d) => tokio::time::timeout(d, connect)
            .await
            .map_err(|_| UpstreamError::Timeout)?,
        None => connect.await,
    }
    .map_err(map_ws_error)?;

    // 兜底整形：保证载荷带 type=response.create（网关已整形时为幂等补齐）
    let body = prepare_ws_body(&req.body, chrono::Utc::now().timestamp_millis());
    let text = serde_json::to_string(&body)
        .map_err(|e| UpstreamError::BodyRead(format!("WS 请求体序列化失败: {e}")))?;
    ws.send(Message::Text(text.into()))
        .await
        .map_err(|e| UpstreamError::Connect(format!("WS 发送失败: {e}")))?;
    Ok(ws)
}

/// 读一帧（带空闲超时）：Ok(None) = 对端正常关闭。
async fn next_message(
    ws: &mut Ws,
    idle: Option<Duration>,
) -> Result<Option<Message>, UpstreamError> {
    let fut = ws.next();
    let item = match idle {
        Some(d) => tokio::time::timeout(d, fut)
            .await
            .map_err(|_| UpstreamError::Timeout)?,
        None => fut.await,
    };
    match item {
        Some(Ok(m)) => Ok(Some(m)),
        Some(Err(e)) => Err(map_ws_error(e)),
        None => Ok(None),
    }
}

/// 流式执行：握手 + 首个事件窗口判定后，返回「WS 事件 → SSE 字节」流。
pub async fn execute_stream(req: WsRequest) -> Result<ByteStream, UpstreamError> {
    let mut ws = connect_and_send(&req).await?;
    let mut pending: VecDeque<Result<Bytes, String>> = VecDeque::new();

    let start = tokio::time::Instant::now();
    let deadline = req.first_event_timeout;
    loop {
        let remaining = deadline.saturating_sub(start.elapsed());
        match tokio::time::timeout(remaining, ws.next()).await {
            // 窗口内无事件：握手已成功，交还客户端继续等待
            Err(_) => break,
            Ok(None) => {
                return Err(UpstreamError::Connect(
                    "websocket 在首个事件前关闭".to_string(),
                ))
            }
            Ok(Some(Err(e))) => return Err(map_ws_error(e)),
            Ok(Some(Ok(Message::Text(text)))) => {
                let raw = text.as_str();
                if let Ok(v) = serde_json::from_str::<Value>(raw) {
                    if is_error_frame(&v) {
                        return Err(error_frame_to_upstream(&v, raw));
                    }
                }
                if let Some((b, _)) = frame_to_sse(raw) {
                    pending.push_back(Ok(b));
                    break;
                }
            }
            Ok(Some(Ok(Message::Close(_)))) => {
                return Err(UpstreamError::Connect(
                    "websocket 在首个事件前关闭".to_string(),
                ))
            }
            Ok(Some(Ok(_))) => {}
        }
    }

    Ok(Box::pin(futures::stream::unfold(
        WsState {
            ws,
            pending,
            idle: req.idle_timeout,
            done: false,
            completed: false,
        },
        ws_stream_step,
    )))
}

/// 非流式执行：读尽事件，取 `response.completed` 内的 response 对象。
pub async fn execute_nonstream(req: WsRequest) -> Result<Value, UpstreamError> {
    let mut ws = connect_and_send(&req).await?;
    loop {
        let Some(msg) = next_message(&mut ws, req.idle_timeout).await? else {
            return Err(UpstreamError::BodyRead(
                "codex ws 流缺少 response.completed 帧".to_string(),
            ));
        };
        match msg {
            Message::Text(text) => {
                let raw = text.as_str();
                let Ok(v) = serde_json::from_str::<Value>(raw) else {
                    continue;
                };
                if is_error_frame(&v) {
                    return Err(error_frame_to_upstream(&v, raw));
                }
                if v.get("type").and_then(|t| t.as_str()) == Some("response.completed") {
                    return Ok(v.get("response").cloned().unwrap_or(v));
                }
            }
            Message::Close(_) => {
                return Err(UpstreamError::BodyRead(
                    "codex ws 在 response.completed 前关闭".to_string(),
                ))
            }
            _ => {}
        }
    }
}

struct WsState {
    ws: Ws,
    pending: VecDeque<Result<Bytes, String>>,
    idle: Option<Duration>,
    done: bool,
    completed: bool,
}

async fn ws_stream_step(mut st: WsState) -> Option<(Result<Bytes, String>, WsState)> {
    loop {
        if let Some(item) = st.pending.pop_front() {
            return Some((item, st));
        }
        if st.done {
            return None;
        }
        match next_message(&mut st.ws, st.idle).await {
            Ok(Some(Message::Text(text))) => {
                let raw = text.as_str();
                if let Some((b, completed)) = frame_to_sse(raw) {
                    st.completed |= completed;
                    st.pending.push_back(Ok(b));
                } else {
                    tracing::debug!("codex ws 非 JSON 文本帧已忽略");
                }
            }
            Ok(Some(Message::Close(_))) => {
                st.done = true;
                if !st.completed {
                    st.pending
                        .push_back(Err("codex ws 在 response.completed 前被关闭".to_string()));
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => {
                st.done = true;
                if !st.completed {
                    st.pending.push_back(Err("codex ws 连接已结束".to_string()));
                }
            }
            Err(e) => {
                st.done = true;
                st.pending.push_back(Err(e.to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_scheme_mapping() {
        assert_eq!(
            ws_url("https://chatgpt.com/backend-api/codex").as_deref(),
            Some("wss://chatgpt.com/backend-api/codex")
        );
        assert_eq!(
            ws_url("http://127.0.0.1:8080/v1").as_deref(),
            Some("ws://127.0.0.1:8080/v1")
        );
        assert_eq!(
            ws_url("wss://example.com/x").as_deref(),
            Some("wss://example.com/x")
        );
        assert_eq!(ws_url("ftp://example.com"), None);
        assert_eq!(ws_url("not a url"), None);
    }

    #[test]
    fn prepare_body_adds_type_and_start_ms() {
        let body = serde_json::json!({"model":"gpt-5-codex","stream":true,"store":false});
        let out = prepare_ws_body(&body, 1_700_000_000_000);
        assert_eq!(out["type"], "response.create");
        assert_eq!(out["client_metadata"][WS_START_MS_KEY], "1700000000000");
        // 已有 client_metadata 不覆盖既有键
        let given = serde_json::json!({
            "client_metadata": {"session_id":"s1", WS_START_MS_KEY: "1"}
        });
        let kept = prepare_ws_body(&given, 99);
        assert_eq!(kept["client_metadata"]["session_id"], "s1");
        assert_eq!(kept["client_metadata"][WS_START_MS_KEY], "1");
    }

    #[test]
    fn text_frame_encodes_sse_with_event_line() {
        let frame = r#"{"type":"response.created","response":{"id":"resp_1"}}"#;
        let (bytes, completed) = frame_to_sse(frame).unwrap();
        assert!(!completed);
        let sse = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(sse.starts_with("event: response.created\ndata: {"), "{sse}");
        assert!(sse.ends_with("\n\n"));
        let done = r#"{"type":"response.completed","response":{}}"#;
        assert!(frame_to_sse(done).unwrap().1);
    }

    #[test]
    fn error_frame_becomes_sse_error_event() {
        let frame = r#"{"type":"error","status":429,"error":{"type":"usage_limit_reached","message":"limit hit"}}"#;
        let (bytes, completed) = frame_to_sse(frame).unwrap();
        assert!(!completed, "错误帧不是终止帧");
        let sse = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(sse.starts_with("event: error\ndata: "), "{sse}");
        let data: Value =
            serde_json::from_str(sse.lines().find_map(|l| l.strip_prefix("data: ")).unwrap())
                .unwrap();
        assert_eq!(data["type"], "error");
        assert_eq!(data["code"], "usage_limit_reached");
        assert_eq!(data["message"], "limit hit");
        assert_eq!(error_frame_status(&serde_json::json!({"status": 429})), 429);
        assert_eq!(error_frame_status(&serde_json::json!({})), 502);
        assert_eq!(error_frame_status(&serde_json::json!({"status": 42})), 502);
    }

    #[test]
    fn non_json_frame_is_dropped() {
        assert!(frame_to_sse("not json").is_none());
        assert!(frame_to_sse("").is_none());
    }

    /// 本地回环 mock 服务端：读取 response.create 后按帧脚本回放（无外部依赖）。
    async fn mock_server(frames: Vec<&'static str>) -> String {
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let msg = ws.next().await.unwrap().unwrap();
            match msg {
                WsMessage::Text(t) => {
                    let v: Value = serde_json::from_str(t.as_str()).unwrap();
                    assert_eq!(v["type"], "response.create", "载荷必须带 response.create");
                }
                other => panic!("期望文本帧，收到 {other:?}"),
            }
            for f in frames {
                ws.send(WsMessage::Text(f.into())).await.unwrap();
            }
            let _ = ws.close(None).await;
        });
        format!("ws://{addr}/responses")
    }

    #[tokio::test]
    async fn stream_roundtrip_against_local_server() {
        let url = mock_server(vec![
            r#"{"type":"response.created","response":{"id":"resp_1"}}"#,
            r#"{"type":"response.output_text.delta","delta":"hi"}"#,
            r#"{"type":"response.completed","response":{"id":"resp_1","output":[]}}"#,
        ])
        .await;
        let req = WsRequest::new(
            url,
            reqwest::header::HeaderMap::new(),
            serde_json::json!({"model":"m","stream":true,"store":false}),
        );
        let mut stream = execute_stream(req).await.unwrap();
        let mut out = Vec::new();
        while let Some(item) = stream.next().await {
            out.push(item.unwrap());
        }
        let text = String::from_utf8(out.concat()).unwrap();
        assert!(text.contains("event: response.created"), "{text}");
        assert!(text.contains("event: response.output_text.delta"), "{text}");
        assert!(text.contains("event: response.completed"), "{text}");
    }

    #[tokio::test]
    async fn nonstream_aggregates_completed_frame() {
        let url = mock_server(vec![
            r#"{"type":"response.created","response":{"id":"resp_1"}}"#,
            r#"{"type":"response.completed","response":{"id":"resp_1","usage":{"input_tokens":3}}}"#,
        ])
        .await;
        let req = WsRequest::new(
            url,
            reqwest::header::HeaderMap::new(),
            serde_json::json!({"model":"m"}),
        );
        let json = execute_nonstream(req).await.unwrap();
        assert_eq!(json["id"], "resp_1");
        assert_eq!(json["usage"]["input_tokens"], 3);
    }

    #[tokio::test]
    async fn first_event_error_frame_returns_status() {
        let url = mock_server(vec![
            r#"{"type":"error","status":429,"error":{"type":"usage_limit_reached","message":"limit hit"}}"#,
        ])
        .await;
        let req = WsRequest::new(
            url,
            reqwest::header::HeaderMap::new(),
            serde_json::json!({"model":"m"}),
        );
        match execute_stream(req).await {
            Err(UpstreamError::Status(429, body)) => assert!(body.contains("limit hit"), "{body}"),
            Err(other) => panic!("期望 429 状态错误，得到 {other:?}"),
            Ok(_) => panic!("期望错误，但握手成功"),
        }
    }
}
