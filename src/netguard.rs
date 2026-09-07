//! 出站立刻校验（SSRF 防护，M11 抽取共享）：
//! - 仅 https；host 为 IP 时禁内网段（含 CGNAT/NAT64/链路本地/ULA/回环）；
//! - localhost/.local/.internal 等内网域名拒绝；域名 DNS 解析后逐 IP 复核
//!   （任一内网即拒，解析失败也拒——宁可失败不可盲发）；
//! - 一次性 client 禁重定向（重定向目标不复查会绕过校验）。
//!
//! 调用方负责：超时、大小上限、代理选择（各业务配置不同）。

use std::time::Duration;

use crate::error::{ApiError, ApiResult};

/// SSRF 校验：URL 合法性 + 协议 + 内网地址/域名 + DNS 复核。
/// 返回复核通过的 IP 列表——调用方应据此钉住连接解析（resolve_to），
/// 否则复核与实际建连两次解析结果可不同（DNS rebinding TOCTOU，发布审阅 M1）。
pub async fn check_outbound_url(url: &str) -> ApiResult<Vec<std::net::IpAddr>> {
    let parsed =
        reqwest::Url::parse(url).map_err(|e| ApiError::bad_request(format!("URL 非法: {e}")))?;
    if parsed.scheme() != "https" {
        return Err(ApiError::bad_request("仅允许 https URL"));
    }
    let host = parsed.host_str().unwrap_or_default();
    if host.is_empty() {
        return Err(ApiError::bad_request("URL 缺少主机名"));
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_internal_ip(ip) {
            return Err(ApiError::bad_request("禁止访问内网地址"));
        }
        return Ok(vec![ip]);
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
        || lower.ends_with(".localdomain")
    {
        return Err(ApiError::bad_request("禁止访问本机/内网域名"));
    }
    // 域名：DNS 解析后复核（防解析到内网/云元数据的域名）；
    // 任一解析结果为内网即拒绝；解析失败也拒绝（宁可失败不可盲发）。
    let ips = tokio::net::lookup_host((host, 443))
        .await
        .map_err(|e| ApiError::bad_request(format!("域名解析失败: {e}")))?
        .collect::<Vec<_>>();
    if ips.is_empty() {
        return Err(ApiError::bad_request("域名无解析结果"));
    }
    if ips.iter().any(|sa| is_internal_ip(sa.ip())) {
        return Err(ApiError::bad_request("域名解析到内网地址，禁止访问"));
    }
    Ok(ips.into_iter().map(|sa| sa.ip()).collect())
}

/// 内网/保留地址判定（IPv4 段 + IPv6 回环/链路本地/ULA/NAT64/映射地址）。
pub fn is_internal_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = u32::from(v4);
            (o & 0xFF00_0000) == 0x0A00_0000 // 10/8
                || (o & 0xFFF0_0000) == 0xAC10_0000 // 172.16/12
                || (o & 0xFFFF_0000) == 0xC0A8_0000 // 192.168/16
                || (o & 0xFF00_0000) == 0x7F00_0000 // 127/8
                || (o & 0xFFFF_0000) == 0xA9FE_0000 // 169.254/16
                || (o & 0xFFC0_0000) == 0x6440_0000 // 100.64/10 CGNAT 共享地址
        }
        std::net::IpAddr::V6(v6) => {
            // IPv4-mapped IPv6（::ffff:a.b.c.d）按内嵌 IPv4 判定
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_internal_ip(std::net::IpAddr::V4(v4));
            }
            let seg = v6.segments();
            v6.is_loopback() // ::1
                || ((seg[0] & 0xFFC0) == 0xFE80) // fe80::/10 链路本地
                || ((seg[0] & 0xFE00) == 0xFC00) // fc00::/7 ULA 私网
                || (seg[0] == 0x0064 && seg[1] == 0xff9b) // 64:ff9b::/96 NAT64（可能映射内网，兜底拒绝）
        }
    }
}

/// 一次性出站 client（禁重定向、禁 gzip 自动解压——受控下载按原始字节计量，
/// 防压缩炸弹；低频操作不共享连接池）。调用方再按业务挂代理。
pub fn onetime_client(timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_gzip()
        .connect_timeout(Duration::from_secs(timeout_secs.max(1).min(30)))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 按代理矩阵构建一次性 client（M11 从 pricing 抽取共享）：
/// use_proxy=true 时 proxy_id 取值顺序为业务配置 proxy_id → 全局 proxy.default_proxy_id；
/// 目标命中 no_proxy 并集（全局 + 代理自身）则直连；代理不存在/已禁用 → 直连。
pub fn onetime_client_via_matrix(
    state: &crate::state::AppState,
    use_proxy: bool,
    proxy_id: &str,
    url: &str,
    pinned_ips: &[std::net::IpAddr],
) -> reqwest::Client {
    let hot = state.hot.load();
    let snap = state.cache.snapshot();

    let mut lists = vec![hot.proxy.no_proxy.clone()];
    let eff_proxy_id = if use_proxy {
        uuid::Uuid::parse_str(proxy_id)
            .ok()
            .or_else(|| uuid::Uuid::parse_str(&hot.proxy.default_proxy_id).ok())
    } else {
        None
    };
    if let Some(pid) = eff_proxy_id {
        if let Some(p) = snap.proxies.get(&pid) {
            lists.push(p.no_proxy.clone());
        }
    }
    let via_proxy = use_proxy && !crate::upstream::no_proxy_match(&lists, url);
    if via_proxy {
        if let Some(pid) = eff_proxy_id {
            if let Some(p) = snap.proxies.get(&pid) {
                if let Ok(proxy) = reqwest::Proxy::all(p.proxy_url()) {
                    if let Ok(client) = reqwest::Client::builder()
                        .proxy(proxy)
                        .redirect(reqwest::redirect::Policy::none())
                        .no_gzip()
                        .build()
                    {
                        return client;
                    }
                }
            }
        }
    }
    // 直连：钉住 DNS 复核结果（TLS SNI/Host 仍用原域名；仅 https，端口 443）。
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()));
    if let (Some(host), false) = (host, pinned_ips.is_empty()) {
        let addrs: Vec<std::net::SocketAddr> = pinned_ips
            .iter()
            .map(|ip| std::net::SocketAddr::new(*ip, 443))
            .collect();
        if let Ok(client) = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_gzip()
            .resolve_to_addrs(&host, &addrs)
            .connect_timeout(Duration::from_secs(10))
            .build()
        {
            return client;
        }
    }
    onetime_client(10)
}

