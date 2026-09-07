#![allow(clippy::field_reassign_with_default)]

//! OpenAI Chat Completions 适配器（/v1/chat/completions）。
//!
//! 这是 IR 的枢轴协议，转换最接近恒等：字段名归一化（max_completion_tokens→max_tokens，
//! 原始字段名记入 IR 供回写保真）、未知字段进 extra 原样携带、usage 提取
//! （含 cache 字段 prompt_tokens_details）。
//! 流式：chat.completion.chunk 逐条映射；终止载荷为 "[DONE]"。
//!
//! 无法映射能力处理（PLAN.md §4.3）：思考/联网搜索等 OpenRouter / Anthropic 扩展字段（ext）
//! 在 OpenAI Chat 无对应字段 → 记录 ctx.degrade。

use serde_json::{Map, Value};

use super::ir::*;
use super::{ConvCtx, ConvertError};

/// 请求体中被本适配器识别并映射的顶层字段；其余进 `extra`。
const KNOWN_REQUEST_FIELDS: [&str; 15] = [
    "model",
    "messages",
    "tools",
    "tool_choice",
    "temperature",
    "top_p",
    "max_tokens",
    "max_completion_tokens",
    "stop",
    "stream",
    "stream_options",
    "response_format",
    "seed",
    "user",
    "n",
];

/// 响应体顶层规范字段；其余进 `extra`。
const KNOWN_RESPONSE_FIELDS: [&str; 6] = [
    "id",
    "model",
    "created",
    "choices",
    "usage",
    "system_fingerprint",
];

/// 流式 chunk 顶层规范字段；其余进 `extra`。
const KNOWN_CHUNK_FIELDS: [&str; 5] = ["id", "model", "created", "choices", "usage"];

// ---------------------------------------------------------------------------
// 请求：OpenAI JSON → IR
// ---------------------------------------------------------------------------

/// 解析角色字符串为 IR 角色；未知角色记降级，缺省按 User 容错。
fn parse_role(role: Option<&str>, ctx: &mut ConvCtx) -> IrRole {
    match role {
        // developer 为 o 系/gpt-5 的系统消息角色（review P5：此前按 user 降级）
        Some("system") | Some("developer") => IrRole::System,
        Some("assistant") => IrRole::Assistant,
        Some("tool") => IrRole::Tool,
        Some(other) => {
            ctx.degrade("message.role", format!("未知角色，按 user 处理: {other}"));
            IrRole::User
        }
        // user 或缺失均默认 User
        _ => IrRole::User,
    }
}

/// 解析 `data:...` URL 片段为 (media_type, base64 data)。
fn split_data_url(rest: &str) -> (String, String) {
    // rest 形如 `<media_type>;base64,<data>`，也可能缺分号/逗号。
    if let Some(semi) = rest.find(';') {
        let media = rest[..semi].to_string();
        let after = &rest[semi + 1..];
        if let Some(comma) = after.find(',') {
            (media, after[comma + 1..].to_string())
        } else {
            (media, after.to_string())
        }
    } else if let Some(comma) = rest.find(',') {
        (media_default(), rest[comma + 1..].to_string())
    } else {
        (media_default(), rest.to_string())
    }
}

fn media_default() -> String {
    "application/octet-stream".into()
}

/// 解析 image_url 载荷（字符串 URL 或 `{url}` 对象）为 IR part。
/// 含 base64 `data:` URL → ImageInline，否则 ImageUrl。
fn parse_image_url(url_val: &Value, ctx: &mut ConvCtx) -> Option<IrPart> {
    let url = if let Some(s) = url_val.as_str() {
        s.to_string()
    } else if let Some(o) = url_val.as_object() {
        // detail（low/high/auto）无 IR 槽位，记降级（review P5：此前静默丢失）
        if o.get("detail").is_some_and(|d| !d.is_null()) {
            ctx.degrade("content.image_url.detail", "image detail 不支持，丢弃");
        }
        o.get("url")
            .and_then(|u| u.as_str())
            .map(String::from)
            .unwrap_or_default()
    } else {
        ctx.degrade("content.image_url", "image_url 既非字符串也非对象");
        return None;
    };
    if url.is_empty() {
        ctx.degrade("content.image_url", "image_url 缺 url");
        return None;
    }
    if let Some(rest) = url.strip_prefix("data:") {
        let (media_type, data) = split_data_url(rest);
        Some(IrPart::ImageInline { media_type, data })
    } else {
        Some(IrPart::ImageUrl { url })
    }
}

/// 解析 file part。
fn parse_file(f: &Value, ctx: &mut ConvCtx) -> Option<IrPart> {
    if !f.is_object() {
        ctx.degrade("content.file", "file part 非对象");
        return None;
    }
    let name = f["file_name"]
        .as_str()
        .or_else(|| f["filename"].as_str())
        .or_else(|| f["file_id"].as_str())
        .unwrap_or_default()
        .to_string();
    let url = f["file_url"]
        .as_str()
        .or_else(|| f["url"].as_str())
        .map(String::from);
    let data = f["file_data"]
        .as_str()
        .or_else(|| f["data"].as_str())
        .map(String::from);
    if name.is_empty() && url.is_none() && data.is_none() {
        ctx.degrade("content.file", "file part 无内容");
        return None;
    }
    Some(IrPart::File { name, url, data })
}

/// 解析单个 content part。
fn parse_part(p: &Value, ctx: &mut ConvCtx) -> Option<IrPart> {
    if !p.is_object() {
        ctx.degrade("content.part", "content part 非对象");
        return None;
    }
    match p["type"].as_str().unwrap_or_default() {
        "text" => Some(IrPart::Text {
            text: p["text"].as_str().unwrap_or_default().to_string(),
        }),
        "image_url" => parse_image_url(&p["image_url"], ctx),
        "input_audio" => {
            let audio = &p["input_audio"];
            Some(IrPart::InputAudio {
                data: audio["data"].as_str().unwrap_or_default().to_string(),
                format: audio["format"].as_str().unwrap_or_default().to_string(),
            })
        }
        "file" => parse_file(&p["file"], ctx),
        other => {
            ctx.degrade("content.part.type", format!("未知 part 类型: {other}"));
            None
        }
    }
}

