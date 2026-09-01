//! usage 提取（4 协议，非流式 JSON / 流式 SSE / 请求参数元数据）。
//! 契约 contracts/m4-logging.md §11，M4-C 实现。

use crate::protocol::ir::Protocol;

/// 从响应提取的 usage（尽力而为；raw = 原始 usage 子对象）
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub raw: Option<serde_json::Value>,
}

/// 将 JSON 数字安全地转为 i64（先试 i64，再回退 u64 截断；token 计数非负）。
fn token(v: &serde_json::Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_u64().map(|u| u as i64))
}

/// 非流式：按协议路径取 usage 对象。
pub fn extract_json_usage(p: Protocol, v: &serde_json::Value) -> Usage {
    let mut u = Usage::default();
    match p {
        Protocol::OpenaiChat => {
            let usage = v.get("usage");
            u.prompt_tokens = usage.and_then(|u| u.get("prompt_tokens")).and_then(token);
            u.completion_tokens = usage.and_then(|u| u.get("completion_tokens")).and_then(token);
            // chat：prompt_tokens_details.cached_tokens → cache_read（读侧命中缓存）
            u.cache_read_tokens = usage
                .and_then(|u| u.get("prompt_tokens_details"))
                .and_then(|d| d.get("cached_tokens"))
                .and_then(token);
            u.raw = usage.cloned();
        }
        Protocol::OpenaiResponses => {
            let usage = v.get("usage");
            u.prompt_tokens = usage.and_then(|u| u.get("input_tokens")).and_then(token);
            u.completion_tokens = usage.and_then(|u| u.get("output_tokens")).and_then(token);
            u.cache_read_tokens = usage
                .and_then(|u| u.get("input_tokens_details"))
                .and_then(|d| d.get("cached_tokens"))
                .and_then(token);
            u.raw = usage.cloned();
        }
        Protocol::Anthropic => {
            let usage = v.get("usage");
            u.prompt_tokens = usage.and_then(|u| u.get("input_tokens")).and_then(token);
            u.completion_tokens = usage.and_then(|u| u.get("output_tokens")).and_then(token);
            u.cache_write_tokens = usage
                .and_then(|u| u.get("cache_creation_input_tokens"))
                .and_then(token);
            u.cache_read_tokens = usage
                .and_then(|u| u.get("cache_read_input_tokens"))
                .and_then(token);
            u.raw = usage.cloned();
        }
        Protocol::Gemini => {
            let usage = v.get("usageMetadata");
            u.prompt_tokens = usage.and_then(|u| u.get("promptTokenCount")).and_then(token);
            u.completion_tokens = usage.and_then(|u| u.get("candidatesTokenCount")).and_then(token);
            u.cache_read_tokens = usage
                .and_then(|u| u.get("cachedContentTokenCount"))
                .and_then(token);
            u.raw = usage.cloned();
        }
    }
    u
}

/// 从一条 SSE `data:` 载荷字符串中解析 JSON（失败返回 None）。
fn parse_sse_data(line: &str) -> Option<serde_json::Value> {
    let line = line.trim();
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" || data.is_empty() {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(data).ok()
}

