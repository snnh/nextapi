//! 供应商预设（PLAN §5.7：内置预设 + 一键接入）。契约 contracts/m7-presets.md，单子代理实现。

use std::time::Duration;
use uuid::Uuid;

use crate::cache::Snapshot;
use crate::entities::UpstreamRow;
use crate::error::{ApiError, ApiResult};
use crate::protocol::ir::Protocol;
use crate::state::AppState;
use crate::upstream;

/// 供应商预设（内置）
#[derive(Debug, Clone, serde::Serialize)]
pub struct Preset {
    pub name: &'static str,
    pub display_name: &'static str,
    pub kind: &'static str,
    pub base_url: &'static str,
    pub protocols: &'static [&'static str],
    /// 图像原生接口根（仅阿里系：文本兼容根与图像原生根不同）
    pub media_base_url: Option<&'static str>,
    pub description: &'static str,
}

/// 全部预设（8 个，见契约 §2）
static PRESETS: [Preset; 8] = [
    Preset {
        name: "dashscope",
        display_name: "阿里云百炼 DashScope",
        kind: "aliyun",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        // 文本走 OpenAI 兼容；图像走 DashScope 原生（同步 Qwen-Image + 异步万相）
        protocols: &[
            "openai_chat",
            "images_dashscope_sync",
            "images_dashscope_async",
        ],
        media_base_url: Some("https://dashscope.aliyuncs.com"),
        description:
            "阿里云百炼：文本走 OpenAI 兼容；图像走 DashScope 原生（Qwen-Image 同步 + 万相异步）。",
    },
    Preset {
        name: "qianfan",
        display_name: "百度千帆",
        kind: "baidu",
        base_url: "https://qianfan.baidubce.com/v2",
        protocols: &["openai_chat"],
        media_base_url: None,
        description: "百度千帆：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "kimi",
        display_name: "月之暗面 Kimi",
        kind: "kimi",
        base_url: "https://api.moonshot.cn/v1",
        protocols: &["openai_chat"],
        media_base_url: None,
        description: "月之暗面 Kimi：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "hunyuan",
        display_name: "腾讯混元 TokenHub",
        kind: "tencent",
        base_url: "https://api.hunyuan.cloud.tencent.com/v1",
        protocols: &["openai_chat"],
        media_base_url: None,
        description: "腾讯混元 TokenHub：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "zhipu",
        display_name: "智谱 GLM",
        kind: "zhipu",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        protocols: &["openai_chat"],
        media_base_url: None,
        description: "智谱 GLM：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "volcano",
        display_name: "字节火山方舟",
        kind: "volcano",
        base_url: "https://ark.cn-beijing.volces.com/api/v3",
        protocols: &["openai_chat"],
        media_base_url: None,
        description: "字节火山方舟：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "openrouter",
        display_name: "OpenRouter",
        kind: "openrouter",
        base_url: "https://openrouter.ai/api/v1",
        protocols: &["openai_chat", "openai_responses"],
        media_base_url: None,
        description: "OpenRouter：OpenAI Chat/Responses 兼容聚合站，模型路由 + 联网搜索透传。",
    },
    Preset {
        name: "zenmux",
        display_name: "Zenmux",
        kind: "zenmux",
        base_url: "https://api.zenmux.ai/v1",
        // 四协议兼容：OpenAI Chat / Responses / Anthropic / Gemini
        protocols: &["openai_chat", "openai_responses", "anthropic", "gemini"],
        media_base_url: None,
        description: "Zenmux：四协议兼容聚合站，zenmux/auto 自动路由、跨协议调用。",
    },
];

/// 全部预设（8 个，见契约 §2）。
pub fn presets() -> &'static [Preset] {
    &PRESETS
}

/// 按 name 查找预设；不存在 → None。
pub fn find(name: &str) -> Option<&'static Preset> {
    presets().iter().find(|p| p.name == name)
}

/// 图像请求 base：extra.media_base_url 优先，回落 base_url（阿里系文本/图像根不同）。
pub fn media_base_url(up: &UpstreamRow) -> &str {
    up.extra
        .get("media_base_url")
        .and_then(|v| v.as_str())
        .unwrap_or(&up.base_url)
}

