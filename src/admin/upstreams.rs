//! 上游渠道管理 API（/api/upstreams，PLAN.md §5.3 / §7.2）。
//!
//! - `api_key_enc` / `api_key_plain` 绝不对外输出，响应改用 `has_api_key: bool`；
//! - api_key 语义（更新）：未传或掩码回传（`"***"`）→ 保持；空串 `""` → 清除；其他 → 加密更新；
//!   创建时非空即加密，`NEXTAPI_SECRET_KEY` 未设置则拒绝保存（BadRequest）；
//! - protocols 元素须可 parse 为 `Protocol`（openai_chat/openai_responses/anthropic/gemini）；
//! - enabled=false → 同时写 disabled_by='manual'；enabled=true → 清 disabled_by/cooldown_until/
//!   consecutive_failures；启停后调用 `state.breaker.reset(id)` 清除内存熔断状态；
//! - POST /{id}/test：真实连通性测试（`client_for` 取客户端 → GET {base_url}/models，超时 10s），
//!   绝不返回 api_key；
//! - 所有写操作先提交 DB 再 `state.cache.reload` 刷新快照并写审计（object_type "upstream"）。
//!
//! 全部 SQL 使用运行时校验（sqlx::query / query_as），不使用 query! 宏。

use axum::{
    extract::{Path, State},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::auth::{self, AdminUsername};
use crate::error::{ApiError, ApiResult};
use crate::protocol::ir::Protocol;
use crate::state::AppState;
use crate::upstream;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_upstreams).post(create_upstream))
        .route("/{id}", put(update_upstream).delete(delete_upstream))
        .route("/{id}/test", post(test_upstream))
}

