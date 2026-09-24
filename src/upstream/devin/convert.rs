//! Devin ↔ OpenAI Responses 双向转换。
//!
//! - 请求：Responses JSON body → GetChatMessageRequest protobuf（devin CLI 形态）
//! - 响应：GetChatMessageResponse 信封帧 → 标准 Responses SSE 事件 / 非流式 JSON
//!
//! 字段号全部来自一手逆向 + 真实流量实证（含 CLI exec 工具循环捕获），
//! 映射表见 `.owc/devin-ref/RESEARCH.md` §6/§7/§11。usage 口径（实测两轮自洽）：
//! f2 = 未缓存输入、f5 = 缓存命中、f3 = 输出（f6=6 常量忽略）。

use bytes::{Bytes, BytesMut};
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::proto::{self, put_double, put_msg, put_str, put_uint};
use crate::protocol::sse::{encode_typed_event, encode_typed_event_with_type};

/// f2 prompt 缺省：调用方未带 instructions 时的极简身份提示（不内嵌 CLI 默认头
/// 提示词——该提示词与 CLI 内置工具强耦合，网关透传调用方工具时不适用）。
pub const DEFAULT_PROMPT: &str =
    "You are Devin, an interactive command line agent from Cognition.";

/// 客户端标识（模拟 devin CLI 3000.11.1）。
const EXTENSION_NAME: &str = "devin-cli";
pub const EXTENSION_VERSION: &str = "3000.11.1";

// ---------------------------------------------------------------------------
// 确定性 id / 代号（同会话多次请求保持稳定，模拟 CLI 会话内轨迹/执行 id）
// ---------------------------------------------------------------------------

fn det_hash(seed: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(seed.as_bytes());
    h.finalize().into()
}

/// 由种子派生确定性 uuid 形态 id。
fn det_uuid(seed: &str) -> String {
    let d = det_hash(seed);
    let hex: String = d[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// 会话代号（形如 "glorious-orchestra"）。
fn codename(seed: &str) -> String {
    const ADJ: &[&str] = &[
        "amber", "ancient", "autumn", "bold", "brave", "bright", "calm", "clever",
        "crimson", "crystal", "curious", "dancing", "dawn", "diamond", "distant", "eager",
        "emerald", "fabled", "fancy", "gentle", "glorious", "golden", "happy", "hidden",
        "humble", "ivory", "joyful", "keen", "lively", "lucky", "lunar", "mellow",
        "misty", "noble", "proud", "quiet", "rapid", "rusty", "silent", "silver",
        "sleepy", "spring", "starry", "steady", "sunny", "velvet", "vivid", "wild",
    ];
    const NOUN: &[&str] = &[
        "anchor", "archive", "aurora", "badger", "beacon", "blossom", "breeze", "brook",
        "canyon", "cascade", "citadel", "comet", "compass", "coral", "crane", "creek",
        "delta", "dolphin", "ember", "falcon", "fjord", "forest", "fountain", "galaxy",
        "harbor", "horizon", "island", "jasper", "lantern", "marsh", "meadow", "meteor",
        "orchestra", "orchid", "otter", "pebble", "pillow", "quill", "raven", "river",
        "savanna", "solstice", "summit", "thicket", "thunder", "valley", "willow", "zephyr",
    ];
    let d = det_hash(seed);
    let a = ADJ[d[0] as usize % ADJ.len()];
    let n = NOUN[d[1] as usize % NOUN.len()];
    format!("{a}-{n}")
}

/// Metadata f31 设备指纹：随机 hex（实测 732 字符，逐请求变化）。
fn fingerprint() -> String {
    let mut raw = [0u8; 366];
    rand::rng().fill_bytes(&mut raw);
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// 请求：Responses JSON → GetChatMessageRequest
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Tc {
    call_id: String,
    name: String,
    args: String,
}

/// 归一后的会话消息（ChatMessagePrompt 三形态）。
enum Msg {
    /// source=1：user / system（CLI 的 system_info 注入也是 user 源）
    User { text: String },
    /// source=2：assistant 文本（f11）+ 工具调用（f6 repeated）
    Assistant { text: String, calls: Vec<Tc> },
    /// source=4：工具结果（f3 + f7 tool_call_id）
    Tool { call_id: String, text: String },
}

/// 取 message content 的纯文本（图片等多模态降级为占位文本）。
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut out = Vec::new();
            for p in parts {
                let ty = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match ty {
                    "input_text" | "output_text" | "text" | "summary_text" => {
                        if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                            out.push(t.to_string());
                        }
                    }
                    "input_image" | "output_image" | "image_url" | "input_file" => {
                        out.push("[图片/文件已省略：Devin 上游不支持多模态输入]".to_string());
                    }
                    _ => {}
                }
            }
            out.join("\n")
        }
        _ => String::new(),
    }
}

