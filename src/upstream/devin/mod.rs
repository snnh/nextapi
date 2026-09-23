//! Devin Connect 上游传输与凭证（kind='devin'）。
//!
//! 上游为 server.codeium.com 的 Codeium Connect 协议（protobuf over HTTP）：
//! 流式 GetChatMessage 用 5 字节信封帧，unary RPC 裸 protobuf；鉴权 `Basic {tok}-{tok}`
//! （token 原文重复两遍，实测 devin CLI 形态）。出站恒流式收取，非流式入口聚合为
//! Responses JSON（同 codex 渠道策略）。字段号见 `.owc/devin-ref/RESEARCH.md`。

pub mod convert;
pub mod proto;

use std::collections::VecDeque;
use std::pin::Pin;

use bytes::{Bytes, BytesMut};
use futures::stream::{self, Stream, StreamExt};
use reqwest::header::HeaderMap;

use crate::upstream::UpstreamError;

use convert::StreamConv;
use proto::{put_msg, put_str};

/// Devin 官方 API 基址（CLI 默认）。
pub const DEFAULT_BASE_URL: &str = "https://server.codeium.com";
/// 流式聊天 RPC。
pub const CHAT_PATH: &str = "/exa.api_server_pb.ApiServerService/GetChatMessage";
/// 登录状态/模型目录 RPC（unary）。
pub const STATUS_PATH: &str = "/exa.seat_management_pb.SeatManagementService/GetUserStatus";
/// CLI PKCE code → session token（unary）。
pub const EXCHANGE_PATH: &str =
    "/exa.seat_management_pb.SeatManagementService/ExchangeDevinCLIPKCECode";

/// 上游字节流（SSE 事件流，错误以 String 表达）。
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, String>> + Send + 'static>>;

fn endpoint(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

/// CLI 形态出站头（实测）：`Authorization: Basic {tok}-{tok}` 为原文重复、非 base64。
fn connect_headers(token: Option<&str>, streaming: bool) -> HeaderMap {
    let mut h = HeaderMap::new();
    if streaming {
        h.insert(
            reqwest::header::CONTENT_TYPE,
            "application/connect+proto".parse().unwrap(),
        );
    } else {
        h.insert(
            reqwest::header::CONTENT_TYPE,
            "application/proto".parse().unwrap(),
        );
    }
    h.insert("connect-protocol-version", "1".parse().unwrap());
    h.insert(reqwest::header::ACCEPT, "*/*".parse().unwrap());
    if let Some(tok) = token {
        let v = format!("Basic {tok}-{tok}");
        if let Ok(val) = v.parse() {
            h.insert(reqwest::header::AUTHORIZATION, val);
        }
    }
    h
}

/// 极简 Metadata（GetUserStatus 等 unary RPC 请求 f1；实测 status 类客户端名为 chisel）。
fn metadata(token: &str) -> BytesMut {
    let mut m = BytesMut::new();
    put_str(&mut m, 1, "chisel");
    put_str(&mut m, 2, convert::EXTENSION_VERSION);
    put_str(&mut m, 3, token);
    put_str(&mut m, 4, "en");
    put_str(&mut m, 5, std::env::consts::OS);
    put_str(&mut m, 7, convert::EXTENSION_VERSION);
    put_str(&mut m, 12, "chisel");
    put_str(&mut m, 28, "chisel");
    m
}

/// Connect 错误体 → UpstreamError（错误映射：Free 计划误报 failed_precondition /
/// permission_denied 为模型门控，附提示；token 过期按报错处理，本期不刷新）。
fn map_error_body(status: u16, body: &[u8]) -> UpstreamError {
    let text = String::from_utf8_lossy(body);
    let (code, msg) = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            let code = v.get("code").and_then(|c| c.as_str()).unwrap_or("").to_string();
            let msg = v
                .get("message")
                .or_else(|| v.pointer("/error/message"))
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            (!code.is_empty() || !msg.is_empty()).then_some((code, msg))
        })
        .unwrap_or_default();
    let hint = match code.as_str() {
        "failed_precondition" | "permission_denied" => {
            "（提示：该模型可能未解锁于当前 Devin 套餐；Free 计划仅个别模型可用）"
        }
        "unauthenticated" | "unauthenticated_error" => {
            "（提示：session token 无效或已过期，请在上游设置中更换凭证）"
        }
        _ => "",
    };
    UpstreamError::Status(status, format!("{code}: {msg}{hint}"))
}

