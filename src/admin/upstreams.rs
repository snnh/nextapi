//! 上游渠道管理 API（/api/upstreams，PLAN.md §5.3 / §7.2）。
//!
//! - `api_key_enc` / `api_key_plain` 绝不对外输出，响应改用 `has_api_key: bool`；
//!   POST /{id}/reveal-key 例外：管理员密码（+ TOTP）安全验证通过后才解密返回明文，仅本次展示；
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
use sqlx::{FromRow, Row};
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
        .route("/{id}/quota", get(quota_get).post(quota_probe))
        .route("/devin/pkce/start", post(devin_pkce_start))
        .route("/devin/pkce/exchange", post(devin_pkce_exchange))
        .route("/{id}/oauth/refresh", post(refresh_oauth))
        .route("/{id}/reveal-key", post(reveal_upstream_key))
}

/// POST /devin/pkce/start：生成 Devin CLI PKCE 授权会话（无状态）。
///
/// 响应携带授权 URL（浏览器打开完成授权）与 code_verifier（前端暂存，兑换时回传）。
async fn devin_pkce_start() -> Json<serde_json::Value> {
    let s = crate::upstream::devin::pkce_start();
    Json(serde_json::json!({
        "authorize_url": s.authorize_url,
        "state": s.state,
        "code_verifier": s.code_verifier,
    }))
}

/// POST /devin/pkce/exchange：授权码 + code_verifier → session token。
///
/// token 即上游凭证（`devin-session-token$…`），由前端填入 api_key 字段加密保存；
/// 本接口不落库、不记日志。token 过期处理本期不做（过期报错）。
#[derive(Debug, Deserialize)]
struct DevinPkceExchangeReq {
    code: String,
    code_verifier: String,
    /// 可选自定义基址（默认官方）
    #[serde(default)]
    base_url: Option<String>,
}

async fn devin_pkce_exchange(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DevinPkceExchangeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let code = body.code.trim();
    let verifier = body.code_verifier.trim();
    if code.is_empty() || verifier.is_empty() {
        return Err(ApiError::bad_request("code 与 code_verifier 均不能为空"));
    }
    let base = body
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(crate::upstream::devin::DEFAULT_BASE_URL);
    let client = state.client_pools.direct_client();
    let r = crate::upstream::devin::exchange_pkce(&client, base, code, verifier, 15000)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "token": r.token,
        "webapp_host": r.webapp_host,
        "api_url": r.api_url,
    })))
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
    model_sync: String,
    model_exclude: Vec<String>,
    models_cache: serde_json::Value,
    models_fetched_at: Option<DateTime<Utc>>,
    oauth_enc: Option<String>,
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
    model_sync: String,
    model_exclude: Vec<String>,
    models_cache: serde_json::Value,
    models_fetched_at: Option<DateTime<Utc>>,
    /// Codex OAuth：仅暴露状态（账号/过期），绝不输出 token
    has_oauth: bool,
    oauth_account_id: Option<String>,
    oauth_expires_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn to_out(row: UpstreamDbRow, crypto: &crate::crypto::Crypto) -> UpstreamOut {
    // oauth 元数据（account_id/expires_at）：解密仅取非敏感字段展示
    let (oauth_account_id, oauth_expires_at) = match row.oauth_enc.as_deref() {
        Some(enc) if !enc.is_empty() => match crypto.decrypt(enc) {
            Ok(json) => match serde_json::from_str::<crate::upstream::codex::CodexOAuth>(&json) {
                Ok(o) => (
                    Some(o.account_id),
                    chrono::DateTime::from_timestamp(o.expires_at, 0),
                ),
                Err(_) => (None, None),
            },
            Err(_) => (None, None),
        },
        _ => (None, None),
    };
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
        model_sync: row.model_sync,
        model_exclude: row.model_exclude,
        models_cache: row.models_cache,
        models_fetched_at: row.models_fetched_at,
        has_oauth: row.oauth_enc.is_some(),
        oauth_account_id,
        oauth_expires_at,
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
    /// codex 渠道可缺省（默认官方后端地址）；其余渠道空串由 validate_upstream 拒绝
    #[serde(default)]
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
    /// 模型同步策略：manual（默认）| auto（跟随上游自动更新托管路由）
    #[serde(default)]
    model_sync: Option<String>,
    /// 自动同步排除名单
    #[serde(default)]
    model_exclude: Option<Vec<String>>,
    /// Codex 渠道：粘贴 codex CLI 的 auth.json 原文（解析后加密存储，不落明文）
    #[serde(default)]
    auth_json: Option<String>,
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
    #[serde(default)]
    model_sync: Option<String>,
    #[serde(default)]
    model_exclude: Option<Vec<String>>,
    #[serde(default)]
    auth_json: Option<String>,
}

