//! 上游额度探测（M12.1：codex / devin 渠道）。
//!
//! - **codex**：ChatGPT 后端在每次 Responses 响应（含 429）回带 `x-codex-*` 额度头
//!   （官方 CLI `/status` 的同一数据源）。网关在真实流量旁路抓头 → 归一化快照 →
//!   合并进 `upstreams.extra.quota`；管理端也可主动探测（极小请求）即时取一次。
//! - **devin**：`GetUserStatus` 的 plan 块携带日/周额度、剩余百分比与重置时间
//!   （见 `devin::user_status`），探测时读取并落同一 `extra.quota` 槽位。
//!
//! 落库只影响展示：失败静默（不打断主链路），SQL 用运行时校验。

use reqwest::header::HeaderMap;
use sqlx::PgPool;
use uuid::Uuid;

use crate::state::AppState;

/// codex 额度头的响应头 → 归一化快照；无任何 `x-codex-*` 头 → None。
///
/// 实测字段（社区多次抓包一致）：plan_type / active_limit / primary-used-percent /
/// secondary-used-percent / {primary,secondary}-window-minutes / {primary,secondary}-reset-at /
/// {primary,secondary}-reset-after-seconds / primary-over-secondary-limit-percent /
/// credits-{has-credits,balance,unlimited}。空字符串视为缺省（如 secondary-reset-at 常为空）。
pub fn parse_codex_headers(headers: &HeaderMap) -> Option<serde_json::Value> {
    let get = |name: &str| -> Option<String> {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };
    let num = |name: &str| -> Option<f64> { get(name).and_then(|s| s.parse::<f64>().ok()) };

    let plan_type = get("x-codex-plan-type");
    let active_limit = get("x-codex-active-limit");
    let primary_used = num("x-codex-primary-used-percent");
    let secondary_used = num("x-codex-secondary-used-percent");
    let credits_balance = get("x-codex-credits-balance");
    let credits_has =
        get("x-codex-credits-has-credits").map(|s| s.eq_ignore_ascii_case("true") || s == "1");
    let credits_unlimited =
        get("x-codex-credits-unlimited").map(|s| s.eq_ignore_ascii_case("true") || s == "1");
    // 无任何额度特征字段 → 认为上游未返回额度头（普通 API Key 渠道等）
    if plan_type.is_none()
        && active_limit.is_none()
        && primary_used.is_none()
        && secondary_used.is_none()
        && credits_balance.is_none()
        && credits_has.is_none()
        && credits_unlimited.is_none()
    {
        return None;
    }

    let mut out = serde_json::Map::new();
    out.insert("kind".into(), serde_json::json!("codex"));
    out.insert(
        "captured_at".into(),
        serde_json::json!(chrono::Utc::now().timestamp()),
    );
    if let Some(v) = plan_type {
        out.insert("plan_type".into(), serde_json::json!(v));
    }
    if let Some(v) = active_limit {
        out.insert("active_limit".into(), serde_json::json!(v));
    }
    // 主/次窗口：使用率 + 窗口长度（分钟）+ 重置时刻（epoch 秒）/ 剩余秒数
    for (key, prefix) in [
        ("primary", "x-codex-primary"),
        ("secondary", "x-codex-secondary"),
    ] {
        let used = num(&format!("{prefix}-used-percent"));
        let window = num(&format!("{prefix}-window-minutes"));
        let reset_at = num(&format!("{prefix}-reset-at"));
        let reset_after = num(&format!("{prefix}-reset-after-seconds"));
        if used.is_none() && window.is_none() && reset_at.is_none() && reset_after.is_none() {
            continue;
        }
        let mut w = serde_json::Map::new();
        if let Some(v) = used {
            w.insert("used_percent".into(), serde_json::json!(v));
        }
        if let Some(v) = window {
            w.insert("window_minutes".into(), serde_json::json!(v));
        }
        if let Some(v) = reset_at {
            w.insert("reset_at".into(), serde_json::json!(v as i64));
        }
        if let Some(v) = reset_after {
            w.insert("reset_after_seconds".into(), serde_json::json!(v as i64));
        }
        out.insert(key.into(), serde_json::Value::Object(w));
    }
    if let Some(v) = num("x-codex-primary-over-secondary-limit-percent") {
        out.insert(
            "primary_over_secondary_limit_percent".into(),
            serde_json::json!(v),
        );
    }
    if credits_has.is_some() || credits_unlimited.is_some() || credits_balance.is_some() {
        let mut c = serde_json::Map::new();
        if let Some(v) = credits_has {
            c.insert("has_credits".into(), serde_json::json!(v));
        }
        if let Some(v) = credits_unlimited {
            c.insert("unlimited".into(), serde_json::json!(v));
        }
        if let Some(v) = credits_balance {
            // 余额可能是 "0" / ""（缺省已过滤）/ 数值字符串，原样保留字符串
            c.insert("balance".into(), serde_json::json!(v));
        }
        out.insert("credits".into(), serde_json::Value::Object(c));
    }
    Some(serde_json::Value::Object(out))
}

