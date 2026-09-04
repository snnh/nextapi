//! Anthropic Messages 适配器（/v1/messages）。
//!
//! 要点（PLAN.md §4.2/§4.4）：
//! - system 提示 → 顶层 `system` 字段；`max_tokens` 必填，入口未提供时转换填充默认 65536；
//! - thinking 扩展块（含 signature 原样携带）、cache_control 透传；
//! - 流式事件：message_start / content_block_* / message_delta / message_stop；
//! - usage：input/output/cache_read_input/cache_creation_input tokens。
//!
//! 转换策略（§4.3）：能直接映射的字段直接映射；Anthropic 无对应能力的字段（如
//! response_format/seed/n/联网搜索等）记 `ctx.degrade`；Anthropic 原生扩展字段
//! （thinking/top_k/cache_control）通过 `req.ext`/`IrExt` 透传。

use std::collections::HashMap;

use serde_json::{Map, Value};

use super::ir::*;
use super::{ConvCtx, ConvertError};

/// 转 Anthropic 时缺省 max_tokens（v1.12 确认：64k）。
pub const DEFAULT_MAX_TOKENS: u64 = 65536;

/// 请求体中被本适配器识别并映射的顶层字段；其余进 `extra`。
const KNOWN_REQUEST_FIELDS: [&str; 13] = [
    "model",
    "system",
    "messages",
    "tools",
    "tool_choice",
    "temperature",
    "top_p",
    "top_k",
    "stop_sequences",
    "max_tokens",
    "stream",
    "metadata",
    "thinking",
];

/// 响应体顶层规范字段；其余进 `extra`。
const KNOWN_RESPONSE_FIELDS: [&str; 8] = [
    "id",
    "type",
    "role",
    "content",
    "model",
    "stop_reason",
    "stop_sequence",
    "usage",
];

fn json_object() -> Value {
    Value::Object(Map::new())
}

/// 把零散的文本片段规整为 IR 内容：空 → None；单个 text part → Text；否则 → Parts。
/// 归一化后便于多协议往返保持稳定（单个文本块回落为字符串）。
fn parts_content(parts: Vec<IrPart>) -> Option<IrContent> {
    if parts.is_empty() {
        return None;
    }
    if parts.len() == 1 {
        if let IrPart::Text { text } = &parts[0] {
            return Some(IrContent::Text(text.clone()));
        }
    }
    Some(IrContent::Parts(parts))
}

/// 将 Anthropic `input`（对象）转为 JSON 字符串；字符串原样、null → 空串。
fn input_to_arguments(input: &Value) -> String {
    if input.is_string() {
        return input.as_str().unwrap_or_default().to_string();
    }
    if input.is_null() {
        return String::new();
    }
    serde_json::to_string(input).unwrap_or_default()
}

/// 将 `tool_result` 的 content（字符串或 text block 数组）抽取为纯文本。
fn tool_result_text(content: Option<&Value>) -> String {
    match content {
        Some(v) if v.is_string() => v.as_str().unwrap_or_default().to_string(),
        Some(v) if v.is_null() => String::new(),
        Some(v) if v.is_array() => v
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b["text"].as_str().map(String::from))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// 请求：Anthropic JSON → IR
// ---------------------------------------------------------------------------

/// 解析 `system` 字段（字符串或 text block 数组）为一条 System 消息；
/// 提取文本，带 cache_control 的 block 的 cache_control 进 `req.ext.cache_control`。
fn parse_system(sys: &Value, req: &mut IrRequest, ctx: &mut ConvCtx) -> Result<(), ConvertError> {
    if sys.is_null() {
        return Ok(());
    }
    let text = if let Some(s) = sys.as_str() {
        s.to_string()
    } else if let Some(arr) = sys.as_array() {
        let mut buf = String::new();
        for block in arr {
            match block["type"].as_str().unwrap_or_default() {
                "text" => buf.push_str(block["text"].as_str().unwrap_or_default()),
                other => ctx.degrade(
                    "system.block.type",
                    format!("未知 system block 类型: {other}"),
                ),
            }
            if let Some(cc) = block.get("cache_control") {
                if req.ext.cache_control.is_none() {
                    req.ext.cache_control = Some(cc.clone());
                }
            }
        }
        buf
    } else {
        ctx.degrade("system", "system 既非字符串也非数组，丢弃");
        return Ok(());
    };

    // 无文本且无 cache_control 时忽略空 system。
    if text.is_empty() && req.ext.cache_control.is_none() {
        return Ok(());
    }
    req.messages.push(IrMessage {
        role: IrRole::System,
        content: Some(IrContent::Text(text)),
        ..Default::default()
    });
    Ok(())
}

/// 解析单条 Anthropic message（user/assistant）。
/// 由于 `tool_result` block 会展开为独立 Tool 消息，此函数返回 Vec<IrMessage>。
fn parse_anth_message(
    msg: &Value,
    ext: &mut IrExt,
    ctx: &mut ConvCtx,
) -> Result<Vec<IrMessage>, ConvertError> {
    if !msg.is_object() {
        return Err(ConvertError::Parse("message 不是对象".into()));
    }
    let role = msg["role"].as_str().unwrap_or_default();
    let ir_role = match role {
        "user" => IrRole::User,
        "assistant" => IrRole::Assistant,
        other => {
            ctx.degrade("message.role", format!("未知角色按 user 处理: {other}"));
            IrRole::User
        }
    };
    let content = msg.get("content");
    let mut out = Vec::new();

    // 字符串 content
    if let Some(s) = content.and_then(|c| c.as_str()) {
        out.push(IrMessage {
            role: ir_role,
            content: Some(IrContent::Text(s.to_string())),
            ..Default::default()
        });
        return Ok(out);
    }

    // block 数组 content
    if let Some(arr) = content.and_then(|c| c.as_array()) {
        let mut parts: Vec<IrPart> = Vec::new();
        let mut tool_calls: Vec<IrToolCall> = Vec::new();
        let mut reasoning: Option<String> = None;
        let mut tool_msgs: Vec<IrMessage> = Vec::new();

        for block in arr {
            if !block.is_object() {
                continue;
            }
            match block["type"].as_str().unwrap_or_default() {
                "text" => parts.push(IrPart::Text {
                    text: block["text"].as_str().unwrap_or_default().to_string(),
                }),
                "image" => {
                    let src = &block["source"];
                    match src["type"].as_str().unwrap_or_default() {
                        "base64" => parts.push(IrPart::ImageInline {
                            media_type: src["media_type"].as_str().unwrap_or_default().to_string(),
                            data: src["data"].as_str().unwrap_or_default().to_string(),
                        }),
                        "url" => parts.push(IrPart::ImageUrl {
                            url: src["url"].as_str().unwrap_or_default().to_string(),
                        }),
                        other => ctx.degrade(
                            "content.block.image.source",
                            format!("未知图片源类型: {other}"),
                        ),
                    }
                }
                "tool_use" => tool_calls.push(IrToolCall {
                    id: block["id"].as_str().unwrap_or_default().to_string(),
                    name: block["name"].as_str().unwrap_or_default().to_string(),
                    arguments: input_to_arguments(&block["input"]),
                }),
                "tool_result" => {
                    let tid = block["tool_use_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    tool_msgs.push(IrMessage {
                        role: IrRole::Tool,
                        tool_call_id: Some(tid),
                        content: Some(IrContent::Text(tool_result_text(block.get("content")))),
                        ..Default::default()
                    });
                }
                "thinking" => {
                    reasoning = Some(block["thinking"].as_str().unwrap_or_default().to_string());
                    // signature 原样保留（IR 无 per-message ext，暂存请求级 ext.extra，PLAN §4.4 防工具链断裂）
                    if let Some(sig) = block["signature"].as_str() {
                        ext.extra
                            .insert("thinking_signature".into(), Value::String(sig.to_string()));
                    }
                }
                other => ctx.degrade("content.block.type", format!("未知块类型: {other}")),
            }
        }

        let content_val = parts_content(parts);
        let main = IrMessage {
            role: ir_role,
            content: content_val,
            tool_calls,
            reasoning_content: reasoning,
            ..Default::default()
        };
        if main.content.is_some() || !main.tool_calls.is_empty() || main.reasoning_content.is_some()
        {
            out.push(main);
        }
        out.extend(tool_msgs);
        return Ok(out);
    }

    // content 为 null/缺失：保留角色但无内容
    out.push(IrMessage {
        role: ir_role,
        ..Default::default()
    });
    Ok(out)
}