/// 信封帧解析器（flags(1) + len(4,BE) + payload，跨网络块拼接）。
struct FrameParser {
    buf: Vec<u8>,
}

/// 一帧信封：(flags, payload)。flags=0 数据帧；flags=2（end）帧。
type Frame = (u8, Bytes);

impl FrameParser {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn feed(&mut self, chunk: &[u8]) -> Vec<Frame> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < 5 {
                break;
            }
            let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
            if self.buf.len() < 5 + len {
                break;
            }
            let flags = self.buf[0];
            let payload = Bytes::copy_from_slice(&self.buf[5..5 + len]);
            self.buf.drain(..5 + len);
            out.push((flags, payload));
        }
        out
    }
}

/// end 帧载荷解析：返回错误消息（无错返回 None）。
fn end_frame_error(payload: &[u8]) -> Option<String> {
    if payload.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(payload).ok()?;
    if v.is_object() && v.as_object().is_some_and(|o| o.is_empty()) {
        return None;
    }
    let code = v.get("code").and_then(|c| c.as_str()).unwrap_or("");
    let msg = v
        .get("message")
        .or_else(|| v.pointer("/error/message"))
        .and_then(|m| m.as_str())
        .unwrap_or("");
    let err = v.pointer("/error");
    if code.is_empty() && msg.is_empty() && err.is_none() {
        return None;
    }
    Some(format!("{code}: {msg}"))
}

/// 发起 GetChatMessage 并返回已建立的响应（首字节前失败在这里映射）。
async fn open_chat(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    body: Bytes,
    timeout_ms: i32,
) -> Result<reqwest::Response, UpstreamError> {
    let mut req = client
        .post(endpoint(base_url, CHAT_PATH))
        .headers(connect_headers(Some(token), true))
        .body(body);
    if timeout_ms > 0 {
        req = req.timeout(std::time::Duration::from_millis(timeout_ms as u64));
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return Err(UpstreamError::Timeout),
        Err(e) => return Err(UpstreamError::Connect(e.to_string())),
    };
    let status = resp.status();
    if !status.is_success() {
        let bytes = resp.bytes().await.unwrap_or_default();
        return Err(map_error_body(status.as_u16(), &bytes));
    }
    Ok(resp)
}

/// 流式：信封帧 → 标准 Responses SSE 字节流。
///
/// 数据帧经 [`StreamConv`] 转换；end 帧携带错误时——已流出内容补
/// `response.failed`，未流出任何内容映射为可重试错误（统一熔断/转移路径）。
pub async fn execute_stream(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    body: Bytes,
    model: &str,
    timeout_ms: i32,
) -> Result<ByteStream, UpstreamError> {
    let resp = open_chat(client, base_url, token, body, timeout_ms).await?;
    let src = resp.bytes_stream().map(|r| r.map_err(|e| e.to_string()));
    Ok(frame_sse_stream(src, model.to_string()))
}

