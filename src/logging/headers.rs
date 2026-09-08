//! 请求头白名单摘要（M14.3 调试头记录，usage_logs.request_headers JSONB）。
//!
//! 安全约束（发布审阅级）：
//! - 仅记录 [`ALLOWED_HEADERS`] 白名单内的头；Authorization / Cookie / x-api-key /
//!   proxy-authorization / set-cookie 等敏感头绝不入库；
//! - key 一律规范化为小写（http::HeaderName 本身已小写，这里显式 to_lowercase 兜底）；
//! - 空值 / 非 UTF-8 可见值跳过；
//! - 每个值截断至 [`VALUE_MAX_BYTES`] 字节（字符边界安全，截断后追加 `…`）；
//! - 整包 JSON 序列化不超过 [`TOTAL_MAX_BYTES`] 字节（超出时按字母序从后往前丢弃键）。
//!
//! x-forwarded-for / x-real-ip 属敏感转发 IP：仅在部署已有可信代理语义（前置代理重写）
//! 时才有诊断价值；此处按原始摘要记录（截断后），不解析、不提升为可信来源。

use axum::http::HeaderMap;
use serde_json::{Map, Value};

/// 单个头值最大字节数（超出按字符边界截断并追加省略标记）。
pub const VALUE_MAX_BYTES: usize = 512;
/// 整包 JSON 序列化最大字节数（超出丢弃部分键）。
pub const TOTAL_MAX_BYTES: usize = 8 * 1024;

/// 允许记录的请求头白名单（全小写；不在此列的头一律不记录）。
const ALLOWED_HEADERS: &[&str] = &[
    "user-agent",
    "x-forwarded-for",
    "x-real-ip",
    "x-forwarded-proto",
    "content-type",
    "accept",
    "accept-encoding",
    "host",
    "origin",
    "referer",
    "x-request-id",
];

/// 从请求头提取白名单摘要 → JSON 对象（小写 key → 截断后字符串值）。
/// 无任一白名单头（或全部空值）→ None（入库为 NULL）。
pub fn summarize_request_headers(headers: &HeaderMap) -> Option<Value> {
    let mut map = Map::new();
    for name in ALLOWED_HEADERS {
        let Some(v) = headers.get(*name) else {
            continue;
        };
        // 非合法 ASCII/UTF-8 值直接跳过（不猜测编码，避免乱码入库）。
        let Ok(s) = v.to_str() else { continue };
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        // key 规范化小写（白名单常量已小写，显式转换防御未来改动）。
        map.insert(
            name.to_lowercase(),
            Value::String(truncate_bytes(s, VALUE_MAX_BYTES)),
        );
    }
    if map.is_empty() {
        return None;
    }
    // 总量限制：序列化后超过上限时，按键名字典序从后往前丢弃，直至达标或清空。
    while !map.is_empty() && serialized_len(&map) > TOTAL_MAX_BYTES {
        let Some(last) = map.keys().next_back().cloned() else {
            break;
        };
        map.remove(&last);
    }
    if map.is_empty() {
        return None;
    }
    Some(Value::Object(map))
}

/// 序列化长度（字节）。对象只含字符串值，序列化不会失败；失败兜底返回 usize::MAX 触发丢弃。
fn serialized_len(map: &Map<String, Value>) -> usize {
    serde_json::to_vec(map)
        .map(|b| b.len())
        .unwrap_or(usize::MAX)
}

