#![allow(clippy::field_reassign_with_default)]

//! OpenAI Responses 适配器（/v1/responses）。
//!
//! 要点（PLAN.md §4.2/§4.4）：
//! - 系统提示 → `instructions`；输入为 `input` 数组（message / function_call_output 等 item）；
//! - 输出 item 类型：message / reasoning / function_call；usage 为 input/output_tokens(+details)；
//! - 有状态能力（previous_response_id、store 等）转为其他协议时列为降级能力；
//! - 流式：response.* 事件序列（response.output_text.delta / response.completed 等）。
//!
//! 转换策略（§4.3）：能直接映射的字段直接映射；Responses 无对应能力的字段（如
//! seed/n 等）记 `ctx.degrade`；Responses 有状态/专属字段（previous_response_id、
//! store/include/background、input.reasoning）作为入口转其他协议时列为降级能力，
//! 值仍原样保留到 `IrRequest.extra` 以便回写。

use std::collections::HashMap;

use serde_json::{Map, Value};

use super::ir::*;
use super::{ConvCtx, ConvertError};

/// 请求体中被本适配器识别并映射的顶层字段；其余进 `extra`。
const KNOWN_REQUEST_FIELDS: [&str; 10] = [
    "model",
    "instructions",
    "input",
    "max_output_tokens",
    "temperature",
    "top_p",
    "text",
    "tools",
    "tool_choice",
    "stream",
];

/// 响应体顶层规范字段；其余进 `extra`。
const KNOWN_RESPONSE_FIELDS: [&str; 7] = [
    "id",
    "object",
    "created_at",
    "status",
    "model",
    "output",
    "usage",
];

fn json_object() -> Value {
    Value::Object(Map::new())
}

fn json_number_f64(v: f64) -> Value {
    serde_json::Number::from_f64(v)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// 将 serde_json::Value 序列化为字符串（用于 SSE data 载荷）。
fn json_to_string(v: &Value) -> Result<String, ConvertError> {
    serde_json::to_string(v).map_err(|e| ConvertError::Parse(e.to_string()))
}

/// 角色 → Responses 字符串（user/assistant；system 不回写进 input）。
fn role_to_response_str(r: IrRole) -> &'static str {
    match r {
        IrRole::System => "system",
        IrRole::User => "user",
        IrRole::Assistant => "assistant",
        IrRole::Tool => "tool",
    }
}

/// 解析输入 message item 的角色；未知角色记降级，缺省按 user 容错。
fn parse_response_role(role: Option<&str>, ctx: &mut ConvCtx) -> IrRole {
    match role {
        Some("user") => IrRole::User,
        Some("assistant") => IrRole::Assistant,
        // developer 为 o 系/gpt-5 的系统消息角色（review P5：此前按 user 降级，系统提示语义丢失）
        Some("system") | Some("developer") => IrRole::System,
        Some(other) => {
            ctx.degrade(
                "input.message.role",
                format!("未知角色，按 user 处理: {other}"),
            );
            IrRole::User
        }
        _ => IrRole::User,
    }
}

/// 将可能的 arguments（字符串或对象）归一为 JSON 字符串。
fn parse_arguments_any(v: &Value) -> String {
    if v.is_string() {
        v.as_str().unwrap_or_default().to_string()
    } else if v.is_null() {
        String::new()
    } else {
        serde_json::to_string(v).unwrap_or_default()
    }
}

/// 解析单个输入 content part。
///
/// Responses 输入 part 类型：`input_text` / `input_image` / `input_file`。
fn parse_input_content_part(p: &Value, ctx: &mut ConvCtx) -> Option<IrPart> {
    if !p.is_object() {
        ctx.degrade("input.content.part", "content part 非对象");
        return None;
    }
    match p["type"].as_str().unwrap_or_default() {
        "input_text" => Some(IrPart::Text {
            text: p["text"].as_str().unwrap_or_default().to_string(),
        }),
        "input_image" => {
            // detail（low/high/auto）无 IR 槽位，记降级（review P5：此前静默丢失）
            if p.get("detail").is_some_and(|d| !d.is_null()) {
                ctx.degrade("input_image.detail", "image detail 不支持，丢弃");
            }
            let url_val = p.get("image_url").cloned().unwrap_or_default();
            let url = if let Some(s) = url_val.as_str() {
                s.to_string()
            } else if let Some(o) = url_val.as_object() {
                o.get("url")
                    .and_then(|u| u.as_str())
                    .unwrap_or_default()
                    .to_string()
            } else {
                String::new()
            };
            if url.is_empty() {
                // file_id 引用已上传文件的形态无 IR 槽位（review P5：此前笼统报缺 url）
                if p.get("file_id").and_then(|f| f.as_str()).is_some() {
                    ctx.degrade("input_image.file_id", "file_id 引用图片不支持，丢弃");
                } else {
                    ctx.degrade("input.image_url", "input_image 缺 url");
                }
                return None;
            }
            if let Some(rest) = url.strip_prefix("data:") {
                let (media_type, data) = split_data_url(rest);
                Some(IrPart::ImageInline { media_type, data })
            } else {
                Some(IrPart::ImageUrl { url })
            }
        }
        "input_file" => {
            let name = p["filename"]
                .as_str()
                .or_else(|| p["file_id"].as_str())
                .unwrap_or_default()
                .to_string();
            let url = p["file_url"]
                .as_str()
                .or_else(|| p["url"].as_str())
                .map(String::from);
            let data = p["file_data"]
                .as_str()
                .or_else(|| p["data"].as_str())
                .map(String::from);
            if name.is_empty() && url.is_none() && data.is_none() {
                ctx.degrade("input.file", "input_file 无内容");
                return None;
            }
            Some(IrPart::File { name, url, data })
        }
        other => {
            ctx.degrade(
                "input.content.part.type",
                format!("未知 part 类型: {other}"),
            );
            None
        }
    }
}

/// `data:...` URL 片段拆分为 (media_type, base64 data)。
fn split_data_url(rest: &str) -> (String, String) {
    if let Some(semi) = rest.find(';') {
        let media = rest[..semi].to_string();
        let after = &rest[semi + 1..];
        if let Some(comma) = after.find(',') {
            (media, after[comma + 1..].to_string())
        } else {
            (media, after.to_string())
        }
    } else if let Some(comma) = rest.find(',') {
        (
            "application/octet-stream".into(),
            rest[comma + 1..].to_string(),
        )
    } else {
        ("application/octet-stream".into(), rest.to_string())
    }
}

/// 解析一条输入 item 为 IR 消息。
/// 返回 `None` 表示该 item 无法/无需形成消息（reasoning、未知类型）。
fn parse_input_item(item: &Value, ctx: &mut ConvCtx) -> Result<Option<IrMessage>, ConvertError> {
    if !item.is_object() {
        ctx.degrade("input.item", "输入项非对象");
        return Ok(None);
    }
    let typ = item["type"].as_str().unwrap_or_default();
    match typ {
        "message" => {
            let mut msg = IrMessage {
                role: parse_response_role(item["role"].as_str(), ctx),
                ..Default::default()
            };
            if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                let mut parts = Vec::new();
                for p in content {
                    if let Some(part) = parse_input_content_part(p, ctx) {
                        parts.push(part);
                    }
                }
                if !parts.is_empty() {
                    msg.content = Some(IrContent::Parts(parts));
                }
            }
            Ok(Some(msg))
        }
        "function_call" => {
            let id = item["call_id"]
                .as_str()
                .or_else(|| item["id"].as_str())
                .unwrap_or_default()
                .to_string();
            let name = item["name"].as_str().unwrap_or_default().to_string();
            let arguments = parse_arguments_any(item.get("arguments").unwrap_or(&Value::Null));
            let mut msg = IrMessage {
                role: IrRole::Assistant,
                ..Default::default()
            };
            msg.tool_calls.push(IrToolCall {
                id,
                name,
                arguments,
            });
            Ok(Some(msg))
        }
        "function_call_output" => {
            let call_id = item["call_id"].as_str().unwrap_or_default().to_string();
            if call_id.is_empty() {
                ctx.degrade(
                    "function_call_output.call_id",
                    "缺 call_id，工具链将无法配对",
                );
            }
            // output 规范为字符串；结构化数组（output_text 等）序列化兜底并记降级
            // （review P5：此前非字符串静默变空串）
            let output = match &item["output"] {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => {
                    ctx.degrade(
                        "function_call_output.output",
                        "output 非字符串，序列化为 JSON 携带",
                    );
                    serde_json::to_string(other).unwrap_or_default()
                }
            };
            let msg = IrMessage {
                role: IrRole::Tool,
                tool_call_id: Some(call_id),
                content: Some(IrContent::Text(output)),
                ..Default::default()
            };
            Ok(Some(msg))
        }
        "reasoning" => {
            ctx.degrade("input.reasoning", "推理 item 无法回放为消息，忽略");
            Ok(None)
        }
        other => {
            ctx.degrade("input.item.type", format!("未知输入项类型，忽略: {other}"));
            Ok(None)
        }
    }
}

/// 将一条待插入的消息合并进 messages 列表。
/// 连续多个 function_call（assistant + tool_calls）会合入同一个 assistant 消息，
/// 使 tool_calls 能成组（对应 Responses 的多个 function_call output item）。
fn push_message(messages: &mut Vec<IrMessage>, msg: IrMessage) {
    if msg.role == IrRole::Assistant && !msg.tool_calls.is_empty() {
        if let Some(last) = messages.last_mut() {
            if last.role == IrRole::Assistant {
                last.tool_calls.extend(msg.tool_calls);
                return;
            }
        }
    }
    messages.push(msg);
}

