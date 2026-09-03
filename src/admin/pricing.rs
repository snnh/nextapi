//! 价格规则管理 API（/api/pricing/*，PLAN §7.2）。契约 contracts/m5-billing.md §8，M5-B 实现。
//!
//! - CRUD：unit/currency/context_basis 白名单、base_price>=0、dimensions/segments 为合法 JSON，
//!   dimension_key 由服务端 `normalize_dimension_key` 重算（不信任客户端）；唯一键冲突 → 409；
//! - preview：调 `billing::price_one`，响应 Decimal 字段按 display_precision 舍入；
//! - suggest：同 model_id 在其他供应商的规则（按 upstream 分组）；
//! - unpriced：snapshot.routes 遍历 + 一次全量启用的 price_rules 内存差集；
//! - export：全量规则按 (upstream, model_id, currency) 聚合为 `PriceExportItem`；
//! - import：multipart(file) 或 JSON url，逐行校验 + 单事务 upsert，source='import'。
//!
//! 全部 SQL 使用运行时校验（sqlx::query / QueryBuilder），不使用 query! 宏。

use axum::{
    body::Body,
    extract::{FromRequest, Multipart, Path, Query, Request, State},
    http::{header, HeaderValue},
    response::Response,
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, QueryBuilder};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::auth::{self, AdminUsername};
use crate::billing::transfer::{self, PriceExportItem};
use crate::billing::{normalize_dimension_key, price_one, PricingInput, PricingResult};
use crate::entities::PriceRuleRow;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// price_rules 查询列清单（与 entities::PriceRuleRow 字段一一对应）。
const PRICE_RULE_COLS: &str = "id, upstream_id, model_id, unit, currency, base_price, source, \
    dimensions, dimension_key, segments, context_basis, effective_from, effective_to, \
    priority, sort_order, enabled, created_at, updated_at";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_rules).post(create_rule))
        // 静态段优先于 /{id} 动态段。
        .route("/preview", post(preview_price))
        .route("/suggest", get(suggest_rules))
        .route("/unpriced", get(unpriced))
        .route("/export", get(export_rules))
        .route("/import", post(import_rules))
        .route("/{id}", put(update_rule).delete(delete_rule))
}

// ---------------------------------------------------------------------------
// 列表 / 创建
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQuery {
    upstream_id: Option<String>,
    model_id: Option<String>,
}

/// 列表返回项：price_rules 全字段 + upstream_name（内存快照 join）。
#[derive(Debug, Serialize)]
struct RuleItem {
    #[serde(flatten)]
    row: PriceRuleRow,
    upstream_name: Option<String>,
}

fn parse_uuid(s: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(s).map_err(|e| ApiError::bad_request(format!("UUID 解析失败: {e}")))
}

