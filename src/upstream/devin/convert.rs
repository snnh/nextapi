//! Devin ↔ OpenAI Responses 双向转换。
//!
//! - 请求：Responses JSON body → GetChatMessageRequest protobuf（devin CLI 形态）
//! - 响应：GetChatMessageResponse 信封帧 → 标准 Responses SSE 事件 / 非流式 JSON
//!
//! 字段号全部来自一手逆向 + 真实流量实证（含 CLI exec 工具循环捕获），
//! 映射表见 `.owc/devin-ref/RESEARCH.md` §6/§7/§11。usage 口径（实测两轮自洽）：
//! f2 = 未缓存输入、f5 = 缓存命中、f3 = 输出（f6=6 常量忽略）。

use bytes::{Bytes, BytesMut};
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::proto::{self, put_double, put_msg, put_str, put_uint};
use crate::protocol::sse::{encode_typed_event, encode_typed_event_with_type};

/// f2 prompt 缺省（CLI 环境模拟关闭时）：调用方未带 instructions 时的极简身份提示。
pub const DEFAULT_PROMPT: &str = "You are Devin, an interactive command line agent from Cognition.";

/// 官方 CLI 头提示词（实测渲染版 17682B，取自 devin CLI 3000.11.1 的真实请求 f2，
/// 见 `.owc/devin-ref/field-0019_f2.txt`）。末行 "You are powered by <模型展示名>"
/// 按实际模型重渲染（见 [`render_cli_prompt`]）。
pub const CLI_HEAD_PROMPT: &str = include_str!("cli_prompt.txt");

/// 官方 CLI 注入的 <system_info> 环境块模板（实测 265B 形态；变量见 [`builtin_vars`]）。
pub const DEFAULT_SYSTEM_INFO: &str = "<system_info>\nThe following information is automatically generated context about your current environment.\nCurrent workspace directories:\n  {cwd} (cwd)\n\nPlatform: {platform}\nOS Version: {os_version}\nToday's date: {weekday}, {date}\n</system_info>";

/// 客户端标识（模拟 devin CLI 3000.11.1）。
const EXTENSION_NAME: &str = "devin-cli";
pub const EXTENSION_VERSION: &str = "3000.11.1";

// ---------------------------------------------------------------------------
// 确定性 id / 代号（同会话多次请求保持稳定，模拟 CLI 会话内轨迹/执行 id）
// ---------------------------------------------------------------------------

fn det_hash(seed: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(seed.as_bytes());
    h.finalize().into()
}