/// GET /：列表（has_api_key 布尔；绝不输出 api_key）。
async fn list_upstreams(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    let rows: Vec<UpstreamDbRow> = sqlx::query_as::<_, UpstreamDbRow>(
        "SELECT id, name, kind, base_url, api_key_enc, protocols, enabled, timeout_ms, \
         breaker_threshold, probe_model, consecutive_failures, disabled_by, cooldown_until, \
         use_proxy, proxy_id, extra, model_sync, model_exclude, models_cache, models_fetched_at, \
         oauth_enc, created_at, updated_at FROM upstreams ORDER BY name",
    )
    .fetch_all(&state.db)
    .await?;
    let items: Vec<UpstreamOut> = rows.into_iter().map(|r| to_out(r, &state.crypto)).collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

/// POST /：创建上游。
async fn create_upstream(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Json(mut body): Json<CreateUpstreamReq>,
) -> ApiResult<Json<serde_json::Value>> {
    // Codex 渠道默认值补全须在 validate 之前（base_url 可缺省）
    let is_codex = body.auth_json.is_some() || body.kind.as_deref() == Some("codex");
    if is_codex && body.base_url.trim().is_empty() {
        body.base_url = crate::upstream::codex::DEFAULT_BASE_URL.to_string();
    }
    if is_codex && body.kind.is_none() {
        body.kind = Some("codex".to_string());
    }
    if is_codex && body.protocols.is_none() {
        body.protocols = Some(vec!["openai_responses".to_string()]);
    }
    // Devin 渠道（kind='devin'）：默认官方基址 + Responses 入口；
    // 凭证为 devin CLI 的 session token（api_key 加密存储，PKCE 兑换后粘贴）
    let is_devin = body.kind.as_deref() == Some("devin");
    if is_devin && body.base_url.trim().is_empty() {
        body.base_url = crate::upstream::devin::DEFAULT_BASE_URL.to_string();
    }
    if is_devin && body.protocols.is_none() {
        body.protocols = Some(vec!["openai_responses".to_string()]);
    }

    validate_upstream(
        &body.name,
        &body.base_url,
        body.protocols.as_deref(),
        body.timeout_ms,
        body.breaker_threshold,
    )?;

    let api_key_enc = encrypt_create_api_key(&state, body.api_key.as_deref())?;
    let protocols = body
        .protocols
        .clone()
        .unwrap_or_else(|| vec!["openai_chat".to_string()]);
    let enabled = body.enabled.unwrap_or(true);
    let disabled_by = if enabled {
        None
    } else {
        Some("manual".to_string())
    };
    let timeout_ms = body.timeout_ms.unwrap_or(300000);
    let breaker_threshold = body.breaker_threshold.unwrap_or(5);
    let extra = body.extra.clone().unwrap_or_else(|| serde_json::json!({}));
    let model_sync = body
        .model_sync
        .clone()
        .unwrap_or_else(|| "manual".to_string());
    let model_exclude = body.model_exclude.clone().unwrap_or_default();
    validate_model_sync(&model_sync, &model_exclude)?;

    // Codex 渠道：auth.json 解析加密
    let oauth_enc = match api_key_semantics(body.auth_json.as_deref()) {
        ApiKeySemantics::Keep | ApiKeySemantics::Clear => None,
        ApiKeySemantics::Set(raw) => Some(encrypt_auth_json(&state, &raw)?),
    };

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO upstreams \
         (name, kind, base_url, api_key_enc, protocols, enabled, timeout_ms, breaker_threshold, \
          probe_model, consecutive_failures, disabled_by, cooldown_until, use_proxy, proxy_id, extra, \
          model_sync, model_exclude, oauth_enc) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,0,$10,NULL,$11,$12,$13,$14,$15,$16) RETURNING id",
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
    .bind(model_sync)
    .bind(&model_exclude)
    .bind(&oauth_enc)
    .fetch_one(&state.db)
    .await?;

    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
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
    Ok(Json(serde_json::json!(to_out(row, &state.crypto))))
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
    let base_url = {
        let b = body.base_url.unwrap_or(row.base_url.clone());
        if kind == "devin" && b.trim().is_empty() {
            crate::upstream::devin::DEFAULT_BASE_URL.to_string()
        } else {
            b
        }
    };
    let protocols = body.protocols.unwrap_or_else(|| row.protocols.clone());
    let enabled = body.enabled.unwrap_or(row.enabled);
    let timeout_ms = body.timeout_ms.unwrap_or(row.timeout_ms);
    let breaker_threshold = body.breaker_threshold.unwrap_or(row.breaker_threshold);
    let probe_model = body.probe_model.or(row.probe_model.clone());
    let use_proxy = body.use_proxy.unwrap_or(row.use_proxy);
    let proxy_id = body.proxy_id.or(row.proxy_id);
    let extra = body.extra.unwrap_or_else(|| row.extra.clone());
    let model_sync = body.model_sync.unwrap_or_else(|| row.model_sync.clone());
    let model_exclude = body
        .model_exclude
        .unwrap_or_else(|| row.model_exclude.clone());
    validate_model_sync(&model_sync, &model_exclude)?;

    // auth_json：未传/掩码保持、空串清除、其他按 auth.json 解析加密更新
    let oauth_enc = match api_key_semantics(body.auth_json.as_deref()) {
        ApiKeySemantics::Keep => row.oauth_enc.clone(),
        ApiKeySemantics::Clear => None,
        ApiKeySemantics::Set(raw) => Some(encrypt_auth_json(&state, &raw)?),
    };

    validate_upstream(
        &name,
        &base_url,
        Some(&protocols),
        Some(timeout_ms),
        Some(breaker_threshold),
    )?;

    // api_key：未传/掩码保持、空串清除、其他加密更新
    let api_key_enc = match api_key_semantics(body.api_key.as_deref()) {
        ApiKeySemantics::Keep => row.api_key_enc,
        ApiKeySemantics::Clear => None,
        ApiKeySemantics::Set(plain) => {
            if !state.crypto.is_available() {
                return Err(ApiError::bad_request(
                    "未设置 NEXTAPI_SECRET_KEY，无法保存上游鉴权 Key",
                ));
            }
            Some(
                state
                    .crypto
                    .encrypt(&plain)
                    .map_err(|e| ApiError::bad_request(e.to_string()))?,
            )
        }
    };

    // enabled 切换：disabled_by / cooldown_until / consecutive_failures 联动
    let (disabled_by, cooldown_until, consecutive_failures) = if body.enabled.is_some() {
        if enabled {
            (None, None, 0)
        } else {
            (
                Some("manual".to_string()),
                row.cooldown_until,
                row.consecutive_failures,
            )
        }
    } else {
        (
            row.disabled_by,
            row.cooldown_until,
            row.consecutive_failures,
        )
    };

    sqlx::query(
        "UPDATE upstreams SET name=$1, kind=$2, base_url=$3, api_key_enc=$4, protocols=$5, \
         enabled=$6, timeout_ms=$7, breaker_threshold=$8, probe_model=$9, consecutive_failures=$10, \
         disabled_by=$11, cooldown_until=$12, use_proxy=$13, proxy_id=$14, extra=$15, \
         model_sync=$16, model_exclude=$17, oauth_enc=$18, updated_at=now() \
         WHERE id=$19",
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
    .bind(cooldown_until)
    .bind(use_proxy)
    .bind(proxy_id)
    .bind(&extra)
    .bind(&model_sync)
    .bind(&model_exclude)
    .bind(&oauth_enc)
    .bind(id)
    .execute(&state.db)
    .await?;

    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    // 启停切换后清除内存熔断状态
    if body.enabled.is_some() {
        state.breaker.reset(id);
    }
    // Key 是否随本次保存被显式变更（Set/Clear；未传或掩码=Keep）——审计留痕，便于排查"Key 何时被改"
    let api_key_changed = matches!(
        api_key_semantics(body.api_key.as_deref()),
        ApiKeySemantics::Set(_) | ApiKeySemantics::Clear
    );
    auth::audit(
        &state,
        &admin.0,
        "upstream.update",
        "upstream",
        Some(&id.to_string()),
        serde_json::json!({ "name": name, "enabled": enabled, "disabled_by": disabled_by,
                            "api_key_changed": api_key_changed }),
        None,
    )
    .await?;

    let row = fetch_upstream(&state, id).await?;
    Ok(Json(serde_json::json!(to_out(row, &state.crypto))))
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

    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth::audit(
        &state,
        &admin.0,
        "upstream.delete",
        "upstream",
        Some(&id.to_string()),
        serde_json::json!({}),
        None,
    )
    .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// codex 额度探测的缓存有效期（秒）：期内直接复用快照，不再发上游请求（省额度）。
const QUOTA_CACHE_SECS: i64 = 300;

/// GET /{id}/quota：最近一次额度快照（真实流量旁路抓取或上次探测结果）。
async fn quota_get(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let snap = state.cache.snapshot();
    let up = snap.upstreams.get(&id).cloned().ok_or(ApiError::NotFound)?;
    let quota = crate::upstream::quota::load(&state.db, id).await;
    Ok(Json(serde_json::json!({
        "id": id,
        "kind": up.kind,
        "supported": supports_quota(&up.kind),
        "quota": quota,
    })))
}

/// POST /{id}/quota：主动探测上游额度。
///
/// - `codex`：发一次极小 Responses 请求（模型取 probe_model → 模型缓存首个 → 官方默认），
///   从响应头解析 `x-codex-*`（ChatGPT 订阅额度：5h/周窗口已用百分比 + 重置时间 + 积分）；
///   5 分钟内的既有快照直接复用（`probed=false`），避免无谓消耗。
/// - `devin`：`GetUserStatus` 的 plan 块（日/周额度 + 百分数 + 重置时刻 + 可用模型数）。
///
/// 失败一律返回 `ok:false` + error（HTTP 200，与 /test 同风格），不打断前端。
async fn quota_probe(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let snap = state.cache.snapshot();
    let up = snap.upstreams.get(&id).cloned().ok_or(ApiError::NotFound)?;
    let client =
        state
            .client_pools
            .client_for(&up, &snap, &state.hot.load().proxy.default_proxy_id);
    let cached = crate::upstream::quota::load(&state.db, id).await;
    let started = std::time::Instant::now();

    if !supports_quota(&up.kind) {
        return Err(ApiError::bad_request(
            "该渠道不支持额度探测（当前支持 codex / devin）",
        ));
    }

    // codex：新鲜快照直接复用（不消耗额度）
    if up.kind == "codex" {
        if let Some(c) = cached.as_ref() {
            if crate::upstream::quota::is_fresh(c, QUOTA_CACHE_SECS) {
                return Ok(Json(serde_json::json!({
                    "ok": true, "probed": false, "cached": true, "latency_ms": 0, "quota": c,
                })));
            }
        }
        let (token, account) = match crate::upstream::codex::ensure_token(&state, &up).await {
            Ok(t) => t,
            Err(e) => {
                return Ok(Json(serde_json::json!({
                    "ok": false, "probed": true, "latency_ms": started.elapsed().as_millis() as u64,
                    "error": format!("codex oauth: {e}"), "quota": cached,
                })))
            }
        };
        let sim = crate::upstream::codex::sim_config(&up.extra);
        let ident = crate::upstream::codex::Identity::resolve(
            None,
            up.id,
            "quota-probe",
            chrono::Utc::now().timestamp_millis(),
            &sim,
        );
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        crate::upstream::codex::apply_headers(&mut headers, &token, &account, &sim, &ident);
        // 极小请求：一句话输入 + 极小输出上限（只为拿响应头）
        let body = crate::upstream::codex::prepare_body(
            &serde_json::json!({
                "model": quota_probe_model(&up),
                "instructions": "Reply with OK.",
                "input": [{
                    "type": "message", "role": "user",
                    "content": [{ "type": "input_text", "text": "ping" }],
                }],
                "stream": true,
                "store": false,
                "max_output_tokens": 16,
            }),
            &ident,
            &sim,
        );
        let url = format!(
            "{}{}",
            up.base_url_for(Protocol::OpenaiResponses).trim_end_matches('/'),
            "/responses"
        );
        let resp = match client
            .post(&url)
            .headers(headers)
            .json(&body)
            .timeout(Duration::from_secs(30))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(Json(serde_json::json!({
                    "ok": false, "probed": true,
                    "latency_ms": started.elapsed().as_millis() as u64,
                    "error": e.to_string(), "quota": cached,
                })))
            }
        };
        let status = resp.status().as_u16();
        // 只需响应头（429 等错误响应同样带额度头）；body 不读，直接丢弃
        let fresh = crate::upstream::quota::snapshot_from_codex_headers(resp.headers(), "probe");
        drop(resp);
        let latency_ms = started.elapsed().as_millis() as u64;
        return Ok(Json(match fresh {
            Some(s) => {
                crate::upstream::quota::store(&state.db, id, &s).await;
                serde_json::json!({
                    "ok": true, "probed": true, "status": status,
                    "latency_ms": latency_ms, "quota": s,
                })
            }
            None => serde_json::json!({
                "ok": false, "probed": true, "status": status, "latency_ms": latency_ms,
                "error": "上游未返回 x-codex-* 额度头（可能非 ChatGPT 订阅账号，或端点不支持）",
                "quota": cached,
            }),
        }));
    }

    // devin：GetUserStatus 读取额度块
    let token = up
        .api_key_plain
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let Some(tok) = token else {
        return Ok(Json(serde_json::json!({
            "ok": false, "probed": true, "latency_ms": 0,
            "error": "尚未完成 Devin 授权：请先兑换 session token", "quota": cached,
        })));
    };
    let base = if up.base_url.trim().is_empty() {
        crate::upstream::devin::DEFAULT_BASE_URL
    } else {
        up.base_url.trim()
    };
    match crate::upstream::devin::user_status(&client, base, tok, 10_000).await {
        Ok(info) => {
            let s = crate::upstream::devin::quota_snapshot(&info);
            crate::upstream::quota::store(&state.db, id, &s).await;
            Ok(Json(serde_json::json!({
                "ok": true, "probed": true, "status": 200,
                "latency_ms": started.elapsed().as_millis() as u64, "quota": s,
            })))
        }
        Err(e) => Ok(Json(serde_json::json!({
            "ok": false, "probed": true,
            "latency_ms": started.elapsed().as_millis() as u64,
            "error": e.to_string(), "quota": cached,
        }))),
    }
}

