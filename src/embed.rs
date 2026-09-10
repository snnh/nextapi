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

use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
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

/// 部署前缀（M15 §6.4 子路径部署）：优先 `X-Forwarded-Prefix`（反代剥离前缀时的唯一可靠来源），
/// 否则回退根路径。始终以 "/" 开头与结尾；仅接受路径安全字符，避免注入 HTML。
pub(crate) fn deployment_prefix(headers: &HeaderMap) -> String {
    let Some(raw) = headers
        .get("x-forwarded-prefix")
        .and_then(|v| v.to_str().ok())
    else {
        return "/".to_string();
    };
    let p = raw.trim();
    if p.is_empty() || p == "/" {
        return "/".to_string();
    }
    let mut s = p.trim_end_matches('/').to_string();
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | '~'))
    {
        tracing::warn!("X-Forwarded-Prefix 含非法字符，已忽略: {raw}");
        return "/".to_string();
    }
    format!("{s}/")
}

/// 去掉部署前缀后的内部路径（用于静态资源查找与 API 前缀判定）。
fn strip_prefix<'a>(path: &'a str, prefix: &str) -> &'a str {
    if prefix == "/" {
        return path;
    }
    let bare = prefix.trim_end_matches('/');
    if path == bare {
        return "/";
    }
    match path.strip_prefix(prefix) {
        Some(rest) => {
            // strip_prefix 去掉尾斜杠后剩余部分以 '/' 开头，补回以保持绝对路径语义
            let start = path.len() - rest.len() - 1;
            &path[start..]
        }
        None => path,
    }
}

