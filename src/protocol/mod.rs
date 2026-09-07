//! 协议层：IR + 四协议适配器 + SSE + 错误体翻译（PLAN.md §4）。
//!
//! 双向转换均走 IR（N→1→N），避免 N×M 适配器爆炸。
//! 无法映射能力三级策略（§4.3）：直接映射 → 降级（记 ConvCtx + X-NextAPI-Degraded）→ 扩展透传。
#![allow(dead_code)] // M2 先落地纯转换逻辑，M3 网关核心接入后移除
#![allow(unused_imports)] // 同上：pub use 供 M3 使用

pub mod anthropic;
pub mod chat;
pub mod errors;
pub mod gemini;
pub mod ir;
pub mod responses;
pub mod sse;

pub use errors::{error_from_ir, error_to_ir, IrError};
pub use ir::*;

/// 转换错误。
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("请求/响应解析失败: {0}")]
    Parse(String),
    #[error("协议能力不支持: {0}")]
    Unsupported(String),
}

/// 降级项（无法映射的字段/能力）。
#[derive(Debug, Clone, PartialEq)]
pub struct DegradedItem {
    pub field: String,
    pub reason: String,
}

/// 转换上下文：收集降级项，转换结束后写入 warning 日志与 X-NextAPI-Degraded 头。
#[derive(Debug, Default)]
pub struct ConvCtx {
    pub degraded: Vec<DegradedItem>,
}

impl ConvCtx {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一个降级项（字段被丢弃/简化）。
    pub fn degrade(&mut self, field: impl Into<String>, reason: impl Into<String>) {
        self.degraded.push(DegradedItem {
            field: field.into(),
            reason: reason.into(),
        });
    }

