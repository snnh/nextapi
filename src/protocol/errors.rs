//! 错误体翻译：上游错误 JSON ⇄ IR ⇄ 入口协议错误结构（PLAN.md §3.2/§5.5）。
//!
//! 原则：错误内容（message）尽量完整保留原文；结构翻译为入口协议格式。

use serde::{Deserialize, Serialize};

use super::ir::Protocol;

/// 归一化错误 IR。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrError {
    /// HTTP 状态码
    pub status: u16,
    /// 错误信息（尽量保留上游原文）
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// 解析上游错误体为 IrError。body 非 JSON 时整体作为 message。
pub fn error_to_ir(p: Protocol, status: u16, body: &serde_json::Value) -> IrError {
    let err = IrError {
        status,
        ..Default::default()
    };
    match p {
        // OpenAI 系：{"error": {"message","type","code"}}
        Protocol::OpenaiChat | Protocol::OpenaiResponses => {
            let e = &body["error"];
            IrError {
                message: e["message"]
                    .as_str()
                    .unwrap_or_else(|| body.as_str().unwrap_or(""))
                    .into(),
                r#type: e["type"].as_str().map(Into::into),
                code: e["code"]
                    .as_str()
                    .map(Into::into)
                    .or_else(|| e["code"].as_i64().map(|c| c.to_string())),
                ..err
            }
        }
        // Anthropic：{"type":"error","error":{"type","message"}}
        // body 非 JSON/形状不符时整体作为 message（与文档承诺一致）
        Protocol::Anthropic => {
            let e = &body["error"];
            IrError {
                message: e["message"]
                    .as_str()
                    .map(Into::into)
                    .unwrap_or_else(|| fallback_body_message(body)),
                r#type: e["type"].as_str().map(Into::into),
                ..err
            }
        }
        // Gemini：{"error":{"code":400,"message":"...","status":"INVALID_ARGUMENT"}}
        // body 非 JSON/形状不符时整体作为 message（与文档承诺一致）
        Protocol::Gemini => {
            let e = &body["error"];
            IrError {
                message: e["message"]
                    .as_str()
                    .map(Into::into)
                    .unwrap_or_else(|| fallback_body_message(body)),
                r#type: e["status"].as_str().map(Into::into),
                code: e["code"].as_i64().map(|c| c.to_string()),
                ..err
            }
        }
    }
}

/// body 无法按预期形状取 message 时的兜底：字符串体整体作为 message；
/// 其余 JSON 值序列化保留（截断/脱敏由网关层负责）。
fn fallback_body_message(body: &serde_json::Value) -> String {
    match body.as_str() {
        Some(s) => s.to_string(),
        None if body.is_null() => String::new(),
        None => body.to_string(),
    }
}

/// 将 IrError 翻译为目标协议的错误结构。
pub fn error_from_ir(p: Protocol, e: &IrError) -> serde_json::Value {
    match p {
        Protocol::OpenaiChat => serde_json::json!({
            "error": {
                "message": e.message,
                "type": e.r#type.as_deref().unwrap_or("server_error"),
                "code": e.code,
            }
        }),
        Protocol::OpenaiResponses => serde_json::json!({
            "error": {
                "message": e.message,
                "type": e.r#type.as_deref().unwrap_or("server_error"),
                "code": e.code,
            }
        }),
        Protocol::Anthropic => serde_json::json!({
            "type": "error",
            "error": {
                "type": e.r#type.as_deref().unwrap_or("api_error"),
                "message": e.message,
            }
        }),
        Protocol::Gemini => serde_json::json!({
            "error": {
                "code": e.status,
                "message": e.message,
                "status": gemini_status(e),
            }
        }),
    }
}

/// Gemini 错误 `status` 必须是 gRPC 状态枚举（SNAKE_UPPER）。
/// r#type 已是合法枚举形态则保真；否则（如 Anthropic 的 overloaded_error）按 HTTP 状态码映射。
fn gemini_status(e: &IrError) -> String {
    if let Some(t) = e.r#type.as_deref() {
        if !t.is_empty() && t.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
            return t.to_string();
        }
    }
    match e.status {
        400 => "INVALID_ARGUMENT",
        401 => "UNAUTHENTICATED",
        403 => "PERMISSION_DENIED",
        404 => "NOT_FOUND",
        408 | 504 => "DEADLINE_EXCEEDED",
        409 => "ABORTED",
        413 | 429 => "RESOURCE_EXHAUSTED",
        499 => "CANCELLED",
        501 => "UNIMPLEMENTED",
        503 | 529 => "UNAVAILABLE",
        s if s >= 500 => "INTERNAL",
        _ => "UNKNOWN",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_error_roundtrip() {
        let body = serde_json::json!({"error":{"message":"Invalid key","type":"auth_error","code":"401_x"}});
        let e = error_to_ir(Protocol::OpenaiChat, 401, &body);
        assert_eq!(e.message, "Invalid key");
        let out = error_from_ir(Protocol::Anthropic, &e);
        assert_eq!(out["type"], "error");
        assert_eq!(out["error"]["message"], "Invalid key");
    }

    #[test]
    fn gemini_error_parse() {
        let body = serde_json::json!({"error":{"code":429,"message":"Quota exceeded","status":"RESOURCE_EXHAUSTED"}});
        let e = error_to_ir(Protocol::Gemini, 429, &body);
        assert_eq!(e.message, "Quota exceeded");
        assert_eq!(e.code.as_deref(), Some("429"));
        let out = error_from_ir(Protocol::OpenaiChat, &e);
        assert_eq!(out["error"]["message"], "Quota exceeded");
    }

    #[test]
    fn anthropic_error_parse() {
        let body = serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}});
        let e = error_to_ir(Protocol::Anthropic, 529, &body);
        assert_eq!(e.message, "Overloaded");
        let out = error_from_ir(Protocol::Gemini, &e);
        assert_eq!(out["error"]["message"], "Overloaded");
        // Anthropic 的 type 非合法 gRPC 枚举 → 按 HTTP 状态映射（review P5）
        assert_eq!(out["error"]["status"], "UNAVAILABLE");
    }

    #[test]
    fn error_non_json_body_fallback() {
        // body 非预期形状（纯文本/空对象）时 message 取 body 整体而非空串（review P5）
        let plain = serde_json::json!("upstream exploded");
        let e = error_to_ir(Protocol::Anthropic, 502, &plain);
        assert_eq!(e.message, "upstream exploded");
        let e2 = error_to_ir(Protocol::Gemini, 502, &plain);
        assert_eq!(e2.message, "upstream exploded");
        let odd = serde_json::json!({"detail":"boom"});
        let e3 = error_to_ir(Protocol::Anthropic, 500, &odd);
        assert_eq!(e3.message, "{\"detail\":\"boom\"}");
        // Gemini 合法 gRPC 枚举的 type 保真
        let g = error_to_ir(
            Protocol::Gemini,
            429,
            &serde_json::json!({"error":{"code":429,"message":"q","status":"RESOURCE_EXHAUSTED"}}),
        );
        let out = error_from_ir(Protocol::Gemini, &g);
        assert_eq!(out["error"]["status"], "RESOURCE_EXHAUSTED");
    }
}
