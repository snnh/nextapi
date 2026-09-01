//! 统计聚合 API（/api/stats/*，PLAN §7.2）。契约 contracts/m4-logging.md §10，M4-B 实现。
//!
//! - `GET /api/stats/summary`：query 同 StatsFilter（from/to 缺省 = 当天 00:00 → 现在）。
//! - `GET /api/stats/series`：+ granularity（缺省 区间 ≤2 天 → hour，否则 day）、dimension、currency。
//!
//! 全部 SQL 运行时校验；统计逻辑委托 `crate::stats`。

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::AdminUsername;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::stats::{self, StatsFilter};

/// `/api/stats/*` 查询参数。
#[derive(Debug, Default, Deserialize)]
struct StatsQuery {
    from_ts: Option<String>,
    to_ts: Option<String>,
    key_id: Option<String>,
    upstream_id: Option<String>,
    model: Option<String>,
    protocol: Option<String>,
    granularity: Option<String>,
    dimension: Option<String>,
    currency: Option<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/summary", get(summary))
        .route("/series", get(series))
}

/// GET /api/stats/summary。
async fn summary(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Query(q): Query<StatsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let tz = state.hot.load().gateway.billing_timezone.clone();
    let f = build_stats_filter(&q, &tz)?;
    let s = stats::summary(&state.db, &f).await?;
    Ok(Json(serde_json::json!(s)))
}

/// GET /api/stats/series。
async fn series(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Query(q): Query<StatsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let tz = state.hot.load().gateway.billing_timezone.clone();
    let f = build_stats_filter(&q, &tz)?;
    let granularity = resolve_granularity(&q, &f)?;
    let dimension = validate_dimension(q.dimension.as_deref())?;
    let currency = resolve_currency(&q, &state)?;
    let points = stats::series(&state.db, &f, &granularity, dimension).await?;
    Ok(Json(serde_json::json!({ "currency": currency, "points": points })))
}

/// 解析查询参数为 StatsFilter（from_ts/to_ts 缺省 = billing_timezone 当天 00:00 → 现在）。
fn build_stats_filter(q: &StatsQuery, tz: &str) -> ApiResult<StatsFilter> {
    let now = Utc::now();
    let from_ts = match q.from_ts.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => parse_rfc3339(s)?,
        None => crate::limit::window_start("daily", tz, now),
    };
    let to_ts = match q.to_ts.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => parse_rfc3339(s)?,
        None => now,
    };
    let key_id = q.key_id.as_deref().filter(|s| !s.is_empty()).map(parse_uuid).transpose()?;
    let upstream_id = q.upstream_id.as_deref().filter(|s| !s.is_empty()).map(parse_uuid).transpose()?;

    Ok(StatsFilter {
        from_ts,
        to_ts,
        key_id,
        upstream_id,
        model: q.model.clone().filter(|s| !s.is_empty()),
        protocol: q.protocol.clone().filter(|s| !s.is_empty()),
        billing_tz: tz.to_string(),
    })
}

/// 解析 granularity：缺省按区间自动选择（≤2 天 → hour，否则 day）；显式时必须为 hour|day。
fn resolve_granularity(q: &StatsQuery, f: &StatsFilter) -> ApiResult<String> {
    if let Some(g) = q.granularity.as_deref().filter(|s| !s.is_empty()) {
        if !matches!(g, "hour" | "day") {
            return Err(ApiError::bad_request("granularity 必须为 hour 或 day"));
        }
        return Ok(g.to_string());
    }
    Ok(default_granularity(f.from_ts, f.to_ts).to_string())
}

/// granularity 自动选择（纯函数，可测）：区间 ≤ 2 天 → "hour"，否则 → "day"。
fn default_granularity(from_ts: DateTime<Utc>, to_ts: DateTime<Utc>) -> &'static str {
    let span_days = (to_ts - from_ts).num_seconds() as f64 / 86_400.0;
    if span_days <= 2.0 {
        "hour"
    } else {
        "day"
    }
}

/// dimension 白名单校验。
fn validate_dimension(d: Option<&str>) -> ApiResult<Option<&'static str>> {
    match d {
        None | Some("") => Ok(None),
        Some("model") => Ok(Some("model")),
        Some("key") => Ok(Some("key")),
        Some("upstream") => Ok(Some("upstream")),
        Some("protocol") => Ok(Some("protocol")),
        Some(other) => Err(ApiError::bad_request(format!(
            "dimension 必须为 model/key/upstream/protocol: {other}"
        ))),
    }
}

