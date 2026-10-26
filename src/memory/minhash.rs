//! 纯算法近似语义召回：带权 MinHash 签名 + LSH 分桶 + 精确 Jaccard 精排。
//!
//! 零第三方依赖、零随机源、确定性（固定 splitmix64 种子，无 clock/rand）。
//! shingle 集合 = 词元（`terms::memory`，按 IDF 过滤库级噪声词）+ 字符 3-gram。
//! 「带权」通过 IDF 阈值过滤实现：高区分度术语保留、全库泛化词剔除，避免
//! 浮点加权带来的跨平台一致性风险，且全程整数运算可复现。
//!
//! 128 位签名按 8 段（每段 16 哈希）做 LSH 分桶，候选初筛后按完整 Jaccard
//! 精排；小库（< `LSH_THRESHOLD`）直接全量精确 Jaccard，无假阴性。字符 3-gram
//! 补词元匹配的短板，捕捉代码标识符、缩写与语序重排这类表层改写。

use std::collections::{HashMap, HashSet};

pub(crate) const N_HASHES: usize = 128;
pub(crate) const N_BANDS: usize = 8;
const BAND_ROWS: usize = N_HASHES / N_BANDS;
/// 超过该规模才走 LSH 候选初筛；小库全量精确 Jaccard（无 LSH 假阴性）。
const LSH_THRESHOLD: usize = 4096;
/// shingle 上限：词元已由 `terms` 限 256，字符 3-gram 无上限，需独立封顶防超大正文。
const MAX_SHINGLES: usize = 1 << 16;

/// query 与每个 doc 的 shingle-Jaccard 相似度（词元 + 字符 3-gram，IDF 过滤后）。
pub(crate) fn scores(query: &str, docs: &[&str]) -> Vec<f64> {
    if docs.is_empty() {
        return Vec::new();
    }
    let n = docs.len();
    let df = doc_freq(docs);
    let q = shingle_set(query, &df, n);
    let ds: Vec<HashSet<u64>> = docs.iter().map(|d| shingle_set(d, &df, n)).collect();
    if n < LSH_THRESHOLD {
        return ds.iter().map(|d| jaccard(&q, d)).collect();
    }
    let q_sig = signature(&q);
    let sigs: Vec<[u64; N_HASHES]> = ds.iter().map(signature).collect();
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
        let mut seen = HashSet::new();
        for t in crate::terms::memory(d) {
            if seen.insert(t.clone()) {
                *df.entry(t).or_insert(0) += 1;
            }
        }
    }
    df
}

/// 单文本 shingle 集合：保留词元（IDF 过滤后）+ 字符 3-gram。
fn shingle_set(text: &str, df: &HashMap<String, usize>, n: usize) -> HashSet<u64> {
    let mut set = HashSet::new();
    for t in crate::terms::memory(text) {
        if kept(df, n, &t) {
            set.insert(crate::memory::util::fnv1a(&t));
        }
    }
    chargrams(text, &mut set);
    set
}

/// IDF 过滤：出现在全部 doc 的库级噪声词不参与 shingle（n<2 时库太小，不过滤）。
fn kept(df: &HashMap<String, usize>, n: usize, t: &str) -> bool {
    n < 2 || df.get(t).copied().unwrap_or(0) < n
}

/// 字符 3-gram（字母/数字/下划线连续段，小写化）；跨非词字符自然断开。
fn chargrams(text: &str, set: &mut HashSet<u64>) {
    if set.len() >= MAX_SHINGLES {
        return;
    }
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let mut gram = String::with_capacity(3);
    for w in chars.windows(3) {
        if set.len() >= MAX_SHINGLES {
            break;
        }
        if !w.iter().all(|&c| eligible(c)) {
            continue;
        }
        gram.clear();
        for &c in w {
            gram.push(c);
        }
        set.insert(crate::memory::util::fnv1a(&gram));
    }
}

fn eligible(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 128 哈希 MinHash 签名：每个位置取 `mix(i, shingle)` 的最小值。
fn signature(set: &HashSet<u64>) -> [u64; N_HASHES] {
    let mut sig = [u64::MAX; N_HASHES];
    for &s in set {
        for (i, slot) in sig.iter_mut().enumerate() {
            let h = mix(i as u64, s);
            if h < *slot {
                *slot = h;
            }
        }
    }
    sig
}

/// splitmix64：固定种子确定性哈希族（seed=哈希序号，x=shingle 键）。
fn mix(seed: u64, x: u64) -> u64 {
    let mut z = x.wrapping_add(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 第 `band` 段的桶键：段内 16 个签名值 XOR 后经 `mix` 定型。
fn band_key(band: usize, sig: &[u64; N_HASHES]) -> u64 {
    let mut acc = 0u64;
    for &v in &sig[band * BAND_ROWS..(band + 1) * BAND_ROWS] {
        acc ^= v;
    }
    mix(band as u64, acc)
}

/// LSH 候选初筛：与 query 共享至少一个 band 桶的 doc 下标集合。
fn candidates(q_sig: &[u64; N_HASHES], doc_sigs: &[[u64; N_HASHES]]) -> HashSet<usize> {
    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    for (di, sig) in doc_sigs.iter().enumerate() {
        for b in 0..N_BANDS {
            buckets.entry(band_key(b, sig)).or_default().push(di);
        }
    }
    let mut cand = HashSet::new();
    for b in 0..N_BANDS {
        if let Some(list) = buckets.get(&band_key(b, q_sig)) {
            cand.extend(list.iter().copied());
        }
    }
    cand
}

/// 精确 Jaccard：`|A∩B| / |A∪B|`，空集返回 0。
fn jaccard(a: &HashSet<u64>, b: &HashSet<u64>) -> f64 {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if large.is_empty() {
        return 0.0;
    }
    let inter = small.iter().filter(|s| large.contains(s)).count();
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
        // "connection" 与 "connect" 无词元交集，但共享字符 3-gram "con"/"onn"/"nne"。
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
    fn signature_is_min_over_shingles() {
        let set: HashSet<u64> = [1u64, 2, 3].into_iter().collect();
        let sig = signature(&set);
        for (i, &v) in sig.iter().enumerate() {
            let expect = [1u64, 2, 3]
                .iter()
                .map(|&s| mix(i as u64, s))
                .min()
                .unwrap();
            assert_eq!(v, expect, "signature[{i}] must be the min hash");
        }
    }

    #[test]
    fn lsh_candidates_include_identical_doc() {
        let a = shingle_set("ERROR db timeout conn_id=9f2a errno=28", &HashMap::new(), 1);
        let b = shingle_set("ERROR db timeout conn_id=9f2a errno=28", &HashMap::new(), 1);
        let c = shingle_set("zzzz yyyy xxxx", &HashMap::new(), 1);
        let sa = signature(&a);
        let cand = candidates(&sa, &[signature(&b), signature(&c)]);
        assert!(
            cand.contains(&0),
            "identical doc must share a band with the query"
        );
    }

    #[test]
    fn band_key_is_deterministic() {
        let set = shingle_set("some text here", &HashMap::new(), 1);
        let sig = signature(&set);
        assert_eq!(band_key(0, &sig), band_key(0, &sig));
    }
}