/// 把帧字节流转为 Responses SSE 流（状态机见 [`StreamConv`]）。
fn frame_sse_stream(
    src: impl Stream<Item = Result<Bytes, String>> + Send + 'static,
    model: String,
) -> ByteStream {
    struct St {
        src: Pin<Box<dyn Stream<Item = Result<Bytes, String>> + Send>>,
        parser: FrameParser,
        conv: StreamConv,
        queue: VecDeque<Result<Bytes, String>>,
        done: bool,
    }
    let st = St {
        src: Box::pin(src),
        parser: FrameParser::new(),
        conv: StreamConv::new(model),
        queue: VecDeque::new(),
        done: false,
    };
    stream::unfold(st, |mut st| async move {
        while st.queue.is_empty() && !st.done {
            match st.src.next().await {
                Some(Ok(chunk)) => {
                    for (flags, payload) in st.parser.feed(&chunk) {
                        match frame_events(&mut st.conv, flags, &payload) {
                            Ok(evs) => push_events(&mut st.queue, evs),
                            Err(msg) => {
                                st.done = true;
                                if st.conv.started() {
                                    // 流中失败：补协议内失败终止帧（response.failed）
                                    let evs = st.conv.fail(&msg);
                                    push_events(&mut st.queue, evs);
                                } else {
                                    // 未流出内容：可重试/转移
                                    st.queue.push_back(Err(msg));
                                }
                                break;
                            }
                        }
                    }
                }
                Some(Err(e)) => {
                    st.done = true;
                    if st.conv.started() {
                        // 流中断：尽力补失败终止帧（协议内表达失败而非截断）
                        let evs = st.conv.fail(&e);
                        push_events(&mut st.queue, evs);
                    } else {
                        st.queue.push_back(Err(e));
                    }
                }
                None => {
                    st.done = true;
                    let evs = st.conv.finish();
                    push_events(&mut st.queue, evs);
                }
            }
        }
        st.queue.pop_front().map(|item| (item, st))
    })
    .boxed()
}

fn push_events(queue: &mut VecDeque<Result<Bytes, String>>, evs: Vec<String>) {
    for e in evs {
        queue.push_back(Ok(Bytes::from(e.into_bytes())));
    }
}

/// 单帧处理（流式/聚合共用）：产出 SSE 事件；end 帧携带错误时返回 Err
/// （由调用方按「是否已流出内容」决定映射为失败终止帧或可重试错误）。
fn frame_events(conv: &mut StreamConv, flags: u8, payload: &[u8]) -> Result<Vec<String>, String> {
    if flags & 0x02 != 0 {
        // end 帧：{}=正常结束；带错误则上抛
        match end_frame_error(payload) {
            Some(msg) => Err(msg),
            None => Ok(conv.finish()),
        }
    } else if flags & 0x01 != 0 {
        // 压缩帧：客户端不压缩信封（实测），服务端也不应发；保守报错
        Err("不支持压缩信封帧".into())
    } else {
        conv.feed(payload)
    }
}

/// 非流式：流式收取全部帧并聚合为完整 Responses JSON（Devin 后端仅流式）。
pub async fn execute_nonstream(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    body: Bytes,
    model: &str,
    timeout_ms: i32,
) -> Result<serde_json::Value, UpstreamError> {
    let resp = open_chat(client, base_url, token, body, timeout_ms).await?;
    let mut src = Box::pin(resp.bytes_stream().map(|r| r.map_err(|e| e.to_string())));
    let mut parser = FrameParser::new();
    let mut conv = StreamConv::new(model);
    while let Some(chunk) = src.next().await {
        let frames = match chunk {
            Ok(bytes) => parser.feed(&bytes),
            Err(e) => return Err(UpstreamError::BodyRead(e)),
        };
        for (flags, payload) in frames {
            if let Err(msg) = frame_events(&mut conv, flags, &payload) {
                return Err(UpstreamError::Status(502, msg));
            }
        }
    }
    let _ = conv.finish();
    Ok(conv.final_json())
}

// ---------------------------------------------------------------------------
// unary RPC：PKCE 兑换 / 登录状态（连通性测试 + 模型目录）
// ---------------------------------------------------------------------------

/// ExchangeDevinCLIPKCECode 结果（响应 {f1 token, f2 webapp_host, f3 api_url}）。
pub struct PkceResult {
    pub token: String,
    pub webapp_host: String,
    pub api_url: String,
}