/// 渠道是否支持额度探测（codex：响应头；devin：GetUserStatus）。可单测。
fn supports_quota(kind: &str) -> bool {
    matches!(kind, "codex" | "devin")
}

/// codex 探测模型：probe_model → 模型缓存首个 → 官方默认（探测只为取额度头）。
fn quota_probe_model(up: &crate::entities::UpstreamRow) -> String {
    if let Some(m) = up
        .probe_model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        return m.to_string();
    }
    if let Some(m) = up
        .models_cache
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| {
            v.get("name")
                .or_else(|| v.get("id"))
                .and_then(|x| x.as_str())
        })
    {
        if !m.trim().is_empty() {
            return m.to_string();
        }
    }
    "gpt-5.1-codex-mini".to_string()
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
    let client =
        state
            .client_pools
            .client_for(&up, &snap, &state.hot.load().proxy.default_proxy_id);

    // Devin 渠道：GetUserStatus 真实探测（返回账号/套餐/可用模型数）
    if up.kind == "devin" {
        return Ok(Json(
            crate::upstream::devin::probe_status(
                &client,
                &up.base_url,
                up.api_key_plain.as_deref(),
                10_000,
            )
            .await,
        ));
    }
    let proto = up
        .protocol_list()
        .first()
        .copied()
        .unwrap_or(Protocol::OpenaiChat);

    let url = up.models_url();
    let mut headers = reqwest::header::HeaderMap::new();
    if up.kind == "codex" {
        // Codex 渠道用 OAuth 凭证（临期自动刷新），无 api_key
        match upstream::codex::ensure_token(&state, &up).await {
            Ok((token, account)) => {
                let sim = upstream::codex::sim_config(&up.extra);
                let ident = upstream::codex::Identity::resolve(
                    None,
                    up.id,
                    "connectivity",
                    chrono::Utc::now().timestamp_millis(),
                    &sim,
                );
                upstream::codex::apply_headers(&mut headers, &token, &account, &sim, &ident);
                headers.insert(
                    reqwest::header::ACCEPT,
                    reqwest::header::HeaderValue::from_static("application/json"),
                );
            }
            Err(e) => {
                return Ok(Json(serde_json::json!({ "ok": false, "error": e })));
            }
        }
    } else {
        // api_key_plain 仅在内存快照中持有；此处加鉴权头（绝不回传）
        upstream::apply_auth(&mut headers, proto, up.api_key_plain.as_deref());
    }

    let started = std::time::Instant::now();
    let resp = client
        .get(&url)
        .headers(headers)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            Ok(Json(
                serde_json::json!({ "ok": status < 500, "status": status, "latency_ms": latency_ms }),
            ))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "ok": false, "latency_ms": latency_ms, "error": e.to_string() }),
        )),
    }
}