/// 抓头 → 快照（带 source 标记：`live` 真实流量 / `probe` 主动探测）。
pub fn snapshot_from_codex_headers(headers: &HeaderMap, source: &str) -> Option<serde_json::Value> {
    let mut snap = parse_codex_headers(headers)?;
    if let Some(obj) = snap.as_object_mut() {
        obj.insert("source".into(), serde_json::json!(source));
    }
    Some(snap)
}

/// 快照是否在 TTL 内（秒）；`captured_at` 缺失视为过期。可单测。
pub fn is_fresh(snapshot: &serde_json::Value, ttl_secs: i64) -> bool {
    let Some(ts) = snapshot.get("captured_at").and_then(|v| v.as_i64()) else {
        return false;
    };
    let now = chrono::Utc::now().timestamp();
    let age = now.saturating_sub(ts);
    age >= 0 && age <= ttl_secs
}

/// 写快照到 `upstreams.extra.quota`（原子合并；失败静默，仅 debug 日志）。
pub async fn store(db: &PgPool, upstream_id: Uuid, snapshot: &serde_json::Value) {
    let res = sqlx::query(
        "UPDATE upstreams SET extra = jsonb_set(coalesce(extra, '{}'::jsonb), '{quota}', $2::jsonb, \
         true) WHERE id = $1",
    )
    .bind(upstream_id)
    .bind(snapshot)
    .execute(db)
    .await;
    if let Err(e) = res {
        tracing::debug!(upstream = %upstream_id, error = %e, "写入额度快照失败（不影响主链路）");
    }
}

/// 读取 `upstreams.extra.quota`（无 → None；查询失败按 None 处理）。
pub async fn load(db: &PgPool, upstream_id: Uuid) -> Option<serde_json::Value> {
    sqlx::query_scalar::<_, Option<serde_json::Value>>(
        "SELECT extra -> 'quota' FROM upstreams WHERE id = $1",
    )
    .bind(upstream_id)
    .fetch_one(db)
    .await
    .ok()
    .flatten()
}

/// 网关旁路：真实 codex 响应的额度头 → 异步落库（节流：内容未变且 60s 内已写过则跳过）。
///
/// 由 `exec_http` 在拿到响应头后调用，永不阻塞/失败主链路。
pub fn capture_codex_headers(state: &AppState, upstream_id: Uuid, headers: &HeaderMap) {
    let Some(snap) = snapshot_from_codex_headers(headers, "live") else {
        return;
    };
    let db = state.db.clone();
    let now = chrono::Utc::now().timestamp();
    let digest = digest_of(&snap);
    // 节流（原子 check-and-set）：同一上游 60s 内且额度内容完全一致 → 滑动时间戳、不写库；
    // 并发首请求只有第一个胜出，避免同内容重复 spawn 多个 UPDATE
    if let Some(skip) = throttle_claim(&state.quota_last, upstream_id, now, &digest) {
        if skip {
            return;
        }
    }
    tokio::spawn(async move {
        store(&db, upstream_id, &snap).await;
    });
}

/// 侧面节流表：per-upstream 最近一次写入时间戳 + 内容指纹（仅用于减少无谓 UPDATE）。
pub type QuotaThrottle = std::sync::Mutex<std::collections::HashMap<Uuid, (i64, String)>>;

/// 原子「占用写入机会」：内容一致且在窗口内 → 返回 `Some(true)`（跳过写库，仅滑动时间戳）；
/// 否则登记本次写入（`Some(false)`）并放行。全程单次持锁，避免并发重复写。
pub fn throttle_claim(t: &QuotaThrottle, id: Uuid, now: i64, digest: &str) -> Option<bool> {
    let mut m = t.lock().unwrap_or_else(|p| p.into_inner());
    let fresh_same = m
        .get(&id)
        .is_some_and(|(ts, prev)| prev == digest && now.saturating_sub(*ts) < 60);
    if fresh_same {
        m.insert(id, (now, digest.to_string()));
        return Some(true);
    }
    m.insert(id, (now, digest.to_string()));
    Some(false)
}

/// 上游删除时清理节流条目（防长期运行的表缓慢膨胀）。
pub fn throttle_forget(t: &QuotaThrottle, id: &Uuid) {
    t.lock().unwrap_or_else(|p| p.into_inner()).remove(id);
}