/// 由种子派生确定性 uuid 形态 id。
fn det_uuid(seed: &str) -> String {
    let d = det_hash(seed);
    let hex: String = d[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// 会话代号（形如 "glorious-orchestra"）。
fn codename(seed: &str) -> String {
    const ADJ: &[&str] = &[
        "amber", "ancient", "autumn", "bold", "brave", "bright", "calm", "clever", "crimson",
        "crystal", "curious", "dancing", "dawn", "diamond", "distant", "eager", "emerald",
        "fabled", "fancy", "gentle", "glorious", "golden", "happy", "hidden", "humble", "ivory",
        "joyful", "keen", "lively", "lucky", "lunar", "mellow", "misty", "noble", "proud", "quiet",
        "rapid", "rusty", "silent", "silver", "sleepy", "spring", "starry", "steady", "sunny",
        "velvet", "vivid", "wild",
    ];
    const NOUN: &[&str] = &[
        "anchor",
        "archive",
        "aurora",
        "badger",
        "beacon",
        "blossom",
        "breeze",
        "brook",
        "canyon",
        "cascade",
        "citadel",
        "comet",
        "compass",
        "coral",
        "crane",
        "creek",
        "delta",
        "dolphin",
        "ember",
        "falcon",
        "fjord",
        "forest",
        "fountain",
        "galaxy",
        "harbor",
        "horizon",
        "island",
        "jasper",
        "lantern",
        "marsh",
        "meadow",
        "meteor",
        "orchestra",
        "orchid",
        "otter",
        "pebble",
        "pillow",
        "quill",
        "raven",
        "river",
        "savanna",
        "solstice",
        "summit",
        "thicket",
        "thunder",
        "valley",
        "willow",
        "zephyr",
    ];
    let d = det_hash(seed);
    let a = ADJ[d[0] as usize % ADJ.len()];
    let n = NOUN[d[1] as usize % NOUN.len()];
    format!("{a}-{n}")
}

/// Metadata f31 设备指纹：随机 hex（实测 732 字符，逐请求变化）。
fn fingerprint() -> String {
    let mut raw = [0u8; 366];
    rand::rng().fill_bytes(&mut raw);
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

/// system_info 注入策略（`extra.system_info`）：
/// - 缺省 → 官方模板 [`DEFAULT_SYSTEM_INFO`]；`false` 或空串 → 不注入；
/// - 字符串 → 自定义模板（支持 `{变量}` 替换，未识别占位符原样保留）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SystemInfoSpec {
    #[default]
    Default,
    Off,
    Template(String),
}

/// 解析 `extra.system_info`（字符串 / `{template}` 对象 / false）。
pub fn system_info_spec(extra: &serde_json::Value) -> SystemInfoSpec {
    match extra.get("system_info") {
        None => SystemInfoSpec::Default,
        Some(v) if v.as_bool() == Some(false) => SystemInfoSpec::Off,
        Some(v) if v.as_str().is_some_and(|s| s.trim().is_empty()) => SystemInfoSpec::Off,
        Some(v) => match v.as_str() {
            Some(s) => SystemInfoSpec::Template(s.to_string()),
            None => v
                .get("template")
                .and_then(|t| t.as_str())
                .filter(|t| !t.trim().is_empty())
                .map(|t| SystemInfoSpec::Template(t.to_string()))
                .unwrap_or(SystemInfoSpec::Default),
        },
    }
}

/// 渲染 CLI 头提示词的模型展示名（`swe-1-6-slow` → `SWE-1.6 Slow`）。
/// 无法识别时返回 None（保留提示词原文）。
fn model_display_name(uid: &str) -> Option<String> {
    fn cap(s: &str) -> String {
        let mut ch = s.chars();
        match ch.next() {
            Some(first) => format!("{}{}", first.to_ascii_uppercase(), ch.as_str()),
            None => String::new(),
        }
    }
    let parts: Vec<&str> = uid
        .strip_prefix("swe-")?
        .split('-')
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() || !parts[0].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // swe-1-6-slow → SWE-1.6 Slow；swe-2-high → SWE-2 High；swe-2 → SWE-2
    let (mut name, mut tiers) = (format!("SWE-{}", parts[0]), &parts[1..]);
    if let Some(minor) = parts
        .get(1)
        .filter(|p| p.chars().all(|c| c.is_ascii_digit()))
    {
        name.push('.');
        name.push_str(minor);
        tiers = &parts[2..];
    }
    for tier in tiers {
        name.push(' ');
        name.push_str(&cap(tier));
    }
    Some(name)
}

/// 按模型渲染 CLI 头提示词（仅替换末行的模型展示名；识别不出则原样返回）。
pub fn render_cli_prompt(model_uid: &str) -> String {
    let Some(name) = model_display_name(model_uid) else {
        return CLI_HEAD_PROMPT.to_string();
    };
    match CLI_HEAD_PROMPT.rfind("You are powered by ") {
        Some(idx) => {
            let line_end = CLI_HEAD_PROMPT[idx..]
                .find('\n')
                .map(|off| idx + off)
                .unwrap_or(CLI_HEAD_PROMPT.len());
            format!(
                "{}You are powered by {name}.{}",
                &CLI_HEAD_PROMPT[..idx],
                &CLI_HEAD_PROMPT[line_end..]
            )
        }
        None => CLI_HEAD_PROMPT.to_string(),
    }
}

fn platform_name() -> String {
    std::env::consts::OS.to_string()
}

fn os_version() -> String {
    if cfg!(target_os = "linux") {
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|s| format!("Linux {}", s.trim()))
            .unwrap_or_else(|_| platform_name())
    } else {
        platform_name()
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// 模板内置变量（键均小写；调用方传来的同名变量覆盖之）。
fn builtin_vars(model_uid: &str) -> Vec<(String, String)> {
    let now = chrono::Local::now();
    vec![
        ("cwd".into(), "/workspace".into()),
        ("platform".into(), platform_name()),
        ("os_version".into(), os_version()),
        ("date".into(), now.format("%Y-%m-%d").to_string()),
        ("weekday".into(), now.format("%A").to_string()),
        (
            "datetime".into(),
            now.format("%Y-%m-%d %H:%M:%S").to_string(),
        ),
        ("hostname".into(), hostname()),
        ("arch".into(), std::env::consts::ARCH.to_string()),
        ("model".into(), model_uid.to_string()),
        (
            "model_display".into(),
            model_display_name(model_uid).unwrap_or_else(|| model_uid.to_string()),
        ),
        (
            "gateway_version".into(),
            env!("CARGO_PKG_VERSION").to_string(),
        ),
    ]
}

/// `{name}` 占位符替换：仅替换已知变量（大小写不敏感），未识别的原样保留。
fn render_vars(template: &str, vars: &[(String, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let name = &after[..close];
        let key = name.trim().to_ascii_lowercase();
        match vars.iter().find(|(k, _)| *k == key) {
            Some((_, v)) => out.push_str(v),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// 渲染 system_info 块；`Off` 返回 None。
pub fn render_system_info(
    spec: &SystemInfoSpec,
    model_uid: &str,
    caller_vars: &[(String, String)],
) -> Option<String> {
    let template = match spec {
        SystemInfoSpec::Off => return None,
        SystemInfoSpec::Default => DEFAULT_SYSTEM_INFO,
        SystemInfoSpec::Template(t) => t.as_str(),
    };
    let mut vars = builtin_vars(model_uid);
    for (k, v) in caller_vars {
        match vars.iter_mut().find(|(bk, _)| bk == k) {
            Some(slot) => slot.1 = v.clone(),
            None => vars.push((k.clone(), v.clone())),
        }
    }
    Some(render_vars(template, &vars))
}

/// 请求构造选项（上游 `extra` 解析结果 + 调用方传入变量）。
/// `Default` 与生产默认一致（CLI 环境模拟开启）。
#[derive(Debug, Clone)]
pub struct RequestOptions {
    /// CLI 环境模拟（`extra.cli_emulation`，默认开）：头提示词 + system_info 注入
    pub cli_emulation: bool,
    pub system_info: SystemInfoSpec,
    /// 调用方传入变量（键小写；请求头 X-System-Info-* 与请求体 metadata）
    pub vars: Vec<(String, String)>,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self::from_extra(&serde_json::Value::Null)
    }
}

impl RequestOptions {
    /// 从上游 `extra` 解析（调用方变量由调用方 push）。
    pub fn from_extra(extra: &serde_json::Value) -> Self {
        Self {
            cli_emulation: extra.get("cli_emulation").and_then(|v| v.as_bool()) != Some(false),
            system_info: system_info_spec(extra),
            vars: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// 请求：Responses JSON → GetChatMessageRequest
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Tc {
    call_id: String,
    name: String,
    args: String,
}

/// 归一后的会话消息（ChatMessagePrompt 三形态）。
///
/// 请求侧槽位与响应侧一一对应（实测 CLI 抓帧）：`f3` = 正文文本（user / assistant
/// 共用），`f11` = assistant 思维链（CLI 的 preserved_thinking），`f6` = tool_calls，
/// `f7` = tool_call_id。此前把 assistant 正文写进 f11，会被上游当思维链，已修正。
enum Msg {
    /// source=1：user / system（CLI 的 system_info 注入也是 user 源）
    User { text: String },
    /// source=2：assistant 正文（f3）+ 思维链（f11）+ 工具调用（f6）
    Assistant {
        text: String,
        thinking: String,
        calls: Vec<Tc>,
    },
    /// source=4：工具结果（f3 + f7 tool_call_id）
    Tool { call_id: String, text: String },
}

/// 取 message content 的纯文本（图片等多模态降级为占位文本）。
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut out = Vec::new();
            for p in parts {
                let ty = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match ty {
                    "input_text" | "output_text" | "text" | "summary_text" => {
                        if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                            out.push(t.to_string());
                        }
                    }
                    "input_image" | "output_image" | "image_url" | "input_file" => {
                        out.push("[图片/文件已省略：Devin 上游不支持多模态输入]".to_string());
                    }
                    _ => {}
                }
            }
            out.join("\n")
        }
        _ => String::new(),
    }
}

/// 从 Responses `reasoning` item 提取思维链文本（summary / content 段）。
/// encrypted_content 为上游不透传密文，忽略。
fn reasoning_text(item: &Value) -> String {
    let mut out = Vec::new();
    for key in ["summary", "content"] {
        if let Some(arr) = item.get(key).and_then(|v| v.as_array()) {
            for p in arr {
                if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                    if !t.is_empty() {
                        out.push(t.to_string());
                    }
                }
            }
        }
    }
    out.join("\n")
}

/// Responses `input`（string | items）→ 归一消息列表；连续 assistant 项（文本 +
/// function_call）合并为一条 ChatMessagePrompt（CLI 形态：f3 正文 + f6 工具 + f11 思维）。
/// `reasoning` 项（客户端回传的思维链）挂到紧随其后的 assistant 消息，走 f11。
fn input_to_messages(input: &Value) -> Result<Vec<Msg>, String> {
    let mut out: Vec<Msg> = Vec::new();
    let mut pending: Option<(String, String, Vec<Tc>)> = None;

    let flush = |pending: &mut Option<(String, String, Vec<Tc>)>, out: &mut Vec<Msg>| {
        if let Some((text, thinking, calls)) = pending.take() {
            out.push(Msg::Assistant {
                text,
                thinking,
                calls,
            });
        }
    };

    match input {
        Value::String(s) => out.push(Msg::User { text: s.clone() }),
        Value::Array(items) => {
            for item in items {
                let ty = item
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("message");
                match ty {
                    "message" | "" => {
                        let role = item.get("role").and_then(|r| r.as_str()).unwrap_or("user");
                        let text = content_text(item.get("content").unwrap_or(&Value::Null));
                        if role == "assistant" {
                            pending
                                .get_or_insert_with(Default::default)
                                .0
                                .push_str(&text);
                        } else {
                            flush(&mut pending, &mut out);
                            out.push(Msg::User { text });
                        }
                    }
                    "reasoning" => {
                        // 客户端回传的思维链：绑定到紧随其后的 assistant 消息（f11）
                        let think = reasoning_text(item);
                        if !think.is_empty() {
                            pending
                                .get_or_insert_with(Default::default)
                                .1
                                .push_str(&think);
                        }
                    }
                    "function_call" => {
                        let tc = Tc {
                            call_id: item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            name: item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            args: item
                                .get("arguments")
                                .and_then(|v| v.as_str())
                                .unwrap_or("{}")
                                .to_string(),
                        };
                        pending.get_or_insert_with(Default::default).2.push(tc);
                    }
                    "function_call_output" => {
                        flush(&mut pending, &mut out);
                        let out_v = item.get("output").unwrap_or(&Value::Null);
                        let text = match out_v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        out.push(Msg::Tool {
                            call_id: item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            text,
                        });
                    }
                    // 其它扩展项：Devin 无对应字段，跳过
                    _ => {}
                }
            }
            flush(&mut pending, &mut out);
        }
        _ => return Err("input 既非字符串也非数组".into()),
    }
    Ok(out)
}

/// 编码单条 ChatMessagePrompt。
fn encode_message(msg: &Msg, idx: usize, seed: &str) -> BytesMut {
    let mut b = BytesMut::new();
    put_str(&mut b, 1, &det_uuid(&format!("{seed}:msg:{idx}")));
    match msg {
        Msg::User { text } => {
            put_uint(&mut b, 2, 1);
            put_str(&mut b, 3, text);
        }
        Msg::Assistant {
            text,
            thinking,
            calls,
        } => {
            put_uint(&mut b, 2, 2);
            if !text.is_empty() {
                put_str(&mut b, 3, text);
            }
            if !thinking.is_empty() {
                put_str(&mut b, 11, thinking);
            }
            for c in calls {
                let mut sub = BytesMut::new();
                put_str(&mut sub, 1, &c.call_id);
                put_str(&mut sub, 2, &c.name);
                put_str(&mut sub, 3, &c.args);
                put_msg(&mut b, 6, &sub);
            }
        }
        Msg::Tool { call_id, text } => {
            put_uint(&mut b, 2, 4);
            put_str(&mut b, 3, text);
            put_str(&mut b, 7, call_id);
        }
    }
    b
}

/// 构造 GetChatMessageRequest 信封字节（单帧，无 end 帧——实测 CLI 形态）。
///
/// `body` 为出站 Responses JSON；`model_uid` 为上游实际模型名（f21）；
/// `token` 为 devin session token（Metadata f3 原文）。
/// 构造 GetChatMessageRequest 信封字节。`opts.cli_emulation` 为 true 时按官方 CLI 形态：
/// f2 = 头提示词（按模型渲染）为底 + 调用方 instructions 追加在后，并在会话首条注入
/// `<system_info>`（模板与变量见 [`SystemInfoSpec`] / [`builtin_vars`]）。
pub fn build_chat_request_ex(
    body: &Value,
    model_uid: &str,
    token: &str,
    opts: &RequestOptions,
) -> Result<Bytes, String> {
    // 会话种子：输入序列化 + 模型 → 同会话多轮请求派生 id 稳定
    let seed = format!("{model_uid}:{}", body.get("input").unwrap_or(&Value::Null));

    let mut messages = input_to_messages(body.get("input").unwrap_or(&Value::Null))?;
    let caller_prompt = body
        .get("instructions")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let prompt = if opts.cli_emulation {
        // CLI 形态（实测）：头提示词为底，调用方 system 提示词追加在后
        match caller_prompt {
            Some(extra) => format!("{}\n\n{extra}", render_cli_prompt(model_uid)),
            None => render_cli_prompt(model_uid),
        }
    } else {
        caller_prompt.unwrap_or(DEFAULT_PROMPT).to_string()
    };
    if opts.cli_emulation {
        // 官方 CLI 在会话首条注入环境信息（source=1）
        if let Some(text) = render_system_info(&opts.system_info, model_uid, &opts.vars) {
            messages.insert(0, Msg::User { text });
        }
    }

    let mut b = BytesMut::with_capacity(8 * 1024);

    // f1 Metadata（实测 devin-cli 形态）
    {
        let mut m = BytesMut::new();
        put_str(&mut m, 1, EXTENSION_NAME);
        put_str(&mut m, 2, EXTENSION_VERSION);
        put_str(&mut m, 3, token);
        put_str(&mut m, 4, "en");
        put_str(&mut m, 5, std::env::consts::OS);
        put_str(&mut m, 7, EXTENSION_VERSION);
        put_str(&mut m, 12, "chisel");
        put_str(&mut m, 28, "chisel");
        put_str(&mut m, 31, &fingerprint());
        put_msg(&mut b, 1, &m);
    }

    // f2 头提示词
    put_str(&mut b, 2, &prompt);

    // f3 会话消息
    for (i, msg) in messages.iter().enumerate() {
        let sub = encode_message(msg, i, &seed);
        put_msg(&mut b, 3, &sub);
    }

    // f7 request_type = 5（chat，实测）
    put_uint(&mut b, 7, 5);

    // f8 CompletionConfiguration（实测默认：1 / 128000 / 400 / temp 1.0 / top_k 40 / top_p 0.95）
    {
        let mut c = BytesMut::new();
        put_uint(&mut c, 1, 1);
        put_uint(
            &mut c,
            2,
            body.get("max_output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(128000),
        );
        put_uint(&mut c, 3, 400);
        put_double(
            &mut c,
            5,
            body.get("temperature")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0),
        );
        put_uint(&mut c, 7, 40);
        put_double(
            &mut c,
            8,
            body.get("top_p").and_then(|v| v.as_f64()).unwrap_or(0.95),
        );
        put_msg(&mut b, 8, &c);
    }

    // f10 工具定义（调用方工具；CLI 内置工具不注入）
    if let Some(tools) = body.get("tools").and_then(|v| v.as_array()) {
        for t in tools {
            if t.get("type").and_then(|v| v.as_str()) != Some("function") {
                continue;
            }
            // 兼容 Responses 扁平形态与 Chat 嵌套形态
            let f = t.get("function").unwrap_or(t);
            let Some(name) = f.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let mut sub = BytesMut::new();
            put_str(&mut sub, 1, name);
            if let Some(desc) = f.get("description").and_then(|v| v.as_str()) {
                put_str(&mut sub, 2, desc);
            }
            let params = f
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            put_str(
                &mut sub,
                3,
                &serde_json::to_string(&params).unwrap_or_else(|_| "{}".into()),
            );
            put_msg(&mut b, 10, &sub);
        }
    }

    // f15 trajectory_reference（会话内稳定；f2 = 轮次）
    let step = messages
        .iter()
        .filter(|m| matches!(m, Msg::Assistant { .. }))
        .count() as u64
        + 1;
    {
        let mut t = BytesMut::new();
        put_str(&mut t, 1, &det_uuid(&format!("{seed}:traj")));
        put_uint(&mut t, 2, step);
        put_uint(&mut t, 3, 4);
        put_msg(&mut b, 15, &t);
    }

    // f16 execution uuid / f20 planner_mode / f21 模型 / f28 会话代号
    put_str(&mut b, 16, &det_uuid(&format!("{seed}:exec")));
    put_uint(&mut b, 20, 1);
    put_str(&mut b, 21, model_uid);
    put_str(&mut b, 28, &codename(&seed));

    Ok(proto::envelope(&b))
}

// ---------------------------------------------------------------------------
// 响应：GetChatMessageResponse 帧 → Responses SSE / 非流式 JSON
// ---------------------------------------------------------------------------

/// 帧内 usage 快照（f7，逐帧滚动；取末帧）。
#[derive(Debug, Clone, Copy, Default)]
pub struct DevinUsage {
    /// f2 未缓存输入
    pub input_uncached: u64,
    /// f5 缓存命中
    pub cache_read: u64,
    /// f3 输出
    pub output: u64,
}

struct FnItem {
    call_id: String,
    name: String,
    args: String,
    item_id: String,
    output_index: u64,
}

/// Devin 响应流 → Responses 事件状态机（流式 SSE 与非流式聚合共用）。
///
/// 两条文本流（实测：真实 CLI 会话经本地中转抓帧 + CLI 会话库对照）：
/// - `f3`（伴随 `f4` 分段标记）= **回答正文** → Responses message / output_text；
/// - `f9` = **思维链（CoT）** → Responses reasoning item / reasoning_summary_text。
///
/// 官方 CLI 把 f3 存为消息正文、把 f11（请求侧同槽位）作为 preserved_thinking
/// 回传，故 f3/f9 语义与请求侧 f3/f11 一一对应，不可互换。
pub struct StreamConv {
    model: String,
    resp_id: String,
    created_at: i64,
    /// 思维链（响应 f9）→ reasoning item
    reasoning_item_id: String,
    reasoning_index: u64,
    reasoning_added: bool,
    reasoning_text: String,
    /// 回答正文（响应 f3）→ message item
    msg_item_id: String,
    msg_index: u64,
    created: bool,
    msg_added: bool,
    msg_text: String,
    /// 下一个 output_index（reasoning / message / function_call 按出现顺序分配）
    next_index: u64,
    calls: Vec<FnItem>,
    usage: Option<DevinUsage>,
    finished: bool,
}

impl StreamConv {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            resp_id: String::new(),
            created_at: chrono::Utc::now().timestamp(),
            reasoning_item_id: format!("rs_{}", uuid::Uuid::new_v4().simple()),
            reasoning_index: 0,
            reasoning_added: false,
            reasoning_text: String::new(),
            msg_item_id: format!("msg_{}", uuid::Uuid::new_v4().simple()),
            msg_index: 0,
            created: false,
            msg_added: false,
            msg_text: String::new(),
            next_index: 0,
            calls: Vec::new(),
            usage: None,
            finished: false,
        }
    }

    /// 是否已输出任何事件（决定错误映射为「可重试」还是「流中失败」）。
    pub fn started(&self) -> bool {
        self.created
    }

    /// 喂入一个数据帧（flags=0 payload），产出已编码 SSE 事件。
    pub fn feed(&mut self, payload: &[u8]) -> Result<Vec<String>, String> {
        let fields = proto::decode(payload)?;
        let mut out = Vec::new();

        // 元数据字段先行解析（resp id / 时间戳），再发 response.created
        for (f, v) in &fields {
            match f {
                1 => {
                    if self.resp_id.is_empty() {
                        if let Some(s) = v.as_str() {
                            let uuid = s.strip_prefix("bot-").unwrap_or(s);
                            self.resp_id = format!("resp_{uuid}");
                        }
                    }
                }
                2 => {
                    if let Some(b) = v.as_bytes() {
                        if let Ok(inner) = proto::decode(b) {
                            if let Some(sec) = proto::get(&inner, 1).and_then(|x| x.as_uint()) {
                                self.created_at = sec as i64;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if !self.created {
            self.created = true;
            out.push(self.event_created());
        }

        let mut stop: Option<u64> = None;
        for (f, v) in &fields {
            match f {
                5 => stop = v.as_uint(),
                6 => {
                    if let Some(b) = v.as_bytes() {
                        out.extend(self.feed_tool_delta(b)?);
                    }
                }
                7 => {
                    if let Some(b) = v.as_bytes() {
                        self.feed_usage(b);
                    }
                }
                9 => {
                    // 思维链增量（f9）
                    if let Some(s) = v.as_str() {
                        out.extend(self.feed_thinking(s));
                    }
                }
                3 => {
                    // 回答正文增量（f3，f4 为分段标记，忽略）
                    if let Some(s) = v.as_str() {
                        out.extend(self.feed_text(s));
                    }
                }
                _ => {}
            }
        }

        // f5 stop_reason（2=文本结束 / 10=函数调用 FUNCTION_CALL）仅作存在性判据：
        // 不能在本帧收尾——实测 stop 帧后仍有滚动 usage 帧（终值在流末），
        // 终止事件统一延迟到流末（EOF / end 帧）由 finish() 发出，保证 usage 取到终值。
        let _ = stop;
        Ok(out)
    }

    /// 回答正文增量（f3）→ message item / output_text.delta。
    fn feed_text(&mut self, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        if !self.msg_added {
            self.msg_added = true;
            self.msg_index = self.next_index;
            self.next_index += 1;
            out.push(self.event_item_added_message());
        }
        self.msg_text.push_str(text);
        out.push(encode_typed_event_with_type(
            &json!({
                "type": "response.output_text.delta",
                "item_id": self.msg_item_id,
                "output_index": self.msg_index,
                "content_index": 0,
                "delta": text,
            })
            .to_string(),
            Some("response.output_text.delta"),
        ));
        out
    }

    /// 思维链增量（f9）→ reasoning item / reasoning_summary_text.delta。
    fn feed_thinking(&mut self, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        if !self.reasoning_added {
            self.reasoning_added = true;
            self.reasoning_index = self.next_index;
            self.next_index += 1;
            out.push(self.event_item_added_reasoning());
        }
        self.reasoning_text.push_str(text);
        out.push(encode_typed_event_with_type(
            &json!({
                "type": "response.reasoning_summary_text.delta",
                "item_id": self.reasoning_item_id,
                "output_index": self.reasoning_index,
                "summary_index": 0,
                "delta": text,
            })
            .to_string(),
            Some("response.reasoning_summary_text.delta"),
        ));
        out
    }

    fn feed_tool_delta(&mut self, payload: &[u8]) -> Result<Vec<String>, String> {
        let fields = proto::decode(payload)?;
        let call_id = proto::get(&fields, 1)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let name = proto::get(&fields, 2)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let frag = proto::get(&fields, 3)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut out = Vec::new();

        let idx = if !call_id.is_empty() {
            match self.calls.iter().position(|c| c.call_id == call_id) {
                Some(i) => i,
                None => {
                    let output_index = self.next_index;
                    self.next_index += 1;
                    self.calls.push(FnItem {
                        call_id: call_id.clone(),
                        name,
                        args: String::new(),
                        item_id: format!("fc_{}", uuid::Uuid::new_v4().simple()),
                        output_index,
                    });
                    let i = self.calls.len() - 1;
                    out.push(self.event_item_added_fn(i));
                    i
                }
            }
        } else if let Some(i) = self.calls.len().checked_sub(1) {
            i
        } else {
            return Ok(out); // 无宿主调用的孤立参数增量：丢弃
        };

        if !frag.is_empty() {
            self.calls[idx].args.push_str(&frag);
            let c = &self.calls[idx];
            out.push(encode_typed_event(
                &json!({
                    "type": "response.function_call_arguments.delta",
                    "item_id": c.item_id,
                    "output_index": c.output_index,
                    "delta": frag,
                })
                .to_string(),
            ));
        }
        Ok(out)
    }

    fn feed_usage(&mut self, payload: &[u8]) {
        let Ok(fields) = proto::decode(payload) else {
            return;
        };
        let u = self.usage.get_or_insert(DevinUsage::default());
        if let Some(n) = proto::get(&fields, 2).and_then(|v| v.as_uint()) {
            u.input_uncached = n;
        }
        if let Some(n) = proto::get(&fields, 3).and_then(|v| v.as_uint()) {
            u.output = n;
        }
        if let Some(n) = proto::get(&fields, 5).and_then(|v| v.as_uint()) {
            u.cache_read = n;
        }
    }

    /// 流正常结束（无 stop 帧时兜底补终止事件）。
    pub fn finish(&mut self) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        let mut out = Vec::new();
        if !self.created {
            self.created = true;
            out.push(self.event_created());
        }
        out.extend(self.finish_events());
        out
    }

    /// 流内失败：已开始则补 response.failed；未开始由调用方映射为可重试错误。
    pub fn fail(&mut self, message: &str) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        if !self.created {
            return Vec::new();
        }
        let mut resp = self.build_final("failed");
        if let Some(o) = resp.as_object_mut() {
            o.insert(
                "error".into(),
                json!({ "code": "server_error", "message": message }),
            );
        }
        vec![encode_typed_event_with_type(
            &json!({ "type": "response.failed", "response": resp }).to_string(),
            Some("response.failed"),
        )]
    }

    fn finish_events(&mut self) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        // 终止事件按 output_index 顺序补发（reasoning → message → function_call）
        let mut done: Vec<(u64, String)> = Vec::new();
        if self.reasoning_added {
            done.push((
                self.reasoning_index,
                encode_typed_event(
                    &json!({
                        "type": "response.reasoning_summary_text.done",
                        "item_id": self.reasoning_item_id,
                        "output_index": self.reasoning_index,
                        "summary_index": 0,
                        "text": self.reasoning_text,
                    })
                    .to_string(),
                ),
            ));
            done.push((
                self.reasoning_index,
                encode_typed_event(
                    &json!({
                        "type": "response.output_item.done",
                        "output_index": self.reasoning_index,
                        "item": self.item_reasoning("completed"),
                    })
                    .to_string(),
                ),
            ));
        }
        if self.msg_added {
            done.push((
                self.msg_index,
                encode_typed_event(
                    &json!({
                        "type": "response.output_text.done",
                        "item_id": self.msg_item_id,
                        "output_index": self.msg_index,
                        "text": self.msg_text,
                    })
                    .to_string(),
                ),
            ));
            done.push((
                self.msg_index,
                encode_typed_event(
                    &json!({
                        "type": "response.output_item.done",
                        "output_index": self.msg_index,
                        "item": self.item_message("completed"),
                    })
                    .to_string(),
                ),
            ));
        }
        for i in 0..self.calls.len() {
            done.push((
                self.calls[i].output_index,
                encode_typed_event(
                    &json!({
                        "type": "response.output_item.done",
                        "output_index": self.calls[i].output_index,
                        "item": self.item_fn(i, "completed"),
                    })
                    .to_string(),
                ),
            ));
        }
        done.sort_by_key(|(idx, _)| *idx);
        let mut out: Vec<String> = done.into_iter().map(|(_, ev)| ev).collect();
        out.push(encode_typed_event(
            &json!({
                "type": "response.completed",
                "response": self.build_final("completed"),
            })
            .to_string(),
        ));
        out
    }

    fn event_created(&mut self) -> String {
        encode_typed_event(
            &json!({
                "type": "response.created",
                "response": {
                    "id": self.resp_id_or_gen(),
                    "object": "response",
                    "created_at": self.created_at,
                    "status": "in_progress",
                    "model": self.model,
                    "output": [],
                    "usage": Value::Null,
                },
            })
            .to_string(),
        )
    }

    fn event_item_added_message(&self) -> String {
        encode_typed_event(
            &json!({
                "type": "response.output_item.added",
                "output_index": self.msg_index,
                "item": self.item_message("in_progress"),
            })
            .to_string(),
        )
    }

    fn event_item_added_reasoning(&self) -> String {
        encode_typed_event(
            &json!({
                "type": "response.output_item.added",
                "output_index": self.reasoning_index,
                "item": self.item_reasoning("in_progress"),
            })
            .to_string(),
        )
    }

    fn event_item_added_fn(&self, i: usize) -> String {
        encode_typed_event(
            &json!({
                "type": "response.output_item.added",
                "output_index": self.calls[i].output_index,
                "item": self.item_fn(i, "in_progress"),
            })
            .to_string(),
        )
    }

    fn item_message(&self, status: &str) -> Value {
        let mut content = Vec::new();
        if !self.msg_text.is_empty() {
            content.push(json!({ "type": "output_text", "text": self.msg_text }));
        }
        json!({
            "id": self.msg_item_id,
            "type": "message",
            "status": status,
            "role": "assistant",
            "content": content,
        })
    }

    /// reasoning item（思维链）：summary 单段 summary_text，与 IR→Responses 编码保持一致。
    fn item_reasoning(&self, status: &str) -> Value {
        json!({
            "id": self.reasoning_item_id,
            "type": "reasoning",
            "status": status,
            "summary": [{ "type": "summary_text", "text": self.reasoning_text }],
        })
    }

    fn item_fn(&self, i: usize, status: &str) -> Value {
        let c = &self.calls[i];
        json!({
            "id": c.item_id,
            "type": "function_call",
            "status": status,
            "call_id": c.call_id,
            "name": c.name,
            "arguments": c.args,
        })
    }

    fn usage_json(&self) -> Value {
        match self.usage {
            Some(u) => {
                let input_total = u.input_uncached.saturating_add(u.cache_read);
                json!({
                    "input_tokens": input_total,
                    "output_tokens": u.output,
                    "total_tokens": input_total.saturating_add(u.output),
                    "input_tokens_details": { "cached_tokens": u.cache_read },
                })
            }
            None => Value::Null,
        }
    }

    fn build_final(&mut self, status: &str) -> Value {
        // 按 output_index 排列（reasoning → message → function_call）
        let mut items: Vec<(u64, Value)> = Vec::new();
        if self.reasoning_added {
            items.push((self.reasoning_index, self.item_reasoning("completed")));
        }
        if self.msg_added {
            items.push((self.msg_index, self.item_message("completed")));
        }
        for i in 0..self.calls.len() {
            items.push((self.calls[i].output_index, self.item_fn(i, "completed")));
        }
        items.sort_by_key(|(idx, _)| *idx);
        let output: Vec<Value> = items.into_iter().map(|(_, v)| v).collect();
        json!({
            "id": self.resp_id_or_gen(),
            "object": "response",
            "created_at": self.created_at,
            "status": status,
            "model": self.model,
            "output": output,
            "usage": self.usage_json(),
        })
    }

    /// 响应 id：首次生成后写回（created 与 completed 必须同 id）
    fn resp_id_or_gen(&mut self) -> String {
        if self.resp_id.is_empty() {
            self.resp_id = format!("resp_{}", uuid::Uuid::new_v4().simple());
        }
        self.resp_id.clone()
    }

    /// 非流式聚合：完整 Responses JSON。
    pub fn final_json(&mut self) -> Value {
        if !self.finished {
            let _ = self.finish();
        }
        self.build_final("completed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn det_uuid_stable_and_formatted() {
        let a = det_uuid("seed1");
        assert_eq!(a, det_uuid("seed1"));
        assert_ne!(a, det_uuid("seed2"));
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
    }

    #[test]
    fn request_encodes_cli_shape() {
        let body = json!({
            "instructions": "You are helpful.",
            "input": [
                {"role": "user", "content": "hi"},
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "先看用户意图"}]},
                {"role": "assistant", "content": "calling"},
                {"type": "function_call", "call_id": "call_1", "name": "exec", "arguments": "{\"c\":1}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "ok"}
            ],
            "tools": [{"type": "function", "name": "exec", "description": "run", "parameters": {"type": "object"}}],
            "temperature": 0.5,
            "max_output_tokens": 1000
        });
        let opts = RequestOptions {
            cli_emulation: false,
            ..Default::default()
        };
        let env = build_chat_request_ex(&body, "swe-1-6-slow", "tok$1", &opts).unwrap();
        // 信封
        assert_eq!(env[0], 0);
        let payload = &env[5..];
        let fields = proto::decode(payload).unwrap();
        assert_eq!(
            proto::get(&fields, 2).unwrap().as_str(),
            Some("You are helpful.")
        );
        assert_eq!(proto::get_all(&fields, 3).len(), 3); // user / assistant(合并) / tool
        assert_eq!(proto::get(&fields, 7).unwrap().as_uint(), Some(5));
        assert_eq!(
            proto::get(&fields, 21).unwrap().as_str(),
            Some("swe-1-6-slow")
        );
        assert_eq!(proto::get_all(&fields, 10).len(), 1);
        // assistant 消息：f3 正文 + f6 工具调用 + f11 思维链（reasoning 回传）
        let assistant = proto::get_all(&fields, 3)[1].as_bytes().unwrap().clone();
        let af = proto::decode(&assistant).unwrap();
        assert_eq!(proto::get(&af, 2).unwrap().as_uint(), Some(2));
        assert_eq!(proto::get(&af, 3).unwrap().as_str(), Some("calling"));
        assert_eq!(proto::get(&af, 11).unwrap().as_str(), Some("先看用户意图"));
        let tc = proto::get(&af, 6).unwrap().as_bytes().unwrap().clone();
        let tf = proto::decode(&tc).unwrap();
        assert_eq!(proto::get(&tf, 1).unwrap().as_str(), Some("call_1"));
        // tool 结果：f3 + f7
        let tool = proto::get_all(&fields, 3)[2].as_bytes().unwrap().clone();
        let tof = proto::decode(&tool).unwrap();
        assert_eq!(proto::get(&tof, 2).unwrap().as_uint(), Some(4));
        assert_eq!(proto::get(&tof, 7).unwrap().as_str(), Some("call_1"));
    }

    #[test]
    fn stream_thinking_and_answer_to_responses_events() {
        let mut conv = StreamConv::new("swe-1-6-slow");
        // 帧：消息 id + 思维链增量（f9）
        let mut f1 = BytesMut::new();
        put_str(&mut f1, 1, "bot-abc");
        put_str(&mut f1, 9, "The user wants ");
        let ev1 = conv.feed(&f1).unwrap();
        assert!(ev1.iter().any(|e| e.contains("response.created")));
        assert!(ev1
            .iter()
            .any(|e| e.contains("response.reasoning_summary_text.delta")));
        assert!(!ev1.iter().any(|e| e.contains("response.output_text.delta")));
        let mut f2 = BytesMut::new();
        put_str(&mut f2, 9, "the answer.");
        let ev2 = conv.feed(&f2).unwrap();
        // 思维链只在首帧加 item（reasoning），output_index=0
        assert!(!ev2.iter().any(|e| e.contains("response.output_item.added")));
        let ev2_data = sse_data(&ev2[0]);
        assert_eq!(ev2_data["output_index"], 0);
        assert_eq!(ev2_data["summary_index"], 0);
        assert_eq!(ev2_data["delta"], "the answer.");

        // 帧：回答正文增量（f3，伴 f4 分段标记）→ message / output_text
        let mut f3 = BytesMut::new();
        put_str(&mut f3, 3, "Hello");
        put_uint(&mut f3, 4, 2);
        let ev3 = conv.feed(&f3).unwrap();
        assert!(ev3.iter().any(|e| e.contains("response.output_item.added")));
        assert!(ev3.iter().any(|e| e.contains("response.output_text.delta")));
        // 正文 item 排在 reasoning 之后：output_index=1
        let added = ev3
            .iter()
            .find(|e| e.contains("output_item.added"))
            .map(|e| sse_data(e))
            .unwrap();
        assert_eq!(added["item"]["type"], "message");
        assert_eq!(added["output_index"], 1);

        // 帧：工具调用（id+name 后参数增量）
        let mut sub = BytesMut::new();
        put_str(&mut sub, 1, "call_x");
        put_str(&mut sub, 2, "exec");
        let mut f4 = BytesMut::new();
        put_msg(&mut f4, 6, &sub);
        let ev4 = conv.feed(&f4).unwrap();
        assert!(ev4.iter().any(|e| e.contains("response.output_item.added")));
        let mut sub3 = BytesMut::new();
        put_str(&mut sub3, 3, "{\"a\"");
        let mut f5 = BytesMut::new();
        put_msg(&mut f5, 6, &sub3);
        let ev5 = conv.feed(&f5).unwrap();
        assert!(ev5
            .iter()
            .any(|e| e.contains("response.function_call_arguments.delta")));

        // 帧：stop + usage
        let mut u = BytesMut::new();
        put_uint(&mut u, 2, 100);
        put_uint(&mut u, 3, 20);
        put_uint(&mut u, 5, 50);
        let mut f6 = BytesMut::new();
        put_uint(&mut f6, 5, 10);
        put_msg(&mut f6, 7, &u);
        // stop 帧不立即收尾（usage 终值可能在后续帧），流末才出终止事件
        let ev6 = conv.feed(&f6).unwrap();
        assert!(!ev6.iter().any(|e| e.contains("response.completed")));
        let ev7 = conv.finish();
        assert!(ev7.iter().any(|e| e.contains("response.completed")));
        // 结束事件按 output_index 顺序：reasoning_summary_text.done → output_item.done(reasoning)
        // → output_text.done → output_item.done(message) → output_item.done(function_call)
        let seq: Vec<&str> = ev7
            .iter()
            .filter_map(|e| e.lines().next())
            .map(|l| l.trim_start_matches("event: "))
            .collect();
        assert_eq!(seq[0], "response.reasoning_summary_text.done");
        assert_eq!(seq[1], "response.output_item.done");
        assert_eq!(seq[2], "response.output_text.done");
        assert_eq!(seq.last().copied(), Some("response.completed"));
        // 思维链完整文本进 reasoning item
        let rdone = sse_data(&ev7[0]);
        assert_eq!(rdone["text"], "The user wants the answer.");
        let ritem = sse_data(&ev7[1]);
        assert_eq!(ritem["item"]["type"], "reasoning");
        assert_eq!(
            ritem["item"]["summary"][0]["text"],
            "The user wants the answer."
        );

        // 终止事件里的 usage：input=150（未缓存100+缓存50）output=20
        let done = ev7
            .iter()
            .find(|e| e.contains("response.completed"))
            .unwrap();
        assert!(done.contains("\"input_tokens\":150"));
        assert!(done.contains("\"cached_tokens\":50"));
        assert!(done.contains("\"output_tokens\":20"));
        // 终态 output 顺序：reasoning → message → function_call，正文只含回答（不含思维链）
        let v = sse_data(done);
        let output = v["response"]["output"].as_array().unwrap();
        assert_eq!(output[0]["type"], "reasoning");
        assert_eq!(output[1]["type"], "message");
        assert_eq!(output[1]["content"][0]["text"], "Hello");
        assert_eq!(output[2]["type"], "function_call");
        assert_eq!(output[2]["name"], "exec");
        assert_eq!(output[2]["arguments"], "{\"a\"");
    }

    #[test]
    fn non_stream_aggregate_keeps_thinking_out_of_text() {
        let mut conv = StreamConv::new("swe-1-6-slow");
        let mut f1 = BytesMut::new();
        put_str(&mut f1, 1, "bot-xyz");
        put_str(&mut f1, 9, "思考中");
        conv.feed(&f1).unwrap();
        let mut f2 = BytesMut::new();
        put_str(&mut f2, 3, "最终回答");
        conv.feed(&f2).unwrap();
        let json = conv.final_json();
        assert_eq!(json["output"][0]["type"], "reasoning");
        assert_eq!(json["output"][0]["summary"][0]["text"], "思考中");
        assert_eq!(json["output"][1]["type"], "message");
        assert_eq!(json["output"][1]["content"][0]["text"], "最终回答");
    }

    #[test]
    fn model_display_name_maps_swe_models() {
        assert_eq!(
            model_display_name("swe-1-6-slow").as_deref(),
            Some("SWE-1.6 Slow")
        );
        assert_eq!(
            model_display_name("swe-1-6-fast").as_deref(),
            Some("SWE-1.6 Fast")
        );
        assert_eq!(
            model_display_name("swe-2-high").as_deref(),
            Some("SWE-2 High")
        );
        assert_eq!(model_display_name("swe-2").as_deref(), Some("SWE-2"));
        assert_eq!(
            model_display_name("swe-1-7-lightning-max").as_deref(),
            Some("SWE-1.7 Lightning Max")
        );
        assert_eq!(model_display_name("swe-check"), None);
        assert_eq!(model_display_name("claude-sonnet-4"), None);
        // 头提示词末行按模型重渲染；不可识别时原样
        let rendered = render_cli_prompt("swe-1-6-slow");
        assert!(rendered.ends_with("You are powered by SWE-1.6 Slow."));
        assert_eq!(CLI_HEAD_PROMPT.len(), 17682);
        assert_eq!(render_cli_prompt("unknown-model"), CLI_HEAD_PROMPT);
    }

    #[test]
    fn cli_emulation_renders_head_prompt_with_instructions_appended() {
        let body = json!({
            "instructions": "调用方的系统提示词",
            "input": [{"role": "user", "content": "hi"}]
        });
        let opts = RequestOptions {
            cli_emulation: true,
            ..Default::default()
        };
        let env = build_chat_request_ex(&body, "swe-1-6-slow", "tok$1", &opts).unwrap();
        let fields = proto::decode(&env[5..]).unwrap();
        let prompt = proto::get(&fields, 2).unwrap().as_str().unwrap();
        assert!(
            prompt.starts_with("You are Devin, an interactive command line agent from Cognition.")
        );
        assert!(prompt.contains("You are powered by SWE-1.6 Slow."));
        assert!(prompt.ends_with("调用方的系统提示词"));
        // 首条消息 = <system_info>（source=1），变量已替换
        let msgs = proto::get_all(&fields, 3);
        assert_eq!(msgs.len(), 2);
        let first = proto::decode(msgs[0].as_bytes().unwrap()).unwrap();
        assert_eq!(proto::get(&first, 2).unwrap().as_uint(), Some(1));
        let info = proto::get(&first, 3).unwrap().as_str().unwrap();
        assert!(info.starts_with("<system_info>"));
        assert!(info.contains("  /workspace (cwd)"));
        assert!(!info.contains("{cwd}"));
        assert!(info.contains("Platform: "));
        assert!(!info.contains("SWE-1.6 Slow")); // system_info 不含模型名
    }

    #[test]
    fn cli_emulation_off_keeps_minimal_prompt_and_no_system_info() {
        let body = json!({"input": [{"role": "user", "content": "hi"}]});
        let opts = RequestOptions {
            cli_emulation: false,
            ..Default::default()
        };
        let env = build_chat_request_ex(&body, "swe-1-6-slow", "tok$1", &opts).unwrap();
        let fields = proto::decode(&env[5..]).unwrap();
        assert_eq!(
            proto::get(&fields, 2).unwrap().as_str(),
            Some(DEFAULT_PROMPT)
        );
        assert_eq!(proto::get_all(&fields, 3).len(), 1);
    }

    #[test]
    fn system_info_template_and_caller_vars() {
        let extra = json!({
            "cli_emulation": true,
            "system_info": "<system_info>{cwd}|{platform}|{custom}|{unknown}</system_info>"
        });
        let mut opts = RequestOptions::from_extra(&extra);
        opts.vars
            .push(("custom".to_string(), "调用方值".to_string()));
        let body = json!({"input": [{"role": "user", "content": "hi"}]});
        let env = build_chat_request_ex(&body, "swe-1-6-slow", "tok$1", &opts).unwrap();
        let fields = proto::decode(&env[5..]).unwrap();
        let first = proto::decode(proto::get_all(&fields, 3)[0].as_bytes().unwrap()).unwrap();
        let info = proto::get(&first, 3).unwrap().as_str().unwrap();
        assert_eq!(
            info,
            format!(
                "<system_info>/workspace|{}|调用方值|{{unknown}}</system_info>",
                std::env::consts::OS
            )
        );
        // 内置变量可被调用方覆盖（同名）
        let extra = json!({"cli_emulation": true, "system_info": "{cwd}"});
        let mut opts = RequestOptions::from_extra(&extra);
        opts.vars.push(("cwd".to_string(), "/repo".to_string()));
        let env = build_chat_request_ex(&body, "swe-1-6-slow", "tok$1", &opts).unwrap();
        let fields = proto::decode(&env[5..]).unwrap();
        let first = proto::decode(proto::get_all(&fields, 3)[0].as_bytes().unwrap()).unwrap();
        assert_eq!(proto::get(&first, 3).unwrap().as_str(), Some("/repo"));
    }

    #[test]
    fn system_info_spec_parsing() {
        assert_eq!(system_info_spec(&json!({})), SystemInfoSpec::Default);
        assert_eq!(
            system_info_spec(&json!({"system_info": false})),
            SystemInfoSpec::Off
        );
        assert_eq!(
            system_info_spec(&json!({"system_info": "   "})),
            SystemInfoSpec::Off
        );
        assert_eq!(
            system_info_spec(&json!({"system_info": "{date}"})),
            SystemInfoSpec::Template("{date}".into())
        );
        assert_eq!(
            system_info_spec(&json!({"system_info": {"template": "{date}"}})),
            SystemInfoSpec::Template("{date}".into())
        );
        // cli_emulation 默认开；显式 false 关
        assert!(RequestOptions::from_extra(&json!({})).cli_emulation);
        assert!(!RequestOptions::from_extra(&json!({"cli_emulation": false})).cli_emulation);
    }

    /// 从 SSE 事件串取出 data 行并解析为 JSON。
    fn sse_data(event: &str) -> Value {
        let line = event
            .lines()
            .find(|l| l.starts_with("data: "))
            .expect("SSE 事件应含 data 行");
        serde_json::from_str(line.trim_start_matches("data: ")).unwrap()
    }
}
