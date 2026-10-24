//! BM25 共享公式：召回、PRF 与评估 B 基线使用同一口径，避免公式漂移。

/// Robertson/Sparck Jones IDF（平滑，非负）。
pub(crate) fn idf(n: f64, df: f64) -> f64 {
    ((1.0 + (n - df + 0.5) / (df + 0.5)).ln()).max(0.0)
}

/// BM25 长度归一化分母；空库（avgdl=0）退化为 1.0，避免 NaN。
pub(crate) fn denom(k1: f64, b: f64, dl: f64, avgdl: f64) -> f64 {
    let norm = if avgdl > 0.0 { dl / avgdl } else { 0.0 };
    1.0 + k1 * (1.0 - b + b * norm)
}

/// 单词语义分（tf 恒为 1 的去重词表口径）。
pub(crate) fn score(idf: f64, k1: f64, denom: f64) -> f64 {
    idf * (k1 + 1.0) / denom
}
