//! 媒体受控下载（M11.1）：`media_download.enabled` 开启后，把上游返回的图片 URL
//! 下载转为 b64_json 回填给客户端（防 CDN URL 过期/客户端网络不可达上游 CDN）。
//!
//! 安全：SSRF 防护（netguard：仅 https + 内网段 + DNS 复核 + 禁重定向）+
//! 大小/超时上限（media_download.max_size_mb / timeout_secs）+ 代理矩阵
//! （media_download.use_proxy / proxy_id → 全局默认代理）。
//!
//! 语义：下载失败的图片**保留原 URL 不阻断响应**（记 warn）；b64_json 与 url 并存返回。

use base64::Engine;

use crate::error::ApiResult;
use crate::state::AppState;

/// 下载单张图片为 base64。mime 取响应 Content-Type，缺省 image/png。
pub async fn download_b64(state: &AppState, url: &str) -> ApiResult<(String, String)> {
    let hot = state.hot.load();
    let cfg = hot.media_download.clone();
    drop(hot);
    if !cfg.enabled {
        return Err(crate::error::ApiError::Forbidden);
    }
    crate::netguard::check_outbound_url(url).await?;
    let client =
        crate::netguard::onetime_client_via_matrix(state, cfg.use_proxy, &cfg.proxy_id, url);
    let out =
        crate::netguard::fetch_limited(&client, url, cfg.timeout_secs, cfg.max_size_mb).await?;
    let mime = out
        .content_type
        .filter(|t| t.starts_with("image/"))
        .unwrap_or_else(|| "image/png".to_string());
    Ok((
        base64::engine::general_purpose::STANDARD.encode(&out.bytes),
        mime,
    ))
}
