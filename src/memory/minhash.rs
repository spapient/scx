//! 纯算法近似语义召回：加权 MinHash-LSH 签名 + band 分桶 + 精确 Jaccard 精排。
//!
//! 采用文献/工业界成熟的 weighted MinHash-LSH 方案：128 哈希、IDF 加权、band
//! 分桶、固定种子哈希（splitmix64）。零第三方依赖、零随机源、确定性（无
//! clock/rand）。shingle = 词元（按 IDF 加权，`terms::memory` 切词）+ 字符
//! 3-gram（定权 1.0）。
//!
//! 加权 MinHash 采用 ICWS（Improved Consistent Weighted Sampling）一致性加权
//! 采样：对每个哈希位置取 `-ln(u)/w` 最小的 shingle 作为签名元素，高 IDF 术语
//! 对指纹的影响更大。**加权只在签名（LSH 候选初筛）生效**；最终相似度用精确
//! 无权 Jaccard（`|A∩B|/|A∪B|`）计算，对齐方案 §2.2.3「完整 Jaccard 精排」。
//!
//! LSH 分桶阈值：`s = (1/b)^(1/r)`，b=`N_BANDS`、r=`BAND_ROWS`。当前
//! 8 段 × 16 行 → s ≈ 0.88（近重复导向、高阈值保守）；调大 `N_BANDS` 可降低
//! 阈值以多召回（候选集变大、剪枝减弱）。小库（< `LSH_THRESHOLD`）走精确
//! Jaccard，不受该阈值影响、无 LSH 假阴性。

use std::collections::HashMap;

pub(crate) const N_HASHES: usize = 128;
pub(crate) const N_BANDS: usize = 8;
const BAND_ROWS: usize = N_HASHES / N_BANDS;
/// 超过该规模才走 LSH 候选初筛；小库全量精确加权 Jaccard（无 LSH 假阴性）。
const LSH_THRESHOLD: usize = 4096;
/// 字符 3-gram 的固定权重（词元权重 = IDF，二者同处一个 shingle 表）。
const CHARGRAM_WEIGHT: f64 = 1.0;
/// shingle 上限：词元已由 `terms` 限 256，字符 3-gram 无上限，需独立封顶防超大正文。
const MAX_SHINGLES: usize = 1 << 16;

/// query 与每个 doc 的加权 Jaccard 相似度（词元 IDF 加权 + 字符 3-gram）。
pub(crate) fn scores(query: &str, docs: &[&str]) -> Vec<f64> {
    if docs.is_empty() {
        return Vec::new();
    }
    let n = docs.len();
    let df = doc_freq(docs);
    let q = shingles(query, &df, n);
    let ds: Vec<HashMap<u64, f64>> = docs.iter().map(|d| shingles(d, &df, n)).collect();
    if n < LSH_THRESHOLD {
        return ds.iter().map(|d| jaccard(&q, d)).collect();
    }
    let q_sig = weighted_signature(&q);
    let sigs: Vec<[u64; N_HASHES]> = ds.iter().map(weighted_signature).collect();
    let cand = candidates(&q_sig, &sigs);
    ds.iter()
        .enumerate()
        .map(|(i, d)| {
            if cand.contains(&i) {
                jaccard(&q, d)
            } else {
                0.0
            }
        })
        .collect()
}

/// 词元 df 预扫描：每个词元出现在多少个 doc（单 doc 内去重）。
fn doc_freq(docs: &[&str]) -> HashMap<String, usize> {
    let mut df = HashMap::new();
    for d in docs {
        let mut seen = std::collections::HashSet::new();
        for t in crate::terms::memory(d) {
            if seen.insert(t.clone()) {
                *df.entry(t).or_insert(0) += 1;
            }
        }
    }
    df
}

