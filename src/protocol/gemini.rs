//! Gemini generateContent 适配器（/v1beta/models/{model}:generateContent）。
//!
//! 要点（PLAN.md §4.2/§4.4）：
//! - 模型在 URL 路径而非请求体（本适配器只处理 body；model 字段留空由上层从路径提取/注入）；
//! - 系统提示 → `systemInstruction`；图片 → parts.inline_data（base64 内联）；
//!   URL 图片需降级或交由上层受控下载（media_download 开关，M3 接线）；
//! - 工具 → tools[functionDeclarations]；工具调用 parts.functionCall / functionResponse；
//! - usage → usageMetadata；流式为 data: 增量块（与 generateContent 响应同构）。
//!
//! 依赖 IR 枢轴（N→1→N），无法映射能力记 `ctx.degrade`（§4.3）。

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use super::ir::*;
use super::{ConvCtx, ConvertError};

/// 被本适配器识别并映射到 IR 的顶层请求字段；其余进 `extra`。
const KNOWN_REQUEST_FIELDS: [&str; 5] = [
    "systemInstruction",
    "contents",
    "generationConfig",
    "tools",
    "safetySettings",
];

fn json_object() -> Value {
    Value::Object(Map::new())
}

/// 解析 Gemini 角色为 IR 角色；未知角色记降级按 user 容错。
fn parse_role(role: Option<&str>, ctx: &mut ConvCtx) -> IrRole {
    match role {
        Some("user") => IrRole::User,
        Some("model") => IrRole::Assistant,
        Some("system") | Some("assistant") => IrRole::Assistant,
        Some(other) => {
            ctx.degrade("content.role", format!("未知角色按 user 处理: {other}"));
            IrRole::User
        }
        _ => IrRole::User,
    }
}

/// IR 角色 → Gemini 角色字符串。
fn role_to_str(r: IrRole) -> &'static str {
    match r {
        IrRole::Assistant => "model",
        // tool 结果在 Gemini 中以 user role + functionResponse part 承载；system 已剥离到 systemInstruction
        IrRole::User | IrRole::Tool | IrRole::System => "user",
    }
}

/// 解析 `generationConfig` → 各 IR 字段。
fn parse_generation_config(gc: &Value, req: &mut IrRequest, ctx: &mut ConvCtx) {
    if !gc.is_object() {
        ctx.degrade("generationConfig", "generationConfig 非对象，忽略");
        return;
    }
    if let Some(mt) = gc["maxOutputTokens"].as_u64() {
        req.max_tokens = Some(mt);
    }
    req.temperature = gc["temperature"].as_f64();
    req.top_p = gc["topP"].as_f64();
    if let Some(k) = gc["topK"].as_u64() {
        match u32::try_from(k) {
            Ok(k32) => req.ext.top_k = Some(k32),
            Err(_) => ctx.degrade("generationConfig.topK", "topK 超出 u32 范围，丢弃"),
        }
    }
    if let Some(seqs) = gc.get("stopSequences") {
        if let Some(arr) = seqs.as_array() {
            req.stop = Some(
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect(),
            );
        } else {
            ctx.degrade(
                "generationConfig.stopSequences",
                "stopSequences 非数组，忽略",
            );
        }
    }
    // responseMimeType/responseSchema → response_format（从简：json 模式 → json_object）
    let mime = gc["responseMimeType"].as_str();
    let schema = gc.get("responseSchema");
    if mime == Some("application/json") {
        if let Some(s) = schema {
            req.response_format =
                Some(serde_json::json!({"type": "json_schema", "json_schema": s}));
        } else {
            req.response_format = Some(serde_json::json!({"type": "json_object"}));
        }
    }
}

/// 解析工具定义为 IR。Gemini `tools[].functionDeclarations[]`。
fn parse_tools(v: &Value, ctx: &mut ConvCtx) -> Result<Vec<IrTool>, ConvertError> {
    let arr = v
        .as_array()
        .ok_or_else(|| ConvertError::Parse("tools 不是数组".into()))?;
    let mut out = Vec::new();
    for t in arr {
        if let Some(fds) = t.get("functionDeclarations").and_then(|f| f.as_array()) {
            for f in fds {
                out.push(IrTool {
                    name: f["name"].as_str().unwrap_or_default().to_string(),
                    description: f["description"].as_str().map(String::from),
                    parameters: f.get("parameters").cloned().unwrap_or_else(json_object),
                });
            }
        } else {
            ctx.degrade(
                "tools",
                format!(
                    "非 functionDeclarations 工具，丢弃: {}",
                    t["type"].as_str().unwrap_or_default()
                ),
            );
        }
    }
    Ok(out)
}

/// 收集 systemInstruction.parts[].text 并合并为一段字符串。
fn collect_system_text(parts: &Value, ctx: &mut ConvCtx) -> String {
    let mut out = String::new();
    if let Some(arr) = parts.as_array() {
        for p in arr {
            if let Some(t) = p["text"].as_str() {
                out.push_str(t);
            } else {
                ctx.degrade("systemInstruction.part", "非 text part，忽略");
            }
        }
    } else if let Some(t) = parts["text"].as_str() {
        out.push_str(t);
    }
    out
}