/// 流式：解析 SSE 文本（data: 行 JSON，[DONE] 跳过），最后出现的 usage 优先
/// （Anthropic 为 message_start.input + 末次 message_delta.output 累加）。
pub fn extract_sse_usage(p: Protocol, sse_text: &str) -> Usage {
    // 各协议需要的累加/滚动变量
    let mut last: Usage = Usage::default();
    // Anthropic：message_start 记录 prompt/缓存侧，message_delta 记录 output（末次覆盖）
    let mut a_input: Option<i64> = None;
    let mut a_output: Option<i64> = None;
    let mut a_cache_write: Option<i64> = None;
    let mut a_cache_read: Option<i64> = None;

    for line in sse_text.lines() {
        let Some(v) = parse_sse_data(line) else { continue };
        match p {
            Protocol::Anthropic => {
                let ev_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if ev_type == "message_start" {
                    if let Some(usage) = v.get("message").and_then(|m| m.get("usage")) {
                        a_input = usage.get("input_tokens").and_then(token);
                        a_cache_write = usage.get("cache_creation_input_tokens").and_then(token);
                        a_cache_read = usage.get("cache_read_input_tokens").and_then(token);
                    }
                } else if ev_type == "message_delta" {
                    if let Some(usage) = v.get("usage") {
                        // 末次 message_delta.output_tokens 覆盖
                        if let Some(t) = usage.get("output_tokens").and_then(token) {
                            a_output = Some(t);
                        }
                    }
                }
            }
            Protocol::Gemini => {
                // 只要出现 usageMetadata 即视为该 chunk 携带 usage；取最后出现的一次
                if v.get("usageMetadata").is_some() {
                    last = extract_json_usage(Protocol::Gemini, &v);
                }
            }
            Protocol::OpenaiResponses => {
                if v.get("type").and_then(|t| t.as_str()) == Some("response.completed") {
                    if let Some(resp) = v.get("response") {
                        last = extract_json_usage(Protocol::OpenaiResponses, resp);
                    }
                }
            }
            Protocol::OpenaiChat => {
                // 最后出现且有实际 token 的 usage 优先（include_usage 注入的末尾空 choices chunk）
                if v.get("usage").is_some() {
                    let u = extract_json_usage(Protocol::OpenaiChat, &v);
                    if u.prompt_tokens.is_some() || u.completion_tokens.is_some() || u.cache_read_tokens.is_some() {
                        last = u;
                    }
                }
            }
        }
    }

    if p == Protocol::Anthropic {
        let mut raw = serde_json::Map::new();
        if let Some(x) = a_input {
            raw.insert("input_tokens".into(), serde_json::Value::from(x));
        }
        if let Some(x) = a_output {
            raw.insert("output_tokens".into(), serde_json::Value::from(x));
        }
        if let Some(x) = a_cache_write {
            raw.insert("cache_creation_input_tokens".into(), serde_json::Value::from(x));
        }
        if let Some(x) = a_cache_read {
            raw.insert("cache_read_input_tokens".into(), serde_json::Value::from(x));
        }
        return Usage {
            prompt_tokens: a_input,
            completion_tokens: a_output,
            cache_write_tokens: a_cache_write,
            cache_read_tokens: a_cache_read,
            raw: if raw.is_empty() { None } else { Some(serde_json::Value::Object(raw)) },
        };
    }

    last
}

/// 请求参数元数据（PLAN §5.5 默认只记参数，不记正文）
pub fn request_params_meta(p: Protocol, v: &serde_json::Value) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    if let Some(m) = v.get("model").and_then(|x| x.as_str()) {
        obj.insert("model".into(), serde_json::Value::String(m.to_string()));
    }
    if let Some(s) = v.get("stream").and_then(|x| x.as_bool()) {
        obj.insert("stream".into(), serde_json::Value::Bool(s));
    }
    if let Some(mt) = max_tokens_field(p, v) {
        obj.insert("max_tokens".into(), serde_json::Value::from(mt));
    }
    if let Some(t) = v.get("temperature").and_then(|x| x.as_f64()) {
        obj.insert("temperature".into(), serde_json::Value::from(t));
    }
    if let Some(tools) = tools_summary(v) {
        obj.insert("tools".into(), tools);
    }
    if let Some(w) = web_search_flag(v) {
        obj.insert("web_search".into(), serde_json::Value::Bool(w));
    }
    serde_json::Value::Object(obj)
}

/// 各协议的 max_tokens 字段名归一化。
fn max_tokens_field(p: Protocol, v: &serde_json::Value) -> Option<u64> {
    match p {
        Protocol::OpenaiChat => v
            .get("max_tokens")
            .or_else(|| v.get("max_completion_tokens"))
            .and_then(|x| x.as_u64()),
        Protocol::OpenaiResponses => v.get("max_output_tokens").and_then(|x| x.as_u64()),
        Protocol::Anthropic => v.get("max_tokens").and_then(|x| x.as_u64()),
        Protocol::Gemini => v.get("maxOutputTokens").and_then(|x| x.as_u64()),
    }
}

