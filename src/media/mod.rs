//! 图片媒体适配核心（PLAN §5.4 v1.18：统一入口 + 三档适配；视频仅占位）。
//! 契约 contracts/m6-media.md §3，M6-A 实现。

pub mod download;
pub mod resolve;
pub mod tasks;

use serde::{Deserialize, Serialize};

use crate::entities::UpstreamRow;
use crate::upstream::UpstreamError;

/// 上游图片 API 形状（由 upstreams.protocols 中的 images_* 值决定；多值取第一个）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageApi {
    Openai,
    Gemini,
    DashscopeSync,
    DashscopeAsync,
}

/// 从 UpstreamRow.protocols 解析图片 API 形状（无 → None）
pub fn image_api_of(up: &UpstreamRow) -> Option<ImageApi> {
    for p in &up.protocols {
        match p.as_str() {
            "images_openai" => return Some(ImageApi::Openai),
            "images_gemini" => return Some(ImageApi::Gemini),
            "images_dashscope_sync" => return Some(ImageApi::DashscopeSync),
            "images_dashscope_async" => return Some(ImageApi::DashscopeAsync),
            _ => {} // 其余协议值忽略（安全）
        }
    }
    None
}

/// 统一图片请求（OpenAI 语义为枢轴）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImageRequest {
    pub prompt: String,
    pub negative_prompt: Option<String>,
    /// OpenAI 形态 "宽x高"（小写 x）
    pub size: Option<String>,
    pub n: Option<u32>,
    pub seed: Option<i64>,
    /// 透传 OpenAI 系；其余忽略
    pub quality: Option<String>,
    pub response_format: Option<String>,
    /// 阿里系透传
    pub watermark: Option<bool>,
}