/// Responses `input`（string | items）→ 归一消息列表；连续 assistant 项（文本 +
/// function_call）合并为一条 ChatMessagePrompt（CLI 形态：f11 + f6 同帧）。
fn input_to_messages(input: &Value) -> Result<Vec<Msg>, String> {
    let mut out: Vec<Msg> = Vec::new();
    let mut pending: Option<(String, Vec<Tc>)> = None;

    let flush = |pending: &mut Option<(String, Vec<Tc>)>, out: &mut Vec<Msg>| {
        if let Some((text, calls)) = pending.take() {
            out.push(Msg::Assistant { text, calls });
        }
    };

    match input {
        Value::String(s) => out.push(Msg::User { text: s.clone() }),
        Value::Array(items) => {
            for item in items {
                let ty = item.get("type").and_then(|t| t.as_str()).unwrap_or("message");
                match ty {
                    "message" | "" => {
                        let role = item.get("role").and_then(|r| r.as_str()).unwrap_or("user");
                        let text = content_text(item.get("content").unwrap_or(&Value::Null));
                        if role == "assistant" {
                            pending.get_or_insert_with(Default::default).0.push_str(&text);
                        } else {
                            flush(&mut pending, &mut out);
                            out.push(Msg::User { text });
                        }
                    }
                    "function_call" => {
                        let tc = Tc {
                            call_id: item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            name: item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            args: item
                                .get("arguments")
                                .and_then(|v| v.as_str())
                                .unwrap_or("{}")
                                .to_string(),
                        };
                        pending.get_or_insert_with(Default::default).1.push(tc);
                    }
                    "function_call_output" => {
                        flush(&mut pending, &mut out);
                        let out_v = item.get("output").unwrap_or(&Value::Null);
                        let text = match out_v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        out.push(Msg::Tool {
                            call_id: item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            text,
                        });
                    }
                    // reasoning / 其它扩展项：Devin 无对应字段，跳过
                    _ => {}
                }
            }
            flush(&mut pending, &mut out);
        }
        _ => return Err("input 既非字符串也非数组".into()),
    }
    Ok(out)
}

/// 编码单条 ChatMessagePrompt。
fn encode_message(msg: &Msg, idx: usize, seed: &str) -> BytesMut {
    let mut b = BytesMut::new();
    put_str(&mut b, 1, &det_uuid(&format!("{seed}:msg:{idx}")));
    match msg {
        Msg::User { text } => {
            put_uint(&mut b, 2, 1);
            put_str(&mut b, 3, text);
        }
        Msg::Assistant { text, calls } => {
            put_uint(&mut b, 2, 2);
            for c in calls {
                let mut sub = BytesMut::new();
                put_str(&mut sub, 1, &c.call_id);
                put_str(&mut sub, 2, &c.name);
                put_str(&mut sub, 3, &c.args);
                put_msg(&mut b, 6, &sub);
            }
            if !text.is_empty() {
                put_str(&mut b, 11, text);
            }
        }
        Msg::Tool { call_id, text } => {
            put_uint(&mut b, 2, 4);
            put_str(&mut b, 3, text);
            put_str(&mut b, 7, call_id);
        }
    }
    b
}

