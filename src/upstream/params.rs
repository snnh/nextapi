//! 上游渠道参数整形。
//!
//! 同一协议下的不同渠道对「思考强度 / 思考开关 / 采样参数」的接受形状并不一致：
//! 例如 OpenAI 兼容渠道里，Qwen 系认 `enable_thinking`，Kimi K3 只收
//! `reasoning_effort`，DeepSeek 用 `thinking{type}` + `reasoning_effort`；
//! Gemini 3.6 起忽略 `temperature/top_p/top_k`，部分渠道还要求
//! `thinkingConfig.includeThoughts`。
//!
//! 协议层只产出「目标协议的规范形状」，渠道差异由本模块按
//! `upstreams.extra.params` 声明改写，覆盖透传与转换两条路径。
//! 未配置任何字段时不做任何改动（保持协议层输出原样）。

use serde_json::{Map, Value};

use crate::protocol::ir::Protocol;

/// 渠道参数整形声明（`upstreams.extra.params`）。
///
/// 全部字段可选：未声明的维度不处理。
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ParamStyle {
    /// 思考参数形状：
    /// - `thinking`：按协议规范形状发（`thinking{type}` + `reasoning_effort`），默认
    /// - `enable_thinking`：`enable_thinking: bool`（Qwen/DashScope、部分国内兼容渠道）
    /// - `effort_only`：只发思考强度，删除 `thinking`（如 Kimi K3）
    /// - `fixed`：思考恒开，写 `thinking: {type: "enabled"}`
    /// - `extended`：Anthropic 旧形状 `thinking: {type: "enabled", budget_tokens: N}`
    /// - `adaptive`：Anthropic 新形状 `thinking: {type: "adaptive"}`
    /// - `none`：删除一切思考参数（含 Gemini `thinkingConfig`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_style: Option<String>,
    /// 思考强度的落点（点路径，如 `reasoning_effort`、`output_config.effort`、
    /// `generationConfig.thinkingConfig.thinkingLevel`）。缺省沿用协议规范落点。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_field: Option<String>,
    /// 需要剔除的字段点路径（如 Gemini 3.6+ 的 `generationConfig.temperature`）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strip: Vec<String>,
    /// `max_tokens` 的字段名（Chat 协议）：`max_tokens`（默认）/ `max_completion_tokens`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_field: Option<String>,
    /// `thinking_style = extended` 时的思考预算。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u64>,
    /// Gemini：是否回传思考内容（`thinkingConfig.includeThoughts`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
}

impl ParamStyle {
    /// 是否为空声明（空则跳过整形）。
    pub fn is_noop(&self) -> bool {
        self.thinking_style.is_none()
            && self.effort_field.is_none()
            && self.strip.is_empty()
            && self.max_tokens_field.is_none()
            && self.thinking_budget.is_none()
            && self.include_thoughts.is_none()
    }
}

/// 各协议下思考强度的规范落点（取第一个有值的）。
const EFFORT_PATHS: [&str; 4] = [
    "reasoning_effort",
    "reasoning.effort",
    "output_config.effort",
    "generationConfig.thinkingConfig.thinkingLevel",
];

/// 按渠道声明改写请求体。`body` 非对象时原样返回。
pub fn apply(body: &mut Value, protocol: Protocol, style: &ParamStyle) {
    if style.is_noop() || !body.is_object() {
        return;
    }

    // 1. 剔除渠道不接受的字段
    for path in &style.strip {
        remove_path(body, path);
    }

    // 2. 思考强度改投到渠道字段
    if let Some(field) = &style.effort_field {
        if let Some(v) = take_effort(body) {
            set_path(body, field, v);
        }
    }

    // 3. 思考开关形状
    if let Some(kind) = style.thinking_style.as_deref() {
        apply_thinking_style(body, protocol, kind, style);
    }

    // 4. max_tokens 字段名（Chat）
    if let Some(field) = &style.max_tokens_field {
        if protocol == Protocol::OpenaiChat
            && field != "max_tokens"
            && field != "max_completion_tokens"
        {
            for k in ["max_tokens", "max_completion_tokens"] {
                if let Some(v) = body.get(k).cloned() {
                    remove_path(body, k);
                    set_path(body, field, v);
                }
            }
        } else if protocol == Protocol::OpenaiChat {
            for k in ["max_tokens", "max_completion_tokens"] {
                if k != field {
                    if let Some(v) = body.get(k).cloned() {
                        remove_path(body, k);
                        set_path(body, field, v);
                    }
                }
            }
        }
    }

    // 5. Gemini 思考内容回传
    if let Some(inc) = style.include_thoughts {
        if protocol == Protocol::Gemini {
            set_path(
                body,
                "generationConfig.thinkingConfig.includeThoughts",
                Value::Bool(inc),
            );
        }
    }
}