/// 统一图片结果
#[derive(Debug, Clone)]
pub enum ImageOutcome {
    Created {
        images: Vec<ImageData>,
        image_count: i64,
        image_size: Option<String>,
    },
    Task {
        provider_task_id: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageData {
    pub url: Option<String>,
    pub b64_json: Option<String>,
}

/// 入口 body（OpenAI images/generations JSON）→ ImageRequest
pub fn parse_image_request(
    body: &serde_json::Value,
) -> Result<ImageRequest, crate::error::ApiError> {
    // prompt 必须为非空字符串
    let prompt = match body.get("prompt") {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() && s.len() <= 32 * 1024 => {
            s.clone()
        }
        _ => {
            return Err(crate::error::ApiError::bad_request(
                "prompt 必须为非空且不超过 32KB 的字符串",
            ))
        }
    };

    // size 若提供必须匹配 ^\d+x\d+$
    let size = match body.get("size") {
        None => None,
        Some(serde_json::Value::String(s)) if is_valid_size(s) => Some(s.clone()),
        Some(serde_json::Value::String(_)) => {
            return Err(crate::error::ApiError::bad_request(
                "size 必须为 '宽x高' 格式（如 1024x1024）",
            ))
        }
        Some(_) => return Err(crate::error::ApiError::bad_request("size 必须为字符串")),
    };

    // quality 若提供必须为供应商兼容的有限枚举，拒绝任意值以免误导上游。
    if let Some(v) = body.get("quality") {
        let valid = matches!(
            v.as_str(),
            Some("standard" | "hd" | "high" | "medium" | "low")
        );
        if !valid {
            return Err(crate::error::ApiError::bad_request(
                "quality 必须为 standard、hd、high、medium 或 low",
            ));
        }
    }

    // response_format 仅支持 OpenAI 图片接口的 url/b64_json。
    if let Some(v) = body.get("response_format") {
        if !matches!(v.as_str(), Some("url" | "b64_json")) {
            return Err(crate::error::ApiError::bad_request(
                "response_format 必须为 url 或 b64_json",
            ));
        }
    }

    // seed 若提供必须为整数，避免浮点/字符串静默丢弃。
    let seed = match body.get("seed") {
        None => None,
        Some(v) => Some(
            v.as_i64()
                .ok_or_else(|| crate::error::ApiError::bad_request("seed 必须为整数"))?,
        ),
    };

    // n 若提供必须为 1..=8
    let n = match body.get("n") {
        None => None,
        Some(v) => {
            let n = v
                .as_u64()
                .ok_or_else(|| crate::error::ApiError::bad_request("n 必须为正整数"))?;
            if !(1..=8).contains(&n) {
                return Err(crate::error::ApiError::bad_request("n 必须在 1 到 8 之间"));
            }
            Some(n as u32)
        }
    };

    Ok(ImageRequest {
        prompt,
        negative_prompt: body
            .get("negative_prompt")
            .and_then(|v| v.as_str())
            .map(String::from),
        size,
        n,
        seed,
        quality: body
            .get("quality")
            .and_then(|v| v.as_str())
            .map(String::from),
        response_format: body
            .get("response_format")
            .and_then(|v| v.as_str())
            .map(String::from),
        watermark: body.get("watermark").and_then(|v| v.as_bool()),
    })
}

fn req_response_format(req: &ImageRequest) -> Option<String> {
    req.response_format.clone()
}

/// 校验 size 是否为 ^\d+x\d+$（小写 x）
fn is_valid_size(s: &str) -> bool {
    let Some((w, h)) = s.split_once('x') else {
        return false;
    };
    !w.is_empty()
        && !h.is_empty()
        && w.bytes().all(|b| b.is_ascii_digit())
        && h.bytes().all(|b| b.is_ascii_digit())
}

/// 构建上游请求：返回 (url_path, body)
pub fn build_image_request(
    api: ImageApi,
    up_model: &str,
    req: &ImageRequest,
) -> (String, serde_json::Value) {
    match api {
        ImageApi::Openai => {
            let mut body = serde_json::Map::new();
            body.insert(
                "model".to_string(),
                serde_json::Value::String(up_model.to_string()),
            );
            body.insert(
                "prompt".to_string(),
                serde_json::Value::String(req.prompt.clone()),
            );
            if let Some(n) = req.n {
                body.insert("n".to_string(), serde_json::Value::from(n));
            }
            if let Some(size) = &req.size {
                body.insert("size".to_string(), serde_json::Value::String(size.clone()));
            }
            if let Some(q) = &req.quality {
                body.insert("quality".to_string(), serde_json::Value::String(q.clone()));
            }
            if let Some(seed) = req.seed {
                body.insert("seed".to_string(), serde_json::Value::from(seed));
            }
            if let Some(format) = req_response_format(req) {
                body.insert(
                    "response_format".to_string(),
                    serde_json::Value::String(format),
                );
            }
            (
                "/v1/images/generations".to_string(),
                serde_json::Value::Object(body),
            )
        }
        ImageApi::Gemini => {
            let mut generation_config = serde_json::Map::new();
            generation_config.insert(
                "responseModalities".to_string(),
                serde_json::Value::Array(vec![serde_json::Value::String("IMAGE".to_string())]),
            );
            if let Some(n) = req.n {
                generation_config.insert("candidateCount".to_string(), serde_json::Value::from(n));
            }
            if let Some(size) = &req.size {
                if let Some((aspect_ratio, image_size)) = size_to_gemini(size) {
                    let mut image_config = serde_json::Map::new();
                    image_config.insert(
                        "aspectRatio".to_string(),
                        serde_json::Value::String(aspect_ratio),
                    );
                    image_config.insert(
                        "imageSize".to_string(),
                        serde_json::Value::String(image_size),
                    );
                    generation_config.insert(
                        "imageConfig".to_string(),
                        serde_json::Value::Object(image_config),
                    );
                }
            }
            let body = serde_json::json!({
                "contents": [{"parts": [{"text": req.prompt}]}],
                "generationConfig": serde_json::Value::Object(generation_config),
            });
            // 模型名作路径段需 URL 编码（与 upstream::endpoint_path 一致，防 ?/&/# 污染）
            let enc = crate::upstream::encode_path_segment_pub(up_model);
            (format!("/v1beta/models/{enc}:generateContent"), body)
        }
        ImageApi::DashscopeSync | ImageApi::DashscopeAsync => {
            // 阿里系 parameters 共用：size 用宽*高（星号）
            let mut parameters = serde_json::Map::new();
            if let Some(size) = &req.size {
                parameters.insert(
                    "size".to_string(),
                    serde_json::Value::String(size_to_dashscope(size)),
                );
            }
            if let Some(n) = req.n {
                parameters.insert("n".to_string(), serde_json::Value::from(n));
            }
            if let Some(seed) = req.seed {
                parameters.insert("seed".to_string(), serde_json::Value::from(seed));
            }
            if let Some(watermark) = req.watermark {
                parameters.insert("watermark".to_string(), serde_json::Value::Bool(watermark));
            }

            if api == ImageApi::DashscopeSync {
                // 同步：negative_prompt 走 parameters
                if let Some(np) = &req.negative_prompt {
                    parameters.insert(
                        "negative_prompt".to_string(),
                        serde_json::Value::String(np.clone()),
                    );
                }
                let body = serde_json::json!({
                    "model": up_model,
                    "input": {"messages": [{"role": "user", "content": [{"text": req.prompt}]}]},
                    "parameters": serde_json::Value::Object(parameters),
                });
                (
                    "/api/v1/services/aigc/multimodal-generation/generation".to_string(),
                    body,
                )
            } else {
                // 异步：negative_prompt 走 input，parameters 只留 size/n/seed/watermark
                let mut input = serde_json::Map::new();
                input.insert(
                    "prompt".to_string(),
                    serde_json::Value::String(req.prompt.clone()),
                );
                if let Some(np) = &req.negative_prompt {
                    input.insert(
                        "negative_prompt".to_string(),
                        serde_json::Value::String(np.clone()),
                    );
                }
                let body = serde_json::json!({
                    "model": up_model,
                    "input": serde_json::Value::Object(input),
                    "parameters": serde_json::Value::Object(parameters),
                });
                (
                    "/api/v1/services/aigc/text2image/image-synthesis".to_string(),
                    body,
                )
            }
        }
    }
}

/// 该 API 形状是否需要请求头 X-DashScope-Async: enable
pub fn needs_async_header(api: ImageApi) -> bool {
    matches!(api, ImageApi::DashscopeAsync)
}

/// 解析上游响应 → ImageOutcome
pub fn parse_image_response(
    api: ImageApi,
    req_size: Option<&str>,
    json: &serde_json::Value,
) -> Result<ImageOutcome, UpstreamError> {
    match api {
        ImageApi::Openai => {
            // data[].url/b64_json；image_count=data.len()；image_size=请求 size（响应无尺寸）
            let data = json.get("data").and_then(|v| v.as_array()).ok_or_else(|| {
                UpstreamError::BodyRead("OpenAI 图片响应缺少 data 数组".to_string())
            })?;
            let mut images = Vec::with_capacity(data.len());
            for item in data {
                let url = item.get("url").and_then(|v| v.as_str()).map(String::from);
                let b64 = item
                    .get("b64_json")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                images.push(ImageData { url, b64_json: b64 });
            }
            if images.is_empty() {
                return Err(UpstreamError::BodyRead(
                    "OpenAI 图片响应 data 为空或缺少 url/b64_json".to_string(),
                ));
            }
            Ok(ImageOutcome::Created {
                image_count: images.len() as i64,
                images,
                image_size: req_size.map(|s| s.to_string()),
            })
        }
        ImageApi::Gemini => {
            // candidates[].content.parts[].inlineData.data（base64）→ b64_json；image_size=None
            let mut images = Vec::new();
            if let Some(candidates) = json.get("candidates").and_then(|v| v.as_array()) {
                for cand in candidates {
                    if let Some(parts) = cand.pointer("/content/parts").and_then(|v| v.as_array()) {
                        for part in parts {
                            if let Some(data) =
                                part.pointer("/inlineData/data").and_then(|v| v.as_str())
                            {
                                images.push(ImageData {
                                    url: None,
                                    b64_json: Some(data.to_string()),
                                });
                            }
                        }
                    }
                }
            }
            if images.is_empty() {
                return Err(UpstreamError::BodyRead(
                    "Gemini 图片响应缺少 inlineData 图片内容".to_string(),
                ));
            }
            Ok(ImageOutcome::Created {
                image_count: images.len() as i64,
                images,
                image_size: None,
            })
        }
        ImageApi::DashscopeSync => {
            // output.choices[].message.content[].image（URL）；usage.{image_count,width,height}
            let mut images = Vec::new();
            if let Some(choices) = json.pointer("/output/choices").and_then(|v| v.as_array()) {
                for choice in choices {
                    if let Some(content) = choice
                        .pointer("/message/content")
                        .and_then(|v| v.as_array())
                    {
                        for item in content {
                            if let Some(image) = item.get("image") {
                                push_image_url(&mut images, image);
                            }
                        }
                    }
                }
            }
            if images.is_empty() {
                return Err(UpstreamError::BodyRead(
                    "DashScope 图片响应缺少图片 URL".to_string(),
                ));
            }
            let reported_count = json.pointer("/usage/image_count").and_then(|v| v.as_i64());
            let image_count = reported_count
                .filter(|&n| n > 0 && n >= images.len() as i64)
                .unwrap_or(images.len() as i64);
            let image_size = match (
                json.pointer("/usage/width").and_then(|v| v.as_i64()),
                json.pointer("/usage/height").and_then(|v| v.as_i64()),
            ) {
                (Some(w), Some(h)) => Some(format!("{}x{}", w, h)),
                _ => None,
            };
            Ok(ImageOutcome::Created {
                image_count,
                images,
                image_size,
            })
        }
        ImageApi::DashscopeAsync => {
            // output.task_id（task_status=PENDING/RUNNING）
            let task_id = json
                .pointer("/output/task_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    UpstreamError::BodyRead("阿里异步响应缺少 output.task_id".to_string())
                })?;
            Ok(ImageOutcome::Task {
                provider_task_id: task_id.to_string(),
            })
        }
    }
}

/// 从阿里 content 项提取图片 URL（支持字符串或字符串数组）
fn push_image_url(images: &mut Vec<ImageData>, val: &serde_json::Value) {
    match val {
        serde_json::Value::String(s) => images.push(ImageData {
            url: Some(s.clone()),
            b64_json: None,
        }),
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(s) = item.as_str() {
                    images.push(ImageData {
                        url: Some(s.to_string()),
                        b64_json: None,
                    });
                }
            }
        }
        _ => {}
    }
}