/// 一键接入：创建 upstream（加密 key + 缓存刷新 + 审计）→ 连通性探测（不阻断）。
///
/// 返回 `{upstream: {...}, test: {ok, status?, latency_ms, error?}}`。
/// name 冲突（UNIQUE）→ 409 ApiError::conflict；api_key 空白 → 400。
pub async fn provision(
    state: &AppState,
    preset: &Preset,
    api_key: &str,
    upstream_name: Option<&str>,
    admin: &str,
) -> ApiResult<serde_json::Value> {
    // 参数校验：api_key 空白 → 400
    validate_provision_key(api_key)?;

    // name 缺省 = preset.name
    let name = upstream_name
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(preset.name)
        .to_string();

    // api_key 加密（crypto 不可用时的处理照抄 upstreams create）
    if !state.crypto.is_available() {
        return Err(ApiError::bad_request(
            "未设置 NEXTAPI_SECRET_KEY，无法保存上游鉴权 Key",
        ));
    }
    let api_key_enc = state
        .crypto
        .encrypt(api_key)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    // extra 写 media_base_url（仅当 Some；保留既有键，勿覆盖——新建行并无既有键）
    let mut extra = serde_json::Map::new();
    if let Some(m) = preset.media_base_url {
        extra.insert(
            "media_base_url".to_string(),
            serde_json::Value::String(m.to_string()),
        );
    }
    let extra = serde_json::Value::Object(extra);

    let protocols: Vec<String> = preset.protocols.iter().map(|s| s.to_string()).collect();

    // 插入 upstream：enabled=true、timeout_ms 默认 300000、breaker_threshold 默认 5
    // name 冲突（UNIQUE）→ From<sqlx::Error> 映射为 409 ApiError::conflict
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO upstreams \
         (name, kind, base_url, api_key_enc, protocols, enabled, timeout_ms, breaker_threshold, \
          probe_model, consecutive_failures, disabled_by, cooldown_until, use_proxy, proxy_id, extra) \
         VALUES ($1,$2,$3,$4,$5,TRUE,$6,$7,NULL,0,NULL,NULL,FALSE,NULL,$8) RETURNING id",
    )
    .bind(&name)
    .bind(preset.kind)
    .bind(preset.base_url)
    .bind(&api_key_enc)
    .bind(&protocols)
    .bind(300_000)
    .bind(5)
    .bind(&extra)
    .fetch_one(&state.db)
    .await?;

    // 写后刷新快照 + 审计
    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth_audit(state, admin, &name, preset).await?;

    // 连通性探测：刷新后从快照取回新建上游（含内存 api_key_plain）
    let snap = state.cache.snapshot();
    let up = snap.upstreams.get(&id).cloned().ok_or(ApiError::NotFound)?;
    let test = probe_upstream(state, &snap, &up).await;

    let upstream_val = serde_json::to_value(&up).map_err(ApiError::internal)?;
    Ok(serde_json::json!({ "upstream": upstream_val, "test": test }))
}

/// 纯函数：校验 provision 的 api_key 是否空白（空白 → 400）。可单测。
fn validate_provision_key(api_key: &str) -> Result<(), ApiError> {
    if api_key.trim().is_empty() {
        return Err(ApiError::bad_request("api_key 不能为空"));
    }
    Ok(())
}

/// 审计（复用 crate::auth::audit；object_type \"upstream\"，summary 含 preset 全量）。
async fn auth_audit(state: &AppState, admin: &str, name: &str, preset: &Preset) -> ApiResult<()> {
    crate::auth::audit(
        state,
        admin,
        "preset.provision",
        "upstream",
        Some(name),
        serde_json::json!({ "preset": preset }),
        None,
    )
    .await
}