/// 按 `thinking_style` 写思考开关字段。
fn apply_thinking_style(body: &mut Value, protocol: Protocol, kind: &str, style: &ParamStyle) {
    match kind {
        "none" => {
            remove_path(body, "thinking");
            remove_path(body, "enable_thinking");
            remove_path(body, "generationConfig.thinkingConfig");
        }
        "enable_thinking" => {
            if protocol != Protocol::OpenaiChat {
                // enable_thinking 是 OpenAI 兼容渠道的私有字段，其他协议不适用
                return;
            }
            let disabled = body["thinking"]["type"] == "disabled";
            remove_path(body, "thinking");
            set_path(body, "enable_thinking", Value::Bool(!disabled));
        }
        "effort_only" => {
            remove_path(body, "thinking");
            remove_path(body, "enable_thinking");
        }
        "fixed" | "extended" | "adaptive" if protocol != Protocol::Gemini => {
            let mut t = Map::new();
            match kind {
                "adaptive" => {
                    t.insert("type".into(), Value::String("adaptive".into()));
                }
                "extended" => {
                    t.insert("type".into(), Value::String("enabled".into()));
                    if let Some(b) = style.thinking_budget {
                        t.insert("budget_tokens".into(), Value::from(b));
                    }
                }
                _ => {
                    t.insert("type".into(), Value::String("enabled".into()));
                }
            }
            remove_path(body, "enable_thinking");
            set_path(body, "thinking", Value::Object(t));
        }
        // "thinking" 与其它未知值：保留协议层输出
        _ => {}
    }
}

/// 取走思考强度值（按协议规范落点顺序查找，命中后删除原字段）。
fn take_effort(body: &mut Value) -> Option<Value> {
    for path in EFFORT_PATHS {
        if let Some(v) = get_path(body, path).cloned() {
            remove_path(body, path);
            return Some(v);
        }
    }
    None
}

/// 读取点路径。空段跳过；不存在返回 None。
pub fn get_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = root;
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

/// 写入点路径（中间对象不存在则创建）。
pub fn set_path(root: &mut Value, path: &str, value: Value) {
    let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return;
    }
    let mut cur = root;
    for (i, seg) in segs.iter().enumerate() {
        if i == segs.len() - 1 {
            if !cur.is_object() {
                *cur = Value::Object(Map::new());
            }
            if let Some(o) = cur.as_object_mut() {
                o.insert((*seg).to_string(), value);
            }
            return;
        }
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        let Some(o) = cur.as_object_mut() else {
            return;
        };
        cur = o
            .entry((*seg).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}

/// 删除点路径（末级对象为空则一并删除父级空对象）。
pub fn remove_path(root: &mut Value, path: &str) {
    let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return;
    }
    remove_segs(root, &segs);
}