/// upstreams 表行（含加密列；对外绝不输出 api_key_enc）。
#[derive(Debug, FromRow)]
struct UpstreamDbRow {
    id: Uuid,
    name: String,
    kind: String,
    base_url: String,
    api_key_enc: Option<String>,
    protocols: Vec<String>,
    enabled: bool,
    timeout_ms: i32,
    breaker_threshold: i32,
    probe_model: Option<String>,
    consecutive_failures: i32,
    disabled_by: Option<String>,
    cooldown_until: Option<DateTime<Utc>>,
    use_proxy: bool,
    proxy_id: Option<Uuid>,
    extra: serde_json::Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// 对外响应结构：不暴露 api_key_enc/api_key_plain，改用 has_api_key。
#[derive(Debug, Serialize)]
struct UpstreamOut {
    id: Uuid,
    name: String,
    kind: String,
    base_url: String,
    has_api_key: bool,
    protocols: Vec<String>,
    enabled: bool,
    timeout_ms: i32,
    breaker_threshold: i32,
    probe_model: Option<String>,
    consecutive_failures: i32,
    disabled_by: Option<String>,
    cooldown_until: Option<DateTime<Utc>>,
    use_proxy: bool,
    proxy_id: Option<Uuid>,
    extra: serde_json::Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn to_out(row: UpstreamDbRow) -> UpstreamOut {
    UpstreamOut {
        has_api_key: row.api_key_enc.is_some(),
        id: row.id,
        name: row.name,
        kind: row.kind,
        base_url: row.base_url,
        protocols: row.protocols,
        enabled: row.enabled,
        timeout_ms: row.timeout_ms,
        breaker_threshold: row.breaker_threshold,
        probe_model: row.probe_model,
        consecutive_failures: row.consecutive_failures,
        disabled_by: row.disabled_by,
        cooldown_until: row.cooldown_until,
        use_proxy: row.use_proxy,
        proxy_id: row.proxy_id,
        extra: row.extra,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// 创建上游请求体。
#[derive(Debug, Deserialize)]
struct CreateUpstreamReq {
    name: String,
    #[serde(default)]
    kind: Option<String>,
    base_url: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    protocols: Option<Vec<String>>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    timeout_ms: Option<i32>,
    #[serde(default)]
    breaker_threshold: Option<i32>,
    #[serde(default)]
    probe_model: Option<String>,
    #[serde(default)]
    use_proxy: Option<bool>,
    #[serde(default)]
    proxy_id: Option<Uuid>,
    #[serde(default)]
    extra: Option<serde_json::Value>,
}

/// 更新上游请求体（全字段 Option）。
#[derive(Debug, Deserialize)]
struct UpdateUpstreamReq {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    protocols: Option<Vec<String>>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    timeout_ms: Option<i32>,
    #[serde(default)]
    breaker_threshold: Option<i32>,
    #[serde(default)]
    probe_model: Option<String>,
    #[serde(default)]
    use_proxy: Option<bool>,
    #[serde(default)]
    proxy_id: Option<Uuid>,
    #[serde(default)]
    extra: Option<serde_json::Value>,
}

/// GET /：列表（has_api_key 布尔；绝不输出 api_key）。
async fn list_upstreams(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    let rows: Vec<UpstreamDbRow> = sqlx::query_as::<_, UpstreamDbRow>(
        "SELECT id, name, kind, base_url, api_key_enc, protocols, enabled, timeout_ms, \
         breaker_threshold, probe_model, consecutive_failures, disabled_by, cooldown_until, \
         use_proxy, proxy_id, extra, created_at, updated_at FROM upstreams ORDER BY name",
    )
    .fetch_all(&state.db)
    .await?;
    let items: Vec<UpstreamOut> = rows.into_iter().map(to_out).collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

/// POST /：创建上游。
async fn create_upstream(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Json(body): Json<CreateUpstreamReq>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_upstream(&body.name, &body.base_url, body.protocols.as_deref(), body.timeout_ms, body.breaker_threshold)?;

    let api_key_enc = encrypt_create_api_key(&state, body.api_key.as_deref())?;
    let protocols = body.protocols.clone().unwrap_or_else(|| vec!["openai_chat".to_string()]);
    let enabled = body.enabled.unwrap_or(true);
    let disabled_by = if enabled { None } else { Some("manual".to_string()) };
    let timeout_ms = body.timeout_ms.unwrap_or(300000);
    let breaker_threshold = body.breaker_threshold.unwrap_or(5);
    let extra = body.extra.clone().unwrap_or_else(|| serde_json::json!({}));

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO upstreams \
         (name, kind, base_url, api_key_enc, protocols, enabled, timeout_ms, breaker_threshold, \
          probe_model, consecutive_failures, disabled_by, cooldown_until, use_proxy, proxy_id, extra) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,0,$10,NULL,$11,$12,$13) RETURNING id",
    )
    .bind(&body.name)
    .bind(body.kind.as_deref().unwrap_or("custom"))
    .bind(&body.base_url)
    .bind(&api_key_enc)
    .bind(&protocols)
    .bind(enabled)
    .bind(timeout_ms)
    .bind(breaker_threshold)
    .bind(&body.probe_model)
    .bind(&disabled_by)
    .bind(body.use_proxy.unwrap_or(false))
    .bind(body.proxy_id)
    .bind(&extra)
    .fetch_one(&state.db)
    .await?;

    state.cache.reload(&state.db, &state.crypto).await.map_err(ApiError::internal)?;
    auth::audit(
        &state,
        &admin.0,
        "upstream.create",
        "upstream",
        Some(&id.to_string()),
        serde_json::json!({ "name": body.name, "kind": body.kind, "base_url": body.base_url, "enabled": enabled }),
        None,
    )
    .await?;

    let row = fetch_upstream(&state, id).await?;
    Ok(Json(serde_json::json!(to_out(row))))
}

/// PUT /{id}：部分更新；api_key 语义见模块注释；enabled 切换见模块注释。
async fn update_upstream(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUpstreamReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let row = fetch_upstream(&state, id).await?;

    let name = body.name.unwrap_or(row.name.clone());
    let kind = body.kind.unwrap_or(row.kind.clone());
    let base_url = body.base_url.unwrap_or(row.base_url.clone());
    let protocols = body.protocols.unwrap_or_else(|| row.protocols.clone());
    let enabled = body.enabled.unwrap_or(row.enabled);
    let timeout_ms = body.timeout_ms.unwrap_or(row.timeout_ms);
    let breaker_threshold = body.breaker_threshold.unwrap_or(row.breaker_threshold);
    let probe_model = body.probe_model.or(row.probe_model.clone());
    let use_proxy = body.use_proxy.unwrap_or(row.use_proxy);
    let proxy_id = body.proxy_id.or(row.proxy_id);
    let extra = body.extra.unwrap_or_else(|| row.extra.clone());

    validate_upstream(&name, &base_url, Some(&protocols), Some(timeout_ms), Some(breaker_threshold))?;

    // api_key：未传/掩码保持、空串清除、其他加密更新
    let api_key_enc = match api_key_semantics(body.api_key.as_deref()) {
        ApiKeySemantics::Keep => row.api_key_enc,
        ApiKeySemantics::Clear => None,
        ApiKeySemantics::Set(plain) => {
            if !state.crypto.is_available() {
                return Err(ApiError::bad_request("未设置 NEXTAPI_SECRET_KEY，无法保存上游鉴权 Key"));
            }
            Some(state.crypto.encrypt(&plain).map_err(|e| ApiError::bad_request(e.to_string()))?)
        }
    };

    // enabled 切换：disabled_by / cooldown_until / consecutive_failures 联动
    let (disabled_by, cooldown_until, consecutive_failures) = if body.enabled.is_some() {
        if enabled {
            (None, None, 0)
        } else {
            (Some("manual".to_string()), row.cooldown_until, row.consecutive_failures)
        }
    } else {
        (row.disabled_by, row.cooldown_until, row.consecutive_failures)
    };

    sqlx::query(
        "UPDATE upstreams SET name=$1, kind=$2, base_url=$3, api_key_enc=$4, protocols=$5, \
         enabled=$6, timeout_ms=$7, breaker_threshold=$8, probe_model=$9, consecutive_failures=$10, \
         disabled_by=$11, cooldown_until=$12, use_proxy=$13, proxy_id=$14, extra=$15, updated_at=now() \
         WHERE id=$16",
    )
    .bind(&name)
    .bind(&kind)
    .bind(&base_url)
    .bind(&api_key_enc)
    .bind(&protocols)
    .bind(enabled)
    .bind(timeout_ms)
    .bind(breaker_threshold)
    .bind(&probe_model)
    .bind(consecutive_failures)
    .bind(&disabled_by)
    .bind(&cooldown_until)
    .bind(use_proxy)
    .bind(proxy_id)
    .bind(&extra)
    .bind(id)
    .execute(&state.db)
    .await?;

    state.cache.reload(&state.db, &state.crypto).await.map_err(ApiError::internal)?;
    // 启停切换后清除内存熔断状态
    if body.enabled.is_some() {
        state.breaker.reset(id);
    }
    auth::audit(
        &state,
        &admin.0,
        "upstream.update",
        "upstream",
        Some(&id.to_string()),
        serde_json::json!({ "name": name, "enabled": enabled, "disabled_by": disabled_by }),
        None,
    )
    .await?;

    let row = fetch_upstream(&state, id).await?;
    Ok(Json(serde_json::json!(to_out(row))))
}

/// DELETE /{id}：删除上游。
async fn delete_upstream(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let res = sqlx::query("DELETE FROM upstreams WHERE id=$1")
        .bind(id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    // 清理熔断状态（删除上游后不再保留冷却/半开残留，review P3）
    state.breaker.reset(id);

    state.cache.reload(&state.db, &state.crypto).await.map_err(ApiError::internal)?;
    auth::audit(&state, &admin.0, "upstream.delete", "upstream", Some(&id.to_string()), serde_json::json!({}), None).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /{id}/test：真实连通性测试（使用快照中解密后的 api_key_plain 加鉴权头）。
async fn test_upstream(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let snap = state.cache.snapshot();
    let up = snap.upstreams.get(&id).cloned().ok_or(ApiError::NotFound)?;

    // 取该上游的客户端（按代理/直连分池）+ 探测协议
    let client = state.client_pools.client_for(&up, &snap, &state.hot.load().proxy.default_proxy_id);
    let proto = up.protocol_list().first().copied().unwrap_or(Protocol::OpenaiChat);

    let url = format!("{}/models", up.base_url.trim_end_matches('/'));
    let mut headers = reqwest::header::HeaderMap::new();
    // api_key_plain 仅在内存快照中持有；此处加鉴权头（绝不回传）
    upstream::apply_auth(&mut headers, proto, up.api_key_plain.as_deref());

    let started = std::time::Instant::now();
    let resp = client.get(&url).headers(headers).timeout(Duration::from_secs(10)).send().await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            Ok(Json(serde_json::json!({ "ok": status < 500, "status": status, "latency_ms": latency_ms })))
        }
        Err(e) => {
            Ok(Json(serde_json::json!({ "ok": false, "latency_ms": latency_ms, "error": e.to_string() })))
        }
    }
}

/// 图片上游协议值（契约 m6：upstreams.protocols 新增四个 images_* 值，其余校验不变）。
fn is_images_protocol(p: &str) -> bool {
    matches!(p, "images_openai" | "images_gemini" | "images_dashscope_sync" | "images_dashscope_async")
}

/// 校验上游：名称/地址非空、协议可解析（或属于图片协议白名单）、超时/熔断阈值 > 0。
fn validate_upstream(
    name: &str,
    base_url: &str,
    protocols: Option<&[String]>,
    timeout_ms: Option<i32>,
    breaker_threshold: Option<i32>,
) -> Result<(), ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::bad_request("上游名称不能为空"));
    }
    if base_url.trim().is_empty() {
        return Err(ApiError::bad_request("上游 base_url 不能为空"));
    }
    if let Some(ps) = protocols {
        for p in ps {
            if p.parse::<Protocol>().is_err() && !is_images_protocol(p) {
                return Err(ApiError::bad_request(format!("protocols 包含非法协议: {p}")));
            }
        }
    }
    if let Some(t) = timeout_ms {
        if t <= 0 {
            return Err(ApiError::bad_request("timeout_ms 必须 > 0"));
        }
    }
    if let Some(b) = breaker_threshold {
        if b <= 0 {
            return Err(ApiError::bad_request("breaker_threshold 必须 > 0"));
        }
    }
    Ok(())
}