/// currency 校验：仅显式传入时校验（默认 display_currency 直接透传；M4 成本恒 NULL）。
fn resolve_currency(q: &StatsQuery, state: &AppState) -> ApiResult<String> {
    match q.currency.as_deref().filter(|s| !s.is_empty()) {
        Some(c) if matches!(c, "CNY" | "USD") => Ok(c.to_string()),
        Some(c) => Err(ApiError::bad_request(format!("currency 必须为 CNY 或 USD: {c}"))),
        None => Ok(state.hot.load().gateway.display_currency.clone()),
    }
}

/// RFC3339 解析 → Utc；失败 → BadRequest。
fn parse_rfc3339(s: &str) -> ApiResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| ApiError::bad_request(format!("时间戳解析失败（需 RFC3339）: {e}")))
}

/// UUID 解析 → 失败 → BadRequest。
fn parse_uuid(s: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(s).map_err(|e| ApiError::bad_request(format!("UUID 解析失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).single().expect("构建 UTC 时间")
    }

    #[test]
    fn default_granularity_short_span_hour() {
        let f = utc(2024, 3, 1, 0, 0, 0);
        let t = utc(2024, 3, 2, 0, 0, 0); // 1 天 ≤ 2
        assert_eq!(default_granularity(f, t), "hour");
    }

    #[test]
    fn default_granularity_long_span_day() {
        let f = utc(2024, 3, 1, 0, 0, 0);
        let t = utc(2024, 3, 10, 0, 0, 0); // 9 天 > 2
        assert_eq!(default_granularity(f, t), "day");
    }

    #[test]
    fn default_granularity_two_days_boundary() {
        let f = utc(2024, 3, 1, 0, 0, 0);
        // 恰 2 天 → hour
        assert_eq!(default_granularity(f, utc(2024, 3, 3, 0, 0, 0)), "hour");
        // 略超 2 天 → day
        assert_eq!(default_granularity(f, utc(2024, 3, 3, 0, 0, 1)), "day");
    }

    #[test]
    fn validate_dimension_cases() {
        assert_eq!(validate_dimension(None).unwrap(), None);
        assert_eq!(validate_dimension(Some("")).unwrap(), None);
        assert_eq!(validate_dimension(Some("model")).unwrap(), Some("model"));
        assert_eq!(validate_dimension(Some("key")).unwrap(), Some("key"));
        assert_eq!(validate_dimension(Some("upstream")).unwrap(), Some("upstream"));
        assert_eq!(validate_dimension(Some("protocol")).unwrap(), Some("protocol"));
        assert!(matches!(validate_dimension(Some("bad")), Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn build_stats_filter_ts_parse_fail_bad_request() {
        let q = StatsQuery { from_ts: Some("bad".into()), ..Default::default() };
        assert!(matches!(build_stats_filter(&q, "Asia/Shanghai"), Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn build_stats_filter_parses_dates_and_tz() {
        let q = StatsQuery {
            from_ts: Some("2024-01-01T00:00:00Z".into()),
            to_ts: Some("2024-01-02T00:00:00Z".into()),
            model: Some("gpt-4o".into()),
            protocol: Some("openai_chat".into()),
            ..Default::default()
        };
        let f = build_stats_filter(&q, "Asia/Shanghai").unwrap();
        assert_eq!(f.from_ts, utc(2024, 1, 1, 0, 0, 0));
        assert_eq!(f.to_ts, utc(2024, 1, 2, 0, 0, 0));
        assert_eq!(f.billing_tz, "Asia/Shanghai");
        assert_eq!(f.model.as_deref(), Some("gpt-4o"));
        assert_eq!(f.protocol.as_deref(), Some("openai_chat"));
    }

    #[test]
    fn build_stats_filter_empty_ts_defaults() {
        let q = StatsQuery {
            from_ts: Some(String::new()),
            to_ts: Some(String::new()),
            ..Default::default()
        };
        let f = build_stats_filter(&q, "Asia/Shanghai").unwrap();
        assert!(f.from_ts <= f.to_ts);
    }
}
