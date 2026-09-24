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
    /// 渠道鉴权方式：`api_key`（默认，provision 必填 Key）/ `oauth`（授权码换取凭证：
    /// 可先创建渠道，凭证随后由授权流程写入，故 provision 允许空 Key）
    pub auth_kind: &'static str,
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
    /// 业务空间专属域名（阿里云百炼：{WorkspaceId}.{region}.maas.aliyuncs.com）。
    /// 接入时可传 workspace_id + region 走专属域名；缺省仍用上面的共享域名。
    pub workspace_domain: Option<WorkspaceDomain>,
    /// 统一接入域名（如千问AI平台 maas.qianwenaiapi.com：无需 Workspace ID，百炼 Key 通用）。
    pub unified_domain: Option<UnifiedDomain>,
    pub description: &'static str,
}

/// 业务空间专属域名地域（id 进 URL，name 供前端展示）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct RegionSpec {
    pub id: &'static str,
    pub name: &'static str,
}

/// 统一接入域名（静态，无占位符；各协议路径与共享域名一致）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct UnifiedDomain {
    /// 前端选项名（如「千问AI平台统一域名」）
    pub label: &'static str,
    pub base_url: &'static str,
    pub media_base_url: Option<&'static str>,
    pub protocol_base_urls: &'static [(&'static str, &'static str)],
    pub hint: &'static str,
}

/// 业务空间专属域名模板：占位符 `{workspaceId}` / `{region}` 由接入时传入值替换。
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkspaceDomain {
    /// OpenAI 兼容根模板（如 https://{workspaceId}.{region}.maas.aliyuncs.com/compatible-mode/v1）
    pub base_url: &'static str,
    /// 图像原生根模板（阿里 DashScope 原生：域名 + /api/v1/...，故模板不含 /api/v1）
    pub media_base_url: Option<&'static str>,
    /// 分协议根模板（Anthropic 等独立根）
    pub protocol_base_urls: &'static [(&'static str, &'static str)],
    /// 支持的地域列表（region 校验白名单，防任意值注入域名）
    pub regions: &'static [RegionSpec],
    /// 前端展示的迁移提示
    pub hint: &'static str,
}

/// 业务空间专属域名入参（provision 可选项）。
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceInput<'a> {
    pub workspace_id: &'a str,
    pub region: &'a str,
}

/// 接入域名选择（provision 可选项；缺省 = 预设默认共享域名）。
#[derive(Debug, Clone, Copy)]
pub enum DomainChoice<'a> {
    /// 预设默认共享域名
    Default,
    /// 统一接入域名（如千问AI平台 maas.qianwenaiapi.com，无需 Workspace ID）
    Unified,
    /// 业务空间专属域名（需 workspace_id + region）
    Workspace(WorkspaceInput<'a>),
}

/// 接入目标（base_url 与 extra）：共享域名取预设字段，专属域名按模板渲染。
pub struct ProvisionTarget {
    pub base_url: String,
    pub extra: serde_json::Value,
}

