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
    let mime = smart_image_mime(out.content_type.as_deref(), url, &out.bytes);
    Ok((
        base64::engine::general_purpose::STANDARD.encode(&out.bytes),
        mime,
    ))
}

/// 请求侧图片下载（M10 收尾，参考 new-api：URL 图片一律受控下载转 inlineData）。
/// 与 download_b64 的区别：不受 `media_download.enabled` 开关控制——
/// 该开关是「响应侧 b64 回填」的语义；请求侧 URL→inline 是 Gemini 转换的必需步骤。
/// 安全约束一致：netguard SSRF 校验 + 大小/超时上限 + 代理矩阵（均复用 media_download 段）。
pub async fn download_image_for_request(
    state: &AppState,
    url: &str,
) -> ApiResult<(String, String)> {
    let hot = state.hot.load();
    let cfg = hot.media_download.clone();
    drop(hot);
    crate::netguard::check_outbound_url(url).await?;
    let client =
        crate::netguard::onetime_client_via_matrix(state, cfg.use_proxy, &cfg.proxy_id, url);
    let out =
        crate::netguard::fetch_limited(&client, url, cfg.timeout_secs, cfg.max_size_mb).await?;
    let mime = smart_image_mime(out.content_type.as_deref(), url, &out.bytes);
    Ok((
        base64::engine::general_purpose::STANDARD.encode(&out.bytes),
        mime,
    ))
}

/// 图片 mime 智能探测（参考 new-api smartDetectMimeType）：
/// Content-Type(image/*) → URL 扩展名 → magic bytes 嗅探 → 兜底 image/png。
pub fn smart_image_mime(content_type: Option<&str>, url: &str, bytes: &[u8]) -> String {
    if let Some(ct) = content_type {
        let ct = ct.split(';').next().unwrap_or("").trim();
        if ct.starts_with("image/") && ct != "image/svg+xml" {
            return ct.to_string();
        }
    }
    if let Some(m) = mime_from_url_ext(url) {
        return m.to_string();
    }
    if let Some(m) = sniff_image_mime(bytes) {
        return m.to_string();
    }
    "image/png".to_string()
}

/// 按 URL 路径扩展名推断图片 mime（忽略 query/fragment）。
fn mime_from_url_ext(url: &str) -> Option<&'static str> {
    let path = reqwest::Url::parse(url).ok()?;
    let seg = path.path().rsplit('/').next()?;
    let ext = seg.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        _ => return None,
    })
}

/// magic bytes 嗅探常见图片格式（无外部依赖）。
fn sniff_image_mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if b.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if b.starts_with(b"BM") {
        return Some("image/bmp");
    }
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    // HEIC/HEIF：ISO BMFF，ftyp box 品牌 heic/heix/hevc/hevx/mif1/msf1
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        let brand = &b[8..12];
        if brand.starts_with(b"he") || brand == b"mif1" || brand == b"msf1" {
            return Some("image/heic");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_mime_priority() {
        // Content-Type 优先
        assert_eq!(
            smart_image_mime(Some("image/jpeg; charset=x"), "https://a.com/x.png", b""),
            "image/jpeg"
        );
        // 非 image/* 的 Content-Type → 扩展名
        assert_eq!(
            smart_image_mime(
                Some("application/octet-stream"),
                "https://a.com/x.webp?q=1",
                b""
            ),
            "image/webp"
        );
        // 无扩展名 → 嗅探
        let png = b"\x89PNG\r\n\x1a\n....";
        assert_eq!(smart_image_mime(None, "https://a.com/x", png), "image/png");
        // 全部失败 → 兜底
        assert_eq!(
            smart_image_mime(None, "https://a.com/x", b"zz"),
            "image/png"
        );
        // svg 不算位图，继续往下探测
        let jpg = b"\xff\xd8\xff\xe0....";
        assert_eq!(
            smart_image_mime(Some("image/svg+xml"), "https://a.com/x", jpg),
            "image/jpeg"
        );
    }

    #[test]
    fn sniff_formats() {
        assert_eq!(sniff_image_mime(b"GIF89a...."), Some("image/gif"));
        assert_eq!(sniff_image_mime(b"BM...."), Some("image/bmp"));
        assert_eq!(sniff_image_mime(b"RIFFxxxxWEBP"), Some("image/webp"));
        assert_eq!(
            sniff_image_mime(b"\x00\x00\x00\x18ftypheic"),
            Some("image/heic")
        );
        assert_eq!(sniff_image_mime(b"random"), None);
    }
}
