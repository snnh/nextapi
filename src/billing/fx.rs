//! 汇率引擎（PLAN §5.6 双币种：manual 优先 / auto TTL+stale / 逆汇率 / fx_snapshot）。
//! 契约 contracts/m5-billing.md §4，M5-A 实现。
//!
//! 要点：
//! - `load_fx_set` 读出 fx_rates 全量并应用「manual 无条件优先 + auto stale 剔除 + 逆汇率展开」。
//! - `convert(from,to,amt)`：from==to 直接直通；否则查正向/逆向汇率并相乘。
//! - `fetch_and_store`：联网拉取（frankfurter/custom JSON、ecb XML）成功后仅 upsert source='auto' 行，
//!   失败返回 Err 由调用方保留旧值（降级链）。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

use crate::entities::FxRateRow;
use crate::error::{ApiError, ApiResult};

/// 一条可用汇率（已应用 manual 优先 + stale 过滤 + 逆汇率）
#[derive(Debug, Clone)]
pub struct FxRate {
    pub rate: Decimal,
    pub source: String,
    pub at: Option<DateTime<Utc>>,
    pub inverse: bool,
}

/// 计费时汇率集合（含正向与逆向条目）
pub struct FxSet {
    // (from,to) -> 可用汇率
    map: HashMap<(String, String), FxRate>,
}

/// 由原始行构建汇率集合（纯函数，供 load_fx_set 与单测复用）。
/// - manual 无条件优先于 auto；
/// - auto 行 fetched_at 超过 stale_max_minutes 剔除；fetched_at 为空视为过期；
/// - 每个 (from,to) 先收敛为唯一可用直接项，再对缺失的反向键做逆汇率展开。
pub(crate) fn build_fx_set(
    rows: &[FxRateRow],
    now: DateTime<Utc>,
    stale_max_minutes: u64,
) -> FxSet {
    let mut direct: HashMap<(String, String), FxRate> = HashMap::new();
    for row in rows {
        let allowed = match row.source.as_str() {
            "manual" => true,
            "auto" => match row.fetched_at {
                Some(ts) => {
                    // 年龄（分钟）不超过阈值视为新鲜；未来时间差为负 → 视为新鲜。
                    now.signed_duration_since(ts).num_minutes() <= stale_max_minutes as i64
                }
                None => false,
            },
            _ => false,
        };
        if !allowed {
            continue;
        }
        let pair = (row.currency_from.clone(), row.currency_to.clone());
        let rate = FxRate {
            rate: row.rate,
            source: row.source.clone(),
            at: row.fetched_at,
            inverse: false,
        };
        match direct.get(&pair) {
            Some(existing) => {
                // manual 覆盖 auto；同 source（PK 已防重复）保留首个。
                if row.source == "manual" && existing.source != "manual" {
                    direct.insert(pair, rate);
                }
            }
            None => {
                direct.insert(pair, rate);
            }
        }
    }

    let mut map: HashMap<(String, String), FxRate> = HashMap::new();
    for (k, v) in &direct {
        map.insert(k.clone(), v.clone());
    }
    // 逆汇率展开：仅当反向键缺失（无显式直接项）时才用 1/r 补齐。
    for ((from, to), v) in &direct {
        let inv_key = (to.clone(), from.clone());
        if map.contains_key(&inv_key) {
            continue;
        }
        if v.rate.is_zero() {
            continue;
        }
        let inv_rate = Decimal::ONE / v.rate;
        map.insert(
            inv_key,
            FxRate {
                rate: inv_rate,
                source: v.source.clone(),
                at: v.at,
                inverse: true,
            },
        );
    }
    FxSet { map }
}

impl FxSet {
    /// from==to → Some((amount, None))；无可用汇率 → None。
    pub fn convert(
        &self,
        from: &str,
        to: &str,
        amount: Decimal,
    ) -> Option<(Decimal, Option<FxRate>)> {
        if from == to {
            return Some((amount, None));
        }
        let key = (from.to_string(), to.to_string());
        let rate = self.map.get(&key)?;
        Some((amount * rate.rate, Some(rate.clone())))
    }
}

