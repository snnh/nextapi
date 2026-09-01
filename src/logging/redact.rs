//! debug_payload 脱敏 + 截断（PLAN §5.5）。契约 contracts/m4-logging.md §6，M4-A 实现。
//!
//! 规则（JSON 递归遍历）：
//! - key ∈ {"data","b64_json","image","base64","file","file_data"} 且字符串值长度 > 256 → "[redacted]"；
//! - 任何以 "data:" 开头的字符串值（任意层级/任意 key）→ "[redacted]"；
//! - 非 JSON 输入按字符串处理；
//! - 序列化后 > max_bytes → 截断为 String 并 truncated=true。

use serde_json::Value;

/// 敏感 key 白名单（命中且为长字符串时脱敏）。
fn is_redact_key(k: &str) -> bool {
    matches!(k, "data" | "b64_json" | "image" | "base64" | "file" | "file_data")
}

/// 递归遍历并脱敏。字符串值以 "data:" 开头 → "[redacted]"；对象 key 命中敏感集合且字符串
/// 长度 > 256 → "[redacted]"。
fn redact_value(v: &Value) -> Value {
    match v {
        Value::String(s) if s.starts_with("data:") => Value::String("[redacted]".into()),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                let new_val = if is_redact_key(k) {
                    if let Value::String(s) = val {
                        // key 命中 + 字符串长度 > 256 → 脱敏（data: 已在上层红点处理）
                        if s.chars().count() > 256 {
                            Value::String("[redacted]".into())
                        } else {
                            redact_value(val)
                        }
                    } else {
                        redact_value(val)
                    }
                } else {
                    redact_value(val)
                };
                out.insert(k.clone(), new_val);
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(redact_value).collect()),
        other => other.clone(),
    }
}

/// 将字符串按字节上限截断（不会切断 UTF-8 字符）。
fn truncate_to_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// 规则：JSON 递归遍历——key 命中敏感集合且字符串 > 256 → "[redacted]"；data: 前缀字符串 →
/// "[redacted]"；非 JSON 输入按字符串处理。序列化后 > max_bytes → 截断为 String 并 truncated=true。
pub fn redact_and_truncate(body: &[u8], max_bytes: usize) -> (Value, bool) {
    let value: Value = match serde_json::from_slice(body) {
        Ok(v) => redact_value(&v),
        Err(_) => {
            // 非 JSON（如图片二进制等）→ 按字符串处理
            Value::String(String::from_utf8_lossy(body).into_owned())
        }
    };

    // 序列化后超限 → 截断为字符串并标记 truncated
    let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    if serialized.len() > max_bytes {
        (Value::String(truncate_to_bytes(&serialized, max_bytes)), true)
    } else {
        (value, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_data_uri_strings_anywhere() {
        // 对象值与数组元素中的 data: 字符串都应收敛为 [redacted]
        let body = br#"{"image":"data:image/png;base64,AAAA","items":["data:text/plain,hello",{"logo":"data:image/gif;base64,BB"}]}"#;
        let (v, truncated) = redact_and_truncate(body, 1024);
        assert!(!truncated);
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("[redacted]"));
        assert!(!s.contains("data:image"));
        assert!(!s.contains("data:text"));
    }

    #[test]
    fn redacts_long_base64_under_sensitive_key() {
        // key 命中 base64 且字符串 > 256 → [redacted]
        let long = "A".repeat(300);
        let body = format!(r#"{{"base64":"{long}"}}"#).into_bytes();
        let (v, _) = redact_and_truncate(&body, 4096);
        assert_eq!(v["base64"], Value::String("[redacted]".into()));
    }

    #[test]
    fn keeps_short_value_under_sensitive_key() {
        // 短字符串不脱敏（不达到 256 阈值）
        let body = br#"{"data":"short"}"#;
        let (v, _) = redact_and_truncate(body, 4096);
        assert_eq!(v["data"], Value::String("short".into()));
    }

    #[test]
    fn preserves_other_keys_nested() {
        // 非敏感 key 的非 data: 值不受影响
        let body = br#"{"content":{"model":"gpt-4","temperature":0.2}}"#;
        let (v, _) = redact_and_truncate(body, 4096);
        assert!(v["content"]["model"].is_string());
        assert!(v["content"]["temperature"].is_number());
    }

    #[test]
    fn truncates_over_limit_and_flags() {
        // 超限 → 截断为 String 并 truncated=true
        let body = format!(r#"{{"a":{{"b":"{}"}}}}"#, "x".repeat(2000)).into_bytes();
        let (v, truncated) = redact_and_truncate(&body, 64);
        assert!(truncated);
        assert!(v.is_string());
        assert!(v.as_str().unwrap().len() <= 64);
    }

    #[test]
    fn non_json_treated_as_string() {
        let body = b"not json at all";
        let (v, truncated) = redact_and_truncate(body, 4096);
        assert!(v.is_string());
        assert!(!truncated);
    }

    #[test]
    fn truncation_respects_utf8_boundary() {
        // 多字节字符截断不破坏 UTF-8
        let body = "你好世界".repeat(100).into_bytes();
        let (v, truncated) = redact_and_truncate(&body, 10);
        assert!(truncated);
        let s = v.as_str().unwrap();
        assert!(std::str::from_utf8(s.as_bytes()).is_ok());
    }
}