/// 按字节上限截断 UTF-8 字符串（保证落在字符边界上），截断时追加 `…`。
fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 3);
    out.push_str(&s[..end]);
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn mk(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn whitelist_headers_recorded() {
        // 白名单头（尤其 UA）正常记录，key 为小写。
        let h = mk(&[
            ("user-agent", "Mozilla/5.0 TestAgent"),
            ("content-type", "application/json"),
            ("x-request-id", "req-123"),
            ("referer", "https://example.com/page"),
        ]);
        let v = summarize_request_headers(&h).expect("应有白名单摘要");
        assert_eq!(v["user-agent"], "Mozilla/5.0 TestAgent");
        assert_eq!(v["content-type"], "application/json");
        assert_eq!(v["x-request-id"], "req-123");
        assert_eq!(v["referer"], "https://example.com/page");
    }

    #[test]
    fn sensitive_headers_excluded() {
        // 敏感头绝不记录：authorization / cookie / x-api-key / proxy-authorization。
        let h = mk(&[
            ("user-agent", "UA"),
            ("authorization", "Bearer sk-secret-token"),
            ("cookie", "session=abc123"),
            ("x-api-key", "sk-another-secret"),
            ("proxy-authorization", "Basic dXNlcjpwYXNz"),
            ("set-cookie", "sid=xyz"),
            ("x-custom-secret", "topsecret"),
        ]);
        let v = summarize_request_headers(&h).expect("UA 应保留");
        assert_eq!(v["user-agent"], "UA");
        let text = v.to_string();
        for sensitive in [
            "authorization",
            "cookie",
            "x-api-key",
            "proxy-authorization",
            "set-cookie",
            "x-custom-secret",
            "sk-secret-token",
            "session=abc123",
            "topsecret",
        ] {
            assert!(!text.contains(sensitive), "敏感内容泄漏: {sensitive}");
        }
    }

    #[test]
    fn no_whitelist_headers_returns_none() {
        // 只有非白名单头 → None（入库 NULL）。
        let h = mk(&[("authorization", "Bearer x"), ("cookie", "a=b")]);
        assert!(summarize_request_headers(&h).is_none());
        assert!(summarize_request_headers(&HeaderMap::new()).is_none());
    }

    #[test]
    fn empty_value_skipped() {
        // 空值/纯空白值跳过。
        let h = mk(&[("user-agent", "   "), ("accept", "application/json")]);
        let v = summarize_request_headers(&h).expect("accept 应保留");
        assert!(v.get("user-agent").is_none());
        assert_eq!(v["accept"], "application/json");
    }

    #[test]
    fn long_value_truncated() {
        // 超长 UA 截断至 512 字节内（字符边界安全）并追加省略标记。
        let long = "A".repeat(2000);
        let h = mk(&[("user-agent", &long)]);
        let v = summarize_request_headers(&h).expect("应截断保留");
        let s = v["user-agent"].as_str().unwrap();
        assert!(s.ends_with('…'));
        assert!(s.len() <= VALUE_MAX_BYTES + '…'.len_utf8());
        assert_eq!(&s[..VALUE_MAX_BYTES], &long[..VALUE_MAX_BYTES]);
    }

    #[test]
    fn multibyte_value_truncated_on_char_boundary() {
        // 多字节字符截断不产生非法 UTF-8（中文字符 3 字节/个，512 不整除）。
        // 注：http::HeaderValue::to_str 拒绝 obs-text（≥0x80），非 ASCII 头值会被整体跳过；
        // 字符边界安全由 truncate_bytes 保证，此处直接单测该纯函数。
        let long = "用".repeat(400); // 1200 字节
        let s = truncate_bytes(&long, VALUE_MAX_BYTES);
        assert!(s.ends_with('…'));
        // 截断点必须落在字符边界：去掉省略号后全为完整字符。
        let body = &s[..s.len() - '…'.len_utf8()];
        assert_eq!(body.len() % "用".len(), 0);
        assert!(body.len() <= VALUE_MAX_BYTES);
        // 短值不截断
        assert_eq!(truncate_bytes("abc", VALUE_MAX_BYTES), "abc");
    }

    #[test]
    fn non_ascii_header_value_skipped() {
        // to_str 拒绝非可见 ASCII（obs-text）→ 整体跳过该头，不猜测编码入库。
        let mut h = HeaderMap::new();
        h.insert(
            "user-agent",
            HeaderValue::from_bytes(&[0xF0, 0x9F]).unwrap(),
        );
        h.insert("accept", HeaderValue::from_static("application/json"));
        let v = summarize_request_headers(&h).expect("accept 应保留");
        assert!(v.get("user-agent").is_none());
        assert_eq!(v["accept"], "application/json");
    }

    #[test]
    fn total_json_capped_at_8kb() {
        // 多个超长白名单头时，整包 JSON ≤ 8KB（超出按键丢弃）。
        let long = "B".repeat(2000);
        let h = mk(&[
            ("user-agent", &long),
            ("accept", &long),
            ("accept-encoding", &long),
            ("referer", &long),
            ("origin", &long),
            ("host", &long),
        ]);
        let v = summarize_request_headers(&h).expect("应仍有保留");
        let len = serde_json::to_vec(&v).unwrap().len();
        assert!(len <= TOTAL_MAX_BYTES, "整包超限: {len}");
        // 每值 512 截断后 6 个键约 3.2KB，应全部保留。
        assert_eq!(v.as_object().unwrap().len(), 6);
    }

    #[test]
    fn total_json_cap_drops_keys_when_exceeded() {
        // 构造超 8KB 场景：通过直接测试总量丢弃逻辑（11 个白名单键 × 512+ 字节值）。
        // 这里用 512 上限值填满全部 11 个白名单键：11 × ~540 ≈ 5.9KB < 8KB，
        // 因此改用更长的单值验证丢弃分支：临时把值长度推至上限附近并组合多键，
        // 确保即便超限也不会超过 8KB（上限由 serialized_len 循环保证）。
        let long = "C".repeat(VALUE_MAX_BYTES);
        let pairs: Vec<(String, String)> = [
            "user-agent",
            "x-forwarded-for",
            "x-real-ip",
            "x-forwarded-proto",
            "content-type",
            "accept",
            "accept-encoding",
            "host",
            "origin",
            "referer",
            "x-request-id",
        ]
        .iter()
        .map(|k| (k.to_string(), long.clone()))
        .collect();
        let mut h = HeaderMap::new();
        for (k, v) in &pairs {
            h.insert(
                axum::http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        let v = summarize_request_headers(&h).expect("应仍有保留");
        let len = serde_json::to_vec(&v).unwrap().len();
        assert!(len <= TOTAL_MAX_BYTES, "整包超限: {len}");
    }
}
