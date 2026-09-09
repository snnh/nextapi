//! 系统信息 / 更新检查 API（/api/system/*，PLAN §7.2；契约 contracts/m8-web.md §1.2）。
//!
//! - GET /version：网关版本与启动时间（AppState.version / started_at）；
//! - POST /check-update：手动触发 GitHub 更新检查（可选 body {use_proxy, proxy_id} 覆盖本次）；
//! - `spawn_scheduled_check`：定时后台检查（仅 tracing::info!/warn!，不落库不审计）。
//!
//! 代理解析遵循 PLAN §5.9 外联矩阵：use_proxy=true 时 proxy_id 取值顺序为
//! body.proxy_id → update_check.proxy_id → proxy.default_proxy_id，全空则直连；
//! 代理不存在/已禁用 → 直连 + tracing::warn。
//!
//! 全部 SQL 使用运行时校验（sqlx::query / query_as），不使用 query! 宏。

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{ConnectInfo, Extension, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::net::SocketAddr;
use uuid::Uuid;

use crate::auth::{self, AdminUsername};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/version", get(get_version))
        .route("/status", get(get_status))
        .route("/diagnostics", get(get_diagnostics))
        .route("/check-update", post(check_update))
}

/// GET /diagnostics：反向代理与部署诊断（M15 §6.5）。
///
/// 安全边界：只返回非敏感信息（Host/scheme、客户端 IP 与可信判定、转发头摘要、
/// 可信代理配置、部署前缀、版本/请求 ID），绝不返回 JWT、API Key、请求体或
/// Authorization/Cookie 等敏感头。
async fn get_diagnostics(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
) -> ApiResult<Json<serde_json::Value>> {
    let trusted_proxies = state
        .file_config
        .read()
        .map(|c| c.server.trusted_proxies.clone())
        .unwrap_or_default();
    let peer_addr = peer.map(|Extension(c)| c.0);
    let peer_ip = peer_addr.map(|a| a.ip());
    let (client_ip, peer_trusted) = auth::diagnose_client_ip(&headers, peer_ip, &trusted_proxies);

    let h = |name: &str| -> Option<String> {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    };
    let xff = h("x-forwarded-for");
    let x_real_ip = h("x-real-ip");
    let x_fwd_host = h("x-forwarded-host");
    let x_fwd_proto = h("x-forwarded-proto");
    let x_fwd_port = h("x-forwarded-port");
    let host = x_fwd_host.clone().or_else(|| h("host"));
    let scheme = x_fwd_proto
        .clone()
        .unwrap_or_else(|| "http".to_string())
        .to_lowercase();

    // 客户端 IP 来源标注（用于解释转发头是否被采信）
    let source = if !peer_trusted {
        "peer"
    } else if xff.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        "xff"
    } else if x_real_ip.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        "x-real-ip"
    } else {
        "peer"
    };

    // 配置建议（只针对当前请求可见的事实，不臆测）
    let mut hints: Vec<String> = Vec::new();
    if trusted_proxies.is_empty() {
        hints.push(
            "未配置 server.trusted_proxies：X-Forwarded-* / X-Real-IP 一律不采信，审计与日志中的客户端 IP 为 TCP 对端".to_string(),
        );
    } else if !peer_trusted {
        hints.push(format!(
            "当前 TCP 对端 {} 不在 trusted_proxies 内：转发头被忽略；请把反向代理主机 IP/CIDR 加入 server.trusted_proxies",
            peer_ip.map(|i| i.to_string()).unwrap_or_else(|| "未知".to_string())
        ));
    }
    if x_fwd_proto.is_none() && host.as_deref().is_some_and(|h| h.contains(':')) {
        hints
            .push("未收到 X-Forwarded-Proto：疑似直连访问，请确认反向代理已配置转发头".to_string());
    }
    if scheme == "https" && x_fwd_proto.is_none() {
        hints.push("检测到 https 但缺少 X-Forwarded-Proto，前端生成的地址可能不正确".to_string());
    }

    Ok(Json(serde_json::json!({
        "version": state.version,
        // 本次诊断请求 ID：可与访问日志/请求日志对照
        "request_id": Uuid::new_v4().to_string(),
        // 部署前缀（来自 X-Forwarded-Prefix；根路径为 "/"）
        "deployment_prefix": crate::embed::deployment_prefix(&headers),
        "request": {
            "host": host,
            "scheme": scheme,
            "user_agent": h("user-agent"),
            "x_forwarded_for": xff,
            "x_real_ip": x_real_ip,
            "x_forwarded_host": x_fwd_host,
            "x_forwarded_proto": x_fwd_proto,
            "x_forwarded_port": x_fwd_port,
            "x_forwarded_prefix": h("x-forwarded-prefix"),
        },
        "client": {
            "peer_addr": peer_addr.map(|a| a.to_string()),
            "peer_ip": peer_ip.map(|i| i.to_string()),
            "peer_trusted": peer_trusted,
            "client_ip": client_ip.map(|i| i.to_string()),
            "source": source,
        },
        "trusted_proxies": trusted_proxies,
        "hints": hints,
    })))
}