/// 解析单条 Gemini `content`（role + parts）→ 可能产出多条 IR 消息（text/media 主消息 + functionResponse 的 Tool 消息）。
fn parse_content(
    c: &Value,
    ctx: &mut ConvCtx,
    messages: &mut Vec<IrMessage>,
) -> Result<(), ConvertError> {
    if !c.is_object() {
        return Err(ConvertError::Parse("content 不是对象".into()));
    }
    let role = parse_role(c["role"].as_str(), ctx);

    let mut text_parts: Vec<String> = Vec::new();
    let mut media_parts: Vec<IrPart> = Vec::new();
    let mut tool_calls: Vec<IrToolCall> = Vec::new();
    let mut function_responses: Vec<(String, String)> = Vec::new();
    let mut reasoning: Option<String> = None;
    let mut fc_seq: u32 = 0;

    if let Some(ps) = c.get("parts").and_then(|p| p.as_array()) {
        for p in ps {
            if !p.is_object() {
                ctx.degrade("content.part", "part 非对象，丢弃");
                continue;
            }
            if let Some(t) = p["text"].as_str() {
                if p["thought"].as_bool().unwrap_or(false) {
                    reasoning = Some(t.to_string());
                } else {
                    text_parts.push(t.to_string());
                }
            } else if let Some(id) = p.get("inlineData") {
                media_parts.push(IrPart::ImageInline {
                    media_type: id["mimeType"].as_str().unwrap_or_default().to_string(),
                    data: id["data"].as_str().unwrap_or_default().to_string(),
                });
            } else if let Some(fd) = p.get("fileData") {
                let uri = fd["fileUri"].as_str().unwrap_or_default();
                media_parts.push(IrPart::File {
                    name: fd["displayName"].as_str().unwrap_or_default().to_string(),
                    url: if uri.is_empty() {
                        None
                    } else {
                        Some(uri.to_string())
                    },
                    data: fd["data"].as_str().map(String::from),
                });
            } else if let Some(fc) = p.get("functionCall") {
                let name = fc["name"].as_str().unwrap_or_default().to_string();
                let arguments = args_to_string(&fc["args"]);
                let id = format!("{name}_{fc_seq}");
                fc_seq += 1;
                tool_calls.push(IrToolCall {
                    id,
                    name,
                    arguments,
                });
            } else if let Some(fr) = p.get("functionResponse") {
                let name = fr["name"].as_str().unwrap_or_default().to_string();
                let response = args_to_string(&fr["response"]);
                function_responses.push((name, response));
            } else {
                ctx.degrade("content.part", "未知 part 结构，丢弃");
            }
        }
    }

    // 主消息：text/media + reasoning + tool_calls
    let mut content_parts: Vec<IrPart> = Vec::new();
    for t in &text_parts {
        content_parts.push(IrPart::Text { text: t.clone() });
    }
    content_parts.extend(media_parts);

    let content = if content_parts.len() == 1 {
        match &content_parts[0] {
            IrPart::Text { text } => Some(IrContent::Text(text.clone())),
            _ => Some(IrContent::Parts(content_parts)),
        }
    } else if !content_parts.is_empty() {
        Some(IrContent::Parts(content_parts))
    } else {
        None
    };

    let msg = IrMessage {
        role,
        content,
        tool_calls,
        reasoning_content: reasoning,
        ..Default::default()
    };
    let has_content =
        msg.content.is_some() || !msg.tool_calls.is_empty() || msg.reasoning_content.is_some();
    if has_content {
        messages.push(msg);
    }

    // functionResponse → 独立 Tool 消息（role=Tool，content=Text(response JSON 串)，tool_call_id=name）
    for (name, response) in function_responses {
        messages.push(IrMessage {
            role: IrRole::Tool,
            content: Some(IrContent::Text(response)),
            name: Some(name.clone()),
            tool_call_id: Some(name),
            ..Default::default()
        });
    }

    Ok(())
}

/// 将 Gemini args/response 值序列化为 JSON 字符串（对象/数字/布尔均可；null → 空串）。
fn args_to_string(v: &Value) -> String {
    if v.is_null() {
        String::new()
    } else {
        serde_json::to_string(v).unwrap_or_default()
    }
}

