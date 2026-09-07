//! 异步媒体任务：poller 轮询计费闭环 + 任务查询端点（PLAN §5.4）。
//! 契约 contracts/m6-media.md §5，M6-C 实现。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::billing::{price_one, PricingInput};
use crate::cache::Snapshot;
use crate::entities::MediaTaskRow;
use crate::error::ApiResult;
use crate::logging::LogEvent;
use crate::media::{self, TaskStatus};
use crate::state::AppState;

/// GET /v1/images/tasks/{task_id}：网关 Key 鉴权 + 归属校验（不跨 Key 泄露，404 语义）。
/// 响应 {task_id, status, model, image_count?, image_size?, data?:[{url}], error?, created_at, finished_at?}。
pub async fn get_image_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Response {
    // 1. Key 鉴权（媒体端点走网关 Key；鉴权失败由 authenticate_gateway_key 返回 401）
    let key = match crate::gateway::authenticate_gateway_key(&state, &headers) {
        Ok(k) => k,
        Err(resp) => return *resp,
    };
    // 2. task_id 必须为合法 UUID，否则 400
    let task_uuid = match Uuid::parse_str(task_id.trim()) {
        Ok(u) => u,
        Err(_) => return err_response(400, "参数错误：task_id 不是合法 UUID"),
    };
    // 3. 查询任务行
    let row = match sqlx::query_as::<_, MediaTaskRow>("SELECT * FROM media_tasks WHERE id = $1")
        .bind(task_uuid)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return err_response(404, "任务不存在"),
        Err(e) => {
            tracing::warn!("媒体任务查询失败: {e}");
            return err_response(500, "查询媒体任务失败");
        }
    };
    // 4. 归属校验：非所属 Key → 404（不泄露存在性）
    if !is_task_owner(row.gateway_key_id, key.id) {
        return err_response(404, "任务不存在");
    }
    // 5. 构造响应体（data 仅在 succeeded 时从 raw.output.results[].url 提取）
    let mut body = json!({
        "task_id": row.id,
        "status": row.status,
        "model": row.model,
        "image_count": row.image_count,
        "image_size": row.image_size,
        "error": row.error,
        "created_at": row.created_at,
        "finished_at": row.finished_at,
    });
    if row.status == "succeeded" {
        body["data"] = json!(extract_image_urls(&row.raw));
    }
    json_response(200, body)
}

/// 后台轮询：pending/processing → fetch_task_status → 完成态单事务 UPDATE + 计价 + sink.log。
/// 每次循环从 `state.hot.load().media_poller` 重读 interval_secs；
/// =0 时空转（不退出），重新置 >0 自动恢复（发布审阅运维 P2-1：此前置 0 永久退出）。
pub async fn run_media_poller(state: Arc<AppState>) {
    loop {
        // 每次从热配置重读：interval_secs=0 → 退出；并取计费时区/汇率陈旧阈值。
        let (interval_secs, max_age_hours, billing_tz, fx_stale) = {
            let hot = state.hot.load();
            (
                hot.media_poller.interval_secs,
                hot.media_poller.max_age_hours,
                hot.gateway.billing_timezone.clone(),
                hot.gateway.fx_stale_max_minutes,
            )
        };
        if interval_secs == 0 {
            // 暂停而非退出：30s 后重读配置，改回 >0 即恢复（进程无需重启）
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            continue;
        }
        // 单轮轮询（失败只 warn，不中断循环）。
        if let Err(e) = poll_once(&state, max_age_hours, &billing_tz, fx_stale).await {
            tracing::warn!("媒体轮询单轮错误: {e}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
    }
}

// ---------------------------------------------------------------------------
// 纯逻辑函数（供单测）
// ---------------------------------------------------------------------------

/// 任务归属纯函数：当前 Key 与任务行 gateway_key_id 相同才可查询（异 key → 404 语义）。
fn is_task_owner(row_key: Uuid, caller_key: Uuid) -> bool {
    row_key == caller_key
}

/// 任务是否超龄（created_at <= now - max_age_hours）。
fn is_expired(created_at: DateTime<Utc>, now: DateTime<Utc>, max_age_hours: u64) -> bool {
    created_at <= now - Duration::hours(max_age_hours as i64)
}

/// 从任务查询结果 raw.output.results[].url 提取 [{url}]（纯函数，可测）。
fn extract_image_urls(raw: &Option<serde_json::Value>) -> Vec<serde_json::Value> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(results) = raw.pointer("/output/results").and_then(|v| v.as_array()) {
        for item in results {
            if let Some(url) = item.get("url").and_then(|v| v.as_str()) {
                out.push(json!({ "url": url }));
            }
        }
    }
    out
}

/// 错误摘要 ≤2000 字符（不切断 UTF-8 码点）。
fn truncate_msg(msg: &str) -> String {
    if msg.len() <= 2000 {
        return msg.to_string();
    }
    let mut end = 2000;
    while end > 0 && !msg.is_char_boundary(end) {
        end -= 1;
    }
    msg[..end].to_string()
}

/// 任务状态归一后的处理类别（纯函数，可测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskOutcome {
    Succeeded,
    Failed,
    InProgress,
}