    /// 生成 X-NextAPI-Degraded 响应头值（无降级时 None）。
    pub fn degraded_header(&self) -> Option<String> {
        if self.degraded.is_empty() {
            None
        } else {
            Some(
                self.degraded
                    .iter()
                    .map(|d| d.field.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            )
        }
    }
}

/// 请求：协议 JSON → IR。
pub fn request_to_ir(
    p: Protocol,
    v: &serde_json::Value,
    ctx: &mut ConvCtx,
) -> Result<IrRequest, ConvertError> {
    match p {
        Protocol::OpenaiChat => chat::request_to_ir(v, ctx),
        Protocol::OpenaiResponses => responses::request_to_ir(v, ctx),
        Protocol::Anthropic => anthropic::request_to_ir(v, ctx),
        Protocol::Gemini => gemini::request_to_ir(v, ctx),
    }
}

/// 请求：IR → 协议 JSON。
pub fn request_from_ir(
    p: Protocol,
    req: &IrRequest,
    ctx: &mut ConvCtx,
) -> Result<serde_json::Value, ConvertError> {
    match p {
        Protocol::OpenaiChat => chat::request_from_ir(req, ctx),
        Protocol::OpenaiResponses => responses::request_from_ir(req, ctx),
        Protocol::Anthropic => anthropic::request_from_ir(req, ctx),
        Protocol::Gemini => gemini::request_from_ir(req, ctx),
    }
}

/// 非流式响应：协议 JSON → IR。
pub fn response_to_ir(
    p: Protocol,
    v: &serde_json::Value,
    ctx: &mut ConvCtx,
) -> Result<IrResponse, ConvertError> {
    match p {
        Protocol::OpenaiChat => chat::response_to_ir(v, ctx),
        Protocol::OpenaiResponses => responses::response_to_ir(v, ctx),
        Protocol::Anthropic => anthropic::response_to_ir(v, ctx),
        Protocol::Gemini => gemini::response_to_ir(v, ctx),
    }
}

/// 非流式响应：IR → 协议 JSON。
pub fn response_from_ir(
    p: Protocol,
    resp: &IrResponse,
    ctx: &mut ConvCtx,
) -> Result<serde_json::Value, ConvertError> {
    match p {
        Protocol::OpenaiChat => chat::response_from_ir(resp, ctx),
        Protocol::OpenaiResponses => responses::response_from_ir(resp, ctx),
        Protocol::Anthropic => anthropic::response_from_ir(resp, ctx),
        Protocol::Gemini => gemini::response_from_ir(resp, ctx),
    }
}

/// 各协议流式状态机（聚合工具调用增量、事件序列状态）。
pub enum AnyStreamState {
    OpenaiChat(chat::StreamState),
    OpenaiResponses(responses::StreamState),
    Anthropic(anthropic::StreamState),
    Gemini(gemini::StreamState),
}

impl AnyStreamState {
    pub fn new(p: Protocol) -> Self {
        match p {
            Protocol::OpenaiChat => Self::OpenaiChat(chat::StreamState::default()),
            Protocol::OpenaiResponses => Self::OpenaiResponses(responses::StreamState::default()),
            Protocol::Anthropic => Self::Anthropic(anthropic::StreamState::default()),
            Protocol::Gemini => Self::Gemini(gemini::StreamState::default()),
        }
    }
}

/// 流式：协议 SSE data 载荷 → IR chunk。
/// `data` 为单条事件的完整 data 载荷（sse::SseParser 产出）；
/// 返回 None 表示该事件无业务内容（如 ping/role 帧已被吸收），可跳过。
pub fn chunk_to_ir(
    p: Protocol,
    data: &str,
    st: &mut AnyStreamState,
    ctx: &mut ConvCtx,
) -> Result<Option<IrChunk>, ConvertError> {
    match (p, st) {
        (Protocol::OpenaiChat, AnyStreamState::OpenaiChat(s)) => chat::chunk_to_ir(data, s, ctx),
        (Protocol::OpenaiResponses, AnyStreamState::OpenaiResponses(s)) => {
            responses::chunk_to_ir(data, s, ctx)
        }
        (Protocol::Anthropic, AnyStreamState::Anthropic(s)) => anthropic::chunk_to_ir(data, s, ctx),
        (Protocol::Gemini, AnyStreamState::Gemini(s)) => gemini::chunk_to_ir(data, s, ctx),
        _ => Err(ConvertError::Unsupported("流状态机与协议不匹配".into())),
    }
}

/// 流式：IR chunk → 协议 SSE data 载荷列表（一条 IR chunk 可能对应多条协议事件）。
pub fn chunk_from_ir(
    p: Protocol,
    chunk: &IrChunk,
    st: &mut AnyStreamState,
    ctx: &mut ConvCtx,
) -> Result<Vec<String>, ConvertError> {
    match (p, st) {
        (Protocol::OpenaiChat, AnyStreamState::OpenaiChat(s)) => chat::chunk_from_ir(chunk, s, ctx),
        (Protocol::OpenaiResponses, AnyStreamState::OpenaiResponses(s)) => {
            responses::chunk_from_ir(chunk, s, ctx)
        }
        (Protocol::Anthropic, AnyStreamState::Anthropic(s)) => {
            anthropic::chunk_from_ir(chunk, s, ctx)
        }
        (Protocol::Gemini, AnyStreamState::Gemini(s)) => gemini::chunk_from_ir(chunk, s, ctx),
        _ => Err(ConvertError::Unsupported("流状态机与协议不匹配".into())),
    }
}

/// 流传输中断（上游连接错误等）：尽力补发协议内「失败终止帧」（发布审阅 L11）——
/// Responses → response.failed；Anthropic → message_delta(null stop_reason)+message_stop；
/// Chat/Gemini → 常规终止（[DONE] / 工具冲刷）。客户端据此区分「失败」与「截断」。
pub fn stream_fail_end(
    p: Protocol,
    st: &mut AnyStreamState,
    ctx: &mut ConvCtx,
) -> Result<Vec<String>, ConvertError> {
    match (p, &mut *st) {
        (Protocol::OpenaiResponses, AnyStreamState::OpenaiResponses(s)) => {
            s.mark_failed();
            responses::stream_end(s, ctx)
        }
        (Protocol::Anthropic, AnyStreamState::Anthropic(s)) => {
            s.mark_failed();
            anthropic::stream_end(s, ctx)
        }
        _ => stream_end(p, st, ctx),
    }
}

/// 流结束：产出协议终止事件（OpenAI 为 ["[DONE]"]；Anthropic 为 message_stop 等；Gemini 无）。
pub fn stream_end(
    p: Protocol,
    st: &mut AnyStreamState,
    ctx: &mut ConvCtx,
) -> Result<Vec<String>, ConvertError> {
    match (p, st) {
        (Protocol::OpenaiChat, AnyStreamState::OpenaiChat(s)) => chat::stream_end(s, ctx),
        (Protocol::OpenaiResponses, AnyStreamState::OpenaiResponses(s)) => {
            responses::stream_end(s, ctx)
        }
        (Protocol::Anthropic, AnyStreamState::Anthropic(s)) => anthropic::stream_end(s, ctx),
        (Protocol::Gemini, AnyStreamState::Gemini(s)) => gemini::stream_end(s, ctx),
        _ => Err(ConvertError::Unsupported("流状态机与协议不匹配".into())),
    }
}

#[cfg(test)]
mod cross_protocol_tests {
    //! 跨协议转换矩阵（review C-P2-3）：IR 枢轴 N→1→N 的文本/工具双向断言。
    use super::*;
    use serde_json::json;

