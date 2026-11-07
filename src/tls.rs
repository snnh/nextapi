//! TLS 加密提供者初始化。
//!
//! reqwest 0.13 使用 `rustls-no-provider`，不会自动选择 CryptoProvider；
//! 进程内必须在使用任何 reqwest Client 之前安装一次。这里选 ring，
//! 与 sqlx / tokio-tungstenite 的 rustls 依赖保持一致，也避免 aws-lc 的
//! 本地 C/cmake 构建依赖。

use std::sync::Once;

static INSTALL: Once = Once::new();

/// 幂等安装 rustls 默认 CryptoProvider；重复调用无副作用。
pub fn ensure_crypto_provider() {
    INSTALL.call_once(|| {
        // 已安装其他 provider 时忽略错误；进程仍可正常使用已有 provider。
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