/// 创建路径的 api_key 加密：非空即加密（密钥缺失 → BadRequest）；空/None → None。
fn encrypt_create_api_key(state: &AppState, api_key: Option<&str>) -> Result<Option<String>, ApiError> {
    match api_key {
        Some(ak) if !ak.is_empty() => {
            if !state.crypto.is_available() {
                return Err(ApiError::bad_request("未设置 NEXTAPI_SECRET_KEY，无法保存上游鉴权 Key"));
            }
            let enc = state.crypto.encrypt(ak).map_err(|e| ApiError::bad_request(e.to_string()))?;
            Ok(Some(enc))
        }
        _ => Ok(None),
    }
}

/// api_key 更新语义：未传或掩码 → Keep；空串 → Clear；其他 → Set(新明文)。
#[derive(Debug, PartialEq)]
enum ApiKeySemantics {
    Keep,
    Clear,
    Set(String),
}

fn api_key_semantics(api_key: Option<&str>) -> ApiKeySemantics {
    match api_key {
        None => ApiKeySemantics::Keep,
        Some("***") => ApiKeySemantics::Keep,
        Some("") => ApiKeySemantics::Clear,
        Some(s) => ApiKeySemantics::Set(s.to_string()),
    }
}

/// 按 id 读取上游行；不存在 → NotFound。
async fn fetch_upstream(state: &AppState, id: Uuid) -> ApiResult<UpstreamDbRow> {
    sqlx::query_as::<_, UpstreamDbRow>(
        "SELECT id, name, kind, base_url, api_key_enc, protocols, enabled, timeout_ms, \
         breaker_threshold, probe_model, consecutive_failures, disabled_by, cooldown_until, \
         use_proxy, proxy_id, extra, created_at, updated_at FROM upstreams WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_semantics_cases() {
        assert_eq!(api_key_semantics(None), ApiKeySemantics::Keep);
        assert_eq!(api_key_semantics(Some("***")), ApiKeySemantics::Keep);
        assert_eq!(api_key_semantics(Some("")), ApiKeySemantics::Clear);
        assert_eq!(api_key_semantics(Some("new-key")), ApiKeySemantics::Set("new-key".into()));
    }

    #[test]
    fn validate_upstream_cases() {
        assert!(validate_upstream("u", "https://x.com", Some(&["openai_chat".into()]), Some(300_000), Some(5)).is_ok());
        assert!(validate_upstream("", "https://x.com", None, None, None).is_err());
        assert!(validate_upstream("u", "", None, None, None).is_err());
        assert!(validate_upstream("u", "https://x.com", Some(&["bad_proto".into()]), None, None).is_err());
        assert!(validate_upstream("u", "https://x.com", None, Some(0), None).is_err());
        assert!(validate_upstream("u", "https://x.com", None, None, Some(-1)).is_err());
    }

    #[test]
    fn validate_upstream_accepts_images_protocols() {
        // 契约 m6：新增四个 images_* 协议值放行
        for p in ["images_openai", "images_gemini", "images_dashscope_sync", "images_dashscope_async"] {
            assert!(validate_upstream("u", "https://x.com", Some(&[p.into()]), None, None).is_ok(), "应放行 {p}");
        }
        // 常规协议 + 图片协议混合放行
        assert!(validate_upstream(
            "u",
            "https://x.com",
            Some(&["openai_chat".into(), "images_gemini".into()]),
            None,
            None,
        )
        .is_ok());
        // 仍拒绝非法协议（图片协议拼错）
        assert!(validate_upstream("u", "https://x.com", Some(&["images_bad".into()]), None, None).is_err());
    }
}