/// 解析消息 content（字符串 → Text；数组 → Parts；null/缺失 → None）。
fn parse_content(v: Option<&Value>, ctx: &mut ConvCtx) -> Option<IrContent> {
    let v = v?;
    if v.is_null() {
        return None;
    }
    if let Some(s) = v.as_str() {
        return Some(IrContent::Text(s.to_string()));
    }
    if let Some(arr) = v.as_array() {
        let mut parts = Vec::new();
        for p in arr {
            if let Some(part) = parse_part(p, ctx) {
                parts.push(part);
            }
        }
        if parts.is_empty() {
            return None;
        }
        return Some(IrContent::Parts(parts));
    }
    ctx.degrade("message.content", "未知内容结构，丢弃");
    None
}

/// 解析工具调用（arguments 若为对象则序列化为 JSON 字符串）。
fn parse_tool_call(tc: &Value, ctx: &mut ConvCtx) -> Option<IrToolCall> {
    if !tc.is_object() {
        ctx.degrade("tool_calls", "tool_call 非对象");
        return None;
    }
    let id = tc["id"].as_str().unwrap_or_default().to_string();
    let name = tc["function"]["name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let args = &tc["function"]["arguments"];
    let arguments = if args.is_string() {
        args.as_str().unwrap_or_default().to_string()
    } else if args.is_null() {
        String::new()
    } else {
        // \u976e\u5b57\u7b26\u4e32\u53c2\u6570\u5e8f\u5217\u5316\u4e3a\u5b57\u7b26\u4e32
        serde_json::to_string(args).unwrap_or_default()
    };
    Some(IrToolCall {
        id,
        name,
        arguments,
    })
}

/// 解析一条消息为 IR 消息。结构错误（非对象）才报 Parse。
fn parse_message(v: &Value, ctx: &mut ConvCtx) -> Result<IrMessage, ConvertError> {
    if !v.is_object() {
        return Err(ConvertError::Parse("message 不是对象".into()));
    }
    let mut msg = IrMessage::default();
    msg.role = parse_role(v["role"].as_str(), ctx);
    msg.name = v["name"].as_str().map(String::from);
    msg.content = parse_content(v.get("content"), ctx);
    msg.tool_call_id = v["tool_call_id"].as_str().map(String::from);
    msg.reasoning_content = v["reasoning_content"].as_str().map(String::from);
    if let Some(tcs) = v.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in tcs {
            if let Some(call) = parse_tool_call(tc, ctx) {
                msg.tool_calls.push(call);
            }
        }
    }
    // message 级已知但无 IR 槽位的字段（few-shot 历史中可能出现）：记降级而非静默丢弃
    // （review P5：refusal 是安全拒绝的唯一载体，annotations 携带 URL 引用）。
    for k in ["refusal", "annotations", "audio", "function_call"] {
        if v.get(k).is_some_and(|x| !x.is_null()) {
            ctx.degrade(
                format!("message.{k}"),
                format!("message 级字段 {k} 无 IR 槽位，丢弃"),
            );
        }
    }
    Ok(msg)
}

fn parse_tools(v: &Value, ctx: &mut ConvCtx) -> Result<Vec<IrTool>, ConvertError> {
    let arr = v
        .as_array()
        .ok_or_else(|| ConvertError::Parse("tools 不是数组".into()))?;
    let mut out = Vec::new();
    for t in arr {
        // function 类型 → 归一化；其余类型（如 web_search、code_interpreter）→ 降级丢弃
        if t["type"].as_str().unwrap_or_default() == "function" {
            let f = &t["function"];
            out.push(IrTool {
                name: f["name"].as_str().unwrap_or_default().to_string(),
                description: f["description"].as_str().map(String::from),
                parameters: f.get("parameters").cloned().unwrap_or_else(json_object),
            });
        } else {
            ctx.degrade(
                "tools",
                format!(
                    "非 function 类型工具定义，丢弃: {}",
                    t["type"].as_str().unwrap_or_default()
                ),
            );
        }
    }
    Ok(out)
}

fn json_object() -> Value {
    Value::Object(Map::new())
}

/// 请求 JSON → IR。
pub fn request_to_ir(v: &Value, ctx: &mut ConvCtx) -> Result<IrRequest, ConvertError> {
    let obj = v
        .as_object()
        .ok_or_else(|| ConvertError::Parse("请求体不是对象".into()))?;
    let mut req = IrRequest::default();
    req.model = v["model"].as_str().unwrap_or_default().to_string();

    if let Some(msgs) = v.get("messages") {
        let arr = msgs
            .as_array()
            .ok_or_else(|| ConvertError::Parse("messages 不是数组".into()))?;
        for m in arr {
            req.messages.push(parse_message(m, ctx)?);
        }
    }

    // max_tokens / max_completion_tokens 统一进 max_tokens，并记录入口原始字段名
    // （round-trip 保真：用户用什么字段就回写什么字段——o 系/gpt-5 只认
    // max_completion_tokens，部分第三方服务商只认 max_tokens）。
    let has_mt = v.get("max_tokens").is_some_and(|x| !x.is_null());
    let has_mct = v.get("max_completion_tokens").is_some_and(|x| !x.is_null());
    if has_mt && has_mct {
        // OpenAI 规范不允许两者并存；取 max_completion_tokens 值并记降级
        ctx.degrade(
            "max_tokens",
            "max_tokens 与 max_completion_tokens 并存，取 max_completion_tokens",
        );
    }
    let mt_val = if has_mct {
        v.get("max_completion_tokens")
    } else if has_mt {
        v.get("max_tokens")
    } else {
        None
    };
    if let Some(n) = mt_val.and_then(|x| x.as_u64()) {
        req.max_tokens = Some(n);
    }
    req.max_tokens_field = if has_mct {
        Some(MaxTokensField::MaxCompletionTokens)
    } else if has_mt {
        Some(MaxTokensField::MaxTokens)
    } else {
        None
    };

    // stop：字符串或数组统一为 Vec<String>
    if let Some(stop) = v.get("stop") {
        if let Some(s) = stop.as_str() {
            req.stop = Some(vec![s.to_string()]);
        } else if let Some(arr) = stop.as_array() {
            req.stop = Some(
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect(),
            );
        }
    }

    req.stream = v["stream"].as_bool().unwrap_or(false);
    if let Some(so) = v.get("stream_options").and_then(|s| s.as_object()) {
        req.stream_include_usage = so
            .get("include_usage")
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
    }

    if let Some(tools) = v.get("tools") {
        req.tools = parse_tools(tools, ctx)?;
    }

    // 以下字段原样携带
    req.tool_choice = v.get("tool_choice").cloned();
    req.response_format = v.get("response_format").cloned();
    req.temperature = v["temperature"].as_f64();
    req.top_p = v["top_p"].as_f64();
    req.seed = v["seed"].as_i64();
    req.user = v["user"].as_str().map(String::from);
    req.n = v["n"].as_u64().and_then(|n| u32::try_from(n).ok());

    // 未知字段进 extra
    for (k, val) in obj {
        if !KNOWN_REQUEST_FIELDS.contains(&k.as_str()) {
            req.extra.insert(k.clone(), val.clone());
        }
    }

    Ok(req)
}

