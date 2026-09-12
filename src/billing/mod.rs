//! 计价引擎（PLAN §5.6：只统计不扣费）。契约 contracts/m5-billing.md §3，M5-A 实现。
//!
//! 匹配语义（契约 §3 / PLAN §5.6）：
//! - 每个「有量」的 unit 独立计价（token_in/out、cache 两维、image、video_second）；
//! - 同 unit 先精确 `dimension_key`，无则回落 ''；仍无 → 该 unit 跳过；
//! - 同 (unit,currency,dimension_key) 多条按 priority 大 → sort_order 小 → created_at,id；
//! - 生效期 effective_from/to（NULL 不限）；
//! - 分段按数组序第一命中（weekdays ∧ windows ∧ 上下文），全未命中 → base_price；
//! - 双币种：各币种直接值，仅汇总时经 fx 换算。

pub mod fx;
pub mod transfer;

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Timelike, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::entities::PriceRuleRow;
use crate::error::ApiResult;

/// 分段（segments JSONB 元素的解析视图）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub name: Option<String>,
    pub price: Decimal,
    /// ["mon",...]（小写三字母）
    pub weekdays: Option<Vec<String>>,
    /// [["09:00","12:00"],...] HH:MM；start>end = 跨午夜
    pub windows: Option<Vec<(String, String)>>,
    pub min_prompt_tokens: Option<i64>,
    pub max_prompt_tokens: Option<i64>,
}

/// 计价输入（一次请求/任务的用量）
#[derive(Debug, Clone, Default)]
pub struct PricingInput {
    pub at: DateTime<Utc>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub images: Option<i64>,
    pub image_size: Option<String>,
    pub video_seconds: Option<Decimal>,
    pub video_resolution: Option<String>,
    pub video_task_type: Option<String>,
}

/// 单行计价结果（一个 unit 一条）
#[derive(Debug, Clone, Serialize)]
pub struct PricedLine {
    pub unit: String,
    pub currency: String,
    /// token 类 = 原始 token 数（非 /1M）；image = 张数；video = CEIL 秒数
    pub quantity: Decimal,
    /// 命中单价（每 1M token / 每张 / 每秒）
    pub price: Decimal,
    /// 该币种成本
    pub cost: Decimal,
    pub rule_id: Uuid,
    pub matched_segment: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PricingResult {
    pub lines: Vec<PricedLine>,
    pub cost_cny: Option<Decimal>,
    pub cost_usd: Option<Decimal>,
    /// [{rule_id,unit,currency,base_price,matched_segment,price,effective_at}]
    pub price_used: serde_json::Value,
    /// [{from,to,rate,source,at,inverse}]（仅实际用到的换算）
    pub fx_snapshot: serde_json::Value,
    /// 至少一行命中规则
    pub priced: bool,
}

/// dimension_key 归一化：对象按键排序紧凑 JSON；空/{} /null → ""。
/// serde_json 默认 Map 走 BTreeMap，序列化时键已排序，故对 Object 直接 to_string()。
pub fn normalize_dimension_key(dimensions: Option<&serde_json::Value>) -> String {
    let Some(v) = dimensions else {
        return String::new();
    };
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(m) if m.is_empty() => String::new(),
        other => other.to_string(),
    }
}

/// 解析计费时区；非法回退 UTC。（生产路径用 unwrap_or/expect-free）
fn parse_tz(tz: &str) -> chrono_tz::Tz {
    tz.parse().unwrap_or(chrono_tz::Tz::UTC)
}

/// 按 unit 从输入导出用于维度匹配的 dimension_key。
fn input_dimension_key(unit: &str, input: &PricingInput) -> String {
    match unit {
        "image" => {
            if let Some(size) = &input.image_size {
                normalize_dimension_key(Some(&serde_json::json!({ "image_size": size })))
            } else {
                String::new()
            }
        }
        "video_second" => {
            let mut m = serde_json::Map::new();
            if let Some(r) = &input.video_resolution {
                m.insert("resolution".into(), serde_json::Value::String(r.clone()));
            }
            if let Some(t) = &input.video_task_type {
                m.insert("task_type".into(), serde_json::Value::String(t.clone()));
            }
            if m.is_empty() {
                String::new()
            } else {
                normalize_dimension_key(Some(&serde_json::Value::Object(m)))
            }
        }
        _ => String::new(),
    }
}

/// token 类用量（>0 才视为有量）。
fn token_measure(v: Option<i64>) -> Option<Decimal> {
    v.filter(|&n| n > 0).map(Decimal::from)
}

/// 某 unit 在输入上的「活跃量」。None = 该 unit 无用量，跳过计价。
fn unit_quantity(unit: &str, input: &PricingInput) -> Option<Decimal> {
    match unit {
        "token_in" => token_measure(input.prompt_tokens),
        "token_out" => token_measure(input.completion_tokens),
        "token_cache_write" => token_measure(input.cache_write_tokens),
        "token_cache_read" => token_measure(input.cache_read_tokens),
        "image" => input.images.filter(|&n| n > 0).map(Decimal::from),
        "video_second" => input.video_seconds.map(ceil_seconds),
        _ => None,
    }
}