/// tools 汇总：`{names:[...], count:n}`（尽力而为，缺省省略）。
fn tools_summary(v: &serde_json::Value) -> Option<serde_json::Value> {
    let arr = v.get("tools")?.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let names: Vec<serde_json::Value> = arr
        .iter()
        .filter_map(|t| {
            t.get("name")
                .or_else(|| t.get("function").and_then(|f| f.get("name")))
                .and_then(|x| x.as_str())
                .map(|s| serde_json::Value::String(s.to_string()))
        })
        .collect();
    Some(serde_json::json!({ "names": names, "count": arr.len() }))
}

/// 是否启用联网搜索（尽力而为：顶层布尔，或 tools 数组中的 web_search / google_search）。
fn web_search_flag(v: &serde_json::Value) -> Option<bool> {
    if let Some(b) = v.get("web_search").and_then(|x| x.as_bool()) {
        return Some(b);
    }
    if let Some(arr) = v.get("tools").and_then(|x| x.as_array()) {
        for t in arr {
            if let Some(o) = t.as_object() {
                if o.contains_key("web_search") || o.contains_key("google_search") {
                    return Some(true);
                }
                if let Some(ty) = o.get("type").and_then(|x| x.as_str()) {
                    if ty.starts_with("web_search") {
                        return Some(true);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_usage_openai_chat() {
        let v = json!({"usage": {
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "prompt_tokens_details": { "cached_tokens": 5 }
        }});
        let u = extract_json_usage(Protocol::OpenaiChat, &v);
        assert_eq!(u.prompt_tokens, Some(100));
        assert_eq!(u.completion_tokens, Some(20));
        assert_eq!(u.cache_read_tokens, Some(5));
        assert_eq!(u.cache_write_tokens, None);
        assert!(u.raw.is_some());
    }

    #[test]
    fn json_usage_openai_responses() {
        let v = json!({"usage": {
            "input_tokens": 8,
            "output_tokens": 4,
            "input_tokens_details": { "cached_tokens": 3 }
        }});
        let u = extract_json_usage(Protocol::OpenaiResponses, &v);
        assert_eq!(u.prompt_tokens, Some(8));
        assert_eq!(u.completion_tokens, Some(4));
        assert_eq!(u.cache_read_tokens, Some(3));
    }

    #[test]
    fn json_usage_anthropic() {
        let v = json!({"usage": {
            "input_tokens": 6,
            "output_tokens": 2,
            "cache_creation_input_tokens": 1,
            "cache_read_input_tokens": 9
        }});
        let u = extract_json_usage(Protocol::Anthropic, &v);
        assert_eq!(u.prompt_tokens, Some(6));
        assert_eq!(u.completion_tokens, Some(2));
        assert_eq!(u.cache_write_tokens, Some(1));
        assert_eq!(u.cache_read_tokens, Some(9));
    }

    #[test]
    fn json_usage_gemini() {
        let v = json!({"usageMetadata": {
            "promptTokenCount": 12,
            "candidatesTokenCount": 6,
            "cachedContentTokenCount": 1
        }});
        let u = extract_json_usage(Protocol::Gemini, &v);
        assert_eq!(u.prompt_tokens, Some(12));
        assert_eq!(u.completion_tokens, Some(6));
        assert_eq!(u.cache_read_tokens, Some(1));
    }

    #[test]
    fn json_usage_missing_returns_default() {
        let u = extract_json_usage(Protocol::OpenaiChat, &json!({}));
        assert_eq!(u.prompt_tokens, None);
        assert_eq!(u.completion_tokens, None);
        assert_eq!(u.raw, None);
    }

    #[test]
    fn sse_usage_chat_include_usage_tail() {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
                   data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
                   data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"prompt_tokens_details\":{\"cached_tokens\":2}}}\n\n\
                   data: [DONE]\n\n";
        let u = extract_sse_usage(Protocol::OpenaiChat, sse);
        assert_eq!(u.prompt_tokens, Some(10));
        assert_eq!(u.completion_tokens, Some(5));
        assert_eq!(u.cache_read_tokens, Some(2));
    }

    #[test]
    fn sse_usage_anthropic_accumulates() {
        let sse = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":0,\"cache_creation_input_tokens\":3,\"cache_read_input_tokens\":4}}}\n\n\
                   data: {\"type\":\"content_block_start\"}\n\n\
                   data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":5}}\n\n\
                   data: {\"type\":\"message_stop\"}\n\n";
        let u = extract_sse_usage(Protocol::Anthropic, sse);
        assert_eq!(u.prompt_tokens, Some(10));
        assert_eq!(u.completion_tokens, Some(5));
        assert_eq!(u.cache_write_tokens, Some(3));
        assert_eq!(u.cache_read_tokens, Some(4));
    }

    #[test]
    fn sse_usage_gemini_usage_metadata() {
        let sse = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}]}\n\n\
                   data: {\"candidates\":[],\"usageMetadata\":{\"promptTokenCount\":12,\"candidatesTokenCount\":6,\"cachedContentTokenCount\":1}}\n\n";
        let u = extract_sse_usage(Protocol::Gemini, sse);
        assert_eq!(u.prompt_tokens, Some(12));
        assert_eq!(u.completion_tokens, Some(6));
        assert_eq!(u.cache_read_tokens, Some(1));
    }

    #[test]
    fn sse_usage_responses_completed() {
        let sse = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n\
                   data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":8,\"output_tokens\":4,\"input_tokens_details\":{\"cached_tokens\":3}}}}\n\n";
        let u = extract_sse_usage(Protocol::OpenaiResponses, sse);
        assert_eq!(u.prompt_tokens, Some(8));
        assert_eq!(u.completion_tokens, Some(4));
        assert_eq!(u.cache_read_tokens, Some(3));
    }

    #[test]
    fn sse_usage_no_usage_returns_default() {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\ndata: [DONE]\n\n";
        let u = extract_sse_usage(Protocol::OpenaiChat, sse);
        assert_eq!(u.prompt_tokens, None);
        assert_eq!(u.completion_tokens, None);
    }

    #[test]
    fn request_params_meta_full() {
        let v = json!({
            "model": "gpt-4o",
            "stream": true,
            "max_tokens": 200,
            "temperature": 0.5,
            "tools": [
                { "type": "function", "function": { "name": "get_weather" } },
                { "type": "web_search" }
            ]
        });
        let meta = request_params_meta(Protocol::OpenaiChat, &v);
        assert_eq!(meta["model"], json!("gpt-4o"));
        assert_eq!(meta["stream"], json!(true));
        assert_eq!(meta["max_tokens"], json!(200));
        assert_eq!(meta["temperature"], json!(0.5));
        assert_eq!(meta["tools"]["count"], json!(2));
        assert_eq!(meta["tools"]["names"][0], json!("get_weather"));
        assert_eq!(meta["web_search"], json!(true));
    }

    #[test]
    fn request_params_meta_anthropic_max_tokens() {
        let v = json!({ "model": "claude-3", "max_tokens": 4096, "temperature": 0.2 });
        let meta = request_params_meta(Protocol::Anthropic, &v);
        assert_eq!(meta["max_tokens"], json!(4096));
        assert_eq!(meta["temperature"], json!(0.2));
        assert!(meta.get("stream").is_none()); // 缺失字段省略
    }

    #[test]
    fn request_params_meta_gemini_max_output_tokens() {
        let v = json!({ "model": "gemini-pro", "maxOutputTokens": 100 });
        let meta = request_params_meta(Protocol::Gemini, &v);
        assert_eq!(meta["max_tokens"], json!(100));
    }

    #[test]
    fn request_params_meta_empty() {
        let meta = request_params_meta(Protocol::OpenaiChat, &json!({}));
        assert_eq!(meta, json!({}));
    }
}