/// 读出 fx_rates 全量并应用 manual 优先 + stale 过滤 + 逆汇率展开。
pub async fn load_fx_set(pool: &PgPool, stale_max_minutes: u64) -> ApiResult<FxSet> {
    let rows = sqlx::query_as::<_, FxRateRow>(
        "SELECT currency_from, currency_to, rate, source, fetched_at, updated_at FROM fx_rates",
    )
    .fetch_all(pool)
    .await?;
    Ok(build_fx_set(&rows, Utc::now(), stale_max_minutes))
}

/// 将 JSON 数值安全转成 Decimal（用原始字符串避免浮点近似）。
fn json_num_to_decimal(v: &serde_json::Value) -> Option<Decimal> {
    match v {
        serde_json::Value::Number(n) => {
            if n.is_i64() {
                return n.as_i64().map(Decimal::from);
            }
            if n.is_u64() {
                return n.as_u64().map(Decimal::from);
            }
            if n.is_f64() {
                return Decimal::from_str_exact(&n.to_string()).ok();
            }
            None
        }
        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
        _ => None,
    }
}

/// frankfurter/custom 同形状 JSON：`{"rates":{"CNY":7.2,...}}`。base 为计价基准币。
/// 返回 (pairs 描述, (from,to,rate) 列表)。仅保留正值汇率。
fn parse_rates_json(
    v: &serde_json::Value,
    base: &str,
) -> ApiResult<(Vec<String>, Vec<(String, String, Decimal)>)> {
    let rates = v
        .get("rates")
        .ok_or_else(|| ApiError::internal("汇率响应缺少 rates 字段"))?;
    let obj = rates
        .as_object()
        .ok_or_else(|| ApiError::internal("汇率响应 rates 不是对象"))?;
    let mut pairs = Vec::new();
    let mut rates_out: Vec<(String, String, Decimal)> = Vec::new();
    for (sym, val) in obj {
        let rate = json_num_to_decimal(val)
            .ok_or_else(|| ApiError::internal(format!("无效汇率值 {}", sym)))?;
        if rate <= Decimal::ZERO {
            continue;
        }
        rates_out.push((base.to_string(), sym.clone(), rate));
        pairs.push(format!("{}->{}", base, sym));
    }
    Ok((pairs, rates_out))
}

/// 解析 ECB 每日汇率 XML（base=EUR）。过滤到 symbols（非空时）；返回 (pairs, (EUR,cur,rate)) 列表。
fn parse_ecb(
    xml: &[u8],
    symbols: &[String],
) -> ApiResult<(Vec<String>, Vec<(String, String, Decimal)>)> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_reader(xml);
    let mut rates_out: Vec<(String, String, Decimal)> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"Cube" {
                    let mut currency: Option<String> = None;
                    let mut rate: Option<Decimal> = None;
                    for attr in e.attributes() {
                        let attr = attr.map_err(|err| {
                            ApiError::internal(format!("ECB XML 属性解析失败: {err}"))
                        })?;
                        match attr.key.as_ref() {
                            b"currency" => {
                                currency = Some(
                                    attr.unescape_value()
                                        .map_err(|err| {
                                            ApiError::internal(format!(
                                                "ECB XML 币种解码失败: {err}"
                                            ))
                                        })?
                                        .to_string(),
                                );
                            }
                            b"rate" => {
                                rate = Some(
                                    attr.unescape_value()
                                        .map_err(|err| {
                                            ApiError::internal(format!(
                                                "ECB XML 汇率解码失败: {err}"
                                            ))
                                        })?
                                        .parse::<Decimal>()
                                        .map_err(|err| {
                                            ApiError::internal(format!(
                                                "ECB XML 汇率解析失败: {err}"
                                            ))
                                        })?,
                                );
                            }
                            _ => {}
                        }
                    }
                    if let (Some(cur), Some(r)) = (currency, rate) {
                        if r > Decimal::ZERO
                            && (symbols.is_empty()
                                || symbols.iter().any(|s| s.eq_ignore_ascii_case(&cur)))
                        {
                            rates_out.push(("EUR".to_string(), cur, r));
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(ApiError::internal(format!("ECB XML 解析失败: {e}"))),
            _ => {}
        }
    }
    let pairs = rates_out
        .iter()
        .map(|(f, t, _)| format!("{}->{}", f, t))
        .collect();
    Ok((pairs, rates_out))
}