/// token 类 cost 需除以 1_000_000；其余单位直接相乘。
fn unit_cost_divisor(unit: &str) -> Decimal {
    match unit {
        "token_in" | "token_out" | "token_cache_write" | "token_cache_read" => {
            Decimal::from(1_000_000)
        }
        _ => Decimal::ONE,
    }
}

/// 视频 CEIL 秒数，至少 1 秒（返回 Decimal，与 quantity 字段类型一致）。
fn ceil_seconds(s: Decimal) -> Decimal {
    let s = if s < Decimal::ZERO { Decimal::ZERO } else { s };
    let mut c = s.trunc();
    if s.fract() > Decimal::ZERO {
        c += Decimal::ONE;
    }
    if c < Decimal::ONE {
        c = Decimal::ONE;
    }
    c
}

/// 生效期：effective_from/to（NULL 不限），直接按 timestamptz 比较。
fn in_effect(rule: &PriceRuleRow, at: DateTime<Utc>) -> bool {
    if let Some(f) = rule.effective_from {
        if at < f {
            return false;
        }
    }
    if let Some(t) = rule.effective_to {
        if at > t {
            return false;
        }
    }
    true
}

/// 同 (unit,currency,dimension_key) 多条：priority 大者优先 → sort_order 小者优先 → created_at,id。
/// 返回 a 是否应替换 b。
fn better_rule(a: &PriceRuleRow, b: &PriceRuleRow) -> bool {
    if a.priority != b.priority {
        return a.priority > b.priority;
    }
    if a.sort_order != b.sort_order {
        return a.sort_order < b.sort_order;
    }
    if a.created_at != b.created_at {
        return a.created_at < b.created_at;
    }
    a.id < b.id
}

/// 解析 HH:MM 为当日分钟数。
fn parse_hhmm(s: &str) -> Option<i32> {
    let mut it = s.split(':');
    let h: i32 = it.next()?.trim().parse().ok()?;
    let min: i32 = it.next()?.trim().parse().ok()?;
    Some(h * 60 + min)
}

/// 命中判定：start>end 为跨午夜（t>=start 或 t<end）；否则闭区间。
fn window_hit(start: &str, end: &str, minutes: i32) -> bool {
    let (s, e) = match (parse_hhmm(start), parse_hhmm(end)) {
        (Some(s), Some(e)) => (s, e),
        _ => return false,
    };
    if s <= e {
        minutes >= s && minutes <= e
    } else {
        minutes >= s || minutes < e
    }
}

/// 上下文长度判定值（context_basis：prompt_tokens 默认 | total_tokens=prompt+completion）。
fn context_value(rule: &PriceRuleRow, input: &PricingInput) -> i64 {
    if rule.context_basis == "total_tokens" {
        input
            .prompt_tokens
            .unwrap_or(0)
            .saturating_add(input.completion_tokens.unwrap_or(0))
    } else {
        input.prompt_tokens.unwrap_or(0)
    }
}

/// 段命中：weekdays ∧ 任一 window ∧ 上下文（min≤v≤max 左闭右闭，未配置侧不限）。全未命中 → base_price。
fn segment_matches(seg: &Segment, ctx: i64, weekday: &str, minutes: i32) -> bool {
    if let Some(wds) = &seg.weekdays {
        if !wds.is_empty() && !wds.iter().any(|w| w.eq_ignore_ascii_case(weekday)) {
            return false;
        }
    }
    if let Some(ws) = &seg.windows {
        if !ws.is_empty() && !ws.iter().any(|(s, e)| window_hit(s, e, minutes)) {
            return false;
        }
    }
    if seg.min_prompt_tokens.is_some() || seg.max_prompt_tokens.is_some() {
        if let Some(min) = seg.min_prompt_tokens {
            if ctx < min {
                return false;
            }
        }
        if let Some(max) = seg.max_prompt_tokens {
            if ctx > max {
                return false;
            }
        }
    }
    true
}

/// 取规则命中的分段价格。返回 (价格, 段名)。segments 解析失败/为空 → None（回落 base_price）。
fn match_segment(
    rule: &PriceRuleRow,
    input: &PricingInput,
    weekday: &str,
    minutes: i32,
) -> Option<(Decimal, Option<String>)> {
    let seg_json = rule.segments.as_ref()?;
    let segs: Vec<Segment> = serde_json::from_value(seg_json.clone()).ok()?;
    if segs.is_empty() {
        return None;
    }
    let ctx = context_value(rule, input);
    for seg in &segs {
        if segment_matches(seg, ctx, weekday, minutes) {
            return Some((seg.price, seg.name.clone()));
        }
    }
    None
}

