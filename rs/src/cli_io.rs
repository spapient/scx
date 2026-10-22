//! CLI 输入读取：stdin / 多文件聚合，带 64 MiB 上限与 UTF-8 容错解码。

use std::io::{self, Read};

/// 64 MiB：交互式上下文压缩的内存上限；更大的日志应先分段，避免一次读入和筛选向量挤爆内存。
pub(crate) const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) fn decode_bytes(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

pub(crate) fn read_limited<R: Read>(reader: R, limit: u64) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader.take(limit.saturating_add(1)).read_to_end(&mut buf)?;
    Ok(buf)
}

pub(crate) fn exceeds_limit(len: usize, limit: u64) -> bool {
    len as u64 > limit
}

pub(crate) fn limit_error(limit: u64) -> String {
    format!(
        "input exceeds the {} MiB limit; split the input first",
        limit / (1024 * 1024)
    )
}

pub(crate) fn read_files(files: &[String], limit: u64) -> Result<String, String> {
    let mut parts = Vec::new();
    let mut total_bytes = 0usize;
    for f in files {
        let file =
            std::fs::File::open(f).map_err(|e| format!("cannot read input: {}: {}", f, e))?;
        let remaining = limit.saturating_sub(total_bytes as u64);
        let buf = read_limited(file, remaining)
            .map_err(|e| format!("cannot read input: {}: {}", f, e))?;
        total_bytes = total_bytes.saturating_add(buf.len());
        if exceeds_limit(total_bytes, limit) {
            return Err(limit_error(limit));
        }
        parts.push(decode_bytes(buf));
    }
    Ok(parts.join("\n"))
}

pub(crate) fn read_stdin(limit: u64) -> Result<String, String> {
    let stdin = io::stdin();
    let buf = read_limited(stdin.lock(), limit).map_err(|e| format!("cannot read input: {}", e))?;
    if exceeds_limit(buf.len(), limit) {
        return Err(limit_error(limit));
    }
    Ok(decode_bytes(buf))
}

pub(crate) fn read_input(files: &[String]) -> Result<String, String> {
    if files.is_empty() {
        read_stdin(MAX_INPUT_BYTES)
    } else {
        read_files(files, MAX_INPUT_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_reader_detects_one_byte_over_limit() {
        let exact = read_limited(std::io::Cursor::new(b"12345678"), 8).unwrap();
        assert_eq!(exact, b"12345678");
        assert!(!exceeds_limit(exact.len(), 8));

        let over = read_limited(std::io::Cursor::new(b"123456789"), 8).unwrap();
        assert_eq!(over, b"123456789");
        assert!(exceeds_limit(over.len(), 8));
    }

    #[test]
    fn file_limit_is_aggregate_across_inputs() {
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let a = dir.join(format!("scx-cap-a-{}.txt", pid));
        let b = dir.join(format!("scx-cap-b-{}.txt", pid));
        std::fs::write(&a, b"1234").unwrap();
        std::fs::write(&b, b"5678").unwrap();
        let files = vec![
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        ];

        assert_eq!(read_files(&files[..1], 4).unwrap(), "1234");
        assert!(read_files(&files, 7).is_err());
        assert_eq!(read_files(&files, 8).unwrap(), "1234\n5678");

        let _ = std::fs::remove_file(a);
        let _ = std::fs::remove_file(b);
    }
}