/// 带超时的请求：成功返回响应字节。
async fn fetch_bytes(
    client: &reqwest::Client,
    url: &str,
    timeout: std::time::Duration,
) -> ApiResult<Vec<u8>> {
    let fut = async {
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| ApiError::internal(format!("汇率请求失败: {e}")))?;
        let resp = resp
            .error_for_status()
            .map_err(|e| ApiError::internal(format!("汇率响应状态异常: {e}")))?;
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| ApiError::internal(format!("汇率响应读取失败: {e}")))
    };
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(ApiError::internal("汇率拉取超时")),
    }
}

/// 联网拉取并 upsert source='auto' 行（client 由调用方按代理矩阵构建，本函数不碰代理）。
/// frankfurter/custom 走 JSON；ecb 走 eurofxref-daily.xml（base=EUR）。
/// 返回 {updated, pairs}；失败返回 Err（调用方 warn 并保留旧值——降级链）。
pub async fn fetch_and_store(
    pool: &PgPool,
    client: &reqwest::Client,
    cfg: &crate::config::FxAutoFetchCfg,
) -> ApiResult<serde_json::Value> {
    let timeout = std::time::Duration::from_secs(cfg.timeout_secs.max(1));
    let (pairs, prices): (Vec<String>, Vec<(String, String, Decimal)>) = match cfg.provider.as_str()
    {
        "ecb" => {
            let url = "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml";
            let bytes = fetch_bytes(client, url, timeout).await?;
            parse_ecb(&bytes, &cfg.symbols)?
        }
        _ => {
            // frankfurter（默认）与 custom：同形状 JSON，仅 URL 不同。
            let url = if cfg.provider == "custom" {
                format!("https://{}", cfg.base)
            } else {
                format!(
                    "https://api.frankfurter.app/latest?base={}&symbols={}",
                    cfg.base,
                    cfg.symbols.join(",")
                )
            };
            let bytes = fetch_bytes(client, &url, timeout).await?;
            let json: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|e| ApiError::internal(format!("汇率 JSON 解析失败: {e}")))?;
            parse_rates_json(&json, &cfg.base)?
        }
    };

    let mut updated = 0usize;
    for (from, to, rate) in &prices {
        sqlx::query(
            "INSERT INTO fx_rates (currency_from, currency_to, rate, source, fetched_at, updated_at) \
             VALUES ($1, $2, $3, 'auto', now(), now()) \
             ON CONFLICT (currency_from, currency_to, source) DO UPDATE SET \
               rate = EXCLUDED.rate, fetched_at = now(), updated_at = now()",
        )
        .bind(from)
        .bind(to)
        .bind(rate)
        .execute(pool)
        .await?;
        updated += 1;
    }

    Ok(serde_json::json!({ "updated": updated, "pairs": pairs }))
}

