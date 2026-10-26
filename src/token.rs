//! Token 计量层（零第三方 crate、确定性）。
//!
//! 两种口径：`Heuristic`（字符三分类启发式，现状默认，`terms::estimate`）与
//! `Calibrated`（按「脚本 × 长度桶」的密度查表，系数冻结在 [`crate::token_calib`]，
//! 由 `tools/gen_token_calib.py` 对真实 tokenizer 离线分桶回归生成后入 repo）。
//! 默认 `Heuristic` 与现状逐字节一致。

#![forbid(unsafe_code)]

/// 计量口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenMode {
    Heuristic,
    Calibrated,
}

impl TokenMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            TokenMode::Heuristic => "heuristic",
            TokenMode::Calibrated => "calibrated",
        }
    }

    pub fn parse(s: &str) -> Option<TokenMode> {
        match s {
            "heuristic" => Some(TokenMode::Heuristic),
            "calibrated" => Some(TokenMode::Calibrated),
            _ => None,
        }
    }
}

/// 估算 token 数（`heuristic`/`calibrated` 两口径）。
pub fn estimate(text: &str, mode: TokenMode) -> f64 {
    match mode {
        TokenMode::Heuristic => crate::terms::estimate(text),
        TokenMode::Calibrated => calibrated(text),
    }
}

/// 字符特征（与 `tools/gen_token_calib.py` 同口径，确定性）：
/// `[words, ascii_alnum, ascii_punct, cjk, other]`。`words` 为 ASCII 词数（≈BPE
/// 主 token 数，捕获「短词多 token 少」）；ASCII 空白不计入（由 `words` 隐含）。
pub fn features(text: &str) -> [f64; 5] {
    let mut words = 0usize;
    let mut alnum = 0usize;
    let mut punct = 0usize;
    let mut cjk = 0usize;
    let mut other = 0usize;
    let mut in_word = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            alnum += 1;
            if !in_word {
                words += 1;
                in_word = true;
            }
            continue;
        }
        in_word = false;
        classify(ch, &mut punct, &mut cjk, &mut other);
    }
    [
        words as f64,
        alnum as f64,
        punct as f64,
        cjk as f64,
        other as f64,
    ]
}

/// 非 ASCII 字母数字字符的分类计数（空白忽略）。
fn classify(ch: char, punct: &mut usize, cjk: &mut usize, other: &mut usize) {
    if ch.is_ascii_punctuation() {
        *punct += 1;
    } else if !ch.is_ascii_whitespace() {
        if crate::terms::full(ch) {
            *cjk += 1;
        } else {
            *other += 1;
        }
    }
}

/// 长度桶索引：按字符总数分 5 桶（1-16 / 17-64 / 65-256 / 257-1024 / 1025+）。
fn bucket(chars: usize) -> usize {
    const BOUNDS: [usize; 4] = [16, 64, 256, 1024];
    BOUNDS
        .iter()
        .position(|&b| chars <= b)
        .unwrap_or(BOUNDS.len())
}

/// 校准口径：按（特征 × 长度桶）密度查表，`tokens = Σ 特征计数 × 密度[bucket]`。
fn calibrated(text: &str) -> f64 {
    let f = features(text);
    let b = bucket((f[1] + f[2] + f[3] + f[4]) as usize);
    let d = crate::token_calib::CALIB_DENSITY;
    f[0] * d[0][b] + f[1] * d[1][b] + f[2] * d[2][b] + f[3] * d[3][b] + f[4] * d[4][b]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_equals_terms_estimate() {
        assert_eq!(
            estimate("hello world", TokenMode::Heuristic),
            crate::terms::estimate("hello world")
        );
        assert_eq!(
            estimate("数据库连接超时", TokenMode::Heuristic),
            crate::terms::estimate("数据库连接超时")
        );
    }

    #[test]
    fn calibrated_is_deterministic() {
        let a = estimate("hello world 数据库", TokenMode::Calibrated);
        let b = estimate("hello world 数据库", TokenMode::Calibrated);
        assert_eq!(a, b);
        assert!(a > 0.0);
    }

    #[test]
    fn features_partition_all_chars() {
        let f = features("abc 数据库!");
        // 非空白字符 = alnum + punct + cjk + other（空白由 words 隐含，不计入）。
        let total = (f[1] + f[2] + f[3] + f[4]) as usize;
        assert_eq!(
            total,
            "abc 数据库!".chars().filter(|c| !c.is_whitespace()).count()
        );
        assert!(f[0] > 0.0, "words");
        assert!(f[1] > 0.0, "alnum");
        assert!(f[3] > 0.0, "cjk");
    }

    #[test]
    fn bucket_partitions_lengths() {
        assert_eq!(bucket(1), 0);
        assert_eq!(bucket(16), 0);
        assert_eq!(bucket(17), 1);
        assert_eq!(bucket(64), 1);
        assert_eq!(bucket(65), 2);
        assert_eq!(bucket(256), 2);
        assert_eq!(bucket(257), 3);
        assert_eq!(bucket(1024), 3);
        assert_eq!(bucket(1025), 4);
    }
}
