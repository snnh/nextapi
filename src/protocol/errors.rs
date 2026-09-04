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
        Protocol::Anthropic => {
            let e = &body["error"];
            IrError {
                message: e["message"].as_str().unwrap_or_default().into(),
                r#type: e["type"].as_str().map(Into::into),
                ..err
            }
        }
        // Gemini：{"error":{"code":400,"message":"...","status":"INVALID_ARGUMENT"}}
        Protocol::Gemini => {
            let e = &body["error"];
            IrError {
                message: e["message"].as_str().unwrap_or_default().into(),
                r#type: e["status"].as_str().map(Into::into),
                code: e["code"].as_i64().map(|c| c.to_string()),
                ..err
            }
        }
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
                "status": e.r#type.as_deref().unwrap_or("INTERNAL"),
            }
        }),
    }
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
        assert_eq!(out["error"]["status"], "overloaded_error");
    }
}
