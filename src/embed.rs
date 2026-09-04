//! M8：管理后台前端静态资源内嵌（rust-embed）。
//!
//! 构建前置：`cd web && npm install && npm run build` 产出 `web/dist` 后，
//! `cargo build` 才能通过（rust-embed 编译期读取该目录；目录缺失会编译失败）。
//! Docker 多阶段构建已自动先构建前端。
//!
//! SPA fallback 语义：
//! - `/api/*`、`/v1/*`、`/healthz`、`/metrics` 未匹配路径 → JSON 404（保持 API 形状）；
//! - 其余 GET 路径：命中内嵌文件则返回（带 Content-Type），否则回退 `index.html`
//!   （前端 vue-router history 模式刷新直达）。
//! - 非 GET 且未匹配 → 404。

use axum::http::{header, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Assets;

/// 简易扩展名 → Content-Type（覆盖 Vite 产物常用类型）。
fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "webmanifest" => "application/manifest+json",
        "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn json_404() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        r#"{"error":{"message":"资源不存在","type":"nextapi_error"}}"#,
    )
        .into_response()
}

/// SPA fallback handler（挂在 Router 末尾，仅接收未匹配请求）。
pub async fn spa_fallback(uri: Uri, method: Method) -> Response {
    let path = uri.path();
    // API / 网关前缀未匹配 → JSON 404，绝不回退 index.html。
    // 前缀比较大小写不敏感并覆盖无尾斜杠形态（/api、/API/xxx，review P2-11）。
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("/api")
        || lower.starts_with("/v1")
        || lower.starts_with("/healthz")
        || lower.starts_with("/metrics")
    {
        return json_404();
    }
    // 仅 GET/HEAD 提供静态页面；其它方法无此资源
    if method != Method::GET && method != Method::HEAD {
        return json_404();
    }

    let rel = path.trim_start_matches('/');
    let file = if rel.is_empty() { "index.html" } else { rel };

    if let Some(f) = Assets::get(file) {
        return (
            [(header::CONTENT_TYPE, content_type(file))],
            f.data.into_owned(),
        )
            .into_response();
    }
    // 未知前端路径（history 路由直达）→ index.html
    match Assets::get("index.html") {
        Some(f) => (
            [(header::CONTENT_TYPE, content_type("index.html"))],
            f.data.into_owned(),
        )
            .into_response(),
        None => json_404(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_basics() {
        assert!(content_type("index.html").contains("text/html"));
        assert!(content_type("assets/a.js").contains("javascript"));
        assert!(content_type("assets/a.css").contains("text/css"));
        assert_eq!(content_type("noext"), "application/octet-stream");
    }

    #[tokio::test]
    async fn api_paths_never_fall_back_to_index() {
        for p in [
            "/api/unknown",
            "/api/logs/xyz",
            "/api",
            "/API/keys",
            "/v1/chat/completions",
            "/v1",
            "/metrics",
            "/metrics/x",
            "/healthz",
            "/healthz/x",
        ] {
            let resp = spa_fallback(Uri::from_static(p), Method::GET).await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "path: {p}");
        }
    }

    #[tokio::test]
    async fn non_get_methods_get_404() {
        let resp = spa_fallback(Uri::from_static("/anything"), Method::POST).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