/// 连通性探测（参考 upstreams.rs test_upstream）：client_for + GET {base_url}/models + 10s 超时。
/// 失败不阻断创建，结果进响应 test 字段。
async fn probe_upstream(state: &AppState, snap: &Snapshot, up: &UpstreamRow) -> serde_json::Value {
    let client = state
        .client_pools
        .client_for(up, snap, &state.hot.load().proxy.default_proxy_id);
    let proto = up
        .protocol_list()
        .first()
        .copied()
        .unwrap_or(Protocol::OpenaiChat);

    let url = format!("{}/models", up.base_url.trim_end_matches('/'));
    let mut headers = reqwest::header::HeaderMap::new();
    // api_key_plain 仅在内存快照中持有；此处加鉴权头（绝不回传）
    upstream::apply_auth(&mut headers, proto, up.api_key_plain.as_deref());

    let started = std::time::Instant::now();
    let latency_ms = started.elapsed().as_millis() as u64;
    let resp = client
        .get(&url)
        .headers(headers)
        .timeout(Duration::from_secs(10))
        .send()
        .await;

    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            serde_json::json!({ "ok": status < 500, "status": status, "latency_ms": latency_ms })
        }
        Err(e) => {
            serde_json::json!({ "ok": false, "latency_ms": latency_ms, "error": e.to_string() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 构建一个测试用 UpstreamRow（默认 openai_chat）。
    fn make_upstream(extra: serde_json::Value) -> UpstreamRow {
        UpstreamRow {
            id: Uuid::new_v4(),
            name: "up".into(),
            kind: "openai".into(),
            base_url: "https://up.example.com/base".into(),
            api_key_plain: None,
            protocols: vec!["openai_chat".into()],
            enabled: true,
            timeout_ms: 300_000,
            breaker_threshold: 5,
            probe_model: None,
            consecutive_failures: 0,
            disabled_by: None,
            cooldown_until: None,
            use_proxy: false,
            proxy_id: None,
            extra,
            model_sync: "manual".into(),
            model_exclude: vec![],
            models_cache: serde_json::json!([]),
            models_fetched_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn presets_complete() {
        let ps = presets();
        assert_eq!(ps.len(), 8, "应有 8 个预设");

        // name 唯一
        let mut names: Vec<&str> = ps.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 8, "预设 name 必须唯一");

        // protocols 全在白名单内
        const WHITELIST: &[&str] = &[
            "openai_chat",
            "openai_responses",
            "anthropic",
            "gemini",
            "images_openai",
            "images_gemini",
            "images_dashscope_sync",
            "images_dashscope_async",
        ];
        for p in ps {
            for proto in p.protocols {
                assert!(
                    WHITELIST.contains(proto),
                    "预设 `{}` 含非法 protocol: {proto}",
                    p.name
                );
            }
            // URL 必须 https
            assert!(
                p.base_url.starts_with("https://"),
                "预设 `{}` 的 base_url 必须为 https: {}",
                p.name,
                p.base_url
            );
            if let Some(m) = p.media_base_url {
                assert!(
                    m.starts_with("https://"),
                    "预设 `{}` 的 media_base_url 必须为 https: {m}",
                    p.name
                );
            }
        }
    }

    #[test]
    fn preset_expected_fields() {
        let dash = find("dashscope").unwrap();
        assert_eq!(dash.kind, "aliyun");
        assert_eq!(
            dash.protocols,
            &[
                "openai_chat",
                "images_dashscope_sync",
                "images_dashscope_async"
            ]
        );
        assert_eq!(dash.media_base_url, Some("https://dashscope.aliyuncs.com"));

        let zen = find("zenmux").unwrap();
        assert_eq!(
            zen.protocols,
            &["openai_chat", "openai_responses", "anthropic", "gemini"]
        );

        let or = find("openrouter").unwrap();
        assert_eq!(or.protocols, &["openai_chat", "openai_responses"]);

        // 其余单协议
        for name in ["qianfan", "kimi", "hunyuan", "zhipu", "volcano"] {
            let p = find(name).unwrap();
            assert_eq!(
                p.protocols,
                &["openai_chat"],
                "预设 `{name}` 应仅支持 openai_chat"
            );
            assert!(
                p.media_base_url.is_none(),
                "预设 `{name}` 不应有 media_base_url"
            );
        }
    }

    #[test]
    fn find_unknown_returns_none() {
        assert!(find("not-exist").is_none());
        assert!(find("DASHSCOPE").is_none()); // 大小写敏感
        assert!(find("").is_none());
    }

    #[test]
    fn media_base_url_uses_extra_when_present() {
        let up = make_upstream(json!({ "media_base_url": "https://dashscope.aliyuncs.com" }));
        assert_eq!(media_base_url(&up), "https://dashscope.aliyuncs.com");
    }

    #[test]
    fn media_base_url_fallback_to_base_url() {
        // extra 无 media_base_url → 回落 base_url
        let up = make_upstream(json!({}));
        assert_eq!(media_base_url(&up), up.base_url);
        // extra 为 null / 非字符串 → 回落 base_url
        let up2 = make_upstream(json!({ "media_base_url": null }));
        assert_eq!(media_base_url(&up2), up2.base_url);
        let up3 = make_upstream(json!({ "media_base_url": 123 }));
        assert_eq!(media_base_url(&up3), up3.base_url);
    }

    #[test]
    fn provision_validate_key_cases() {
        assert!(validate_provision_key("sk-123").is_ok());
        assert!(validate_provision_key(" ").is_err()); // 纯空白
        assert!(validate_provision_key("").is_err());
        assert!(validate_provision_key("\t\n").is_err());
    }
}