/// CLI PKCE 兑换：授权码 + code_verifier → session token（`devin-session-token$…`）。
///
/// 本渠道凭证即 session token（存 upstreams.api_key，加密 + 掩码回传）。
/// token 过期处理本期不做（过期报错，见 map_error_body）。
pub async fn exchange_pkce(
    client: &reqwest::Client,
    base_url: &str,
    code: &str,
    code_verifier: &str,
    timeout_ms: i32,
) -> Result<PkceResult, UpstreamError> {
    let mut body = BytesMut::new();
    put_str(&mut body, 1, code);
    put_str(&mut body, 2, code_verifier);
    let mut req = client
        .post(endpoint(base_url, EXCHANGE_PATH))
        .headers(connect_headers(None, false))
        .body(body.freeze());
    if timeout_ms > 0 {
        req = req.timeout(std::time::Duration::from_millis(timeout_ms as u64));
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return Err(UpstreamError::Timeout),
        Err(e) => return Err(UpstreamError::Connect(e.to_string())),
    };
    let status = resp.status();
    let bytes = resp.bytes().await.unwrap_or_default();
    if !status.is_success() {
        return Err(map_error_body(status.as_u16(), &bytes));
    }
    let fields = proto::decode(&bytes).map_err(|e| UpstreamError::BodyRead(e))?;
    let token = proto::get(&fields, 1)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if token.is_empty() {
        return Err(UpstreamError::BodyRead("兑换响应缺少 session token".into()));
    }
    Ok(PkceResult {
        token,
        webapp_host: proto::get(&fields, 2).and_then(|v| v.as_str()).unwrap_or("").to_string(),
        api_url: proto::get(&fields, 3).and_then(|v| v.as_str()).unwrap_or("").to_string(),
    })
}

/// PKCE 授权会话（无头流程：前端暂存 verifier，授权拿到 code 后交回 exchange 兑换）。
pub struct PkceStart {
    pub state: String,
    pub code_verifier: String,
    pub authorize_url: String,
}

/// 生成 CLI PKCE 授权会话（`.owc/devin-ref/RESEARCH.md` §4 实测参数）。
pub fn pkce_start() -> PkceStart {
    use base64::Engine;
    use rand::RngCore;
    use sha2::Digest;
    let state = uuid::Uuid::new_v4().to_string();
    // code_verifier：48 字节随机 → base64url（64 字符，满足 RFC 7636 的 43–128）
    let mut raw = [0u8; 48];
    rand::rng().fill_bytes(&mut raw);
    let code_verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let digest = sha2::Sha256::digest(code_verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    let authorize_url = format!(
        "{APP_AUTH_URL}?state={state}&prompt=select_account&code_challenge={challenge}\
         &code_challenge_method=S256&cli_pkce_marker=1"
    );
    PkceStart {
        state,
        code_verifier,
        authorize_url,
    }
}

/// Devin Web 授权入口（CLI PKCE）。
pub const APP_AUTH_URL: &str = "https://app.devin.ai/auth/cli/continue";

/// GetUserStatus 结果（连通性 + 模型目录 + 额度）。
#[derive(Debug, Clone)]
pub struct StatusInfo {
    pub account_id: String,
    pub email: String,
    pub plan: String,
    /// 可用模型 uid（已按计划门控过滤：带 f33 门控的条目剔除）。
    pub models: Vec<String>,
    /// 额度块（plan 块 f13 携带；缺失 → None）
    pub quota: Option<QuotaInfo>,
}

/// 额度信息（GetUserStatus 的 plan 块，字段号实测）。
///
/// - 日/周额度：`f13.f9` / `f13.f8`（实测 500 / 2500，与官方文档「日额度大于周额度
///   的 1/7」一致）；
/// - 百分比对：`f13.f14` / `f13.f15`（实测 99 / 100，即额度用量百分数）；
/// - 重置时刻：`f13.f17`（次日 08:00 UTC ≈ 太平洋日历日零点）/ `f13.f18`（次周日同时刻）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct QuotaInfo {
    pub weekly_quota: i64,
    pub daily_quota: i64,
    pub percent_value: i64,
    pub percent_total: i64,
    pub daily_reset_at: i64,
    pub weekly_reset_at: i64,
}