/// 单次计价（纯函数）：rules = 该 (upstream_id, model_id) 的全部 enabled 规则（调用方查好）。
pub fn price_with_rules(
    rules: &[PriceRuleRow],
    fx_set: &fx::FxSet,
    input: &PricingInput,
    billing_tz: &str,
) -> PricingResult {
    let tz = parse_tz(billing_tz);
    let local = input.at.with_timezone(&tz);
    let weekday = local.weekday().to_string().to_lowercase(); // "mon"
    let minutes = local.hour() as i32 * 60 + local.minute() as i32;

    let units = [
        "token_in",
        "token_out",
        "token_cache_write",
        "token_cache_read",
        "image",
        "video_second",
    ];

    let mut lines: Vec<PricedLine> = Vec::new();
    let mut price_used: Vec<serde_json::Value> = Vec::new();

    for unit in units {
        let Some(quantity) = unit_quantity(unit, input) else {
            continue;
        };
        let input_dim = input_dimension_key(unit, input);

        // 该 unit 的全部启用规则（调用方已过滤 enabled，这里再兜底一次），并先过滤生效期。
        let unit_rules: Vec<&PriceRuleRow> = rules
            .iter()
            .filter(|r| r.unit == unit && r.enabled)
            .collect();
        let active: Vec<&PriceRuleRow> = unit_rules
            .iter()
            .copied()
            .filter(|r| in_effect(r, input.at))
            .collect();
        // 层级：精确 dimension_key → 回落 ''。
        let mut cands: Vec<&PriceRuleRow> = active
            .iter()
            .copied()
            .filter(|r| r.dimension_key == input_dim)
            .collect();
        if cands.is_empty() && !input_dim.is_empty() {
            cands = active
                .iter()
                .copied()
                .filter(|r| r.dimension_key.is_empty())
                .collect();
        }
        if cands.is_empty() {
            continue;
        }

        // 按币种取最优规则。
        let mut best_by_cur: HashMap<&str, &PriceRuleRow> = HashMap::new();
        for r in cands.iter().copied() {
            let cur = r.currency.as_str();
            match best_by_cur.get(cur) {
                Some(best) => {
                    if better_rule(r, best) {
                        best_by_cur.insert(cur, r);
                    }
                }
                None => {
                    best_by_cur.insert(cur, r);
                }
            }
        }

        let div = unit_cost_divisor(unit);
        for (cur, rule) in best_by_cur {
            let (price, seg_name) =
                match_segment(rule, input, &weekday, minutes).unwrap_or((rule.base_price, None));
            let cost = price * quantity / div;
            lines.push(PricedLine {
                unit: unit.to_string(),
                currency: cur.to_string(),
                quantity,
                price,
                cost,
                rule_id: rule.id,
                matched_segment: seg_name.clone(),
            });
            price_used.push(serde_json::json!({
                "rule_id": rule.id,
                "unit": unit,
                "currency": cur,
                "base_price": rule.base_price,
                "matched_segment": seg_name,
                "price": price,
                "effective_at": input.at,
            }));
        }
    }

    let priced = !lines.is_empty();

    // ---- 双币种汇总：各币种直接值，另一币种经 fx 换算；不可用则该部分跳过。 ----
    let mut cny_direct = Decimal::ZERO;
    let mut usd_direct = Decimal::ZERO;
    let mut has_cny = false;
    let mut has_usd = false;
    for line in &lines {
        if line.currency == "CNY" {
            cny_direct += line.cost;
            has_cny = true;
        } else if line.currency == "USD" {
            usd_direct += line.cost;
            has_usd = true;
        }
    }
    let mut cost_cny_accum = cny_direct;
    let mut cost_usd_accum = usd_direct;
    let mut cny_obtained = has_cny;
    let mut usd_obtained = has_usd;
    let mut fx_used: Vec<serde_json::Value> = Vec::new();

    for line in &lines {
        if line.currency == "USD" {
            if let Some((v, fx_opt)) = fx_set.convert("USD", "CNY", line.cost) {
                cost_cny_accum += v;
                cny_obtained = true;
                if let Some(r) = fx_opt {
                    fx_used.push(serde_json::json!({
                        "from": "USD", "to": "CNY", "rate": r.rate, "source": r.source, "at": r.at, "inverse": r.inverse
                    }));
                }
            }
        } else if line.currency == "CNY" {
            if let Some((v, fx_opt)) = fx_set.convert("CNY", "USD", line.cost) {
                cost_usd_accum += v;
                usd_obtained = true;
                if let Some(r) = fx_opt {
                    fx_used.push(serde_json::json!({
                        "from": "CNY", "to": "USD", "rate": r.rate, "source": r.source, "at": r.at, "inverse": r.inverse
                    }));
                }
            }
        }
    }

    // 汇率缺失致单边成本不可得：对侧成本配额不累计（fail-open 面）。
    // 显式告警保证可观测（发布审阅数据批 #5）；配额 fail-closed 需汇率恢复后
    // 由重放/后续事件自然收敛，不在请求路径阻断。
    if (has_cny || has_usd) && (!cny_obtained || !usd_obtained) {
        tracing::warn!(
            "汇率缺失：双币种成本仅单边可得（cny={cny_obtained} usd={usd_obtained}），             对侧成本配额本事件不累计"
        );
    }

    PricingResult {
        lines,
        cost_cny: if cny_obtained {
            Some(cost_cny_accum)
        } else {
            None
        },
        cost_usd: if usd_obtained {
            Some(cost_usd_accum)
        } else {
            None
        },
        price_used: serde_json::Value::Array(price_used),
        fx_snapshot: serde_json::Value::Array(fx_used),
        priced,
    }
}

