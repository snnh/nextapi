//! 网关 Key 管理 API（/api/keys，PLAN.md §5.2 / §7.2）。
//!
//! - Key 格式 `sk-nx-{32位随机小写字母数字}`，DB 只存 SHA-256 哈希 + 前 8 位前缀；
//! - 完整 Key 仅在创建/轮换时展示一次；列表与更新绝不输出 Key/key_hash/prefix 之外的敏感信息；
//! - quota_unit 枚举 `tokens/cost_cny/cost_usd`；quota_window 枚举 `daily/monthly/total`；
//! - debug_enabled=true 时 debug_expires_at = now + hot.gateway.log_debug_ttl_minutes 分钟，
//!   若显式传 debug_expires_at 且超过该上限则截断（上限 1 小时语义）；
//! - 所有写操作先提交 DB 再调 `state.cache.reload` 刷新快照并写审计（action: key.*）。
//!
//! 全部 SQL 使用运行时校验（sqlx::query / query_as），不使用 query! 宏。

use axum::{
    extract::{Path, State},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rand::Rng;
use rust_decimal::Decimal;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{self, AdminUsername};
use crate::entities::ApiKeyRow;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_keys).post(create_key))
        .route("/{id}", put(update_key).delete(delete_key))
        .route("/{id}/rotate", post(rotate_key))
}

/// 创建 Key 请求体。
#[derive(Debug, Deserialize)]
struct CreateKeyReq {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    models: Option<Vec<String>>,
    #[serde(default)]
    rpm: Option<i32>,
    #[serde(default)]
    tpm: Option<i32>,
    #[serde(default)]
    quota_limit: Option<Decimal>,
    #[serde(default)]
    quota_unit: Option<String>,
    #[serde(default)]
    quota_window: Option<String>,
    #[serde(default)]
    allow_upstream_passthrough: Option<bool>,
    #[serde(default)]
    passthrough_upstreams: Option<Vec<String>>,
    #[serde(default)]
    debug_enabled: Option<bool>,
    #[serde(default)]
    debug_expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
}

/// 更新 Key 请求体（全字段 Option；可空字段用 Option<Option<T>> 表达「显式置空」）。
#[derive(Debug, Deserialize)]
struct UpdateKeyReq {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    models: Option<Option<Vec<String>>>,
    #[serde(default)]
    rpm: Option<i32>,
    #[serde(default)]
    tpm: Option<i32>,
    #[serde(default)]
    quota_limit: Option<Option<Decimal>>,
    #[serde(default)]
    quota_unit: Option<Option<String>>,
    #[serde(default)]
    quota_window: Option<Option<String>>,
    #[serde(default)]
    allow_upstream_passthrough: Option<bool>,
    #[serde(default)]
    passthrough_upstreams: Option<Option<Vec<String>>>,
    #[serde(default)]
    debug_enabled: Option<bool>,
    #[serde(default)]
    debug_expires_at: Option<Option<DateTime<Utc>>>,
    #[serde(default)]
    expires_at: Option<Option<DateTime<Utc>>>,
}

