//! MMR（最大边际相关）多样性去冗余（§3.8）。
//!
//! 贪心选取：下一候选 = argmax( score_norm − λ · max( sim(c, 已选) ) )。
//! 文档相似度用词袋 Jaccard（共享 token / 并集），零依赖、确定性、可解释；
//! λ 从 meta.toml [params] mmr_lambda 读取，默认 0.5。
//!
//! 输入：候选 doc 下标 + 词表 + 归一化得分 + 目标 k + λ。
//! 输出：选中下标（保持分数相对序，去冗余）。

/// 贪心 MMR 选取：返回选中下标（顺序 = 分数优先下的多样性序）。
/// `cands`：候选下标（应已按 score 降序）；`docs`：全部 doc 词表；
/// `scores_norm`：与 docs 对齐的归一化得分 [0,1]；`lambda` ∈ [0,1]。
pub(crate) fn select(
    cands: &[usize],
    docs: &[Vec<String>],
    scores_norm: &[f64],
    k: usize,
    lambda: f64,
) -> Vec<usize> {
    let mut picked: Vec<usize> = Vec::with_capacity(k.min(cands.len()));
    let mut pool: Vec<usize> = cands.to_vec();
    let mut iter = 0;
    while !pool.is_empty() && picked.len() < k && iter <= cands.len() {
        // 下一候选：score − λ·max(与已选相似度)。
        let mut best_i = 0usize;
        let mut best_v = f64::NEG_INFINITY;
        for (pi, &ci) in pool.iter().enumerate() {
            let mut mx = 0.0;
            for &pj in &picked {
                let s = jaccard(&docs[ci], &docs[pj]);
                if s > mx {
                    mx = s;
                }
            }
            let v = scores_norm[ci] - lambda * mx;
            if v > best_v {
                best_v = v;
                best_i = pi;
            }
        }
        picked.push(pool[best_i]);
        pool.remove(best_i);
        iter += 1;
    }
    picked
}

/// 词袋 Jaccard：|A∩B| / |A∪B|（空∪空 = 0）。
fn jaccard(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let (small, big) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut inter = 0usize;
    for t in small {
        if big.iter().any(|u| u == t) {
            inter += 1;
        }
    }
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

    fn terms(ts: &[&str]) -> Vec<String> {
        ts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mmr_removes_redundant_variant() {
        // a/b 高度重叠（同故障变体），c 词面不同。k=2 应选 a + c。
        let docs = vec![
            terms(&["db", "timeout", "conn", "errno", "alpha"]),
            terms(&["db", "timeout", "conn", "errno", "beta"]),
            terms(&["disk", "full", "space", "fatal"]),
        ];
        let cands = vec![0usize, 1, 2];
        // 变体 1 分差不足以抵消 jaccard 扣减（0.7 − 0.5·4/6 ≈ 0.37 < 0.6）→ 被去冗余。
        let scores = vec![1.0, 0.7, 0.6];
        let picked = select(&cands, &docs, &scores, 2, 0.5);
        assert_eq!(picked.len(), 2, "选中 2 个");
        let has_var = picked.contains(&0) && picked.contains(&1);
        assert!(!has_var, "两个同词变体不应同时入选: {picked:?}");
        assert!(picked.contains(&2) || picked.contains(&0), "多样候选应入选");
    }

    #[test]
    fn mmr_lambda_zero_keeps_topk() {
        // λ=0 → 纯分数序，与去冗余无关。
        let docs = vec![
            terms(&["db", "timeout"]),
            terms(&["db", "timeout"]),
            terms(&["db", "timeout"]),
        ];
        let cands = vec![0usize, 1, 2];
        let scores = vec![1.0, 0.9, 0.8];
        let picked = select(&cands, &docs, &scores, 3, 0.0);
        assert_eq!(picked, vec![0, 1, 2]);
    }

    #[test]
    fn mmr_k_larger_than_pool() {
        let docs = vec![terms(&["a", "b"]), terms(&["c", "d"])];
        let cands = vec![1usize, 0];
        let scores = vec![0.8, 0.7];
        let picked = select(&cands, &docs, &scores, 5, 0.5);
        assert_eq!(picked.len(), 2);
    }
}