/// 解析工具定义（name/description/input_schema → parameters）。`input_schema` 缺省为空对象。
fn parse_tools(v: &Value, _ctx: &mut ConvCtx) -> Result<Vec<IrTool>, ConvertError> {
    let arr = v
        .as_array()
        .ok_or_else(|| ConvertError::Parse("tools 不是数组".into()))?;
    let mut out = Vec::new();
    for t in arr {
        if !t.is_object() {
            continue;
        }
        out.push(IrTool {
            name: t["name"].as_str().unwrap_or_default().to_string(),
            description: t["description"].as_str().map(String::from),
            parameters: t.get("input_schema").cloned().unwrap_or_else(json_object),
        });
    }
    Ok(out)
}

/// Anthropic tool_choice → OpenAI 形态。
/// `auto`→"auto"，`any`→"required"，`tool`→{type:"function",function:{name}}。
fn parse_tool_choice(tc: &Value) -> Option<Value> {
    if tc.is_null() {
        return None;
    }
    let t = tc["type"].as_str().unwrap_or_default();
    match t {
        "auto" => Some(Value::String("auto".into())),
        "any" => Some(Value::String("required".into())),
        "tool" => Some(serde_json::json!({
            "type": "function",
            "function": {"name": tc["name"].as_str().unwrap_or_default()},
        })),
        _ => Some(tc.clone()),
    }
}

/// 请求 JSON → IR。未知主体字段进 `req.extra`。
pub fn request_to_ir(v: &Value, ctx: &mut ConvCtx) -> Result<IrRequest, ConvertError> {
    let obj = v
        .as_object()
        .ok_or_else(|| ConvertError::Parse("请求体不是对象".into()))?;
    let mut req = IrRequest::default();
    req.model = v["model"].as_str().unwrap_or_default().to_string();

    if let Some(sys) = v.get("system") {
        parse_system(sys, &mut req, ctx)?;
    }

    if let Some(msgs) = v.get("messages") {
        let arr = msgs
            .as_array()
            .ok_or_else(|| ConvertError::Parse("messages 不是数组".into()))?;
        for m in arr {
            req.messages
                .extend(parse_anth_message(m, &mut req.ext, ctx)?);
        }
    }

    req.max_tokens = v["max_tokens"].as_u64();

    // stop_sequences → stop（字符串或数组统一为 Vec<String>）
    if let Some(ss) = v.get("stop_sequences") {
        req.stop = if let Some(s) = ss.as_str() {
            Some(vec![s.to_string()])
        } else if let Some(arr) = ss.as_array() {
            Some(
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect(),
            )
        } else {
            None
        };
    }

    req.temperature = v["temperature"].as_f64();
    req.top_p = v["top_p"].as_f64();
    if let Some(k) = v["top_k"].as_u64() {
        match u32::try_from(k) {
            Ok(n) => req.ext.top_k = Some(n),
            Err(_) => ctx.degrade("top_k", "超出 u32 范围，丢弃"),
        }
    }
    req.stream = v["stream"].as_bool().unwrap_or(false);

    if let Some(tools) = v.get("tools") {
        req.tools = parse_tools(tools, ctx)?;
    }
    req.tool_choice = v.get("tool_choice").and_then(parse_tool_choice);

    // metadata.user_id → user
    req.user = v["metadata"]["user_id"].as_str().map(String::from);

    // thinking 请求参数 → ext.thinking（如 {type:"enabled",budget_tokens:N}）
    if let Some(t) = v.get("thinking") {
        req.ext.thinking = Some(t.clone());
    }

    // 未知字段进 extra
    for (k, val) in obj {
        if !KNOWN_REQUEST_FIELDS.contains(&k.as_str()) {
            req.extra.insert(k.clone(), val.clone());
        }
    }

    Ok(req)
}

// ---------------------------------------------------------------------------
// 请求：IR → Anthropic JSON
// ---------------------------------------------------------------------------

fn number_f64(v: f64) -> Value {
    serde_json::Number::from_f64(v)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn text_block(text: &str) -> Value {
    serde_json::json!({"type": "text", "text": text})
}

/// 将 IR 内容（Text/Parts）转为 Anthropic content block 数组。
fn content_to_anthropic_blocks(content: &IrContent, ctx: &mut ConvCtx) -> Vec<Value> {
    match content {
        IrContent::Text(s) => vec![text_block(s)],
        IrContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| part_to_anth_block(p, ctx))
            .collect(),
    }
}

/// 单个 content part → Anthropic block。Anthropic 不支持的 part（音频/文件）降级并跳过。
fn part_to_anth_block(p: &IrPart, ctx: &mut ConvCtx) -> Option<Value> {
    match p {
        IrPart::Text { text } => Some(text_block(text)),
        IrPart::ImageUrl { url } => Some(serde_json::json!({
            "type": "image",
            "source": {"type": "url", "url": url},
        })),
        IrPart::ImageInline { media_type, data } => Some(serde_json::json!({
            "type": "image",
            "source": {"type": "base64", "media_type": media_type, "data": data},
        })),
        IrPart::InputAudio { .. } => {
            ctx.degrade("content.input_audio", "Anthropic 不支持音频");
            None
        }
        IrPart::File { .. } => {
            ctx.degrade("content.file", "Anthropic 不支持文件");
            None
        }
    }
}

/// assistant 消息的 thinking block；signature 从请求级 ext.extra[thinking_signature] 原样携带。
fn thinking_block(rc: &str, req: &IrRequest) -> Value {
    let mut b = serde_json::json!({"type": "thinking", "thinking": rc});
    if let Some(sig) = req
        .ext
        .extra
        .get("thinking_signature")
        .and_then(|v| v.as_str())
    {
        b["signature"] = Value::String(sig.to_string());
    }
    b
}

/// assistant 消息的 tool_use block；arguments JSON 字符串解析回对象，解析失败用原字符串兜底。
fn tool_use_block(tc: &IrToolCall) -> Value {
    let input = serde_json::from_str::<Value>(&tc.arguments)
        .unwrap_or_else(|_| Value::String(tc.arguments.clone()));
    serde_json::json!({"type": "tool_use", "id": tc.id, "name": tc.name, "input": input})
}

/// tool_result block（Tool 消息 → 用户消息内 content block）。
fn tool_result_block(m: &IrMessage, ctx: &mut ConvCtx) -> Value {
    let content = match &m.content {
        Some(IrContent::Text(s)) => Value::String(s.clone()),
        Some(IrContent::Parts(parts)) => Value::Array(
            parts
                .iter()
                .filter_map(|p| part_to_anth_block(p, ctx))
                .collect(),
        ),
        None => Value::String(String::new()),
    };
    serde_json::json!({
        "type": "tool_result",
        "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
        "content": content,
    })
}