/// GET /：列全部规则，支持 upstream_id / model_id 过滤。
async fn list_rules(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let up_id = q.upstream_id.as_deref().filter(|s| !s.is_empty()).map(parse_uuid).transpose()?;
    let model = q.model_id.clone().filter(|s| !s.is_empty());

    let mut qb = QueryBuilder::<Postgres>::new(format!("SELECT {PRICE_RULE_COLS} FROM price_rules WHERE TRUE"));
    if let Some(up) = up_id {
        qb.push(" AND upstream_id = ").push_bind(up);
    }
    if let Some(m) = model.as_deref() {
        qb.push(" AND model_id = ").push_bind(m.to_string());
    }
    qb.push(" ORDER BY upstream_id, model_id, currency, unit, dimension_key, priority DESC");
    let rows: Vec<PriceRuleRow> = qb.build_query_as().fetch_all(&state.db).await?;

    let snap = state.cache.snapshot();
    let items: Vec<RuleItem> = rows
        .into_iter()
        .map(|row| RuleItem {
            upstream_name: snap.upstreams.get(&row.upstream_id).map(|u| u.name.clone()),
            row,
        })
        .collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

#[derive(Debug, Deserialize)]
struct CreateRuleReq {
    upstream_id: Uuid,
    model_id: String,
    unit: String,
    currency: String,
    base_price: Decimal,
    #[serde(default)]
    dimensions: Option<serde_json::Value>,
    #[serde(default)]
    segments: Option<serde_json::Value>,
    #[serde(default)]
    context_basis: Option<String>,
    #[serde(default)]
    effective_from: Option<DateTime<Utc>>,
    #[serde(default)]
    effective_to: Option<DateTime<Utc>>,
    #[serde(default)]
    priority: Option<i32>,
    #[serde(default)]
    sort_order: Option<i32>,
    #[serde(default)]
    enabled: Option<bool>,
}

/// POST /：创建规则（source='manual'；dimension_key 服务端重算）。
async fn create_rule(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Json(body): Json<CreateRuleReq>,
) -> ApiResult<Json<RuleItem>> {
    validate_create_fields(&body)?;
    let dimension_key = normalize_dimension_key(body.dimensions.as_ref());

    let row: PriceRuleRow = sqlx::query_as::<_, PriceRuleRow>(&format!(
        "INSERT INTO price_rules \
         (upstream_id, model_id, unit, currency, base_price, source, dimensions, dimension_key, \
          segments, context_basis, effective_from, effective_to, priority, sort_order, enabled) \
         VALUES ($1,$2,$3,$4,$5,'manual',$6,$7,$8,$9,$10,$11,$12,$13,$14) \
         RETURNING {PRICE_RULE_COLS}"
    ))
    .bind(body.upstream_id)
    .bind(&body.model_id)
    .bind(&body.unit)
    .bind(&body.currency)
    .bind(body.base_price)
    .bind(&body.dimensions)
    .bind(&dimension_key)
    .bind(&body.segments)
    .bind(body.context_basis.as_deref().unwrap_or("prompt_tokens"))
    .bind(body.effective_from)
    .bind(body.effective_to)
    .bind(body.priority.unwrap_or(10))
    .bind(body.sort_order.unwrap_or(0))
    .bind(body.enabled.unwrap_or(true))
    .fetch_one(&state.db)
    .await?;

    auth::audit(
        &state,
        &admin.0,
        "pricing.create",
        "price_rule",
        Some(&row.id.to_string()),
        serde_json::json!({
            "model_id": row.model_id, "unit": row.unit, "currency": row.currency,
            "base_price": row.base_price, "dimension_key": row.dimension_key,
        }),
        None,
    )
    .await?;

    let snap = state.cache.snapshot();
    let upstream_name = snap.upstreams.get(&row.upstream_id).map(|u| u.name.clone());
    Ok(Json(RuleItem { row, upstream_name }))
}

#[derive(Debug, Deserialize)]
struct UpdateRuleReq {
    #[serde(default)]
    upstream_id: Option<Uuid>,
    #[serde(default)]
    model_id: Option<String>,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    base_price: Option<Decimal>,
    /// 显式 null 可清空维度。
    #[serde(default)]
    dimensions: Option<Option<serde_json::Value>>,
    /// 显式 null 可清空分段。
    #[serde(default)]
    segments: Option<Option<serde_json::Value>>,
    #[serde(default)]
    context_basis: Option<String>,
    #[serde(default)]
    effective_from: Option<Option<DateTime<Utc>>>,
    #[serde(default)]
    effective_to: Option<Option<DateTime<Utc>>>,
    #[serde(default)]
    priority: Option<i32>,
    #[serde(default)]
    sort_order: Option<i32>,
    #[serde(default)]
    enabled: Option<bool>,
}

/// PUT /{id}：合并更新；不存在 → NotFound；维度变更时重算 dimension_key。
async fn update_rule(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRuleReq>,
) -> ApiResult<Json<RuleItem>> {
    let existing = fetch_rule(&state, id).await?;

    let upstream_id = body.upstream_id.unwrap_or(existing.upstream_id);
    let model_id = body.model_id.unwrap_or(existing.model_id);
    let unit = body.unit.unwrap_or(existing.unit);
    let currency = body.currency.unwrap_or(existing.currency);
    let base_price = body.base_price.unwrap_or(existing.base_price);
    let dimensions = match body.dimensions {
        Some(v) => v,
        None => existing.dimensions.clone(),
    };
    let segments = match body.segments {
        Some(v) => v,
        None => existing.segments.clone(),
    };
    let context_basis = body.context_basis.unwrap_or(existing.context_basis.clone());
    let effective_from = match body.effective_from {
        Some(v) => v,
        None => existing.effective_from,
    };
    let effective_to = match body.effective_to {
        Some(v) => v,
        None => existing.effective_to,
    };
    let priority = body.priority.unwrap_or(existing.priority);
    let sort_order = body.sort_order.unwrap_or(existing.sort_order);
    let enabled = body.enabled.unwrap_or(existing.enabled);

    validate_fields(
        &unit, &currency, Some(context_basis.as_str()), base_price, dimensions.as_ref(),
        segments.as_ref(),
    )?;
    let dimension_key = normalize_dimension_key(dimensions.as_ref());

    let row: PriceRuleRow = sqlx::query_as::<_, PriceRuleRow>(&format!(
        "UPDATE price_rules SET \
         upstream_id=$1, model_id=$2, unit=$3, currency=$4, base_price=$5, dimensions=$6, \
         dimension_key=$7, segments=$8, context_basis=$9, effective_from=$10, effective_to=$11, \
         priority=$12, sort_order=$13, enabled=$14, updated_at=now() \
         WHERE id=$15 RETURNING {PRICE_RULE_COLS}"
    ))
    .bind(upstream_id)
    .bind(&model_id)
    .bind(&unit)
    .bind(&currency)
    .bind(base_price)
    .bind(&dimensions)
    .bind(&dimension_key)
    .bind(&segments)
    .bind(&context_basis)
    .bind(effective_from)
    .bind(effective_to)
    .bind(priority)
    .bind(sort_order)
    .bind(enabled)
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    auth::audit(
        &state,
        &admin.0,
        "pricing.update",
        "price_rule",
        Some(&row.id.to_string()),
        serde_json::json!({
            "model_id": row.model_id, "unit": row.unit, "currency": row.currency,
            "base_price": row.base_price, "dimension_key": row.dimension_key, "enabled": row.enabled,
        }),
        None,
    )
    .await?;

    let snap = state.cache.snapshot();
    let upstream_name = snap.upstreams.get(&row.upstream_id).map(|u| u.name.clone());
    Ok(Json(RuleItem { row, upstream_name }))
}

/// DELETE /{id}：删除；不存在 → NotFound。
async fn delete_rule(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let res = sqlx::query("DELETE FROM price_rules WHERE id=$1")
        .bind(id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    auth::audit(&state, &admin.0, "pricing.delete", "price_rule", Some(&id.to_string()), serde_json::json!({}), None).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn fetch_rule(state: &AppState, id: Uuid) -> ApiResult<PriceRuleRow> {
    sqlx::query_as::<_, PriceRuleRow>(&format!("SELECT {PRICE_RULE_COLS} FROM price_rules WHERE id=$1"))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// preview
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PreviewReq {
    upstream_id: Uuid,
    model_id: String,
    #[serde(default)]
    at: Option<DateTime<Utc>>,
    #[serde(default)]
    prompt_tokens: Option<i64>,
    #[serde(default)]
    completion_tokens: Option<i64>,
    #[serde(default)]
    cache_write_tokens: Option<i64>,
    #[serde(default)]
    cache_read_tokens: Option<i64>,
    #[serde(default)]
    images: Option<i64>,
    #[serde(default)]
    image_size: Option<String>,
    #[serde(default)]
    video_seconds: Option<Decimal>,
    #[serde(default)]
    video_resolution: Option<String>,
    #[serde(default)]
    video_task_type: Option<String>,
}

/// POST /preview：模拟计价，响应 Decimal 按 display_precision 舍入。
async fn preview_price(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Json(body): Json<PreviewReq>,
) -> ApiResult<Json<PricingResult>> {
    let input = PricingInput {
        at: body.at.unwrap_or_else(Utc::now),
        prompt_tokens: body.prompt_tokens,
        completion_tokens: body.completion_tokens,
        cache_write_tokens: body.cache_write_tokens,
        cache_read_tokens: body.cache_read_tokens,
        images: body.images,
        image_size: body.image_size,
        video_seconds: body.video_seconds,
        video_resolution: body.video_resolution,
        video_task_type: body.video_task_type,
    };
    let hot = state.hot.load();
    let precision = hot.gateway.display_precision;
    let billing_tz = hot.gateway.billing_timezone.clone();
    let stale = hot.gateway.fx_stale_max_minutes;
    drop(hot);
    let mut result = price_one(&state.db, body.upstream_id, &body.model_id, &input, &billing_tz, stale).await?;
    round_priced_result(&mut result, precision);
    Ok(Json(result))
}

/// 把价格数值舍入到展示精度（Decimal 字段 + price_used 里的 price/base_price）。
fn round_priced_result(res: &mut PricingResult, precision: u32) {
    if let Some(v) = &mut res.cost_cny {
        *v = v.round_dp(precision);
    }
    if let Some(v) = &mut res.cost_usd {
        *v = v.round_dp(precision);
    }
    for line in &mut res.lines {
        line.price = line.price.round_dp(precision);
        line.cost = line.cost.round_dp(precision);
        line.quantity = line.quantity.round_dp(precision);
    }
    for arr in [&mut res.price_used, &mut res.fx_snapshot] {
        if let Some(items) = arr.as_array_mut() {
            for item in items {
                if let Some(obj) = item.as_object_mut() {
                    for k in ["price", "base_price", "rate"] {
                        if let Some(v) = obj.get_mut(k) {
                            if let Some(d) = json_to_decimal(v) {
                                *v = serde_json::json!(d.round_dp(precision));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 从 JSON 值转为 Decimal：本代码库中 Decimal 经 serde 序列化为**字符串**（带引号由
/// `Value::to_string()` 体现），因此需同时支持 Number 与 String 两种形态。
fn json_to_decimal(v: &serde_json::Value) -> Option<Decimal> {
    match v {
        serde_json::Value::Number(_) => Decimal::from_str_exact(&v.to_string()).ok(),
        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// suggest
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SuggestQuery {
    #[serde(default)]
    upstream_id: Option<String>,
    #[serde(default)]
    model_id: Option<String>,
}

/// GET /suggest：同 model_id 在其他供应商的规则，按 upstream 分组。
async fn suggest_rules(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Query(q): Query<SuggestQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let up_id = q
        .upstream_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(parse_uuid)
        .transpose()?
        .ok_or_else(|| ApiError::bad_request("请提供 upstream_id"))?;
    let model_id = q.model_id.clone().filter(|s| !s.is_empty());
    let Some(model_id) = model_id else {
        return Err(ApiError::bad_request("请提供 model_id"));
    };

    let rows: Vec<PriceRuleRow> = sqlx::query_as::<_, PriceRuleRow>(&format!(
        "SELECT {PRICE_RULE_COLS} FROM price_rules WHERE model_id=$1 AND upstream_id<>$2 \
         ORDER BY upstream_id, currency, unit, dimension_key"
    ))
    .bind(&model_id)
    .bind(up_id)
    .fetch_all(&state.db)
    .await?;

    let snap = state.cache.snapshot();
    // 按 upstream 分组（保序）。
    let mut order: Vec<Uuid> = Vec::new();
    let mut grouped: HashMap<Uuid, (String, Vec<PriceRuleRow>, DateTime<Utc>)> = HashMap::new();
    for row in rows {
        let name = snap
            .upstreams
            .get(&row.upstream_id)
            .map(|u| u.name.clone())
            .unwrap_or_default();
        let entry = grouped.entry(row.upstream_id).or_insert_with(|| {
            order.push(row.upstream_id);
            (name.clone(), Vec::new(), row.created_at)
        });
        entry.1.push(row.clone());
        if row.updated_at > entry.2 {
            entry.2 = row.updated_at;
        }
    }
    let items: Vec<serde_json::Value> = order
        .iter()
        .filter_map(|uid| {
            grouped.get(uid).map(|(name, rules, updated)| {
                serde_json::json!({
                    "upstream_id": uid,
                    "upstream_name": name,
                    "rules": rules,
                    "updated_at": updated,
                })
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

// ---------------------------------------------------------------------------
// unpriced
// ---------------------------------------------------------------------------

/// GET /unpriced：snapshot.routes 中有模型但无任何启用价格规则的 (upstream, model)。
async fn unpriced(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    // 一次全量读启用规则键。
    let keys: HashSet<(Uuid, String)> = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT upstream_id, model_id FROM price_rules WHERE enabled",
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();

    let snap = state.cache.snapshot();
    let mut seen: HashSet<(Uuid, String)> = HashSet::new();
    let mut items: Vec<serde_json::Value> = Vec::new();
    for route in snap.routes.iter() {
        // 跳过含通配符的 pattern 与 disabled/未加载的上游。
        if route.model_pattern.contains('*') {
            continue;
        }
        if !route.enabled {
            continue;
        }
        let Some(up) = snap.upstreams.get(&route.upstream_id) else {
            continue;
        };
        if !up.enabled {
            continue;
        }
        let model = route.override_model.clone().unwrap_or_else(|| route.model_pattern.clone());
        if keys.contains(&(route.upstream_id, model.clone())) {
            continue;
        }
        if seen.insert((route.upstream_id, model.clone())) {
            items.push(serde_json::json!({
                "upstream_id": route.upstream_id,
                "upstream_name": up.name,
                "model_id": model,
            }));
        }
    }
    Ok(Json(serde_json::json!({ "items": items })))
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ExportQuery {
    format: Option<String>,
}

/// GET /export：全量规则按 (upstream, model_id, currency) 聚合导出。
async fn export_rules(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    Query(q): Query<ExportQuery>,
) -> ApiResult<Response> {
    let rows: Vec<PriceRuleRow> = sqlx::query_as::<_, PriceRuleRow>(&format!(
        "SELECT {PRICE_RULE_COLS} FROM price_rules ORDER BY upstream_id, model_id, currency, unit, dimension_key"
    ))
    .fetch_all(&state.db)
    .await?;

    let snap = state.cache.snapshot();
    let items = build_export_items(&rows, &snap.upstreams);
    let format = q.format.unwrap_or_else(|| "xml".into());
    match format.as_str() {
        "json" => {
            let body = transfer::export_json(&items).to_string();
            Ok(xml_response(body, "application/json"))
        }
        _ => {
            let body = transfer::export_xml(&items);
            Ok(xml_response(body, "application/xml; charset=utf-8"))
        }
    }
}

/// 聚合中间态。
struct ItemAgg {
    model_id: String,
    upstream: String,
    currency: String,
    input_per_m: Option<Decimal>,
    output_per_m: Option<Decimal>,
    cache_read_per_m: Option<Decimal>,
    cache_write_per_m: Option<Decimal>,
    segments: serde_json::Map<String, serde_json::Value>,
    image: Vec<serde_json::Value>,
    video: Vec<serde_json::Value>,
}

fn build_export_items(
    rows: &[PriceRuleRow],
    upstreams: &HashMap<Uuid, crate::entities::UpstreamRow>,
) -> Vec<PriceExportItem> {
    let mut map: HashMap<(Uuid, String, String), ItemAgg> = HashMap::new();
    for row in rows {
        let name = upstreams.get(&row.upstream_id).map(|u| u.name.clone()).unwrap_or_default();
        let agg = map.entry((row.upstream_id, row.model_id.clone(), row.currency.clone())).or_insert_with(|| ItemAgg {
            model_id: row.model_id.clone(),
            upstream: name,
            currency: row.currency.clone(),
            input_per_m: None,
            output_per_m: None,
            cache_read_per_m: None,
            cache_write_per_m: None,
            segments: serde_json::Map::new(),
            image: Vec::new(),
            video: Vec::new(),
        });
        match row.unit.as_str() {
            "token_in" => agg.input_per_m = Some(row.base_price),
            "token_out" => agg.output_per_m = Some(row.base_price),
            "token_cache_read" => agg.cache_read_per_m = Some(row.base_price),
            "token_cache_write" => agg.cache_write_per_m = Some(row.base_price),
            "image" => agg.image.push(media_export_value(row)),
            "video_second" => agg.video.push(media_export_value(row)),
            _ => {}
        }
        if matches!(
            row.unit.as_str(),
            "token_in" | "token_out" | "token_cache_read" | "token_cache_write"
        ) {
            if let Some(seg) = &row.segments {
                agg.segments.insert(row.unit.clone(), seg.clone());
            }
        }
    }
    // 确定性输出：按 (upstream, model_id, currency) 排序（review P3：HashMap 顺序不稳定）
    let mut items: Vec<PriceExportItem> = map
        .into_values()
        .map(|agg| PriceExportItem {
            model_id: agg.model_id,
            upstream: agg.upstream,
            currency: agg.currency,
            input_per_m: agg.input_per_m,
            output_per_m: agg.output_per_m,
            cache_read_per_m: agg.cache_read_per_m,
            cache_write_per_m: agg.cache_write_per_m,
            segments: if agg.segments.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(agg.segments))
            },
            image: if agg.image.is_empty() { None } else { Some(serde_json::Value::Array(agg.image)) },
            video: if agg.video.is_empty() { None } else { Some(serde_json::Value::Array(agg.video)) },
        })
        .collect();
    items.sort_by(|a, b| {
        a.upstream
            .cmp(&b.upstream)
            .then_with(|| a.model_id.cmp(&b.model_id))
            .then_with(|| a.currency.cmp(&b.currency))
    });
    items
}

/// 图片/视频规则的导出值（base_price + dimension_key + dimensions + segments?）。
fn media_export_value(row: &PriceRuleRow) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("base_price".into(), serde_json::json!(row.base_price));
    m.insert("dimension_key".into(), serde_json::json!(row.dimension_key));
    if let Some(d) = &row.dimensions {
        m.insert("dimensions".into(), d.clone());
    }
    if let Some(s) = &row.segments {
        m.insert("segments".into(), s.clone());
    }
    serde_json::Value::Object(m)
}

fn xml_response(body: String, content_type: &str) -> Response {
    let mut resp = Response::new(Body::from(body));
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    resp
}

// ---------------------------------------------------------------------------
// import
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ImportQuery {
    #[serde(default)]
    dry_run: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ImportBody {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    dry_run: Option<bool>,
}

/// POST /import：multipart(form field file) 或 JSON {"url":...}；dry_run 来自 query/body。
async fn import_rules(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    Query(q): Query<ImportQuery>,
    req: Request,
) -> ApiResult<Json<serde_json::Value>> {
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let (bytes, dry_run) = if content_type.contains("multipart/form-data") {
        let mut mp = Multipart::from_request(req, &state)
            .await
            .map_err(|e| ApiError::bad_request(format!("multipart 解析失败: {e}")))?;
        let mut file_bytes: Option<Vec<u8>> = None;
        while let Some(field) = mp
            .next_field()
            .await
            .map_err(|e| ApiError::bad_request(format!("读取 multipart 字段失败: {e}")))?
        {
            if field.name().map(|n| n == "file").unwrap_or(false) {
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("读取文件失败: {e}")))?;
                file_bytes = Some(data.to_vec());
            }
        }
        let fb = file_bytes.ok_or_else(|| ApiError::bad_request("缺少 file 字段"))?;
        (fb, q.dry_run.unwrap_or(false))
    } else {
        let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
            .await
            .map_err(|e| ApiError::bad_request(format!("读取请求体失败: {e}")))?;
        let body: ImportBody = serde_json::from_slice(&body_bytes)
            .map_err(|e| ApiError::bad_request(format!("JSON 解析失败: {e}")))?;
        let url = body.url.ok_or_else(|| ApiError::bad_request("请输入 url"))?;
        let bytes = fetch_url(&state, &url).await?;
        (bytes, q.dry_run.unwrap_or(body.dry_run.unwrap_or(false)))
    };

    let fmt = transfer::detect_format(&bytes);
    let items = match fmt {
        "xml" => transfer::parse_xml(&bytes)?,
        _ => transfer::parse_json(&bytes)?,
    };

    let snap = state.cache.snapshot();
    let (rows, report) = analyze_import(&items, &snap.upstream_ids);

    if !dry_run && !rows.is_empty() {
        let mut tx = state.db.begin().await?;
        for r in &rows {
            sqlx::query(
                "INSERT INTO price_rules \
                 (upstream_id, model_id, unit, currency, base_price, source, dimensions, \
                  dimension_key, segments, context_basis, priority, sort_order, enabled) \
                 VALUES ($1,$2,$3,$4,$5,'import',$6,$7,$8,$9,10,0,TRUE) \
                 ON CONFLICT (upstream_id, model_id, unit, currency, dimension_key) DO UPDATE SET \
                   base_price=EXCLUDED.base_price, dimensions=EXCLUDED.dimensions, \
                   dimension_key=EXCLUDED.dimension_key, segments=EXCLUDED.segments, \
                   context_basis=EXCLUDED.context_basis, source='import', updated_at=now()",
            )
            .bind(r.upstream_id)
            .bind(&r.model_id)
            .bind(&r.unit)
            .bind(&r.currency)
            .bind(r.base_price)
            .bind(&r.dimensions)
            .bind(&r.dimension_key)
            .bind(&r.segments)
            .bind(&r.context_basis)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        auth::audit(
            &state,
            &admin.0,
            "pricing.import",
            "price_rules",
            None,
            serde_json::json!({
                "total": report.total, "succeeded": report.succeeded,
                "skipped": report.skipped, "failed": report.failed,
            }),
            None,
        )
        .await?;
    }

    Ok(Json(serde_json::json!({
        "total": report.total,
        "succeeded": report.succeeded,
        "skipped": report.skipped,
        "failed": report.failed,
        "dry_run": dry_run,
    })))
}

/// URL 渠道：allow_url 开关 + 仅 https + SSRF 防护 + 大小/超时限制 + 代理矩阵。
async fn fetch_url(state: &AppState, url: &str) -> ApiResult<Vec<u8>> {
    let hot = state.hot.load();
    let cfg = hot.price_import.clone();
    let timeout_secs = cfg.timeout_secs.clone();
    let max_size_mb = cfg.max_size_mb.clone();
    drop(hot);

    if !cfg.allow_url {
        return Err(ApiError::Forbidden);
    }
    check_ssrf(url)?;

    let client = build_import_client(state, url);
    let max_bytes = max_size_mb.saturating_mul(1_048_576u64).max(1);
    let timeout = Duration::from_secs(timeout_secs.max(1));

    let resp = client
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| ApiError::internal(format!("URL 拉取失败: {e}")))?;
    let resp = resp
        .error_for_status()
        .map_err(|e| ApiError::bad_request(format!("URL 返回异常状态: {e}")))?;
    if let Some(cl) = resp.content_length() {
        if cl > max_bytes {
            return Err(ApiError::bad_request(format!("文件超过大小限制 {}MB", max_size_mb)));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ApiError::internal(format!("读取响应失败: {e}")))?;
    if bytes.len() > max_bytes as usize {
        return Err(ApiError::bad_request(format!("文件超过大小限制 {}MB", max_size_mb)));
    }
    Ok(bytes.to_vec())
}

/// SSRF 防护：仅 https；host 为 IP 时禁内网段；域名为 localhost/内网后缀时拒绝。
fn check_ssrf(url: &str) -> ApiResult<()> {
    let parsed = reqwest::Url::parse(url).map_err(|e| ApiError::bad_request(format!("URL 非法: {e}")))?;
    if parsed.scheme() != "https" {
        return Err(ApiError::bad_request("仅允许 https URL"));
    }
    let host = parsed.host_str().unwrap_or_default();
    if host.is_empty() {
        return Err(ApiError::bad_request("URL 缺少主机名"));
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_internal_ip(ip) {
            return Err(ApiError::bad_request("禁止访问内网地址"));
        }
        return Ok(());
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
        || lower.ends_with(".localdomain")
    {
        return Err(ApiError::bad_request("禁止访问本机/内网域名"));
    }
    Ok(())
}

fn is_internal_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = u32::from(v4);
            (o & 0xFF00_0000) == 0x0A00_0000 // 10/8
                || (o & 0xFFF0_0000) == 0xAC10_0000 // 172.16/12
                || (o & 0xFFFF_0000) == 0xC0A8_0000 // 192.168/16
                || (o & 0xFF00_0000) == 0x7F00_0000 // 127/8
                || (o & 0xFFFF_0000) == 0xA9FE_0000 // 169.254/16
        }
        std::net::IpAddr::V6(v6) => {
            let seg = v6.segments();
            v6.is_loopback() // ::1
                || ((seg[0] & 0xFFC0) == 0xFE80) // fe80::/10 链路本地
                || ((seg[0] & 0xFE00) == 0xFC00) // fc00::/7 ULA 私网
        }
    }
}

/// 构建 URL 导入的 HTTP client（按 price_import.use_proxy/proxy_id + 全局 no_proxy 并集）。
fn build_import_client(state: &AppState, url: &str) -> reqwest::Client {
    let hot = state.hot.load();
    let cfg = &hot.price_import;
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

// ---------------------------------------------------------------------------
// import 校验（纯函数，契约 §7/§10：幂等键去重、非法行报告）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ImportRow {
    upstream_id: Uuid,
    model_id: String,
    unit: String,
    currency: String,
    base_price: Decimal,
    dimensions: Option<serde_json::Value>,
    dimension_key: String,
    segments: Option<serde_json::Value>,
    context_basis: String,
}

#[derive(Debug, Clone, Serialize)]
struct FailedRow {
    index: usize,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct ImportReport {
    total: usize,
    succeeded: usize,
    skipped: usize,
    failed: Vec<FailedRow>,
}

/// 单个文件条目展开为候选行（token 四列 + image/video 各自规则）。
fn expand_item(item: &PriceExportItem, up_id: Uuid) -> Vec<ImportRow> {
    let mut out = Vec::new();
    let seg_for = |unit: &str| item.segments.as_ref().and_then(|m| m.get(unit)).cloned();

    let mut push = |unit: &str, price: Decimal, dims: Option<serde_json::Value>, segs: Option<serde_json::Value>| {
        out.push(make_row(up_id, &item.model_id, unit, &item.currency, price, dims, segs));
    };
    if let Some(p) = item.input_per_m {
        push("token_in", p, None, seg_for("token_in"));
    }
    if let Some(p) = item.output_per_m {
        push("token_out", p, None, seg_for("token_out"));
    }
    if let Some(p) = item.cache_read_per_m {
        push("token_cache_read", p, None, seg_for("token_cache_read"));
    }
    if let Some(p) = item.cache_write_per_m {
        push("token_cache_write", p, None, seg_for("token_cache_write"));
    }
    let media_rules = |v: &Option<serde_json::Value>| -> Vec<serde_json::Value> {
        v.as_ref().and_then(|x| x.as_array()).cloned().unwrap_or_default()
    };
    for img in media_rules(&item.image) {
        let (price, dims, segs) = media_parts(&img);
        push("image", price, dims, segs);
    }
    for vid in media_rules(&item.video) {
        let (price, dims, segs) = media_parts(&vid);
        push("video_second", price, dims, segs);
    }
    out
}

fn media_parts(v: &serde_json::Value) -> (Decimal, Option<serde_json::Value>, Option<serde_json::Value>) {
    let price = v.get("base_price").and_then(json_to_decimal).unwrap_or(Decimal::ZERO);
    let dimensions = v.get("dimensions").cloned();
    let segments = v.get("segments").cloned();
    (price, dimensions, segments)
}

fn make_row(
    up_id: Uuid,
    model_id: &str,
    unit: &str,
    currency: &str,
    base_price: Decimal,
    dimensions: Option<serde_json::Value>,
    segments: Option<serde_json::Value>,
) -> ImportRow {
    let dimension_key = normalize_dimension_key(dimensions.as_ref());
    ImportRow {
        upstream_id: up_id,
        model_id: model_id.to_string(),
        unit: unit.to_string(),
        currency: currency.to_string(),
        base_price,
        dimensions,
        dimension_key,
        segments,
        context_basis: "prompt_tokens".into(),
    }
}

fn valid_unit(u: &str) -> bool {
    matches!(
        u,
        "token_in" | "token_out" | "token_cache_write" | "token_cache_read" | "image" | "video_second"
    )
}

fn valid_currency(c: &str) -> bool {
    matches!(c, "CNY" | "USD")
}

/// 单行校验；非法 → Err(reason)。
fn validate_row(row: ImportRow) -> Result<ImportRow, String> {
    if !valid_unit(&row.unit) {
        return Err(format!("非法计价单位: {}", row.unit));
    }
    if !valid_currency(&row.currency) {
        return Err(format!("非法币种: {}", row.currency));
    }
    if row.base_price < Decimal::ZERO {
        return Err("价格不能为负".into());
    }
    if let Some(d) = &row.dimensions {
        if !d.is_null() && !d.is_object() {
            return Err("维度必须为对象".into());
        }
    }
    if let Some(s) = &row.segments {
        if !s.is_array() {
            return Err("分段必须为数组".into());
        }
    }
    Ok(row)
}

/// 幂等键。
type IdemKey = (Uuid, String, String, String, String);

/// 逐行校验 + 文件内幂等键去重，返回待 upsert 行与报告。
fn analyze_import(items: &[PriceExportItem], upstream_ids: &HashMap<String, Uuid>) -> (Vec<ImportRow>, ImportReport) {
    let mut rows: Vec<ImportRow> = Vec::new();
    let mut seen: HashSet<IdemKey> = HashSet::new();
    let mut failed: Vec<FailedRow> = Vec::new();
    let mut succeeded = 0usize;
    let mut skipped = 0usize;
    let total = items.len();

    for (idx, item) in items.iter().enumerate() {
        let Some(up_id) = upstream_ids.get(&item.upstream).copied() else {
            failed.push(FailedRow { index: idx, reason: format!("上游不存在: {}", item.upstream) });
            continue;
        };
        let frags = expand_item(item, up_id);
        if frags.is_empty() {
            failed.push(FailedRow { index: idx, reason: "无可导入的价格项".into() });
            continue;
        }
        // 校验每个候选行；任一失败 → 整条失败。
        let mut valid: Vec<ImportRow> = Vec::new();
        let mut bad: Option<String> = None;
        for f in frags {
            match validate_row(f) {
                Ok(v) => valid.push(v),
                Err(reason) => {
                    bad = Some(reason);
                    break;
                }
            }
        }
        if let Some(reason) = bad {
            failed.push(FailedRow { index: idx, reason });
            continue;
        }
        let keys: Vec<IdemKey> = valid
            .iter()
            .map(|r| (r.upstream_id, r.model_id.clone(), r.unit.clone(), r.currency.clone(), r.dimension_key.clone()))
            .collect();
        if keys.iter().all(|k| seen.contains(k)) {
            skipped += 1;
            continue;
        }
        for k in &keys {
            seen.insert(k.clone());
        }
        for r in valid {
            rows.push(r);
        }
        succeeded += 1;
    }

    (rows, ImportReport { total, succeeded, skipped, failed })
}

// ---------------------------------------------------------------------------
// 通用校验
// ---------------------------------------------------------------------------

fn validate_create_fields(body: &CreateRuleReq) -> ApiResult<()> {
    validate_fields(
        &body.unit,
        &body.currency,
        body.context_basis.as_deref(),
        body.base_price,
        body.dimensions.as_ref(),
        body.segments.as_ref(),
    )
}

/// CRUD 字段校验：unit/currency/context_basis 白名单、base_price>=0、JSON 形态合法。
fn validate_fields(
    unit: &str,
    currency: &str,
    context_basis: Option<&str>,
    base_price: Decimal,
    dimensions: Option<&serde_json::Value>,
    segments: Option<&serde_json::Value>,
) -> ApiResult<()> {
    if !valid_unit(unit) {
        return Err(ApiError::bad_request("unit 必须为 token_in/token_out/token_cache_write/token_cache_read/image/video_second"));
    }
    if !valid_currency(currency) {
        return Err(ApiError::bad_request("currency 必须为 CNY/USD"));
    }
    if let Some(cb) = context_basis {
        if !matches!(cb, "prompt_tokens" | "total_tokens") {
            return Err(ApiError::bad_request("context_basis 必须为 prompt_tokens/total_tokens"));
        }
    }
    if base_price < Decimal::ZERO {
        return Err(ApiError::bad_request("base_price 不能为负"));
    }
    if let Some(d) = dimensions {
        if !d.is_null() && !d.is_object() {
            return Err(ApiError::bad_request("dimensions 必须为 JSON 对象"));
        }
    }
    if let Some(s) = segments {
        if !s.is_array() {
            return Err(ApiError::bad_request("segments 必须为 JSON 数组"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 单测（契约 §10）：导入校验纯函数（幂等键去重、非法行报告）。
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn item(upstream: &str, input: Option<&str>) -> PriceExportItem {
        PriceExportItem {
            model_id: "m".into(),
            upstream: upstream.into(),
            currency: "CNY".into(),
            input_per_m: input.map(dec),
            ..Default::default()
        }
    }

    #[test]
    fn dedup_idempotency_key_in_file() {
        // 同一 upstream×model×unit×currency×dimension_key 出现两次 → 后者跳过。
        let items = vec![item("upstream-a", Some("3.0")), item("upstream-a", Some("4.0"))];
        let mut ids = HashMap::new();
        ids.insert("upstream-a".to_string(), Uuid::new_v4());
        let (rows, report) = analyze_import(&items, &ids);
        assert_eq!(report.total, 2);
        assert_eq!(report.succeeded, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.failed.len(), 0);
        // 实际 upsert 仅一行（首个）。
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].base_price, dec("3.0"));
    }

    #[test]
    fn unknown_upstream_fails_row() {
        let items = vec![item("no-such", Some("3.0"))];
        let mut ids = HashMap::new();
        ids.insert("upstream-a".to_string(), Uuid::new_v4());
        let (rows, report) = analyze_import(&items, &ids);
        assert_eq!(rows.len(), 0);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].index, 0);
        assert!(report.failed[0].reason.contains("上游不存在"));
    }

    #[test]
    fn bad_currency_and_negative_price_reported() {
        let mut bad = item("upstream-a", Some("-1.0"));
        bad.currency = "EUR".into();
        let items = vec![bad];
        let mut ids = HashMap::new();
        ids.insert("upstream-a".to_string(), Uuid::new_v4());
        let (rows, report) = analyze_import(&items, &ids);
        assert_eq!(rows.len(), 0);
        assert_eq!(report.total, 1);
        assert_eq!(report.failed.len(), 1);
    }

    #[test]
    fn empty_item_fails() {
        let mut e = item("upstream-a", None);
        e.input_per_m = None;
        let mut ids = HashMap::new();
        ids.insert("upstream-a".to_string(), Uuid::new_v4());
        let (rows, report) = analyze_import(&[e], &ids);
        assert_eq!(rows.len(), 0);
        assert_eq!(report.failed.len(), 1);
    }

    #[test]
    fn media_image_video_expanded_with_dimension_key() {
        let mut it = item("upstream-a", Some("3.0"));
        it.image = Some(serde_json::json!([
            { "base_price": "2.0", "dimensions": { "image_size": "1024x1024" } }
        ]));
        it.video = Some(serde_json::json!([
            { "base_price": "0.5", "dimensions": { "resolution": "720p", "task_type": "txt2vid" } }
        ]));
        let mut ids = HashMap::new();
        ids.insert("upstream-a".to_string(), Uuid::new_v4());
        let (rows, report) = analyze_import(&[it], &ids);
        assert_eq!(report.failed.len(), 0);
        assert_eq!(report.succeeded, 1);
        assert_eq!(rows.len(), 3); // token_in + image + video
        let img = rows.iter().find(|r| r.unit == "image").unwrap();
        assert_eq!(img.base_price, dec("2.0"));
        assert_eq!(img.currency, "CNY");
        assert_eq!(img.dimension_key, "{\"image_size\":\"1024x1024\"}");
        let vid = rows.iter().find(|r| r.unit == "video_second").unwrap();
        assert_eq!(vid.base_price, dec("0.5"));
        assert_eq!(vid.dimension_key, "{\"resolution\":\"720p\",\"task_type\":\"txt2vid\"}");
    }
}