fn remove_segs(cur: &mut Value, segs: &[&str]) -> bool {
    if segs.len() == 1 {
        if let Some(o) = cur.as_object_mut() {
            o.remove(segs[0]);
            return o.is_empty();
        }
        return false;
    }
    let empty = match cur.as_object_mut().and_then(|o| o.get_mut(segs[0])) {
        Some(child) => remove_segs(child, &segs[1..]),
        None => false,
    };
    if empty {
        if let Some(o) = cur.as_object_mut() {
            o.remove(segs[0]);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn style(v: Value) -> ParamStyle {
        serde_json::from_value(v).expect("声明解析失败")
    }

    #[test]
    fn noop_when_empty() {
        let mut body = json!({"model":"m","temperature":0.7});
        let before = body.clone();
        apply(&mut body, Protocol::OpenaiChat, &ParamStyle::default());
        assert_eq!(body, before);
    }

    #[test]
    fn enable_thinking_switches_shape() {
        let mut body =
            json!({"model":"m","thinking":{"type":"disabled"},"reasoning_effort":"high"});
        apply(
            &mut body,
            Protocol::OpenaiChat,
            &style(json!({"thinking_style":"enable_thinking"})),
        );
        assert_eq!(body["enable_thinking"], json!(false));
        assert!(body.get("thinking").is_none());
        // 强度字段不受形状切换影响
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn enable_thinking_defaults_on_without_thinking_block() {
        let mut body = json!({"model":"m","reasoning_effort":"low"});
        apply(
            &mut body,
            Protocol::OpenaiChat,
            &style(json!({"thinking_style":"enable_thinking"})),
        );
        assert_eq!(body["enable_thinking"], json!(true));
    }

    #[test]
    fn effort_only_drops_thinking_block() {
        let mut body =
            json!({"model":"m","thinking":{"type":"enabled"},"reasoning_effort":"medium"});
        apply(
            &mut body,
            Protocol::OpenaiChat,
            &style(json!({"thinking_style":"effort_only"})),
        );
        assert!(body.get("thinking").is_none());
        assert_eq!(body["reasoning_effort"], json!("medium"));
    }

    #[test]
    fn effort_field_moves_value_and_drops_source() {
        let mut body = json!({"model":"m","reasoning_effort":"xhigh"});
        apply(
            &mut body,
            Protocol::OpenaiChat,
            &style(json!({"effort_field":"output_config.effort"})),
        );
        assert_eq!(body["output_config"]["effort"], json!("xhigh"));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn effort_field_fills_empty_target() {
        let mut body = json!({"model":"m"});
        apply(
            &mut body,
            Protocol::OpenaiChat,
            &style(json!({"effort_field":"reasoning_effort"})),
        );
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn adaptive_and_extended_shapes() {
        let mut body = json!({"model":"m"});
        apply(
            &mut body,
            Protocol::Anthropic,
            &style(json!({"thinking_style":"adaptive"})),
        );
        assert_eq!(body["thinking"]["type"], json!("adaptive"));

        let mut body = json!({"model":"m"});
        apply(
            &mut body,
            Protocol::Anthropic,
            &style(json!({"thinking_style":"extended","thinking_budget":32000})),
        );
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["thinking"]["budget_tokens"], json!(32000));
    }

    #[test]
    fn strip_removes_sampling_params() {
        let mut body =
            json!({"generationConfig":{"temperature":0.7,"topP":0.9,"maxOutputTokens":100}});
        apply(
            &mut body,
            Protocol::Gemini,
            &style(json!({"strip":["generationConfig.temperature","generationConfig.topP"]})),
        );
        assert!(body["generationConfig"].get("temperature").is_none());
        assert!(body["generationConfig"].get("topP").is_none());
        assert_eq!(body["generationConfig"]["maxOutputTokens"], json!(100));
    }

    #[test]
    fn max_tokens_field_renames_once() {
        let mut body = json!({"model":"m","max_tokens":512});
        apply(
            &mut body,
            Protocol::OpenaiChat,
            &style(json!({"max_tokens_field":"max_completion_tokens"})),
        );
        assert_eq!(body["max_completion_tokens"], json!(512));
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn include_thoughts_only_for_gemini() {
        let mut body = json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":"high"}}});
        apply(
            &mut body,
            Protocol::Gemini,
            &style(json!({"include_thoughts":true})),
        );
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["includeThoughts"],
            json!(true)
        );
        // 非 Gemini 协议不写（避免污染请求体）
        let mut other = json!({"model":"m"});
        apply(
            &mut other,
            Protocol::OpenaiChat,
            &style(json!({"include_thoughts":true})),
        );
        assert!(other.get("generationConfig").is_none());
    }

    #[test]
    fn thinking_style_none_clears_gemini_config() {
        let mut body = json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":"high"},"maxOutputTokens":8}});
        apply(
            &mut body,
            Protocol::Gemini,
            &style(json!({"thinking_style":"none"})),
        );
        assert!(body["generationConfig"].get("thinkingConfig").is_none());
        assert_eq!(body["generationConfig"]["maxOutputTokens"], json!(8));
    }

    #[test]
    fn remove_path_prunes_empty_parent() {
        let mut body = json!({"output_config":{"effort":"high"},"model":"m"});
        remove_path(&mut body, "output_config.effort");
        assert!(body.get("output_config").is_none());
        assert_eq!(body["model"], json!("m"));
    }
}