/// index.html 统一出口：加 no-cache，确保升级后浏览器不会用旧首页引用已删除的旧资源。
fn index_html(prefix: &str) -> Response {
    let Some(f) = Assets::get("index.html") else {
        return json_404();
    };
    let mut body = String::from_utf8_lossy(&f.data).into_owned();
    if prefix != "/" {
        // 兼容 Vite 规范化后的两种写法
        body = body
            .replace(
                r#"<base href="/" />"#,
                &format!(r#"<base href="{prefix}" />"#),
            )
            .replace(r#"<base href="/">"#, &format!(r#"<base href="{prefix}">"#));
    }
    (
        [
            (header::CONTENT_TYPE, content_type("index.html")),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

/// SPA fallback handler（挂在 Router 末尾，仅接收未匹配请求）。
pub async fn spa_fallback(uri: Uri, method: Method, headers: HeaderMap) -> Response {
    let path = uri.path();
    let prefix = deployment_prefix(&headers);
    // 子路径反代可能保留前缀（proxy_pass 无尾斜杠）——先剥离再匹配资源与 API 前缀
    let inner = strip_prefix(path, &prefix);
    // API / 网关前缀未匹配 → JSON 404，绝不回退 index.html。
    // 前缀比较大小写不敏感并覆盖无尾斜杠形态（/api、/API/xxx，review P2-11）。
    let lower = inner.to_ascii_lowercase();
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

    let rel = inner.trim_start_matches('/');
    let file = if rel.is_empty() { "index.html" } else { rel };

    if file == "index.html" {
        return index_html(&prefix);
    }
    if let Some(f) = Assets::get(file) {
        return file_response(file, f.data.into_owned());
    }
    // 兜底：反代保留前缀但未传 X-Forwarded-Prefix 时，剥离首段再试一次
    // （仅在原路径未命中时触发；深层路径刷新仍需 X-Forwarded-Prefix 才能注入正确的 <base>）
    if let Some((_seg, rest)) = file.split_once('/') {
        if let Some(f) = Assets::get(rest) {
            return file_response(rest, f.data.into_owned());
        }
    }
    // Vite 构建产物（assets/ 下均为内容哈希文件名）未命中即不存在——
    // 典型场景：浏览器缓存的旧 index.html 引用已随升级删除的旧哈希资源。
    // 直接 404，避免回退 HTML 被浏览器当 JS/CSS 解析造成误导性报错。
    if file.starts_with("assets/") {
        return json_404();
    }
    // 未知前端路径（history 路由直达）→ index.html
    index_html(&prefix)
}

/// 静态资源响应：按文件类型给出缓存策略。
/// - `assets/`（Vite 哈希文件，内容变即文件名变）→ 一年强缓存 immutable；
/// - 其它（index.html / favicon.svg 等固定名）→ no-cache，每次校验，避免升级后
///   浏览器拿旧 index.html 引用已删除的旧哈希资源（本次线上问题的根因类型）。
fn file_response(file: &str, data: Vec<u8>) -> Response {
    let cache = if file.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, content_type(file)),
            (header::CACHE_CONTROL, cache),
        ],
        data,
    )
        .into_response()
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
            let resp = spa_fallback(
                Uri::from_static(p),
                Method::GET,
                axum::http::HeaderMap::new(),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "path: {p}");
        }
    }

    #[tokio::test]
    async fn non_get_methods_get_404() {
        let resp = spa_fallback(
            Uri::from_static("/anything"),
            Method::POST,
            axum::http::HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    fn prefix_headers(raw: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-prefix", raw.parse().unwrap());
        h
    }

    #[test]
    fn deployment_prefix_parsing() {
        let empty = HeaderMap::new();
        assert_eq!(deployment_prefix(&empty), "/");
        assert_eq!(deployment_prefix(&prefix_headers("")), "/");
        assert_eq!(deployment_prefix(&prefix_headers("/")), "/");
        assert_eq!(deployment_prefix(&prefix_headers("nextapi")), "/nextapi/");
        assert_eq!(deployment_prefix(&prefix_headers("/nextapi")), "/nextapi/");
        assert_eq!(deployment_prefix(&prefix_headers("/a/b/")), "/a/b/");
        // 非法字符（可能注入 HTML）→ 忽略
        assert_eq!(deployment_prefix(&prefix_headers("/a\"onload=")), "/");
    }

    #[test]
    fn strip_prefix_behaviour() {
        assert_eq!(
            strip_prefix("/nextapi/assets/a.js", "/nextapi/"),
            "/assets/a.js"
        );
        assert_eq!(strip_prefix("/nextapi", "/nextapi/"), "/");
        assert_eq!(strip_prefix("/nextapi/", "/nextapi/"), "/");
        assert_eq!(strip_prefix("/assets/a.js", "/"), "/assets/a.js");
        // 不匹配时原样返回
        assert_eq!(strip_prefix("/other/a.js", "/nextapi/"), "/other/a.js");
    }

    #[tokio::test]
    async fn subpath_requests_resolve_assets_and_inject_base() {
        // 保留前缀的反代：/nextapi/api/* 仍是 JSON 404，不回落 HTML
        let resp = spa_fallback(
            Uri::from_static("/nextapi/api/nope"),
            Method::GET,
            prefix_headers("/nextapi"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // 前端路由直达 → index.html，且注入部署前缀
        let resp = spa_fallback(
            Uri::from_static("/nextapi/logs"),
            Method::GET,
            prefix_headers("/nextapi"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains(r#"<base href="/nextapi/" />"#)
                || text.contains(r#"<base href="/nextapi/">"#),
            "index.html 应注入部署前缀: {text}"
        );

        // 静态资源：带前缀的路径剥离后命中内嵌文件
        let resp = spa_fallback(
            Uri::from_static("/nextapi/favicon.svg"),
            Method::GET,
            prefix_headers("/nextapi"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/svg+xml"
        );

        // 未传 X-Forwarded-Prefix 时，未命中的首段前缀会被剥离后重试一次
        let resp = spa_fallback(
            Uri::from_static("/nextapi/favicon.svg"),
            Method::GET,
            HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/svg+xml"
        );
    }

    /// 旧缓存首页引用的旧哈希资源 → 404（不回退 HTML，避免被当 JS 解析）。
    #[tokio::test]
    async fn missing_hashed_asset_returns_404_not_index() {
        let resp = spa_fallback(
            Uri::from_static("/assets/index-DEADBEEF.js"),
            Method::GET,
            HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json; charset=utf-8"
        );
    }

    /// 缓存策略：index.html no-cache；assets 强缓存 immutable。
    #[tokio::test]
    async fn cache_control_headers() {
        let resp = spa_fallback(Uri::from_static("/"), Method::GET, HeaderMap::new()).await;
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-cache"
        );

        let resp = spa_fallback(
            Uri::from_static("/assets/no-such.js"),
            Method::GET,
            HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // 命中真实 exists 的资源（构建产物至少有一个 assets/*.js）
        let js = Assets::iter()
            .find(|p| p.starts_with("assets/") && p.ends_with(".js"))
            .expect("dist 中应存在 assets/*.js");
        let resp = spa_fallback(
            Uri::from_static(Box::leak(format!("/{js}").into_boxed_str())),
            Method::GET,
            HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=31536000, immutable"
        );
    }
}
