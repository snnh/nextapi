//! 统一内部表示 IR：以 OpenAI Chat Completions 为骨架 + 扩展字段（PLAN.md §4.1）。
//!
//! 归一化约定：
//! - `max_tokens`：OpenAI `max_tokens`/`max_completion_tokens`、Anthropic `max_tokens`、
//!   Gemini `maxOutputTokens` 统一进此字段；
//! - `finish_reason`：归一化为 OpenAI 词汇（stop/length/tool_calls/content_filter），
//!   原始值保留在 extra；
//! - 工具调用参数 `arguments` 一律为 JSON 字符串（增量片段亦然）；
//! - 各协议无法归类的字段放入 `extra`/`ext` 原样携带。

use serde::{Deserialize, Serialize};

/// 支持的四种协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    OpenaiChat,
    OpenaiResponses,
    Anthropic,
    Gemini,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenaiChat => "openai_chat",
            Self::OpenaiResponses => "openai_responses",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
        }
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Protocol {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "openai_chat" | "openai" => Ok(Self::OpenaiChat),
            "openai_responses" | "responses" => Ok(Self::OpenaiResponses),
            "anthropic" => Ok(Self::Anthropic),
            "gemini" => Ok(Self::Gemini),
            other => Err(format!("未知协议: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IrRole {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

/// 消息内容：纯文本或多模态 parts。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IrContent {
    Text(String),
    Parts(Vec<IrPart>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IrPart {
    Text {
        text: String,
    },
    /// URL 图片（默认透传 URL）
    ImageUrl {
        url: String,
    },
    /// base64 内联图片（目标协议要求内联时使用，如 Gemini inline_data）
    ImageInline {
        media_type: String,
        data: String,
    },
    InputAudio {
        data: String,
        format: String,
    },
    File {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
}

/// 工具调用（完整形态；arguments 为 JSON 字符串）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// 工具定义（归一化为 OpenAI function 形态；parameters 为 JSON Schema）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrMessage {
    pub role: IrRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<IrContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<IrToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

/// 扩展字段（无法归类的进 extra 原样携带）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrExt {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl IrExt {
    pub fn is_empty(&self) -> bool {
        self.thinking.is_none()
            && self.web_search.is_none()
            && self.cache_control.is_none()
            && self.top_k.is_none()
            && self.service_tier.is_none()
            && self.extra.is_empty()
    }
}

/// Chat 入口 max_tokens 的原始字段名（round-trip 保真用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaxTokensField {
    /// `max_tokens`（传统字段；第三方 OpenAI 兼容服务商普遍只认这个）
    MaxTokens,
    /// `max_completion_tokens`（o 系/gpt-5 必填的新字段）
    MaxCompletionTokens,
}

/// 请求 IR。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrRequest {
    pub model: String,
    #[serde(default)]
    pub messages: Vec<IrMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<IrTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Chat 入口请求 max_tokens 使用的原始字段名；None = 未携带或非 Chat 入口。
    /// 回写策略：用户请求用什么字段就回写什么字段（round-trip 保真）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens_field: Option<MaxTokensField>,
    /// 归一为字符串数组
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(default)]
    pub stream: bool,
    /// stream_options.include_usage
    #[serde(default)]
    pub stream_include_usage: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(default, skip_serializing_if = "IrExt::is_empty")]
    pub ext: IrExt,
    /// 入口协议的未知字段原样保留
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// token 用量（cache 读/写单列；total 可选）。
///
/// 口径归一约定（P0 计价修复）：
/// - `prompt_tokens` 语义统一为「未命中缓存的输入 token」，与 `cache_read_tokens`
///   互不重叠（配额/计价口径 = 未缓存输入 + 输出 + 缓存写 + 缓存读）；
/// - OpenAI Chat `prompt_tokens`、Responses `input_tokens`、Gemini `promptTokenCount`
///   的上游语义均为「含缓存命中」的全量输入（cached 是其子集）：
///   协议入站解析时以 saturating_sub 扣减 cached（cached 缺失不扣、cached > prompt
///   钳 0），出站生成时把 cached 加回，保持上游/客户端看到的协议语义不变；
/// - Anthropic `input_tokens` 本身不含 cache_read，进出站均不加减。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrUsage {
    /// 未命中缓存的输入 token（不含 cache_read；见上方口径约定）
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    /// 缓存命中（读）token，与 prompt_tokens 互不重叠
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// 缓存写入 token
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// 非流式响应 IR。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub choices: Vec<IrChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<IrUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrChoice {
    #[serde(default)]
    pub index: u32,
    pub message: IrMessage,
    /// 归一化 OpenAI 词汇：stop/length/tool_calls/content_filter/...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// 流式 chunk IR（对齐 OpenAI chat.completion.chunk）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrChunk {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub choices: Vec<IrChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<IrUsage>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrChunkChoice {
    #[serde(default)]
    pub index: u32,
    pub delta: IrDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<IrRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<IrToolCallDelta>,
}