/// 将 fetch_task_status 归一化状态映射为处理类别：succeeded→成功；failed→失败；
/// pending/processing 及未知值（已归一到前四）→ 同步进行中。
fn finalize_outcome(status: &str) -> TaskOutcome {
    match status {
        "succeeded" => TaskOutcome::Succeeded,
        "failed" => TaskOutcome::Failed,
        _ => TaskOutcome::InProgress,
    }
}

// ---------------------------------------------------------------------------
// 轮询处理
// ---------------------------------------------------------------------------

/// 单轮轮询：SELECT pending/processing 图片任务 → 逐个处理（单项失败 warn 继续下一项）。
async fn poll_once(
    state: &AppState,
    max_age_hours: u64,
    billing_tz: &str,
    fx_stale: u64,
) -> ApiResult<()> {
    let tasks = sqlx::query_as::<_, MediaTaskRow>(
        "SELECT * FROM media_tasks \
         WHERE media_type='image' AND status IN ('pending','processing') \
         ORDER BY created_at LIMIT 100",
    )
    .fetch_all(&state.db)
    .await?;
    let snap = state.cache.snapshot();
    for task in tasks {
        if let Err(e) = process_task(state, &task, &snap, max_age_hours, billing_tz, fx_stale).await
        {
            tracing::warn!("处理媒体任务失败 {task_id}: {e}", task_id = task.id);
        }
    }
    Ok(())
}

/// 处理单个媒体任务。返回 Err 表示该单项失败（调用方 warn 后继续下一项）。
async fn process_task(
    state: &AppState,
    task: &MediaTaskRow,
    snap: &Snapshot,
    max_age_hours: u64,
    billing_tz: &str,
    fx_stale: u64,
) -> ApiResult<()> {
    let now = Utc::now();

    // 超龄（created_at <= now - max_age_hours）→ timeout，不调用上游（状态守卫防重）。
    if is_expired(task.created_at, now, max_age_hours) {
        sqlx::query(
            "UPDATE media_tasks SET status='timeout', finished_at=now(), updated_at=now() \
             WHERE id=$1 AND status IN ('pending','processing')",
        )
        .bind(task.id)
        .execute(&state.db)
        .await?;
        return Ok(());
    }

    // 上游被删/禁用 → skip 留到下轮。
    let Some(up) = snap.upstreams.get(&task.upstream_id) else {
        tracing::warn!("媒体任务上游不存在，跳过: {}", task.id);
        return Ok(());
    };
    if !up.enabled {
        tracing::warn!("媒体任务上游已禁用，跳过: {}", task.id);
        return Ok(());
    }
    let Some(provider_task_id) = task.provider_task_id.as_deref() else {
        tracing::warn!("媒体任务缺少 provider_task_id，跳过: {}", task.id);
        return Ok(());
    };

    // 客户端按代理矩阵选择（参照 run_gateway：use_proxy=true 且 proxy_id 空跟随 default_proxy_id，
    // no_proxy 并集命中直连；未配置直连）。
    let (default_proxy_id, global_no_proxy) = {
        let hot = state.hot.load();
        (
            hot.proxy.default_proxy_id.clone(),
            hot.proxy.no_proxy.clone(),
        )
    };
    let mut lists: Vec<Vec<String>> = vec![global_no_proxy];
    let eff_proxy_id = if up.use_proxy {
        up.proxy_id
            .or_else(|| Uuid::parse_str(&default_proxy_id).ok())
    } else {
        None
    };
    if let Some(pid) = eff_proxy_id {
        if let Some(p) = snap.proxies.get(&pid) {
            lists.push(p.no_proxy.clone());
        }
    }
    let client = if up.use_proxy && crate::upstream::no_proxy_match(&lists, &up.base_url) {
        state.client_pools.direct_client()
    } else {
        state.client_pools.client_for(up, snap, &default_proxy_id)
    };

    // 查询上游任务状态（失败 → warn 后留待下轮，不改动任务状态）。
    let st = match media::fetch_task_status(
        &client,
        crate::presets::media_base_url(up),
        provider_task_id,
        up.api_key_plain.as_deref(),
        up.timeout_ms,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                "媒体任务状态查询失败（留待下轮） {task_id}: {e}",
                task_id = task.id
            );
            return Ok(());
        }
    };

    match finalize_outcome(&st.status) {
        TaskOutcome::Succeeded => finalize_succeeded(state, task, &st, billing_tz, fx_stale).await,
        TaskOutcome::Failed => finalize_failed(state, task, &st).await,
        TaskOutcome::InProgress => sync_status(state, task, &st).await,
    }
}