/// 单文本加权 shingle 表：词元 → IDF 权重（词元先入为主，字符 gram 不覆盖）。
fn shingles(text: &str, df: &HashMap<String, usize>, n: usize) -> HashMap<u64, f64> {
    let mut map = HashMap::new();
    for t in crate::terms::memory(text) {
        // 库级噪声词（出现在全部 doc）不参与：IDF 加权只能降到 1.0、无法剔除，
        // 硬过滤掉可避免它稀释加权 Jaccard 的分母（语义层实测退化根因）。
        if n >= 2 && df.get(&t).copied().unwrap_or(0) >= n {
            continue;
        }
        let key = crate::memory::util::fnv1a(&t);
        let w = idf(df.get(&t).copied().unwrap_or(0), n);
        map.entry(key).or_insert(w);
    }
    chargrams(text, &mut map);
    map
}

/// 平滑 IDF：`ln((n+1)/(df+1)) + 1`，恒 ≥ 1；高频词低权重（不归零，属加权而非剔除）。
fn idf(df: usize, n: usize) -> f64 {
    ((n as f64 + 1.0) / (df as f64 + 1.0)).ln() + 1.0
}

/// 字符 3-gram（字母/数字/下划线连续段，小写化）以定权并入 shingle 表；
/// 已有词元键不覆盖（词元 IDF 权重优先）。
fn chargrams(text: &str, map: &mut HashMap<u64, f64>) {
    if map.len() >= MAX_SHINGLES {
        return;
    }
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let mut gram = String::with_capacity(3);
    for w in chars.windows(3) {
        if map.len() >= MAX_SHINGLES {
            break;
        }
        if !w.iter().all(|&c| eligible(c)) {
            continue;
        }
        gram.clear();
        for &c in w {
            gram.push(c);
        }
        map.entry(crate::memory::util::fnv1a(&gram))
            .or_insert(CHARGRAM_WEIGHT);
    }
}