/// 解析 Responses 工具（function 类型平铺或嵌套）。
/// 非 function 类型（web_search 等）记降级丢弃。
fn parse_response_tools(v: &Value, ctx: &mut ConvCtx) -> Result<Vec<IrTool>, ConvertError> {
    let arr = v
        .as_array()
        .ok_or_else(|| ConvertError::Parse("tools 不是数组".into()))?;
    let mut out = Vec::new();
    for t in arr {
        if t["type"].as_str().unwrap_or_default() != "function" {
            ctx.degrade(
                "tools",
                format!(
                    "非 function 类型工具定义，丢弃: {}",
                    t["type"].as_str().unwrap_or_default()
                ),
            );
            continue;
        }
        // 平铺：{type,name,description,parameters}；嵌套：{type,function:{...}} 皆可容错解析
        let f = if t.get("function").is_some() {
            &t["function"]
        } else {
            t
        };
        let name = f["name"].as_str().unwrap_or_default().to_string();
        if name.is_empty() {
            ctx.degrade("tools", "function 工具缺 name");
            continue;
        }
        out.push(IrTool {
            name,
            description: f["description"].as_str().map(String::from),
            parameters: f.get("parameters").cloned().unwrap_or_else(json_object),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 请求：Responses JSON → IR
// ---------------------------------------------------------------------------

/// 请求 JSON → IR。
pub fn request_to_ir(v: &Value, ctx: &mut ConvCtx) -> Result<IrRequest, ConvertError> {
    let obj = v
        .as_object()
        .ok_or_else(|| ConvertError::Parse("请求体不是对象".into()))?;
    let mut req = IrRequest::default();
    req.model = v["model"].as_str().unwrap_or_default().to_string();
    req.stream = v["stream"].as_bool().unwrap_or(false);

    // instructions → system 消息（置于最前）
    if let Some(instr) = v.get("instructions").and_then(|i| i.as_str()) {
        if !instr.is_empty() {
            req.messages.push(IrMessage {
                role: IrRole::System,
                content: Some(IrContent::Text(instr.to_string())),
                ..Default::default()
            });
        }
    }

    // input：字符串 → 用户文本消息；数组/对象 → 逐 item 解析；连续 function_call 合入同一 assistant 消息
    if let Some(input) = v.get("input") {
        if let Some(s) = input.as_str() {
            req.messages.push(IrMessage {
                role: IrRole::User,
                content: Some(IrContent::Text(s.to_string())),
                ..Default::default()
            });
        } else if let Some(arr) = input.as_array() {
            for item in arr {
                if let Some(msg) = parse_input_item(item, ctx)? {
                    push_message(&mut req.messages, msg);
                }
            }
        } else if input.is_object() {
            if let Some(msg) = parse_input_item(input, ctx)? {
                push_message(&mut req.messages, msg);
            }
        } else {
            ctx.degrade("input", "input 既非字符串/数组/对象，忽略");
        }
    }

    // max_output_tokens → max_tokens
    if let Some(mt) = v.get("max_output_tokens") {
        if let Some(n) = mt.as_u64() {
            req.max_tokens = Some(n);
        }
    }

    req.temperature = v["temperature"].as_f64();
    req.top_p = v["top_p"].as_f64();

    // text.format → response_format；text.verbosity 为 Responses 专属 → 降级
    if let Some(text) = v.get("text").and_then(|t| t.as_object()) {
        if let Some(fmt) = text.get("format") {
            req.response_format = Some(fmt.clone());
        }
        if text.get("verbosity").is_some() {
            ctx.degrade("text.verbosity", "Responses 专属字段，无法映射");
        }
    }

    if let Some(tools) = v.get("tools") {
        req.tools = parse_response_tools(tools, ctx)?;
    }
    req.tool_choice = v.get("tool_choice").cloned();

    // 有状态/Responses 专属字段：作为入口转其他协议时记为降级能力（值仍保留到 extra）
    for f in ["previous_response_id", "store", "include", "background"] {
        if v.get(f).is_some() {
            ctx.degrade(
                f,
                "Responses 有状态/专属字段，无法映射为 IR，原样保留到 extra",
            );
        }
    }

    // 未知字段进 extra（含 metadata、previous_response_id、store、include、background 等）
    for (k, val) in obj {
        if !KNOWN_REQUEST_FIELDS.contains(&k.as_str()) {
            req.extra.insert(k.clone(), val.clone());
        }
    }

    Ok(req)
}

// ---------------------------------------------------------------------------
// 请求：IR → Responses JSON
// ---------------------------------------------------------------------------

/// 将 IR 内容归一为纯文本（用于 instructions / 工具输出拼接）。
fn content_as_text(content: Option<&IrContent>) -> String {
    match content {
        Some(IrContent::Text(s)) => s.clone(),
        Some(IrContent::Parts(ps)) => {
            let mut out = String::new();
            for p in ps {
                if let IrPart::Text { text } = p {
                    out.push_str(text);
                }
            }
            out
        }
        None => String::new(),
    }
}

/// IR part → Responses 输入 content part。
fn ir_part_to_input_part(p: &IrPart, text_type: &str, ctx: &mut ConvCtx) -> Value {
    match p {
        IrPart::Text { text } => json_build(&[("type", text_type), ("text", text)]),
        IrPart::ImageUrl { url } => json_build(&[("type", "input_image"), ("image_url", url)]),
        IrPart::ImageInline { media_type, data } => json_build(&[
            ("type", "input_image"),
            ("image_url", &format!("data:{media_type};base64,{data}")),
        ]),
        IrPart::InputAudio { .. } => {
            ctx.degrade("content.input_audio", "Responses 不支持 input_audio，丢弃");
            json_build(&[("type", text_type), ("text", "")])
        }
        IrPart::File { name, url, data } => {
            let mut o = Map::new();
            o.insert("type".into(), Value::String("input_file".into()));
            if !name.is_empty() {
                o.insert("filename".into(), Value::String(name.clone()));
            }
            if let Some(u) = url {
                o.insert("file_url".into(), Value::String(u.clone()));
            }
            if let Some(d) = data {
                o.insert("file_data".into(), Value::String(d.clone()));
            }
            Value::Object(o)
        }
    }
}

/// 便捷构造字符串键值 JSON 对象（用于 Responses part）。
fn json_build(pairs: &[(&str, &str)]) -> Value {
    let mut o = Map::new();
    for (k, v) in pairs {
        o.insert(k.to_string(), Value::String(v.to_string()));
    }
    Value::Object(o)
}

/// 构造 Responses 输入 message item（roles：user/assistant）。
/// OpenAI 2025-10 起拒绝 assistant 消息内的 input_text（实测 400：
/// "Supported values are: 'output_text' and 'refusal'"），文本 part 类型随角色分派。
fn request_message_item(role: IrRole, content: &IrContent, ctx: &mut ConvCtx) -> Value {
    let (role_str, text_type) = if role == IrRole::Assistant {
        ("assistant", "output_text")
    } else {
        ("user", "input_text")
    };
    let parts = match content {
        IrContent::Text(s) => vec![json_build(&[("type", text_type), ("text", s)])],
        IrContent::Parts(ps) => ps
            .iter()
            .map(|p| ir_part_to_input_part(p, text_type, ctx))
            .collect(),
    };
    json_build_with("message", role_str, parts)
}

/// 构造带 role + content 数组的输入 message item。
fn json_build_with(type_: &str, role: &str, parts: Vec<Value>) -> Value {
    let mut o = Map::new();
    o.insert("type".into(), Value::String(type_.into()));
    o.insert("role".into(), Value::String(role.into()));
    o.insert("content".into(), Value::Array(parts));
    Value::Object(o)
}

/// 构造 Responses 输入 function_call item。
fn function_call_input_item(tc: &IrToolCall) -> Value {
    let mut o = Map::new();
    o.insert("type".into(), Value::String("function_call".into()));
    o.insert("call_id".into(), Value::String(tc.id.clone()));
    o.insert("name".into(), Value::String(tc.name.clone()));
    o.insert("arguments".into(), Value::String(tc.arguments.clone()));
    Value::Object(o)
}

/// IR 工具定义 → Responses 工具（function 平铺）。
fn response_tool_from_ir(t: &IrTool) -> Value {
    let mut o = Map::new();
    o.insert("type".into(), Value::String("function".into()));
    o.insert("name".into(), Value::String(t.name.clone()));
    if let Some(d) = &t.description {
        o.insert("description".into(), Value::String(d.clone()));
    }
    o.insert("parameters".into(), t.parameters.clone());
    Value::Object(o)
}

/// 记录 ext 字段在 Responses 请求无对应输入槽 → 降级。
fn degrade_ext(req: &IrRequest, ctx: &mut ConvCtx) {
    if req.ext.thinking.is_some() {
        ctx.degrade("ext.thinking", "Responses 请求无对应输入字段");
    }
    if req.ext.web_search.is_some() {
        ctx.degrade("ext.web_search", "Responses 请求无对应输入字段");
    }
    if req.ext.cache_control.is_some() {
        ctx.degrade("ext.cache_control", "Responses 请求无对应输入字段");
    }
    if req.ext.top_k.is_some() {
        ctx.degrade("ext.top_k", "Responses 请求无对应输入字段");
    }
    if req.ext.service_tier.is_some() {
        ctx.degrade("ext.service_tier", "Responses 请求无对应输入字段");
    }
    for k in req.ext.extra.keys() {
        ctx.degrade(k.clone(), "Responses 请求无对应输入字段");
    }
}

/// IR → 请求 JSON。extra 先铺底，再写规范字段（规范字段覆盖 extra 同名字段）。
pub fn request_from_ir(req: &IrRequest, ctx: &mut ConvCtx) -> Result<Value, ConvertError> {
    let mut body = req.extra.clone();
    degrade_ext(req, ctx);

    body.insert("model".into(), Value::String(req.model.clone()));

    // system 消息 → instructions；其余消息 → input items
    let mut instructions = String::new();
    let mut input_items: Vec<Value> = Vec::new();
    for m in &req.messages {
        match m.role {
            IrRole::System => {
                let text = content_as_text(m.content.as_ref());
                if !text.is_empty() {
                    if !instructions.is_empty() {
                        instructions.push('\n');
                    }
                    instructions.push_str(&text);
                }
            }
            IrRole::User | IrRole::Assistant => {
                if let Some(content) = m.content.as_ref() {
                    input_items.push(request_message_item(m.role, content, ctx));
                }
                for tc in &m.tool_calls {
                    input_items.push(function_call_input_item(tc));
                }
            }
            IrRole::Tool => {
                let mut o = Map::new();
                o.insert("type".into(), Value::String("function_call_output".into()));
                o.insert(
                    "call_id".into(),
                    Value::String(m.tool_call_id.clone().unwrap_or_default()),
                );
                o.insert(
                    "output".into(),
                    Value::String(content_as_text(m.content.as_ref())),
                );
                input_items.push(Value::Object(o));
            }
        }
    }
    if !instructions.is_empty() {
        body.insert("instructions".into(), Value::String(instructions));
    }
    body.insert("input".into(), Value::Array(input_items));

    if !req.tools.is_empty() {
        body.insert(
            "tools".into(),
            Value::Array(req.tools.iter().map(response_tool_from_ir).collect()),
        );
    }
    if let Some(tc) = &req.tool_choice {
        // 跨协议形状归一（review P5）：Chat 的 {"type":"function","function":{"name":"x"}}
        // 转 Responses 需摊平为 {"type":"function","name":"x"}
        let tc = match tc {
            Value::Object(o)
                if o.get("type").and_then(|t| t.as_str()) == Some("function")
                    && o.get("name").is_none()
                    && o.get("function").and_then(|f| f.get("name")).is_some() =>
            {
                serde_json::json!({
                    "type": "function",
                    "name": o["function"]["name"].clone(),
                })
            }
            other => other.clone(),
        };
        body.insert("tool_choice".into(), tc);
    }
    if let Some(t) = req.temperature {
        body.insert("temperature".into(), json_number_f64(t));
    }
    if let Some(tp) = req.top_p {
        body.insert("top_p".into(), json_number_f64(tp));
    }
    if let Some(mt) = req.max_tokens {
        body.insert("max_output_tokens".into(), Value::from(mt));
    }
    if req.seed.is_some() {
        // Responses API 不支持 seed（review P5：此前直接写入请求体，与本文件头注释矛盾）
        ctx.degrade("seed", "Responses 不支持 seed，忽略");
    }
    if req.stop.is_some() {
        // Responses API 无 stop 参数（review P5：此前静默丢弃）
        ctx.degrade("stop", "Responses 不支持 stop，忽略");
    }
    if let Some(u) = &req.user {
        body.insert("user".into(), Value::String(u.clone()));
    }
    if req.n.is_some() {
        ctx.degrade("n", "Responses 不支持 n，忽略");
    }
    // response_format → text.format
    if let Some(rf) = &req.response_format {
        let mut text = Map::new();
        text.insert("format".into(), rf.clone());
        body.insert("text".into(), Value::Object(text));
    }

    body.insert("stream".into(), Value::Bool(req.stream));

    Ok(Value::Object(body))
}

// ---------------------------------------------------------------------------
// 非流式响应：Responses JSON → IR
// ---------------------------------------------------------------------------

/// 解析 Responses usage（input/output_tokens + details）。
fn parse_responses_usage(u: &Value) -> IrUsage {
    let mut usage = IrUsage::default();
    usage.prompt_tokens = u["input_tokens"].as_u64().unwrap_or(0);
    usage.completion_tokens = u["output_tokens"].as_u64().unwrap_or(0);
    usage.total_tokens = u["total_tokens"].as_u64();

    if let Some(details) = u.get("input_tokens_details").and_then(|d| d.as_object()) {
        if let Some(cached) = details.get("cached_tokens").and_then(|c| c.as_u64()) {
            usage.cache_read_tokens = Some(cached);
        }
        let mut rest = Map::new();
        for (k, val) in details {
            if k != "cached_tokens" {
                rest.insert(k.clone(), val.clone());
            }
        }
        if !rest.is_empty() {
            usage
                .extra
                .insert("input_tokens_details".into(), Value::Object(rest));
        }
    }
    if let Some(details) = u.get("output_tokens_details").and_then(|d| d.as_object()) {
        if let Some(rt) = details.get("reasoning_tokens").and_then(|r| r.as_u64()) {
            usage
                .extra
                .insert("reasoning_tokens".into(), Value::from(rt));
        }
        let mut rest = Map::new();
        for (k, val) in details {
            if k != "reasoning_tokens" {
                rest.insert(k.clone(), val.clone());
            }
        }
        if !rest.is_empty() {
            usage
                .extra
                .insert("output_tokens_details".into(), Value::Object(rest));
        }
    }
    if let Some(o) = u.as_object() {
        for (k, val) in o {
            if !matches!(
                k.as_str(),
                "input_tokens"
                    | "output_tokens"
                    | "total_tokens"
                    | "input_tokens_details"
                    | "output_tokens_details"
            ) {
                usage.extra.insert(k.clone(), val.clone());
            }
        }
    }
    usage
}

/// 将文本追加到 IR 消息内容（多个 output_text part 拼接）。
fn append_message_text(msg: &mut IrMessage, text: &str) {
    if text.is_empty() {
        return;
    }
    match msg.content.as_mut() {
        Some(IrContent::Text(s)) => s.push_str(text),
        Some(IrContent::Parts(ps)) => match ps.last_mut() {
            Some(IrPart::Text { text: t }) => t.push_str(text),
            _ => ps.push(IrPart::Text {
                text: text.to_string(),
            }),
        },
        None => msg.content = Some(IrContent::Text(text.to_string())),
    }
}

/// 非流式响应 JSON → IR。
pub fn response_to_ir(v: &Value, ctx: &mut ConvCtx) -> Result<IrResponse, ConvertError> {
    let obj = v
        .as_object()
        .ok_or_else(|| ConvertError::Parse("响应体不是对象".into()))?;
    let mut resp = IrResponse::default();
    resp.id = v["id"].as_str().unwrap_or_default().to_string();
    resp.model = v["model"].as_str().unwrap_or_default().to_string();
    resp.created = v["created_at"].as_i64().unwrap_or(0);

    let mut msg = IrMessage {
        role: IrRole::Assistant,
        ..Default::default()
    };

    // output items → assistant 消息 + tool_calls + reasoning_content
    if let Some(output) = v.get("output").and_then(|o| o.as_array()) {
        for item in output {
            match item["type"].as_str().unwrap_or_default() {
                "message" => {
                    // output_text parts 拼接
                    if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                        let mut text = String::new();
                        for part in content {
                            if let Some(t) = part["text"].as_str() {
                                text.push_str(t);
                            } else {
                                ctx.degrade(
                                    "output.message.content.part",
                                    "未知 message content part，忽略",
                                );
                            }
                        }
                        append_message_text(&mut msg, &text);
                    }
                }
                "function_call" => {
                    let id = item["call_id"]
                        .as_str()
                        .or_else(|| item["id"].as_str())
                        .unwrap_or_default()
                        .to_string();
                    let name = item["name"].as_str().unwrap_or_default().to_string();
                    let arguments =
                        parse_arguments_any(item.get("arguments").unwrap_or(&Value::Null));
                    msg.tool_calls.push(IrToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
                "reasoning" => {
                    // summary 文本拼接
                    if let Some(summary) = item.get("summary").and_then(|s| s.as_array()) {
                        let mut texts = String::new();
                        for s in summary {
                            if let Some(t) = s["text"].as_str() {
                                texts.push_str(t);
                            }
                        }
                        if !texts.is_empty() {
                            if let Some(rc) = msg.reasoning_content.as_mut() {
                                rc.push_str(&texts);
                            } else {
                                msg.reasoning_content = Some(texts);
                            }
                        }
                    }
                }
                other => {
                    ctx.degrade("output.item.type", format!("未知输出项类型: {other}"));
                }
            }
        }
    }

    // status → finish_reason；incomplete_details 进 extra
    let status = v["status"].as_str();
    let finish_reason = status.map(normalize_finish_reason);
    resp.choices.push(IrChoice {
        index: 0,
        message: msg,
        finish_reason,
    });
    if status == Some("incomplete") {
        if let Some(details) = v.get("incomplete_details") {
            resp.extra
                .insert("incomplete_details".into(), details.clone());
        }
    }

    if let Some(u) = v.get("usage") {
        resp.usage = Some(parse_responses_usage(u));
    }

    // 未知响应字段进 extra
    for (k, val) in obj {
        if !KNOWN_RESPONSE_FIELDS.contains(&k.as_str()) {
            resp.extra.insert(k.clone(), val.clone());
        }
    }

    Ok(resp)
}

// ---------------------------------------------------------------------------
// 非流式响应：IR → Responses JSON
// ---------------------------------------------------------------------------

/// IR usage → Responses usage（input/output_tokens + details）。
fn usage_from_ir(u: &IrUsage, ctx: &mut ConvCtx) -> Value {
    let mut out = Map::new();
    out.insert("input_tokens".into(), Value::from(u.prompt_tokens));
    out.insert("output_tokens".into(), Value::from(u.completion_tokens));
    if let Some(t) = u.total_tokens {
        out.insert("total_tokens".into(), Value::from(t));
    }

    let mut input_details = Map::new();
    if let Some(cached) = u.cache_read_tokens {
        input_details.insert("cached_tokens".into(), Value::from(cached));
    }
    if let Some(d) = u
        .extra
        .get("input_tokens_details")
        .and_then(|v| v.as_object())
    {
        for (k, val) in d {
            input_details
                .entry(k.clone())
                .or_insert_with(|| val.clone());
        }
    }
    if !input_details.is_empty() {
        out.insert("input_tokens_details".into(), Value::Object(input_details));
    }

    let mut output_details = Map::new();
    if let Some(rt) = u.extra.get("reasoning_tokens") {
        output_details.insert("reasoning_tokens".into(), rt.clone());
    }
    // Chat 形状 completion_tokens_details 归一消费：子字段并入 output_details
    // （reasoning_tokens 由上方标准槽位优先），该键不再原样透传进 Responses usage
    // （review P5：跨协议 usage 键形状归一）。
    if let Some(d) = u
        .extra
        .get("completion_tokens_details")
        .and_then(|v| v.as_object())
    {
        for (k, val) in d {
            output_details
                .entry(k.clone())
                .or_insert_with(|| val.clone());
        }
    }
    if let Some(d) = u
        .extra
        .get("output_tokens_details")
        .and_then(|v| v.as_object())
    {
        for (k, val) in d {
            output_details
                .entry(k.clone())
                .or_insert_with(|| val.clone());
        }
    }
    if !output_details.is_empty() {
        out.insert(
            "output_tokens_details".into(),
            Value::Object(output_details),
        );
    }

    for (k, val) in &u.extra {
        if k != "reasoning_tokens"
            && k != "output_tokens_details"
            && k != "input_tokens_details"
            && k != "completion_tokens_details"
        {
            out.insert(k.clone(), val.clone());
        }
    }

    if u.cache_write_tokens.is_some() {
        ctx.degrade(
            "usage.cache_write_tokens",
            "Responses 无 cache_write_tokens 映射",
        );
    }
    Value::Object(out)
}

/// 构造 Responses 输出 message item。
fn output_message_item(role: IrRole, text: &str, id: &str) -> Value {
    let mut content = Vec::new();
    if !text.is_empty() {
        let mut part = Map::new();
        part.insert("type".into(), Value::String("output_text".into()));
        part.insert("text".into(), Value::String(text.to_string()));
        content.push(Value::Object(part));
    }
    json_build_item_message(role, content, id)
}

/// 构造输出 message item（type=message, status=completed）。
fn json_build_item_message(role: IrRole, content: Vec<Value>, id: &str) -> Value {
    let mut o = Map::new();
    o.insert("id".into(), Value::String(id.to_string()));
    o.insert("type".into(), Value::String("message".into()));
    o.insert("status".into(), Value::String("completed".into()));
    o.insert(
        "role".into(),
        Value::String(role_to_response_str(role).into()),
    );
    o.insert("content".into(), Value::Array(content));
    Value::Object(o)
}

/// 构造输出 function_call item。
fn output_function_call_item(tc: &IrToolCall) -> Value {
    let mut o = Map::new();
    o.insert("id".into(), Value::String(format!("fc_{}", rand_suffix())));
    o.insert("type".into(), Value::String("function_call".into()));
    o.insert("status".into(), Value::String("completed".into()));
    o.insert("call_id".into(), Value::String(tc.id.clone()));
    o.insert("name".into(), Value::String(tc.name.clone()));
    o.insert("arguments".into(), Value::String(tc.arguments.clone()));
    Value::Object(o)
}

/// 构造输出 reasoning item。
fn output_reasoning_item(text: &str, id: &str) -> Value {
    let mut summary = Vec::new();
    if !text.is_empty() {
        let mut part = Map::new();
        part.insert("type".into(), Value::String("summary_text".into()));
        part.insert("text".into(), Value::String(text.to_string()));
        summary.push(Value::Object(part));
    }
    let mut o = Map::new();
    o.insert("id".into(), Value::String(id.to_string()));
    o.insert("type".into(), Value::String("reasoning".into()));
    o.insert("status".into(), Value::String("completed".into()));
    o.insert("summary".into(), Value::Array(summary));
    Value::Object(o)
}

/// 生成短随机后缀（纳秒 + 原子计数器，保证同进程内单调不碰撞）。
fn rand_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let s = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{n:x}{s:x}")
}

/// IR → 非流式响应 JSON。
pub fn response_from_ir(resp: &IrResponse, ctx: &mut ConvCtx) -> Result<Value, ConvertError> {
    let mut body = resp.extra.clone();
    body.insert("id".into(), Value::String(resp.id.clone()));
    body.insert("object".into(), Value::String("response".into()));
    body.insert("created_at".into(), Value::from(resp.created));
    body.insert("model".into(), Value::String(resp.model.clone()));

    // status 由 finish_reason 推导（review P5：此前恒 completed，length/failed 语义被反转）；
    // extra 里上游原有的 incomplete_details 优先保留。
    let finish = resp
        .choices
        .first()
        .and_then(|c| c.finish_reason.as_deref());
    let has_incomplete = resp.extra.contains_key("incomplete_details");
    let status = match finish {
        Some("length") => "incomplete",
        Some("failed") => "failed",
        _ if has_incomplete => "incomplete",
        _ => "completed",
    };
    body.insert("status".into(), Value::String(status.into()));
    if let Some(details) = resp.extra.get("incomplete_details") {
        body.insert("incomplete_details".into(), details.clone());
    } else if status == "incomplete" {
        body.insert(
            "incomplete_details".into(),
            serde_json::json!({ "reason": "max_output_tokens" }),
        );
    }
    if status == "failed" {
        body.insert(
            "error".into(),
            serde_json::json!({ "code": "server_error", "message": "上游响应失败" }),
        );
    }

    let mut output = Vec::new();
    if resp.choices.len() > 1 {
        // 多 choice 平铺进同一 output 数组（Responses 单响应语义），记降级
        ctx.degrade(
            "choices",
            format!(
                "多候选（{}）平铺为单个 response 的 output",
                resp.choices.len()
            ),
        );
    }
    for ch in &resp.choices {
        let msg = &ch.message;
        if let Some(content) = &msg.content {
            output.push(output_message_item(
                msg.role,
                &content_as_text(Some(content)),
                &format!("msg_{}", rand_suffix()),
            ));
        }
        for tc in &msg.tool_calls {
            output.push(output_function_call_item(tc));
        }
        if let Some(rc) = &msg.reasoning_content {
            output.push(output_reasoning_item(rc, &format!("rs_{}", rand_suffix())));
        }
    }
    body.insert("output".into(), Value::Array(output));

    if let Some(u) = &resp.usage {
        body.insert("usage".into(), usage_from_ir(u, ctx));
    }

    Ok(Value::Object(body))
}

// ---------------------------------------------------------------------------
// 流式 chunk
// ---------------------------------------------------------------------------

/// Responses 流式状态（双向复用）：
/// - 入站（Responses 流 → IR chunk）：id/model、function_call item_id → 工具序号映射。
/// - 出站（IR chunk → Responses 事件）：created/completed 标记、输出 item 编排、
///   message/reasoning/function_call 的累积文本与参数。
#[derive(Debug, Default)]
pub struct StreamState {
    /// 响应骨架（来自 response.created / 首个 IR chunk）
    pub id: String,
    pub model: String,
    pub created_at: i64,

    // —— 入站 (chunk_to_ir) ——
    /// function_call item_id → IR 内工具序号（index，供聚合器对齐）
    item_to_index: HashMap<String, u32>,
    /// IR 工具序号 → (call_id, name)（function_call item 元数据，跨协议工具链必需）
    fn_meta: HashMap<u32, (String, String)>,
    /// 已分配的工具序号计数
    fn_index_counter: u32,

    // —— 出站 (chunk_from_ir) ——
    /// 是否已发出 response.created
    created: bool,
    /// 是否已发出 response.completed
    completed: bool,
    /// 下一个输出 item 的 output_index
    next_output_index: u32,
    /// message 输出 item 状态
    message_item_added: bool,
    message_item_id: String,
    message_output_index: u32,
    message_text: String,
    /// reasoning 输出 item 状态
    reasoning_item_added: bool,
    reasoning_item_id: String,
    reasoning_output_index: u32,
    reasoning_text: String,
    /// function_call 输出 item：IR 工具 index → 状态
    tool_items: HashMap<u32, FnItemState>,
    /// 已见的最新 usage（OpenAI include_usage 下真实 usage 常在 finish 后的独立尾帧）
    last_usage: Option<crate::protocol::ir::IrUsage>,
    /// finish 已收但尚无 usage：延迟终止帧至 usage 尾帧或 stream_end（发布审阅 H1）
    pending_finish: Option<String>,
}

/// function_call 输出 item 的累积状态（出站用）。
#[derive(Debug, Default, Clone)]
struct FnItemState {
    output_index: u32,
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
}

/// SSE data 载荷 → IR chunk；无业务内容（created/output_item.added/空 delta/完成帧 等）返回 Ok(None)。
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
        "response.created" => {
            let resp = &v["response"];
            st.id = resp["id"].as_str().unwrap_or_default().to_string();
            st.model = resp["model"].as_str().unwrap_or_default().to_string();
            st.created_at = resp["created_at"].as_i64().unwrap_or(0);
            Ok(None)
        }
        "response.output_item.added" => {
            let item = &v["item"];
            if item["type"].as_str() == Some("function_call") {
                // 记录 item_id → index（按出现顺序分配），并保存 call_id/name 元数据：
                // 后续 function_call_arguments.delta 只带 item_id 与参数增量，
                // 若无此处记录，聚合出的 IrToolCall 将丢失 name/id（review P1-#9）。
                let item_id = item["id"].as_str().unwrap_or_default().to_string();
                let idx = st.fn_index_counter;
                st.fn_index_counter += 1;
                st.item_to_index.insert(item_id.clone(), idx);
                let call_id = item["call_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .or_else(|| item["id"].as_str())
                    .unwrap_or_default()
                    .to_string();
                let name = item["name"].as_str().unwrap_or_default().to_string();
                st.fn_meta.insert(idx, (call_id, name));
            }
            Ok(None)
        }
        "response.output_item.done" => {
            // function_call 完成帧：若 added 帧未带 name（部分实现延迟到 done），补记元数据
            let item = &v["item"];
            if item["type"].as_str() == Some("function_call") {
                let item_id = item["id"].as_str().unwrap_or_default();
                if let Some(&idx) = st.item_to_index.get(item_id) {
                    let entry = st.fn_meta.entry(idx).or_default();
                    if entry.0.is_empty() {
                        entry.0 = item["call_id"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .or_else(|| item["id"].as_str())
                            .unwrap_or_default()
                            .to_string();
                    }
                    if entry.1.is_empty() {
                        entry.1 = item["name"].as_str().unwrap_or_default().to_string();
                    }
                }
            }
            Ok(None)
        }
        "response.output_text.delta" => {
            let text = v["delta"].as_str().unwrap_or_default();
            if text.is_empty() {
                return Ok(None);
            }
            let mut chunk = base_chunk(st);
            chunk.choices.push(IrChunkChoice {
                index: 0,
                delta: IrDelta {
                    content: Some(text.to_string()),
                    ..Default::default()
                },
                finish_reason: None,
            });
            Ok(Some(chunk))
        }
        "response.reasoning_summary_text.delta" => {
            let text = v["delta"].as_str().unwrap_or_default();
            if text.is_empty() {
                return Ok(None);
            }
            let mut chunk = base_chunk(st);
            chunk.choices.push(IrChunkChoice {
                index: 0,
                delta: IrDelta {
                    reasoning_content: Some(text.to_string()),
                    ..Default::default()
                },
                finish_reason: None,
            });
            Ok(Some(chunk))
        }
        "response.function_call_arguments.delta" => {
            let item_id = v["item_id"].as_str().unwrap_or_default().to_string();
            let text = v["delta"].as_str().unwrap_or_default();
            if text.is_empty() {
                return Ok(None);
            }
            // 未知 item_id（added 帧未到/不匹配）：此前静默并入工具 0 且无日志（review P5），
            // 现记 degrade 并丢弃该增量，避免参数串拼错工具。
            let Some(&index) = st.item_to_index.get(&item_id) else {
                ctx.degrade(
                    "stream.function_call_arguments.delta",
                    format!("未知 item_id，丢弃参数增量: {item_id}"),
                );
                return Ok(None);
            };
            let (call_id, name) = st.fn_meta.get(&index).cloned().unwrap_or_default();
            let mut chunk = base_chunk(st);
            chunk.choices.push(IrChunkChoice {
                index: 0,
                delta: IrDelta {
                    tool_calls: vec![IrToolCallDelta {
                        index,
                        id: if call_id.is_empty() {
                            None
                        } else {
                            Some(call_id)
                        },
                        name: if name.is_empty() { None } else { Some(name) },
                        arguments: Some(text.to_string()),
                    }],
                    ..Default::default()
                },
                finish_reason: None,
            });
            Ok(Some(chunk))
        }
        "response.completed" | "response.incomplete" => {
            let resp = &v["response"];
            let finish = if etype == "response.completed" {
                "completed"
            } else {
                "incomplete"
            };
            let mut chunk = base_chunk(st);
            chunk.choices.push(IrChunkChoice {
                index: 0,
                delta: IrDelta::default(),
                finish_reason: Some(normalize_finish_reason(finish)),
            });
            if let Some(u) = resp.get("usage") {
                chunk.usage = Some(parse_responses_usage(u));
            }
            Ok(Some(chunk))
        }
        // 上游流式失败（review P5）：此前落入 other 分支记 degrade 后丢弃，下游还会
        // 补发伪造的 completed，把失败伪装成成功。现显式映射为 finish=failed 的终止 chunk，
        // 由出站侧发 response.failed / 对应协议的失败终止。
        "response.failed" => {
            let resp = &v["response"];
            let msg = resp
                .pointer("/error/message")
                .and_then(|m| m.as_str())
                .unwrap_or("未知错误");
            ctx.degrade("stream.response.failed", format!("上游响应失败: {msg}"));
            let mut chunk = base_chunk(st);
            chunk.choices.push(IrChunkChoice {
                index: 0,
                delta: IrDelta::default(),
                finish_reason: Some("failed".into()),
            });
            if let Some(u) = resp.get("usage") {
                chunk.usage = Some(parse_responses_usage(u));
            }
            Ok(Some(chunk))
        }
        // 流内独立 error 事件（部分实现在流中途发 error 后继续/终止）
        "error" => {
            let msg = v["message"].as_str().unwrap_or("未知错误");
            ctx.degrade("stream.error", format!("上游流错误事件: {msg}"));
            let mut chunk = base_chunk(st);
            chunk.choices.push(IrChunkChoice {
                index: 0,
                delta: IrDelta::default(),
                finish_reason: Some("failed".into()),
            });
            Ok(Some(chunk))
        }
        // 良性生命周期事件：正常流每轮必现，显式无操作跳过（此前全落 other 分支
        // 产生大量 degrade 噪音，淹没真实降级，review P5）。
        "response.in_progress"
        | "response.queued"
        | "response.output_text.done"
        | "response.content_part.added"
        | "response.content_part.done"
        | "response.function_call_arguments.done"
        | "response.reasoning_summary_part.added"
        | "response.reasoning_summary_part.done"
        | "response.reasoning_summary_text.done"
        | "response.output_text.annotation.added"
        | "response.refusal.delta"
        | "response.refusal.done" => Ok(None),
        other => {
            ctx.degrade("stream.event.type", format!("未知事件，忽略: {other}"));
            Ok(None)
        }
    }
}

/// 基于 st 骨架构建空 chunk。
fn base_chunk(st: &StreamState) -> IrChunk {
    IrChunk {
        id: st.id.clone(),
        model: st.model.clone(),
        created: st.created_at,
        ..Default::default()
    }
}

/// 构造 response.created 事件。
fn encode_response_created(st: &StreamState) -> Result<String, ConvertError> {
    let mut resp = Map::new();
    resp.insert("id".into(), Value::String(st.id.clone()));
    resp.insert("object".into(), Value::String("response".into()));
    resp.insert("created_at".into(), Value::from(st.created_at));
    resp.insert("status".into(), Value::String("in_progress".into()));
    resp.insert("model".into(), Value::String(st.model.clone()));
    resp.insert("output".into(), Value::Array(vec![]));
    resp.insert("usage".into(), Value::Null);
    json_to_string(&json_build_event("response.created", Value::Object(resp)))
}

/// 构造 events 时使用的通用包装（type + response/…）。
fn json_build_event(type_: &str, payload: Value) -> Value {
    let mut o = Map::new();
    o.insert("type".into(), Value::String(type_.into()));
    o.insert("response".into(), payload);
    Value::Object(o)
}

/// 构造 message output_item.added 事件。
fn encode_output_item_added_message(st: &StreamState) -> Result<String, ConvertError> {
    // item.id 必须与 delta 的 item_id 及 completed 帧内 item.id 一致（review P5）：
    // 统一使用 st.message_item_id（分配时已带 msg_ 前缀）。
    let mut item = Map::new();
    item.insert("id".into(), Value::String(st.message_item_id.clone()));
    item.insert("type".into(), Value::String("message".into()));
    item.insert("status".into(), Value::String("in_progress".into()));
    item.insert("role".into(), Value::String("assistant".into()));
    item.insert("content".into(), Value::Array(vec![]));
    let mut o = Map::new();
    o.insert(
        "type".into(),
        Value::String("response.output_item.added".into()),
    );
    o.insert("output_index".into(), Value::from(st.message_output_index));
    o.insert("item".into(), Value::Object(item));
    json_to_string(&Value::Object(o))
}

/// 构造 output_text.delta 事件。
fn encode_output_text_delta(st: &StreamState, text: &str) -> Result<String, ConvertError> {
    let mut o = Map::new();
    o.insert(
        "type".into(),
        Value::String("response.output_text.delta".into()),
    );
    o.insert("item_id".into(), Value::String(st.message_item_id.clone()));
    o.insert("output_index".into(), Value::from(st.message_output_index));
    o.insert("content_index".into(), Value::from(0));
    o.insert("delta".into(), Value::String(text.to_string()));
    json_to_string(&Value::Object(o))
}

/// 构造 reasoning output_item.added 事件。
fn encode_output_item_added_reasoning(st: &StreamState) -> Result<String, ConvertError> {
    let mut summary = Vec::new();
    let mut part = Map::new();
    part.insert("type".into(), Value::String("summary_text".into()));
    part.insert("text".into(), Value::String("".into()));
    summary.push(Value::Object(part));
    let mut item = Map::new();
    item.insert("id".into(), Value::String(st.reasoning_item_id.clone()));
    item.insert("type".into(), Value::String("reasoning".into()));
    item.insert("status".into(), Value::String("in_progress".into()));
    item.insert("summary".into(), Value::Array(summary));
    let mut o = Map::new();
    o.insert(
        "type".into(),
        Value::String("response.output_item.added".into()),
    );
    o.insert(
        "output_index".into(),
        Value::from(st.reasoning_output_index),
    );
    o.insert("item".into(), Value::Object(item));
    json_to_string(&Value::Object(o))
}

/// 构造 reasoning_summary_text.delta 事件。
fn encode_reasoning_summary_text_delta(
    st: &StreamState,
    text: &str,
) -> Result<String, ConvertError> {
    let mut o = Map::new();
    o.insert(
        "type".into(),
        Value::String("response.reasoning_summary_text.delta".into()),
    );
    o.insert(
        "item_id".into(),
        Value::String(st.reasoning_item_id.clone()),
    );
    o.insert(
        "output_index".into(),
        Value::from(st.reasoning_output_index),
    );
    o.insert("summary_index".into(), Value::from(0));
    o.insert("delta".into(), Value::String(text.to_string()));
    json_to_string(&Value::Object(o))
}

/// 构造 function_call output_item.added 事件。
fn encode_output_item_added_function_call(
    st: &StreamState,
    idx: u32,
) -> Result<String, ConvertError> {
    let state = st.tool_items.get(&idx).cloned().unwrap_or_default();
    let mut item = Map::new();
    item.insert("id".into(), Value::String(state.item_id));
    item.insert("type".into(), Value::String("function_call".into()));
    item.insert("status".into(), Value::String("in_progress".into()));
    item.insert("call_id".into(), Value::String(state.call_id));
    item.insert("name".into(), Value::String(state.name));
    item.insert("arguments".into(), Value::String(state.arguments));
    let mut o = Map::new();
    o.insert(
        "type".into(),
        Value::String("response.output_item.added".into()),
    );
    o.insert("output_index".into(), Value::from(state.output_index));
    o.insert("item".into(), Value::Object(item));
    json_to_string(&Value::Object(o))
}

/// 构造 function_call_arguments.delta 事件。
fn encode_function_call_arguments_delta(
    st: &StreamState,
    idx: u32,
    text: &str,
) -> Result<String, ConvertError> {
    let state = st.tool_items.get(&idx).cloned().unwrap_or_default();
    let mut o = Map::new();
    o.insert(
        "type".into(),
        Value::String("response.function_call_arguments.delta".into()),
    );
    o.insert("item_id".into(), Value::String(state.item_id));
    o.insert("output_index".into(), Value::from(state.output_index));
    o.insert("delta".into(), Value::String(text.to_string()));
    json_to_string(&Value::Object(o))
}

/// 基于 st 累积状态构建终止帧（completed/incomplete/failed）的 response 对象。
/// item id 复用 added/delta 阶段的 st 内 id（review P5：此前 completed 帧重新随机生成，
/// 导致同一 item 在 added/delta/completed 三类事件中 id 互不一致）。
fn build_final_response(
    st: &StreamState,
    usage: Option<&IrUsage>,
    ctx: &mut ConvCtx,
    status: &str,
) -> Value {
    let mut output = Vec::new();
    if st.message_item_added {
        output.push(output_message_item(
            IrRole::Assistant,
            &st.message_text,
            &st.message_item_id,
        ));
    }
    if st.reasoning_item_added {
        output.push(output_reasoning_item(
            &st.reasoning_text,
            &st.reasoning_item_id,
        ));
    }
    let mut tools: Vec<&FnItemState> = st.tool_items.values().collect();
    tools.sort_by_key(|t| t.output_index);
    for t in tools {
        output.push(output_function_call_item_from_state(t));
    }

    let usage_val = usage.map(|u| usage_from_ir(u, ctx)).unwrap_or(Value::Null);
    let mut resp = Map::new();
    resp.insert("id".into(), Value::String(st.id.clone()));
    resp.insert("object".into(), Value::String("response".into()));
    resp.insert("created_at".into(), Value::from(st.created_at));
    resp.insert("status".into(), Value::String(status.into()));
    if status == "incomplete" {
        resp.insert(
            "incomplete_details".into(),
            serde_json::json!({ "reason": "max_output_tokens" }),
        );
    }
    if status == "failed" {
        resp.insert(
            "error".into(),
            serde_json::json!({ "code": "server_error", "message": "上游响应失败" }),
        );
    }
    resp.insert("model".into(), Value::String(st.model.clone()));
    resp.insert("output".into(), Value::Array(output));
    resp.insert("usage".into(), usage_val);
    Value::Object(resp)
}

/// 由累积状态构造 function_call 输出 item。
fn output_function_call_item_from_state(t: &FnItemState) -> Value {
    let mut o = Map::new();
    o.insert("id".into(), Value::String(t.item_id.clone()));
    o.insert("type".into(), Value::String("function_call".into()));
    o.insert("status".into(), Value::String("completed".into()));
    o.insert("call_id".into(), Value::String(t.call_id.clone()));
    o.insert("name".into(), Value::String(t.name.clone()));
    o.insert("arguments".into(), Value::String(t.arguments.clone()));
    Value::Object(o)
}

/// 构造 response.completed 事件。
fn encode_response_completed(
    st: &StreamState,
    usage: Option<&crate::protocol::ir::IrUsage>,
    ctx: &mut ConvCtx,
) -> Result<String, ConvertError> {
    let resp = build_final_response(st, usage, ctx, "completed");
    json_to_string(&Value::Object({
        let mut o = Map::new();
        o.insert("type".into(), Value::String("response.completed".into()));
        o.insert("response".into(), resp);
        o
    }))
}

/// 构造终止帧事件：按 finish_reason 选择 completed / incomplete / failed。
/// failed/incomplete 语义来自上游（review P5：此前无论成败恒发 completed，
/// 上游流式失败被伪装成成功响应）。
fn encode_response_finished(
    st: &StreamState,
    finish: Option<&str>,
    usage: Option<&crate::protocol::ir::IrUsage>,
    ctx: &mut ConvCtx,
) -> Result<String, ConvertError> {
    let (status, etype) = match finish {
        Some("failed") => ("failed", "response.failed"),
        Some("length") => ("incomplete", "response.incomplete"),
        _ => return encode_response_completed(st, usage, ctx),
    };
    let resp = build_final_response(st, usage, ctx, status);
    json_to_string(&Value::Object({
        let mut o = Map::new();
        o.insert("type".into(), Value::String(etype.into()));
        o.insert("response".into(), resp);
        o
    }))
}

/// IR chunk → Responses 事件 JSON 字符串列表。事件序列保证合法（created→…→completed）。
pub fn chunk_from_ir(
    chunk: &IrChunk,
    st: &mut StreamState,
    ctx: &mut ConvCtx,
) -> Result<Vec<String>, ConvertError> {
    let mut events: Vec<String> = Vec::new();

    // 首个 chunk 产出 response.created 并记录骨架
    if !st.created {
        st.created = true;
        st.id = chunk.id.clone();
        st.model = chunk.model.clone();
        st.created_at = chunk.created;
        events.push(encode_response_created(st)?);
    }

    if let Some(choice) = chunk.choices.first() {
        // 文本增量
        if let Some(content) = &choice.delta.content {
            if !content.is_empty() {
                if !st.message_item_added {
                    st.message_item_added = true;
                    st.message_output_index = st.next_output_index;
                    st.next_output_index += 1;
                    st.message_item_id = format!("msg_{}", rand_suffix());
                    events.push(encode_output_item_added_message(st)?);
                }
                st.message_text.push_str(content);
                events.push(encode_output_text_delta(st, content)?);
            }
        }
        // 思考增量
        if let Some(rc) = &choice.delta.reasoning_content {
            if !rc.is_empty() {
                if !st.reasoning_item_added {
                    st.reasoning_item_added = true;
                    st.reasoning_output_index = st.next_output_index;
                    st.next_output_index += 1;
                    st.reasoning_item_id = format!("rs_{}", rand_suffix());
                    events.push(encode_output_item_added_reasoning(st)?);
                }
                st.reasoning_text.push_str(rc);
                events.push(encode_reasoning_summary_text_delta(st, rc)?);
            }
        }
        // 工具调用增量（同一 index 只发一次 output_item.added）
        for tc in &choice.delta.tool_calls {
            let idx = tc.index;
            if !st.tool_items.contains_key(&idx) {
                let output_index = st.next_output_index;
                st.next_output_index += 1;
                let state = FnItemState {
                    output_index,
                    item_id: format!("fc_{}", rand_suffix()),
                    call_id: tc.id.clone().unwrap_or_default(),
                    name: tc.name.clone().unwrap_or_default(),
                    arguments: String::new(),
                };
                st.tool_items.insert(idx, state);
                events.push(encode_output_item_added_function_call(st, idx)?);
            }
            if let Some(args) = &tc.arguments {
                if let Some(state) = st.tool_items.get_mut(&idx) {
                    state.arguments.push_str(args);
                    events.push(encode_function_call_arguments_delta(st, idx, args)?);
                }
            }
        }
    }

    // usage 累积：OpenAI include_usage 下内容 chunk 的 usage 为 null（入口已滤除），
    // 真实 usage 常在 finish 之后的独立尾帧到达（发布审阅 H1）。
    if chunk.usage.is_some() {
        st.last_usage = chunk.usage.clone();
    }
    if let Some(fr) = chunk.choices.first().and_then(|c| c.finish_reason.clone()) {
        st.pending_finish = Some(fr);
    }
    // 终止时机：failed（上游错误）立即终止；否则 finish 且已见 usage（同帧或尾帧）；
    // finish 先到而 usage 未至 → 等 usage 尾帧，stream_end 兜底（usage 为 None）。
    if !st.completed && st.pending_finish.is_some() {
        let failed = st.pending_finish.as_deref() == Some("failed");
        if failed || st.last_usage.is_some() {
            st.completed = true;
            let fr = st.pending_finish.take();
            events.push(encode_response_finished(
                st,
                fr.as_deref(),
                st.last_usage.as_ref(),
                ctx,
            )?);
        }
    }

    Ok(events)
}

/// 流终止：若未发过终止帧则补发 completed（无 usage）；否则返回空。
/// 注意：本函数仅在「上游流正常 EOF 但无 finish 帧」时触发（上游连接错误走
impl StreamState {
    /// 流传输中断（上游连接错误）：标记 failed，使 stream_end 发 response.failed
    /// 而非 completed（发布审阅 L11）。
    pub fn mark_failed(&mut self) {
        if !self.completed {
            self.pending_finish = Some("failed".into());
        }
    }
}

/// 流错误通道，不会到这里）；补发 completed 是对截断流的务实兜底。
pub fn stream_end(st: &mut StreamState, ctx: &mut ConvCtx) -> Result<Vec<String>, ConvertError> {
    if st.completed {
        return Ok(vec![]);
    }
    st.completed = true;
    let fr = st.pending_finish.take();
    let (status, etype) = match fr.as_deref() {
        Some("failed") => ("failed", "response.failed"),
        Some("length") => ("incomplete", "response.incomplete"),
        _ => ("completed", "response.completed"),
    };
    let resp = build_final_response(st, st.last_usage.as_ref(), ctx, status);
    let mut o = Map::new();
    o.insert("type".into(), Value::String(etype.into()));
    o.insert("response".into(), resp);
    Ok(vec![json_to_string(&Value::Object(o))?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ConvCtx {
        ConvCtx::new()
    }

    #[test]
    fn assistant_input_uses_output_text() {
        // OpenAI 2025-10 起拒绝 assistant 消息内的 input_text（实测 400）→ 文本 part 用 output_text
        let req = IrRequest {
            messages: vec![
                IrMessage {
                    role: IrRole::Assistant,
                    content: Some(IrContent::Text("prev answer".into())),
                    ..Default::default()
                },
                IrMessage {
                    role: IrRole::User,
                    content: Some(IrContent::Text("next".into())),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let out = request_from_ir(&req, &mut ctx()).unwrap();
        assert_eq!(out["input"][0]["content"][0]["type"], "output_text");
        assert_eq!(out["input"][0]["content"][0]["text"], "prev answer");
        assert_eq!(out["input"][1]["content"][0]["type"], "input_text");
    }

    fn event_types(evs: &[String]) -> Vec<String> {
        evs.iter()
            .map(|e| {
                serde_json::from_str::<Value>(e).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn text_conversation() {
        let j = serde_json::json!({
            "model": "gpt-4o",
            "instructions": "You are helpful.",
            "input": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Hello"}]}
            ],
            "max_output_tokens": 128,
            "temperature": 0.7,
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.max_tokens, Some(128));
        assert_eq!(req.temperature, Some(0.7));
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, IrRole::System);
        assert_eq!(
            req.messages[0].content,
            Some(IrContent::Text("You are helpful.".into()))
        );
        assert_eq!(req.messages[1].role, IrRole::User);
        assert_eq!(
            req.messages[1].content,
            Some(IrContent::Parts(vec![IrPart::Text {
                text: "Hello".into()
            }]))
        );

        // IR → Responses
        let back = request_from_ir(&req, &mut c).unwrap();
        assert_eq!(back["instructions"], "You are helpful.");
        assert_eq!(back["max_output_tokens"], 128);
        let input = back["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[0]["content"][0]["text"], "Hello");
    }

    #[test]
    fn tool_call_and_result() {
        let j = serde_json::json!({
            "model": "gpt-4o",
            "input": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "weather in bj?"}]},
                {"type": "function_call", "call_id": "call_1", "name": "get_weather", "arguments": "{\"city\":\"bj\"}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "sunny"}
            ],
            "tools": [{"type": "function", "name": "get_weather", "description": "weather", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}]
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].name, "get_weather");
        // user -> function_call(assistant) -> function_call_output(tool)
        assert_eq!(req.messages[0].role, IrRole::User);
        assert_eq!(req.messages[1].role, IrRole::Assistant);
        assert_eq!(req.messages[1].tool_calls.len(), 1);
        assert_eq!(req.messages[1].tool_calls[0].id, "call_1");
        assert_eq!(req.messages[1].tool_calls[0].name, "get_weather");
        assert_eq!(req.messages[1].tool_calls[0].arguments, "{\"city\":\"bj\"}");
        assert_eq!(req.messages[2].role, IrRole::Tool);
        assert_eq!(req.messages[2].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(
            req.messages[2].content,
            Some(IrContent::Text("sunny".into()))
        );

        // IR → Responses
        let back = request_from_ir(&req, &mut c).unwrap();
        let input = back["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_1");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[2]["output"], "sunny");
        assert_eq!(back["tools"][0]["type"], "function");
        assert_eq!(back["tools"][0]["name"], "get_weather");
    }

    #[test]
    fn image_input_image() {
        let j = serde_json::json!({
            "model": "gpt-4o",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "what is this?"},
                    {"type": "input_image", "image_url": "https://example.com/a.png"},
                    {"type": "input_image", "image_url": {"url": "data:image/png;base64,AAAABBBB"}}
                ]
            }]
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        let content = req.messages[0].content.as_ref().unwrap();
        let parts = match content {
            IrContent::Parts(ps) => ps,
            other => panic!("应为 Parts: {other:?}"),
        };
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts[0],
            IrPart::Text {
                text: "what is this?".into()
            }
        );
        assert_eq!(
            parts[1],
            IrPart::ImageUrl {
                url: "https://example.com/a.png".into()
            }
        );
        assert_eq!(
            parts[2],
            IrPart::ImageInline {
                media_type: "image/png".into(),
                data: "AAAABBBB".into()
            }
        );

        // IR → Responses：input_image 回写
        let back = request_from_ir(&req, &mut c).unwrap();
        let content = back["input"][0]["content"].as_array().unwrap();
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], "https://example.com/a.png");
        assert_eq!(content[2]["image_url"], "data:image/png;base64,AAAABBBB");
    }

    #[test]
    fn previous_response_id_degraded() {
        let j = serde_json::json!({
            "model": "gpt-4o",
            "input": "hi",
            "previous_response_id": "resp_abc",
            "store": true,
            "metadata": {"a": 1}
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, IrRole::User);
        // 有状态字段保留到 extra，并记为降级
        assert_eq!(req.extra["previous_response_id"], "resp_abc");
        assert_eq!(req.extra["store"], true);
        assert_eq!(req.extra["metadata"]["a"], 1);
        let fields: Vec<String> = c.degraded.iter().map(|d| d.field.clone()).collect();
        assert!(fields.contains(&"previous_response_id".to_string()));
        assert!(fields.contains(&"store".to_string()));
        // metadata 只进 extra，不降级
        assert!(!fields.contains(&"metadata".to_string()));
    }

    #[test]
    fn max_output_tokens_normalized_and_n_degraded() {
        // Responses → IR：max_output_tokens → max_tokens
        let j = serde_json::json!({
            "model": "gpt-4o",
            "input": "hi",
            "max_output_tokens": 256
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.max_tokens, Some(256));

        // IR → Responses：max_tokens → max_output_tokens；n 降级
        let mut req2 = IrRequest {
            model: "gpt-4o".into(),
            messages: vec![IrMessage {
                role: IrRole::User,
                content: Some(IrContent::Text("hi".into())),
                ..Default::default()
            }],
            max_tokens: Some(256),
            n: Some(2),
            ..Default::default()
        };
        let mut c2 = ctx();
        let back = request_from_ir(&req2, &mut c2).unwrap();
        assert_eq!(back["max_output_tokens"], 256);
        assert!(c2.degraded.iter().any(|d| d.field == "n"));
        req2.n = None;
        let _ = req2;
    }

    #[test]
    fn usage_cache_and_reasoning_tokens() {
        // Responses → IR
        let j = serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 123,
            "status": "completed",
            "model": "gpt-4o",
            "output": [{
                "type": "message",
                "id": "msg_1",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "hi there"}]
            }],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "total_tokens": 15,
                "input_tokens_details": {"cached_tokens": 3},
                "output_tokens_details": {"reasoning_tokens": 2}
            }
        });
        let mut c = ctx();
        let resp = response_to_ir(&j, &mut c).unwrap();
        assert_eq!(resp.id, "resp_1");
        assert_eq!(resp.created, 123);
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(
            resp.choices[0].message.content,
            Some(IrContent::Text("hi there".into()))
        );
        let usage = resp.usage.as_ref().unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, Some(15));
        assert_eq!(usage.cache_read_tokens, Some(3));
        assert_eq!(usage.extra["reasoning_tokens"], 2);

        // IR → Responses
        let back = response_from_ir(&resp, &mut c).unwrap();
        assert_eq!(back["object"], "response");
        assert_eq!(back["status"], "completed");
        assert_eq!(back["created_at"], 123);
        assert_eq!(back["output"][0]["type"], "message");
        assert_eq!(back["output"][0]["content"][0]["type"], "output_text");
        assert_eq!(back["output"][0]["content"][0]["text"], "hi there");
        assert_eq!(back["usage"]["input_tokens"], 10);
        assert_eq!(back["usage"]["output_tokens"], 5);
        assert_eq!(back["usage"]["total_tokens"], 15);
        assert_eq!(back["usage"]["input_tokens_details"]["cached_tokens"], 3);
        assert_eq!(
            back["usage"]["output_tokens_details"]["reasoning_tokens"],
            2
        );
    }

    /// review P5 回归：Chat 形状 usage（completion_tokens_details）转 Responses 时
    /// 归一进 output_tokens_details，不再原样透传 Chat 专属键。
    #[test]
    fn usage_completion_details_normalized_to_responses() {
        let mut c = ctx();
        let mut u = IrUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            ..Default::default()
        };
        u.extra.insert(
            "completion_tokens_details".into(),
            serde_json::json!({"reasoning_tokens": 3, "audio_tokens": 1}),
        );
        let out = usage_from_ir(&u, &mut c);
        assert_eq!(out["output_tokens_details"]["reasoning_tokens"], 3);
        assert_eq!(out["output_tokens_details"]["audio_tokens"], 1);
        assert!(out.get("completion_tokens_details").is_none());
    }

    /// review P5 回归：Chat 形状 tool_choice 转 Responses 时摊平 name。
    #[test]
    fn tool_choice_chat_shape_flattened() {
        let mut c = ctx();
        let mut req = IrRequest {
            model: "gpt-5".into(),
            ..Default::default()
        };
        req.tool_choice =
            Some(serde_json::json!({"type": "function", "function": {"name": "get_weather"}}));
        let body = request_from_ir(&req, &mut c).unwrap();
        assert_eq!(
            body["tool_choice"],
            serde_json::json!({"type": "function", "name": "get_weather"})
        );
    }

    #[test]
    fn response_output_items_combined() {
        let j = serde_json::json!({
            "id": "resp_1", "object": "response", "created_at": 1,
            "status": "completed", "model": "gpt-4o",
            "output": [
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "thinking..."}]},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "answer"}]},
                {"type": "function_call", "call_id": "call_1", "name": "get_weather", "arguments": "{\"city\":\"bj\"}"}
            ]
        });
        let mut c = ctx();
        let resp = response_to_ir(&j, &mut c).unwrap();
        let msg = &resp.choices[0].message;
        assert_eq!(msg.reasoning_content.as_deref(), Some("thinking..."));
        assert_eq!(msg.content, Some(IrContent::Text("answer".into())));
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].id, "call_1");

        let back = response_from_ir(&resp, &mut c).unwrap();
        let output = back["output"].as_array().unwrap();
        assert_eq!(output.len(), 3);
        assert_eq!(output[0]["type"], "message");
        assert_eq!(output[1]["type"], "function_call");
        assert_eq!(output[2]["type"], "reasoning");
    }

    #[test]
    fn streaming_event_sequence_and_roundtrip() {
        let mut c = ctx();
        let mut st = StreamState::default();

        // 1. 文本 delta（首 chunk）
        let ch1 = IrChunk {
            id: "resp_1".into(),
            model: "gpt-4o".into(),
            created: 123,
            choices: vec![IrChunkChoice {
                index: 0,
                delta: IrDelta {
                    role: Some(IrRole::Assistant),
                    content: Some("Hello".into()),
                    ..Default::default()
                },
                finish_reason: None,
            }],
            ..Default::default()
        };
        let ev1 = chunk_from_ir(&ch1, &mut st, &mut c).unwrap();
        assert_eq!(
            event_types(&ev1),
            [
                "response.created",
                "response.output_item.added",
                "response.output_text.delta"
            ]
        );

        // 2. 工具调用（新 index）
        let ch2 = IrChunk {
            id: "resp_1".into(),
            model: "gpt-4o".into(),
            created: 123,
            choices: vec![IrChunkChoice {
                index: 0,
                delta: IrDelta {
                    tool_calls: vec![IrToolCallDelta {
                        index: 0,
                        id: Some("call_1".into()),
                        name: Some("get_weather".into()),
                        arguments: Some("{\"city\"".into()),
                    }],
                    ..Default::default()
                },
                finish_reason: None,
            }],
            ..Default::default()
        };
        let ev2 = chunk_from_ir(&ch2, &mut st, &mut c).unwrap();
        assert_eq!(
            event_types(&ev2),
            [
                "response.output_item.added",
                "response.function_call_arguments.delta"
            ]
        );
        // function_call item 的 call_id/name
        let added_item = serde_json::from_str::<Value>(&ev2[0]).unwrap();
        assert_eq!(added_item["item"]["type"], "function_call");
        assert_eq!(added_item["item"]["name"], "get_weather");
        assert_eq!(added_item["output_index"], 1);

        // 3. 工具参数增量（复用 index 0）
        let ch3 = IrChunk {
            id: "resp_1".into(),
            model: "gpt-4o".into(),
            created: 123,
            choices: vec![IrChunkChoice {
                index: 0,
                delta: IrDelta {
                    tool_calls: vec![IrToolCallDelta {
                        index: 0,
                        arguments: Some(":\"bj\"}".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                finish_reason: None,
            }],
            ..Default::default()
        };
        let ev3 = chunk_from_ir(&ch3, &mut st, &mut c).unwrap();
        assert_eq!(
            event_types(&ev3),
            ["response.function_call_arguments.delta"]
        );

        // 4. 完成 + usage
        let ch4 = IrChunk {
            id: "resp_1".into(),
            model: "gpt-4o".into(),
            created: 123,
            choices: vec![IrChunkChoice {
                index: 0,
                delta: IrDelta::default(),
                finish_reason: Some("stop".into()),
            }],
            usage: Some(IrUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: Some(15),
                cache_read_tokens: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        };
        let ev4 = chunk_from_ir(&ch4, &mut st, &mut c).unwrap();
        assert_eq!(event_types(&ev4), ["response.completed"]);
        let completed = serde_json::from_str::<Value>(&ev4[0]).unwrap();
        assert_eq!(completed["response"]["status"], "completed");
        assert_eq!(completed["response"]["usage"]["input_tokens"], 10);
        assert_eq!(completed["response"]["usage"]["output_tokens"], 5);
        assert_eq!(
            completed["response"]["usage"]["input_tokens_details"]["cached_tokens"],
            3
        );
        // completed 的 output 骨架包含 message + function_call
        let out = completed["response"]["output"].as_array().unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["type"], "message");
        assert_eq!(out[1]["type"], "function_call");
        assert_eq!(out[1]["arguments"], "{\"city\":\"bj\"}");

        // 已发过 completed，stream_end 不再补发
        assert!(stream_end(&mut st, &mut c).unwrap().is_empty());

        // 回放：把用户侧事件喂回 chunk_to_ir，重建内容/工具/usage
        let mut st2 = StreamState::default();
        let mut chunks: Vec<IrChunk> = Vec::new();
        for e in ev1
            .iter()
            .chain(ev2.iter())
            .chain(ev3.iter())
            .chain(ev4.iter())
        {
            if let Some(ch) = chunk_to_ir(e, &mut st2, &mut c).unwrap() {
                chunks.push(ch);
            }
        }
        // 文本增量
        assert!(chunks.iter().any(|ch| ch
            .choices
            .iter()
            .any(|c| c.delta.content.as_deref() == Some("Hello"))));
        // 工具参数增量（两个片段）
        let tool_args: Vec<&str> = chunks
            .iter()
            .filter_map(|ch| ch.choices.first().and_then(|c| c.delta.tool_calls.first()))
            .filter_map(|tc| tc.arguments.as_deref())
            .collect();
        assert!(tool_args.contains(&"{\"city\""));
        assert!(tool_args.contains(&":\"bj\"}"));
        // 完成帧：finish_reason + usage
        let last = chunks.last().unwrap();
        assert_eq!(last.choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(last.usage.as_ref().unwrap().cache_read_tokens, Some(3));
    }

    #[test]
    fn stream_end_completes_when_missing() {
        let mut c = ctx();
        let mut st = StreamState::default();
        let resp = stream_end(&mut st, &mut c).unwrap();
        assert_eq!(resp.len(), 1);
        let ev = serde_json::from_str::<Value>(&resp[0]).unwrap();
        assert_eq!(ev["type"], "response.completed");
    }

    /// review P5 回归：同一 message item 在 added/delta/completed 三类事件中 id 一致，
    /// 且带 msg_ 前缀（此前三处各自随机生成，客户端按 item_id 关联事件会失配）。
    #[test]
    fn stream_item_ids_consistent_across_events() {
        let mut c = ctx();
        let mut st = StreamState::default();
        let mk = |text: &str, finish: Option<&str>| IrChunk {
            id: "resp_1".into(),
            model: "gpt-5".into(),
            created: 1,
            choices: vec![IrChunkChoice {
                index: 0,
                delta: IrDelta {
                    content: Some(text.into()),
                    ..Default::default()
                },
                finish_reason: finish.map(String::from),
            }],
            ..Default::default()
        };
        let mut evs: Vec<Value> = Vec::new();
        for ch in [mk("he", None), mk("llo", Some("stop"))] {
            for e in chunk_from_ir(&ch, &mut st, &mut c).unwrap() {
                evs.push(serde_json::from_str(&e).unwrap());
            }
        }
        // 无 usage 的 finish 帧延迟终止（发布审阅 H1）：completed 在 stream_end 补发
        for e in stream_end(&mut st, &mut c).unwrap() {
            evs.push(serde_json::from_str(&e).unwrap());
        }
        let added_id = evs
            .iter()
            .find(|e| e["type"] == "response.output_item.added")
            .unwrap()["item"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(added_id.starts_with("msg_"));
        for e in evs
            .iter()
            .filter(|e| e["type"] == "response.output_text.delta")
        {
            assert_eq!(e["item_id"].as_str().unwrap(), added_id);
        }
        let completed = evs
            .iter()
            .find(|e| e["type"] == "response.completed")
            .unwrap();
        assert_eq!(
            completed["response"]["output"][0]["id"].as_str().unwrap(),
            added_id
        );
    }

    /// 发布审阅 L11：流传输中断 mark_failed → stream_end 发 response.failed（非 completed）。
    #[test]
    fn stream_fail_end_emits_failed() {
        let mut c = ctx();
        let mut st = StreamState::default();
        let ch = IrChunk {
            id: "r".into(),
            model: "m".into(),
            created: 1,
            choices: vec![IrChunkChoice {
                index: 0,
                delta: IrDelta {
                    content: Some("hi".into()),
                    ..Default::default()
                },
                finish_reason: None,
            }],
            ..Default::default()
        };
        chunk_from_ir(&ch, &mut st, &mut c).unwrap();
        st.mark_failed();
        let end = stream_end(&mut st, &mut c).unwrap();
        let v: Value = serde_json::from_str(&end[0]).unwrap();
        assert_eq!(v["type"], "response.failed");
        assert_eq!(v["response"]["status"], "failed");
        // 重复调用不再发
        assert!(stream_end(&mut st, &mut c).unwrap().is_empty());
    }

    /// 发布审阅 H1 回归：OpenAI include_usage 模式——finish 帧 usage=null（入口已滤除），
    /// 真实 usage 在独立尾帧。completed 必须在 usage 尾帧发出且携带真实 usage。
    #[test]
    fn stream_completed_waits_for_usage_tail() {
        let mut c = ctx();
        let mut st = StreamState::default();
        let mk = |text: Option<&str>, finish: Option<&str>, usage: Option<(u64, u64)>| IrChunk {
            id: "chatcmpl_1".into(),
            model: "gpt-5".into(),
            created: 1,
            choices: vec![IrChunkChoice {
                index: 0,
                delta: IrDelta {
                    content: text.map(String::from),
                    ..Default::default()
                },
                finish_reason: finish.map(String::from),
            }],
            usage: usage.map(|(p, ct)| crate::protocol::ir::IrUsage {
                prompt_tokens: p,
                completion_tokens: ct,
                ..Default::default()
            }),
            ..Default::default()
        };
        // 内容 chunk（usage 已被入口滤除）→ 不得发 completed
        let e1 = chunk_from_ir(&mk(Some("hi"), None, None), &mut st, &mut c).unwrap();
        assert!(!e1.iter().any(|e| e.contains("response.completed")));
        // finish 帧（无 usage）→ 仍不得发 completed
        let e2 = chunk_from_ir(&mk(None, Some("stop"), None), &mut st, &mut c).unwrap();
        assert!(!e2.iter().any(|e| e.contains("response.completed")));
        // usage 尾帧 → 发出 completed 且带真实 usage
        let e3 = chunk_from_ir(&mk(None, None, Some((11, 7))), &mut st, &mut c).unwrap();
        let completed = e3
            .iter()
            .find(|e| e.contains("response.completed"))
            .unwrap();
        let v: Value = serde_json::from_str(completed).unwrap();
        assert_eq!(v["response"]["usage"]["input_tokens"], 11);
        assert_eq!(v["response"]["usage"]["output_tokens"], 7);
        // stream_end 不再重复发
        assert!(stream_end(&mut st, &mut c).unwrap().is_empty());
    }

    /// 无 usage 尾帧的上游（未开 include_usage）：stream_end 兜底发 completed。
    #[test]
    fn stream_completed_fallback_at_stream_end() {
        let mut c = ctx();
        let mut st = StreamState::default();
        let ch = IrChunk {
            id: "chatcmpl_2".into(),
            model: "m".into(),
            created: 1,
            choices: vec![IrChunkChoice {
                index: 0,
                delta: IrDelta::default(),
                finish_reason: Some("length".into()),
            }],
            ..Default::default()
        };
        let evs = chunk_from_ir(&ch, &mut st, &mut c).unwrap();
        assert!(!evs.iter().any(|e| e.contains("response.incomplete")));
        let end = stream_end(&mut st, &mut c).unwrap();
        let v: Value = serde_json::from_str(&end[0]).unwrap();
        assert_eq!(v["type"], "response.incomplete"); // finish=length → incomplete
    }

    /// review P5 回归：上游 response.failed 不再被丢弃后伪装成 completed，
    /// 入站映射为 finish=failed，出站立发 response.failed 帧。
    #[test]
    fn stream_failed_event_propagates() {
        let mut c = ctx();
        let mut st = StreamState::default();
        let created = serde_json::json!({
            "type": "response.created",
            "response": {"id": "resp_x", "model": "gpt-5", "created_at": 1}
        });
        chunk_to_ir(&created.to_string(), &mut st, &mut c).unwrap();
        let failed = serde_json::json!({
            "type": "response.failed",
            "response": {"status": "failed", "error": {"code": "server_error", "message": "boom"}}
        });
        let ch = chunk_to_ir(&failed.to_string(), &mut st, &mut c)
            .unwrap()
            .expect("failed 应产出终止 chunk");
        assert_eq!(ch.choices[0].finish_reason.as_deref(), Some("failed"));

        let mut st2 = StreamState::default();
        let evs = chunk_from_ir(&ch, &mut st2, &mut c).unwrap();
        let types: Vec<String> = evs
            .iter()
            .map(|e| {
                serde_json::from_str::<Value>(e).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert!(types.contains(&"response.failed".to_string()));
        let failed_ev = evs
            .iter()
            .map(|e| serde_json::from_str::<Value>(e).unwrap())
            .find(|e| e["type"] == "response.failed")
            .unwrap();
        assert_eq!(failed_ev["response"]["status"], "failed");
    }

    /// 入站工具链（review P1-#9）：output_item.added 后随 arguments delta，
    /// 聚合出的 IR delta 必须携带 name 与 call_id。
    #[test]
    fn inbound_function_call_keeps_name_and_id() {
        let mut c = ctx();
        let mut st = StreamState::default();

        // added 帧：item 带 id/call_id/name（OpenAI Responses 实际形状）
        let added = serde_json::json!({
            "type": "response.output_item.added",
            "output_index": 1,
            "item": {
                "type": "function_call",
                "id": "fc_abc",
                "call_id": "call_1",
                "name": "get_weather",
                "arguments": ""
            }
        });
        let r1 = chunk_to_ir(&added.to_string(), &mut st, &mut c).unwrap();
        assert!(r1.is_none());

        // 参数增量帧：只带 item_id + delta
        let delta = serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "fc_abc",
            "output_index": 1,
            "delta": "{\"city\":\"bj\"}"
        });
        let r2 = chunk_to_ir(&delta.to_string(), &mut st, &mut c).unwrap();
        let ch = r2.expect("参数增量应产出 IR chunk");
        let tc = &ch.choices[0].delta.tool_calls[0];
        assert_eq!(tc.index, 0);
        assert_eq!(tc.id.as_deref(), Some("call_1"));
        assert_eq!(tc.name.as_deref(), Some("get_weather"));
        assert_eq!(tc.arguments.as_deref(), Some("{\"city\":\"bj\"}"));

        // call_id 缺省时回退 item.id
        let mut st2 = StreamState::default();
        let added2 = serde_json::json!({
            "type": "response.output_item.added",
            "item": { "type": "function_call", "id": "fc_x", "name": "lookup" }
        });
        chunk_to_ir(&added2.to_string(), &mut st2, &mut c).unwrap();
        let delta2 = serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "fc_x",
            "delta": "{}"
        });
        let ch2 = chunk_to_ir(&delta2.to_string(), &mut st2, &mut c)
            .unwrap()
            .unwrap();
        let tc2 = &ch2.choices[0].delta.tool_calls[0];
        assert_eq!(tc2.id.as_deref(), Some("fc_x"));
        assert_eq!(tc2.name.as_deref(), Some("lookup"));
    }
}