/// StatusInfo → 额度快照 JSON（与 codex 快照同槽位：`upstreams.extra.quota`）。
///
/// `percent_value/percent_total` 原样保留（上游语义：额度百分数对），前端按「xx/100」展示。
pub fn quota_snapshot(info: &StatusInfo) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    out.insert("kind".into(), serde_json::json!("devin"));
    out.insert(
        "captured_at".into(),
        serde_json::json!(chrono::Utc::now().timestamp()),
    );
    out.insert("source".into(), serde_json::json!("probe"));
    out.insert("plan".into(), serde_json::json!(info.plan));
    out.insert("account".into(), serde_json::json!(info.account_id));
    out.insert("email".into(), serde_json::json!(info.email));
    out.insert(
        "available_models".into(),
        serde_json::json!(info.models.len()),
    );
    if let Some(q) = info.quota {
        out.insert("daily_quota".into(), serde_json::json!(q.daily_quota));
        out.insert("weekly_quota".into(), serde_json::json!(q.weekly_quota));
        out.insert("percent_value".into(), serde_json::json!(q.percent_value));
        out.insert("percent_total".into(), serde_json::json!(q.percent_total));
        out.insert("daily_reset_at".into(), serde_json::json!(q.daily_reset_at));
        out.insert(
            "weekly_reset_at".into(),
            serde_json::json!(q.weekly_reset_at),
        );
    }
    serde_json::Value::Object(out)
}

/// 连通性探测 → 统一 JSON（admin 上游测试与预设一键接入共用同一形状）。
///
/// 无 session token（尚未授权）时返回 `ok:false` 而非报错：预设接入允许先建渠道后授权。
pub async fn probe_status(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
    timeout_ms: u64,
) -> serde_json::Value {
    let base = if base_url.trim().is_empty() {
        DEFAULT_BASE_URL
    } else {
        base_url.trim()
    };
    let started = std::time::Instant::now();
    let res = match token.map(str::trim).filter(|t| !t.is_empty()) {
        Some(tok) => user_status(client, base, tok, timeout_ms as i32).await,
        None => Err(UpstreamError::Status(
            400,
            "尚未完成 Devin 授权：请用「Devin 授权」以 Devin 账号换取会话凭证".to_string(),
        )),
    };
    let latency_ms = started.elapsed().as_millis() as u64;
    match res {
        Ok(info) => serde_json::json!({
            "ok": true,
            "status": 200,
            "latency_ms": latency_ms,
            "account": info.account_id,
            "email": info.email,
            "plan": info.plan,
            "models": info.models.len(),
            "quota": info.quota.map(|q| serde_json::json!({
                "daily_quota": q.daily_quota,
                "weekly_quota": q.weekly_quota,
                "percent_value": q.percent_value,
                "percent_total": q.percent_total,
                "daily_reset_at": q.daily_reset_at,
                "weekly_reset_at": q.weekly_reset_at,
            })),
        }),
        Err(e) => {
            serde_json::json!({ "ok": false, "latency_ms": latency_ms, "error": e.to_string() })
        }
    }
}

