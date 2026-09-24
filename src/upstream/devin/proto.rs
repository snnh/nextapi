//! 最小 protobuf wire 编解码（手写，无 prost 依赖）——Devin Connect 协议专用。
//!
//! 字段号映射来源：`.owc/devin-ref/RESEARCH.md`（一手逆向 + 真实流量实证）。
//! 仅覆盖本项目用到的 wire 类型：0(varint)/1(f64)/2(len)/5(f32)；解码侧未知类型报错。

use bytes::{BufMut, Bytes, BytesMut};

/// 写 varint。
pub fn put_varint(buf: &mut BytesMut, mut n: u64) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            buf.put_u8(b);
            return;
        }
        buf.put_u8(b | 0x80);
    }
}

/// 写字符串/字节字段（wire type 2）。
pub fn put_bytes(buf: &mut BytesMut, field: u32, val: &[u8]) {
    put_varint(buf, ((field << 3) | 2) as u64);
    put_varint(buf, val.len() as u64);
    buf.put_slice(val);
}

/// 写字符串字段。
pub fn put_str(buf: &mut BytesMut, field: u32, val: &str) {
    put_bytes(buf, field, val.as_bytes());
}

/// 写 varint 字段（wire type 0）。
pub fn put_uint(buf: &mut BytesMut, field: u32, n: u64) {
    put_varint(buf, (field << 3) as u64);
    put_varint(buf, n);
}

/// 写 double 字段（wire type 1）。
pub fn put_double(buf: &mut BytesMut, field: u32, x: f64) {
    put_varint(buf, ((field << 3) | 1) as u64);
    buf.put_slice(&x.to_le_bytes());
}

/// 写嵌套消息字段（wire type 2，调用方先编码子消息）。
pub fn put_msg(buf: &mut BytesMut, field: u32, sub: &[u8]) {
    put_bytes(buf, field, sub);
}

/// Connect 流式信封封装：flags(1) + len(4, BE) + payload。
pub fn envelope(payload: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(payload.len() + 5);
    buf.put_u8(0);
    buf.put_u32(payload.len() as u32);
    buf.put_slice(payload);
    buf.freeze()
}

/// 解码后的字段值（wire 全类型保留，便于完整往返）。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Val {
    Varint(u64),
    Fixed64([u8; 8]),
    Bytes(Bytes),
    Fixed32([u8; 4]),
}

impl Val {
    pub fn as_bytes(&self) -> Option<&Bytes> {
        match self {
            Val::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::Bytes(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    pub fn as_uint(&self) -> Option<u64> {
        match self {
            Val::Varint(n) => Some(*n),
            _ => None,
        }
    }

    /// 仅测试使用（round-trip 校验 wire 全类型可解）
    #[cfg(test)]
    pub fn as_double(&self) -> Option<f64> {
        match self {
            Val::Fixed64(b) => Some(f64::from_le_bytes(*b)),
            _ => None,
        }
    }
}

/// 一条解码出的字段：(字段号, 值)。
pub type Field = (u32, Val);

/// 解码 protobuf 消息为字段列表（保序；未知 wire 类型返回 Err）。
pub fn decode(buf: &[u8]) -> Result<Vec<Field>, String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < buf.len() {
        let (key, ni) = read_varint(buf, i)?;
        let (field, wt) = ((key >> 3) as u32, (key & 7) as u8);
        i = ni;
        match wt {
            0 => {
                let (n, ni) = read_varint(buf, i)?;
                i = ni;
                out.push((field, Val::Varint(n)));
            }
            1 => {
                if buf.len().saturating_sub(i) < 8 {
                    return Err("f64 越界".into());
                }
                let mut b = [0u8; 8];
                b.copy_from_slice(&buf[i..i + 8]);
                i += 8;
                out.push((field, Val::Fixed64(b)));
            }
            2 => {
                let (ln, ni) = read_varint(buf, i)?;
                i = ni;
                // u64 → usize 转换与 i+ln 均需防溢出：恶意 varint（如 2^64-1）会让
                // 加法回绕绕过越界守卫，随后切片 panic
                let ln = usize::try_from(ln).map_err(|_| "len 溢出".to_string())?;
                if ln > buf.len().saturating_sub(i) {
                    return Err("len 越界".into());
                }
                out.push((field, Val::Bytes(Bytes::copy_from_slice(&buf[i..i + ln]))));
                i += ln;
            }
            5 => {
                if buf.len().saturating_sub(i) < 4 {
                    return Err("f32 越界".into());
                }
                let mut b = [0u8; 4];
                b.copy_from_slice(&buf[i..i + 4]);
                i += 4;
                out.push((field, Val::Fixed32(b)));
            }
            other => return Err(format!("未知 wire type {other}")),
        }
    }
    Ok(out)
}

fn read_varint(buf: &[u8], mut i: usize) -> Result<(u64, usize), String> {
    let mut val = 0u64;
    let mut shift = 0;
    loop {
        if i >= buf.len() {
            return Err("varint 越界".into());
        }
        let b = buf[i];
        i += 1;
        val |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok((val, i));
        }
        shift += 7;
        if shift > 63 {
            return Err("varint 过长".into());
        }
    }
}

/// 取字段列表中某字段的首个值。
pub fn get(fields: &[Field], field: u32) -> Option<&Val> {
    fields.iter().find(|(f, _)| *f == field).map(|(_, v)| v)
}

/// 取字段列表中某字段的所有值（repeated）。
pub fn get_all(fields: &[Field], field: u32) -> Vec<&Val> {
    fields
        .iter()
        .filter(|(f, _)| *f == field)
        .map(|(_, v)| v)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_fields() {
        let mut buf = BytesMut::new();
        put_str(&mut buf, 1, "devin-cli");
        put_uint(&mut buf, 2, 300);
        put_double(&mut buf, 5, 1.0);
        put_msg(&mut buf, 6, b"sub");
        let fields = decode(&buf).unwrap();
        assert_eq!(fields.len(), 4);
        assert_eq!(get(&fields, 1).unwrap().as_str(), Some("devin-cli"));
        assert_eq!(get(&fields, 2).unwrap().as_uint(), Some(300));
        assert_eq!(get(&fields, 5).unwrap().as_double(), Some(1.0));
        assert_eq!(get(&fields, 6).unwrap().as_bytes().unwrap(), "sub");
    }

    #[test]
    fn envelope_layout() {
        let env = envelope(b"abc");
        assert_eq!(&env[..], &[0, 0, 0, 0, 3, b'a', b'b', b'c']);
    }

    #[test]
    fn varint_large() {
        let mut buf = BytesMut::new();
        put_uint(&mut buf, 1, 128000);
        let fields = decode(&buf).unwrap();
        assert_eq!(get(&fields, 1).unwrap().as_uint(), Some(128000));
    }
}