/// 构造 GetChatMessageRequest 信封字节（单帧，无 end 帧——实测 CLI 形态）。
///
/// `body` 为出站 Responses JSON；`model_uid` 为上游实际模型名（f21）；
/// `token` 为 devin session token（Metadata f3 原文）。
pub fn build_chat_request(body: &Value, model_uid: &str, token: &str) -> Result<Bytes, String> {
    // 会话种子：输入序列化 + 模型 → 同会话多轮请求派生 id 稳定
    let seed = format!("{model_uid}:{}", body.get("input").unwrap_or(&Value::Null));

    let messages = input_to_messages(body.get("input").unwrap_or(&Value::Null))?;
    let prompt = body
        .get("instructions")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(DEFAULT_PROMPT)
        .to_string();

    let mut b = BytesMut::with_capacity(8 * 1024);

    // f1 Metadata（实测 devin-cli 形态）
    {
        let mut m = BytesMut::new();
        put_str(&mut m, 1, EXTENSION_NAME);
        put_str(&mut m, 2, EXTENSION_VERSION);
        put_str(&mut m, 3, token);
        put_str(&mut m, 4, "en");
        put_str(&mut m, 5, std::env::consts::OS);
        put_str(&mut m, 7, EXTENSION_VERSION);
        put_str(&mut m, 12, "chisel");
        put_str(&mut m, 28, "chisel");
        put_str(&mut m, 31, &fingerprint());
        put_msg(&mut b, 1, &m);
    }

    // f2 头提示词
    put_str(&mut b, 2, &prompt);

    // f3 会话消息
    for (i, msg) in messages.iter().enumerate() {
        let sub = encode_message(msg, i, &seed);
        put_msg(&mut b, 3, &sub);
    }

    // f7 request_type = 5（chat，实测）
    put_uint(&mut b, 7, 5);

    // f8 CompletionConfiguration（实测默认：1 / 128000 / 400 / temp 1.0 / top_k 40 / top_p 0.95）
    {
        let mut c = BytesMut::new();
        put_uint(&mut c, 1, 1);
        put_uint(&mut c, 2, body.get("max_output_tokens").and_then(|v| v.as_u64()).unwrap_or(128000));
        put_uint(&mut c, 3, 400);
        put_double(&mut c, 5, body.get("temperature").and_then(|v| v.as_f64()).unwrap_or(1.0));
        put_uint(&mut c, 7, 40);
        put_double(&mut c, 8, body.get("top_p").and_then(|v| v.as_f64()).unwrap_or(0.95));
        put_msg(&mut b, 8, &c);
    }

    // f10 工具定义（调用方工具；CLI 内置工具不注入）
    if let Some(tools) = body.get("tools").and_then(|v| v.as_array()) {
        for t in tools {
            if t.get("type").and_then(|v| v.as_str()) != Some("function") {
                continue;
            }
            // 兼容 Responses 扁平形态与 Chat 嵌套形态
            let f = t.get("function").unwrap_or(t);
            let Some(name) = f.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let mut sub = BytesMut::new();
            put_str(&mut sub, 1, name);
            if let Some(desc) = f.get("description").and_then(|v| v.as_str()) {
                put_str(&mut sub, 2, desc);
            }
            let params = f
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            put_str(&mut sub, 3, &serde_json::to_string(&params).unwrap_or_else(|_| "{}".into()));
            put_msg(&mut b, 10, &sub);
        }
    }

    // f15 trajectory_reference（会话内稳定；f2 = 轮次）
    let step = messages
        .iter()
        .filter(|m| matches!(m, Msg::Assistant { .. }))
        .count() as u64
        + 1;
    {
        let mut t = BytesMut::new();
        put_str(&mut t, 1, &det_uuid(&format!("{seed}:traj")));
        put_uint(&mut t, 2, step);
        put_uint(&mut t, 3, 4);
        put_msg(&mut b, 15, &t);
    }

    // f16 execution uuid / f20 planner_mode / f21 模型 / f28 会话代号
    put_str(&mut b, 16, &det_uuid(&format!("{seed}:exec")));
    put_uint(&mut b, 20, 1);
    put_str(&mut b, 21, model_uid);
    put_str(&mut b, 28, &codename(&seed));

    Ok(proto::envelope(&b))
}