    fn roundtrip(
        from: Protocol,
        to: Protocol,
        body: serde_json::Value,
    ) -> (serde_json::Value, Vec<DegradedItem>) {
        let mut ctx = ConvCtx::default();
        let ir =
            crate::protocol::request_to_ir(from, &body, &mut ctx).expect("request_to_ir 应成功");
        let out =
            crate::protocol::request_from_ir(to, &ir, &mut ctx).expect("request_from_ir 应成功");
        (out, ctx.degraded)
    }

    #[test]
    fn chat_text_request_to_anthropic() {
        // OpenAI Chat → Anthropic：system/messages/温度映射 + max_tokens 默认
        let (out, _deg) = roundtrip(
            Protocol::OpenaiChat,
            Protocol::Anthropic,
            json!({
                "model": "gpt-4o",
                "messages": [
                    {"role": "system", "content": "你是助手"},
                    {"role": "user", "content": "你好"}
                ],
                "temperature": 0.5
            }),
        );
        // system 映射 + 用户内容保留 + 温度映射
        let sys = out["system"].as_str().map(String::from);
        assert!(sys.as_deref() == Some("你是助手") || format!("{:?}", out).contains("你是助手"));
        assert!(format!("{:?}", out["messages"]).contains("你好"));
        assert_eq!(out["temperature"], 0.5);
        // Anthropic 必填 max_tokens 由 IR 层补齐
        assert!(out["max_tokens"].is_number() && out["max_tokens"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn chat_tool_request_to_gemini() {
        // OpenAI Chat（工具定义 + tool_calls 语义）→ Gemini：functions → functionDeclarations
        let (out, _deg) = roundtrip(
            Protocol::OpenaiChat,
            Protocol::Gemini,
            json!({
                "model": "gpt-4o",
                "messages": [
                    {"role": "user", "content": "北京天气？"},
                    {"role": "assistant", "content": null,
                     "tool_calls": [{"id": "call_1", "type": "function",
                                     "function": {"name": "get_weather", "arguments": "{\"city\":\"bj\"}"}}]},
                    {"role": "tool", "tool_call_id": "call_1", "content": "晴"}
                ],
                "tools": [{"type": "function", "function": {
                    "name": "get_weather",
                    "description": "查天气",
                    "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
                }}]
            }),
        );
        // Gemini 请求体：contents / tools.functionDeclarations；序列化文本含关键信息
        let text = format!("{:?}", out);
        assert!(text.contains("contents"), "应有 contents: {out}");
        assert!(
            text.contains("functionDeclarations") || text.contains("tools"),
            "工具应映射: {out}"
        );
        assert!(text.contains("get_weather"), "工具名应保留: {out}");
        assert!(
            text.contains("bj") || text.contains("\"city\""),
            "工具参数应保留: {out}"
        );
    }

    #[test]
    fn anthropic_request_to_openai_chat() {
        let (out, _deg) = roundtrip(
            Protocol::Anthropic,
            Protocol::OpenaiChat,
            json!({
                "model": "claude-3",
                "max_tokens": 4096,
                "system": "保持简洁",
                "messages": [{"role": "user", "content": "1+1=?"}]
            }),
        );
        assert_eq!(out["model"], "claude-3");
        let text = format!("{:?}", out);
        assert!(text.contains("1+1=?"), "用户消息应保留: {out}");
        assert!(text.contains("保持简洁"), "system 内容应保留: {out}");
        assert!(
            out["messages"].as_array().is_some_and(|a| !a.is_empty()),
            "messages 应有内容: {out}"
        );
    }
}