/// 成功：单事务 UPDATE（状态守卫防重）+ 计价 + LogEvent。
async fn finalize_succeeded(
    state: &AppState,
    task: &MediaTaskRow,
    st: &TaskStatus,
    billing_tz: &str,
    fx_stale: u64,
) -> ApiResult<()> {
    let now = Utc::now();

    // 计价（失败只 warn：任务照常标记成功，cost 留 NULL，事件照记 pricing_source=None）。
    let input = PricingInput {
        at: now,
        images: st.image_count,
        image_size: st.image_size.clone(),
        ..Default::default()
    };
    let (cost_cny, cost_usd, pricing_source, price_used, fx_snapshot) = match price_one(
        &state.db,
        task.upstream_id,
        &task.model,
        &input,
        billing_tz,
        fx_stale,
    )
    .await
    {
        Ok(r) => (
            r.cost_cny,
            r.cost_usd,
            if r.priced {
                Some("bound".to_string())
            } else {
                None
            },
            if r.priced { Some(r.price_used) } else { None },
            if r.priced { Some(r.fx_snapshot) } else { None },
        ),
        Err(e) => {
            tracing::warn!(
                "媒体任务计价失败（cost 留 NULL） {task_id}: {e}",
                task_id = task.id
            );
            (None, None, None, None, None)
        }
    };

    let image_count: Option<i32> = st.image_count.map(|c| c as i32);
    let updated = sqlx::query(
        "UPDATE media_tasks SET status='succeeded', image_count=$2, image_size=$3, raw=$4, \
         finished_at=now(), updated_at=now(), cost_cny=$5, cost_usd=$6 \
         WHERE id=$1 AND status IN ('pending','processing') \
         RETURNING id",
    )
    .bind(task.id)
    .bind(image_count)
    .bind(&st.image_size)
    .bind(&st.raw)
    .bind(cost_cny)
    .bind(cost_usd)
    .execute(&state.db)
    .await?;

    // 状态守卫防重：rows_affected=0 说明已被其它轮次处理，跳过记账（避免重复计费）。
    if updated.rows_affected() == 0 {
        tracing::warn!("媒体任务已完成（状态守卫跳过）: {}", task.id);
        return Ok(());
    }

    let request_id = format!(
        "{}-done",
        task.request_id
            .clone()
            .unwrap_or_else(|| task.id.to_string())
    );
    let ev = LogEvent {
        request_id,
        ts: now,
        key_id: Some(task.gateway_key_id),
        model: task.model.clone(),
        requested_model: None,
        upstream_id: Some(task.upstream_id),
        protocol_in: "openai_image".to_string(),
        protocol_out: "images_dashscope_async".to_string(),
        convert_mode: "media_task".to_string(),
        stream: false,
        status: 200,
        error: None,
        prompt_tokens: None,
        completion_tokens: None,
        cache_write_tokens: None,
        cache_read_tokens: None,
        latency_ms: None,
        retry_count: 0,
        ttfb_ms: None,
        degraded: false,
        usage_raw: None,
        debug_payload: None,
        cost_cny,
        cost_usd,
        pricing_source,
        price_used,
        fx_snapshot,
        images: st.image_count,
        image_size: st.image_size.clone(),
        video_seconds: None,
        video_resolution: None,
        video_task_type: None,
    };
    state.log_sink.log(ev);
    Ok(())
}