/// 异步任务状态查询（DashscopeAsync）：GET {base_url}/api/v1/tasks/{provider_task_id}
pub async fn fetch_task_status(
    client: &reqwest::Client,
    base_url: &str,
    provider_task_id: &str,
    api_key: Option<&str>,
    timeout_ms: i32,
) -> Result<TaskStatus, UpstreamError> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/v1/tasks/{provider_task_id}");

    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(key) = api_key {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")) {
            headers.insert(reqwest::header::AUTHORIZATION, v);
        }
    }

    let mut req = client.get(&url).headers(headers);
    if timeout_ms > 0 {
        req = req.timeout(std::time::Duration::from_millis(timeout_ms as u64));
    }
    let resp = req.send().await.map_err(|e| {
        if e.is_timeout() {
            UpstreamError::Timeout
        } else {
            UpstreamError::Connect(e.to_string())
        }
    })?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| UpstreamError::BodyRead(e.to_string()))?;
    if !status.is_success() {
        return Err(UpstreamError::Status(
            status.as_u16(),
            truncate_text(&bytes, 2048),
        ));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| UpstreamError::BodyRead(e.to_string()))?;
    Ok(parse_task_status(json))
}

#[derive(Debug, Clone)]
pub struct TaskStatus {
    /// pending|processing|succeeded|failed（canceled 归一到 failed）
    pub status: String,
    pub image_count: Option<i64>,
    pub image_size: Option<String>,
    pub error: Option<String>,
    pub raw: serde_json::Value,
}