// ---------------------------------------------------------------------------
// 请求：IR → OpenAI JSON
// ---------------------------------------------------------------------------

fn role_to_str(r: IrRole) -> &'static str {
    match r {
        IrRole::System => "system",
        IrRole::User => "user",
        IrRole::Assistant => "assistant",
        IrRole::Tool => "tool",
    }
}

fn tool_to_json(t: &IrTool) -> Value {
    // OpenAI 工具定义外层为 {type:"function", function:{name,description,parameters}}
    let mut f = Map::new();
    f.insert("name".into(), Value::String(t.name.clone()));
    if let Some(d) = &t.description {
        f.insert("description".into(), Value::String(d.clone()));
    }
    f.insert("parameters".into(), t.parameters.clone());
    let mut tool = Map::new();
    tool.insert("function".into(), Value::Object(f));
    json_object_with("type", "function", tool)
}

fn json_object_with(tag: &str, tag_val: &str, mut fields: Map<String, Value>) -> Value {
    fields.insert(tag.to_string(), Value::String(tag_val.to_string()));
    Value::Object(fields)
}

fn tool_call_to_json(tc: &IrToolCall) -> Value {
    let mut f = Map::new();
    f.insert("name".into(), Value::String(tc.name.clone()));
    f.insert("arguments".into(), Value::String(tc.arguments.clone()));
    let mut c = Map::new();
    c.insert("id".into(), Value::String(tc.id.clone()));
    c.insert("function".into(), Value::Object(f));
    json_object_with("type", "function", c)
}

fn part_to_json(p: &IrPart) -> Value {
    match p {
        IrPart::Text { text } => {
            let mut m = Map::new();
            m.insert("text".into(), Value::String(text.clone()));
            json_object_with("type", "text", m)
        }
        IrPart::ImageUrl { url } => {
            let mut inner = Map::new();
            inner.insert("url".into(), Value::String(url.clone()));
            let mut m = Map::new();
            m.insert("image_url".into(), Value::Object(inner));
            json_object_with("type", "image_url", m)
        }
        IrPart::ImageInline { media_type, data } => {
            // base64 内联 → 拼回 data: URL
            let mut m = Map::new();
            m.insert(
                "image_url".into(),
                Value::String(format!("data:{media_type};base64,{data}")),
            );
            json_object_with("type", "image_url", m)
        }
        IrPart::InputAudio { data, format } => {
            let mut inner = Map::new();
            inner.insert("data".into(), Value::String(data.clone()));
            inner.insert("format".into(), Value::String(format.clone()));
            let mut m = Map::new();
            m.insert("input_audio".into(), Value::Object(inner));
            json_object_with("type", "input_audio", m)
        }
        IrPart::File { name, url, data } => {
            let mut f = Map::new();
            if !name.is_empty() {
                f.insert("file_name".into(), Value::String(name.clone()));
            }
            if let Some(u) = url {
                f.insert("file_url".into(), Value::String(u.clone()));
            }
            if let Some(d) = data {
                f.insert("file_data".into(), Value::String(d.clone()));
            }
            let mut m = Map::new();
            m.insert("file".into(), Value::Object(f));
            json_object_with("type", "file", m)
        }
    }
}

fn content_to_json(content: &IrContent) -> Value {
    match content {
        IrContent::Text(s) => Value::String(s.clone()),
        IrContent::Parts(parts) => Value::Array(parts.iter().map(part_to_json).collect()),
    }
}

fn message_to_json(msg: &IrMessage) -> Value {
    let mut m = Map::new();
    m.insert("role".into(), Value::String(role_to_str(msg.role).into()));
    if let Some(content) = &msg.content {
        m.insert("content".into(), content_to_json(content));
    }
    if let Some(name) = &msg.name {
        m.insert("name".into(), Value::String(name.clone()));
    }
    if !msg.tool_calls.is_empty() {
        m.insert(
            "tool_calls".into(),
            Value::Array(msg.tool_calls.iter().map(tool_call_to_json).collect()),
        );
    }
    if let Some(tcid) = &msg.tool_call_id {
        m.insert("tool_call_id".into(), Value::String(tcid.clone()));
    }
    if let Some(rc) = &msg.reasoning_content {
        m.insert("reasoning_content".into(), Value::String(rc.clone()));
    }
    Value::Object(m)
}

/// 记录 ext 字段在 OpenAI Chat 无对应 → 降级。
fn degrade_ext(req: &IrRequest, ctx: &mut ConvCtx) {
    if req.ext.thinking.is_some() {
        ctx.degrade("ext.thinking", "OpenAI Chat 无对应字段");
    }
    if req.ext.web_search.is_some() {
        ctx.degrade("ext.web_search", "OpenAI Chat 无对应字段");
    }
    if req.ext.cache_control.is_some() {
        ctx.degrade("ext.cache_control", "OpenAI Chat 无对应字段");
    }
    if req.ext.top_k.is_some() {
        ctx.degrade("ext.top_k", "OpenAI Chat 无对应字段");
    }
    if req.ext.service_tier.is_some() {
        ctx.degrade("ext.service_tier", "OpenAI Chat 无对应字段");
    }
    for k in req.ext.extra.keys() {
        ctx.degrade(k.clone(), "OpenAI Chat 无对应字段");
    }
}