/// 校验模型同步策略与排除名单边界（review P2 风格：长度/数量上限）。
fn validate_model_sync(sync: &str, exclude: &[String]) -> Result<(), ApiError> {
    if sync != "manual" && sync != "auto" {
        return Err(ApiError::bad_request("model_sync 必须为 manual 或 auto"));
    }
    if exclude.len() > 500 {
        return Err(ApiError::bad_request("model_exclude 不能超过 500 条"));
    }
    for m in exclude {
        if m.is_empty() || m.len() > 200 {
            return Err(ApiError::bad_request("model_exclude 单项长度须为 1..=200"));
        }
    }
    Ok(())
}

/// 图片上游协议值（契约 m6：upstreams.protocols 新增四个 images_* 值，其余校验不变）。
fn is_images_protocol(p: &str) -> bool {
    matches!(
        p,
        "images_openai" | "images_gemini" | "images_dashscope_sync" | "images_dashscope_async"
    )
}

/// 校验上游：名称/地址非空、协议可解析（或属于图片协议白名单）、超时/熔断阈值 > 0。
fn validate_upstream(
    name: &str,
    base_url: &str,
    protocols: Option<&[String]>,
    timeout_ms: Option<i32>,
    breaker_threshold: Option<i32>,
) -> Result<(), ApiError> {
    const MAX_NAME: usize = 255;
    const MAX_URL: usize = 2048;
    const MAX_PROTOCOLS: usize = 16;
    const MAX_TIMEOUT_MS: i32 = 86_400_000; // 24h
    if name.trim().is_empty() {
        return Err(ApiError::bad_request("上游名称不能为空"));
    }
    if name.chars().count() > MAX_NAME {
        return Err(ApiError::bad_request(format!(
            "name 不能超过 {MAX_NAME} 字符"
        )));
    }
    if base_url.trim().is_empty() {
        return Err(ApiError::bad_request("上游 base_url 不能为空"));
    }
    if base_url.chars().count() > MAX_URL {
        return Err(ApiError::bad_request(format!(
            "base_url 不能超过 {MAX_URL} 字符"
        )));
    }
    let parsed = reqwest::Url::parse(base_url.trim())
        .map_err(|_| ApiError::bad_request("base_url 必须是合法 URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(ApiError::bad_request(
            "base_url 仅支持带主机的 http/https URL",
        ));
    }
    if parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ApiError::bad_request(
            "base_url 不得包含用户名、密码、查询参数或片段",
        ));
    }
    if let Some(ps) = protocols {
        if ps.len() > MAX_PROTOCOLS {
            return Err(ApiError::bad_request(format!(
                "protocols 最多 {MAX_PROTOCOLS} 项"
            )));
        }
        for p in ps {
            if p.parse::<Protocol>().is_err() && !is_images_protocol(p) {
                return Err(ApiError::bad_request(format!(
                    "protocols 包含非法协议: {p}"
                )));
            }
        }
    }
    if let Some(t) = timeout_ms {
        if t <= 0 {
            return Err(ApiError::bad_request("timeout_ms 必须 > 0"));
        }
        if t > MAX_TIMEOUT_MS {
            return Err(ApiError::bad_request("timeout_ms 超出上限（24h）"));
        }
    }
    if let Some(b) = breaker_threshold {
        if b <= 0 {
            return Err(ApiError::bad_request("breaker_threshold 必须 > 0"));
        }
        if b > 1_000_000 {
            return Err(ApiError::bad_request("breaker_threshold 超出上限"));
        }
    }
    Ok(())
}