/// 合并所有 System 消息为顶层 `system`（字符串或 block 数组）。
/// 带 cache_control 时输出为数组，否则单个文本输出为字符串。
fn system_to_anthropic(req: &IrRequest) -> Option<Value> {
    let mut parts: Vec<String> = Vec::new();
    for m in &req.messages {
        if m.role != IrRole::System {
            continue;
        }
        match &m.content {
            Some(IrContent::Text(s)) => parts.push(s.clone()),
            Some(IrContent::Parts(ps)) => {
                for p in ps {
                    if let IrPart::Text { text } = p {
                        parts.push(text.clone());
                    }
                }
            }
            None => {}
        }
    }
    if parts.is_empty() {
        return None;
    }
    if let Some(cc) = &req.ext.cache_control {
        let joined = parts.join("");
        let mut b = text_block(&joined);
        b["cache_control"] = cc.clone();
        return Some(Value::Array(vec![b]));
    }
    if parts.len() == 1 {
        return Some(Value::String(parts[0].clone()));
    }
    Some(Value::Array(parts.iter().map(|t| text_block(t)).collect()))
}

/// 遍历 IR 消息产出 Anthropic `messages` 数组。
/// 规则：System 汇入 system（在此跳过）；Tool 合并成用户消息内 tool_result block；
/// user/assistant 直接转换；相邻 Tool 结果与后续 user 合并为单条 user 消息。
fn messages_to_anthropic(req: &IrRequest, ctx: &mut ConvCtx) -> Result<Value, ConvertError> {
    let mut out: Vec<Value> = Vec::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();

    for m in &req.messages {
        match m.role {
            IrRole::System => continue,
            IrRole::User => {
                let mut blocks = std::mem::take(&mut pending_tool_results);
                if let Some(content) = &m.content {
                    blocks.extend(content_to_anthropic_blocks(content, ctx));
                }
                out.push(serde_json::json!({"role": "user", "content": blocks}));
            }
            IrRole::Assistant => {
                if !pending_tool_results.is_empty() {
                    out.push(serde_json::json!({
                        "role": "user",
                        "content": std::mem::take(&mut pending_tool_results),
                    }));
                }
                out.push(message_to_anthropic(m, req, ctx));
            }
            IrRole::Tool => pending_tool_results.push(tool_result_block(m, ctx)),
        }
    }
    if !pending_tool_results.is_empty() {
        out.push(serde_json::json!({
            "role": "user",
            "content": pending_tool_results,
        }));
    }
    Ok(Value::Array(out))
}

/// assistant 消息 → Anthropic 消息（thinking/text/tool_use blocks）。
fn message_to_anthropic(m: &IrMessage, req: &IrRequest, _ctx: &mut ConvCtx) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    if let Some(rc) = &m.reasoning_content {
        blocks.push(thinking_block(rc, req));
    }
    if let Some(content) = &m.content {
        blocks.extend(content_to_anthropic_blocks(content, _ctx));
    }
    for tc in &m.tool_calls {
        blocks.push(tool_use_block(tc));
    }
    serde_json::json!({"role": "assistant", "content": blocks})
}

/// 记录在 Anthropic 无对应能力的字段 → 降级。
fn degrade_request_ext(req: &IrRequest, ctx: &mut ConvCtx) {
    if req.response_format.is_some() {
        ctx.degrade("response_format", "Anthropic 无对应字段");
    }
    if req.seed.is_some() {
        ctx.degrade("seed", "Anthropic 无对应字段");
    }
    if req.n.is_some() {
        ctx.degrade("n", "Anthropic 无对应字段");
    }
    if req.ext.web_search.is_some() {
        ctx.degrade("ext.web_search", "Anthropic 无对应字段");
    }
    if req.ext.service_tier.is_some() {
        ctx.degrade("ext.service_tier", "Anthropic 无对应字段");
    }
    for k in req.ext.extra.keys() {
        // thinking_signature 为跨协议携带的扩展思考签名，用于重建 thinking block，不算降级。
        if k == "thinking_signature" {
            continue;
        }
        ctx.degrade(k.clone(), "Anthropic 无对应字段");
    }
}

/// OpenAI 形态 tool_choice → Anthropic 形态。
fn tool_choice_to_anthropic(tc: &Value) -> Value {
    if let Some(s) = tc.as_str() {
        return match s {
            "auto" => serde_json::json!({"type": "auto"}),
            "required" => serde_json::json!({"type": "any"}),
            _ => serde_json::json!({"type": "auto"}),
        };
    }
    if let Some(o) = tc.as_object() {
        if o.get("type").and_then(|t| t.as_str()) == Some("function") {
            let name = o
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            return serde_json::json!({"type": "tool", "name": name});
        }
        return tc.clone();
    }
    serde_json::json!({"type": "auto"})
}

/// IR → 请求 JSON。max_tokens 缺失时填充 DEFAULT_MAX_TOKENS 并记录降级。
pub fn request_from_ir(req: &IrRequest, ctx: &mut ConvCtx) -> Result<Value, ConvertError> {
    // 未知主体字段先铺底，规范字段再覆盖（保留入口未知字段）。
    let mut body = req.extra.clone();
    degrade_request_ext(req, ctx);

    let max_tokens = req.max_tokens.unwrap_or_else(|| {
        ctx.degrade("max_tokens", "入口未提供，填充默认 65536");
        DEFAULT_MAX_TOKENS
    });

    body.insert("model".into(), Value::String(req.model.clone()));
    if let Some(sys) = system_to_anthropic(req) {
        body.insert("system".into(), sys);
    }
    body.insert("messages".into(), messages_to_anthropic(req, ctx)?);
    body.insert("max_tokens".into(), Value::from(max_tokens));

    if !req.tools.is_empty() {
        body.insert(
            "tools".into(),
            Value::Array(req.tools.iter().map(tool_to_json).collect()),
        );
    }
    if let Some(tc) = &req.tool_choice {
        body.insert("tool_choice".into(), tool_choice_to_anthropic(tc));
    }
    if let Some(t) = req.temperature {
        body.insert("temperature".into(), number_f64(t));
    }
    if let Some(tp) = req.top_p {
        body.insert("top_p".into(), number_f64(tp));
    }
    if let Some(tk) = req.ext.top_k {
        body.insert("top_k".into(), Value::from(tk));
    }
    if let Some(stop) = &req.stop {
        body.insert(
            "stop_sequences".into(),
            Value::Array(stop.iter().map(|s| Value::String(s.clone())).collect()),
        );
    }
    body.insert("stream".into(), Value::Bool(req.stream));
    if let Some(u) = &req.user {
        body.insert("metadata".into(), serde_json::json!({"user_id": u}));
    }
    if let Some(t) = &req.ext.thinking {
        body.insert("thinking".into(), t.clone());
    }

    Ok(Value::Object(body))
}

/// 工具定义 → Anthropic tool（name/description/input_schema）。
fn tool_to_json(t: &IrTool) -> Value {
    let mut o = Map::new();
    o.insert("name".into(), Value::String(t.name.clone()));
    if let Some(d) = &t.description {
        o.insert("description".into(), Value::String(d.clone()));
    }
    o.insert("input_schema".into(), t.parameters.clone());
    Value::Object(o)
}