/// GET /status：运维状态摘要（M10.4 / PLAN.md §4.4）。
/// 安全边界：不返回 API Key、密码、JWT、加密字段或请求体——错误摘要截断至 120 字符。
async fn get_status(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    // 1. 数据库连通性 + 延迟
    let db_start = Instant::now();
    let db_ok = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();
    let db_latency = db_start.elapsed().as_millis() as i64;

    // 2. 配置来源计数（system_settings.source：ui/file/…）
    let source_rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT source, count(*) FROM system_settings GROUP BY source")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
    let mut settings_sources = serde_json::Map::new();
    for (src, n) in source_rows {
        settings_sources.insert(src, serde_json::Value::from(n));
    }

    // 3. 日志队列 / WAL 状态
    let (queue_used, queue_capacity) = state
        .log_sink
        .queue_status()
        .map(|(u, c)| (Some(u as i64), Some(c as i64)))
        .unwrap_or((None, None));
    let (wal_files, wal_bytes) = std::fs::read_dir(state.log_sink.wal_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
                .fold((0i64, 0i64), |(nf, nb), e| {
                    (
                        nf + 1,
                        nb + e.metadata().map(|m| m.len()).unwrap_or(0) as i64,
                    )
                })
        })
        .unwrap_or((0, 0));

    // 4. 媒体轮询状态
    let poller_cfg = state.hot.load().media_poller.clone();
    let pending_tasks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM media_tasks WHERE status = 'pending'")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);

    // 5. 熔断/异常上游（快照读，零 DB）
    let snap = state.cache.snapshot();
    let breakers: Vec<serde_json::Value> = snap
        .upstreams
        .values()
        .filter(|u| {
            u.disabled_by.is_some() || u.consecutive_failures > 0 || u.cooldown_until.is_some()
        })
        .map(|u| {
            serde_json::json!({
                "name": u.name,
                "consecutive_failures": u.consecutive_failures,
                "disabled_by": u.disabled_by,
                "cooldown_until": u.cooldown_until.map(|t| t.to_rfc3339()),
            })
        })
        .collect();

    // 6. 最近错误摘要（最近 5 条失败明细；error 截断 120 字符）
    let recent_errors: Vec<(chrono::DateTime<chrono::Utc>, String, i32, Option<String>)> =
        sqlx::query_as(
            "SELECT ts, model, status, error FROM usage_logs WHERE status >= 400 \
             ORDER BY ts DESC LIMIT 5",
        )
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let recent_errors: Vec<serde_json::Value> = recent_errors
        .into_iter()
        .map(|(ts, model, status, err)| {
            serde_json::json!({
                "ts": ts.to_rfc3339(),
                "model": model,
                "status": status,
                "error": err.map(|e| e.chars().take(120).collect::<String>()),
            })
        })
        .collect();

    let uptime = (chrono::Utc::now() - state.started_at).num_seconds();
    Ok(Json(serde_json::json!({
        "version": state.version,
        "started_at": state.started_at.to_rfc3339(),
        "uptime_secs": uptime,
        "database": { "ok": db_ok, "latency_ms": db_latency },
        "settings_sources": settings_sources,
        "logging": {
            "queue_used": queue_used,
            "queue_capacity": queue_capacity,
            "overflow_total": state.log_sink.overflow_total(),
            "wal_files": wal_files,
            "wal_bytes": wal_bytes,
        },
        "media_poller": {
            "interval_secs": poller_cfg.interval_secs,
            "max_age_hours": poller_cfg.max_age_hours,
            "pending_tasks": pending_tasks,
        },
        "breakers": breakers,
        "recent_errors": recent_errors,
    })))
}

/// GET /version：网关版本 + 启动时间（RFC3339）。
async fn get_version(
    State(state): State<Arc<AppState>>,
    _admin: AdminUsername,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({
        "version": state.version,
        "started_at": state.started_at.to_rfc3339(),
    })))
}

/// POST /check-update 请求体：全可选（serde snake_case），仅覆盖本次检查。
#[derive(Debug, Default, Deserialize)]
struct CheckUpdateReq {
    #[serde(default)]
    use_proxy: Option<bool>,
    #[serde(default)]
    proxy_id: Option<String>,
}

