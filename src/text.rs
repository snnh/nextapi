//! 文本截断小工具（单一实现，多处复用）。
//!
//! 各调用场景（错误摘要、错误正文、debug 脱敏、头信息）此前各写了一份等价实现，
//! 统一到这里，行为口径一致：**只在 UTF-8 字符边界上截断**，绝不产生半个码点。

/// 按字节上限截断（不切断 UTF-8 字符，不追加省略标记）。
pub fn truncate_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// 按字节上限截断并追加 `…`（用于头信息等需明示被截断的展示位）。
pub fn truncate_bytes_ellipsis(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 3);
    out.push_str(&s[..end]);
    out.push('…');
    out
}

/// 按字符数上限截断（错误摘要等按「字符」限长的场景）。
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

/// 字节串 → 按字节上限截断的字符串（先 UTF-8 lossy 解码，用于上游错误正文）。
pub fn truncate_bytes_lossy(bytes: &[u8], max_bytes: usize) -> String {
    truncate_bytes(&String::from_utf8_lossy(bytes), max_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_on_char_boundary() {
        // 多字节字符边界：截断点落在码点中间时向前回退，不产生半个字
        let s = "中文abc";
        assert_eq!(truncate_bytes(s, 100), s);
        assert_eq!(truncate_bytes(s, 4), "中");
        assert_eq!(truncate_bytes(s, 6), "中文");
        assert_eq!(truncate_bytes_ellipsis(s, 4), "中…");
        assert_eq!(truncate_bytes_lossy("中文".as_bytes(), 4), "中");
        // 字符数口径与字节数口径相互独立
        assert_eq!(truncate_chars("中文abc", 3), "中文a");
    }
}