/// 快照内容指纹（用于节流比较；captured_at 不参与）。可单测。
pub fn digest_of(snapshot: &serde_json::Value) -> String {
    let mut v = snapshot.clone();
    if let Some(obj) = v.as_object_mut() {
        obj.remove("captured_at");
    }
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    /// 真实抓包形态（社区实证：Pro 计划 5h + 周窗口）
    fn sample() -> HeaderMap {
        hm(&[
            ("x-codex-plan-type", "pro"),
            ("x-codex-active-limit", "premium"),
            ("x-codex-primary-used-percent", "4"),
            ("x-codex-secondary-used-percent", "0"),
            ("x-codex-primary-window-minutes", "10080"),
            ("x-codex-secondary-window-minutes", "0"),
            ("x-codex-primary-reset-after-seconds", "518529"),
            ("x-codex-secondary-reset-after-seconds", "0"),
            ("x-codex-primary-reset-at", "1790416866"),
            ("x-codex-secondary-reset-at", ""),
            ("x-codex-credits-has-credits", "False"),
            ("x-codex-credits-balance", "0"),
            ("x-codex-credits-unlimited", "False"),
            ("x-codex-primary-over-secondary-limit-percent", "0"),
            ("x-request-id", "abc"),
        ])
    }

    #[test]
    fn parse_codex_headers_full() {
        let snap = parse_codex_headers(&sample()).expect("应解析出快照");
        assert_eq!(snap["kind"], "codex");
        assert_eq!(snap["plan_type"], "pro");
        assert_eq!(snap["active_limit"], "premium");
        assert_eq!(snap["primary"]["used_percent"], 4.0);
        assert_eq!(snap["primary"]["window_minutes"], 10080.0);
        assert_eq!(snap["primary"]["reset_at"], 1790416866);
        assert_eq!(snap["primary"]["reset_after_seconds"], 518529);
        assert_eq!(snap["secondary"]["used_percent"], 0.0);
        // 空的 secondary-reset-at 被剔除（不写 null）
        assert!(snap["secondary"].get("reset_at").is_none());
        assert_eq!(snap["credits"]["balance"], "0");
        assert_eq!(snap["credits"]["has_credits"], false);
        assert_eq!(snap["credits"]["unlimited"], false);
        assert!(snap["captured_at"].is_i64());
    }

    #[test]
    fn parse_codex_headers_absent_returns_none() {
        assert!(parse_codex_headers(&HeaderMap::new()).is_none());
        // 只有无关头 → None（普通渠道 API Key 场景）
        assert!(parse_codex_headers(&hm(&[("x-request-id", "x")])).is_none());
    }

    #[test]
    fn parse_codex_headers_free_plan_windows() {
        // Free 计划实测：单窗口 43200 分钟（30 天）+ 百分数
        let snap = parse_codex_headers(&hm(&[
            ("x-codex-primary-used-percent", "9"),
            ("x-codex-primary-window-minutes", "43200"),
            ("x-codex-primary-reset-at", "1789051611"),
            ("x-codex-plan-type", "free"),
        ]))
        .unwrap();
        assert_eq!(snap["primary"]["window_minutes"], 43200.0);
        assert_eq!(snap["plan_type"], "free");
        assert!(snap.get("secondary").is_none(), "无次窗口字段时不输出");
        assert!(snap.get("credits").is_none());
    }

    #[test]
    fn snapshot_source_and_freshness() {
        let snap = snapshot_from_codex_headers(&sample(), "probe").unwrap();
        assert_eq!(snap["source"], "probe");
        assert!(is_fresh(&snap, 300), "刚生成的快照应新鲜");
        assert!(!is_fresh(&snap, -1), "负 TTL → 过期");
        assert!(
            !is_fresh(&serde_json::json!({"kind": "codex"}), 300),
            "缺 captured_at → 过期"
        );
    }

    #[test]
    fn throttle_claim_dedups_and_slides() {
        let t: QuotaThrottle = std::sync::Mutex::new(std::collections::HashMap::new());
        let id = Uuid::new_v4();
        assert_eq!(
            throttle_claim(&t, id, 100, "A"),
            Some(false),
            "首次写入放行"
        );
        assert_eq!(
            throttle_claim(&t, id, 110, "A"),
            Some(true),
            "同内容窗口内跳过"
        );
        assert_eq!(
            throttle_claim(&t, id, 170, "A"),
            Some(false),
            "超出 60s 窗口再次放行"
        );
        assert_eq!(
            throttle_claim(&t, id, 175, "B"),
            Some(false),
            "内容变化立即放行"
        );
        throttle_forget(&t, &id);
        assert!(t.lock().unwrap().get(&id).is_none(), "删除后无残留");
    }

    #[test]
    fn digest_ignores_captured_at() {
        let a = serde_json::json!({"kind":"codex","captured_at":1,"plan_type":"pro"});
        let b = serde_json::json!({"kind":"codex","captured_at":999,"plan_type":"pro"});
        assert_eq!(digest_of(&a), digest_of(&b));
        let c = serde_json::json!({"kind":"codex","captured_at":1,"plan_type":"plus"});
        assert_ne!(digest_of(&a), digest_of(&c));
    }
}
