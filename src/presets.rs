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
    /// 分协议 base_url 覆盖（多根供应商：如 DeepSeek 的 Anthropic 根独立于 OpenAI 根），
    /// provision 时写入 upstreams.extra.protocol_base_urls
    pub protocol_base_urls: Option<&'static [(&'static str, &'static str)]>,
    /// 模型列表路径覆盖（provision 时写入 upstreams.extra.models_path；默认 /models，
    /// 如千帆 Token Plan 的模型列表在 /v1/models 而 /models 不存在）
    pub models_path: Option<&'static str>,
    pub description: &'static str,
}

/// 全部预设（12 个）
static PRESETS: [Preset; 12] = [
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
        protocol_base_urls: None,
        models_path: None,
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
        protocol_base_urls: None,
        models_path: None,
        description: "百度千帆：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "qianfan_tokenplan",
        display_name: "百度千帆 Token Plan",
        kind: "baidu",
        base_url: "https://qianfan.baidubce.com/v2/tokenplan/personal",
        // 三协议实测可用：Chat/Responses 走 OpenAI 兼容根，Anthropic 走独立 /anthropic 根
        protocols: &["openai_chat", "openai_responses", "anthropic"],
        media_base_url: None,
        protocol_base_urls: Some(&[(
            "anthropic",
            "https://qianfan.baidubce.com/anthropic/tokenplan/personal/v1",
        )]),
        // 模型列表在 /v1/models（/models 不存在，若不覆盖探测与模型拉取会 404）
        models_path: Some("/v1/models"),
        description:
            "百度千帆 Token Plan（订阅套餐，需专属 API Key）：OpenAI 兼容 Chat/Responses 与 Anthropic 兼容，接入即用。",
    },
    Preset {
        name: "kimi",
        display_name: "月之暗面 Kimi",
        kind: "kimi",
        base_url: "https://api.moonshot.cn/v1",
        protocols: &["openai_chat"],
        media_base_url: None,
        protocol_base_urls: None,
        models_path: None,
        description: "月之暗面 Kimi：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "hunyuan",
        display_name: "腾讯云 TokenHub",
        kind: "tencent",
        base_url: "https://tokenhub.tencentmaas.com/v1",
        // hy4-preview 等兼容三协议；Anthropic 为独立根（Claude Code 拼接 /v1/messages）
        protocols: &["openai_chat", "openai_responses", "anthropic"],
        media_base_url: None,
        protocol_base_urls: Some(&[("anthropic", "https://tokenhub.tencentmaas.com/v1")]),
        models_path: None,
        description:
            "腾讯云 TokenHub：混元及第三方模型，OpenAI 兼容 Chat/Responses 与 Anthropic 兼容，接入即用。",
    },
    Preset {
        name: "tencent_tokenplan",
        display_name: "腾讯云 Token Plan",
        kind: "tencent",
        base_url: "https://api.lkeap.cloud.tencent.com/plan/v3",
        // 官方双根：OpenAI 兼容 /plan/v3；Anthropic 独立根 /plan/anthropic（+ /v1/messages）
        protocols: &["openai_chat", "anthropic"],
        media_base_url: None,
        protocol_base_urls: Some(&[(
            "anthropic",
            "https://api.lkeap.cloud.tencent.com/plan/anthropic/v1",
        )]),
        models_path: None,
        description:
            "腾讯云 Token Plan（个人版，订阅专属 Key：sk-tp- 开头）：OpenAI 兼容与 Anthropic 兼容，接入即用。",
    },
    Preset {
        name: "dashscope_tokenplan",
        display_name: "阿里云百炼 Token Plan",
        kind: "aliyun",
        base_url: "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        // 官方双根：OpenAI 兼容 /compatible-mode/v1；Anthropic 独立根 /apps/anthropic（+ /v1/messages）
        protocols: &["openai_chat", "openai_responses", "anthropic"],
        media_base_url: None,
        protocol_base_urls: Some(&[(
            "anthropic",
            "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic/v1",
        )]),
        models_path: None,
        description:
            "阿里云百炼 Token Plan（个人版/团队版，订阅专属 Key：sk-sp- 开头）：OpenAI 兼容 Chat/Responses 与 Anthropic 兼容。",
    },
    Preset {
        name: "zhipu",
        display_name: "智谱 GLM",
        kind: "zhipu",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        protocols: &["openai_chat"],
        media_base_url: None,
        protocol_base_urls: None,
        models_path: None,
        description: "智谱 GLM：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "volcano",
        display_name: "字节火山方舟",
        kind: "volcano",
        base_url: "https://ark.cn-beijing.volces.com/api/v3",
        protocols: &["openai_chat"],
        media_base_url: None,
        protocol_base_urls: None,
        models_path: None,
        description: "字节火山方舟：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "openrouter",
        display_name: "OpenRouter",
        kind: "openrouter",
        base_url: "https://openrouter.ai/api/v1",
        protocols: &["openai_chat", "openai_responses"],
        media_base_url: None,
        protocol_base_urls: None,
        models_path: None,
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
        protocol_base_urls: None,
        models_path: None,
        description: "Zenmux：四协议兼容聚合站，zenmux/auto 自动路由、跨协议调用。",
    },
    Preset {
        name: "deepseek",
        display_name: "DeepSeek",
        kind: "deepseek",
        base_url: "https://api.deepseek.com/v1",
        // 官网原生三协议：OpenAI Chat + Responses（/v1 根）+ Anthropic（/anthropic 独立根）
        protocols: &["openai_chat", "openai_responses", "anthropic"],
        media_base_url: None,
        protocol_base_urls: Some(&[("anthropic", "https://api.deepseek.com/anthropic/v1")]),
        models_path: None,
        description:
            "DeepSeek 官方：原生支持 OpenAI Chat/Responses 与 Anthropic 三协议（Anthropic 走独立 /anthropic 根，已内置映射）。",
    },
];

