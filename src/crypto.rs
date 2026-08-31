//! 敏感字段应用层加密：AES-256-GCM。
//!
//! 密钥来自环境变量 `NEXTAPI_SECRET_KEY`（env-only，不落 DB/YAML，不可 UI 设置），
//! 取其 SHA-256 作为 32 字节密钥。存储格式：`base64(12B nonce || ciphertext+tag)`。
//! 未设置密钥时 `is_available() == false`，涉及敏感字段的写入应被拒绝。

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::RngCore;
use sha2::{Digest, Sha256};

pub const ENV_SECRET_KEY: &str = "NEXTAPI_SECRET_KEY";

#[derive(Clone)]
pub struct Crypto(Option<Aes256Gcm>);

impl Crypto {
    pub fn from_env() -> Self {
        match std::env::var(ENV_SECRET_KEY) {
            Ok(s) if !s.is_empty() => Self::from_secret(&s),
            _ => Self(None),
        }
    }

    pub fn from_secret(secret: &str) -> Self {
        let key = Sha256::digest(secret.as_bytes());
        Self(Some(Aes256Gcm::new_from_slice(&key).expect("32 字节密钥")))
    }

    pub fn is_available(&self) -> bool {
        self.0.is_some()
    }

    pub fn encrypt(&self, plain: &str) -> anyhow::Result<String> {
        let cipher = self
            .0
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("{ENV_SECRET_KEY} 未设置，无法加密敏感字段"))?;
        let mut nonce_bytes = [0u8; 12];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), plain.as_bytes())
            .map_err(|_| anyhow::anyhow!("AES-256-GCM 加密失败"))?;
        let mut buf = Vec::with_capacity(12 + ct.len());
        buf.extend_from_slice(&nonce_bytes);
        buf.extend_from_slice(&ct);
        Ok(B64.encode(buf))
    }

    pub fn decrypt(&self, enc: &str) -> anyhow::Result<String> {
        let cipher = self
            .0
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("{ENV_SECRET_KEY} 未设置，无法解密敏感字段"))?;
        let buf = B64.decode(enc).map_err(|e| anyhow::anyhow!("密文 base64 解码失败: {e}"))?;
        if buf.len() < 13 {
            anyhow::bail!("密文长度非法");
        }
        let (nonce, ct) = buf.split_at(12);
        let pt = cipher
            .decrypt(Nonce::from_slice(nonce), ct)
            .map_err(|_| anyhow::anyhow!("AES-256-GCM 解密失败（密钥错误或密文损坏）"))?;
        Ok(String::from_utf8(pt)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let c = Crypto::from_secret("test-secret");
        assert!(c.is_available());
        let enc = c.encrypt("proxy-password-123").unwrap();
        assert_ne!(enc, "proxy-password-123");
        assert_eq!(c.decrypt(&enc).unwrap(), "proxy-password-123");
    }

    #[test]
    fn wrong_key_fails() {
        let a = Crypto::from_secret("key-a");
        let b = Crypto::from_secret("key-b");
        let enc = a.encrypt("hello").unwrap();
        assert!(b.decrypt(&enc).is_err());
    }

    #[test]
    fn unavailable() {
        let c = Crypto(None);
        assert!(!c.is_available());
        assert!(c.encrypt("x").is_err());
    }
}