/// 将 IR arguments JSON 字符串解析回 Gemini 字面量（对象）。非法则原样字符串。
fn args_string_to_value(args: &str, field: &str, ctx: &mut ConvCtx) -> Value {
    if args.trim().is_empty() {
        return json_object();
    }
    match serde_json::from_str::<Value>(args) {
        Ok(v) => v,
        Err(_) => {
            ctx.degrade(field, "arguments 非合法 JSON，按字符串携带");
            Value::String(args.to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// 请求：Gemini JSON → IR
// ---------------------------------------------------------------------------

/// 请求 JSON → IR。model 留空（在 URL 路径，由上层注入）。
pub fn request_to_ir(v: &Value, ctx: &mut ConvCtx) -> Result<IrRequest, ConvertError> {
    let obj = v
        .as_object()
        .ok_or_else(|| ConvertError::Parse("请求体不是对象".into()))?;
    let mut req = IrRequest::default();
    // 注意：model 不在 body（URL 路径），留空字符串由上层注入
    req.model = String::new();

    // systemInstruction → system 消息
    if let Some(si) = v.get("systemInstruction") {
        let text = si
            .get("parts")
            .map(|p| collect_system_text(p, ctx))
            .unwrap_or_else(|| si["text"].as_str().unwrap_or_default().to_string());
        if !text.is_empty() {
            req.messages.push(IrMessage {
                role: IrRole::System,
                content: Some(IrContent::Text(text)),
                ..Default::default()
            });
        }
    }

    // contents → messages
    if let Some(contents) = v.get("contents") {
        let arr = contents
            .as_array()
            .ok_or_else(|| ConvertError::Parse("contents 不是数组".into()))?;
        for c in arr {
            parse_content(c, ctx, &mut req.messages)?;
        }
    }

    // generationConfig
    if let Some(gc) = v.get("generationConfig") {
        parse_generation_config(gc, &mut req, ctx);
    }

    // tools
    if let Some(tools) = v.get("tools") {
        req.tools = parse_tools(tools, ctx)?;
    }

    // safetySettings → extra
    if let Some(ss) = v.get("safetySettings") {
        req.extra.insert("safetySettings".into(), ss.clone());
    }

    // 未知请求字段 → extra
    for (k, val) in obj {
        if !KNOWN_REQUEST_FIELDS.contains(&k.as_str()) {
            req.extra.insert(k.clone(), val.clone());
        }
    }

    Ok(req)
}

// ---------------------------------------------------------------------------
// 请求：IR → Gemini JSON
// ---------------------------------------------------------------------------

fn tool_to_json(t: &IrTool) -> Value {
    let mut f = Map::new();
    f.insert("name".into(), Value::String(t.name.clone()));
    if let Some(d) = &t.description {
        f.insert("description".into(), Value::String(d.clone()));
    }
    f.insert("parameters".into(), t.parameters.clone());
    Value::Object(f)
}

/// IR part → Gemini part。
fn part_to_json(p: &IrPart, ctx: &mut ConvCtx) -> Option<Value> {
    match p {
        IrPart::Text { text } => Some(serde_json::json!({"text": text})),
        IrPart::ImageInline { media_type, data } => {
            Some(serde_json::json!({"inlineData": {"mimeType": media_type, "data": data}}))
        }
        // Gemini 需求内联字节；URL 图片无法直接内联 → 以 fileData(fileUri) 携带并降级记录 URL 风险
        IrPart::ImageUrl { url } => {
            ctx.degrade("image_url", "Gemini 需内联数据，URL 图片需上层受控下载");
            Some(serde_json::json!({"fileData": {"fileUri": url}}))
        }
        IrPart::File { name, url, data } => {
            let mut fd = Map::new();
            if let Some(u) = url {
                fd.insert("fileUri".into(), Value::String(u.clone()));
            }
            if let Some(d) = data {
                fd.insert("data".into(), Value::String(d.clone()));
            }
            if !name.is_empty() {
                fd.insert("displayName".into(), Value::String(name.clone()));
            }
            Some(serde_json::json!({"fileData": Value::Object(fd)}))
        }
        IrPart::InputAudio { .. } => {
            ctx.degrade("input_audio", "Gemini 不支持音频");
            None
        }
    }
}

/// converts job message → Gemini content（system 已在 `systemInstruction`，这里只处理 contents）。
fn message_to_content(m: &IrMessage, ctx: &mut ConvCtx) -> Option<Value> {
    if m.role == IrRole::System {
        return None;
    }

    let mut parts: Vec<Value> = Vec::new();

    if m.role == IrRole::Tool {
        // 工具结果 → user role 的 functionResponse part
        let name = m
            .name
            .clone()
            .or_else(|| m.tool_call_id.clone())
            .unwrap_or_default();
        if m.name.is_none() {
            ctx.degrade(
                "tool_response_name",
                "Gemini functionResponse 需函数名，使用 tool_call_id 代替",
            );
        }
        let response = response_content_to_value(m.content.as_ref(), ctx);
        parts.push(serde_json::json!({"functionResponse": {"name": name, "response": response}}));
    } else {
        // 常规 content
        if let Some(content) = &m.content {
            match content {
                IrContent::Text(s) => {
                    if !s.is_empty() {
                        parts.push(serde_json::json!({"text": s}));
                    }
                }
                IrContent::Parts(ps) => {
                    for p in ps {
                        if let Some(pv) = part_to_json(p, ctx) {
                            parts.push(pv);
                        }
                    }
                }
            }
        }
        // 思考 → thought part
        if let Some(rc) = &m.reasoning_content {
            parts.push(serde_json::json!({"text": rc, "thought": true}));
        }
        // 工具调用 → functionCall parts
        for tc in &m.tool_calls {
            let args = args_string_to_value(&tc.arguments, "tool_call.arguments", ctx);
            parts.push(serde_json::json!({"functionCall": {"name": tc.name, "args": args}}));
        }
    }

    if parts.is_empty() {
        // 空 content 无业务内容 → 丢弃（避免 Gemini 空 parts 报错）
        return None;
    }
    Some(serde_json::json!({"role": role_to_str(m.role), "parts": parts}))
}

/// Tool 消息 content（JSON 字符串）→ Gemini functionResponse.response 字面量。
fn response_content_to_value(content: Option<&IrContent>, ctx: &mut ConvCtx) -> Value {
    let s = match content {
        Some(IrContent::Text(s)) => s.clone(),
        Some(inner) => {
            ctx.degrade(
                "tool_response_content",
                "functionResponse 内容非纯文本，序列化处理",
            );
            serde_json::to_string(inner).unwrap_or_default()
        }
        None => String::new(),
    };
    if s.trim().is_empty() {
        return Value::Null;
    }
    match serde_json::from_str::<Value>(&s) {
        Ok(v) => v,
        Err(_) => {
            ctx.degrade(
                "tool_response_json",
                "functionResponse 内容非合法 JSON，按字符串携带",
            );
            Value::String(s)
        }
    }
}

/// 把 IR 系统消息转成 systemInstruction.parts 文本数组。
fn system_msg_to_instruction_parts(m: &IrMessage, ctx: &mut ConvCtx) -> Vec<Value> {
    let mut parts = Vec::new();
    match &m.content {
        Some(IrContent::Text(s)) => {
            if !s.is_empty() {
                parts.push(serde_json::json!({"text": s}));
            }
        }
        Some(IrContent::Parts(ps)) => {
            for p in ps {
                if let IrPart::Text { text } = p {
                    if !text.is_empty() {
                        parts.push(serde_json::json!({"text": text}));
                    }
                } else {
                    ctx.degrade("systemInstruction.part", "系统提示含非文本 part，忽略");
                }
            }
        }
        None => {}
    }
    parts
}

/// IR → 请求 JSON。model 不写入 body（在 URL 路径中）。
pub fn request_from_ir(req: &IrRequest, ctx: &mut ConvCtx) -> Result<Value, ConvertError> {
    let mut body = req.extra.clone();
    // Gemni 无对应字段 → 降级
    if req.tool_choice.is_some() {
        ctx.degrade("tool_choice", "Gemini 无 tool_choice 字段，忽略");
    }
    if req.seed.is_some() {
        ctx.degrade("seed", "Gemini 无 seed 字段，忽略");
    }
    if req.n.is_some() {
        ctx.degrade("n", "Gemini 无 n 字段，忽略");
    }
    if req.user.is_some() {
        ctx.degrade("user", "Gemini 无 user 字段，忽略");
    }
    if req.stream {
        ctx.degrade(
            "stream",
            "Gemini 流式经独立端点（streamGenerateContent），body 不含 stream",
        );
    }
    if !req.ext.extra.is_empty() {
        for k in req.ext.extra.keys() {
            ctx.degrade(k.clone(), "Gemini 无对应字段");
        }
    }

    body.remove("model");

    // system 消息 → systemInstruction
    let system_msgs: Vec<&IrMessage> = req
        .messages
        .iter()
        .filter(|m| m.role == IrRole::System)
        .collect();
    if !system_msgs.is_empty() {
        let mut parts: Vec<Value> = Vec::new();
        for m in &system_msgs {
            parts.extend(system_msg_to_instruction_parts(m, ctx));
        }
        if !parts.is_empty() {
            body.insert(
                "systemInstruction".into(),
                serde_json::json!({"parts": parts}),
            );
        }
    }

    // 其余消息 → contents
    let mut contents = Vec::new();
    for m in req.messages.iter().filter(|m| m.role != IrRole::System) {
        if let Some(c) = message_to_content(m, ctx) {
            contents.push(c);
        }
    }
    if !contents.is_empty() {
        body.insert("contents".into(), Value::Array(contents));
    }

    // generationConfig
    let mut gc = Map::new();
    if let Some(mt) = req.max_tokens {
        gc.insert("maxOutputTokens".into(), Value::from(mt));
    }
    if let Some(t) = req.temperature {
        gc.insert(
            "temperature".into(),
            serde_json::Number::from_f64(t)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
    }
    if let Some(tp) = req.top_p {
        gc.insert(
            "topP".into(),
            serde_json::Number::from_f64(tp)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
    }
    if let Some(k) = req.ext.top_k {
        gc.insert("topK".into(), Value::from(k));
    }
    if let Some(stop) = &req.stop {
        gc.insert(
            "stopSequences".into(),
            Value::Array(stop.iter().map(|s| Value::String(s.clone())).collect()),
        );
    }
    if let Some(rf) = &req.response_format {
        match rf["type"].as_str() {
            Some("json_object") => {
                gc.insert(
                    "responseMimeType".into(),
                    Value::String("application/json".into()),
                );
            }
            Some("json_schema") => {
                gc.insert(
                    "responseMimeType".into(),
                    Value::String("application/json".into()),
                );
                if let Some(s) = rf.get("json_schema") {
                    gc.insert("responseSchema".into(), s.clone());
                }
            }
            _ => {}
        }
    }
    if !gc.is_empty() {
        body.insert("generationConfig".into(), Value::Object(gc));
    }

    // tools
    if !req.tools.is_empty() {
        let fds: Vec<Value> = req.tools.iter().map(tool_to_json).collect();
        body.insert(
            "tools".into(),
            serde_json::json!([{"functionDeclarations": fds}]),
        );
    }

    Ok(Value::Object(body))
}

// ---------------------------------------------------------------------------
// 非流式响应：Gemini JSON → IR
// ---------------------------------------------------------------------------

fn parse_usage_metadata(u: &Value) -> IrUsage {
    let mut usage = IrUsage::default();
    usage.prompt_tokens = u["promptTokenCount"].as_u64().unwrap_or(0);
    usage.completion_tokens = u["candidatesTokenCount"].as_u64().unwrap_or(0);
    usage.cache_read_tokens = u["cachedContentTokenCount"].as_u64();
    usage.total_tokens = u["totalTokenCount"].as_u64();
    if let Some(t) = u["thoughtsTokenCount"].as_u64() {
        usage
            .extra
            .insert("thoughtsTokenCount".into(), Value::from(t));
    }
    // 其余未知字段原样保留
    if let Some(o) = u.as_object() {
        for (k, val) in o {
            if !matches!(
                k.as_str(),
                "promptTokenCount"
                    | "candidatesTokenCount"
                    | "cachedContentTokenCount"
                    | "totalTokenCount"
                    | "thoughtsTokenCount"
            ) {
                usage.extra.entry(k.clone()).or_insert_with(|| val.clone());
            }
        }
    }
    usage
}

/// 解析响应 candidate 的 content → IR 消息（role=assistant）。
fn parse_response_content(content: &Value, ctx: &mut ConvCtx) -> IrMessage {
    let mut msg = IrMessage::default();
    msg.role = IrRole::Assistant;
    let mut text_parts = Vec::new();
    let mut media_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut reasoning = None;
    let mut fc_seq = 0u32;

    if let Some(ps) = content.get("parts").and_then(|p| p.as_array()) {
        for p in ps {
            if let Some(t) = p["text"].as_str() {
                if p["thought"].as_bool().unwrap_or(false) {
                    reasoning = Some(t.to_string());
                } else {
                    text_parts.push(t.to_string());
                }
            } else if let Some(id) = p.get("inlineData") {
                media_parts.push(IrPart::ImageInline {
                    media_type: id["mimeType"].as_str().unwrap_or_default().to_string(),
                    data: id["data"].as_str().unwrap_or_default().to_string(),
                });
            } else if let Some(fc) = p.get("functionCall") {
                let name = fc["name"].as_str().unwrap_or_default().to_string();
                let arguments = args_to_string(&fc["args"]);
                let id = format!("{name}_{fc_seq}");
                fc_seq += 1;
                tool_calls.push(IrToolCall {
                    id,
                    name,
                    arguments,
                });
            } else {
                ctx.degrade("response.content.part", "未知响应 part 结构，丢弃");
            }
        }
    }

    let mut content_parts: Vec<IrPart> = Vec::new();
    for t in text_parts {
        content_parts.push(IrPart::Text { text: t });
    }
    content_parts.extend(media_parts);

    msg.content = if content_parts.len() == 1 {
        match &content_parts[0] {
            IrPart::Text { text } => Some(IrContent::Text(text.clone())),
            _ => Some(IrContent::Parts(content_parts)),
        }
    } else if !content_parts.is_empty() {
        Some(IrContent::Parts(content_parts))
    } else {
        None
    };
    msg.tool_calls = tool_calls;
    msg.reasoning_content = reasoning;
    msg
}

/// 非流式响应 JSON → IR。finishReason 归一化，原始值进 extra["gemini_finish_reason"]。
pub fn response_to_ir(v: &Value, ctx: &mut ConvCtx) -> Result<IrResponse, ConvertError> {
    let obj = v
        .as_object()
        .ok_or_else(|| ConvertError::Parse("响应体不是对象".into()))?;
    let mut resp = IrResponse::default();
    resp.model = String::new();

    if let Some(candidates) = v.get("candidates") {
        let arr = candidates
            .as_array()
            .ok_or_else(|| ConvertError::Parse("candidates 不是数组".into()))?;
        for (i, c) in arr.iter().enumerate() {
            let mut ch = IrChoice::default();
            ch.index = c["index"]
                .as_u64()
                .and_then(|x| u32::try_from(x).ok())
                .unwrap_or(i as u32);
            // 候选无 content 时容错（保留空 assistant 消息）
            if let Some(content) = c.get("content") {
                ch.message = parse_response_content(content, ctx);
            }
            if let Some(fr) = c["finishReason"].as_str() {
                ch.finish_reason = Some(normalize_finish_reason(fr));
                // 原始值进响应级 extra
                if i == 0 {
                    resp.extra
                        .insert("gemini_finish_reason".into(), Value::String(fr.to_string()));
                }
            }
            resp.choices.push(ch);
        }
    }

    if let Some(u) = v.get("usageMetadata") {
        resp.usage = Some(parse_usage_metadata(u));
    }

    // 其他顶层字段 → extra
    for (k, val) in obj {
        if !matches!(k.as_str(), "candidates" | "usageMetadata") {
            resp.extra.insert(k.clone(), val.clone());
        }
    }

    Ok(resp)
}

// ---------------------------------------------------------------------------
// 非流式响应：IR → Gemini JSON
// ---------------------------------------------------------------------------

/// finish_reason 反归一化为 Gemini 词汇。
/// Gemini 无 tool finish → 用 "STOP"（工具调用链以 STOP 终止，注释见下）。
fn reverse_finish_reason(fr: &str) -> String {
    match fr {
        "stop" => "STOP".to_string(),
        "length" => "MAX_TOKENS".to_string(),
        // Gemini 无 tool_calls finish 原因，统一以 STOP 标记内容结束
        "tool_calls" => "STOP".to_string(),
        "content_filter" => "SAFETY".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

fn usage_metadata_to_json(u: &IrUsage, ctx: &mut ConvCtx) -> Value {
    let mut out = u.extra.clone();
    out.insert("promptTokenCount".into(), Value::from(u.prompt_tokens));
    out.insert(
        "candidatesTokenCount".into(),
        Value::from(u.completion_tokens),
    );
    if let Some(t) = u.total_tokens {
        out.insert("totalTokenCount".into(), Value::from(t));
    }
    if let Some(c) = u.cache_read_tokens {
        out.insert("cachedContentTokenCount".into(), Value::from(c));
    }
    if u.cache_write_tokens.is_some() {
        ctx.degrade(
            "usage.cache_write_tokens",
            "Gemini 无 cache_write_tokens 对应字段",
        );
    }
    Value::Object(out)
}

/// 从 IR message 提取 candidate 的 content parts。
fn message_to_response_parts(m: &IrMessage, ctx: &mut ConvCtx) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    // 思考 → thought part
    if let Some(rc) = &m.reasoning_content {
        parts.push(serde_json::json!({"text": rc, "thought": true}));
    }
    if let Some(content) = &m.content {
        match content {
            IrContent::Text(s) => {
                if !s.is_empty() {
                    parts.push(serde_json::json!({"text": s}));
                }
            }
            IrContent::Parts(ps) => {
                for p in ps {
                    if let Some(pv) = part_to_json(p, ctx) {
                        parts.push(pv);
                    }
                }
            }
        }
    }
    for tc in &m.tool_calls {
        let args = args_string_to_value(&tc.arguments, "tool_call.arguments", ctx);
        parts.push(serde_json::json!({"functionCall": {"name": tc.name, "args": args}}));
    }
    parts
}

/// IR → 非流式响应 JSON（candidates[0].content、finishReason、usageMetadata）。
pub fn response_from_ir(resp: &IrResponse, ctx: &mut ConvCtx) -> Result<Value, ConvertError> {
    let mut body = resp.extra.clone();
    // 内部标记不写回响应
    body.remove("gemini_finish_reason");

    let mut candidates: Vec<Value> = Vec::new();
    for ch in &resp.choices {
        let msg = &ch.message;
        let parts = message_to_response_parts(msg, ctx);
        let mut content = Map::new();
        content.insert("role".into(), Value::String("model".into()));
        if !parts.is_empty() {
            content.insert("parts".into(), Value::Array(parts));
        }
        let mut cand = Map::new();
        cand.insert("content".into(), Value::Object(content));
        cand.insert("index".into(), Value::from(ch.index));
        if let Some(fr) = &ch.finish_reason {
            cand.insert(
                "finishReason".into(),
                Value::String(reverse_finish_reason(fr)),
            );
        }
        candidates.push(Value::Object(cand));
    }
    body.insert("candidates".into(), Value::Array(candidates));

    if let Some(u) = &resp.usage {
        body.insert("usageMetadata".into(), usage_metadata_to_json(u, ctx));
    }

    Ok(Value::Object(body))
}

// ---------------------------------------------------------------------------
// 流式 chunk
// ---------------------------------------------------------------------------

/// Gemini 流式状态。
/// 入站（chunk_to_ir）：分配工具调用序号；出站（chunk_from_ir）：聚合工具增量并在参数成合法 JSON 后才发。
#[derive(Debug, Default)]
pub struct StreamState {
    // 入站
    pub next_tool_index: u32,
    pub role_sent: bool,
    // 出站：工具调用聚合与发出跟踪
    pub tool_agg: ToolCallAggregator,
    pub tool_names: HashMap<u32, String>,
    pub tool_args: HashMap<u32, String>,
    pub tool_ids: HashMap<u32, String>,
    pub emitted_tools: HashSet<u32>,
}

fn delta_is_empty(d: &IrDelta) -> bool {
    d.role.is_none()
        && d.content.is_none()
        && d.reasoning_content.is_none()
        && d.tool_calls.is_empty()
}

/// SSE data 载荷 → IR chunk；无业务内容（空帧/无增量）返回 Ok(None)。
/// Gemini 流式 data 为完整 generateContent 响应 JSON（增量式）。
pub fn chunk_to_ir(
    data: &str,
    st: &mut StreamState,
    ctx: &mut ConvCtx,
) -> Result<Option<IrChunk>, ConvertError> {
    let v: Value = serde_json::from_str(data).map_err(|e| ConvertError::Parse(e.to_string()))?;
    let obj = v
        .as_object()
        .ok_or_else(|| ConvertError::Parse("chunk 不是对象".into()))?;
    let mut chunk = IrChunk::default();

    let mut delta = IrDelta::default();
    let mut finish_reason: Option<String> = None;

    if let Some(c) = v
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
    {
        // content + parts
        if let Some(content) = c.get("content") {
            if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                for p in parts {
                    if let Some(t) = p["text"].as_str() {
                        if p["thought"].as_bool().unwrap_or(false) {
                            delta.reasoning_content = Some(t.to_string());
                        } else {
                            // 文本增量：同一 chunk 多 text part 拼接
                            match &mut delta.content {
                                Some(existing) => existing.push_str(t),
                                None => delta.content = Some(t.to_string()),
                            }
                        }
                    } else if let Some(fc) = p.get("functionCall") {
                        let name = fc["name"].as_str().unwrap_or_default().to_string();
                        let arguments = args_to_string(&fc["args"]);
                        let idx = st.next_tool_index;
                        st.next_tool_index += 1;
                        delta.tool_calls.push(IrToolCallDelta {
                            index: idx,
                            id: Some(format!("{name}_{idx}")),
                            name: Some(name),
                            arguments: Some(arguments),
                        });
                    } else {
                        ctx.degrade("stream.content.part", "未知流式 part 结构，丢弃");
                    }
                }
            }
            // 首帧带角色
            if !st.role_sent {
                st.role_sent = true;
                if delta.role.is_none() {
                    delta.role = Some(IrRole::Assistant);
                }
            }
        }
        if let Some(fr) = c["finishReason"].as_str() {
            finish_reason = Some(normalize_finish_reason(fr));
            chunk
                .extra
                .insert("gemini_finish_reason".into(), Value::String(fr.to_string()));
        }
    }

    if let Some(u) = v.get("usageMetadata") {
        chunk.usage = Some(parse_usage_metadata(u));
    }

    // 无业务内容 → 跳过
    if finish_reason.is_none() && chunk.usage.is_none() && delta_is_empty(&delta) {
        return Ok(None);
    }

    let choice = IrChunkChoice {
        index: 0,
        delta,
        finish_reason,
    };
    chunk.choices.push(choice);

    // 其他顶层字段 → extra
    for (k, val) in obj {
        if !matches!(k.as_str(), "candidates" | "usageMetadata") {
            chunk.extra.insert(k.clone(), val.clone());
        }
    }

    Ok(Some(chunk))
}

/// 强制冲刷 st 中未完成的工具聚合为 functionCall part。
fn flush_unfinished_tools(st: &mut StreamState, parts: &mut Vec<Value>) {
    let idxs: Vec<u32> = st.tool_args.keys().copied().collect();
    for i in idxs {
        if st.emitted_tools.contains(&i) {
            continue;
        }
        if let Some(name) = st.tool_names.get(&i) {
            let args_val = st
                .tool_args
                .get(&i)
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or_else(json_object);
            parts.push(serde_json::json!({"functionCall": {"name": name, "args": args_val}}));
            st.emitted_tools.insert(i);
        }
    }
}

/// IR chunk → Gemini 流式响应 JSON 字符串（单条）。
pub fn chunk_from_ir(
    chunk: &IrChunk,
    st: &mut StreamState,
    ctx: &mut ConvCtx,
) -> Result<Vec<String>, ConvertError> {
    let mut delta = IrDelta::default();
    let mut finish_reason: Option<String> = None;
    if let Some(c) = chunk.choices.first() {
        delta = c.delta.clone();
        finish_reason = c.finish_reason.clone();
    }
    let usage = chunk.usage.clone();

    let mut parts: Vec<Value> = Vec::new();

    // 文本增量 → text part
    if let Some(text) = &delta.content {
        parts.push(serde_json::json!({"text": text}));
    }
    // 思考增量 → thought part
    if let Some(rc) = &delta.reasoning_content {
        parts.push(serde_json::json!({"text": rc, "thought": true}));
    }

    // 工具增量：累积，参数成合法 JSON 后才发 functionCall part
    for tc in &delta.tool_calls {
        st.tool_agg.feed(tc);
        let i = tc.index;
        if let Some(name) = &tc.name {
            st.tool_names.insert(i, name.clone());
        }
        if let Some(a) = &tc.arguments {
            st.tool_args.entry(i).or_default().push_str(a);
        }
        if let Some(id) = &tc.id {
            st.tool_ids.entry(i).or_insert_with(|| id.clone());
        }
        if !st.emitted_tools.contains(&i) {
            if let (Some(name), Some(args_str)) = (st.tool_names.get(&i), st.tool_args.get(&i)) {
                if let Ok(args_val) = serde_json::from_str::<Value>(args_str) {
                    parts.push(
                        serde_json::json!({"functionCall": {"name": name, "args": args_val}}),
                    );
                    st.emitted_tools.insert(i);
                }
            }
        }
    }

    // finish_reason → 强制冲刷未完成的聚合
    if finish_reason.is_some() {
        flush_unfinished_tools(st, &mut parts);
    }

    let mut content = Map::new();
    content.insert("role".into(), Value::String("model".into()));
    if !parts.is_empty() {
        content.insert("parts".into(), Value::Array(parts));
    }

    let mut cand = Map::new();
    cand.insert("content".into(), Value::Object(content));
    cand.insert("index".into(), Value::from(0));
    if let Some(fr) = &finish_reason {
        cand.insert(
            "finishReason".into(),
            Value::String(reverse_finish_reason(fr)),
        );
    }

    let mut body = Map::new();
    body.insert(
        "candidates".into(),
        serde_json::json!([Value::Object(cand)]),
    );
    if let Some(u) = &usage {
        body.insert("usageMetadata".into(), usage_metadata_to_json(u, ctx));
    }
    // 其他 chunk 顶层字段（过滤内部标记）
    for (k, val) in &chunk.extra {
        if k != "gemini_finish_reason" {
            body.entry(k.clone()).or_insert_with(|| val.clone());
        }
    }

    let s = serde_json::to_string(&Value::Object(body))
        .map_err(|e| ConvertError::Parse(e.to_string()))?;
    Ok(vec![s])
}

/// Gemini 流无显式终止事件 → 返回空 vec，但先冲刷 st 中未完成工具调用（作为最后帧发出）。
pub fn stream_end(st: &mut StreamState, _ctx: &mut ConvCtx) -> Result<Vec<String>, ConvertError> {
    let mut parts: Vec<Value> = Vec::new();
    flush_unfinished_tools(st, &mut parts);
    if parts.is_empty() {
        return Ok(vec![]);
    }
    let mut content = Map::new();
    content.insert("role".into(), Value::String("model".into()));
    content.insert("parts".into(), Value::Array(parts));
    let mut cand = Map::new();
    cand.insert("content".into(), Value::Object(content));
    cand.insert("index".into(), Value::from(0));
    let body = serde_json::json!({"candidates": [Value::Object(cand)]});
    let s = serde_json::to_string(&body).map_err(|e| ConvertError::Parse(e.to_string()))?;
    Ok(vec![s])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ConvCtx {
        ConvCtx::new()
    }

    // ---------- 文本对话 ----------
    #[test]
    fn text_request_roundtrip() {
        let j = serde_json::json!({
            "systemInstruction": {"parts": [{"text": "You are helpful."}]},
            "contents": [
                {"role": "user", "parts": [{"text": "Hello"}]},
                {"role": "model", "parts": [{"text": "Hi there"}]}
            ],
            "generationConfig": {"maxOutputTokens": 100, "temperature": 0.7, "topP": 0.9}
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.model, "", "model 留空由上层注入");
        assert_eq!(req.messages.len(), 3);
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
        assert_eq!(req.messages[2].role, IrRole::Assistant);
        assert_eq!(
            req.messages[2].content,
            Some(IrContent::Text("Hi there".into()))
        );
        assert_eq!(req.max_tokens, Some(100));
        assert_eq!(req.temperature, Some(0.7));
        assert_eq!(req.top_p, Some(0.9));

        let back = request_from_ir(&req, &mut c).unwrap();
        assert!(back.get("model").is_none(), "model 不写入 body");
        assert_eq!(
            back["systemInstruction"]["parts"][0]["text"],
            "You are helpful."
        );
        assert_eq!(back["contents"][0]["role"], "user");
        assert_eq!(back["contents"][0]["parts"][0]["text"], "Hello");
        assert_eq!(back["contents"][1]["role"], "model");
        assert_eq!(back["contents"][1]["parts"][0]["text"], "Hi there");
        assert_eq!(back["generationConfig"]["maxOutputTokens"], 100);
        assert_eq!(back["generationConfig"]["temperature"], 0.7);
        assert_eq!(back["generationConfig"]["topP"], 0.9);
    }

    // ---------- inline_data 图片 ----------
    #[test]
    fn inline_data_image_roundtrip() {
        let j = serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": [
                    {"text": "what is this?"},
                    {"inlineData": {"mimeType": "image/png", "data": "AAAABBBB"}}
                ]
            }]
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        let parts = match &req.messages[0].content {
            Some(IrContent::Parts(p)) => p,
            other => panic!("应为 Parts: {other:?}"),
        };
        assert_eq!(parts.len(), 2);
        assert_eq!(
            parts[1],
            IrPart::ImageInline {
                media_type: "image/png".into(),
                data: "AAAABBBB".into()
            }
        );

        let back = request_from_ir(&req, &mut c).unwrap();
        assert_eq!(
            back["contents"][0]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
        assert_eq!(
            back["contents"][0]["parts"][1]["inlineData"]["data"],
            "AAAABBBB"
        );
    }

    // ---------- functionDeclarations + functionCall + functionResponse ----------
    #[test]
    fn tools_definitions_calls_and_results() {
        let j = serde_json::json!({
            "contents": [
                {"role": "user", "parts": [{"text": "weather in bj?"}]},
                {"role": "model", "parts": [{"functionCall": {"name": "get_weather", "args": {"city": "bj"}}}]},
                {"role": "user", "parts": [{"functionResponse": {"name": "get_weather", "response": {"temperature": 25}}}]}
            ],
            "tools": [{
                "functionDeclarations": [{
                    "name": "get_weather",
                    "description": "get weather",
                    "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
                }]
            }]
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();

        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].name, "get_weather");
        assert_eq!(req.tools[0].parameters["type"], "object");

        // functionCall → assistant tool_calls
        assert_eq!(req.messages[1].role, IrRole::Assistant);
        assert_eq!(req.messages[1].tool_calls.len(), 1);
        assert_eq!(req.messages[1].tool_calls[0].name, "get_weather");
        assert_eq!(req.messages[1].tool_calls[0].arguments, "{\"city\":\"bj\"}");
        assert_eq!(req.messages[1].tool_calls[0].id, "get_weather_0");

        // functionResponse → Tool 消息
        assert_eq!(req.messages[2].role, IrRole::Tool);
        assert_eq!(req.messages[2].tool_call_id.as_deref(), Some("get_weather"));
        assert_eq!(
            req.messages[2].content,
            Some(IrContent::Text("{\"temperature\":25}".into()))
        );

        // IR → Gemini
        let back = request_from_ir(&req, &mut c).unwrap();
        assert_eq!(
            back["tools"][0]["functionDeclarations"][0]["name"],
            "get_weather"
        );
        assert_eq!(back["contents"][0]["parts"][0]["text"], "weather in bj?");
        assert_eq!(back["contents"][1]["role"], "model");
        assert_eq!(
            back["contents"][1]["parts"][0]["functionCall"]["name"],
            "get_weather"
        );
        assert_eq!(
            back["contents"][1]["parts"][0]["functionCall"]["args"]["city"],
            "bj"
        );
        assert_eq!(back["contents"][2]["role"], "user");
        assert_eq!(
            back["contents"][2]["parts"][0]["functionResponse"]["name"],
            "get_weather"
        );
        assert_eq!(
            back["contents"][2]["parts"][0]["functionResponse"]["response"]["temperature"],
            25
        );
    }

    // ---------- generationConfig 全字段映射（含 topK 进 ext） ----------
    #[test]
    fn generation_config_full_mapping() {
        let j = serde_json::json!({
            "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
            "generationConfig": {
                "maxOutputTokens": 256,
                "temperature": 0.5,
                "topP": 0.95,
                "topK": 40,
                "stopSequences": ["END", "STOP"],
                "responseMimeType": "application/json"
            }
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.max_tokens, Some(256));
        assert_eq!(req.temperature, Some(0.5));
        assert_eq!(req.top_p, Some(0.95));
        assert_eq!(req.ext.top_k, Some(40));
        assert_eq!(req.stop, Some(vec!["END".to_string(), "STOP".to_string()]));
        assert_eq!(
            req.response_format,
            Some(serde_json::json!({"type": "json_object"}))
        );

        let back = request_from_ir(&req, &mut c).unwrap();
        assert_eq!(back["generationConfig"]["maxOutputTokens"], 256);
        assert_eq!(back["generationConfig"]["temperature"], 0.5);
        assert_eq!(back["generationConfig"]["topP"], 0.95);
        assert_eq!(back["generationConfig"]["topK"], 40);
        assert_eq!(
            back["generationConfig"]["stopSequences"],
            serde_json::json!(["END", "STOP"])
        );
        assert_eq!(
            back["generationConfig"]["responseMimeType"],
            "application/json"
        );
    }

    // ---------- usageMetadata 提取（含 cached） ----------
    #[test]
    fn usage_metadata_extraction() {
        let j = serde_json::json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [
                        {"text": "hi there"},
                        {"thought": true, "text": "thinking..."}
                    ]
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "cachedContentTokenCount": 3,
                "thoughtsTokenCount": 2,
                "totalTokenCount": 15
            }
        });
        let mut c = ctx();
        let resp = response_to_ir(&j, &mut c).unwrap();
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(
            resp.choices[0].message.content,
            Some(IrContent::Text("hi there".into()))
        );
        assert_eq!(
            resp.choices[0].message.reasoning_content.as_deref(),
            Some("thinking...")
        );
        assert_eq!(resp.extra["gemini_finish_reason"], "STOP");

        let u = resp.usage.as_ref().unwrap();
        assert_eq!(u.prompt_tokens, 10);
        assert_eq!(u.completion_tokens, 5);
        assert_eq!(u.cache_read_tokens, Some(3));
        assert_eq!(u.total_tokens, Some(15));
        assert_eq!(u.extra["thoughtsTokenCount"], 2);

        // IR → Gemini 响应
        let back = response_from_ir(&resp, &mut c).unwrap();
        assert_eq!(back["candidates"][0]["content"]["role"], "model");
        // 思考 part 在正文之前
        assert_eq!(
            back["candidates"][0]["content"]["parts"][0]["thought"],
            true
        );
        assert_eq!(
            back["candidates"][0]["content"]["parts"][0]["text"],
            "thinking..."
        );
        assert_eq!(
            back["candidates"][0]["content"]["parts"][1]["text"],
            "hi there"
        );
        assert_eq!(back["candidates"][0]["finishReason"], "STOP");
        assert_eq!(back["usageMetadata"]["promptTokenCount"], 10);
        assert_eq!(back["usageMetadata"]["cachedContentTokenCount"], 3);
        assert_eq!(back["usageMetadata"]["thoughtsTokenCount"], 2);
    }

    // ---------- 流式文本增量 ----------
    #[test]
    fn streaming_text_increments() {
        let mut c = ctx();
        let mut st = StreamState::default();

        let d1 = serde_json::json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "Hello"}]}}]});
        let c1 = chunk_to_ir(&serde_json::to_string(&d1).unwrap(), &mut st, &mut c)
            .unwrap()
            .unwrap();
        assert_eq!(c1.choices[0].delta.role, Some(IrRole::Assistant));
        assert_eq!(c1.choices[0].delta.content.as_deref(), Some("Hello"));

        let d2 = serde_json::json!({"candidates": [{"content": {"role": "model", "parts": [{"text": " world"}]}}]});
        let c2 = chunk_to_ir(&serde_json::to_string(&d2).unwrap(), &mut st, &mut c)
            .unwrap()
            .unwrap();
        assert_eq!(c2.choices[0].delta.content.as_deref(), Some(" world"));

        // IR chunk → Gemini 流帧
        let out = chunk_from_ir(&c1, &mut st, &mut c).unwrap();
        let v: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(v["candidates"][0]["content"]["role"], "model");
        assert_eq!(v["candidates"][0]["content"]["parts"][0]["text"], "Hello");
    }

    // ---------- 流式工具调用聚合、冲刷、finish ----------
    #[test]
    fn streaming_tool_call_aggregation_and_flush() {
        let mut c = ctx();
        let mut st = StreamState::default();

        // 部分参数增量
        let d1 = serde_json::json!({"candidates": [{"content": {"role": "model", "parts": [
            {"functionCall": {"name": "get_weather", "args": {"city": "bj"}}}
        ]}}]});
        let c1 = chunk_to_ir(&serde_json::to_string(&d1).unwrap(), &mut st, &mut c)
            .unwrap()
            .unwrap();
        let tc = &c1.choices[0].delta.tool_calls[0];
        assert_eq!(tc.index, 0);
        assert_eq!(tc.name.as_deref(), Some("get_weather"));
        assert_eq!(tc.arguments.as_deref(), Some("{\"city\":\"bj\"}"));

        // 出站：参数分两片，首片不合法不发送
        let mut st2 = StreamState::default();
        let frag1 = serde_json::json!({
            "choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_1", "name": "get_weather", "arguments": "{\"ci"}]}, "finish_reason": null}]
        });
        let chunk1 = serde_json::from_value::<IrChunk>(frag1).unwrap();
        let out1 = chunk_from_ir(&chunk1, &mut st2, &mut c).unwrap();
        // 参数未成合法 JSON → 该帧不携带 functionCall part
        if out1[0].contains("functionCall") {
            // 极端：若 name 已有但 args 未完成，不应发
            panic!("参数未成合法 JSON，不应发出 functionCall");
        }

        // 第二片补齐参数 → 发出
        let frag2 = serde_json::json!({
            "choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "arguments": "ty\":\"bj\"}"}]}, "finish_reason": null}]
        });
        let chunk2 = serde_json::from_value::<IrChunk>(frag2).unwrap();
        let out2 = chunk_from_ir(&chunk2, &mut st2, &mut c).unwrap();
        let v2: Value = serde_json::from_str(&out2[0]).unwrap();
        assert_eq!(
            v2["candidates"][0]["content"]["parts"][0]["functionCall"]["name"],
            "get_weather"
        );
        assert_eq!(
            v2["candidates"][0]["content"]["parts"][0]["functionCall"]["args"]["city"],
            "bj"
        );

        // finish_reason 强制冲刷未完成聚合
        let frag3 = serde_json::json!({
            "choices": [{"index": 0, "delta": {"tool_calls": [{"index": 1, "name": "other_tool", "arguments": "{\"x\""}]}, "finish_reason": "tool_calls"}]
        });
        let chunk3 = serde_json::from_value::<IrChunk>(frag3).unwrap();
        let out3 = chunk_from_ir(&chunk3, &mut st2, &mut c).unwrap();
        let v3: Value = serde_json::from_str(&out3[0]).unwrap();
        assert_eq!(v3["candidates"][0]["finishReason"], "STOP");
        // 未完成聚合被冲刷为一个 functionCall part
        assert_eq!(
            v3["candidates"][0]["content"]["parts"][0]["functionCall"]["name"],
            "other_tool"
        );

        // stream_end：无未完成聚合 → 空
        let mut empty_st = StreamState::default();
        assert_eq!(
            stream_end(&mut empty_st, &mut c).unwrap(),
            Vec::<String>::new()
        );
    }

    // ---------- URL 图片降级记录 ----------
    #[test]
    fn url_image_degraded() {
        let req = IrRequest {
            model: "m".into(),
            messages: vec![IrMessage {
                role: IrRole::User,
                content: Some(IrContent::Parts(vec![IrPart::ImageUrl {
                    url: "https://example.com/a.png".into(),
                }])),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut c = ctx();
        let back = request_from_ir(&req, &mut c).unwrap();
        // 以 fileData(fileUri) 携带
        assert_eq!(
            back["contents"][0]["parts"][0]["fileData"]["fileUri"],
            "https://example.com/a.png"
        );
        // 记录降级
        assert!(c.degraded.iter().any(|d| d.field == "image_url"));
    }
}