// ---------------------------------------------------------------------------
// 响应：GetChatMessageResponse 帧 → Responses SSE / 非流式 JSON
// ---------------------------------------------------------------------------

/// 帧内 usage 快照（f7，逐帧滚动；取末帧）。
#[derive(Debug, Clone, Copy, Default)]
pub struct DevinUsage {
    /// f2 未缓存输入
    pub input_uncached: u64,
    /// f5 缓存命中
    pub cache_read: u64,
    /// f3 输出
    pub output: u64,
}

struct FnItem {
    call_id: String,
    name: String,
    args: String,
    item_id: String,
    output_index: u64,
}

/// Devin 响应流 → Responses 事件状态机（流式 SSE 与非流式聚合共用）。
pub struct StreamConv {
    model: String,
    resp_id: String,
    created_at: i64,
    msg_item_id: String,
    msg_index: u64,
    created: bool,
    msg_added: bool,
    msg_text: String,
    calls: Vec<FnItem>,
    usage: Option<DevinUsage>,
    finished: bool,
}

impl StreamConv {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            resp_id: String::new(),
            created_at: chrono::Utc::now().timestamp(),
            msg_item_id: format!("msg_{}", uuid::Uuid::new_v4().simple()),
            msg_index: 0,
            created: false,
            msg_added: false,
            msg_text: String::new(),
            calls: Vec::new(),
            usage: None,
            finished: false,
        }
    }

    /// 是否已输出任何事件（决定错误映射为「可重试」还是「流中失败」）。
    pub fn started(&self) -> bool {
        self.created
    }

    /// 喂入一个数据帧（flags=0 payload），产出已编码 SSE 事件。
    pub fn feed(&mut self, payload: &[u8]) -> Result<Vec<String>, String> {
        let fields = proto::decode(payload)?;
        let mut out = Vec::new();

        // 元数据字段先行解析（resp id / 时间戳），再发 response.created
        for (f, v) in &fields {
            match f {
                1 => {
                    if self.resp_id.is_empty() {
                        if let Some(s) = v.as_str() {
                            let uuid = s.strip_prefix("bot-").unwrap_or(s);
                            self.resp_id = format!("resp_{uuid}");
                        }
                    }
                }
                2 => {
                    if let Some(b) = v.as_bytes() {
                        if let Ok(inner) = proto::decode(b) {
                            if let Some(sec) = proto::get(&inner, 1).and_then(|x| x.as_uint()) {
                                self.created_at = sec as i64;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if !self.created {
            self.created = true;
            out.push(self.event_created());
        }

        let mut stop: Option<u64> = None;
        for (f, v) in &fields {
            match f {
                5 => stop = v.as_uint(),
                6 => {
                    if let Some(b) = v.as_bytes() {
                        out.extend(self.feed_tool_delta(b)?);
                    }
                }
                7 => {
                    if let Some(b) = v.as_bytes() {
                        self.feed_usage(b);
                    }
                }
                9 => {
                    if let Some(s) = v.as_str() {
                        out.extend(self.feed_text(s));
                    }
                }
                _ => {}
            }
        }

        // f5 stop_reason（2=文本结束 / 10=函数调用 FUNCTION_CALL）仅作存在性判据：
        // 不能在本帧收尾——实测 stop 帧后仍有滚动 usage 帧（终值在流末），
        // 终止事件统一延迟到流末（EOF / end 帧）由 finish() 发出，保证 usage 取到终值。
        let _ = stop;
        Ok(out)
    }

    fn feed_text(&mut self, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        if !self.msg_added {
            self.msg_added = true;
            out.push(self.event_item_added_message());
        }
        self.msg_text.push_str(text);
        out.push(encode_typed_event_with_type(
            &json!({
                "type": "response.output_text.delta",
                "item_id": self.msg_item_id,
                "output_index": self.msg_index,
                "content_index": 0,
                "delta": text,
            })
            .to_string(),
            Some("response.output_text.delta"),
        ));
        out
    }

    fn feed_tool_delta(&mut self, payload: &[u8]) -> Result<Vec<String>, String> {
        let fields = proto::decode(payload)?;
        let call_id = proto::get(&fields, 1).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let name = proto::get(&fields, 2).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let frag = proto::get(&fields, 3).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let mut out = Vec::new();

        let idx = if !call_id.is_empty() {
            match self.calls.iter().position(|c| c.call_id == call_id) {
                Some(i) => i,
                None => {
                    self.calls.push(FnItem {
                        call_id: call_id.clone(),
                        name,
                        args: String::new(),
                        item_id: format!("fc_{}", uuid::Uuid::new_v4().simple()),
                        output_index: self.msg_index + 1 + self.calls.len() as u64,
                    });
                    let i = self.calls.len() - 1;
                    out.push(self.event_item_added_fn(i));
                    i
                }
            }
        } else if let Some(i) = self.calls.len().checked_sub(1) {
            i
        } else {
            return Ok(out); // 无宿主调用的孤立参数增量：丢弃
        };

        if !frag.is_empty() {
            self.calls[idx].args.push_str(&frag);
            let c = &self.calls[idx];
            out.push(encode_typed_event(
                &json!({
                    "type": "response.function_call_arguments.delta",
                    "item_id": c.item_id,
                    "output_index": c.output_index,
                    "delta": frag,
                })
                .to_string(),
            ));
        }
        Ok(out)
    }

    fn feed_usage(&mut self, payload: &[u8]) {
        let Ok(fields) = proto::decode(payload) else {
            return;
        };
        let u = self.usage.get_or_insert(DevinUsage::default());
        if let Some(n) = proto::get(&fields, 2).and_then(|v| v.as_uint()) {
            u.input_uncached = n;
        }
        if let Some(n) = proto::get(&fields, 3).and_then(|v| v.as_uint()) {
            u.output = n;
        }
        if let Some(n) = proto::get(&fields, 5).and_then(|v| v.as_uint()) {
            u.cache_read = n;
        }
    }

    /// 流正常结束（无 stop 帧时兜底补终止事件）。
    pub fn finish(&mut self) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        let mut out = Vec::new();
        if !self.created {
            self.created = true;
            out.push(self.event_created());
        }
        out.extend(self.finish_events());
        out
    }

    /// 流内失败：已开始则补 response.failed；未开始由调用方映射为可重试错误。
    pub fn fail(&mut self, message: &str) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        if !self.created {
            return Vec::new();
        }
        let mut resp = self.build_final("failed");
        if let Some(o) = resp.as_object_mut() {
            o.insert(
                "error".into(),
                json!({ "code": "server_error", "message": message }),
            );
        }
        vec![encode_typed_event_with_type(
            &json!({ "type": "response.failed", "response": resp }).to_string(),
            Some("response.failed"),
        )]
    }

    fn finish_events(&mut self) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut out = Vec::new();
        if self.msg_added {
            out.push(encode_typed_event(
                &json!({
                    "type": "response.output_text.done",
                    "item_id": self.msg_item_id,
                    "output_index": self.msg_index,
                    "text": self.msg_text,
                })
                .to_string(),
            ));
            out.push(encode_typed_event(
                &json!({
                    "type": "response.output_item.done",
                    "output_index": self.msg_index,
                    "item": self.item_message("completed"),
                })
                .to_string(),
            ));
        }
        for i in 0..self.calls.len() {
            out.push(encode_typed_event(
                &json!({
                    "type": "response.output_item.done",
                    "output_index": self.calls[i].output_index,
                    "item": self.item_fn(i, "completed"),
                })
                .to_string(),
            ));
        }
        out.push(encode_typed_event(
            &json!({
                "type": "response.completed",
                "response": self.build_final("completed"),
            })
            .to_string(),
        ));
        out
    }

    fn event_created(&mut self) -> String {
        encode_typed_event(
            &json!({
                "type": "response.created",
                "response": {
                    "id": self.resp_id_or_gen(),
                    "object": "response",
                    "created_at": self.created_at,
                    "status": "in_progress",
                    "model": self.model,
                    "output": [],
                    "usage": Value::Null,
                },
            })
            .to_string(),
        )
    }

    fn event_item_added_message(&self) -> String {
        encode_typed_event(
            &json!({
                "type": "response.output_item.added",
                "output_index": self.msg_index,
                "item": self.item_message("in_progress"),
            })
            .to_string(),
        )
    }

    fn event_item_added_fn(&self, i: usize) -> String {
        encode_typed_event(
            &json!({
                "type": "response.output_item.added",
                "output_index": self.calls[i].output_index,
                "item": self.item_fn(i, "in_progress"),
            })
            .to_string(),
        )
    }

    fn item_message(&self, status: &str) -> Value {
        let mut content = Vec::new();
        if !self.msg_text.is_empty() {
            content.push(json!({ "type": "output_text", "text": self.msg_text }));
        }
        json!({
            "id": self.msg_item_id,
            "type": "message",
            "status": status,
            "role": "assistant",
            "content": content,
        })
    }

    fn item_fn(&self, i: usize, status: &str) -> Value {
        let c = &self.calls[i];
        json!({
            "id": c.item_id,
            "type": "function_call",
            "status": status,
            "call_id": c.call_id,
            "name": c.name,
            "arguments": c.args,
        })
    }

    fn usage_json(&self) -> Value {
        match self.usage {
            Some(u) => {
                let input_total = u.input_uncached.saturating_add(u.cache_read);
                json!({
                    "input_tokens": input_total,
                    "output_tokens": u.output,
                    "total_tokens": input_total.saturating_add(u.output),
                    "input_tokens_details": { "cached_tokens": u.cache_read },
                })
            }
            None => Value::Null,
        }
    }

    fn build_final(&mut self, status: &str) -> Value {
        let mut output = Vec::new();
        if self.msg_added {
            output.push(self.item_message("completed"));
        }
        for i in 0..self.calls.len() {
            output.push(self.item_fn(i, "completed"));
        }
        json!({
            "id": self.resp_id_or_gen(),
            "object": "response",
            "created_at": self.created_at,
            "status": status,
            "model": self.model,
            "output": output,
            "usage": self.usage_json(),
        })
    }

    /// 响应 id：首次生成后写回（created 与 completed 必须同 id）
    fn resp_id_or_gen(&mut self) -> String {
        if self.resp_id.is_empty() {
            self.resp_id = format!("resp_{}", uuid::Uuid::new_v4().simple());
        }
        self.resp_id.clone()
    }

    /// 非流式聚合：完整 Responses JSON。
    pub fn final_json(&mut self) -> Value {
        if !self.finished {
            let _ = self.finish();
        }
        self.build_final("completed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn det_uuid_stable_and_formatted() {
        let a = det_uuid("seed1");
        assert_eq!(a, det_uuid("seed1"));
        assert_ne!(a, det_uuid("seed2"));
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
    }

    #[test]
    fn request_encodes_cli_shape() {
        let body = json!({
            "instructions": "You are helpful.",
            "input": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "calling"},
                {"type": "function_call", "call_id": "call_1", "name": "exec", "arguments": "{\"c\":1}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "ok"}
            ],
            "tools": [{"type": "function", "name": "exec", "description": "run", "parameters": {"type": "object"}}],
            "temperature": 0.5,
            "max_output_tokens": 1000
        });
        let env = build_chat_request(&body, "swe-1-6-slow", "tok$1").unwrap();
        // 信封
        assert_eq!(env[0], 0);
        let payload = &env[5..];
        let fields = proto::decode(payload).unwrap();
        assert_eq!(proto::get(&fields, 2).unwrap().as_str(), Some("You are helpful."));
        assert_eq!(proto::get_all(&fields, 3).len(), 3); // user / assistant(合并) / tool
        assert_eq!(proto::get(&fields, 7).unwrap().as_uint(), Some(5));
        assert_eq!(proto::get(&fields, 21).unwrap().as_str(), Some("swe-1-6-slow"));
        assert_eq!(proto::get_all(&fields, 10).len(), 1);
        // assistant 消息：f6 工具调用 + f11 文本
        let assistant = proto::get_all(&fields, 3)[1].as_bytes().unwrap().clone();
        let af = proto::decode(&assistant).unwrap();
        assert_eq!(proto::get(&af, 2).unwrap().as_uint(), Some(2));
        assert_eq!(proto::get(&af, 11).unwrap().as_str(), Some("calling"));
        let tc = proto::get(&af, 6).unwrap().as_bytes().unwrap().clone();
        let tf = proto::decode(&tc).unwrap();
        assert_eq!(proto::get(&tf, 1).unwrap().as_str(), Some("call_1"));
        // tool 结果：f3 + f7
        let tool = proto::get_all(&fields, 3)[2].as_bytes().unwrap().clone();
        let tof = proto::decode(&tool).unwrap();
        assert_eq!(proto::get(&tof, 2).unwrap().as_uint(), Some(4));
        assert_eq!(proto::get(&tof, 7).unwrap().as_str(), Some("call_1"));
    }

    #[test]
    fn stream_text_and_tool_to_responses_events() {
        let mut conv = StreamConv::new("swe-1-6-slow");
        // 帧：消息 id + 文本增量
        let mut f1 = BytesMut::new();
        put_str(&mut f1, 1, "bot-abc");
        put_str(&mut f1, 9, "Hello");
        let ev1 = conv.feed(&f1).unwrap();
        assert!(ev1.iter().any(|e| e.contains("response.created")));
        assert!(ev1.iter().any(|e| e.contains("response.output_text.delta")));
        // 帧：工具调用（id+name 后参数增量）
        let mut sub = BytesMut::new();
        put_str(&mut sub, 1, "call_x");
        put_str(&mut sub, 2, "exec");
        let mut f2 = BytesMut::new();
        put_msg(&mut f2, 6, &sub);
        let ev2 = conv.feed(&f2).unwrap();
        assert!(ev2.iter().any(|e| e.contains("response.output_item.added")));
        let mut sub3 = BytesMut::new();
        put_str(&mut sub3, 3, "{\"a\"");
        let mut f3 = BytesMut::new();
        put_msg(&mut f3, 6, &sub3);
        let ev3 = conv.feed(&f3).unwrap();
        assert!(ev3
            .iter()
            .any(|e| e.contains("response.function_call_arguments.delta")));
        // 帧：stop + usage
        let mut u = BytesMut::new();
        put_uint(&mut u, 2, 100);
        put_uint(&mut u, 3, 20);
        put_uint(&mut u, 5, 50);
        let mut f4 = BytesMut::new();
        put_uint(&mut f4, 5, 10);
        put_msg(&mut f4, 7, &u);
        // stop 帧不立即收尾（usage 终值可能在后续帧），流末才出终止事件
        let ev4 = conv.feed(&f4).unwrap();
        assert!(!ev4.iter().any(|e| e.contains("response.completed")));
        let ev5 = conv.finish();
        assert!(ev5.iter().any(|e| e.contains("response.completed")));
        // 终止事件里的 usage：input=150（未缓存100+缓存50）output=20
        let done = ev5.iter().find(|e| e.contains("response.completed")).unwrap();
        assert!(done.contains("\"input_tokens\":150"));
        assert!(done.contains("\"cached_tokens\":50"));
        assert!(done.contains("\"output_tokens\":20"));
        // 工具参数聚合（事件为 SSE 编码：event 行 + data 行）
        let data = done
            .lines()
            .find(|l| l.starts_with("data: "))
            .and_then(|l| l.strip_prefix("data: "))
            .unwrap();
        let v: Value = serde_json::from_str(data).unwrap();
        let fitem = v["response"]["output"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["type"] == "function_call")
            .unwrap();
        assert_eq!(fitem["name"], "exec");
        assert_eq!(fitem["arguments"], "{\"a\"");
    }
}