/// 全部预设（12 个）。
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

/// 预设 → upstreams.extra（media_base_url / protocol_base_urls / models_path；仅写入 Some 项）。
fn preset_extra(preset: &Preset) -> serde_json::Value {
    let mut extra = serde_json::Map::new();
    if let Some(m) = preset.media_base_url {
        extra.insert(
            "media_base_url".to_string(),
            serde_json::Value::String(m.to_string()),
        );
    }
    if let Some(pbu) = preset.protocol_base_urls {
        let map: serde_json::Map<String, serde_json::Value> = pbu
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect();
        extra.insert(
            "protocol_base_urls".to_string(),
            serde_json::Value::Object(map),
        );
    }
    if let Some(mp) = preset.models_path {
        extra.insert(
            "models_path".to_string(),
            serde_json::Value::String(mp.to_string()),
        );
    }
    serde_json::Value::Object(extra)
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

    let extra = preset_extra(preset);

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

    let url = up.models_url();
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
            oauth_plain: None,
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
        assert_eq!(ps.len(), 12, "应有 12 个预设");

        // name 唯一
        let mut names: Vec<&str> = ps.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 12, "预设 name 必须唯一");

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

        // 千帆 Token Plan：三协议 + 独立 Anthropic 根 + /v1/models 模型列表
        let tp = find("qianfan_tokenplan").unwrap();
        assert_eq!(
            tp.base_url,
            "https://qianfan.baidubce.com/v2/tokenplan/personal"
        );
        assert_eq!(
            tp.protocols,
            &["openai_chat", "openai_responses", "anthropic"]
        );
        assert_eq!(tp.models_path, Some("/v1/models"));
        assert!(tp.media_base_url.is_none());
        let anthropic_root: &[(&str, &str)] = &[(
            "anthropic",
            "https://qianfan.baidubce.com/anthropic/tokenplan/personal/v1",
        )];
        assert_eq!(tp.protocol_base_urls, Some(anthropic_root));

        // 腾讯云 TokenHub：官方新地址 + 三协议（Anthropic 走同域 /v1 前缀根）
        let hy = find("hunyuan").unwrap();
        assert_eq!(hy.base_url, "https://tokenhub.tencentmaas.com/v1");
        assert_eq!(
            hy.protocols,
            &["openai_chat", "openai_responses", "anthropic"]
        );
        let hy_anthropic: &[(&str, &str)] = &[("anthropic", "https://tokenhub.tencentmaas.com/v1")];
        assert_eq!(hy.protocol_base_urls, Some(hy_anthropic));

        // 腾讯云 Token Plan（个人版）：OpenAI /plan/v3 + Anthropic /plan/anthropic
        let tctp = find("tencent_tokenplan").unwrap();
        assert_eq!(tctp.base_url, "https://api.lkeap.cloud.tencent.com/plan/v3");
        assert_eq!(tctp.protocols, &["openai_chat", "anthropic"]);
        let tctp_anthropic: &[(&str, &str)] = &[(
            "anthropic",
            "https://api.lkeap.cloud.tencent.com/plan/anthropic/v1",
        )];
        assert_eq!(tctp.protocol_base_urls, Some(tctp_anthropic));

        // 阿里云百炼 Token Plan：OpenAI /compatible-mode/v1 + Anthropic /apps/anthropic
        let ali_tp = find("dashscope_tokenplan").unwrap();
        assert_eq!(
            ali_tp.base_url,
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(
            ali_tp.protocols,
            &["openai_chat", "openai_responses", "anthropic"]
        );
        let ali_anthropic: &[(&str, &str)] = &[(
            "anthropic",
            "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic/v1",
        )];
        assert_eq!(ali_tp.protocol_base_urls, Some(ali_anthropic));

        // 其余单协议
        for name in ["qianfan", "kimi", "zhipu", "volcano"] {
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

    /// Token Plan 三预设（千帆 / 腾讯 / 阿里）：预设 → extra → 实际请求 URL 组合（防回归）。
    #[test]
    fn tokenplan_presets_urls_compose_correctly() {
        use crate::protocol::ir::Protocol;
        use crate::upstream::endpoint_path;

        // (预设名, chat 完整 URL, anthropic 完整 URL, models URL)
        let cases: [(&str, &str, &str, &str); 3] = [
            (
                "qianfan_tokenplan",
                "https://qianfan.baidubce.com/v2/tokenplan/personal/chat/completions",
                "https://qianfan.baidubce.com/anthropic/tokenplan/personal/v1/messages",
                "https://qianfan.baidubce.com/v2/tokenplan/personal/v1/models",
            ),
            (
                "tencent_tokenplan",
                "https://api.lkeap.cloud.tencent.com/plan/v3/chat/completions",
                "https://api.lkeap.cloud.tencent.com/plan/anthropic/v1/messages",
                "https://api.lkeap.cloud.tencent.com/plan/v3/models",
            ),
            (
                "dashscope_tokenplan",
                "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/chat/completions",
                "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic/v1/messages",
                "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/models",
            ),
        ];
        for (name, chat_exp, anthropic_exp, models_exp) in cases {
            let p = find(name).unwrap();
            let mut up = make_upstream(preset_extra(p));
            up.base_url = p.base_url.to_string();
            assert_eq!(
                up.base_url_for(Protocol::OpenaiChat),
                p.base_url,
                "{name} openai 根"
            );
            let chat = format!(
                "{}{}",
                up.base_url_for(Protocol::OpenaiChat),
                endpoint_path(Protocol::OpenaiChat, "m", false)
            );
            assert_eq!(chat, chat_exp, "{name} chat URL");
            let anthropic = format!(
                "{}{}",
                up.base_url_for(Protocol::Anthropic),
                endpoint_path(Protocol::Anthropic, "m", false)
            );
            assert_eq!(anthropic, anthropic_exp, "{name} anthropic URL");
            assert_eq!(up.models_url(), models_exp, "{name} models URL");
        }

        // 千帆 Token Plan 的 Responses 入口（OpenAI 兼容根 + /responses）
        let p = find("qianfan_tokenplan").unwrap();
        let mut up = make_upstream(preset_extra(p));
        up.base_url = p.base_url.to_string();
        let responses = format!(
            "{}{}",
            up.base_url_for(Protocol::OpenaiResponses),
            endpoint_path(Protocol::OpenaiResponses, "m", false)
        );
        assert_eq!(
            responses,
            "https://qianfan.baidubce.com/v2/tokenplan/personal/responses"
        );
    }
}
