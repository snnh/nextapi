//! SSE 行解析与编码工具（协议无关）。
//!
//! SSE 透传/转换不是纯字节 pipe：网关按行解析 data 载荷，改写后重新编码（PLAN.md §3.2）。
//! 单行解析失败时该行原样放行，不中断流（由上层决定，解析器只负责切帧）。

/// SSE 帧解析器：增量喂入字节，产出完整的 data 载荷（可能一条事件多行 data，已按 SSE 规范拼接）。
#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    /// 当前事件累积的 data 行
    data_lines: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入字节块，返回本次解析出的完整事件 data 载荷列表。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line = self.buf.drain(..=pos).collect::<Vec<u8>>();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                // 空行 = 事件结束
                if !self.data_lines.is_empty() {
                    out.push(self.data_lines.join("\n"));
                    self.data_lines.clear();
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                // data: 后允许一个前导空格
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                self.data_lines.push(rest.to_string());
            }
            // 其他行（event:/id:/retry:/注释）忽略：转换以 data 载荷为准
        }
        out
    }

    /// 流结束时冲刷残余（无换行结尾的 data）。
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&self.buf)
                .trim_end_matches(['\n', '\r'])
                .to_string();
            self.buf.clear();
            if let Some(rest) = line.strip_prefix("data:") {
                self.data_lines
                    .push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
        }
        if !self.data_lines.is_empty() {
            out.push(self.data_lines.join("\n"));
            self.data_lines.clear();
        }
        out
    }
}

/// 编码单条 SSE 事件（data 载荷 + 空行）。
pub fn encode_event(data: &str) -> String {
    format!("data: {data}\n\n")
}

/// 编码单条 SSE 事件，并按 data 的 JSON `type` 补 `event:` 行。
///
/// 背景（全局 review）：Anthropic Messages / OpenAI Responses 流式规范以 `event:` 行分派事件，
/// 而事件名与其 data JSON 的 `type` 字段一致（如 `message_start` / `response.created`）；
/// OpenAI Chat / Gemini 的 data 无 `type`，按其规范不应输出 `event:` 行。
/// 透传重编码与转换出站统一走本函数：有 type 补事件名，无 type 退化为纯 data。
pub fn encode_typed_event(data: &str) -> String {
    let event: Option<String> = serde_json::from_str::<serde_json::Value>(data)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from))
        .filter(|t| !t.is_empty());
    match event {
        Some(name) => format!("event: {name}\ndata: {data}\n\n"),
        None => format!("data: {data}\n\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_split_across_feeds() {
        let mut p = SseParser::new();
        assert!(p.feed(b"data: {\"a\":").is_empty());
        let out = p.feed(b"1}\n\ndata: {\"b\":2}\n\n");
        assert_eq!(out, vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn multiline_data_joined() {
        let mut p = SseParser::new();
        let out = p.feed(b"data: line1\ndata: line2\n\n");
        assert_eq!(out, vec!["line1\nline2"]);
    }

    #[test]
    fn ignores_event_and_comment_lines() {
        let mut p = SseParser::new();
        let out = p.feed(b": ping\nevent: message\ndata: x\n\n");
        assert_eq!(out, vec!["x"]);
    }

    #[test]
    fn finish_flushes_trailing() {
        let mut p = SseParser::new();
        p.feed(b"data: tail");
        assert_eq!(p.finish(), vec!["tail"]);
    }

    #[test]
    fn typed_encode_adds_event_line_for_json_type() {
        // Anthropic/Responses 出站：data 带顶层 type → 输出 event: <type> 行
        assert_eq!(
            encode_typed_event(r#"{"type":"message_start","message":{}}"#),
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n"
        );
        assert_eq!(
            encode_typed_event(r#"{"type":"response.completed","response":{}}"#),
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        );
        // OpenAI Chat / Gemini：无 type → 纯 data
        assert_eq!(
            encode_typed_event(r#"{"choices":[{"delta":{"content":"hi"}}]}"#),
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n"
        );
        // 非 JSON（[DONE]、纯文本）→ 纯 data
        assert_eq!(encode_typed_event("[DONE]"), "data: [DONE]\n\n");
        // type 非字符串/为空 → 不输出 event 行
        assert_eq!(
            encode_typed_event(r#"{"type":123}"#),
            "data: {\"type\":123}\n\n"
        );
    }
}
