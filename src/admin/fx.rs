//! 汇率管理 API（/api/fx/*，PLAN §7.2）。契约 contracts/m5-billing.md §9，M5-B 实现。
//!
//! - GET /：fx_rates 全量（manual+auto 分行），附 stale 判定；
//! - PUT /：body [{from,to,rate}] 手动 upsert（source='manual'，rate>0，仅 CNY/USD 之间的对）；
//! - POST /refresh：按代理矩阵构建 client 后调用 `billing::fx::fetch_and_store`。
//!
//! 全部 SQL 使用运行时校验（sqlx::query / query_as），不使用 query! 宏。

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderValue, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{self, AdminUsername};
use crate::billing::fx as billing_fx;
use crate::entities::FxRateRow;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_fx).put(put_fx))
        .route("/refresh", post(refresh_fx))
}

/// GET /：全量汇率（manual+auto 分行，附 stale 判定）。
async fn list_fx(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    let stale_minutes = state.hot.load().gateway.fx_stale_max_minutes;
    let now = chrono::Utc::now();

    let rows: Vec<FxRateRow> = sqlx::query_as::<_, FxRateRow>(
        "SELECT currency_from, currency_to, rate, source, fetched_at, updated_at \
         FROM fx_rates ORDER BY currency_from, currency_to, source",
    )
    .fetch_all(&state.db)
    .await?;

    let items: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|row| {
            let stale = row.source == "auto"
                && row
                    .fetched_at
                    .map(|ts| now.signed_duration_since(ts).num_minutes() > stale_minutes as i64)
                    .unwrap_or(true);
            serde_json::json!({
                "currency_from": row.currency_from,
                "currency_to": row.currency_to,
                "rate": row.rate,
                "source": row.source,
                "fetched_at": row.fetched_at,
                "updated_at": row.updated_at,
                "stale": stale,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

#[derive(Debug, Deserialize)]
struct PutFxItem {
    from: String,
    to: String,
    rate: Decimal,
}

/// PUT /：手动汇率 upsert（source='manual'）。
async fn put_fx(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Json(body): Json<Vec<PutFxItem>>,
) -> ApiResult<Json<serde_json::Value>> {
    if body.is_empty() {
        return Err(ApiError::bad_request("请求体不能为空"));
    }
    let mut updated = 0usize;
    let mut pairs: Vec<(String, String)> = Vec::new();
    let now = chrono::Utc::now();
    for item in &body {
        let from = item.from.trim().to_ascii_uppercase();
        let to = item.to.trim().to_ascii_uppercase();
        validate_pair(&from, &to)?;
        if item.rate <= Decimal::ZERO {
            return Err(ApiError::bad_request("rate 必须大于 0"));
        }
        sqlx::query(
            "INSERT INTO fx_rates (currency_from, currency_to, rate, source, fetched_at, updated_at) \
             VALUES ($1,$2,$3,'manual',NULL,$4) \
             ON CONFLICT (currency_from, currency_to, source) DO UPDATE SET \
               rate=EXCLUDED.rate, updated_at=now()",
        )
        .bind(&from)
        .bind(&to)
        .bind(item.rate)
        .bind(now)
        .execute(&state.db)
        .await?;
        updated += 1;
        pairs.push((from, to));
    }

    auth::audit(
        &state,
        &admin.0,
        "fx.put",
        "fx_rates",
        None,
        serde_json::json!({ "pairs": pairs, "updated": updated }),
        None,
    )
    .await?;

    Ok(Json(serde_json::json!({ "ok": true, "updated": updated })))
}

/// POST /refresh：立即联网拉取（fetch 内部先成功才 upsert，失败保留旧值）。
/// 失败 → 502 Bad Gateway + 错误消息。
async fn refresh_fx(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
) -> ApiResult<Response> {
    let hot = state.hot.load();
    let cfg = hot.fx_auto_fetch.clone();
    let probe_url = fx_probe_url(&cfg);
    let client = build_fx_client(&state, &cfg, &probe_url);

    match billing_fx::fetch_and_store(&state.db, &client, &cfg).await {
        Ok(report) => {
            auth::audit(
                &state,
                &admin.0,
                "fx.refresh",
                "fx_rates",
                None,
                serde_json::json!({ "report": report }),
                None,
            )
            .await?;
            let body = serde_json::json!({ "ok": true, "report": report });
            Ok(json_response(StatusCode::OK, body))
        }
        Err(e) => {
            let body = serde_json::json!({
                "error": { "message": e.to_string(), "type": "nextapi_error" }
            });
            Ok(json_response(StatusCode::BAD_GATEWAY, body))
        }
    }
}

/// 仅允许 CNY/USD 之间的对（from != to）。
fn validate_pair(from: &str, to: &str) -> ApiResult<()> {
    if !matches!(from, "CNY" | "USD") || !matches!(to, "CNY" | "USD") {
        return Err(ApiError::bad_request("仅允许 CNY/USD 之间的汇率对"));
    }
    if from == to {
        return Err(ApiError::bad_request("from 与 to 不能相同"));
    }
    Ok(())
}

/// 供 no_proxy 判定用的代表 URL（与 fetch_and_store 内部 URL 推导一致）。
fn fx_probe_url(cfg: &crate::config::FxAutoFetchCfg) -> String {
    match cfg.provider.as_str() {
        "ecb" => "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml".into(),
        "custom" => format!("https://{}", cfg.base),
        _ => format!("https://api.frankfurter.app/latest?base={}", cfg.base),
    }
}

/// 构建汇率拉取 client（按 fx_auto_fetch.use_proxy/proxy_id + 全局 no_proxy 并集）。
fn build_fx_client(
    state: &AppState,
    cfg: &crate::config::FxAutoFetchCfg,
    url: &str,
) -> reqwest::Client {
    let hot = state.hot.load();
    let snap = state.cache.snapshot();

    let mut lists = vec![hot.proxy.no_proxy.clone()];
    let eff_proxy_id = if cfg.use_proxy {
        Uuid::parse_str(&cfg.proxy_id)
            .ok()
            .or_else(|| Uuid::parse_str(&hot.proxy.default_proxy_id).ok())
    } else {
        None
    };
    if let Some(pid) = eff_proxy_id {
        if let Some(p) = snap.proxies.get(&pid) {
            lists.push(p.no_proxy.clone());
        }
    }
    let use_proxy = cfg.use_proxy && !crate::upstream::no_proxy_match(&lists, url);
    if use_proxy {
        if let Some(pid) = eff_proxy_id {
            if let Some(p) = snap.proxies.get(&pid) {
                if let Some(client) = proxy_client(p) {
                    return client;
                }
            }
        }
    }
    state.client_pools.direct_client()
}

fn proxy_client(p: &crate::entities::ProxyRow) -> Option<reqwest::Client> {
    let proxy = reqwest::Proxy::all(p.proxy_url()).ok()?;
    reqwest::Client::builder().proxy(proxy).build().ok()
}

fn json_response(status: StatusCode, body: serde_json::Value) -> Response {
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    let mut resp = Response::new(Body::from(bytes));
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_pairs() {
        assert!(validate_pair("USD", "CNY").is_ok());
        assert!(validate_pair("CNY", "USD").is_ok());
    }

    #[test]
    fn reject_bad_pairs() {
        assert!(validate_pair("EUR", "CNY").is_err());
        assert!(validate_pair("CNY", "EUR").is_err());
        assert!(validate_pair("CNY", "CNY").is_err());
        assert!(validate_pair("", "CNY").is_err());
    }
}