/// 全部预设（13 个）
static PRESETS: [Preset; 13] = [
    Preset {
        name: "dashscope",
        display_name: "阿里云百炼 DashScope",
        kind: "aliyun",
        auth_kind: "api_key",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        // 文本走 OpenAI 兼容（Chat/Responses 同根）；Anthropic 兼容独立根 /apps/anthropic；
        // 图像走 DashScope 原生（同步 Qwen-Image + 异步万相）
        protocols: &[
            "openai_chat",
            "openai_responses",
            "anthropic",
            "images_dashscope_sync",
            "images_dashscope_async",
        ],
        media_base_url: Some("https://dashscope.aliyuncs.com"),
        // Anthropic 兼容的 SDK base_url 为 /apps/anthropic，实际端点为 /apps/anthropic/v1/messages，
        // 故覆盖根写到 /v1（endpoint_path 只拼 /messages）
        protocol_base_urls: Some(&[("anthropic", "https://dashscope.aliyuncs.com/apps/anthropic/v1")]),
        models_path: None,
        // 业务空间专属域名（推荐）：共享域名 dashscope.aliyuncs.com 自 2026-09-30 起停止新特性
        workspace_domain: Some(WorkspaceDomain {
            base_url: "https://{workspaceId}.{region}.maas.aliyuncs.com/compatible-mode/v1",
            media_base_url: Some("https://{workspaceId}.{region}.maas.aliyuncs.com"),
            protocol_base_urls: &[(
                "anthropic",
                "https://{workspaceId}.{region}.maas.aliyuncs.com/apps/anthropic/v1",
            )],
            regions: &[
                RegionSpec {
                    id: "cn-beijing",
                    name: "华北2（北京）",
                },
                RegionSpec {
                    id: "ap-southeast-1",
                    name: "新加坡",
                },
                RegionSpec {
                    id: "cn-hongkong",
                    name: "中国香港",
                },
                RegionSpec {
                    id: "ap-northeast-1",
                    name: "日本（东京）",
                },
                RegionSpec {
                    id: "eu-central-1",
                    name: "德国（法兰克福）",
                },
                RegionSpec {
                    id: "us-east-1",
                    name: "美国（弗吉尼亚）",
                },
            ],
            hint: "业务空间专属域名（推荐）：{WorkspaceId} 在百炼控制台「业务空间详情」或 API Key 弹窗的 API Host 中查看，替换后调用方式不变。共享域名 dashscope.aliyuncs.com 自 2026-09-30 起不再迭代新特性。",
        }),
        // 千问AI平台统一域名：无 Workspace ID、百炼 API Key 通用（路径与共享域名一致）
        unified_domain: Some(UnifiedDomain {
            label: "千问AI平台统一域名",
            base_url: "https://maas.qianwenaiapi.com/compatible-mode/v1",
            media_base_url: Some("https://maas.qianwenaiapi.com"),
            protocol_base_urls: &[(
                "anthropic",
                "https://maas.qianwenaiapi.com/apps/anthropic/v1",
            )],
            hint: "千问AI平台统一域名（maas.qianwenaiapi.com）：无需 Workspace ID，百炼 API Key 通用，路径与共享域名一致。",
        }),
        description:
            "阿里云百炼：文本走 OpenAI 兼容（Chat/Responses）；Anthropic 兼容独立根；图像走 DashScope 原生（Qwen-Image 同步 + 万相异步）。建议接入时选用业务空间专属域名或千问AI平台统一域名。",
    },
    Preset {
        name: "qianfan",
        display_name: "百度千帆",
        kind: "baidu",
        auth_kind: "api_key",
        base_url: "https://qianfan.baidubce.com/v2",
        protocols: &["openai_chat"],
        media_base_url: None,
        protocol_base_urls: None,
        models_path: None,
        workspace_domain: None,
        unified_domain: None,
        description: "百度千帆：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "qianfan_tokenplan",
        display_name: "百度千帆 Token Plan",
        kind: "baidu",
        auth_kind: "api_key",
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
        workspace_domain: None,
        unified_domain: None,
        description:
            "百度千帆 Token Plan（订阅套餐，需专属 API Key）：OpenAI 兼容 Chat/Responses 与 Anthropic 兼容，接入即用。",
    },
    Preset {
        name: "kimi",
        display_name: "月之暗面 Kimi",
        kind: "kimi",
        auth_kind: "api_key",
        base_url: "https://api.moonshot.cn/v1",
        protocols: &["openai_chat"],
        media_base_url: None,
        protocol_base_urls: None,
        models_path: None,
        workspace_domain: None,
        unified_domain: None,
        description: "月之暗面 Kimi：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "hunyuan",
        display_name: "腾讯云 TokenHub",
        kind: "tencent",
        auth_kind: "api_key",
        base_url: "https://tokenhub.tencentmaas.com/v1",
        // hy4-preview 等兼容三协议；Anthropic 为独立根（Claude Code 拼接 /v1/messages）
        protocols: &["openai_chat", "openai_responses", "anthropic"],
        media_base_url: None,
        protocol_base_urls: Some(&[("anthropic", "https://tokenhub.tencentmaas.com/v1")]),
        models_path: None,
        workspace_domain: None,
        unified_domain: None,
        description:
            "腾讯云 TokenHub：混元及第三方模型，OpenAI 兼容 Chat/Responses 与 Anthropic 兼容，接入即用。",
    },
    Preset {
        name: "tencent_tokenplan",
        display_name: "腾讯云 Token Plan",
        kind: "tencent",
        auth_kind: "api_key",
        base_url: "https://api.lkeap.cloud.tencent.com/plan/v3",
        // 官方双根：OpenAI 兼容 /plan/v3；Anthropic 独立根 /plan/anthropic（+ /v1/messages）
        protocols: &["openai_chat", "anthropic"],
        media_base_url: None,
        protocol_base_urls: Some(&[(
            "anthropic",
            "https://api.lkeap.cloud.tencent.com/plan/anthropic/v1",
        )]),
        models_path: None,
        workspace_domain: None,
        unified_domain: None,
        description:
            "腾讯云 Token Plan（个人版，订阅专属 Key：sk-tp- 开头）：OpenAI 兼容与 Anthropic 兼容，接入即用。",
    },
    Preset {
        name: "dashscope_tokenplan",
        display_name: "阿里云百炼 Token Plan",
        kind: "aliyun",
        auth_kind: "api_key",
        base_url: "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        // 官方双根：OpenAI 兼容 /compatible-mode/v1；Anthropic 独立根 /apps/anthropic（+ /v1/messages）
        protocols: &["openai_chat", "openai_responses", "anthropic"],
        media_base_url: None,
        protocol_base_urls: Some(&[(
            "anthropic",
            "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic/v1",
        )]),
        models_path: None,
        workspace_domain: None,
        unified_domain: None,
        description:
            "阿里云百炼 Token Plan（个人版/团队版，订阅专属 Key：sk-sp- 开头）：OpenAI 兼容 Chat/Responses 与 Anthropic 兼容。",
    },
    Preset {
        name: "zhipu",
        display_name: "智谱 GLM",
        kind: "zhipu",
        auth_kind: "api_key",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        protocols: &["openai_chat"],
        media_base_url: None,
        protocol_base_urls: None,
        models_path: None,
        workspace_domain: None,
        unified_domain: None,
        description: "智谱 GLM：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "volcano",
        display_name: "字节火山方舟",
        kind: "volcano",
        auth_kind: "api_key",
        base_url: "https://ark.cn-beijing.volces.com/api/v3",
        protocols: &["openai_chat"],
        media_base_url: None,
        protocol_base_urls: None,
        models_path: None,
        workspace_domain: None,
        unified_domain: None,
        description: "字节火山方舟：OpenAI 兼容对话，接入即用。",
    },
    Preset {
        name: "openrouter",
        display_name: "OpenRouter",
        kind: "openrouter",
        auth_kind: "api_key",
        base_url: "https://openrouter.ai/api/v1",
        protocols: &["openai_chat", "openai_responses"],
        media_base_url: None,
        protocol_base_urls: None,
        models_path: None,
        workspace_domain: None,
        unified_domain: None,
        description: "OpenRouter：OpenAI Chat/Responses 兼容聚合站，模型路由 + 联网搜索透传。",
    },
    Preset {
        name: "zenmux",
        display_name: "Zenmux",
        kind: "zenmux",
        auth_kind: "api_key",
        base_url: "https://api.zenmux.ai/v1",
        // 四协议兼容：OpenAI Chat / Responses / Anthropic / Gemini
        protocols: &["openai_chat", "openai_responses", "anthropic", "gemini"],
        media_base_url: None,
        protocol_base_urls: None,
        models_path: None,
        workspace_domain: None,
        unified_domain: None,
        description: "Zenmux：四协议兼容聚合站，zenmux/auto 自动路由、跨协议调用。",
    },
    Preset {
        name: "deepseek",
        display_name: "DeepSeek",
        kind: "deepseek",
        auth_kind: "api_key",
        base_url: "https://api.deepseek.com/v1",
        // 官网原生三协议：OpenAI Chat + Responses（/v1 根）+ Anthropic（/anthropic 独立根）
        protocols: &["openai_chat", "openai_responses", "anthropic"],
        media_base_url: None,
        protocol_base_urls: Some(&[("anthropic", "https://api.deepseek.com/anthropic/v1")]),
        models_path: None,
        workspace_domain: None,
        unified_domain: None,
        description:
            "DeepSeek 官方：原生支持 OpenAI Chat/Responses 与 Anthropic 三协议（Anthropic 走独立 /anthropic 根，已内置映射）。",
    },
    Preset {
        name: "devin",
        display_name: "Devin（OAuth 授权）",
        kind: "devin",
        // OAuth 渠道：免 API Key，凭证由 Devin CLI PKCE 授权流程换取后写入
        auth_kind: "oauth",
        base_url: crate::upstream::devin::DEFAULT_BASE_URL,
        // 对外入口 Responses（CLI 形态请求 → Devin Connect 协议）；其余三协议经 IR 转换互通
        protocols: &["openai_responses"],
        media_base_url: None,
        protocol_base_urls: None,
        models_path: None,
        workspace_domain: None,
        unified_domain: None,
        description:
            "Devin 官方：以 devin CLI 形态接入（DevIn Connect 协议），用 Devin 账号 OAuth 授权换取会话凭证，\
             免 API Key；对外提供 Responses 接口（含工具调用），其余协议经内部转换互通。",
    },
];