/// IR → 请求 JSON。extra 先铺底，再写规范字段（规范字段覆盖 extra 同名字段）。
pub fn request_from_ir(req: &IrRequest, ctx: &mut ConvCtx) -> Result<Value, ConvertError> {
    let mut body = req.extra.clone();
    degrade_ext(req, ctx);

    body.insert("model".into(), Value::String(req.model.clone()));
    body.insert(
        "messages".into(),
        Value::Array(req.messages.iter().map(message_to_json).collect()),
    );

    if !req.tools.is_empty() {
        body.insert(
            "tools".into(),
            Value::Array(req.tools.iter().map(tool_to_json).collect()),
        );
    }
    if let Some(tc) = &req.tool_choice {
        // 跨协议形状归一（review P5）：Responses 的 {"type":"function","name":"x"}
        // 转 Chat 需包装为 {"type":"function","function":{"name":"x"}}
        let tc = match tc {
            Value::Object(o)
                if o.get("type").and_then(|t| t.as_str()) == Some("function")
                    && o.get("function").is_none()
                    && o.get("name").is_some() =>
            {
                serde_json::json!({
                    "type": "function",
                    "function": {"name": o.get("name").cloned().unwrap_or_default()},
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
        // 字段名回写策略：Chat 入口记录了原字段名 → 原样回写（round-trip 保真）；
        // 跨协议转换（无原字段名）→ 默认 max_tokens（第三方 OpenAI 兼容服务商
        // 普遍只认此字段；o 系/gpt-5 场景的按上游配置覆盖待 M10 能力矩阵）。
        let key = match req.max_tokens_field {
            Some(MaxTokensField::MaxCompletionTokens) => "max_completion_tokens",
            Some(MaxTokensField::MaxTokens) | None => "max_tokens",
        };
        body.insert(key.into(), Value::from(mt));
    }
    if let Some(stop) = &req.stop {
        body.insert(
            "stop".into(),
            Value::Array(stop.iter().map(|s| Value::String(s.clone())).collect()),
        );
    }

    body.insert("stream".into(), Value::Bool(req.stream));
    if req.stream_include_usage {
        body.insert(
            "stream_options".into(),
            serde_json::json!({"include_usage": true}),
        );
    }

    if let Some(rf) = &req.response_format {
        body.insert("response_format".into(), rf.clone());
    }
    if let Some(s) = req.seed {
        body.insert("seed".into(), Value::from(s));
    }
    if let Some(u) = &req.user {
        body.insert("user".into(), Value::String(u.clone()));
    }
    if let Some(n) = req.n {
        body.insert("n".into(), Value::from(n));
    }

    Ok(Value::Object(body))
}

fn json_number_f64(v: f64) -> Value {
    serde_json::Number::from_f64(v)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// 非流式响应：OpenAI JSON → IR
// ---------------------------------------------------------------------------

fn parse_usage(u: &Value) -> IrUsage {
    let mut usage = IrUsage::default();
    usage.prompt_tokens = u["prompt_tokens"].as_u64().unwrap_or(0);
    usage.completion_tokens = u["completion_tokens"].as_u64().unwrap_or(0);
    usage.total_tokens = u["total_tokens"].as_u64();
    if let Some(details) = u.get("prompt_tokens_details") {
        if let Some(cached) = details["cached_tokens"].as_u64() {
            usage.cache_read_tokens = Some(cached);
        }
    }
    if let Some(details) = u.get("completion_tokens_details") {
        // reasoning_tokens 提升为 IR 标准槽位（供跨协议转出；嵌套 details 原样保留）
        if let Some(rt) = details["reasoning_tokens"].as_u64() {
            usage
                .extra
                .insert("reasoning_tokens".into(), Value::from(rt));
        }
        usage
            .extra
            .insert("completion_tokens_details".into(), details.clone());
    }
    // 其余未知 usage 字段原样保留
    if let Some(o) = u.as_object() {
        for (k, v) in o {
            if !matches!(
                k.as_str(),
                "prompt_tokens"
                    | "completion_tokens"
                    | "total_tokens"
                    | "prompt_tokens_details"
                    | "completion_tokens_details"
            ) {
                usage.extra.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
    }
    usage
}

fn parse_choice(c: &Value, ctx: &mut ConvCtx) -> Result<IrChoice, ConvertError> {
    if !c.is_object() {
        return Err(ConvertError::Parse("choice 不是对象".into()));
    }
    let mut ch = IrChoice::default();
    ch.index = c["index"]
        .as_u64()
        .and_then(|x| u32::try_from(x).ok())
        .unwrap_or(0);
    if let Some(msg) = c.get("message") {
        ch.message = parse_message(msg, ctx)?;
    }
    if let Some(fr) = c["finish_reason"].as_str() {
        ch.finish_reason = Some(normalize_finish_reason(fr));
    }
    Ok(ch)
}

/// 非流式响应 JSON → IR。
pub fn response_to_ir(v: &Value, ctx: &mut ConvCtx) -> Result<IrResponse, ConvertError> {
    let obj = v
        .as_object()
        .ok_or_else(|| ConvertError::Parse("响应体不是对象".into()))?;
    let mut resp = IrResponse::default();
    resp.id = v["id"].as_str().unwrap_or_default().to_string();
    resp.model = v["model"].as_str().unwrap_or_default().to_string();
    resp.created = v["created"].as_i64().unwrap_or(0);
    resp.system_fingerprint = v["system_fingerprint"].as_str().map(String::from);

    if let Some(choices) = v.get("choices") {
        let arr = choices
            .as_array()
            .ok_or_else(|| ConvertError::Parse("choices 不是数组".into()))?;
        for c in arr {
            resp.choices.push(parse_choice(c, ctx)?);
        }
    }
    if let Some(u) = v.get("usage") {
        resp.usage = Some(parse_usage(u));
    }

    // 未知响应字段进 extra（响应级）
    for (k, val) in obj {
        if !KNOWN_RESPONSE_FIELDS.contains(&k.as_str()) {
            resp.extra.insert(k.clone(), val.clone());
        }
    }

    Ok(resp)
}

// ---------------------------------------------------------------------------
// 非流式响应：IR → OpenAI JSON
// ---------------------------------------------------------------------------

fn choice_to_json(c: &IrChoice) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(c.index));
    m.insert("message".into(), message_to_json(&c.message));
    if let Some(fr) = &c.finish_reason {
        m.insert("finish_reason".into(), Value::String(fr.clone()));
    }
    Value::Object(m)
}

fn usage_to_json(u: &IrUsage, ctx: &mut ConvCtx) -> Value {
    let mut out = u.extra.clone();
    // reasoning_tokens 归一槽位移入 completion_tokens_details（review P5：跨协议归一，
    // 此前来自 Responses 的顶层 reasoning_tokens 原样铺底成 Chat 非法 usage 键）；
    // Responses 专属 details 键不属于 Chat usage，丢弃并记 degrade。
    let rt = out.remove("reasoning_tokens");
    if out.remove("output_tokens_details").is_some() || out.remove("input_tokens_details").is_some()
    {
        ctx.degrade(
            "usage.details",
            "Responses 专属 usage details 已按 Chat 形状归一",
        );
    }
    out.insert("prompt_tokens".into(), Value::from(u.prompt_tokens));
    out.insert("completion_tokens".into(), Value::from(u.completion_tokens));
    if let Some(t) = u.total_tokens {
        out.insert("total_tokens".into(), Value::from(t));
    }
    if let Some(c) = u.cache_read_tokens {
        let mut pd = Map::new();
        pd.insert("cached_tokens".into(), Value::from(c));
        out.insert("prompt_tokens_details".into(), Value::Object(pd));
    }
    if let Some(rt) = rt {
        let mut ctd = match out.remove("completion_tokens_details") {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
        ctd.insert("reasoning_tokens".into(), rt);
        out.insert("completion_tokens_details".into(), Value::Object(ctd));
    }
    if u.cache_write_tokens.is_some() {
        ctx.degrade(
            "usage.cache_write_tokens",
            "OpenAI Chat 无 cache_write_tokens",
        );
    }
    Value::Object(out)
}

/// IR → 非流式响应 JSON。
pub fn response_from_ir(resp: &IrResponse, ctx: &mut ConvCtx) -> Result<Value, ConvertError> {
    let mut body = resp.extra.clone();
    body.insert("id".into(), Value::String(resp.id.clone()));
    body.insert("model".into(), Value::String(resp.model.clone()));
    body.insert("created".into(), Value::from(resp.created));
    if let Some(sf) = &resp.system_fingerprint {
        body.insert("system_fingerprint".into(), Value::String(sf.clone()));
    }
    body.insert(
        "choices".into(),
        Value::Array(resp.choices.iter().map(choice_to_json).collect()),
    );
    if let Some(u) = &resp.usage {
        body.insert("usage".into(), usage_to_json(u, ctx));
    }
    Ok(Value::Object(body))
}

// ---------------------------------------------------------------------------
// 流式 chunk
// ---------------------------------------------------------------------------

/// OpenAI Chat 流式状态（无跨事件状态需求，保留占位）。
#[derive(Debug, Default)]
pub struct StreamState {
    _private: (),
}

fn parse_delta(d: &Value, ctx: &mut ConvCtx) -> IrDelta {
    let mut delta = IrDelta::default();
    if let Some(role) = d["role"].as_str() {
        delta.role = Some(parse_role(Some(role), ctx));
    }
    if let Some(content) = d["content"].as_str() {
        delta.content = Some(content.to_string());
    }
    if let Some(rc) = d["reasoning_content"].as_str() {
        delta.reasoning_content = Some(rc.to_string());
    }
    if let Some(tcs) = d.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in tcs {
            if let Some(d) = parse_tool_call_delta(tc, ctx) {
                delta.tool_calls.push(d);
            }
        }
    }
    delta
}

fn parse_tool_call_delta(tc: &Value, ctx: &mut ConvCtx) -> Option<IrToolCallDelta> {
    if !tc.is_object() {
        ctx.degrade("chunk.tool_calls", "工具调用增量非对象");
        return None;
    }
    let mut d = IrToolCallDelta::default();
    d.index = tc["index"]
        .as_u64()
        .and_then(|x| u32::try_from(x).ok())
        .unwrap_or(0);
    d.id = tc["id"].as_str().map(String::from);
    let f = &tc["function"];
    d.name = f["name"].as_str().map(String::from);
    d.arguments = f["arguments"].as_str().map(String::from);
    Some(d)
}

fn parse_chunk_choice(c: &Value, ctx: &mut ConvCtx) -> Result<IrChunkChoice, ConvertError> {
    if !c.is_object() {
        return Err(ConvertError::Parse("chunk choice 不是对象".into()));
    }
    let mut ch = IrChunkChoice::default();
    ch.index = c["index"]
        .as_u64()
        .and_then(|x| u32::try_from(x).ok())
        .unwrap_or(0);
    if let Some(delta) = c.get("delta") {
        ch.delta = parse_delta(delta, ctx);
    }
    if let Some(fr) = c["finish_reason"].as_str() {
        ch.finish_reason = Some(normalize_finish_reason(fr));
    }
    Ok(ch)
}

/// SSE data 载荷 → IR chunk；"[DONE]" 返回 Ok(None)。
pub fn chunk_to_ir(
    data: &str,
    _st: &mut StreamState,
    ctx: &mut ConvCtx,
) -> Result<Option<IrChunk>, ConvertError> {
    if data.trim() == "[DONE]" {
        return Ok(None);
    }
    let v: Value = serde_json::from_str(data).map_err(|e| ConvertError::Parse(e.to_string()))?;
    let obj = v
        .as_object()
        .ok_or_else(|| ConvertError::Parse("chunk 不是对象".into()))?;
    let mut chunk = IrChunk::default();
    chunk.id = v["id"].as_str().unwrap_or_default().to_string();
    chunk.model = v["model"].as_str().unwrap_or_default().to_string();
    chunk.created = v["created"].as_i64().unwrap_or(0);

    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
        for c in choices {
            chunk.choices.push(parse_chunk_choice(c, ctx)?);
        }
    }
    // include_usage 下内容 chunk 会带 "usage": null——null 不是 usage，
    // 否则下游（Responses/Anthropic 出口）会把首个 chunk 误判为终止帧（发布审阅 H1）。
    if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
        chunk.usage = Some(parse_usage(u));
    }

    // 未知 chunk 顶层字段进 extra
    for (k, val) in obj {
        if !KNOWN_CHUNK_FIELDS.contains(&k.as_str()) {
            chunk.extra.insert(k.clone(), val.clone());
        }
    }

    Ok(Some(chunk))
}

fn tool_call_delta_to_json(t: &IrToolCallDelta) -> Value {
    let mut f = Map::new();
    if let Some(name) = &t.name {
        f.insert("name".into(), Value::String(name.clone()));
    }
    if let Some(args) = &t.arguments {
        f.insert("arguments".into(), Value::String(args.clone()));
    }
    let mut c = Map::new();
    c.insert("index".into(), Value::from(t.index));
    if let Some(id) = &t.id {
        c.insert("id".into(), Value::String(id.clone()));
    }
    c.insert("function".into(), Value::Object(f));
    json_object_with("type", "function", c)
}

fn delta_to_json(d: &IrDelta) -> Value {
    let mut m = Map::new();
    if let Some(role) = d.role {
        m.insert("role".into(), Value::String(role_to_str(role).into()));
    }
    if let Some(content) = &d.content {
        m.insert("content".into(), Value::String(content.clone()));
    }
    if let Some(rc) = &d.reasoning_content {
        m.insert("reasoning_content".into(), Value::String(rc.clone()));
    }
    if !d.tool_calls.is_empty() {
        m.insert(
            "tool_calls".into(),
            Value::Array(d.tool_calls.iter().map(tool_call_delta_to_json).collect()),
        );
    }
    Value::Object(m)
}

fn chunk_choice_to_json(c: &IrChunkChoice) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(c.index));
    m.insert("delta".into(), delta_to_json(&c.delta));
    if let Some(fr) = &c.finish_reason {
        m.insert("finish_reason".into(), Value::String(fr.clone()));
    }
    Value::Object(m)
}

/// IR chunk → SSE data 载荷列表。
pub fn chunk_from_ir(
    chunk: &IrChunk,
    _st: &mut StreamState,
    ctx: &mut ConvCtx,
) -> Result<Vec<String>, ConvertError> {
    let mut obj = chunk.extra.clone();
    obj.insert("id".into(), Value::String(chunk.id.clone()));
    obj.insert("model".into(), Value::String(chunk.model.clone()));
    obj.insert("created".into(), Value::from(chunk.created));
    obj.insert(
        "choices".into(),
        Value::Array(chunk.choices.iter().map(chunk_choice_to_json).collect()),
    );
    if let Some(u) = &chunk.usage {
        obj.insert("usage".into(), usage_to_json(u, ctx));
    }
    let s = serde_json::to_string(&Value::Object(obj))
        .map_err(|e| ConvertError::Parse(e.to_string()))?;
    Ok(vec![s])
}

/// 流终止载荷：["[DONE]"]。
pub fn stream_end(_st: &mut StreamState, _ctx: &mut ConvCtx) -> Result<Vec<String>, ConvertError> {
    Ok(vec!["[DONE]".to_string()])
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
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Hello"}
            ],
            "temperature": 0.7,
            "max_tokens": 100,
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, IrRole::System);
        assert_eq!(
            req.messages[1].content,
            Some(IrContent::Text("Hello".into()))
        );
        assert_eq!(req.temperature, Some(0.7));
        assert_eq!(req.max_tokens, Some(100));

        let back = request_from_ir(&req, &mut c).unwrap();
        let req2 = request_to_ir(&back, &mut c).unwrap();
        assert_eq!(req, req2);
    }

    #[test]
    fn text_response_roundtrip() {
        let j = serde_json::json!({
            "id": "chatcmpl-x",
            "model": "gpt-4o",
            "created": 123,
            "system_fingerprint": "fp_1",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hi there", "reasoning_content": "thinking..."},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15,
                "prompt_tokens_details": {"cached_tokens": 3},
                "completion_tokens_details": {"reasoning_tokens": 2}
            }
        });
        let mut c = ctx();
        let resp = response_to_ir(&j, &mut c).unwrap();
        assert_eq!(resp.id, "chatcmpl-x");
        assert_eq!(
            resp.choices[0].message.content,
            Some(IrContent::Text("hi there".into()))
        );
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
        let usage = resp.usage.as_ref().unwrap();
        assert_eq!(usage.cache_read_tokens, Some(3));
        assert!(usage.extra.contains_key("completion_tokens_details"));

        let back = response_from_ir(&resp, &mut c).unwrap();
        let resp2 = response_to_ir(&back, &mut c).unwrap();
        assert_eq!(resp, resp2);
    }

    #[test]
    fn images_url_and_inline_roundtrip() {
        let j = serde_json::json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "what is this?"},
                    {"type": "image_url", "image_url": {"url": "https://example.com/a.png"}},
                    {"type": "image_url", "image_url": "https://example.com/b.png"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAABBBB"}},
                    {"type": "input_audio", "input_audio": {"data": "aGVsbG8=", "format": "wav"}},
                    {"type": "file", "file": {"file_name": "README.md", "file_data": "aGVsbG8="}}
                ]
            }]
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        let parts = match &req.messages[0].content {
            Some(IrContent::Parts(parts)) => parts,
            _ => panic!("应为 Parts"),
        };
        assert_eq!(parts.len(), 6);
        assert_eq!(
            parts[1],
            IrPart::ImageUrl {
                url: "https://example.com/a.png".into()
            }
        );
        assert_eq!(
            parts[2],
            IrPart::ImageUrl {
                url: "https://example.com/b.png".into()
            }
        );
        assert_eq!(
            parts[3],
            IrPart::ImageInline {
                media_type: "image/png".into(),
                data: "AAAABBBB".into()
            }
        );
        assert_eq!(
            parts[4],
            IrPart::InputAudio {
                data: "aGVsbG8=".into(),
                format: "wav".into()
            }
        );
        assert_eq!(
            parts[5],
            IrPart::File {
                name: "README.md".into(),
                url: None,
                data: Some("aGVsbG8=".into())
            }
        );

        let back = request_from_ir(&req, &mut c).unwrap();
        let req2 = request_to_ir(&back, &mut c).unwrap();
        assert_eq!(req, req2);
    }

    #[test]
    fn tools_definitions_calls_and_results() {
        let j = serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "user", "content": "weather in bj?"},
                {
                    "role": "assistant",
                    "content": null,
                    "reasoning_content": "need tool",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{\"city\":\"bj\"}"}
                    }]
                },
                {"role": "tool", "tool_call_id": "call_1", "content": "sunny"}
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "get weather",
                    "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
                }
            }],
            "tool_choice": "auto"
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].name, "get_weather");
        assert_eq!(req.tool_choice, Some(serde_json::json!("auto")));
        let asst = &req.messages[1];
        assert_eq!(asst.tool_calls.len(), 1);
        assert_eq!(asst.tool_calls[0].name, "get_weather");
        assert_eq!(asst.tool_calls[0].arguments, "{\"city\":\"bj\"}");
        assert_eq!(asst.reasoning_content.as_deref(), Some("need tool"));
        assert_eq!(req.messages[2].tool_call_id.as_deref(), Some("call_1"));

        let back = request_from_ir(&req, &mut c).unwrap();
        let req2 = request_to_ir(&back, &mut c).unwrap();
        assert_eq!(req, req2);
    }

    #[test]
    fn tools_non_function_degraded() {
        let j = serde_json::json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [
                {"type": "function", "function": {"name": "a"}},
                {"type": "web_search", "web_search": {}}
            ]
        });
        let mut c = ctx();
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.tools.len(), 1);
        assert!(c.degraded.iter().any(|d| d.field == "tools"));
    }

    #[test]
    fn streaming_chunks_roundtrip() {
        let mut c = ctx();
        let mut st = StreamState::default();

        // 角色 + 内容增量
        let d1 = serde_json::json!({
            "id": "cmpl-1", "model": "gpt-4o", "created": 1,
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": "Hello"}, "finish_reason": null}]
        });
        let c1 = chunk_to_ir(&serde_json::to_string(&d1).unwrap(), &mut st, &mut c)
            .unwrap()
            .unwrap();
        assert_eq!(c1.choices[0].delta.role, Some(IrRole::Assistant));
        assert_eq!(c1.choices[0].delta.content.as_deref(), Some("Hello"));
        let out = chunk_from_ir(&c1, &mut st, &mut c).unwrap();
        let c1b = chunk_to_ir(&out[0], &mut st, &mut c).unwrap().unwrap();
        assert_eq!(c1, c1b);

        // 工具调用增量
        let d2 = serde_json::json!({
            "id": "cmpl-1", "model": "gpt-4o", "created": 1,
            "choices": [{"index": 0, "delta": {
                "tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":"}}]
            }, "finish_reason": null}]
        });
        let c2 = chunk_to_ir(&serde_json::to_string(&d2).unwrap(), &mut st, &mut c)
            .unwrap()
            .unwrap();
        let tc = &c2.choices[0].delta.tool_calls[0];
        assert_eq!(tc.index, 0);
        assert_eq!(tc.id.as_deref(), Some("call_1"));
        assert_eq!(tc.name.as_deref(), Some("get_weather"));
        assert_eq!(tc.arguments.as_deref(), Some("{\"city\":"));
        let out2 = chunk_from_ir(&c2, &mut st, &mut c).unwrap();
        let c2b = chunk_to_ir(&out2[0], &mut st, &mut c).unwrap().unwrap();
        assert_eq!(c2, c2b);

        // usage 块
        let d3 = serde_json::json!({
            "id": "cmpl-1", "model": "gpt-4o", "created": 1,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15,
                "prompt_tokens_details": {"cached_tokens": 3},
                "completion_tokens_details": {"reasoning_tokens": 2}
            }
        });
        let c3 = chunk_to_ir(&serde_json::to_string(&d3).unwrap(), &mut st, &mut c)
            .unwrap()
            .unwrap();
        assert_eq!(c3.choices[0].finish_reason.as_deref(), Some("stop"));
        let u = c3.usage.as_ref().unwrap();
        assert_eq!(u.cache_read_tokens, Some(3));
        let out3 = chunk_from_ir(&c3, &mut st, &mut c).unwrap();
        let c3b = chunk_to_ir(&out3[0], &mut st, &mut c).unwrap().unwrap();
        assert_eq!(c3, c3b);

        // [DONE]
        let done = chunk_to_ir("[DONE]", &mut st, &mut c).unwrap();
        assert!(done.is_none());
    }

    #[test]
    fn stream_end_returns_done() {
        let mut st = StreamState::default();
        let mut c = ctx();
        assert_eq!(
            stream_end(&mut st, &mut c).unwrap(),
            vec!["[DONE]".to_string()]
        );
    }

    #[test]
    fn unknown_fields_preserved_in_extra() {
        let mut c = ctx();
        let j = serde_json::json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "custom_unknown": 42,
            "logprobs": true
        });
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.extra["custom_unknown"], 42);
        assert_eq!(req.extra["logprobs"], true);
        // 往返后仍保留
        let back = request_from_ir(&req, &mut c).unwrap();
        let req2 = request_to_ir(&back, &mut c).unwrap();
        assert_eq!(req, req2);

        // 响应级 extra
        let rj = serde_json::json!({
            "id": "x", "model": "m", "created": 1,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}],
            "system_fingerprint": "fp",
            "extra_field": "v"
        });
        let resp = response_to_ir(&rj, &mut c).unwrap();
        assert_eq!(resp.extra["extra_field"], "v");
        let back2 = response_from_ir(&resp, &mut c).unwrap();
        let resp2 = response_to_ir(&back2, &mut c).unwrap();
        assert_eq!(resp, resp2);
    }

    #[test]
    fn degrade_on_web_search_ext() {
        let req = IrRequest {
            model: "m".into(),
            ext: IrExt {
                web_search: Some(serde_json::json!({"enabled": true})),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut c = ctx();
        request_from_ir(&req, &mut c).unwrap();
        assert!(c.degraded.iter().any(|d| d.field == "ext.web_search"));

        let req2 = IrRequest {
            model: "m".into(),
            ext: IrExt {
                thinking: Some(serde_json::json!({"sig": "x"})),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut c2 = ctx();
        request_from_ir(&req2, &mut c2).unwrap();
        assert!(c2.degraded.iter().any(|d| d.field == "ext.thinking"));
    }

    #[test]
    fn response_finish_reason_normalized() {
        let mut c = ctx();
        let rj = serde_json::json!({
            "id": "x", "model": "m", "created": 1,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "end_turn"}]
        });
        let resp = response_to_ir(&rj, &mut c).unwrap();
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    /// review P5 回归：usage 跨协议归一。Chat 入站把 reasoning_tokens 提升到标准槽位；
    /// 出站移入 completion_tokens_details，Responses 专属 details 键不泄漏进 Chat usage。
    #[test]
    fn usage_reasoning_tokens_normalized() {
        let mut c = ctx();
        // Chat 入站：completion_tokens_details.reasoning_tokens → extra["reasoning_tokens"]
        let rj = serde_json::json!({
            "id": "x", "model": "m", "created": 1,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15,
                "completion_tokens_details": {"reasoning_tokens": 2, "audio_tokens": 0}
            }
        });
        let resp = response_to_ir(&rj, &mut c).unwrap();
        let usage = resp.usage.as_ref().unwrap();
        assert_eq!(usage.extra["reasoning_tokens"], 2);

        // Chat 出站：reasoning_tokens 嵌套回 details，无裸键
        let back = response_from_ir(&resp, &mut c).unwrap();
        assert_eq!(
            back["usage"]["completion_tokens_details"]["reasoning_tokens"],
            2
        );
        assert!(back["usage"].get("reasoning_tokens").is_none());

        // 来自 Responses 的 extra 形状（顶层 reasoning_tokens + output_tokens_details）→ Chat
        let mut u2 = IrUsage {
            prompt_tokens: 3,
            completion_tokens: 4,
            ..Default::default()
        };
        u2.extra.insert("reasoning_tokens".into(), Value::from(7));
        u2.extra.insert(
            "output_tokens_details".into(),
            serde_json::json!({"reasoning_tokens": 7}),
        );
        let out = usage_to_json(&u2, &mut c);
        assert_eq!(out["completion_tokens_details"]["reasoning_tokens"], 7);
        assert!(out.get("reasoning_tokens").is_none());
        assert!(out.get("output_tokens_details").is_none());
    }

    /// review P5 回归：Responses 形状 tool_choice 转 Chat 时包装 function 对象。
    #[test]
    fn tool_choice_responses_shape_wrapped() {
        let mut c = ctx();
        let mut req = IrRequest {
            model: "gpt-5".into(),
            ..Default::default()
        };
        req.tool_choice = Some(serde_json::json!({"type": "function", "name": "get_weather"}));
        let body = request_from_ir(&req, &mut c).unwrap();
        assert_eq!(
            body["tool_choice"],
            serde_json::json!({"type": "function", "function": {"name": "get_weather"}})
        );
    }

    /// max_tokens 字段名 round-trip 保真：用户请求用什么字段就回写什么字段；
    /// 未携带则不写；跨协议（无原始字段名）默认 max_tokens。
    #[test]
    fn max_tokens_field_roundtrip_fidelity() {
        let mut c = ctx();
        // max_completion_tokens 入口 → 回写同名字段
        let j = serde_json::json!({
            "model": "gpt-5", "messages": [{"role": "user", "content": "hi"}],
            "max_completion_tokens": 256
        });
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.max_tokens, Some(256));
        assert_eq!(
            req.max_tokens_field,
            Some(MaxTokensField::MaxCompletionTokens)
        );
        let back = request_from_ir(&req, &mut c).unwrap();
        assert_eq!(back["max_completion_tokens"], 256);
        assert!(back.get("max_tokens").is_none());

        // max_tokens 入口 → 回写 max_tokens
        let j = serde_json::json!({
            "model": "gpt-4o", "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 128
        });
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.max_tokens_field, Some(MaxTokensField::MaxTokens));
        let back = request_from_ir(&req, &mut c).unwrap();
        assert_eq!(back["max_tokens"], 128);
        assert!(back.get("max_completion_tokens").is_none());

        // 未携带 → 不写任何字段
        let j = serde_json::json!({
            "model": "gpt-4o", "messages": [{"role": "user", "content": "hi"}]
        });
        let req = request_to_ir(&j, &mut c).unwrap();
        assert_eq!(req.max_tokens, None);
        assert_eq!(req.max_tokens_field, None);
        let back = request_from_ir(&req, &mut c).unwrap();
        assert!(back.get("max_tokens").is_none());
        assert!(back.get("max_completion_tokens").is_none());

        // 跨协议转换（IR 有值但无原始字段名）→ 默认 max_tokens
        let req = IrRequest {
            model: "gpt-4o".into(),
            max_tokens: Some(64),
            ..Default::default()
        };
        let back = request_from_ir(&req, &mut c).unwrap();
        assert_eq!(back["max_tokens"], 64);
        assert!(back.get("max_completion_tokens").is_none());

        // 两字段并存 → 取 max_completion_tokens 值并记降级，回写新字段名
        let j = serde_json::json!({
            "model": "gpt-5", "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 100, "max_completion_tokens": 200
        });
        let mut c2 = ctx();
        let req = request_to_ir(&j, &mut c2).unwrap();
        assert_eq!(req.max_tokens, Some(200));
        assert!(c2.degraded.iter().any(|d| d.field == "max_tokens"));
        let back = request_from_ir(&req, &mut c2).unwrap();
        assert_eq!(back["max_completion_tokens"], 200);
    }
}