/// 受限读取结果（字节 + 可选 Content-Type，媒体下载需要 mime）。
pub struct FetchOut {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

/// 带大小上限读取响应体（先查 content-length，再查实际字节数；拒绝重定向状态）。
pub async fn fetch_limited(
    client: &reqwest::Client,
    url: &str,
    timeout_secs: u64,
    max_size_mb: u64,
) -> ApiResult<FetchOut> {
    let max_bytes = max_size_mb.saturating_mul(1_048_576u64).max(1);
    let timeout = Duration::from_secs(timeout_secs.max(1));
    let resp = client
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| ApiError::internal(format!("URL 拉取失败: {e}")))?;
    if resp.status().is_redirection() {
        return Err(ApiError::bad_request("不允许重定向（禁跟随 3xx）"));
    }
    let resp = resp
        .error_for_status()
        .map_err(|e| ApiError::bad_request(format!("URL 返回异常状态: {e}")))?;
    if let Some(cl) = resp.content_length() {
        if cl > max_bytes {
            return Err(ApiError::bad_request(format!(
                "文件超过大小限制 {max_size_mb}MB"
            )));
        }
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty());
    // 流式读取逐 chunk 计量，超限即中断——不能 bytes() 全量入内存后再判
    // （content-length 可伪造/缺失，全量缓冲 + 解压炸弹会打爆内存，发布审阅 H2）。
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    loop {
        let chunk = futures::StreamExt::next(&mut stream)
            .await
            .transpose()
            .map_err(|e| ApiError::internal(format!("读取响应失败: {e}")))?;
        let Some(chunk) = chunk else { break };
        if buf.len() + chunk.len() > max_bytes as usize {
            return Err(ApiError::bad_request(format!(
                "文件超过大小限制 {max_size_mb}MB"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(FetchOut {
        bytes: buf,
        content_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_ip_rules() {
        use std::net::IpAddr;
        let v4 = |a: u8, b: u8, c: u8, d: u8| IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, d));
        assert!(is_internal_ip(v4(10, 0, 0, 1)));
        assert!(is_internal_ip(v4(172, 16, 0, 1)));
        assert!(is_internal_ip(v4(172, 31, 255, 1)));
        assert!(is_internal_ip(v4(192, 168, 1, 1)));
        assert!(is_internal_ip(v4(127, 0, 0, 1)));
        assert!(is_internal_ip(v4(169, 254, 169, 254))); // 云元数据
        assert!(is_internal_ip(v4(100, 64, 0, 1))); // CGNAT
        assert!(!is_internal_ip(v4(8, 8, 8, 8)));
        assert!(!is_internal_ip(v4(172, 15, 0, 1))); // 172.15 不在 /12
                                                     // IPv6：回环/ULA/链路本地/映射
        assert!(is_internal_ip("::1".parse().unwrap()));
        assert!(is_internal_ip("fd00::1".parse().unwrap()));
        assert!(is_internal_ip("fe80::1".parse().unwrap()));
        assert!(is_internal_ip("::ffff:10.0.0.1".parse().unwrap()));
        assert!(!is_internal_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[tokio::test]
    async fn url_scheme_and_host_rules() {
        // 非 https
        assert!(check_outbound_url("http://example.com/a.png")
            .await
            .is_err());
        // 内网 IP 字面量
        assert!(check_outbound_url("https://10.0.0.1/a").await.is_err());
        assert!(check_outbound_url("https://127.0.0.1/").await.is_err());
        assert!(check_outbound_url("https://[::1]/").await.is_err());
        // localhost 域
        assert!(check_outbound_url("https://localhost/x").await.is_err());
        assert!(check_outbound_url("https://a.localhost/x").await.is_err());
        assert!(check_outbound_url("https://svc.internal/x").await.is_err());
        // 非法 URL
        assert!(check_outbound_url("not a url").await.is_err());
        // 公网 IP 字面量放行
        assert!(check_outbound_url("https://8.8.8.8/a.png").await.is_ok());
    }
}