/// 拉登录状态与模型目录（unary）。
pub async fn user_status(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    timeout_ms: i32,
) -> Result<StatusInfo, UpstreamError> {
    let mut outer = BytesMut::new();
    let m = metadata(token);
    put_msg(&mut outer, 1, &m);
    let mut req = client
        .post(endpoint(base_url, STATUS_PATH))
        .headers(connect_headers(Some(token), false))
        .body(outer.freeze());
    if timeout_ms > 0 {
        req = req.timeout(std::time::Duration::from_millis(timeout_ms as u64));
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return Err(UpstreamError::Timeout),
        Err(e) => return Err(UpstreamError::Connect(e.to_string())),
    };
    let status = resp.status();
    let bytes = resp.bytes().await.unwrap_or_default();
    if !status.is_success() {
        return Err(map_error_body(status.as_u16(), &bytes));
    }
    let fields = proto::decode(&bytes).map_err(UpstreamError::BodyRead)?;
    let Some(status_bytes) = proto::get(&fields, 1).and_then(|v| v.as_bytes()) else {
        return Err(UpstreamError::BodyRead("响应缺少 user_status".into()));
    };
    let uf = proto::decode(status_bytes).map_err(UpstreamError::BodyRead)?;

    let plan = proto::get(&uf, 13)
        .and_then(|v| v.as_bytes())
        .and_then(|b| proto::decode(b).ok())
        // plan_info{ f1: { f2: "Free", f33: org{…} } } —— 套餐名在内层 f2
        .and_then(|pf| {
            proto::get(&pf, 1)
                .and_then(|v| v.as_bytes())
                .and_then(|b| proto::decode(b).ok())
                .and_then(|inner| {
                    proto::get(&inner, 2).and_then(|v| v.as_str()).map(String::from)
                })
        })
        .unwrap_or_default();

    // plan 块（f13）里的额度字段：f8 周额度 / f9 日额度 / f14-f15 百分比对 /
    // f17 每日重置 / f18 每周重置（epoch 秒）
    let quota = proto::get(&uf, 13)
        .and_then(|v| v.as_bytes())
        .and_then(|b| proto::decode(b).ok())
        .and_then(|pf| {
            let num = |f: u32| proto::get(&pf, f).and_then(|v| v.as_uint()).unwrap_or(0) as i64;
            let q = QuotaInfo {
                weekly_quota: num(8),
                daily_quota: num(9),
                percent_value: num(14),
                percent_total: num(15),
                daily_reset_at: num(17),
                weekly_reset_at: num(18),
            };
            // 全 0 → 上游未给额度块
            (q != QuotaInfo::default()).then_some(q)
        });

    let mut models = Vec::new();
    // f33 模型目录包装 { f1: repeated 条目(251) , f2: 展示分组, f3: 版本 }；
    // 条目 { f22: model_uid, f33: 计划门控("Upgrade to Pro…") } —— 带门控的不可用
    for wrapper in proto::get_all(&uf, 33) {
        let Some(b) = wrapper.as_bytes() else { continue };
        let Ok(wf) = proto::decode(b) else { continue };
        for entry in proto::get_all(&wf, 1) {
            let Some(eb) = entry.as_bytes() else { continue };
            let Ok(ef) = proto::decode(eb) else { continue };
            if proto::get(&ef, 33).is_some() {
                continue; // 计划门控 → 当前凭证不可用
            }
            if let Some(uid) = proto::get(&ef, 22).and_then(|v| v.as_str()) {
                models.push(uid.to_string());
            }
        }
    }

    Ok(StatusInfo {
        account_id: proto::get(&uf, 5).and_then(|v| v.as_str()).unwrap_or("").to_string(),
        email: proto::get(&uf, 7).and_then(|v| v.as_str()).unwrap_or("").to_string(),
        plan,
        models,
        quota,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_parser_across_chunks() {
        let mut p = FrameParser::new();
        let payload = b"hello";
        let mut full = vec![0u8, 0, 0, 0, 5];
        full.extend_from_slice(payload);
        // 跨两个 chunk 喂入
        assert!(p.feed(&full[..3]).is_empty());
        let frames = p.feed(&full[3..]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, 0);
        assert_eq!(&frames[0].1[..], payload);
    }

    #[test]
    fn end_frame_error_shapes() {
        assert_eq!(end_frame_error(b"{}"), None);
        assert_eq!(end_frame_error(b""), None);
        let e = end_frame_error(br#"{"code":"failed_precondition","message":"nope"}"#).unwrap();
        assert!(e.contains("failed_precondition"));
    }

    #[test]
    fn auth_header_is_plain_dup() {
        let h = connect_headers(Some("devin-session-token$abc"), true);
        assert_eq!(
            h.get(reqwest::header::AUTHORIZATION).unwrap(),
            "Basic devin-session-token$abc-devin-session-token$abc"
        );
        assert_eq!(
            h.get(reqwest::header::CONTENT_TYPE).unwrap(),
            "application/connect+proto"
        );
    }
}