/// POST /check-update：手动触发 GitHub 更新检查。
/// 失败（网络/非 200）→ 502（ApiError::BadGateway）；成功 200 返回版本对比结果。
async fn check_update(
    State(state): State<Arc<AppState>>,
    admin: AdminUsername,
    body: Option<Json<CheckUpdateReq>>,
) -> ApiResult<Json<serde_json::Value>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let result = perform_check(&state, req.use_proxy, req.proxy_id).await?;

    let summary = serde_json::json!({
        "current_version": result.get("current_version"),
        "latest_version": result.get("latest_version"),
        "update_available": result.get("update_available"),
    });
    auth::audit(
        &state,
        &admin.0,
        "system.check_update",
        "system",
        None,
        summary,
        None,
    )
    .await?;

    Ok(Json(result))
}

/// 执行更新检查：读配置 → 解代理解析 → GET GitHub latest → 解析并比较版本。
///
/// `use_proxy_override` / `proxy_id_override` 为本次覆盖（None 表示不覆盖，用配置默认）。
async fn perform_check(
    state: &Arc<AppState>,
    use_proxy_override: Option<bool>,
    proxy_id_override: Option<String>,
) -> ApiResult<serde_json::Value> {
    let hot = state.hot.load();
    let cfg = hot.update_check.clone();
    drop(hot);

    if cfg.repo.trim().is_empty() {
        return Err(ApiError::bad_request("未配置 update_check.repo"));
    }

    let client = build_client(state, use_proxy_override, proxy_id_override);

    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        cfg.repo.trim()
    );
    let ua = format!("nextapi/{}", state.version);
    let resp = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, ua)
        .timeout(Duration::from_secs(10))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r
                .json()
                .await
                .map_err(|e| ApiError::BadGateway(format!("GitHub 响应解析失败: {e}")))?;
            Ok(build_check_response(state.version, &body))
        }
        Ok(r) => {
            let status = r.status().as_u16();
            Err(ApiError::BadGateway(format!(
                "GitHub 返回非 200 状态码: {status}"
            )))
        }
        Err(e) => Err(ApiError::BadGateway(format!("请求 GitHub 失败: {e}"))),
    }
}

/// 构建检查 client：use_proxy 由 override 或配置决定；走代理时从快照找 ProxyRow，
/// 不存在/已禁用/URL 解析失败 → 回退直连（tracing::warn）。
fn build_client(
    state: &AppState,
    use_proxy_override: Option<bool>,
    proxy_id_override: Option<String>,
) -> reqwest::Client {
    let hot = state.hot.load();
    let cfg = hot.update_check.clone();
    let default_proxy_id = hot.proxy.default_proxy_id.clone();
    drop(hot);

    let use_proxy = use_proxy_override.unwrap_or(cfg.use_proxy);
    if !use_proxy {
        return reqwest::Client::new();
    }

    // proxy_id 取值：body → cfg.proxy_id → proxy.default_proxy_id（非空才取，全空直连）
    let proxy_id = proxy_id_override
        .filter(|s| !s.is_empty())
        .or({
            if cfg.proxy_id.is_empty() {
                None
            } else {
                Some(cfg.proxy_id)
            }
        })
        .or({
            if default_proxy_id.is_empty() {
                None
            } else {
                Some(default_proxy_id)
            }
        });

    let Some(pid_str) = proxy_id else {
        return reqwest::Client::new();
    };
    let Ok(pid) = Uuid::parse_str(&pid_str) else {
        tracing::warn!("无效代理 id `{pid_str}`；更新检查回退直连");
        return reqwest::Client::new();
    };

    let snap = state.cache.snapshot();
    let Some(row) = snap.proxies.get(&pid).cloned() else {
        tracing::warn!("代理 `{pid}` 未找到；更新检查回退直连");
        return reqwest::Client::new();
    };
    if !row.enabled {
        tracing::warn!("代理 `{}` 已禁用；更新检查回退直连", row.name);
        return reqwest::Client::new();
    }

    match reqwest::Proxy::all(row.proxy_url()) {
        Ok(proxy) => match reqwest::Client::builder().proxy(proxy).build() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("构建代理 client 失败: {e}；更新检查回退直连");
                reqwest::Client::new()
            }
        },
        Err(e) => {
            tracing::warn!("代理 `{}` URL 解析失败: {e}；更新检查回退直连", row.name);
            reqwest::Client::new()
        }
    }
}

/// 校验并构建成功响应（纯函数，便于单测版本比较）。
fn build_check_response(current_version: &str, body: &serde_json::Value) -> serde_json::Value {
    let latest_version = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let release_name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let published_at = body
        .get("published_at")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let html_url = body
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let release_body = body.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let release_notes: String = if release_body.is_empty() {
        String::new()
    } else {
        release_body.chars().take(2000).collect()
    };

    let update_available = match compare_versions(current_version, &latest_version) {
        Some(std::cmp::Ordering::Less) => true,
        Some(_) => false,
        None => {
            tracing::warn!(
                "版本号非数字，无法比较: current={current_version}, latest={latest_version}"
            );
            false
        }
    };

    serde_json::json!({
        "current_version": current_version,
        "latest_version": latest_version,
        "release_name": release_name,
        "published_at": published_at,
        "html_url": html_url,
        "update_available": update_available,
        "release_notes": release_notes,
    })
}