fn eligible(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 加权 MinHash 签名（ICWS）：每个哈希位置取 `-ln(u)/w` 最小的 shingle。
fn weighted_signature(shingles: &HashMap<u64, f64>) -> [u64; N_HASHES] {
    let mut sig = [0u64; N_HASHES];
    let mut best = [f64::INFINITY; N_HASHES];
    for (&s, &w) in shingles {
        if w <= 0.0 {
            continue;
        }
        for (i, slot) in sig.iter_mut().enumerate() {
            let y = -u01(i as u64, s).ln() / w;
            if y < best[i] {
                best[i] = y;
                *slot = s;
            }
        }
    }
    sig
}

/// `mix(seed, x)` 归一化到 (0, 1] 的确定性伪均匀值。
fn u01(seed: u64, x: u64) -> f64 {
    (mix(seed, x) as f64 + 1.0) / 2.0_f64.powi(64)
}

/// splitmix64：固定种子确定性哈希族（seed=哈希序号，x=shingle 键）。
fn mix(seed: u64, x: u64) -> u64 {
    let mut z = x.wrapping_add(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 第 `band` 段的桶键：段内 16 个签名 shingle XOR 后经 `mix` 定型。
fn band_key(band: usize, sig: &[u64; N_HASHES]) -> u64 {
    let mut acc = 0u64;
    for &v in &sig[band * BAND_ROWS..(band + 1) * BAND_ROWS] {
        acc ^= v;
    }
    mix(band as u64, acc)
}

/// LSH 候选初筛：与 query 共享至少一个 band 桶的 doc 下标集合。
fn candidates(
    q_sig: &[u64; N_HASHES],
    doc_sigs: &[[u64; N_HASHES]],
) -> std::collections::HashSet<usize> {
    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    for (di, sig) in doc_sigs.iter().enumerate() {
        for b in 0..N_BANDS {
            buckets.entry(band_key(b, sig)).or_default().push(di);
        }
    }
    let mut cand = std::collections::HashSet::new();
    for b in 0..N_BANDS {
        if let Some(list) = buckets.get(&band_key(b, q_sig)) {
            cand.extend(list.iter().copied());
        }
    }
    cand
}

/// 精确（无权）Jaccard：`|A∩B| / |A∪B|`，空集返回 0。
///
/// 精排用无权 Jaccard（对齐方案 §2.2.3「完整 Jaccard 精排」）；IDF 加权只作用在
/// 加权 MinHash 签名（LSH 候选初筛）上，不进入最终相似度。
fn jaccard(a: &HashMap<u64, f64>, b: &HashMap<u64, f64>) -> f64 {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if large.is_empty() {
        return 0.0;
    }
    let inter = small.keys().filter(|k| large.contains_key(k)).count();
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(text: &str) -> HashMap<u64, f64> {
        shingles(text, &HashMap::new(), 1)
    }

    #[test]
    fn identical_text_scores_one() {
        let d = "ERROR db timeout conn_id=9f2a errno=28";
        let s = scores(d, &[d]);
        assert!(
            (s[0] - 1.0).abs() < 1e-9,
            "identical text must be Jaccard 1: {}",
            s[0]
        );
    }

    #[test]
    fn disjoint_text_scores_zero() {
        let s = scores("aaaa bbbb", &["zzzz yyyy"]);
        assert!(
            (s[0] - 0.0).abs() < 1e-9,
            "disjoint text must be Jaccard 0: {}",
            s[0]
        );
    }

    #[test]
    fn shared_terms_score_higher_than_disjoint() {
        let q = "db connection timeout";
        let related = "connection timeout root cause";
        let unrelated = "disk full errno 28";
        let s = scores(q, &[related, unrelated]);
        assert!(s[0] > s[1], "{} should beat {} for {:?}", s[0], s[1], q);
    }

    #[test]
    fn chargrams_bridge_surface_rewrites() {
        let q = "connection timeout";
        let rewrite = "connect timeout";
        let unrelated = "disk full";
        let s = scores(q, &[rewrite, unrelated]);
        assert!(
            s[0] > s[1],
            "char-gram should bridge rewrite: {} vs {}",
            s[0],
            s[1]
        );
    }

    #[test]
    fn deterministic_across_runs() {
        let docs = ["ERROR db timeout", "disk full errno=28", "连接池耗尽"];
        let q = "db timeout 连接池";
        assert_eq!(scores(q, &docs), scores(q, &docs));
    }

    #[test]
    fn empty_docs_returns_empty() {
        assert!(scores("q", &[]).is_empty());
    }

    #[test]
    fn jaccard_is_unweighted() {
        // 精排无权：a 与 b 仅共享键 1，J = |{1}| / |{1,2,3}| = 1/3（权重不参与）。
        let a: HashMap<u64, f64> = [(1, 100.0), (2, 1.0)].into_iter().collect();
        let b: HashMap<u64, f64> = [(1, 0.5), (3, 3.0)].into_iter().collect();
        let j = jaccard(&a, &b);
        assert!(
            (j - 1.0 / 3.0).abs() < 1e-9,
            "unweighted Jaccard should be 1/3: {j}"
        );
    }

    #[test]
    fn word_token_weight_wins_over_chargram_collision() {
        // "abc" 同时是词元与字符 gram；词元 IDF 权重应优先（不被 1.0 覆盖）。
        let map = ids("abc abcd");
        let key = crate::memory::util::fnv1a("abc");
        assert!(
            map.get(&key).copied().unwrap_or(0.0) > 1.0,
            "word token idf weight must beat chargram weight"
        );
    }

    #[test]
    fn weighted_signature_is_deterministic() {
        let a = ids("ERROR db timeout conn_id=9f2a");
        assert_eq!(weighted_signature(&a), weighted_signature(&a));
        assert_eq!(weighted_signature(&a).len(), N_HASHES);
    }

    #[test]
    fn lsh_candidates_include_identical_doc() {
        let a = ids("ERROR db timeout conn_id=9f2a errno=28");
        let b = ids("ERROR db timeout conn_id=9f2a errno=28");
        let c = ids("zzzz yyyy xxxx");
        let sa = weighted_signature(&a);
        let cand = candidates(&sa, &[weighted_signature(&b), weighted_signature(&c)]);
        assert!(
            cand.contains(&0),
            "identical doc must share a band with the query"
        );
    }

    #[test]
    fn band_key_is_deterministic() {
        let set = ids("some text here");
        let sig = weighted_signature(&set);
        assert_eq!(band_key(0, &sig), band_key(0, &sig));
    }
}
