//! 请求侧 URL 图片解析（M10 收尾）：跨协议转换目标为 Gemini 时，Gemini 仅接受
//! inlineData（或 Files API/gs:// 的 fileData），任意 http(s) URL 图片需先受控下载
//! 转 base64 内联。实现参考 new-api（`ResolveBase64Data` 路径：URL 一律下载转
//! inlineData，而非 fileData.fileUri 直传——兼容性最广，旧模型/中转均可用）。
//!
//! 语义（与协议层 degrade 体系一致，不阻断主链路）：
//! - 同请求内同 URL 只下载一次（调用方传入跨候选共享的缓存）；
//! - 并发下载（上限 4）；单张受 media_download 大小/超时限制（netguard SSRF 防护）；
//! - 下载失败 / 非 https URL / mime 非 Gemini 支持集 → ctx.degrade 记录并丢弃该 part；
//! - data: base64 在入口解析时已转 ImageInline，此处不处理。

use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::{self, StreamExt};

use crate::protocol::{ConvCtx, IrContent, IrPart, IrRequest};
use crate::state::AppState;

/// Gemini inlineData 支持的图片 mime 白名单（image-understanding 文档）。
const GEMINI_IMAGE_MIMES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/heic",
    "image/heif",
];

/// 单请求 URL 图片数量上限（防外连放大：16MB body 可容纳数千 URL，
/// 每个都触发一次受控下载——发布审阅 M2）。
pub const MAX_URL_IMAGES_PER_REQUEST: usize = 10;

/// 下载结果缓存：url → 成功 (b64, mime) 或失败原因（失败也缓存，避免候选间重复外连）。
pub type ResolveCache = HashMap<String, Arc<Result<(String, String), String>>>;

/// 收集 IR 中全部 URL 图片（去重，保持出现顺序）。
pub fn collect_image_urls(ir: &IrRequest) -> Vec<String> {
    let mut urls = Vec::new();
    for m in &ir.messages {
        if let Some(IrContent::Parts(parts)) = &m.content {
            for p in parts {
                if let IrPart::ImageUrl { url } = p {
                    if !urls.contains(url) {
                        urls.push(url.clone());
                    }
                }
            }
        }
    }
    urls
}

/// 并发下载所有 URL 图片填入缓存（已在缓存中的跳过）。
pub async fn fetch_all(state: &AppState, urls: &[String], cache: &mut ResolveCache) {
    let pending: Vec<String> = urls
        .iter()
        .filter(|u| !cache.contains_key(u.as_str()))
        .cloned()
        .collect();
    if pending.is_empty() {
        return;
    }
    let results: Vec<(String, Result<(String, String), String>)> = stream::iter(pending)
        .map(|u| async move {
            let r = super::download::download_image_for_request(state, &u)
                .await
                .map_err(|e| e.to_string());
            (u, r)
        })
        .buffered(4)
        .collect()
        .await;
    for (u, r) in results {
        cache.insert(u, Arc::new(r));
    }
}

/// 用缓存把 IR 中的 ImageUrl 重写为 ImageInline；失败项 degrade 记录并丢弃。
/// 返回是否有 URL 图片被成功/失败处理（用于判断是否需要 degrade 语义）。
pub fn rewrite_with_cache(ir: &mut IrRequest, cache: &ResolveCache, ctx: &mut ConvCtx) {
    for m in &mut ir.messages {
        let Some(IrContent::Parts(parts)) = &mut m.content else {
            continue;
        };
        let mut out: Vec<IrPart> = Vec::with_capacity(parts.len());
        for p in std::mem::take(parts) {
            match p {
                IrPart::ImageUrl { url } => match cache.get(&url) {
                    Some(r) => match r.as_ref() {
                        Ok((b64, mime)) => {
                            if GEMINI_IMAGE_MIMES.contains(&mime.as_str()) {
                                out.push(IrPart::ImageInline {
                                    media_type: mime.clone(),
                                    data: b64.clone(),
                                });
                            } else {
                                ctx.degrade(
                                    "image_url",
                                    format!("Gemini 不支持的图片类型 {mime}（{url}），丢弃"),
                                );
                            }
                        }
                        Err(e) => {
                            ctx.degrade("image_url", format!("图片下载失败（{url}）: {e}，丢弃"));
                        }
                    },
                    // 未下载 = 超出单请求上限（fetch 列表被截断）→ degrade 丢弃
                    None => {
                        ctx.degrade(
                            "image_url",
                            format!("单请求 URL 图片超过上限 {MAX_URL_IMAGES_PER_REQUEST}，丢弃（{url}）"),
                        );
                    }
                },
                other => out.push(other),
            }
        }
        *parts = out;
    }
}

/// 一步到位：收集 → 下载 → 重写。
pub async fn resolve_url_images(
    state: &AppState,
    ir: &mut IrRequest,
    cache: &mut ResolveCache,
    ctx: &mut ConvCtx,
) {
    let urls = collect_image_urls(ir);
    if urls.is_empty() {
        return;
    }
    // 超出上限的 URL 不下载，rewrite 时按「超过上限」degrade 丢弃
    let (fetch_list, _excess) = urls.split_at(urls.len().min(MAX_URL_IMAGES_PER_REQUEST));
    fetch_all(state, fetch_list, cache).await;
    rewrite_with_cache(ir, cache, ctx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{IrMessage, IrRole};

    fn ir_with_urls(urls: &[&str]) -> IrRequest {
        IrRequest {
            model: "m".into(),
            messages: vec![IrMessage {
                role: IrRole::User,
                content: Some(IrContent::Parts(
                    urls.iter()
                        .map(|u| IrPart::ImageUrl { url: u.to_string() })
                        .collect(),
                )),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
                reasoning_content: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn collect_dedup() {
        let ir = ir_with_urls(&[
            "https://a.com/1.png",
            "https://a.com/1.png",
            "https://b.com/2.jpg",
        ]);
        assert_eq!(collect_image_urls(&ir).len(), 2);
    }

    #[test]
    fn rewrite_success_failure_and_missing() {
        let mut ir = ir_with_urls(&[
            "https://a.com/ok.png",
            "https://a.com/bad.png",
            "https://a.com/gif.gif",
            "https://a.com/missed.png",
        ]);
        let mut cache = ResolveCache::new();
        cache.insert(
            "https://a.com/ok.png".into(),
            Arc::new(Ok(("QUJD".into(), "image/png".into()))),
        );
        cache.insert(
            "https://a.com/bad.png".into(),
            Arc::new(Err("仅允许 https URL".into())),
        );
        cache.insert(
            "https://a.com/gif.gif".into(),
            Arc::new(Ok(("R0lG".into(), "image/gif".into()))),
        );
        let mut ctx = ConvCtx::new();
        rewrite_with_cache(&mut ir, &cache, &mut ctx);

        let Some(IrContent::Parts(parts)) = &ir.messages[0].content else {
            panic!();
        };
        // ok → inline；bad/gif/missed → 丢弃
        assert_eq!(parts.len(), 1);
        assert!(matches!(
            &parts[0],
            IrPart::ImageInline { media_type, data } if media_type == "image/png" && data == "QUJD"
        ));
        assert_eq!(ctx.degraded.len(), 3);
        assert!(ctx.degraded.iter().any(|d| d.reason.contains("下载失败")));
        assert!(ctx.degraded.iter().any(|d| d.reason.contains("不支持")));
        assert!(ctx.degraded.iter().any(|d| d.reason.contains("超过上限")));
    }
}