/// 失败/取消（已归一到 failed）→ 状态 failed + error；LogEvent status=502 不计价。
async fn finalize_failed(state: &AppState, task: &MediaTaskRow, st: &TaskStatus) -> ApiResult<()> {
    let error = truncate_msg(&st.error.clone().unwrap_or_else(|| "任务失败".to_string()));
    let updated = sqlx::query(
        "UPDATE media_tasks SET status='failed', error=$2, raw=$3, finished_at=now(), updated_at=now() \
         WHERE id=$1 AND status IN ('pending','processing')",
    )
    .bind(task.id)
    .bind(&error)
    .bind(&st.raw)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Ok(());
    }

    let request_id = format!(
        "{}-done",
        task.request_id
            .clone()
            .unwrap_or_else(|| task.id.to_string())
    );
    let ev = LogEvent {
        request_id,
        ts: Utc::now(),
        key_id: Some(task.gateway_key_id),
        model: task.model.clone(),
        requested_model: None,
        upstream_id: Some(task.upstream_id),
        protocol_in: "openai_image".to_string(),
        protocol_out: "images_dashscope_async".to_string(),
        convert_mode: "media_task".to_string(),
        stream: false,
        status: 502,
        error: Some(error),
        prompt_tokens: None,
        completion_tokens: None,
        cache_write_tokens: None,
        cache_read_tokens: None,
        latency_ms: None,
        retry_count: 0,
        ttfb_ms: None,
        degraded: false,
        usage_raw: None,
        debug_payload: None,
        cost_cny: None,
        cost_usd: None,
        pricing_source: None,
        price_used: None,
        fx_snapshot: None,
        images: None,
        image_size: None,
        video_seconds: None,
        video_resolution: None,
        video_task_type: None,
    };
    state.log_sink.log(ev);
    Ok(())
}

/// pending/processing：仅同步状态（状态守卫，防覆盖已定稿任务）。
async fn sync_status(state: &AppState, task: &MediaTaskRow, st: &TaskStatus) -> ApiResult<()> {
    sqlx::query(
        "UPDATE media_tasks SET status=$2, updated_at=now() \
         WHERE id=$1 AND status IN ('pending','processing')",
    )
    .bind(task.id)
    .bind(&st.status)
    .execute(&state.db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 响应构造
// ---------------------------------------------------------------------------

/// 构造 JSON 响应（Content-Type: application/json）。
fn json_response(status: u16, body: serde_json::Value) -> Response {
    let status =
        axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::BAD_GATEWAY);
    (status, Json(body)).into_response()
}

/// 错误响应（OpenAI 形状错误体）。
fn err_response(status: u16, message: &str) -> Response {
    let status =
        axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::BAD_GATEWAY);
    (
        status,
        Json(json!({ "error": { "message": message, "type": "nextapi_error" } })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// 单测（contracts/m6-media.md §7 / §10）
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_owner_same_key_ok() {
        let k = Uuid::new_v4();
        assert!(is_task_owner(k, k));
    }

    #[test]
    fn task_owner_diff_key_404_semantics() {
        // 异 key → 不属于当前 Key（纯函数返回 false，调用方据此走 404 语义）。
        let owner = Uuid::new_v4();
        let caller = Uuid::new_v4();
        assert!(!is_task_owner(owner, caller));
        // 「不存在」无行 → 纯函数无行可判，由调用方在无行时直接 404（此处验证 owner 判断）。
    }

    #[test]
    fn finalize_outcome_mapping() {
        assert_eq!(finalize_outcome("succeeded"), TaskOutcome::Succeeded);
        assert_eq!(finalize_outcome("failed"), TaskOutcome::Failed);
        assert_eq!(finalize_outcome("pending"), TaskOutcome::InProgress);
        assert_eq!(finalize_outcome("processing"), TaskOutcome::InProgress);
        // 未知值已在 media::normalize_task_status 归一 → 保守按进行中处理
        assert_eq!(finalize_outcome("unknown"), TaskOutcome::InProgress);
    }

    #[test]
    fn expired_by_age() {
        let now = Utc::now();
        let old = now - Duration::hours(48);
        let recent = now - Duration::hours(1);
        assert!(is_expired(old, now, 24));
        assert!(!is_expired(recent, now, 24));
        // 恰好等于 cutoff（左闭区间）视为超龄
        let exact = now - Duration::hours(24);
        assert!(is_expired(exact, now, 24));
    }

    #[test]
    fn extract_urls_from_raw() {
        let raw = Some(json!({
            "output": { "results": [ { "url": "https://a/1.png" }, { "url": "https://a/2.png" } ] }
        }));
        let urls = extract_image_urls(&raw);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0]["url"], "https://a/1.png");
        assert_eq!(urls[1]["url"], "https://a/2.png");
        // 无 raw / 无 results → 空
        assert_eq!(extract_image_urls(&None).len(), 0);
        assert_eq!(extract_image_urls(&Some(json!({"output": {}}))).len(), 0);
    }

    #[test]
    fn truncate_msg_handles_char_boundary() {
        let long = "x".repeat(2500);
        let t = truncate_msg(&long);
        assert_eq!(t.len(), 2000);
        assert!(t.is_char_boundary(t.len()));
        // 短消息原样返回
        assert_eq!(truncate_msg("ok"), "ok");
    }
}
