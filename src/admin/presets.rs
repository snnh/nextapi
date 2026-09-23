//! 供应商预设 API（/api/presets，PLAN §7.2）。契约 contracts/m7-presets.md。
//!
//! - `GET /api/presets`：返回全部预设（含 media_base_url）；
//! - `POST /api/presets/{name}/provision`：body `{api_key, name?}`，api_key 空 → 400，
//!   name 缺省 = preset.name，调 presets::provision 一键接入（创建上游 + 加密 + 审计 + 探测）。
//!
//! 全部 SQL 使用运行时校验（sqlx::query / query_as），不使用 query! 宏。

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::auth::AdminUsername;
use crate::error::{ApiError, ApiResult};
use crate::presets;
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_presets))
        .route("/{name}/provision", post(provision_preset))
}

/// GET /api/presets：返回全部预设（含 media_base_url）。
async fn list_presets(
    State(_state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    let items: Vec<serde_json::Value> = presets::presets().iter().map(preset_json).collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

/// 预设 → 前端 JSON（纯函数，可单测；字段名是前端契约，勿改）。
fn preset_json(p: &presets::Preset) -> serde_json::Value {
    // protocol_base_urls / workspace_domain.protocol_base_urls 均以对象（协议→URL）输出，
    // 前端直接展示/编辑
    let mut v = serde_json::json!({
        "name": p.name,
        "display_name": p.display_name,
        "kind": p.kind,
        // 鉴权方式：api_key（默认）/ oauth（Devin：免 Key，走授权流程）
        "auth_kind": p.auth_kind,
        "base_url": p.base_url,
        "protocols": p.protocols,
        "media_base_url": p.media_base_url,
        "description": p.description,
    });
    if let Some(pbu) = p.protocol_base_urls {
        v["protocol_base_urls"] = roots_json(pbu);
    }
    // 业务空间专属域名模板（阿里云百炼）：前端据此渲染 workspaceId/region 表单与 URL 预览
    if let Some(wd) = &p.workspace_domain {
        v["workspace_domain"] = serde_json::json!({
            "base_url": wd.base_url,
            "media_base_url": wd.media_base_url,
            "protocol_base_urls": roots_json(wd.protocol_base_urls),
            "regions": wd
                .regions
                .iter()
                .map(|r| serde_json::json!({ "id": r.id, "name": r.name }))
                .collect::<Vec<_>>(),
            "hint": wd.hint,
        });
    }
    // 统一接入域名（千问AI平台等，静态无需参数）
    if let Some(uni) = &p.unified_domain {
        v["unified_domain"] = serde_json::json!({
            "label": uni.label,
            "base_url": uni.base_url,
            "media_base_url": uni.media_base_url,
            "protocol_base_urls": roots_json(uni.protocol_base_urls),
            "hint": uni.hint,
        });
    }
    v
}

/// 协议根列表 → JSON 对象（协议 → URL）。
fn roots_json(roots: &[(&'static str, &'static str)]) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = roots
        .iter()
        .map(|(k, u)| (k.to_string(), serde_json::Value::String(u.to_string())))
        .collect();
    serde_json::Value::Object(map)
}

/// POST /api/presets/{name}/provision 请求体。
#[derive(Debug, Deserialize)]
struct ProvisionReq {
    /// OAuth 预设（Devin）可省略：凭证由授权流程换取后粘贴，或建渠道后再授权
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    name: Option<String>,
    /// 接入域名：`shared`（缺省）/ `unified`（统一域名）/ `workspace`（业务空间专属域名）
    #[serde(default)]
    domain: Option<String>,
    /// 业务空间专属域名参数（domain=workspace 时必填）
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    region: Option<String>,
}

/// 纯函数：请求字段 → 接入域名选择（可单测）。
///
/// 兼容旧客户端：未指定 `domain` 但给了 workspace 参数时按专属域名处理。
fn parse_domain_choice<'a>(
    domain: Option<&str>,
    workspace_id: Option<&'a str>,
    region: Option<&'a str>,
) -> ApiResult<presets::DomainChoice<'a>> {
    use presets::DomainChoice;
    let normalized = domain.map(str::trim).filter(|s| !s.is_empty());
    let wants_workspace = normalized == Some("workspace")
        || (normalized.is_none() && (workspace_id.is_some() || region.is_some()));
    if wants_workspace {
        let (Some(id), Some(region)) = (workspace_id, region) else {
            return Err(ApiError::bad_request(
                "业务空间专属域名需同时提供 workspace_id 与 region",
            ));
        };
        return Ok(DomainChoice::Workspace(presets::WorkspaceInput {
            workspace_id: id.trim(),
            region: region.trim(),
        }));
    }
    // 非专属域名选项不允许携带（避免静默忽略用户输入）
    if workspace_id.is_some() || region.is_some() {
        return Err(ApiError::bad_request(
            "workspace_id / region 仅用于 domain=workspace（业务空间专属域名）",
        ));
    }
    match normalized {
        None | Some("shared") => Ok(DomainChoice::Default),
        Some("unified") => Ok(DomainChoice::Unified),
        Some(other) => Err(ApiError::bad_request(format!(
            "domain `{other}` 非法：可选 shared / unified / workspace"
        ))),
    }
}

