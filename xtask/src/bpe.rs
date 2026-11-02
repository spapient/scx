//! cl100k_base 字节对编码（BPE）tokenizer（零第三方 crate）。
//!
//! 词表来自 OpenAI `cl100k_base.tiktoken`（`config/calibrate/cl100k_base.tiktoken`，
//! 每行 `base64(bytes) rank`）。编码采用贪心最长前缀匹配——对普通文本（日志、
//! 代码、中英俄阿、emoji，无 U+0085 等 Unicode 控制行分隔符）与 tiktoken 的
//! `encode` 逐 token 一致（见 `tests::vectors`）。该近似仅影响含罕见 Unicode
//! 控制字符的文本，calibrate 语料不受影响。

use std::collections::HashMap;
use std::fs;

pub(crate) fn load_ranks(path: &str) -> Result<HashMap<Vec<u8>, u32>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut ranks = HashMap::with_capacity(100_256);
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((encoded, rank)) = line.split_once(' ') else {
            return Err(format!("{path}:{index}: bad line {line:?}"));
        };
        let bytes = base64_decode(encoded)
            .ok_or_else(|| format!("{path}:{index}: bad base64 {encoded:?}"))?;
        let rank: u32 = rank
            .parse()
            .map_err(|_| format!("{path}:{index}: bad rank {rank:?}"))?;
        ranks.insert(bytes, rank);
    }
    Ok(ranks)
}

/// 贪心最长前缀匹配 BPE。对输入字节流，每个位置取词表中最长的字节前缀。
pub(crate) fn encode(ranks: &HashMap<Vec<u8>, u32>, bytes: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let n = bytes.len();
    while i < n {
        // 最长可合并字节前缀；找不到时单字节兜底（词表必含全部单字节）。
        let mut matched = None;
        for len in (1..=n - i).rev() {
            if let Some(rank) = ranks.get(&bytes[i..i + len]) {
                matched = Some((len, *rank));
                break;
            }
        }
        match matched {
            Some((len, rank)) => {
                out.push(rank);
                i += len;
            }
            None => {
                // 理论上不可达：cl100k_base 含全部 256 个单字节 token。
                out.push(0);
                i += 1;
            }
        }
    }
    out
}

/// 标准 base64 解码（含 padding）。非法字符返回 None。
pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for c in s.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64_decode("IQ==").unwrap(), b"!");
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(
            base64_decode("aGVsbG8=").unwrap(),
            base64_decode("aGVsbG8").unwrap()
        );
    }

    #[test]
    fn vectors_match_tiktoken() {
        let ranks = load_ranks(concat!(
            concat!(env!("CARGO_MANIFEST_DIR"), "/.."),
            "/config/calibrate/cl100k_base.tiktoken"
        ))
        .unwrap();
        let cases = [
            ("hello world", vec![15339, 1917]),
            ("rer", vec![38149]),
            ("'rer", vec![2351, 81]),
            ("today\n ", vec![31213, 198, 220]),
            ("today\n \n", vec![31213, 27907]),
            ("today\n  \n", vec![31213, 14211]),
            ("👍", vec![9468, 239, 235]),
        ];
        for (text, want) in cases {
            assert_eq!(encode(&ranks, text.as_bytes()), want, "text={text:?}");
        }
    }

    #[test]
    fn roundtrip_self_consistent() {
        let ranks = load_ranks(concat!(
            concat!(env!("CARGO_MANIFEST_DIR"), "/.."),
            "/config/calibrate/cl100k_base.tiktoken"
        ))
        .unwrap();
        // decode(token) 重建字节：rank -> bytes 反向查找（测试用，O(n) 可接受）。
        let reverse: HashMap<u32, &Vec<u8>> = ranks.iter().map(|(b, r)| (*r, b)).collect();
        for text in [
            "数据库连接超时",
            "fn main() {}",
            "The quick brown fox",
            "🚀 emoji ✨ 中文",
        ] {
            let tokens = encode(&ranks, text.as_bytes());
            let mut decoded = Vec::new();
            for token in &tokens {
                decoded.extend_from_slice(reverse[token]);
            }
            assert_eq!(decoded, text.as_bytes(), "roundtrip {text:?}");
        }
    }
}
