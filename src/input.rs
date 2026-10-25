//! CLI 输入读取：stdin / 多文件聚合，带 64 MiB 上限与 UTF-8 容错解码。

use std::io::{self, Read};

/// 64 MiB：交互式上下文压缩的内存上限；更大的日志应先分段，避免一次读入和筛选向量挤爆内存。
pub(crate) const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) fn decode(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

pub(crate) fn read<R: Read>(reader: R, limit: u64) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader.take(limit.saturating_add(1)).read_to_end(&mut buf)?;
    Ok(buf)
}

pub(crate) fn large(len: usize, limit: u64) -> bool {
    len as u64 > limit
}

pub(crate) fn error(limit: u64) -> String {
    format!(
        "input exceeds the {} MiB limit; split the input first",
        limit / (1024 * 1024)
    )
}

pub(crate) fn sources(files: &[String], limit: u64) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut total_bytes = 0usize;
    for f in files {
        // 文件边界只在必要时插入换行；限制仍按真实输入字节计数，不把生成的分隔符
        // 算进配额。单缓冲同时避免“每个文件一份 Vec + join 再复制”的峰值内存。
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        let file =
            std::fs::File::open(f).map_err(|e| format!("cannot read input: {}: {}", f, e))?;
        let remaining = limit.saturating_sub(total_bytes as u64);
        let before = bytes.len();
        file.take(remaining.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| format!("cannot read input: {}: {}", f, e))?;
        total_bytes = total_bytes.saturating_add(bytes.len() - before);
        if large(total_bytes, limit) {
            return Err(error(limit));
        }
    }
    Ok(decode(bytes))
}

pub(crate) fn stdin(limit: u64) -> Result<String, String> {
    let stdin = io::stdin();
    let buf = read(stdin.lock(), limit).map_err(|e| format!("cannot read input: {}", e))?;
    if large(buf.len(), limit) {
        return Err(error(limit));
    }
    Ok(decode(buf))
}

pub fn input(files: &[String]) -> Result<String, String> {
    if files.is_empty() {
        stdin(MAX_INPUT_BYTES)
    } else {
        sources(files, MAX_INPUT_BYTES)
    }
}

/// 读取 `--fix-file` 文本，并在分配前限制到与记忆库相同的上限。
pub fn fix(path: &str, limit: u64) -> Result<String, String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("cannot read --fix-file {path}: {e}"))?;
    let buf = read(file, limit).map_err(|e| format!("cannot read --fix-file {path}: {e}"))?;
    if large(buf.len(), limit) {
        return Err(format!("--fix-file {path} exceeds the {limit}-byte limit"));
    }
    Ok(decode(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_reader_detects_one_byte_over_limit() {
        let exact = read(std::io::Cursor::new(b"12345678"), 8).unwrap();
        assert_eq!(exact, b"12345678");
        assert!(!large(exact.len(), 8));

        let over = read(std::io::Cursor::new(b"123456789"), 8).unwrap();
        assert_eq!(over, b"123456789");
        assert!(large(over.len(), 8));
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

        assert_eq!(sources(&files[..1], 4).unwrap(), "1234");
        assert!(sources(&files, 7).is_err());
        assert_eq!(sources(&files, 8).unwrap(), "1234\n5678");

        let _ = std::fs::remove_file(a);
        let _ = std::fs::remove_file(b);
    }

    #[test]
    fn file_boundary_does_not_add_blank_lines() {
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let a = dir.join(format!("scx-boundary-a-{pid}.txt"));
        let b = dir.join(format!("scx-boundary-b-{pid}.txt"));
        std::fs::write(&a, b"first\n").unwrap();
        std::fs::write(&b, b"second\n").unwrap();
        let files = vec![
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        ];
        assert_eq!(sources(&files, 13).unwrap(), "first\nsecond\n");
        let _ = std::fs::remove_file(a);
        let _ = std::fs::remove_file(b);
    }

    #[test]
    fn fix_file_is_capped_before_decode() {
        let path = std::env::temp_dir().join(format!(
            "scx-fix-cap-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"12345").unwrap();
        let path = path.to_string_lossy();
        assert_eq!(fix(&path, 5).unwrap(), "12345");
        assert!(fix(&path, 4).is_err());
        let _ = std::fs::remove_file(path.as_ref());
    }
}