/// POST /api/presets/{name}/provision：一键接入（预设不存在 → 404）。
async fn provision_preset(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(name): Path<String>,
    Json(body): Json<ProvisionReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let preset = presets::find(&name).ok_or(ApiError::NotFound)?;
    let domain = parse_domain_choice(
        body.domain.as_deref(),
        body.workspace_id.as_deref(),
        body.region.as_deref(),
    )?;
    let result = presets::provision(
        &state,
        preset,
        &body.api_key,
        body.name.as_deref(),
        domain,
        &admin.0,
    )
    .await?;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 预设 JSON 契约：前端依赖的字段名与 workspace_domain 结构（改名前先改前端）。
    #[test]
    fn preset_json_contract() {
        let v = preset_json(presets::find("dashscope").unwrap());
        assert_eq!(v["name"], "dashscope");
        assert!(v["protocols"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("anthropic")));
        // 共享域名 Anthropic 根以对象（协议→URL）输出
        assert_eq!(
            v["protocol_base_urls"]["anthropic"],
            "https://dashscope.aliyuncs.com/apps/anthropic/v1"
        );
        // 专属域名模板（占位符原样透出，前端/后端各自渲染）
        let wd = &v["workspace_domain"];
        assert_eq!(
            wd["base_url"],
            "https://{workspaceId}.{region}.maas.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(
            wd["media_base_url"],
            "https://{workspaceId}.{region}.maas.aliyuncs.com"
        );
        assert_eq!(
            wd["protocol_base_urls"]["anthropic"],
            "https://{workspaceId}.{region}.maas.aliyuncs.com/apps/anthropic/v1"
        );
        let regions = wd["regions"].as_array().unwrap();
        assert_eq!(regions.len(), 6);
        assert_eq!(regions[0]["id"], "cn-beijing");
        assert!(regions[0]["name"].as_str().unwrap().contains("北京"));
        assert!(wd["hint"].as_str().unwrap().contains("2026-09-30"));

        // 统一接入域名（千问AI平台；静态域名 + 三协议根）
        let uni = &v["unified_domain"];
        assert_eq!(uni["label"], "千问AI平台统一域名");
        assert_eq!(
            uni["base_url"],
            "https://maas.qianwenaiapi.com/compatible-mode/v1"
        );
        assert_eq!(uni["media_base_url"], "https://maas.qianwenaiapi.com");
        assert_eq!(
            uni["protocol_base_urls"]["anthropic"],
            "https://maas.qianwenaiapi.com/apps/anthropic/v1"
        );

        // 无专属域名的预设不输出这些键（前端按缺失判断）
        let kimi = preset_json(presets::find("kimi").unwrap());
        assert!(kimi.get("workspace_domain").is_none());
        assert!(kimi.get("unified_domain").is_none());
        assert!(kimi.get("protocol_base_urls").is_none());
    }

    /// provision 域名参数解析：缺省 / shared / unified / workspace / 兼容旧字段 / 非法值。
    #[test]
    fn parse_domain_choice_cases() {
        use presets::DomainChoice;

        // 缺省（无任何参数）→ 共享域名
        assert!(matches!(
            parse_domain_choice(None, None, None).unwrap(),
            DomainChoice::Default
        ));
        assert!(matches!(
            parse_domain_choice(Some("shared"), None, None).unwrap(),
            DomainChoice::Default
        ));
        assert!(matches!(
            parse_domain_choice(Some("unified"), None, None).unwrap(),
            DomainChoice::Unified
        ));
        // 兼容旧客户端：未传 domain 但给了 workspace 参数
        match parse_domain_choice(None, Some(" llm-abc "), Some("cn-beijing")).unwrap() {
            DomainChoice::Workspace(ws) => {
                assert_eq!(ws.workspace_id, "llm-abc");
                assert_eq!(ws.region, "cn-beijing");
            }
            other => panic!("应解析为 workspace: {other:?}"),
        }
        assert!(
            parse_domain_choice(Some("workspace"), Some("llm-abc"), Some("cn-beijing")).is_ok()
        );

        // 非法：domain=workspace 缺参数 / 非 workspace 选项带参数 / 未知值
        assert!(parse_domain_choice(Some("workspace"), Some("llm-abc"), None).is_err());
        assert!(parse_domain_choice(Some("workspace"), None, None).is_err());
        assert!(parse_domain_choice(Some("shared"), Some("llm-abc"), Some("cn-beijing")).is_err());
        assert!(parse_domain_choice(Some("unified"), None, Some("cn-beijing")).is_err());
        assert!(parse_domain_choice(Some("bogus"), None, None).is_err());
        // 空串视同缺省
        assert!(matches!(
            parse_domain_choice(Some("  "), None, None).unwrap(),
            DomainChoice::Default
        ));
    }
}
