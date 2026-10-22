//! postings.bin 段封装（P3.1/P3.2）：写段头 + 编码原语（varint/crc32）。
//!
//! 与 index.rs 共享布局常量（自产自销，零依赖）：
//! ```text
//! [magic u32 "POST"][ver u16][body_len u64][body][crc32 u32]
//! ```
//! 段尾 CRC-32（IEEE 802.3 查表）是崩溃一致性的最后防线：
//! 写入时整段校验，加载时 body_len 边界 + crc 双校验，坏段报 Err(corrupt)。

use std::path::{Path, PathBuf};

pub(crate) const POST_MAGIC: u32 = 0x5453_4f50; // "POST"
pub(crate) const POST_VER: u16 = 3;

pub(crate) fn postings_path(root: &str) -> PathBuf {
    Path::new(root).join("index").join("postings.bin")
}

/// 写 postings.bin：header + body + crc32（段尾校验，P3.2）。
pub(crate) fn write_file(root: &str, body: &[u8]) -> std::io::Result<()> {
    let p = postings_path(root);
    if let Some(d) = p.parent() {
        crate::memory::lock::create_dir_all(d)?;
    }
    let crc = crc32(body);
    let mut buf = Vec::with_capacity(14 + body.len() + 4);
    buf.extend_from_slice(&POST_MAGIC.to_le_bytes());
    buf.extend_from_slice(&POST_VER.to_le_bytes());
    buf.extend_from_slice(&(body.len() as u64).to_le_bytes());
    buf.extend_from_slice(body);
    buf.extend_from_slice(&crc.to_le_bytes());
    crate::memory::lock::atomic_write(&p, &buf)
}

// ---- varint（LEB128，无符号，7bit 组）----

pub(crate) fn varint_put(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

pub(crate) fn varint_get(buf: &[u8], i: &mut usize) -> Option<u64> {
    let mut v: u64 = 0;
    let mut shift = 0u32;
    loop {
        let b = *buf.get(*i)?;
        *i += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

// ---- CRC-32（IEEE 802.3，多项式 0xEDB88320，查表）----

pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        table[i as usize] = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_and_varint_are_deterministic() {
        let a = crc32(b"hello world");
        assert_eq!(a, crc32(b"hello world"));
        assert_ne!(a, crc32(b"hello worle"));
        let mut buf = Vec::new();
        varint_put(&mut buf, 0);
        varint_put(&mut buf, 127);
        varint_put(&mut buf, 128);
        varint_put(&mut buf, 300);
        varint_put(&mut buf, 1 << 35);
        let mut i = 0;
        assert_eq!(varint_get(&buf, &mut i), Some(0));
        assert_eq!(varint_get(&buf, &mut i), Some(127));
        assert_eq!(varint_get(&buf, &mut i), Some(128));
        assert_eq!(varint_get(&buf, &mut i), Some(300));
        assert_eq!(varint_get(&buf, &mut i), Some(1 << 35));
        assert_eq!(varint_get(&buf, &mut i), None, "越界返回 None");
    }
}
