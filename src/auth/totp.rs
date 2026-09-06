//! TOTP（RFC 6238）二次验证：HMAC-SHA1 / 30s 步长 / 6 位动态截断。
//! - 机密为 20 字节随机数，对外展示与存储均为 base32（RFC 4648，无填充）；
//! - 校验容忍 ±1 步（前后 30s 时钟偏差），比对用常量时间；
//! - otpauth:// URI 供认证器（Google Authenticator / 1Password 等）扫码或手输。

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use rand::RngCore;

const SECRET_LEN: usize = 20;
const STEP_SECS: u64 = 30;
const CODE_DIGITS: u32 = 6;
/// 校验窗口：±1 步（容忍客户端时钟偏差与输入耗时）。
const WINDOW: i64 = 1;

/// 生成新机密（原始字节）。
pub fn generate_secret() -> [u8; SECRET_LEN] {
    let mut s = [0u8; SECRET_LEN];
    rand::rng().fill_bytes(&mut s);
    s
}

/// 机密 → base32 展示/存储形态。
pub fn secret_to_base32(secret: &[u8]) -> String {
    BASE32_NOPAD.encode(secret)
}

/// base32 → 机密字节；非法输入返回 None。
pub fn secret_from_base32(s: &str) -> Option<Vec<u8>> {
    let norm: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    BASE32_NOPAD
        .decode(norm.to_ascii_uppercase().as_bytes())
        .ok()
        .filter(|v| v.len() >= 10) // 至少 80bit，防过短机密
}

/// 计算指定时间步的 TOTP 码（6 位数字）。
pub fn code_at(secret: &[u8], step: u64) -> String {
    let mut mac = <Hmac<sha1::Sha1> as Mac>::new_from_slice(secret).expect("HMAC 任意长度密钥");
    mac.update(&step.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = (hash[hash.len() - 1] & 0x0f) as usize;
    let bin = ((hash[offset] as u32 & 0x7f) << 24)
        | ((hash[offset + 1] as u32) << 16)
        | ((hash[offset + 2] as u32) << 8)
        | (hash[offset + 3] as u32);
    let code = bin % 10u32.pow(CODE_DIGITS);
    format!("{code:0width$}", width = CODE_DIGITS as usize)
}

/// 校验用户输入的 TOTP 码（容忍 ±WINDOW 步；空/非数字直接失败）。
pub fn verify(secret: &[u8], code: &str, now_unix: u64) -> bool {
    let code = code.trim();
    if code.len() != CODE_DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let step = now_unix / STEP_SECS;
    for w in -WINDOW..=WINDOW {
        let s = step as i64 + w;
        if s < 0 {
            continue;
        }
        let expect = code_at(secret, s as u64);
        // 常量时间比较（长度已固定 6 位）
        if expect
            .bytes()
            .zip(code.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
        {
            return true;
        }
    }
    false
}

/// 生成 otpauth:// URI（issuer 展示在认证器条目名前）。
pub fn otpauth_url(issuer: &str, account: &str, secret_b32: &str) -> String {
    let enc = |s: &str| {
        s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>()
    };
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits=6&period=30",
        enc(issuer),
        enc(account),
        secret_b32,
        enc(issuer)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Appendix B 测试向量（SHA1，8 位码；截取后 6 位比对末 6 位）。
    #[test]
    fn rfc6238_vectors() {
        let secret = b"12345678901234567890";
        // (unix_time, 8 位期望)
        let cases = [
            (59u64, "94287082"),
            (1111111109, "07081804"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
            (20000000000, "65353130"),
        ];
        for (t, expect8) in cases {
            let got = code_at(secret, t / STEP_SECS);
            assert_eq!(&got, &expect8[2..], "t={t}");
        }
    }

    #[test]
    fn base32_roundtrip_and_rules() {
        let s = generate_secret();
        let b32 = secret_to_base32(&s);
        assert_eq!(secret_from_base32(&b32).unwrap(), s.to_vec());
        // 容忍空格/连字符与小写
        let messy = format!(" {} ", b32.to_lowercase());
        assert_eq!(secret_from_base32(&messy).unwrap(), s.to_vec());
        assert!(secret_from_base32("!!!").is_none());
        assert!(secret_from_base32("JBSWY3DP").is_none()); // 仅 5 字节，过短
    }

    #[test]
    fn verify_window_and_reject() {
        let s = generate_secret();
        let now = 1_700_000_000u64;
        let step = now / STEP_SECS;
        assert!(verify(&s, &code_at(&s, step), now));
        assert!(verify(&s, &code_at(&s, step - 1), now)); // 前一步容忍
        assert!(verify(&s, &code_at(&s, step + 1), now)); // 后一步容忍
        assert!(!verify(&s, &code_at(&s, step + 3), now)); // 超窗拒绝
        assert!(!verify(&s, "12345", now)); // 位数不足
        assert!(!verify(&s, "abcdef", now)); // 非数字
        assert!(!verify(&s, "", now));
    }

    #[test]
    fn otpauth_format() {
        let u = otpauth_url("NextAPI", "admin", "JBSWY3DPEHPK3PXP");
        assert!(u.starts_with("otpauth://totp/NextAPI:admin?secret=JBSWY3DPEHPK3PXP"));
        assert!(u.contains("issuer=NextAPI"));
        let u2 = otpauth_url("Next API", "ad min", "X");
        assert!(u2.contains("Next%20API:ad%20min"));
    }
}