// ---------------------------------------------------------------------------
// 单测（契约 §10）：优先级 / stale / 逆汇率 / from==to / ECB XML 解析。
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::FxRateRow;
    use chrono::{TimeZone, Utc};
    use std::str::FromStr;

    fn row(
        from: &str,
        to: &str,
        rate: &str,
        source: &str,
        fetched_at: Option<DateTime<Utc>>,
    ) -> FxRateRow {
        FxRateRow {
            currency_from: from.to_string(),
            currency_to: to.to_string(),
            rate: Decimal::from_str(rate).unwrap(),
            source: source.to_string(),
            fetched_at,
            updated_at: fetched_at.unwrap_or_else(Utc::now),
        }
    }

    #[test]
    fn manual_overrides_auto() {
        let dt = Utc.timestamp_opt(1_750_000_000, 0).unwrap();
        let rows = vec![
            row("USD", "CNY", "7.2", "auto", Some(dt)),
            row("USD", "CNY", "7.0000", "manual", Some(dt)),
        ];
        let set = build_fx_set(&rows, Utc::now(), 1440);
        let (v, fx) = set.convert("USD", "CNY", Decimal::from(10)).unwrap();
        assert_eq!(v, Decimal::from_str("70.0").unwrap());
        let fxr = fx.unwrap();
        assert_eq!(fxr.source, "manual");
        assert!(!fxr.inverse);
    }

    #[test]
    fn auto_stale_is_dropped() {
        // 拉取于很久以前 → 超过 stale_max_minutes，auto 不可用。
        let stale = Utc::now() - chrono::Duration::hours(48);
        let rows = vec![row("USD", "CNY", "7.2", "auto", Some(stale))];
        let set = build_fx_set(&rows, Utc::now(), 60);
        assert!(set.convert("USD", "CNY", Decimal::from(1)).is_none());
    }

    #[test]
    fn auto_fresh_used_when_no_manual() {
        let fresh = Utc::now();
        let rows = vec![row("USD", "CNY", "7.2", "auto", Some(fresh))];
        let set = build_fx_set(&rows, Utc::now(), 60);
        let (v, fx) = set.convert("USD", "CNY", Decimal::from(1)).unwrap();
        assert_eq!(v, Decimal::from_str("7.2").unwrap());
        assert_eq!(fx.unwrap().source, "auto");
    }

    #[test]
    fn inverse_rate_expanded() {
        let dt = Utc::now();
        // 只有 USD->CNY，CNY->USD 需逆汇率。
        let rows = vec![row("USD", "CNY", "7.2", "manual", Some(dt))];
        let set = build_fx_set(&rows, Utc::now(), 60);
        let (v, fx) = set.convert("CNY", "USD", Decimal::from(72)).unwrap();
        let fxr = fx.unwrap();
        assert_eq!(fxr.inverse, true);
        // 逆汇率 ≈ 1/7.2，用容差断言避免精度差异。
        let inv = fxr.rate;
        assert!(
            (inv - Decimal::from_str("0.138888888888888888889").unwrap()).abs()
                < Decimal::from_str("0.0000000001").unwrap()
        );
        // 72 * (1/7.2) ≈ 10
        assert!((v - Decimal::from(10)).abs() < Decimal::from_str("0.00000001").unwrap());
    }

    #[test]
    fn explicit_beats_inverse() {
        let dt = Utc::now();
        let rows = vec![
            row("USD", "CNY", "7.2", "manual", Some(dt)),
            row("CNY", "USD", "0.14", "manual", Some(dt)),
        ];
        let set = build_fx_set(&rows, Utc::now(), 60);
        let (v, fx) = set.convert("CNY", "USD", Decimal::from(100)).unwrap();
        assert_eq!(v, Decimal::from(14));
        assert_eq!(fx.unwrap().inverse, false);
    }

    #[test]
    fn from_eq_to_passthrough() {
        let rows: Vec<FxRateRow> = Vec::new();
        let set = build_fx_set(&rows, Utc::now(), 60);
        let (v, fx) = set.convert("CNY", "CNY", Decimal::from(5)).unwrap();
        assert_eq!(v, Decimal::from(5));
        assert!(fx.is_none());
    }

    #[test]
    fn ecb_xml_parse() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<gesmes:Envelope xmlns:gesmes="http://www.gesmes.org/xml/2002-08-01" xmlns="http://www.ecb.int/vocabulary/2002-08-01">
  <gesmes:subject>Reference rates</gesmes:subject>
  <Cube>
    <Cube time="2026-01-02">
      <Cube currency="USD" rate="1.09"/>
      <Cube currency="CNY" rate="7.20"/>
      <Cube currency="JPY" rate="156.3"/>
    </Cube>
  </Cube>
</gesmes:Envelope>"#;
        let (pairs, rates) = parse_ecb(xml, &["CNY".to_string()]).unwrap();
        assert_eq!(rates.len(), 1);
        assert_eq!(
            rates[0],
            (
                "EUR".to_string(),
                "CNY".to_string(),
                Decimal::from_str("7.20").unwrap()
            )
        );
        // 全部汇率（symbols 为空时）
        let (_, all) = parse_ecb(xml, &[]).unwrap();
        assert_eq!(all.len(), 3);
        let _ = pairs;
    }

    #[test]
    fn parse_rates_json_shape() {
        let v = serde_json::json!({"base":"USD","rates":{"CNY":7.2,"EUR":0.92}});
        let (pairs, rates) = parse_rates_json(&v, "USD").unwrap();
        assert_eq!(rates.len(), 2);
        assert!(pairs.contains(&"USD->CNY".to_string()));
        assert!(rates
            .iter()
            .any(|(_, t, r)| t == "CNY" && r == &Decimal::from_str("7.2").unwrap()));
    }
}
