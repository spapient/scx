//! Token 计量层（零第三方 crate、确定性）。
//!
//! 两种口径：`Heuristic`（字符三分类启发式，现状默认，`terms::estimate`）与
//! `Calibrated`（字符特征线性回归，系数冻结在 [`crate::token_calib`]，由
//! `tools/gen_token_calib.py` 对真实 tokenizer 离线回归生成后入 repo）。
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

/// 校准口径：字符特征线性回归。特征序与 [`crate::token_calib::CALIB_COEFFS`] 对齐：
/// `[ascii_alnum, ascii_punct, ascii_space, cjk, other, lines, intercept]`。
fn calibrated(text: &str) -> f64 {
    let f = features(text);
    let c = crate::token_calib::CALIB_COEFFS;
    f[0] * c[0] + f[1] * c[1] + f[2] * c[2] + f[3] * c[3] + f[4] * c[4] + f[5] * c[5] + c[6]
}

/// 字符特征（与 `tools/gen_token_calib.py` 同口径，确定性）：
/// `[ascii_alnum, ascii_punct, ascii_space, cjk, other, lines]`。
pub fn features(text: &str) -> [f64; 6] {
    let mut ascii_alnum = 0usize;
    let mut ascii_punct = 0usize;
    let mut ascii_space = 0usize;
    let mut cjk = 0usize;
    let mut other = 0usize;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            ascii_alnum += 1;
        } else if ch.is_ascii_punctuation() {
            ascii_punct += 1;
        } else if ch.is_ascii_whitespace() {
            ascii_space += 1;
        } else if crate::terms::full(ch) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    [
        ascii_alnum as f64,
        ascii_punct as f64,
        ascii_space as f64,
        cjk as f64,
        other as f64,
        text.lines().count() as f64,
    ]
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
        let f = features("abc 123, 数据库!");
        let total = (f[0] + f[1] + f[2] + f[3] + f[4]) as usize;
        assert_eq!(total, "abc 123, 数据库!".chars().count());
        assert!(f[0] > 0.0, "alnum");
        assert!(f[1] > 0.0, "punct");
        assert!(f[2] > 0.0, "space");
        assert!(f[3] > 0.0, "cjk");
    }
}