/// 创建路径的 api_key 加密：非空即加密（密钥缺失 → BadRequest）；空/None → None。
fn encrypt_create_api_key(
    state: &AppState,
    api_key: Option<&str>,
) -> Result<Option<String>, ApiError> {
    match api_key {
        Some(ak) if !ak.is_empty() => {
            // 掩码字面量是「保持原值」的保留语义，创建路径无旧值可保持 → 拒绝（review P2-13）
            if ak == "***" {
                return Err(ApiError::bad_request(
                    "\"***\" 为掩码保留值，不能作为真实密钥",
                ));
            }
            if !state.crypto.is_available() {
                return Err(ApiError::bad_request(
                    "未设置 NEXTAPI_SECRET_KEY，无法保存上游鉴权 Key",
                ));
            }
            let enc = state
                .crypto
                .encrypt(ak)
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            Ok(Some(enc))
        }
        _ => Ok(None),
    }
}

/// 解析 auth.json 并加密为 oauth_enc；未配置 NEXTAPI_SECRET_KEY → 400。
fn encrypt_auth_json(state: &AppState, raw: &str) -> Result<String, ApiError> {
    if !state.crypto.is_available() {
        return Err(ApiError::bad_request(
            "未设置 NEXTAPI_SECRET_KEY，无法保存 OAuth 凭证",
        ));
    }
    let oauth = crate::upstream::codex::parse_auth_json(raw).map_err(ApiError::bad_request)?;
    state
        .crypto
        .encrypt(&serde_json::to_string(&oauth).map_err(ApiError::internal)?)
        .map_err(|e| ApiError::bad_request(e.to_string()))
}