/// GET /：列表。ApiKeyRow 已 Serialize 且 `key_hash` 为 `#[serde(skip)]`，
/// 因此响应天然不泄露哈希与完整 Key。
async fn list_keys(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    let items: Vec<ApiKeyRow> = sqlx::query_as::<_, ApiKeyRow>(
        "SELECT id, name, key_hash, prefix, enabled, models, rpm, tpm, quota_limit, \
         quota_unit, quota_window, allow_upstream_passthrough, passthrough_upstreams, \
         debug_enabled, debug_expires_at, expires_at, created_at, last_used_at \
         FROM api_keys ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

/// POST /：创建 Key，响应含完整 Key（仅此一次）+ 记录行。
async fn create_key(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Json(body): Json<CreateKeyReq>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_quota(body.quota_unit.as_deref(), body.quota_window.as_deref())?;
    validate_key_limits(
        body.name.as_deref().unwrap_or(""),
        body.models.as_deref(),
        body.rpm,
        body.tpm,
        body.quota_limit,
        body.passthrough_upstreams.as_deref(),
    )?;

    // 生成 Key / 哈希 / 前缀
    let key = generate_key();
    let key_hash = sha256_hex(&key);
    let prefix = key[..8].to_string();

    // debug 开关：由 now + ttl 推导过期时间（显式传值且超上限则截断）
    let ttl_minutes = state.hot.load().gateway.log_debug_ttl_minutes;
    let now = Utc::now();
    let (debug_enabled, debug_expires_at) = resolve_debug(
        body.debug_enabled.unwrap_or(false),
        body.debug_expires_at,
        ttl_minutes,
        now,
    );

    let name = body.name.unwrap_or_default();
    let enable_passthrough = body.allow_upstream_passthrough.unwrap_or(false);

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO api_keys \
         (name, key_hash, prefix, enabled, models, rpm, tpm, quota_limit, quota_unit, \
          quota_window, allow_upstream_passthrough, passthrough_upstreams, debug_enabled, \
          debug_expires_at, expires_at) \
         VALUES ($1,$2,$3,TRUE,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) RETURNING id",
    )
    .bind(&name)
    .bind(&key_hash)
    .bind(&prefix)
    .bind(&body.models)
    .bind(body.rpm)
    .bind(body.tpm)
    .bind(&body.quota_limit)
    .bind(&body.quota_unit)
    .bind(&body.quota_window)
    .bind(enable_passthrough)
    .bind(&body.passthrough_upstreams)
    .bind(debug_enabled)
    .bind(&debug_expires_at)
    .bind(&body.expires_at)
    .fetch_one(&state.db)
    .await?;

    // 刷新快照 + 审计
    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth::audit(
        &state,
        &admin.0,
        "key.create",
        "key",
        Some(&id.to_string()),
        serde_json::json!({ "name": name, "prefix": prefix, "debug_enabled": debug_enabled }),
        None,
    )
    .await?;

    let row = fetch_key(&state, id).await?;
    Ok(Json(serde_json::json!({ "key": key, "row": row })))
}

/// PUT /{id}：部分更新；debug 开关规则同创建。
async fn update_key(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateKeyReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let existing = fetch_key(&state, id).await?;

    // 合并出新值（未传沿用旧值；显式 null 仅针对可空字段 = 清空）
    let name = body.name.unwrap_or(existing.name.clone());
    let enabled = body.enabled.unwrap_or(existing.enabled);
    let models = body.models.unwrap_or_else(|| existing.models.clone());
    let rpm = body.rpm.or(existing.rpm);
    let tpm = body.tpm.or(existing.tpm);
    let quota_limit = body.quota_limit.unwrap_or(existing.quota_limit);
    let quota_unit = body.quota_unit.unwrap_or(existing.quota_unit.clone());
    let quota_window = body.quota_window.unwrap_or(existing.quota_window.clone());
    let allow_passthrough = body
        .allow_upstream_passthrough
        .unwrap_or(existing.allow_upstream_passthrough);
    let passthrough_upstreams = body
        .passthrough_upstreams
        .unwrap_or_else(|| existing.passthrough_upstreams.clone());
    let expires_at = body.expires_at.unwrap_or(existing.expires_at);

    validate_quota(quota_unit.as_deref(), quota_window.as_deref())?;
    validate_key_limits(
        &name,
        models.as_deref(),
        rpm,
        tpm,
        quota_limit,
        passthrough_upstreams.as_deref(),
    )?;

    // debug：仅当显式传 debug_enabled / debug_expires_at 时才重新推导，否则沿用现有状态。
    let debug_enabled = body.debug_enabled.unwrap_or(existing.debug_enabled);
    let (debug_enabled, debug_expires_at) =
        if body.debug_enabled.is_some() || body.debug_expires_at.is_some() {
            let ttl_minutes = state.hot.load().gateway.log_debug_ttl_minutes;
            resolve_debug(
                debug_enabled,
                body.debug_expires_at.flatten(),
                ttl_minutes,
                Utc::now(),
            )
        } else {
            (existing.debug_enabled, existing.debug_expires_at)
        };

    sqlx::query(
        "UPDATE api_keys SET name=$1, enabled=$2, models=$3, rpm=$4, tpm=$5, quota_limit=$6, \
         quota_unit=$7, quota_window=$8, allow_upstream_passthrough=$9, passthrough_upstreams=$10, \
         debug_enabled=$11, debug_expires_at=$12, expires_at=$13 WHERE id=$14",
    )
    .bind(&name)
    .bind(enabled)
    .bind(&models)
    .bind(rpm)
    .bind(tpm)
    .bind(&quota_limit)
    .bind(&quota_unit)
    .bind(&quota_window)
    .bind(allow_passthrough)
    .bind(&passthrough_upstreams)
    .bind(debug_enabled)
    .bind(&debug_expires_at)
    .bind(&expires_at)
    .bind(id)
    .execute(&state.db)
    .await?;

    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth::audit(
        &state,
        &admin.0,
        "key.update",
        "key",
        Some(&id.to_string()),
        serde_json::json!({ "name": name, "enabled": enabled, "debug_enabled": debug_enabled }),
        None,
    )
    .await?;

    let row = fetch_key(&state, id).await?;
    Ok(Json(serde_json::json!({ "row": row })))
}

/// DELETE /{id}：删除 Key。
async fn delete_key(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let res = sqlx::query("DELETE FROM api_keys WHERE id=$1")
        .bind(id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    // 清理内存中该 Key 的限流窗口与用量判定缓存（review P3）
    state.limiter.remove_key(id);
    state.quota_cache.remove(id);

    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth::audit(
        &state,
        &admin.0,
        "key.delete",
        "key",
        Some(&id.to_string()),
        serde_json::json!({}),
        None,
    )
    .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /{id}/rotate：轮换出全新 Key（原 key_hash/prefix 被覆盖，旧 Key 立即失效）。
async fn rotate_key(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    // 先确认存在（不存在 → NotFound）
    let _ = fetch_key(&state, id).await?;

    let key = generate_key();
    let key_hash = sha256_hex(&key);
    let prefix = key[..8].to_string();

    sqlx::query("UPDATE api_keys SET key_hash=$1, prefix=$2 WHERE id=$3")
        .bind(&key_hash)
        .bind(&prefix)
        .bind(id)
        .execute(&state.db)
        .await?;

    state
        .cache
        .reload(&state.db, &state.crypto)
        .await
        .map_err(ApiError::internal)?;
    auth::audit(
        &state,
        &admin.0,
        "key.rotate",
        "key",
        Some(&id.to_string()),
        serde_json::json!({ "prefix": prefix }),
        None,
    )
    .await?;

    let row = fetch_key(&state, id).await?;
    Ok(Json(serde_json::json!({ "key": key, "row": row })))
}

/// 按 id 读取 Key 行；不存在 → NotFound。
async fn fetch_key(state: &AppState, id: Uuid) -> ApiResult<ApiKeyRow> {
    sqlx::query_as::<_, ApiKeyRow>(
        "SELECT id, name, key_hash, prefix, enabled, models, rpm, tpm, quota_limit, \
         quota_unit, quota_window, allow_upstream_passthrough, passthrough_upstreams, \
         debug_enabled, debug_expires_at, expires_at, created_at, last_used_at \
         FROM api_keys WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)
}

/// 校验 quota_unit / quota_window 枚举（仅在提供时校验）。
fn validate_quota(unit: Option<&str>, window: Option<&str>) -> Result<(), ApiError> {
    if let Some(u) = unit {
        if !matches!(u, "tokens" | "cost_cny" | "cost_usd") {
            return Err(ApiError::bad_request(
                "quota_unit 必须为 tokens/cost_cny/cost_usd",
            ));
        }
    }
    if let Some(w) = window {
        if !matches!(w, "daily" | "monthly" | "total") {
            return Err(ApiError::bad_request(
                "quota_window 必须为 daily/monthly/total",
            ));
        }
    }
    Ok(())
}

/// Key 字段边界校验（review P2）：数值非负且不超上限、字符串/数组长度上限，
/// 防止负数 quota_limit 全量阻断与存储膨胀。
fn validate_key_limits(
    name: &str,
    models: Option<&[String]>,
    rpm: Option<i32>,
    tpm: Option<i32>,
    quota_limit: Option<Decimal>,
    passthrough: Option<&[String]>,
) -> Result<(), ApiError> {
    const MAX_RATE: i32 = 10_000_000; // rpm/tpm 上限
    const MAX_NAME: usize = 255;
    const MAX_MODELS: usize = 500;
    const MAX_ITEM_LEN: usize = 255;
    const MAX_PASSTHROUGH: usize = 200;

    if name.chars().count() > MAX_NAME {
        return Err(ApiError::bad_request(format!(
            "name 不能超过 {MAX_NAME} 字符"
        )));
    }
    for v in [rpm, tpm].into_iter().flatten() {
        if !(0..=MAX_RATE).contains(&v) {
            return Err(ApiError::bad_request(format!(
                "rpm/tpm 必须在 0..={MAX_RATE}"
            )));
        }
    }
    if let Some(ql) = quota_limit {
        if ql.is_sign_negative() {
            return Err(ApiError::bad_request("quota_limit 不能为负数"));
        }
        if ql > Decimal::from(1_000_000_000_000_000i64) {
            return Err(ApiError::bad_request("quota_limit 超出上限（1e15）"));
        }
    }
    if let Some(ms) = models {
        if ms.len() > MAX_MODELS {
            return Err(ApiError::bad_request(format!(
                "模型白名单最多 {MAX_MODELS} 项"
            )));
        }
        for m in ms {
            if m.chars().count() > MAX_ITEM_LEN {
                return Err(ApiError::bad_request(format!(
                    "模型名不能超过 {MAX_ITEM_LEN} 字符"
                )));
            }
        }
    }
    if let Some(ps) = passthrough {
        if ps.len() > MAX_PASSTHROUGH {
            return Err(ApiError::bad_request(format!(
                "透传上游最多 {MAX_PASSTHROUGH} 项"
            )));
        }
        for p in ps {
            if p.chars().count() > MAX_ITEM_LEN {
                return Err(ApiError::bad_request(format!(
                    "上游名不能超过 {MAX_ITEM_LEN} 字符"
                )));
            }
        }
    }
    Ok(())
}

/// 计算 debug 过期时间：关闭 → (false, None)；开启 → now+ttl，显式传值且超上限则截断。
fn resolve_debug(
    enabled: bool,
    requested: Option<DateTime<Utc>>,
    ttl_minutes: u64,
    now: DateTime<Utc>,
) -> (bool, Option<DateTime<Utc>>) {
    if !enabled {
        return (false, None);
    }
    let max = now + chrono::Duration::minutes(ttl_minutes as i64);
    let expires = requested.map(|e| e.min(max)).unwrap_or(max);
    (true, Some(expires))
}

/// 生成 `sk-nx-` + 32 位随机小写字母数字的网关 Key。
fn generate_key() -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    let mut token = String::with_capacity(32);
    for _ in 0..32 {
        let idx = rng.random_range(0..CHARSET.len());
        token.push(CHARSET[idx] as char);
    }
    format!("sk-nx-{token}")
}

/// SHA-256 十六进制（小写）。
fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let out = hasher.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_key_format() {
        let key = generate_key();
        assert!(key.starts_with("sk-nx-"));
        assert_eq!(key.len(), 6 + 32);
        // 前缀 = 前 8 字符（sk-nx- + 2 位 token）
        let prefix = &key[..8];
        assert_eq!(prefix.len(), 8);
        assert!(prefix.starts_with("sk-nx-"));
    }

    #[test]
    fn generate_key_random() {
        // 随机性：两次生成大概率不同
        assert_ne!(generate_key(), generate_key());
    }

    #[test]
    fn sha256_hex_deterministic() {
        assert_eq!(
            sha256_hex("sk-nx-abc"),
            "44745406c81c5fa8a6936b80dfe5fbf805f0432968148edc224d648de496afed".to_string()
        );
    }

    #[test]
    fn validate_quota_cases() {
        assert!(validate_quota(Some("tokens"), Some("daily")).is_ok());
        assert!(validate_quota(Some("cost_cny"), Some("monthly")).is_ok());
        assert!(validate_quota(Some("cost_usd"), Some("total")).is_ok());
        assert!(validate_quota(Some("bad"), None).is_err());
        assert!(validate_quota(None, Some("hourly")).is_err());
        assert!(validate_quota(None, None).is_ok());
    }

    #[test]
    fn resolve_debug_cases() {
        let now = Utc::now();
        let within = now + chrono::Duration::minutes(30);
        let beyond = now + chrono::Duration::minutes(1000);

        // 关闭 → (false, None)
        assert_eq!(resolve_debug(false, Some(beyond), 60, now), (false, None));
        // 开启无显式值 → now + 60min
        let (on, exp) = resolve_debug(true, None, 60, now);
        assert!(on);
        assert!((exp.unwrap() - now).num_minutes() == 60);
        // 显式值超过上限 → 截断到 now + 60min
        let (_on, exp) = resolve_debug(true, Some(beyond), 60, now);
        assert!((exp.unwrap() - now).num_minutes() == 60);
        // 显式值在窗口内 → 采用显式值
        let (_on, exp) = resolve_debug(true, Some(within), 60, now);
        assert!((exp.unwrap() - now).num_minutes() == 30);
    }
}