/// 去掉版本号前导 `v`/`V`。
fn strip_version_prefix(s: &str) -> &str {
    s.strip_prefix('v')
        .or_else(|| s.strip_prefix('V'))
        .unwrap_or(s)
}

/// 版本比较：去前导 v/V，按 `.` 分段数字比较；任一段非数字 → None（无法比较，视为无更新）。
fn compare_versions(current: &str, latest: &str) -> Option<std::cmp::Ordering> {
    let c = strip_version_prefix(current).split('.').collect::<Vec<_>>();
    let l = strip_version_prefix(latest).split('.').collect::<Vec<_>>();
    let max = c.len().max(l.len());
    for i in 0..max {
        let cp = c.get(i).copied().unwrap_or("0");
        let lp = l.get(i).copied().unwrap_or("0");
        let cn: u64 = cp.parse().ok()?;
        let ln: u64 = lp.parse().ok()?;
        match cn.cmp(&ln) {
            std::cmp::Ordering::Equal => continue,
            ord => return Some(ord),
        }
    }
    Some(std::cmp::Ordering::Equal)
}

/// 定时后台更新检查（M8）：每 60s tick，按 update_check 条件与任务内局部 last 间隔执行。
/// 结果只记 tracing::info!/warn!，不落库不审计。
pub fn spawn_scheduled_check(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut last: Option<Instant> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;

            let hot = state.hot.load();
            let cfg = hot.update_check.clone();
            drop(hot);

            if !cfg.enabled || cfg.interval_hours == 0 || cfg.repo.trim().is_empty() {
                continue;
            }
            if let Some(prev) = last {
                if prev.elapsed() < Duration::from_secs(cfg.interval_hours.saturating_mul(3600)) {
                    continue;
                }
            }

            match perform_check(&state, None, None).await {
                Ok(v) => {
                    if v.get("update_available")
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false)
                    {
                        tracing::warn!("发现新版本可更新: {v}");
                    } else {
                        tracing::info!("更新检查完成（无新版本）");
                    }
                }
                Err(e) => tracing::info!("更新检查失败: {e}"),
            }
            last = Some(Instant::now());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_basic() {
        assert_eq!(
            compare_versions("0.1.0", "v0.1.0"),
            Some(std::cmp::Ordering::Equal)
        );
        assert_eq!(
            compare_versions("0.1.0", "v1.2.3"),
            Some(std::cmp::Ordering::Less)
        );
        assert_eq!(
            compare_versions("2.0.0", "1.9.9"),
            Some(std::cmp::Ordering::Greater)
        );
        assert_eq!(
            compare_versions("1.2", "1.2.3"),
            Some(std::cmp::Ordering::Less)
        );
        assert_eq!(
            compare_versions("1.2.3", "1.2"),
            Some(std::cmp::Ordering::Greater)
        );
    }

    #[test]
    fn version_compare_strips_vcase() {
        assert_eq!(
            compare_versions("1.0.0", "V1.0.0"),
            Some(std::cmp::Ordering::Equal)
        );
        assert_eq!(
            compare_versions("1.0.0", "v1.0.1"),
            Some(std::cmp::Ordering::Less)
        );
    }

    #[test]
    fn version_compare_non_numeric_is_none() {
        assert_eq!(compare_versions("0.1.0", "alpha"), None);
        assert_eq!(compare_versions("1.0.0", "1.x.0"), None);
    }

    #[test]
    fn build_response_marks_update_available() {
        let body = serde_json::json!({
            "tag_name": "v9.9.9",
            "name": "NextAPI 9.9",
            "published_at": "2026-01-01T00:00:00Z",
            "html_url": "https://github.com/a/b/releases/tag/v9.9.9",
            "body": "notes",
        });
        let out = build_check_response("0.1.0", &body);
        assert_eq!(out["current_version"], "0.1.0");
        assert_eq!(out["latest_version"], "v9.9.9");
        assert_eq!(out["release_name"], "NextAPI 9.9");
        assert_eq!(out["published_at"], "2026-01-01T00:00:00Z");
        assert_eq!(
            out["html_url"],
            "https://github.com/a/b/releases/tag/v9.9.9"
        );
        assert_eq!(out["update_available"], true);
        assert_eq!(out["release_notes"], "notes");
    }
}