/// 计价用模型名（计价修复 2026-09-12）：优先「上游实际模型名」（路由 override_model
/// 改写后的名字，仅在与入口 model 不同时记录）；价格为「供应商 + 上游模型 ID」绑定，
/// 因此计价必须跟随改写后的名字，否则 `entry → override` 类路由（如 glm-5.3-qf →
/// glm-5.3 走千帆套餐）会被误判为未定价。
pub fn effective_price_model(ev: &crate::logging::LogEvent) -> &str {
    ev.upstream_model
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(ev.model.as_str())
}

/// 事件是否含计价用量（token 类 + 图片/视频维度）。M6：media 维度纳入判定。
fn has_usage(ev: &crate::logging::LogEvent) -> bool {
    ev.prompt_tokens.unwrap_or(0) > 0
        || ev.completion_tokens.unwrap_or(0) > 0
        || ev.cache_write_tokens.unwrap_or(0) > 0
        || ev.cache_read_tokens.unwrap_or(0) > 0
        || ev.images.unwrap_or(0) > 0
        || ev.video_seconds.is_some()
}

/// 从事件构造计价输入。M6：填入图片/视频维度（media_tasks 已完成计价的事件由 price_batch 跳过）。
fn input_from_event(ev: &crate::logging::LogEvent) -> PricingInput {
    PricingInput {
        at: ev.ts,
        prompt_tokens: ev.prompt_tokens,
        completion_tokens: ev.completion_tokens,
        cache_write_tokens: ev.cache_write_tokens,
        cache_read_tokens: ev.cache_read_tokens,
        images: ev.images,
        image_size: ev.image_size.clone(),
        video_seconds: ev.video_seconds,
        video_resolution: ev.video_resolution.clone(),
        video_task_type: ev.video_task_type.clone(),
    }
}