/// POST /{id}/oauth/refresh：手动刷新 Codex access_token（调试/轮换冲突恢复用）。
async fn refresh_oauth(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let snap = state.cache.snapshot();
    let up = snap.upstreams.get(&id).cloned().ok_or(ApiError::NotFound)?;
    drop(snap);
    let oauth = up
        .oauth_plain
        .clone()
        .ok_or_else(|| ApiError::bad_request("该渠道未配置 OAuth 凭证"))?;
    let (_tok, account) = crate::upstream::codex::refresh_token(&state, &up, &oauth)
        .await
        .map_err(ApiError::bad_gateway)?;
    auth::audit(
        &state,
        &admin.0,
        "upstream.oauth.refresh",
        "upstream",
        Some(&id.to_string()),
        serde_json::json!({ "name": up.name, "account_id": account }),
        None,
    )
    .await?;
    Ok(Json(
        serde_json::json!({ "ok": true, "account_id": account }),
    ))
}

/// 查看明文 API Key 请求体：管理员密码二次验证（已启用 TOTP 时还需动态码）。
#[derive(Debug, Deserialize)]
struct RevealKeyReq {
    password: String,
    /// 已启用 TOTP 时必填（6 位数字）。
    #[serde(default)]
    totp_code: Option<String>,
}