/// 工具调用流式增量（index 对齐；arguments 为 JSON 字符串片段）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrToolCallDelta {
    #[serde(default)]
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

/// 工具调用流式聚合器（跨协议转换时按 index 对齐、拼接参数，PLAN.md §4.4）。
#[derive(Debug, Default)]
pub struct ToolCallAggregator {
    calls: Vec<IrToolCall>,
}

impl ToolCallAggregator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入增量，按 index 对齐累积。
    pub fn feed(&mut self, delta: &IrToolCallDelta) {
        let idx = delta.index as usize;
        if self.calls.len() <= idx {
            self.calls.resize_with(idx + 1, IrToolCall::default);
        }
        let call = &mut self.calls[idx];
        if let Some(id) = &delta.id {
            call.id.push_str(id);
        }
        if let Some(name) = &delta.name {
            call.name.push_str(name);
        }
        if let Some(args) = &delta.arguments {
            call.arguments.push_str(args);
        }
    }

    pub fn feed_all(&mut self, deltas: &[IrToolCallDelta]) {
        for d in deltas {
            self.feed(d);
        }
    }

    /// 取出聚合结果。
    pub fn finish(self) -> Vec<IrToolCall> {
        self.calls
    }
}

/// finish_reason 归一化（各协议原始值 → OpenAI 词汇）。
pub fn normalize_finish_reason(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "stop" | "end_turn" | "stop_sequence" | "completed" | "finished" => "stop".into(),
        "length" | "max_tokens" | "max_output_tokens" | "incomplete" => "length".into(),
        "tool_calls" | "tool_use" | "function_call" => "tool_calls".into(),
        "content_filter" | "safety" | "blocked" | "recitation" => "content_filter".into(),
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_aggregator_aligns_by_index() {
        let mut agg = ToolCallAggregator::new();
        agg.feed(&IrToolCallDelta {
            index: 0,
            id: Some("call_1".into()),
            name: Some("get_".into()),
            arguments: Some("{\"ci".into()),
        });
        agg.feed(&IrToolCallDelta {
            index: 1,
            id: Some("call_2".into()),
            name: Some("other".into()),
            arguments: None,
        });
        agg.feed(&IrToolCallDelta {
            index: 0,
            id: None,
            name: Some("weather".into()),
            arguments: Some("ty\":\"bj\"}".into()),
        });
        let calls = agg.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].arguments, "{\"city\":\"bj\"}");
        assert_eq!(calls[1].name, "other");
    }

    #[test]
    fn finish_reason_normalized() {
        assert_eq!(normalize_finish_reason("end_turn"), "stop");
        assert_eq!(normalize_finish_reason("MAX_TOKENS"), "length");
        assert_eq!(normalize_finish_reason("tool_use"), "tool_calls");
        assert_eq!(normalize_finish_reason("custom_x"), "custom_x");
    }

    #[test]
    fn protocol_roundtrip() {
        assert_eq!(
            "openai_chat".parse::<Protocol>().unwrap(),
            Protocol::OpenaiChat
        );
        assert_eq!(Protocol::Gemini.as_str(), "gemini");
        assert!("unknown".parse::<Protocol>().is_err());
    }
}