/// 批量计价（writer 用）：收集 events 中 status<400 且有任一用量且含 (upstream_id,model) 的去重键，
/// 一次 SELECT 查规则 + load_fx_set 一次，逐事件 price_with_rules 并回填
/// cost_cny/cost_usd/pricing_source/price_used/fx_snapshot；返回未定价事件数。
pub async fn price_batch(
    pool: &PgPool,
    events: &mut [crate::logging::LogEvent],
    billing_tz: &str,
    fx_stale_max_minutes: u64,
) -> ApiResult<u64> {
    if events.is_empty() {
        return Ok(0);
    }

    // 去重收集（upstream_id, model_id）。
    let mut pairs: Vec<(Uuid, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for ev in events.iter() {
        // M6：media_tasks 由 poller 直接计价并落 usage_logs（pricing_source 已填），跳过防重复。
        if ev.status >= 400 || ev.pricing_source.is_some() || !has_usage(ev) {
            continue;
        }
        let Some(up_id) = ev.upstream_id else {
            continue;
        };
        let key = (up_id, effective_price_model(ev).to_string());
        if seen.insert(key.clone()) {
            pairs.push(key);
        }
    }
    if pairs.is_empty() {
        return Ok(0);
    }

    // 汇率一次读取（手动优先 + stale 过滤 + 逆汇率）。
    let fx_set = fx::load_fx_set(pool, fx_stale_max_minutes).await?;

    let upstream_ids: Vec<Uuid> = pairs.iter().map(|(u, _)| *u).collect();
    let model_ids: Vec<String> = pairs.iter().map(|(_, m)| m.clone()).collect();
    let rows = sqlx::query_as::<_, PriceRuleRow>(
        "SELECT id, upstream_id, model_id, unit, currency, base_price, source, dimensions, dimension_key, \
                segments, context_basis, effective_from, effective_to, priority, sort_order, enabled, \
                created_at, updated_at \
         FROM price_rules \
         WHERE enabled AND (upstream_id, model_id) IN (SELECT * FROM UNNEST($1::uuid[], $2::text[]))",
    )
    .bind(&upstream_ids)
    .bind(&model_ids)
    .fetch_all(pool)
    .await?;

    let mut rules_by: HashMap<(Uuid, String), Vec<PriceRuleRow>> = HashMap::new();
    for r in rows {
        rules_by
            .entry((r.upstream_id, r.model_id.clone()))
            .or_default()
            .push(r);
    }

    let mut unpriced = 0u64;
    for ev in events.iter_mut() {
        // M6：media_tasks 已计价事件（pricing_source 已填）跳过，防重复计价。
        if ev.status >= 400 || ev.pricing_source.is_some() || !has_usage(ev) {
            continue;
        }
        let Some(up_id) = ev.upstream_id else {
            continue;
        };
        let rules = rules_by.get(&(up_id, effective_price_model(ev).to_string()));
        let input = input_from_event(ev);
        let result = price_with_rules(
            rules.map(|v| v.as_slice()).unwrap_or(&[]),
            &fx_set,
            &input,
            billing_tz,
        );
        if !result.priced {
            unpriced += 1;
        }
        // 回填计价字段（契约 §0）：priced → pricing_source='bound'；未定价保持全 NULL。
        ev.cost_cny = result.cost_cny;
        ev.cost_usd = result.cost_usd;
        ev.pricing_source = if result.priced {
            Some("bound".into())
        } else {
            None
        };
        ev.price_used = if result.priced {
            Some(result.price_used)
        } else {
            None
        };
        ev.fx_snapshot = if result.priced {
            Some(result.fx_snapshot)
        } else {
            None
        };
    }
    Ok(unpriced)
}

/// 单事件计价（preview/测试用）：查规则 + fx 后走 price_with_rules。
pub async fn price_one(
    pool: &PgPool,
    upstream_id: Uuid,
    model_id: &str,
    input: &PricingInput,
    billing_tz: &str,
    fx_stale_max_minutes: u64,
) -> ApiResult<PricingResult> {
    let fx_set = fx::load_fx_set(pool, fx_stale_max_minutes).await?;
    let rules = sqlx::query_as::<_, PriceRuleRow>(
        "SELECT id, upstream_id, model_id, unit, currency, base_price, source, dimensions, dimension_key, \
                segments, context_basis, effective_from, effective_to, priority, sort_order, enabled, \
                created_at, updated_at \
         FROM price_rules \
         WHERE enabled AND upstream_id = $1 AND model_id = $2",
    )
    .bind(upstream_id)
    .bind(model_id)
    .fetch_all(pool)
    .await?;
    Ok(price_with_rules(&rules, &fx_set, input, billing_tz))
}

// ---------------------------------------------------------------------------
// 单测（契约 §3 末段 / §10）。
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::FxRateRow;
    use chrono::TimeZone;
    use std::str::FromStr;

    /// 构造最小可用事件（serde 反序列化路径，与 WAL 兼容测试同口径）。
    fn mk_log_event(model: &str, upstream_model: Option<&str>) -> crate::logging::LogEvent {
        serde_json::from_value(serde_json::json!({
            "request_id": "r1",
            "ts": "2024-01-01T00:00:00Z",
            "key_id": null,
            "model": model,
            "upstream_model": upstream_model,
            "upstream_id": null,
            "protocol_in": "openai_chat",
            "protocol_out": "openai_chat",
            "convert_mode": "passthrough",
            "stream": false,
            "status": 200,
            "error": null,
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "cache_write_tokens": null,
            "cache_read_tokens": null,
            "latency_ms": null,
            "retry_count": 0,
            "ttfb_ms": null,
            "degraded": false,
            "usage_raw": null,
            "debug_payload": null
        }))
        .expect("构造 LogEvent 失败")
    }

    /// 计价模型名：override 生效时用上游实际模型名，否则回落入口名（计价修复 2026-09-12）。
    #[test]
    fn price_model_follows_upstream_override() {
        assert_eq!(
            effective_price_model(&mk_log_event("glm-5.3-qf", Some("glm-5.3"))),
            "glm-5.3"
        );
        assert_eq!(
            effective_price_model(&mk_log_event("glm-5.3-qf", None)),
            "glm-5.3-qf"
        );
        // 空串防御：按未改写处理
        assert_eq!(
            effective_price_model(&mk_log_event("glm-5.3-qf", Some(""))),
            "glm-5.3-qf"
        );
    }

    /// 构造一条默认价格规则（id 随机、dimension_key=''、无分段、生效期无限）。
    fn base_rule(unit: &str, currency: &str, base: &str) -> PriceRuleRow {
        PriceRuleRow {
            id: Uuid::new_v4(),
            upstream_id: Uuid::new_v4(),
            model_id: "deepseek-chat".into(),
            unit: unit.into(),
            currency: currency.into(),
            base_price: Decimal::from_str(base).unwrap(),
            source: "manual".into(),
            dimensions: None,
            dimension_key: String::new(),
            segments: None,
            context_basis: "prompt_tokens".into(),
            effective_from: None,
            effective_to: None,
            priority: 10,
            sort_order: 0,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// 用一组 (from,to,rate) 构造汇率集合（全 manual）。
    fn fx_set(pairs: &[(&str, &str, &str)]) -> fx::FxSet {
        let rows: Vec<FxRateRow> = pairs
            .iter()
            .map(|(f, t, r)| FxRateRow {
                currency_from: f.to_string(),
                currency_to: t.to_string(),
                rate: Decimal::from_str(r).unwrap(),
                source: "manual".into(),
                fetched_at: Some(Utc::now()),
                updated_at: Utc::now(),
            })
            .collect();
        fx::build_fx_set(&rows, Utc::now(), 60)
    }

    /// 以给定时区构造本地时刻（返回 UTC）。
    fn local_dt(tz: &str, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        let tz: chrono_tz::Tz = tz.parse().unwrap();
        tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn segment_windows_parse() {
        let v: Vec<Segment> = serde_json::from_value(serde_json::json!([
            {"name":"a","price":1.0,"windows":[["09:00","12:00"],["23:00","02:00"]],"weekdays":["mon","tue"]}
        ]))
        .unwrap();
        assert_eq!(v[0].windows.as_ref().unwrap().len(), 2);
        assert_eq!(v[0].weekdays.as_ref().unwrap()[0], "mon");
        assert_eq!(v[0].price, Decimal::from_str("1.0").unwrap());
    }

    #[test]
    fn deepseek_segments_peak_long_default_base() {
        // 示例采用「特定优先在前」的顺序（数组序第一命中），以保证复合段命中 4.0。
        let seg = serde_json::json!([
            {"name":"peak_long","price":4.0,"weekdays":["mon","tue","wed","thu","fri"],
             "windows":[["09:00","12:00"],["14:00","18:00"]],"min_prompt_tokens":32769},
            {"name":"peak","price":3.0,"weekdays":["mon","tue","wed","thu","fri"],
             "windows":[["09:00","12:00"],["14:00","18:00"]]},
            {"name":"long_context","price":3.5,"min_prompt_tokens":32769},
            {"name":"default","price":1.5}
        ]);
        let mut r = base_rule("token_in", "CNY", "3.0");
        r.segments = Some(seg);
        let rules = vec![r];
        let fxs = fx_set(&[]);

        // 高峰 + 短上下文 → peak 3.0
        let at = local_dt("Asia/Shanghai", 2026, 1, 5, 10, 0);
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                prompt_tokens: Some(1000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("3.0").unwrap());
        assert_eq!(res.lines[0].matched_segment.as_deref(), Some("peak"));

        // 非高峰 + 长上下文 → long_context 3.5
        let at = local_dt("Asia/Shanghai", 2026, 1, 6, 20, 0);
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                prompt_tokens: Some(40000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("3.5").unwrap());
        assert_eq!(
            res.lines[0].matched_segment.as_deref(),
            Some("long_context")
        );

        // 高峰 + 长上下文（复合）→ peak_long 4.0
        let at = local_dt("Asia/Shanghai", 2026, 1, 7, 10, 0);
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                prompt_tokens: Some(40000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("4.0").unwrap());
        assert_eq!(res.lines[0].matched_segment.as_deref(), Some("peak_long"));

        // 非高峰 + 短上下文 → default 1.5
        let at = local_dt("Asia/Shanghai", 2026, 1, 8, 20, 0);
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                prompt_tokens: Some(1000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("1.5").unwrap());
        assert_eq!(res.lines[0].matched_segment.as_deref(), Some("default"));

        // base 回落：无分段 → base_price
        let mut r2 = base_rule("token_in", "CNY", "0.75");
        r2.segments = None;
        let res = price_with_rules(
            &[r2],
            &fxs,
            &PricingInput {
                at: local_dt("Asia/Shanghai", 2026, 1, 5, 10, 0),
                prompt_tokens: Some(1000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("0.75").unwrap());
        assert_eq!(res.lines[0].matched_segment, None);
    }

    #[test]
    fn cross_midnight_window() {
        let seg = serde_json::json!([{"name":"night","price":2.0,"windows":[["23:00","02:00"]]}]);
        let mut r = base_rule("token_in", "CNY", "1.0");
        r.segments = Some(seg);
        let rules = vec![r];
        let fxs = fx_set(&[]);

        // 23:30 命中
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at: local_dt("Asia/Shanghai", 2026, 1, 5, 23, 30),
                prompt_tokens: Some(1000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("2.0").unwrap());

        // 次日 01:30 命中（t<end）
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at: local_dt("Asia/Shanghai", 2026, 1, 6, 1, 30),
                prompt_tokens: Some(1000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("2.0").unwrap());

        // 12:00 未命中 → base 1.0
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at: local_dt("Asia/Shanghai", 2026, 1, 5, 12, 0),
                prompt_tokens: Some(1000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("1.0").unwrap());
        assert_eq!(res.lines[0].matched_segment, None);
    }

    #[test]
    fn context_boundaries_inclusive() {
        let seg = serde_json::json!([{"name":"ctx","price":5.0,"min_prompt_tokens":100,"max_prompt_tokens":200}]);
        let mut r = base_rule("token_in", "CNY", "1.0");
        r.segments = Some(seg);
        let rules = vec![r];
        let fxs = fx_set(&[]);
        let at = local_dt("Asia/Shanghai", 2026, 1, 5, 12, 0);

        // 下界含、上界含
        for v in [100i64, 200] {
            let res = price_with_rules(
                &rules,
                &fxs,
                &PricingInput {
                    at,
                    prompt_tokens: Some(v),
                    ..Default::default()
                },
                "Asia/Shanghai",
            );
            assert_eq!(res.lines[0].price, Decimal::from_str("5.0").unwrap());
        }
        // 左开、右开 → base
        for v in [99i64, 201] {
            let res = price_with_rules(
                &rules,
                &fxs,
                &PricingInput {
                    at,
                    prompt_tokens: Some(v),
                    ..Default::default()
                },
                "Asia/Shanghai",
            );
            assert_eq!(res.lines[0].price, Decimal::from_str("1.0").unwrap());
        }
    }

    #[test]
    fn context_basis_total_tokens() {
        let seg = serde_json::json!([{"name":"big","price":5.0,"min_prompt_tokens":50}]);
        let mut r = base_rule("token_in", "CNY", "1.0");
        r.context_basis = "total_tokens".into();
        r.segments = Some(seg);
        let rules = vec![r];
        let fxs = fx_set(&[]);
        let at = local_dt("Asia/Shanghai", 2026, 1, 5, 12, 0);

        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                prompt_tokens: Some(30),
                completion_tokens: Some(30),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("5.0").unwrap());

        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                prompt_tokens: Some(30),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("1.0").unwrap());
    }

    #[test]
    fn normalize_dimension_key_sorted() {
        let a = normalize_dimension_key(Some(
            &serde_json::json!({"task_type":"txt2vid","resolution":"720p"}),
        ));
        let b = normalize_dimension_key(Some(
            &serde_json::json!({"resolution":"720p","task_type":"txt2vid"}),
        ));
        assert_eq!(a, b);
        assert_eq!(a, "{\"resolution\":\"720p\",\"task_type\":\"txt2vid\"}");
        assert_eq!(normalize_dimension_key(None), "");
        assert_eq!(normalize_dimension_key(Some(&serde_json::json!({}))), "");
        assert_eq!(normalize_dimension_key(Some(&serde_json::Value::Null)), "");
    }

    #[test]
    fn image_dimension_exact_then_fallback() {
        let mut exact = base_rule("image", "CNY", "2.0");
        exact.dimension_key = "{\"image_size\":\"1024x1024\"}".into();
        let mut fallback = base_rule("image", "CNY", "3.0");
        fallback.dimension_key = String::new();
        let rules = vec![exact, fallback];
        let fxs = fx_set(&[]);
        let at = local_dt("Asia/Shanghai", 2026, 1, 5, 12, 0);

        // 精确命中：2 张 × 2.0 = 4.0
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                images: Some(2),
                image_size: Some("1024x1024".into()),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("2.0").unwrap());
        assert_eq!(res.lines[0].cost, Decimal::from_str("4.0").unwrap());

        // 无精确尺寸 → 回落 ''：2 张 × 3.0 = 6.0
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                images: Some(2),
                image_size: Some("512x512".into()),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("3.0").unwrap());
        assert_eq!(res.lines[0].cost, Decimal::from_str("6.0").unwrap());
    }

    #[test]
    fn same_dimension_priority_wins() {
        let mut low = base_rule("token_in", "CNY", "1.0");
        low.priority = 10;
        let mut high = base_rule("token_in", "CNY", "2.0");
        high.priority = 20;
        let rules = vec![low, high];
        let fxs = fx_set(&[]);
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at: local_dt("Asia/Shanghai", 2026, 1, 5, 12, 0),
                prompt_tokens: Some(1000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].price, Decimal::from_str("2.0").unwrap());
    }

    #[test]
    fn video_ceil_seconds_min_1() {
        let mut r = base_rule("video_second", "CNY", "0.5");
        r.segments = None;
        let rules = vec![r];
        let fxs = fx_set(&[]);
        let at = local_dt("Asia/Shanghai", 2026, 1, 5, 12, 0);

        // 2.5 秒 → CEIL=3 → 0.5*3=1.5
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                video_seconds: Some(Decimal::from_str("2.5").unwrap()),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].quantity, Decimal::from(3));
        assert_eq!(res.lines[0].cost, Decimal::from_str("1.5").unwrap());

        // 0.2 秒 → CEIL=1（至少 1 秒）→ 0.5*1=0.5
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                video_seconds: Some(Decimal::from_str("0.2").unwrap()),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines[0].quantity, Decimal::from(1));
        assert_eq!(res.lines[0].cost, Decimal::from_str("0.5").unwrap());
    }

    #[test]
    fn dual_currency_direct_not_crossed() {
        let cny = base_rule("token_in", "CNY", "3.0");
        let usd = base_rule("token_in", "USD", "1.0");
        let rules = vec![cny, usd];
        let fxs = fx_set(&[("USD", "CNY", "7.2")]);
        let at = local_dt("Asia/Shanghai", 2026, 1, 5, 12, 0);

        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                prompt_tokens: Some(1000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert_eq!(res.lines.len(), 2);
        let cny_line = res.lines.iter().find(|l| l.currency == "CNY").unwrap();
        let usd_line = res.lines.iter().find(|l| l.currency == "USD").unwrap();
        // 各行成本为直接值（不互相折算）
        assert_eq!(cny_line.cost, Decimal::from_str("0.003").unwrap());
        assert_eq!(usd_line.cost, Decimal::from_str("0.001").unwrap());
        assert_eq!(cny_line.price, Decimal::from_str("3.0").unwrap());

        // cost_cny = 0.003 + 0.001*7.2 = 0.0102
        assert_eq!(res.cost_cny.unwrap(), Decimal::from_str("0.0102").unwrap());
        // cost_usd = 0.001 + 0.003/7.2
        let usd_expected = Decimal::from_str("0.001").unwrap()
            + Decimal::from_str("0.003").unwrap() / Decimal::from_str("7.2").unwrap();
        assert!(
            (res.cost_usd.unwrap() - usd_expected).abs()
                < Decimal::from_str("0.0000000001").unwrap()
        );
        // 实际用到两次换算（USD→CNY、CNY→USD）
        assert_eq!(res.fx_snapshot.as_array().unwrap().len(), 2);
    }

    #[test]
    /// 新语义（默认汇率 6.73 兜底）：无 manual/auto 汇率时 CNY↔USD 走内置兜底，
    /// fx_snapshot 记 source=builtin 保证可审计。
    fn fx_unavailable_uses_builtin_default() {
        let cny = base_rule("token_in", "CNY", "3.0");
        let rules = vec![cny];
        let fxs = fx_set(&[]);
        let at = local_dt("Asia/Shanghai", 2026, 1, 5, 12, 0);

        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                prompt_tokens: Some(1000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert!(res.cost_cny.is_some());
        // USD 由内置 6.73 兜底换算
        assert!(res.cost_usd.is_some());
        let snap = res.fx_snapshot.as_array().unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0]["source"], "builtin");
        assert!(res.priced);
    }

    #[test]
    /// P0 计价修复回归：缓存命中不重复计价。记账层已把 prompt_tokens 归一为
    /// 「未缓存输入」（如 OpenAI input=8/cached=3 → prompt=5、cache_read=3），
    /// 故 token_in 用量必须为 5（而非全量 8），token_cache_read 单独计 3。
    fn cached_tokens_not_double_counted() {
        let rules = vec![
            base_rule("token_in", "CNY", "1.0"),
            base_rule("token_out", "CNY", "2.0"),
            base_rule("token_cache_read", "CNY", "0.1"),
        ];
        let fxs = fx_set(&[]);
        let at = local_dt("Asia/Shanghai", 2026, 1, 5, 12, 0);
        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                // 归一后：未缓存输入 5、输出 4、缓存读 3
                prompt_tokens: Some(5),
                completion_tokens: Some(4),
                cache_read_tokens: Some(3),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        let qty = |unit: &str| {
            res.lines
                .iter()
                .find(|l| l.unit == unit)
                .map(|l| l.quantity)
        };
        // token_in 按未缓存输入 5 计（修复前若用全量 8 则缓存 3 被重复计费）
        assert_eq!(qty("token_in"), Some(Decimal::from(5)));
        assert_eq!(qty("token_out"), Some(Decimal::from(4)));
        // token_cache_read 单独计 3
        assert_eq!(qty("token_cache_read"), Some(Decimal::from(3)));
        // CNY 成本 = (5×1.0 + 4×2.0 + 3×0.1) / 1e6
        let expect = Decimal::from_str("0.0000133").unwrap();
        assert_eq!(res.cost_cny, Some(expect));
    }

    #[test]
    fn only_usd_no_fx_uses_builtin_default() {
        let usd = base_rule("token_in", "USD", "1.0");
        let rules = vec![usd];
        let fxs = fx_set(&[]);
        let at = local_dt("Asia/Shanghai", 2026, 1, 5, 12, 0);

        let res = price_with_rules(
            &rules,
            &fxs,
            &PricingInput {
                at,
                prompt_tokens: Some(1000),
                ..Default::default()
            },
            "Asia/Shanghai",
        );
        assert!(res.cost_usd.is_some());
        // CNY 由内置 6.73 兜底换算
        assert!(res.cost_cny.is_some());
        let snap = res.fx_snapshot.as_array().unwrap();
        assert_eq!(snap[0]["source"], "builtin");
    }

    #[test]
    fn effective_window_boundaries() {
        let t0 = local_dt("Asia/Shanghai", 2026, 1, 1, 0, 0);
        let t1 = local_dt("Asia/Shanghai", 2026, 1, 31, 23, 59);
        let mut r = base_rule("token_in", "CNY", "2.0");
        r.effective_from = Some(t0);
        r.effective_to = Some(t1);
        let rules = vec![r];
        let fxs = fx_set(&[]);
        let input = |at| PricingInput {
            at,
            prompt_tokens: Some(1000),
            ..Default::default()
        };

        assert!(price_with_rules(&rules, &fxs, &input(t0), "Asia/Shanghai").priced);
        assert!(price_with_rules(&rules, &fxs, &input(t1), "Asia/Shanghai").priced);
        assert!(
            !price_with_rules(
                &rules,
                &fxs,
                &input(t0 - chrono::Duration::seconds(1)),
                "Asia/Shanghai"
            )
            .priced
        );
        assert!(
            !price_with_rules(
                &rules,
                &fxs,
                &input(t1 + chrono::Duration::seconds(1)),
                "Asia/Shanghai"
            )
            .priced
        );
    }
}