/// POST /{id}/reveal-key：管理员密码（+ TOTP）安全验证通过后返回明文 API Key。
///
/// - 先验证凭据再取上游密文：错误信息与资源状态无关，未授权/弱口令请求无法探测上游；
/// - 明文仅本次响应展示，不做缓存/放行，每次查看都须重新验证；
/// - 每次成功查看写审计（action: upstream.key_reveal）；
/// - 未设置 API Key 的上游（如 codex OAuth 渠道）返回 400 不可查看。
async fn reveal_upstream_key(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
    Json(body): Json<RevealKeyReq>,
) -> ApiResult<Json<serde_json::Value>> {
    auth::sensitive_verify(&state, &admin.0, &body.password, body.totp_code.as_deref()).await?;

    let row = sqlx::query("SELECT name, api_key_enc FROM upstreams WHERE id=$1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    let name: String = row.get("name");
    let enc: Option<String> = row.try_get("api_key_enc").unwrap_or(None);
    let enc = enc.ok_or_else(|| ApiError::bad_request("该上游未设置 API Key，无需查看"))?;

    let api_key = state.crypto.decrypt(&enc).map_err(|e| {
        tracing::error!("解密上游 {id} 的 API Key 失败: {e}");
        ApiError::internal("API Key 解密失败")
    })?;

    auth::audit(
        &state,
        &admin.0,
        "upstream.key_reveal",
        "upstream",
        Some(&id.to_string()),
        serde_json::json!({ "name": name }),
        None,
    )
    .await?;

    Ok(Json(serde_json::json!({ "api_key": api_key })))
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
         use_proxy, proxy_id, extra, model_sync, model_exclude, models_cache, models_fetched_at, \
         oauth_enc, created_at, updated_at FROM upstreams WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::{quota_probe_model, supports_quota};

    #[test]
    fn quota_supported_kinds() {
        assert!(supports_quota("codex"));
        assert!(supports_quota("devin"));
        assert!(!supports_quota("openai"));
        assert!(!supports_quota("aliyun"));
    }

    #[test]
    fn quota_probe_model_precedence() {
        let mut up = crate::entities::UpstreamRow {
            id: Uuid::new_v4(),
            name: "up".into(),
            kind: "codex".into(),
            base_url: "https://chatgpt.com/backend-api/codex".into(),
            api_key_plain: None,
            oauth_plain: None,
            protocols: vec!["openai_responses".into()],
            enabled: true,
            timeout_ms: 300_000,
            breaker_threshold: 5,
            probe_model: None,
            consecutive_failures: 0,
            disabled_by: None,
            cooldown_until: None,
            use_proxy: false,
            proxy_id: None,
            extra: serde_json::json!({}),
            model_sync: "manual".into(),
            model_exclude: vec![],
            models_cache: serde_json::json!([{"name": "cached-model"}]),
            models_fetched_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        // 缓存首个模型优先于官方默认
        assert_eq!(quota_probe_model(&up), "cached-model");
        // probe_model 优先于缓存
        up.probe_model = Some("probe-model".into());
        assert_eq!(quota_probe_model(&up), "probe-model");
        // 无 probe_model 且缓存为空 → 官方默认（探测仅为取额度头）
        up.probe_model = None;
        up.models_cache = serde_json::json!([]);
        assert_eq!(quota_probe_model(&up), "gpt-5.1-codex-mini");
    }

    use super::*;

    #[test]
    fn api_key_semantics_cases() {
        assert_eq!(api_key_semantics(None), ApiKeySemantics::Keep);
        assert_eq!(api_key_semantics(Some("***")), ApiKeySemantics::Keep);
        assert_eq!(api_key_semantics(Some("")), ApiKeySemantics::Clear);
        assert_eq!(
            api_key_semantics(Some("new-key")),
            ApiKeySemantics::Set("new-key".into())
        );
    }

    #[test]
    fn validate_upstream_cases() {
        assert!(validate_upstream(
            "u",
            "https://x.com",
            Some(&["openai_chat".into()]),
            Some(300_000),
            Some(5)
        )
        .is_ok());
        assert!(validate_upstream("", "https://x.com", None, None, None).is_err());
        assert!(validate_upstream("u", "", None, None, None).is_err());
        assert!(validate_upstream(
            "u",
            "https://x.com",
            Some(&["bad_proto".into()]),
            None,
            None
        )
        .is_err());
        assert!(validate_upstream("u", "https://x.com", None, Some(0), None).is_err());
        assert!(validate_upstream("u", "https://x.com", None, None, Some(-1)).is_err());
    }

    #[test]
    fn validate_upstream_accepts_images_protocols() {
        // 契约 m6：新增四个 images_* 协议值放行
        for p in [
            "images_openai",
            "images_gemini",
            "images_dashscope_sync",
            "images_dashscope_async",
        ] {
            assert!(
                validate_upstream("u", "https://x.com", Some(&[p.into()]), None, None).is_ok(),
                "应放行 {p}"
            );
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
        assert!(validate_upstream(
            "u",
            "https://x.com",
            Some(&["images_bad".into()]),
            None,
            None
        )
        .is_err());
    }
}