/// 解析任务查询 JSON → TaskStatus（纯函数，便于单测）
fn parse_task_status(json: serde_json::Value) -> TaskStatus {
    let raw = json.clone();
    let status = normalize_task_status(
        json.pointer("/output/task_status")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );

    let image_count = json.pointer("/usage/image_count").and_then(|v| v.as_i64());
    // 异步查询响应无尺寸信息 → None
    let image_size = None;

    let error = if status == "failed" {
        Some(
            json.pointer("/output/message")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| {
                    json.pointer("/output/code")
                        .and_then(|v| v.as_str())
                        .map(|c| format!("code={c}"))
                })
                .unwrap_or_else(|| "任务失败".to_string()),
        )
    } else {
        None
    };

    TaskStatus {
        status,
        image_count,
        image_size,
        error,
        raw,
    }
}

/// 归一化任务状态：PENDING/RUNNING/SUCCEEDED/FAILED/CANCELED/UNKNOWN
/// → pending/processing/succeeded/failed/failed/pending
fn normalize_task_status(s: &str) -> String {
    match s.trim().to_ascii_uppercase().as_str() {
        "" | "PENDING" | "UNKNOWN" => "pending",
        "RUNNING" => "processing",
        "SUCCEEDED" => "succeeded",
        "FAILED" | "CANCELED" | "CANCELLED" => "failed",
        // 未知值保守归 pending
        _ => "pending",
    }
    .to_string()
}

/// "宽x高" → 阿里 "宽*高"
pub fn size_to_dashscope(size: &str) -> String {
    size.replace('x', "*")
}

/// Gemini 设备支持的宽高比（aspect ratio 白名单）
const GEMINI_RATIOS: &[(&str, f64)] = &[
    ("1:1", 1.0),
    ("2:3", 2.0 / 3.0),
    ("3:2", 3.0 / 2.0),
    ("3:4", 3.0 / 4.0),
    ("4:3", 4.0 / 3.0),
    ("4:5", 4.0 / 5.0),
    ("5:4", 5.0 / 4.0),
    ("9:16", 9.0 / 16.0),
    ("16:9", 16.0 / 9.0),
    ("21:9", 21.0 / 9.0),
];