/// 全部预设（13 个）。
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

/// 预设 → 接入目标（base_url + extra：media_base_url / protocol_base_urls / models_path）。
///
/// 共享域名取预设字段；传 `workspace`（业务空间专属域名）时按预设模板渲染并对同协议根
/// 覆盖共享根；预设不支持（`workspace_domain` 为 None）→ 400。
pub fn provision_target(
    preset: &Preset,
    domain: DomainChoice<'_>,
) -> Result<ProvisionTarget, ApiError> {
    let mut base_url = preset.base_url.to_string();
    let mut media = preset.media_base_url.map(str::to_string);
    let mut protocol_urls: Vec<(String, String)> = preset
        .protocol_base_urls
        .map(|pbu| {
            pbu.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        })
        .unwrap_or_default();

    match domain {
        DomainChoice::Default => {}
        DomainChoice::Unified => {
            let uni = preset.unified_domain.as_ref().ok_or_else(|| {
                ApiError::bad_request(format!("预设 `{}` 不支持统一接入域名", preset.name))
            })?;
            base_url = uni.base_url.to_string();
            media = uni.media_base_url.map(str::to_string);
            merge_roots(&mut protocol_urls, uni.protocol_base_urls, None);
        }
        DomainChoice::Workspace(ws) => {
            let dom = preset.workspace_domain.as_ref().ok_or_else(|| {
                ApiError::bad_request(format!("预设 `{}` 不支持业务空间专属域名", preset.name))
            })?;
            validate_workspace_id(ws.workspace_id)?;
            if !dom.regions.iter().any(|r| r.id == ws.region) {
                return Err(ApiError::bad_request(format!(
                    "地域 `{}` 不在预设 `{}` 的业务空间专属域名支持范围内",
                    ws.region, preset.name
                )));
            }
            base_url = render_domain(dom.base_url, ws.workspace_id, ws.region);
            media = dom
                .media_base_url
                .map(|t| render_domain(t, ws.workspace_id, ws.region));
            merge_roots(
                &mut protocol_urls,
                dom.protocol_base_urls,
                Some((ws.workspace_id, ws.region)),
            );
        }
    }

    let mut extra = serde_json::Map::new();
    if let Some(m) = media {
        extra.insert("media_base_url".to_string(), serde_json::Value::String(m));
    }
    if !protocol_urls.is_empty() {
        let map: serde_json::Map<String, serde_json::Value> = protocol_urls
            .into_iter()
            .map(|(k, v)| (k, serde_json::Value::String(v)))
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
    Ok(ProvisionTarget {
        base_url,
        extra: serde_json::Value::Object(extra),
    })
}

/// 协议根合并：按协议覆盖已有根（同 key 覆盖，新增协议追加）；`render` 为 Some 时渲染占位符。
fn merge_roots(
    target: &mut Vec<(String, String)>,
    roots: &[(&'static str, &'static str)],
    render: Option<(&str, &str)>,
) {
    for (k, tpl) in roots {
        let url = match render {
            Some((ws, region)) => render_domain(tpl, ws, region),
            None => tpl.to_string(),
        };
        match target.iter_mut().find(|(pk, _)| pk == k) {
            Some(slot) => slot.1 = url,
            None => target.push((k.to_string(), url)),
        }
    }
}

/// 渲染专属域名模板：替换 `{workspaceId}` / `{region}` 占位符。
fn render_domain(tpl: &str, workspace_id: &str, region: &str) -> String {
    tpl.replace("{workspaceId}", workspace_id)
        .replace("{region}", region)
}

/// 纯函数：校验业务空间 ID（进 URL 的子域名段）——仅字母/数字/连字符、长度 1..=63、
/// 首尾非连字符，防 URL 注入。可单测。
fn validate_workspace_id(id: &str) -> Result<(), ApiError> {
    let ok = !id.is_empty()
        && id.len() <= 63
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-');
    if !ok {
        return Err(ApiError::bad_request(
            "workspace_id 非法：仅允许字母、数字与连字符，长度 1-63，且首尾不能是连字符",
        ));
    }
    Ok(())
}

/// 一键接入：创建 upstream（加密 key + 缓存刷新 + 审计）→ 连通性探测（不阻断）。
///
/// `domain` 选择接入域名（缺省共享域名；统一域名 / 业务空间专属域名见预设声明）。
/// 返回 `{upstream: {...}, test: {ok, status?, latency_ms, error?}}`。
/// name 冲突（UNIQUE）→ 409 ApiError::conflict；api_key 空白 / 域名参数非法 → 400。
pub async fn provision(
    state: &AppState,
    preset: &Preset,
    api_key: &str,
    upstream_name: Option<&str>,
    domain: DomainChoice<'_>,
    admin: &str,
) -> ApiResult<serde_json::Value> {
    // 参数校验：api_key 空白 → 400（OAuth 预设除外：可先建渠道，凭证由授权流程后置写入）；
    // 域名参数非法 → 400（在插入前 fail-fast）
    if !is_oauth(preset) {
        validate_provision_key(api_key)?;
    }
    let target = provision_target(preset, domain)?;

    // name 缺省 = preset.name
    let name = upstream_name
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(preset.name)
        .to_string();

    // api_key 加密（crypto 不可用时的处理照抄 upstreams create）；
    // OAuth 预设留空 → NULL（授权流程兑换到凭证后再写入）
    let api_key_enc = if api_key.trim().is_empty() {
        None
    } else {
        if !state.crypto.is_available() {
            return Err(ApiError::bad_request(
                "未设置 NEXTAPI_SECRET_KEY，无法保存上游鉴权 Key",
            ));
        }
        Some(
            state
                .crypto
                .encrypt(api_key.trim())
                .map_err(|e| ApiError::bad_request(e.to_string()))?,
        )
    };

    let extra = target.extra;

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
    .bind(&target.base_url)
    .bind(api_key_enc.as_deref())
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
    auth_audit(state, admin, &name, preset, domain).await?;

    // 连通性探测：刷新后从快照取回新建上游（含内存 api_key_plain）
    let snap = state.cache.snapshot();
    let up = snap.upstreams.get(&id).cloned().ok_or(ApiError::NotFound)?;
    let test = probe_upstream(state, &snap, &up).await;

    let upstream_val = serde_json::to_value(&up).map_err(ApiError::internal)?;
    Ok(serde_json::json!({ "upstream": upstream_val, "test": test }))
}

/// 预设是否为 OAuth 渠道（免 API Key：凭证由授权流程后置写入）。可单测。
pub fn is_oauth(preset: &Preset) -> bool {
    preset.auth_kind == "oauth"
}

/// 纯函数：校验 provision 的 api_key 是否空白（空白 → 400）。可单测。
fn validate_provision_key(api_key: &str) -> Result<(), ApiError> {
    if api_key.trim().is_empty() {
        return Err(ApiError::bad_request("api_key 不能为空"));
    }
    Ok(())
}

/// 审计（复用 crate::auth::audit；object_type \"upstream\"，summary 含 preset 全量 + 接入域名选择）。
async fn auth_audit(
    state: &AppState,
    admin: &str,
    name: &str,
    preset: &Preset,
    domain: DomainChoice<'_>,
) -> ApiResult<()> {
    crate::auth::audit(
        state,
        admin,
        "preset.provision",
        "upstream",
        Some(name),
        serde_json::json!({
            "preset": preset,
            "domain": audit_domain(domain),
        }),
        None,
    )
    .await
}

/// 域名选择 → 审计摘要（workspace 选项带 workspace_id / region）。
fn audit_domain(domain: DomainChoice<'_>) -> serde_json::Value {
    match domain {
        DomainChoice::Default => serde_json::json!({ "kind": "shared" }),
        DomainChoice::Unified => serde_json::json!({ "kind": "unified" }),
        DomainChoice::Workspace(ws) => serde_json::json!({
            "kind": "workspace",
            "workspace_id": ws.workspace_id,
            "region": ws.region,
        }),
    }
}

/// 连通性探测（参考 upstreams.rs test_upstream）：client_for + GET {base_url}/models + 10s 超时。
/// 失败不阻断创建，结果进响应 test 字段。
async fn probe_upstream(state: &AppState, snap: &Snapshot, up: &UpstreamRow) -> serde_json::Value {
    let client = state
        .client_pools
        .client_for(up, snap, &state.hot.load().proxy.default_proxy_id);

    // Devin 渠道：GetUserStatus 探测（账号/套餐/可用模型数）；未授权 → ok:false 提示授权
    if up.kind == "devin" {
        return crate::upstream::devin::probe_status(
            &client,
            &up.base_url,
            up.api_key_plain.as_deref(),
            10_000,
        )
        .await;
    }

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
    let resp = client
        .get(&url)
        .headers(headers)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match resp {
        Ok(r) => upstream::probe_ok_json(r.status().as_u16(), latency_ms),
        Err(e) => upstream::probe_err_json(latency_ms, e),
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
        assert_eq!(ps.len(), 13, "应有 13 个预设");

        // name 唯一
        let mut names: Vec<&str> = ps.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 13, "预设 name 必须唯一");

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
                "openai_responses",
                "anthropic",
                "images_dashscope_sync",
                "images_dashscope_async"
            ]
        );
        assert_eq!(dash.media_base_url, Some("https://dashscope.aliyuncs.com"));
        // 共享域名的 Anthropic 兼容根（SDK base_url /apps/anthropic + /v1/messages）
        let dash_anthropic: &[(&str, &str)] = &[(
            "anthropic",
            "https://dashscope.aliyuncs.com/apps/anthropic/v1",
        )];
        assert_eq!(dash.protocol_base_urls, Some(dash_anthropic));
        // 业务空间专属域名模板（阿里云百炼迁移公告 2026-09-30）
        let wd = dash
            .workspace_domain
            .as_ref()
            .expect("dashscope 应有专属域名模板");
        assert_eq!(
            wd.base_url,
            "https://{workspaceId}.{region}.maas.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(
            wd.media_base_url,
            Some("https://{workspaceId}.{region}.maas.aliyuncs.com")
        );
        assert_eq!(wd.regions.len(), 6);
        assert!(wd.regions.iter().any(|r| r.id == "cn-beijing"));
        // 统一接入域名（千问AI平台；无需 Workspace ID，百炼 Key 通用）
        let uni = dash
            .unified_domain
            .as_ref()
            .expect("dashscope 应有统一域名");
        assert_eq!(
            uni.base_url,
            "https://maas.qianwenaiapi.com/compatible-mode/v1"
        );
        assert_eq!(uni.media_base_url, Some("https://maas.qianwenaiapi.com"));
        // 其余预设无专属/统一域名模板
        for p in presets().iter().filter(|p| p.name != "dashscope") {
            assert!(
                p.workspace_domain.is_none(),
                "预设 `{}` 不应有专属域名模板",
                p.name
            );
            assert!(
                p.unified_domain.is_none(),
                "预设 `{}` 不应有统一域名",
                p.name
            );
        }

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
    fn preset_auth_kind_and_devin_oauth() {
        // auth_kind 只能是 api_key / oauth；devin 必须为 oauth（免 Key → 授权流程写入凭证）
        for p in presets() {
            assert!(
                ["api_key", "oauth"].contains(&p.auth_kind),
                "预设 `{}` auth_kind 非法: {}",
                p.name,
                p.auth_kind
            );
        }
        let devin = find("devin").expect("应有 devin 预设");
        assert_eq!(devin.kind, "devin");
        assert!(is_oauth(devin), "devin 预设必须是 oauth 鉴权");
        assert_eq!(
            devin.protocols,
            &["openai_responses"],
            "devin 入口以 Responses 优先"
        );
        assert_eq!(devin.base_url, crate::upstream::devin::DEFAULT_BASE_URL);
        // 非 OAuth 预设仍要求 Key
        assert!(!is_oauth(find("deepseek").unwrap()));
    }

    #[test]
    fn provision_key_requirement_by_auth_kind() {
        // 非 OAuth 预设：空 Key → 400（沿用原校验）
        assert!(validate_provision_key("").is_err());
        // OAuth 预设：空 Key 合法（provision 只对非 OAuth 预设做校验）
        let devin = find("devin").unwrap();
        assert!(is_oauth(devin));
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
            let target = provision_target(p, DomainChoice::Default).unwrap();
            let mut up = make_upstream(target.extra);
            up.base_url = target.base_url;
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
        let target = provision_target(p, DomainChoice::Default).unwrap();
        let mut up = make_upstream(target.extra);
        up.base_url = target.base_url;
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

    /// 阿里云百炼业务空间专属域名：模板渲染 → 实际请求 URL；非法入参被拒。
    #[test]
    fn dashscope_workspace_domain_renders_urls() {
        use crate::protocol::ir::Protocol;
        use crate::upstream::endpoint_path;

        let p = find("dashscope").unwrap();
        let ws = WorkspaceInput {
            workspace_id: "llm-abc123",
            region: "cn-beijing",
        };
        let target = provision_target(p, DomainChoice::Workspace(ws)).unwrap();
        assert_eq!(
            target.base_url,
            "https://llm-abc123.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
        );
        let mut up = make_upstream(target.extra);
        up.base_url = target.base_url;

        // Chat / Responses 同根
        assert_eq!(
            format!(
                "{}{}",
                up.base_url_for(Protocol::OpenaiChat),
                endpoint_path(Protocol::OpenaiChat, "m", false)
            ),
            "https://llm-abc123.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/chat/completions"
        );
        assert_eq!(
            format!(
                "{}{}",
                up.base_url_for(Protocol::OpenaiResponses),
                endpoint_path(Protocol::OpenaiResponses, "m", false)
            ),
            "https://llm-abc123.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/responses"
        );
        // Anthropic 兼容独立根
        assert_eq!(
            format!(
                "{}{}",
                up.base_url_for(Protocol::Anthropic),
                endpoint_path(Protocol::Anthropic, "m", false)
            ),
            "https://llm-abc123.cn-beijing.maas.aliyuncs.com/apps/anthropic/v1/messages"
        );
        // 模型列表 = OpenAI 兼容根 + /models
        assert_eq!(
            up.models_url(),
            "https://llm-abc123.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/models"
        );
        // 图像原生根：域名 + /api/v1/...（渲染后不含 /api/v1）
        assert_eq!(
            media_base_url(&up),
            "https://llm-abc123.cn-beijing.maas.aliyuncs.com"
        );

        // 非法 workspace_id：空 / 首尾连字符 / 含点或斜杠 / 超长 → 400
        for bad in ["", "-abc", "abc-", "a.b", "a/b", "a b"] {
            let ws = WorkspaceInput {
                workspace_id: bad,
                region: "cn-beijing",
            };
            assert!(
                provision_target(p, DomainChoice::Workspace(ws)).is_err(),
                "workspace_id `{bad}` 应被拒"
            );
        }
        let long = "a".repeat(64);
        assert!(provision_target(
            p,
            DomainChoice::Workspace(WorkspaceInput {
                workspace_id: &long,
                region: "cn-beijing",
            })
        )
        .is_err());

        // 不支持的地域 → 400；不支持的预设 → 400
        assert!(provision_target(
            p,
            DomainChoice::Workspace(WorkspaceInput {
                workspace_id: "llm-abc123",
                region: "cn-shanghai",
            })
        )
        .is_err());
        assert!(provision_target(
            find("kimi").unwrap(),
            DomainChoice::Workspace(WorkspaceInput {
                workspace_id: "llm-abc123",
                region: "cn-beijing",
            })
        )
        .is_err());

        // 统一域名（千问AI平台 maas.qianwenaiapi.com）：静态渲染，无需 Workspace ID，百炼 Key 通用
        let uni = provision_target(p, DomainChoice::Unified).unwrap();
        assert_eq!(
            uni.base_url,
            "https://maas.qianwenaiapi.com/compatible-mode/v1"
        );
        let mut up_u = make_upstream(uni.extra);
        up_u.base_url = uni.base_url;
        assert_eq!(
            format!(
                "{}{}",
                up_u.base_url_for(Protocol::OpenaiChat),
                endpoint_path(Protocol::OpenaiChat, "m", false)
            ),
            "https://maas.qianwenaiapi.com/compatible-mode/v1/chat/completions"
        );
        assert_eq!(
            format!(
                "{}{}",
                up_u.base_url_for(Protocol::Anthropic),
                endpoint_path(Protocol::Anthropic, "m", false)
            ),
            "https://maas.qianwenaiapi.com/apps/anthropic/v1/messages"
        );
        assert_eq!(
            up_u.models_url(),
            "https://maas.qianwenaiapi.com/compatible-mode/v1/models"
        );
        assert_eq!(media_base_url(&up_u), "https://maas.qianwenaiapi.com");

        // 无统一域名的预设 → 400
        assert!(provision_target(find("kimi").unwrap(), DomainChoice::Unified).is_err());
    }
}