// ---------------------------------------------------------------------------
// 非流式响应：Anthropic JSON → IR
// ---------------------------------------------------------------------------

/// usage：input_tokens→prompt_tokens、output_tokens→completion_tokens、
/// cache_read_input_tokens→cache_read_tokens、cache_creation_input_tokens→cache_write_tokens。
fn parse_usage(u: &Value) -> IrUsage {
    let mut usage = IrUsage::default();
    usage.prompt_tokens = u["input_tokens"]
        .as_u64()
        .or_else(|| u["prompt_tokens"].as_u64())
        .unwrap_or(0);
    usage.completion_tokens = u["output_tokens"]
        .as_u64()
        .or_else(|| u["completion_tokens"].as_u64())
        .unwrap_or(0);
    usage.cache_read_tokens = u["cache_read_input_tokens"]
        .as_u64()
        .or_else(|| u["cache_read_tokens"].as_u64());
    usage.cache_write_tokens = u["cache_creation_input_tokens"]
        .as_u64()
        .or_else(|| u["cache_creation_tokens"].as_u64());
    usage.total_tokens = u["total_tokens"].as_u64();
    if let Some(o) = u.as_object() {
        for (k, v) in o {
            if !matches!(
                k.as_str(),
                "input_tokens"
                    | "output_tokens"
                    | "prompt_tokens"
                    | "completion_tokens"
                    | "total_tokens"
                    | "cache_read_input_tokens"
                    | "cache_read_tokens"
                    | "cache_creation_input_tokens"
                    | "cache_creation_tokens"
            ) {
                usage.extra.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
    }
    usage
}

/// 解析响应 content blocks 为单条 assistant 消息。
/// text 拼接为 IrContent（单个 text 回落 Text，多文本/含图则 Parts）；tool_use→tool_calls；
/// thinking→reasoning_content；signature 存响应级 extra["thinking_signature"]。
fn parse_response_content(
    content: &Value,
    resp: &mut IrResponse,
    ctx: &mut ConvCtx,
) -> Result<IrMessage, ConvertError> {
    let arr = content
        .as_array()
        .ok_or_else(|| ConvertError::Parse("响应 content 不是数组".into()))?;
    let mut parts: Vec<IrPart> = Vec::new();
    let mut tool_calls: Vec<IrToolCall> = Vec::new();
    let mut reasoning: Option<String> = None;

    for block in arr {
        if !block.is_object() {
            continue;
        }
        match block["type"].as_str().unwrap_or_default() {
            "text" => parts.push(IrPart::Text {
                text: block["text"].as_str().unwrap_or_default().to_string(),
            }),
            "tool_use" => tool_calls.push(IrToolCall {
                id: block["id"].as_str().unwrap_or_default().to_string(),
                name: block["name"].as_str().unwrap_or_default().to_string(),
                arguments: input_to_arguments(&block["input"]),
            }),
            "thinking" => {
                reasoning = Some(block["thinking"].as_str().unwrap_or_default().to_string());
                if let Some(sig) = block["signature"].as_str() {
                    resp.extra
                        .insert("thinking_signature".into(), Value::String(sig.to_string()));
                }
            }
            other => ctx.degrade("content.block.type", format!("未知响应块类型: {other}")),
        }
    }

    Ok(IrMessage {
        role: IrRole::Assistant,
        content: parts_content(parts),
        tool_calls,
        reasoning_content: reasoning,
        ..Default::default()
    })
}

/// 非流式响应 JSON → IR。stop_reason 归一化后放 finish_reason，原始值放 extra["anthropic_stop_reason"]。
pub fn response_to_ir(v: &Value, ctx: &mut ConvCtx) -> Result<IrResponse, ConvertError> {
    let obj = v
        .as_object()
        .ok_or_else(|| ConvertError::Parse("响应体不是对象".into()))?;
    let mut resp = IrResponse::default();
    resp.id = v["id"].as_str().unwrap_or_default().to_string();
    resp.model = v["model"].as_str().unwrap_or_default().to_string();
    resp.created = v["created"].as_i64().unwrap_or(0); // Anthropic 无 created

    if let Some(content) = v.get("content") {
        let msg = parse_response_content(content, &mut resp, ctx).unwrap_or_else(|_| {
            ctx.degrade("content", "响应 content 解析失败，置空");
            IrMessage::default()
        });
        resp.choices.push(IrChoice {
            index: 0,
            message: msg,
            finish_reason: None,
        });
    }
    if let Some(sr) = v["stop_reason"].as_str() {
        if let Some(ch) = resp.choices.first_mut() {
            ch.finish_reason = Some(normalize_finish_reason(sr));
        }
        resp.extra.insert(
            "anthropic_stop_reason".into(),
            Value::String(sr.to_string()),
        );
    }
    if let Some(u) = v.get("usage") {
        resp.usage = Some(parse_usage(u));
    }

    // 未知响应字段进 extra（stop_reason 已单独用 anthropic_stop_reason 保留原始值）
    for (k, val) in obj {
        if !KNOWN_RESPONSE_FIELDS.contains(&k.as_str()) {
            resp.extra.insert(k.clone(), val.clone());
        }
    }

    Ok(resp)
}

// ---------------------------------------------------------------------------
// 非流式响应：IR → Anthropic JSON
// ---------------------------------------------------------------------------

/// finish_reason 反归一化为 Anthropic stop_reason 词汇。
fn reverse_finish_reason(fr: &str) -> String {
    match fr {
        "stop" => "end_turn".into(),
        "length" => "max_tokens".into(),
        "tool_calls" => "tool_use".into(),
        other => other.into(),
    }
}

fn usage_to_anthropic(u: &IrUsage, ctx: &mut ConvCtx) -> Value {
    let mut o = u.extra.clone();
    o.insert("input_tokens".into(), Value::from(u.prompt_tokens));
    o.insert("output_tokens".into(), Value::from(u.completion_tokens));
    if let Some(c) = u.cache_read_tokens {
        o.insert("cache_read_input_tokens".into(), Value::from(c));
    }
    if let Some(c) = u.cache_write_tokens {
        o.insert("cache_creation_input_tokens".into(), Value::from(c));
    }
    if u.total_tokens.is_some() {
        ctx.degrade("usage.total_tokens", "Anthropic 无 total_tokens");
    }
    Value::Object(o)
}

/// assistant 消息 → 响应 content blocks（thinking/text/tool_use）。
fn response_blocks(msg: &IrMessage, resp: &IrResponse, ctx: &mut ConvCtx) -> Vec<Value> {
    let mut blocks: Vec<Value> = Vec::new();
    if let Some(rc) = &msg.reasoning_content {
        let mut b = serde_json::json!({"type": "thinking", "thinking": rc});
        if let Some(sig) = resp
            .extra
            .get("thinking_signature")
            .and_then(|v| v.as_str())
        {
            b["signature"] = Value::String(sig.to_string());
        }
        blocks.push(b);
    }
    if let Some(content) = &msg.content {
        blocks.extend(content_to_anthropic_blocks(content, ctx));
    }
    for tc in &msg.tool_calls {
        blocks.push(tool_use_block(tc));
    }
    blocks
}

/// IR → 非流式响应 JSON。
pub fn response_from_ir(resp: &IrResponse, ctx: &mut ConvCtx) -> Result<Value, ConvertError> {
    let mut body = resp.extra.clone();
    // 移除内部标记，避免作为未知顶层字段导出。
    body.remove("anthropic_stop_reason");
    body.remove("thinking_signature");

    body.insert("id".into(), Value::String(resp.id.clone()));
    body.insert("type".into(), Value::String("message".into()));
    body.insert("role".into(), Value::String("assistant".into()));

    let (msg, finish_reason) = match resp.choices.first() {
        Some(c) => (&c.message, c.finish_reason.as_deref()),
        None => (&IrMessage::default(), None),
    };
    body.insert(
        "content".into(),
        Value::Array(response_blocks(msg, resp, ctx)),
    );
    body.insert("model".into(), Value::String(resp.model.clone()));

    let sr = finish_reason
        .map(reverse_finish_reason)
        .unwrap_or_else(|| "end_turn".into());
    body.insert("stop_reason".into(), Value::String(sr));
    body.insert("stop_sequence".into(), Value::Null);

    if let Some(u) = &resp.usage {
        body.insert("usage".into(), usage_to_anthropic(u, ctx));
    }

    Ok(Value::Object(body))
}

// ---------------------------------------------------------------------------
// 流式 chunk
// ---------------------------------------------------------------------------

/// Anthropic 流式状态（双向复用）：
/// - 入站（Anthropic 流 → IR chunk）：id/model、usage 累积、content block 索引 ↔ 工具序号映射。
/// - 出站（IR chunk → Anthropic 事件）：message_start 是否已发、各内容块 open/index、消息停止标记。
#[derive(Debug, Default)]
pub struct StreamState {
    pub id: String,
    pub model: String,
    pub usage: IrUsage,
    // 入站：content block index → 工具调用序号
    pub block_to_tool: HashMap<u64, u32>,
    pub next_tool_index: u32,
    // 出站：内容块编排
    pub message_start_emitted: bool,
    pub message_stop_emitted: bool,
    pub next_block_index: u32,
    pub text_block_open: bool,
    pub text_block_index: u32,
    pub thinking_block_open: bool,
    pub thinking_block_index: u32,
    pub tool_block_open: HashMap<u32, u32>,
}

fn event_str(v: Value) -> Result<String, ConvertError> {
    serde_json::to_string(&v).map_err(|e| ConvertError::Parse(e.to_string()))
}

/// SSE data 载荷 → IR chunk；无业务内容（ping/text 空帧/stop）返回 Ok(None)。
pub fn chunk_to_ir(
    data: &str,
    st: &mut StreamState,
    ctx: &mut ConvCtx,
) -> Result<Option<IrChunk>, ConvertError> {
    let v: Value = serde_json::from_str(data).map_err(|e| ConvertError::Parse(e.to_string()))?;
    let _obj = v
        .as_object()
        .ok_or_else(|| ConvertError::Parse("chunk 不是对象".into()))?;
    let etype = v["type"].as_str().unwrap_or_default();

    match etype {
        "message_start" => {
            let msg = &v["message"];
            st.id = msg["id"].as_str().unwrap_or_default().to_string();
            st.model = msg["model"].as_str().unwrap_or_default().to_string();
            if let Some(u) = msg.get("usage") {
                st.usage = parse_usage(u);
            }
            let mut chunk = IrChunk::default();
            chunk.id = st.id.clone();
            chunk.model = st.model.clone();
            let delta = IrDelta {
                role: Some(IrRole::Assistant),
                ..Default::default()
            };
            chunk.choices.push(IrChunkChoice {
                index: 0,
                delta,
                finish_reason: None,
            });
            Ok(Some(chunk))
        }
        "content_block_start" => {
            let idx = v["index"].as_u64().unwrap_or(0);
            let bt = v["content_block"]["type"].as_str().unwrap_or_default();
            match bt {
                // text/thinking 空帧跳过（其增量经 content_block_delta 给出）
                "text" => Ok(None),
                "thinking" => Ok(None),
                "tool_use" => {
                    let tool_idx = st.next_tool_index;
                    st.next_tool_index += 1;
                    st.block_to_tool.insert(idx, tool_idx);
                    let mut chunk = IrChunk::default();
                    chunk.id = st.id.clone();
                    chunk.model = st.model.clone();
                    let delta = IrDelta {
                        tool_calls: vec![IrToolCallDelta {
                            index: tool_idx,
                            id: v["content_block"]["id"].as_str().map(String::from),
                            name: v["content_block"]["name"].as_str().map(String::from),
                            arguments: None,
                        }],
                        ..Default::default()
                    };
                    chunk.choices.push(IrChunkChoice {
                        index: 0,
                        delta,
                        finish_reason: None,
                    });
                    Ok(Some(chunk))
                }
                other => {
                    ctx.degrade(
                        "stream.content_block_start.type",
                        format!("未知内容块类型: {other}"),
                    );
                    Ok(None)
                }
            }
        }
        "content_block_delta" => {
            let idx = v["index"].as_u64().unwrap_or(0);
            let dt = v["delta"]["type"].as_str().unwrap_or_default();
            let mut delta = IrDelta::default();
            match dt {
                "text_delta" => {
                    delta.content =
                        Some(v["delta"]["text"].as_str().unwrap_or_default().to_string())
                }
                "thinking_delta" => {
                    delta.reasoning_content = Some(
                        v["delta"]["thinking"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                }
                "input_json_delta" => {
                    let tool_idx = st.block_to_tool.get(&idx).copied().unwrap_or(0);
                    delta.tool_calls.push(IrToolCallDelta {
                        index: tool_idx,
                        id: None,
                        name: None,
                        arguments: Some(
                            v["delta"]["partial_json"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        ),
                    });
                }
                other => {
                    ctx.degrade(
                        "stream.content_block_delta.type",
                        format!("未知增量类型: {other}"),
                    );
                    return Ok(None);
                }
            }
            let mut chunk = IrChunk::default();
            chunk.id = st.id.clone();
            chunk.model = st.model.clone();
            chunk.choices.push(IrChunkChoice {
                index: 0,
                delta,
                finish_reason: None,
            });
            Ok(Some(chunk))
        }
        "content_block_stop" => {
            if let Some(idx) = v["index"].as_u64() {
                st.block_to_tool.remove(&idx);
            }
            Ok(None)
        }
        "message_delta" => {
            if let Some(u) = v.get("usage") {
                // output_tokens 为「截至当前累计」值：多次 delta 用 max 而非累加，防虚增（review P2-5）
                let ot = u["output_tokens"].as_u64().unwrap_or(0);
                st.usage.completion_tokens = st.usage.completion_tokens.max(ot);
                if let Some(c) = u["cache_read_input_tokens"].as_u64() {
                    st.usage.cache_read_tokens = Some(c);
                }
                if let Some(c) = u["cache_creation_input_tokens"].as_u64() {
                    st.usage.cache_write_tokens = Some(c);
                }
            }
            let mut chunk = IrChunk::default();
            chunk.id = st.id.clone();
            chunk.model = st.model.clone();
            let mut choice = IrChunkChoice::default();
            if let Some(sr) = v["delta"]["stop_reason"].as_str() {
                choice.finish_reason = Some(normalize_finish_reason(sr));
            }
            choice.delta = IrDelta::default();
            chunk.choices.push(choice);
            chunk.usage = Some(st.usage.clone());
            Ok(Some(chunk))
        }
        "message_stop" => {
            st.message_stop_emitted = true;
            Ok(None)
        }
        "ping" => Ok(None),
        "error" => Err(ConvertError::Parse(format!(
            "Anthropic 流错误: {}",
            v["error"].to_string()
        ))),
        other => {
            ctx.degrade("stream.event.type", format!("未知事件: {other}"));
            Ok(None)
        }
    }
}

fn message_start_event(st: &StreamState) -> Value {
    let mut usage = Map::new();
    usage.insert("input_tokens".into(), Value::from(st.usage.prompt_tokens));
    usage.insert(
        "output_tokens".into(),
        Value::from(st.usage.completion_tokens),
    );
    if let Some(c) = st.usage.cache_read_tokens {
        usage.insert("cache_read_input_tokens".into(), Value::from(c));
    }
    if let Some(c) = st.usage.cache_write_tokens {
        usage.insert("cache_creation_input_tokens".into(), Value::from(c));
    }
    serde_json::json!({
        "type": "message_start",
        "message": {
            "id": st.id,
            "type": "message",
            "role": "assistant",
            "model": st.model,
            "content": [],
            "usage": usage,
            "stop_reason": null,
            "stop_sequence": null,
        }
    })
}

fn content_block_start_text(idx: u32) -> Value {
    serde_json::json!({"type": "content_block_start", "index": idx, "content_block": {"type": "text", "text": ""}})
}

fn content_block_delta_text(idx: u32, text: &str) -> Value {
    serde_json::json!({"type": "content_block_delta", "index": idx, "delta": {"type": "text_delta", "text": text}})
}

fn content_block_start_thinking(idx: u32) -> Value {
    serde_json::json!({"type": "content_block_start", "index": idx, "content_block": {"type": "thinking", "thinking": "", "signature": ""}})
}

fn content_block_delta_thinking(idx: u32, thinking: &str) -> Value {
    serde_json::json!({"type": "content_block_delta", "index": idx, "delta": {"type": "thinking_delta", "thinking": thinking}})
}

fn content_block_start_tool_use(idx: u32, tc: &IrToolCallDelta) -> Value {
    let mut cb = Map::new();
    cb.insert("type".into(), Value::String("tool_use".into()));
    cb.insert(
        "id".into(),
        Value::String(tc.id.clone().unwrap_or_default()),
    );
    cb.insert(
        "name".into(),
        Value::String(tc.name.clone().unwrap_or_default()),
    );
    cb.insert("input".into(), json_object());
    serde_json::json!({"type": "content_block_start", "index": idx, "content_block": Value::Object(cb)})
}

fn content_block_delta_input_json(idx: u32, tc: &IrToolCallDelta) -> Value {
    serde_json::json!({
        "type": "content_block_delta",
        "index": idx,
        "delta": {"type": "input_json_delta", "partial_json": tc.arguments.clone().unwrap_or_default()},
    })
}

fn content_block_stop(idx: u32) -> Value {
    serde_json::json!({"type": "content_block_stop", "index": idx})
}

fn message_delta_event(fr: &str, st: &StreamState) -> Value {
    let sr = reverse_finish_reason(fr);
    let mut usage = Map::new();
    usage.insert(
        "output_tokens".into(),
        Value::from(st.usage.completion_tokens),
    );
    serde_json::json!({
        "type": "message_delta",
        "delta": {"stop_reason": sr, "stop_sequence": null},
        "usage": usage,
    })
}

fn message_stop_event() -> Value {
    serde_json::json!({"type": "message_stop"})
}

/// IR chunk → Anthropic 事件 JSON 字符串列表。事件序列保证合法（start→delta*→stop）。
pub fn chunk_from_ir(
    chunk: &IrChunk,
    st: &mut StreamState,
    _ctx: &mut ConvCtx,
) -> Result<Vec<String>, ConvertError> {
    let mut events: Vec<String> = Vec::new();

    // 首个 chunk 产出 message_start
    if !st.message_start_emitted {
        st.message_start_emitted = true;
        st.id = chunk.id.clone();
        st.model = chunk.model.clone();
        if let Some(u) = &chunk.usage {
            st.usage.prompt_tokens = u.prompt_tokens;
            st.usage.cache_read_tokens = u.cache_read_tokens;
            st.usage.cache_write_tokens = u.cache_write_tokens;
        }
        events.push(event_str(message_start_event(st))?);
    }

    let (delta, finish_reason) = match chunk.choices.first() {
        Some(c) => (&c.delta, c.finish_reason.as_deref()),
        None => (&IrDelta::default(), None),
    };

    // 文本增量
    if let Some(text) = &delta.content {
        if !st.text_block_open {
            let idx = st.next_block_index;
            st.next_block_index += 1;
            st.text_block_open = true;
            st.text_block_index = idx;
            events.push(event_str(content_block_start_text(idx))?);
        }
        events.push(event_str(content_block_delta_text(
            st.text_block_index,
            text,
        ))?);
    }

    // 思考增量
    if let Some(rc) = &delta.reasoning_content {
        if !st.thinking_block_open {
            let idx = st.next_block_index;
            st.next_block_index += 1;
            st.thinking_block_open = true;
            st.thinking_block_index = idx;
            events.push(event_str(content_block_start_thinking(idx))?);
        }
        events.push(event_str(content_block_delta_thinking(
            st.thinking_block_index,
            rc,
        ))?);
    }

    // 工具调用增量（同一 index 只发一次 content_block_start）
    for tc in &delta.tool_calls {
        let block_idx = if let Some(&b) = st.tool_block_open.get(&tc.index) {
            b
        } else {
            let b = st.next_block_index;
            st.next_block_index += 1;
            st.tool_block_open.insert(tc.index, b);
            events.push(event_str(content_block_start_tool_use(b, tc))?);
            b
        };
        events.push(event_str(content_block_delta_input_json(block_idx, tc))?);
    }

    // usage 累积（IR usage 为截至该 chunk 的累计值 → 取 max，防多次携带时翻倍；review P2-5）
    if let Some(u) = &chunk.usage {
        st.usage.prompt_tokens = u.prompt_tokens;
        st.usage.completion_tokens = st.usage.completion_tokens.max(u.completion_tokens);
        st.usage.cache_read_tokens = u.cache_read_tokens.or(st.usage.cache_read_tokens);
        st.usage.cache_write_tokens = u.cache_write_tokens.or(st.usage.cache_write_tokens);
    }

    // finish_reason：关闭所有 open block，再发 message_delta
    if let Some(fr) = finish_reason {
        if st.text_block_open {
            events.push(event_str(content_block_stop(st.text_block_index))?);
            st.text_block_open = false;
        }
        if st.thinking_block_open {
            events.push(event_str(content_block_stop(st.thinking_block_index))?);
            st.thinking_block_open = false;
        }
        let mut keys: Vec<u32> = st.tool_block_open.keys().cloned().collect();
        keys.sort_unstable();
        for k in keys {
            let b = st.tool_block_open.remove(&k).unwrap();
            events.push(event_str(content_block_stop(b))?);
        }
        events.push(event_str(message_delta_event(fr, st))?);
    }

    Ok(events)
}

/// 流终止：产出 message_stop（若 st 中已发过则空）。
pub fn stream_end(st: &mut StreamState, _ctx: &mut ConvCtx) -> Result<Vec<String>, ConvertError> {
    if st.message_stop_emitted {
        return Ok(vec![]);
    }
    st.message_stop_emitted = true;
    Ok(vec![event_str(message_stop_event())?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ConvCtx {
        ConvCtx::new()
    }

    #[test]
    fn text_request_roundtrip() {
        let j = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "max_tokens": 1024,
            "system": "You are helpful.",
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there"}
            ],
            "temperature": 0.7,
            "top_p": 0.9,
            "top_k": 40,
            "stop_sequences": ["END"],
            "stream": false,
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.model, "claude-3-5-sonnet");
        assert_eq!(req.max_tokens, Some(1024));
        assert_eq!(req.messages[0].role, IrRole::System);
        assert_eq!(
            req.messages[0].content,
            Some(IrContent::Text("You are helpful.".into()))
        );
        assert_eq!(req.messages[1].role, IrRole::User);
        assert_eq!(
            req.messages[1].content,
            Some(IrContent::Text("Hello".into()))
        );
        assert_eq!(
            req.messages[2].content,
            Some(IrContent::Text("Hi there".into()))
        );
        assert_eq!(req.ext.top_k, Some(40));
        assert_eq!(req.stop, Some(vec!["END".to_string()]));

        let back = request_from_ir(&req, &mut c).unwrap();
        let req2 = request_to_ir(&back, &mut c).unwrap();
        assert_eq!(req, req2);
    }

    #[test]
    fn image_url_and_base64_roundtrip() {
        let j = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "max_tokens": 100,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "what is this?"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAABBBB"}},
                    {"type": "image", "source": {"type": "url", "url": "https://example.com/a.png"}}
                ]
            }]
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        let content = req.messages[0].content.as_ref().unwrap();
        let parts = match content {
            IrContent::Parts(p) => p,
            other => panic!("应为 Parts: {other:?}"),
        };
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts[1],
            IrPart::ImageInline {
                media_type: "image/png".into(),
                data: "AAAABBBB".into()
            }
        );
        assert_eq!(
            parts[2],
            IrPart::ImageUrl {
                url: "https://example.com/a.png".into()
            }
        );

        let back = request_from_ir(&req, &mut c).unwrap();
        let req2 = request_to_ir(&back, &mut c).unwrap();
        assert_eq!(req, req2);
    }

    #[test]
    fn tools_definitions_calls_and_results() {
        let j = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": "weather in bj?"},
                {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "thinking": "need tool", "signature": "sig_abc"},
                        {"type": "text", "text": "let me check"},
                        {"type": "tool_use", "id": "toolu_1", "name": "get_weather", "input": {"city": "bj"}}
                    ]
                },
                {
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "sunny"}]
                }
            ],
            "tools": [{
                "name": "get_weather",
                "description": "get weather",
                "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}}
            }],
            "tool_choice": {"type": "auto"}
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].name, "get_weather");
        assert_eq!(
            req.tools[0].parameters["properties"]["city"]["type"],
            "string"
        );
        assert_eq!(req.tool_choice, Some(serde_json::json!("auto")));

        let asst = &req.messages[1];
        assert_eq!(asst.reasoning_content.as_deref(), Some("need tool"));
        assert_eq!(asst.content, Some(IrContent::Text("let me check".into())));
        assert_eq!(asst.tool_calls.len(), 1);
        assert_eq!(asst.tool_calls[0].id, "toolu_1");
        assert_eq!(asst.tool_calls[0].name, "get_weather");
        assert_eq!(asst.tool_calls[0].arguments, "{\"city\":\"bj\"}");

        // tool_result → 独立 Tool 消息
        let tool_msg = req.messages[2].clone();
        assert_eq!(tool_msg.role, IrRole::Tool);
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("toolu_1"));
        assert_eq!(tool_msg.content, Some(IrContent::Text("sunny".into())));

        let back = request_from_ir(&req, &mut c).unwrap();
        let req2 = request_to_ir(&back, &mut c).unwrap();
        assert_eq!(req, req2);
    }

    #[test]
    fn system_with_cache_control() {
        let j = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "max_tokens": 100,
            "system": [{
                "type": "text",
                "text": "You are a helpful assistant",
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [{"role": "user", "content": "hi"}]
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.messages[0].role, IrRole::System);
        assert_eq!(
            req.messages[0].content,
            Some(IrContent::Text("You are a helpful assistant".into()))
        );
        assert_eq!(
            req.ext.cache_control,
            Some(serde_json::json!({"type": "ephemeral"}))
        );

        let back = request_from_ir(&req, &mut c).unwrap();
        assert!(back["system"].is_array());
        assert_eq!(back["system"][0]["text"], "You are a helpful assistant");
        assert_eq!(back["system"][0]["cache_control"]["type"], "ephemeral");

        let req2 = request_to_ir(&back, &mut c).unwrap();
        assert_eq!(req, req2);
    }

    #[test]
    fn thinking_signature_preserved() {
        let j = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "max_tokens": 100,
            "thinking": {"type": "enabled", "budget_tokens": 1024},
            "messages": [
                {"role": "user", "content": "solve"},
                {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "thinking": "let me think", "signature": "sig_xyz"},
                        {"type": "text", "text": "answer"}
                    ]
                }
            ]
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(
            req.ext.thinking,
            Some(serde_json::json!({"type": "enabled", "budget_tokens": 1024}))
        );
        let asst = &req.messages[1];
        assert_eq!(asst.reasoning_content.as_deref(), Some("let me think"));
        assert_eq!(req.ext.extra["thinking_signature"], "sig_xyz");

        // 回写保留 signature 与 thinking 顶层字段
        let back = request_from_ir(&req, &mut c).unwrap();
        assert_eq!(back["thinking"]["type"], "enabled");
        let content = back["messages"][1]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "let me think");
        assert_eq!(content[0]["signature"], "sig_xyz");

        let req2 = request_to_ir(&back, &mut c).unwrap();
        assert_eq!(req, req2);
    }

    #[test]
    fn max_tokens_default_fill_and_degrade() {
        let req = IrRequest {
            model: "m".into(),
            messages: vec![IrMessage {
                role: IrRole::User,
                content: Some(IrContent::Text("hi".into())),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut c = ctx();
        let back = request_from_ir(&req, &mut c).unwrap();
        assert_eq!(back["max_tokens"], 65536);
        assert!(c.degraded.iter().any(|d| d.field == "max_tokens"));
    }

    #[test]
    fn degrade_on_unmappable_fields() {
        let req = IrRequest {
            model: "m".into(),
            response_format: Some(serde_json::json!({"type": "json"})),
            seed: Some(42),
            n: Some(1),
            ext: IrExt {
                web_search: Some(serde_json::json!({"a": 1})),
                service_tier: Some("scale".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut c = ctx();
        request_from_ir(&req, &mut c).unwrap();
        let fields: Vec<&str> = c.degraded.iter().map(|d| d.field.as_str()).collect();
        assert!(fields.contains(&"response_format"));
        assert!(fields.contains(&"seed"));
        assert!(fields.contains(&"n"));
        assert!(fields.contains(&"ext.web_search"));
        assert!(fields.contains(&"ext.service_tier"));
    }

    #[test]
    fn response_roundtrip_with_usage_cache() {
        let j = serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "hi there"},
                {"type": "tool_use", "id": "toolu_2", "name": "get_weather", "input": {"city": "bj"}}
            ],
            "model": "claude-3-5-sonnet",
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_read_input_tokens": 3,
                "cache_creation_input_tokens": 2
            }
        });
        let mut c = ctx();
        let resp = response_to_ir(&j, &mut c).unwrap();
        assert_eq!(resp.id, "msg_1");
        assert_eq!(resp.model, "claude-3-5-sonnet");
        let msg = &resp.choices[0].message;
        assert_eq!(msg.content, Some(IrContent::Text("hi there".into())));
        assert_eq!(msg.tool_calls[0].id, "toolu_2");
        assert_eq!(msg.tool_calls[0].arguments, "{\"city\":\"bj\"}");
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(resp.extra["anthropic_stop_reason"], "tool_use");

        let u = resp.usage.as_ref().unwrap();
        assert_eq!(u.prompt_tokens, 10);
        assert_eq!(u.completion_tokens, 5);
        assert_eq!(u.cache_read_tokens, Some(3));
        assert_eq!(u.cache_write_tokens, Some(2));

        let back = response_from_ir(&resp, &mut c).unwrap();
        assert_eq!(back["stop_reason"], "tool_use");
        let uj = &back["usage"];
        assert_eq!(uj["input_tokens"], 10);
        assert_eq!(uj["output_tokens"], 5);
        assert_eq!(uj["cache_read_input_tokens"], 3);
        assert_eq!(uj["cache_creation_input_tokens"], 2);

        let resp2 = response_to_ir(&back, &mut c).unwrap();
        assert_eq!(resp, resp2);
    }

    #[test]
    fn streaming_tool_call_aggregation() {
        let mut c = ctx();

        // 构造 IR chunk 序列（角色 → 文本 → 工具增量 → 结束 + usage）
        let chunks = vec![
            IrChunk {
                id: "msg_s1".into(),
                model: "claude-3-5-sonnet".into(),
                usage: Some(IrUsage {
                    prompt_tokens: 10,
                    cache_read_tokens: Some(5),
                    ..Default::default()
                }),
                choices: vec![IrChunkChoice {
                    index: 0,
                    delta: IrDelta {
                        role: Some(IrRole::Assistant),
                        ..Default::default()
                    },
                    finish_reason: None,
                }],
                ..Default::default()
            },
            IrChunk {
                id: "msg_s1".into(),
                model: "claude-3-5-sonnet".into(),
                choices: vec![IrChunkChoice {
                    index: 0,
                    delta: IrDelta {
                        content: Some("Hello".into()),
                        ..Default::default()
                    },
                    finish_reason: None,
                }],
                ..Default::default()
            },
            IrChunk {
                id: "msg_s1".into(),
                model: "claude-3-5-sonnet".into(),
                choices: vec![IrChunkChoice {
                    index: 0,
                    delta: IrDelta {
                        tool_calls: vec![IrToolCallDelta {
                            index: 0,
                            id: Some("toolu_s1".into()),
                            name: Some("get_weather".into()),
                            arguments: Some("{\"city\":".into()),
                        }],
                        ..Default::default()
                    },
                    finish_reason: None,
                }],
                ..Default::default()
            },
            IrChunk {
                id: "msg_s1".into(),
                model: "claude-3-5-sonnet".into(),
                choices: vec![IrChunkChoice {
                    index: 0,
                    delta: IrDelta {
                        tool_calls: vec![IrToolCallDelta {
                            index: 0,
                            arguments: Some("\"bj\"}".into()),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    finish_reason: None,
                }],
                ..Default::default()
            },
            IrChunk {
                id: "msg_s1".into(),
                model: "claude-3-5-sonnet".into(),
                choices: vec![IrChunkChoice {
                    index: 0,
                    delta: IrDelta {
                        content: Some(" world".into()),
                        ..Default::default()
                    },
                    finish_reason: None,
                }],
                ..Default::default()
            },
            IrChunk {
                id: "msg_s1".into(),
                model: "claude-3-5-sonnet".into(),
                usage: Some(IrUsage {
                    completion_tokens: 3,
                    ..Default::default()
                }),
                choices: vec![IrChunkChoice {
                    index: 0,
                    delta: IrDelta::default(),
                    finish_reason: Some("tool_calls".into()),
                }],
                ..Default::default()
            },
        ];

        // 出站：IR chunk → Anthropic 事件
        let mut st = StreamState::default();
        let mut events: Vec<String> = Vec::new();
        for ch in &chunks {
            let evs = chunk_from_ir(ch, &mut st, &mut c).unwrap();
            events.extend(evs);
        }
        // 流终止事件（message_stop）
        events.extend(stream_end(&mut st, &mut c).unwrap());

        // 入站：Anthropic 事件 → IR chunk（新状态机）
        let mut st2 = StreamState::default();
        let mut out_chunks: Vec<IrChunk> = Vec::new();
        for ev in &events {
            if let Some(ch) = chunk_to_ir(ev, &mut st2, &mut c).unwrap() {
                out_chunks.push(ch);
            }
        }

        // 聚合工具调用增量验证
        let mut agg = ToolCallAggregator::new();
        for ch in &out_chunks {
            for choice in &ch.choices {
                agg.feed_all(&choice.delta.tool_calls);
            }
        }
        let calls = agg.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_s1");
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].arguments, "{\"city\":\"bj\"}");

        // 文本增量聚合
        let mut text = String::new();
        for ch in &out_chunks {
            for choice in &ch.choices {
                if let Some(t) = &choice.delta.content {
                    text.push_str(t);
                }
            }
        }
        assert_eq!(text, "Hello world");

        // 首 chunk 仅含 role=Assistant delta
        assert_eq!(out_chunks[0].choices[0].delta.role, Some(IrRole::Assistant));
        // 末 chunk 携带归一化 finish_reason 与 usage
        let last = out_chunks.last().unwrap();
        assert_eq!(last.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        let u = last.usage.as_ref().unwrap();
        assert_eq!(u.prompt_tokens, 10);
        assert_eq!(u.completion_tokens, 3);
        assert_eq!(u.cache_read_tokens, Some(5));
    }

    #[test]
    fn streaming_thinking_delta() {
        let mut c = ctx();
        let mut st = StreamState::default();

        // 角色 + 思考增量 + finish
        let chunks = vec![
            IrChunk {
                id: "msg_t".into(),
                model: "m".into(),
                choices: vec![IrChunkChoice {
                    index: 0,
                    delta: IrDelta {
                        role: Some(IrRole::Assistant),
                        ..Default::default()
                    },
                    finish_reason: None,
                }],
                ..Default::default()
            },
            IrChunk {
                id: "msg_t".into(),
                model: "m".into(),
                choices: vec![IrChunkChoice {
                    index: 0,
                    delta: IrDelta {
                        reasoning_content: Some("thinking...".into()),
                        ..Default::default()
                    },
                    finish_reason: None,
                }],
                ..Default::default()
            },
            IrChunk {
                id: "msg_t".into(),
                model: "m".into(),
                choices: vec![IrChunkChoice {
                    index: 0,
                    delta: IrDelta::default(),
                    finish_reason: Some("stop".into()),
                }],
                ..Default::default()
            },
        ];

        let mut events: Vec<String> = Vec::new();
        for ch in &chunks {
            events.extend(chunk_from_ir(ch, &mut st, &mut c).unwrap());
        }
        events.extend(stream_end(&mut st, &mut c).unwrap());

        let mut st2 = StreamState::default();
        let mut reasoning = String::new();
        for ev in &events {
            if let Some(ch) = chunk_to_ir(ev, &mut st2, &mut c).unwrap() {
                for choice in &ch.choices {
                    if let Some(r) = &choice.delta.reasoning_content {
                        reasoning.push_str(r);
                    }
                }
            }
        }
        assert_eq!(reasoning, "thinking...");
    }

    #[test]
    fn stream_end_idempotent() {
        let mut st = StreamState::default();
        let mut c = ctx();
        let e1 = stream_end(&mut st, &mut c).unwrap();
        assert_eq!(e1.len(), 1);
        // 第二次调用不再产出
        let e2 = stream_end(&mut st, &mut c).unwrap();
        assert!(e2.is_empty());
    }
}