/// "宽x高" → Gemini (aspectRatio, imageSize)
pub fn size_to_gemini(size: &str) -> Option<(String, String)> {
    let (w_s, h_s) = size.split_once('x')?;
    let w: u32 = w_s.parse().ok()?;
    let h: u32 = h_s.parse().ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    // 比例归约：用 gcd 约分得到最简比例，再归一到最接近的 Gemini 支持宽高比
    let g = gcd(w, h);
    let rw = w / g;
    let rh = h / g;
    let ratio = rw as f64 / rh as f64;

    let mut best = GEMINI_RATIOS[0];
    let mut best_diff = (ratio - GEMINI_RATIOS[0].1).abs();
    for cand in &GEMINI_RATIOS[1..] {
        let diff = (ratio - cand.1).abs();
        if diff < best_diff {
            best_diff = diff;
            best = *cand;
        }
    }

    let max_dim = w.max(h);
    let image_size = if max_dim <= 512 {
        "0.5K"
    } else if max_dim <= 1024 {
        "1K"
    } else if max_dim <= 2048 {
        "2K"
    } else {
        "4K"
    };
    Some((best.0.to_string(), image_size.to_string()))
}

/// 最大公约数
fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// 截断错误正文（避免切开 UTF-8 码点，同 upstream::truncate_text 语义）
fn truncate_text(bytes: &[u8], max_bytes: usize) -> String {
    let len = bytes.len();
    if len <= max_bytes {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut end = max_bytes;
    while end > 0 && (bytes[end - 1] & 0xC0) == 0x80 {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::UpstreamRow;
    use serde_json::json;
    use uuid::Uuid;

    fn make_upstream(protocols: Vec<&str>) -> UpstreamRow {
        UpstreamRow {
            id: Uuid::new_v4(),
            name: "up".into(),
            kind: "openai".into(),
            base_url: "http://127.0.0.1".into(),
            api_key_plain: None,
            protocols: protocols.into_iter().map(String::from).collect(),
            enabled: true,
            timeout_ms: 30000,
            breaker_threshold: 5,
            probe_model: None,
            consecutive_failures: 0,
            disabled_by: None,
            cooldown_until: None,
            use_proxy: false,
            proxy_id: None,
            extra: json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    // ---- image_api_of ----
    #[test]
    fn image_api_of_selects_first_images_protocol() {
        assert_eq!(
            image_api_of(&make_upstream(vec!["images_gemini"])),
            Some(ImageApi::Gemini)
        );
        assert_eq!(
            image_api_of(&make_upstream(vec![
                "images_openai",
                "images_dashscope_async"
            ])),
            Some(ImageApi::Openai)
        );
        assert_eq!(
            image_api_of(&make_upstream(vec!["images_dashscope_sync"])),
            Some(ImageApi::DashscopeSync)
        );
        assert_eq!(
            image_api_of(&make_upstream(vec!["images_dashscope_async"])),
            Some(ImageApi::DashscopeAsync)
        );
    }

    #[test]
    fn image_api_of_none_when_missing() {
        assert_eq!(image_api_of(&make_upstream(vec!["openai_chat"])), None);
        assert_eq!(image_api_of(&make_upstream(vec!["anthropic"])), None);
        assert_eq!(image_api_of(&make_upstream(vec![])), None);
    }

    // ---- parse_image_request 校验 ----
    #[test]
    fn parse_image_request_happy_path() {
        let body = json!({
            "prompt": "猫",
            "negative_prompt": "狗",
            "size": "1024x1024",
            "n": 2,
            "seed": 42,
            "quality": "hd",
            "watermark": true
        });
        let req = parse_image_request(&body).unwrap();
        assert_eq!(req.prompt, "猫");
        assert_eq!(req.negative_prompt.as_deref(), Some("狗"));
        assert_eq!(req.size.as_deref(), Some("1024x1024"));
        assert_eq!(req.n, Some(2));
        assert_eq!(req.seed, Some(42));
        assert_eq!(req.quality.as_deref(), Some("hd"));
        assert_eq!(req.watermark, Some(true));
    }

    #[test]
    fn parse_image_request_prompt_must_be_nonempty() {
        assert!(parse_image_request(&json!({})).is_err());
        assert!(parse_image_request(&json!({"prompt": ""})).is_err());
        assert!(parse_image_request(&json!({"prompt": "   "})).is_err());
        assert!(parse_image_request(&json!({"prompt": 123})).is_err());
        assert!(parse_image_request(&json!({"prompt": ["x"]})).is_err());
    }

    #[test]
    fn parse_image_request_size_validation() {
        assert!(parse_image_request(&json!({"prompt":"p","size":"1024x1024"})).is_ok());
        assert!(parse_image_request(&json!({"prompt":"p","size":"1024X1024"})).is_err());
        assert!(parse_image_request(&json!({"prompt":"p","size":"abc"})).is_err());
        assert!(parse_image_request(&json!({"prompt":"p","size":"1024"})).is_err());
        assert!(parse_image_request(&json!({"prompt":"p","size":"1024x"})).is_err());
        assert!(parse_image_request(&json!({"prompt":"p","size":"x1024"})).is_err());
        assert!(parse_image_request(&json!({"prompt":"p","size":1024})).is_err());
        // 未提供 size 则允许
        assert!(parse_image_request(&json!({"prompt":"p"})).is_ok());
    }

    #[test]
    fn parse_image_request_n_range() {
        assert!(parse_image_request(&json!({"prompt":"p","n":1})).is_ok());
        assert!(parse_image_request(&json!({"prompt":"p","n":8})).is_ok());
        assert!(parse_image_request(&json!({"prompt":"p","n":0})).is_err());
        assert!(parse_image_request(&json!({"prompt":"p","n":9})).is_err());
        assert!(parse_image_request(&json!({"prompt":"p","n":"2"})).is_err());
        assert!(parse_image_request(&json!({"prompt":"p","n":2.5})).is_err());
        assert!(parse_image_request(&json!({"prompt":"p"})).is_ok());
    }

    // ---- size 映射 ----
    #[test]
    fn size_to_dashscope_swaps_x_to_star() {
        assert_eq!(size_to_dashscope("1024x1024"), "1024*1024");
        assert_eq!(size_to_dashscope("1536x2688"), "1536*2688");
    }

    #[test]
    fn size_to_gemini_known_examples() {
        assert_eq!(
            size_to_gemini("1024x1024"),
            Some(("1:1".into(), "1K".into()))
        );
        assert_eq!(
            size_to_gemini("2688x1536"),
            Some(("16:9".into(), "4K".into()))
        );
        assert_eq!(
            size_to_gemini("1536x2688"),
            Some(("9:16".into(), "4K".into()))
        );
        assert_eq!(
            size_to_gemini("2368x1728"),
            Some(("4:3".into(), "4K".into()))
        );
        assert_eq!(
            size_to_gemini("1728x2368"),
            Some(("3:4".into(), "4K".into()))
        );
        assert_eq!(
            size_to_gemini("2048x1152"),
            Some(("16:9".into(), "2K".into()))
        );
    }

    #[test]
    fn size_to_gemini_image_size_tiers() {
        // 只断言 imageSize 分档（aspect ratio 由比例归约决定，另测）
        assert_eq!(size_to_gemini("512x512").unwrap().1, "0.5K");
        assert_eq!(size_to_gemini("513x512").unwrap().1, "1K");
        assert_eq!(size_to_gemini("1024x512").unwrap().1, "1K");
        assert_eq!(size_to_gemini("1025x512").unwrap().1, "2K");
        assert_eq!(size_to_gemini("2048x512").unwrap().1, "2K");
        assert_eq!(size_to_gemini("2049x512").unwrap().1, "4K");
    }

    #[test]
    fn size_to_gemini_invalid_returns_none() {
        assert_eq!(size_to_gemini("abc"), None);
        assert_eq!(size_to_gemini("0x5"), None);
        assert_eq!(size_to_gemini("1024x0"), None);
        assert_eq!(size_to_gemini("1024"), None);
    }

    // ---- build_image_request 四形状 ----
    #[test]
    fn build_openai_passthrough() {
        let req = parse_image_request(&json!({
            "prompt":"p","size":"1024x1024","n":2,"quality":"hd","seed":7,"response_format":"b64_json","negative_prompt":"x"
        }))
        .unwrap();
        let (path, body) = build_image_request(ImageApi::Openai, "gpt-image-2", &req);
        assert_eq!(path, "/v1/images/generations");
        assert_eq!(body["model"], json!("gpt-image-2"));
        assert_eq!(body["prompt"], json!("p"));
        assert_eq!(body["size"], json!("1024x1024"));
        assert_eq!(body["n"], json!(2));
        assert_eq!(body["quality"], json!("hd"));
        assert_eq!(body["response_format"], json!("b64_json"));
        assert_eq!(body["seed"], json!(7));
        // negative_prompt / watermark 不进入 OpenAI 透传体
        assert!(body.get("negative_prompt").is_none());
        assert!(body.get("watermark").is_none());
    }

    #[test]
    fn build_gemini_generation_config() {
        let req = parse_image_request(&json!({
            "prompt":"p","size":"1024x1024","n":3
        }))
        .unwrap();
        let (path, body) = build_image_request(ImageApi::Gemini, "gemini-3.1-flash-image", &req);
        assert_eq!(
            path,
            "/v1beta/models/gemini-3.1-flash-image:generateContent"
        );
        assert_eq!(body["contents"][0]["parts"][0]["text"], json!("p"));
        assert_eq!(
            body["generationConfig"]["responseModalities"],
            json!(["IMAGE"])
        );
        assert_eq!(body["generationConfig"]["candidateCount"], json!(3));
        assert_eq!(
            body["generationConfig"]["imageConfig"]["aspectRatio"],
            json!("1:1")
        );
        assert_eq!(
            body["generationConfig"]["imageConfig"]["imageSize"],
            json!("1K")
        );
    }

    #[test]
    fn build_gemini_no_image_config_on_invalid_size() {
        // 构造一个不合法 size（parse 层拦截，这里直接构造以测 build 的降级）
        let req = ImageRequest {
            prompt: "p".into(),
            size: Some("bad".into()),
            ..Default::default()
        };
        let (_, body) = build_image_request(ImageApi::Gemini, "gemini-3.1-flash-image", &req);
        assert!(body["generationConfig"].get("imageConfig").is_none());
        // 尺寸合法但映射失败（超 u32）同样降级
        let req2 = ImageRequest {
            prompt: "p".into(),
            size: Some("99999999999999999999x1".into()),
            ..Default::default()
        };
        let (_, body2) = build_image_request(ImageApi::Gemini, "gemini-3.1-flash-image", &req2);
        assert!(body2["generationConfig"].get("imageConfig").is_none());
    }

    #[test]
    fn build_dashscope_sync_uses_star_size() {
        let req = parse_image_request(&json!({
            "prompt":"p","negative_prompt":"n","size":"1024x1024","n":2,"seed":11,"watermark":false
        }))
        .unwrap();
        let (path, body) = build_image_request(ImageApi::DashscopeSync, "qwen-image-2.0-pro", &req);
        assert_eq!(
            path,
            "/api/v1/services/aigc/multimodal-generation/generation"
        );
        assert_eq!(body["model"], json!("qwen-image-2.0-pro"));
        assert_eq!(body["input"]["messages"][0]["role"], json!("user"));
        assert_eq!(
            body["input"]["messages"][0]["content"][0]["text"],
            json!("p")
        );
        assert_eq!(body["parameters"]["size"], json!("1024*1024"));
        assert_eq!(body["parameters"]["n"], json!(2));
        assert_eq!(body["parameters"]["negative_prompt"], json!("n"));
        assert_eq!(body["parameters"]["seed"], json!(11));
        assert_eq!(body["parameters"]["watermark"], json!(false));
    }

    #[test]
    fn build_dashscope_async_shape_and_header_flag() {
        let req = parse_image_request(&json!({
            "prompt":"p","negative_prompt":"n","size":"1536x2688","n":1
        }))
        .unwrap();
        let (path, body) = build_image_request(ImageApi::DashscopeAsync, "wanx", &req);
        assert_eq!(path, "/api/v1/services/aigc/text2image/image-synthesis");
        assert_eq!(body["model"], json!("wanx"));
        assert_eq!(body["input"]["prompt"], json!("p"));
        assert_eq!(body["input"]["negative_prompt"], json!("n"));
        assert_eq!(body["parameters"]["size"], json!("1536*2688"));
        assert_eq!(body["parameters"]["n"], json!(1));
        // negative_prompt 不在 parameters（已在 input）
        assert!(body["parameters"].get("negative_prompt").is_none());

        assert!(needs_async_header(ImageApi::DashscopeAsync));
        assert!(!needs_async_header(ImageApi::DashscopeSync));
        assert!(!needs_async_header(ImageApi::Openai));
        assert!(!needs_async_header(ImageApi::Gemini));
    }

    // ---- parse_image_response 四形状 ----
    #[test]
    fn parse_openai_response() {
        let json = json!({
            "created": 123,
            "data": [
                {"url": "https://cdn/u1.png"},
                {"b64_json": "aGVsbG8="}
            ]
        });
        let out = parse_image_response(ImageApi::Openai, Some("1024x1024"), &json).unwrap();
        match out {
            ImageOutcome::Created {
                images,
                image_count,
                image_size,
            } => {
                assert_eq!(image_count, 2);
                assert_eq!(image_size.as_deref(), Some("1024x1024"));
                assert_eq!(images[0].url.as_deref(), Some("https://cdn/u1.png"));
                assert!(images[0].b64_json.is_none());
                assert_eq!(images[1].b64_json.as_deref(), Some("aGVsbG8="));
                assert!(images[1].url.is_none());
            }
            other => panic!("期望 Created, 得到 {other:?}"),
        }
    }

    #[test]
    fn parse_gemini_response_inline_data() {
        let json = json!({
            "candidates": [{
                "content": {
                    "parts": [
                        {"inlineData": {"mimeType": "image/png", "data": "iVBORw0KGgo="}}
                    ]
                }
            }]
        });
        let out = parse_image_response(ImageApi::Gemini, None, &json).unwrap();
        match out {
            ImageOutcome::Created {
                images,
                image_count,
                image_size,
                ..
            } => {
                assert_eq!(image_count, 1);
                assert_eq!(image_size, None);
                assert_eq!(images[0].b64_json.as_deref(), Some("iVBORw0KGgo="));
                assert!(images[0].url.is_none());
            }
            other => panic!("期望 Created, 得到 {other:?}"),
        }
    }

    #[test]
    fn parse_dashscope_sync_response() {
        let json = json!({
            "output": {
                "choices": [
                    {"message": {"content": [{"image": "https://dashscope/1.png"}]}}
                ]
            },
            "usage": {"image_count": 1, "width": 1024, "height": 1024}
        });
        let out = parse_image_response(ImageApi::DashscopeSync, None, &json).unwrap();
        match out {
            ImageOutcome::Created {
                images,
                image_count,
                image_size,
                ..
            } => {
                assert_eq!(image_count, 1);
                assert_eq!(image_size.as_deref(), Some("1024x1024"));
                assert_eq!(images[0].url.as_deref(), Some("https://dashscope/1.png"));
            }
            other => panic!("期望 Created, 得到 {other:?}"),
        }
    }

    #[test]
    fn parse_dashscope_async_response_task_id() {
        let json = json!({
            "output": {"task_id": "task-abc-123", "task_status": "PENDING"}
        });
        let out = parse_image_response(ImageApi::DashscopeAsync, None, &json).unwrap();
        match out {
            ImageOutcome::Task { provider_task_id } => assert_eq!(provider_task_id, "task-abc-123"),
            other => panic!("期望 Task, 得到 {other:?}"),
        }
    }

    #[test]
    fn parse_dashscope_async_missing_task_id_errors() {
        let json = json!({"output": {"task_status": "PENDING"}});
        assert!(parse_image_response(ImageApi::DashscopeAsync, None, &json).is_err());
    }

    // ---- 任务状态归一化（纯函数） ----
    #[test]
    fn task_status_normalization_mapping() {
        assert_eq!(normalize_task_status("PENDING"), "pending");
        assert_eq!(normalize_task_status("RUNNING"), "processing");
        assert_eq!(normalize_task_status("SUCCEEDED"), "succeeded");
        assert_eq!(normalize_task_status("FAILED"), "failed");
        assert_eq!(normalize_task_status("CANCELED"), "failed");
        assert_eq!(normalize_task_status("UNKNOWN"), "pending");
        assert_eq!(normalize_task_status(""), "pending");
    }

    #[test]
    fn parse_task_status_succeeded_collects_results() {
        let json = json!({
            "output": {"task_status": "SUCCEEDED", "results": [{"url": "https://r/1.png"}]},
            "usage": {"image_count": 1}
        });
        let ts = parse_task_status(json);
        assert_eq!(ts.status, "succeeded");
        assert_eq!(ts.image_count, Some(1));
        assert!(ts.error.is_none());
    }

    #[test]
    fn parse_task_status_failed_sets_error() {
        let json = json!({
            "output": {"task_status": "FAILED", "code": "InvalidParameter", "message": "参数错误"}
        });
        let ts = parse_task_status(json);
        assert_eq!(ts.status, "failed");
        assert_eq!(ts.error.as_deref(), Some("参数错误"));
    }

    #[test]
    fn parse_task_status_cancel_is_failed() {
        let json = json!({"output": {"task_status": "CANCELED"}});
        let ts = parse_task_status(json);
        assert_eq!(ts.status, "failed");
        assert!(ts.error.is_some());
    }
}
