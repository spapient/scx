//! 证据自适应融合（EAR：Evidence-Adaptive Retention）。
//!
//! ## 为什么需要它
//!
//! 生产检索管线（`recall::pipeline`）只做 BM25 + 字段重排 + PRF，**完全不含
//! MinHash 语义路**。Gauge 实测（150 例，单相关）：
//!
//! | 层 | 生产管线 r@5 / MRR | 纯 MinHash 排序 |
//! | --- | --- | --- |
//! | literal（n=60） | 1.000 / 1.000 | 1.000 / 1.000 |
//! | paraphrase（n=60） | 0.583 / 0.249 | 0.717 / 0.276 |
//! | semantic（n=30） | 1.000 / 0.908 | 1.000 / 0.767 |
//!
//! MinHash 能救 paraphrase（+13.4pp r@5），但直接接管排序会把 semantic 的
//! MRR 从 0.908 打到 0.767 —— 序列型查询的收益来自管线重排（字段/实体/
//! 同义词/L1 直答），不是 MinHash。
//!
//! ## 关键设计：用「归一化分数优势」而不是「秩」
//!
//! 实测发现 paraphrase 与 semantic 的 MinHash 峰差分布几乎重合
//! （para p50 = 0.106 vs 0.112），**没有任何阈值能把两层分开**。所以这里
//! 不按查询类别切分，而是让两路的**相对优势**自己决定谁主导：
//!
//! ```text
//! s(i) = (1-w) · score(i)/score(top1)  +  w · mh(i)/mh(top1)
//! ```
//!
//! - semantic：rerank 后的 `score` 有明确领先项（归一化后接近 1，其余掉得
//!   快），MinHash 项再高也只是并列，排序不被推翻；
//! - paraphrase：`score` 平坦（0.2~0.3 挤在一起），MinHash 的相对差更大，
//!   正确候选因此浮上来。
//!
//! `w` 由 `fuse_adapt_min_peak` 控制的 MinHash 峰差门槛决定是否启用，
//! `fuse_adapt_max_w` 封顶（防 MinHash 在长尾噪声上翻盘）。
//!
//! 靠 MinHash 救回、不在 base 候选内的 case，`score`/`sim` 记 0 —— 不触发
//! L1 直答（与 `search_fused` 同规约）。

use super::minhash;
use super::recall::Hit;
use super::store::{self, Meta};
use std::collections::HashMap;

/// 单次融合从 MinHash 侧引入的候选数上限（相对 k 的倍数）。
const SIDE_FACTOR: usize = 2;

/// 融合候选：id、融合分、BM25 来源分数（`None` = 仅由 MinHash 救回）。
type Cand<'a> = (&'a str, f64, Option<(f64, f64)>);

/// 救回候选的可见性：只能是 active，且与锚点同桶（否则会越过桶过滤与
/// 生命周期过滤，把 retired/revised 或别场景的 case 拉回结果）。
fn visible(case: &store::Case, anchor: Option<&str>) -> bool {
    match anchor {
        Some(b) => case.life == "active" && case.bucket == b,
        None => case.life == "active",
    }
}

pub(crate) fn fuse(
    root: &str,
    query: &str,
    base: &[Hit],
    k: usize,
    meta: &Meta,
) -> std::io::Result<Option<Vec<Hit>>> {
    if base.is_empty() || k == 0 {
        return Ok(None);
    }
    // 字面已高置信：BM25 顶分本来就把目标放稳了，融合只会引入噪声。
    if base[0].sim >= meta.param("confidence_high_sim") {
        return Ok(None);
    }
    let ids = store::list(root)?;
    if ids.is_empty() {
        return Ok(None);
    }
    // 规模守卫：融合每查询要读全库正文并算 MinHash，库一大（10k 级）单次
    // recall 会从亚毫秒退化到秒级（`bench memory` 的 5ms 门禁）。超过上限
    // 直接不融合 —— 保留小库（真实记忆库规模）的收益，不为大库付代价。
    let max_docs = meta.param("fuse_adapt_max_docs");
    if max_docs > 0.0 && ids.len() as f64 > max_docs {
        return Ok(None);
    }
    let cases: Vec<store::Case> = ids
        .iter()
        .map(|id| store::load(root, id))
        .collect::<std::io::Result<_>>()?;
    let texts: Vec<String> = cases.iter().map(|c| c.text.clone()).collect();
    let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let mh = minhash::scores(query, &refs);
    if mh.is_empty() {
        return Ok(None);
    }

    let mh_top = mh.iter().copied().fold(f64::MIN, f64::max);
    let mh_med = median(&mh);
    // 峰的突出程度：没有明显近重复时 MinHash 只是均匀背景，权重应归零。
    let para = (mh_top - mh_med).max(0.0);
    if std::env::var("SCX_EAR_DEBUG").is_ok() {
        eprintln!(
            "EAR: q={:.40} mh_top={mh_top:.3} med={mh_med:.3} para={para:.3} lex={:.3}",
            query, base[0].sim
        );
    }
    if para < meta.param("fuse_adapt_min_peak") {
        return Ok(None);
    }
    let w_mh = meta.param("fuse_adapt_max_w").clamp(0.0, 1.0);
    if w_mh <= 0.0 {
        return Ok(None);
    }

    // 头部锁定：base 前 N 位由生产管线（rerank/字段/L1 直答）判定，融合不动。
    let lock = (meta.param("fuse_adapt_lock").max(0.0) as usize).clamp(0, base.len().min(k));
    let head: Vec<Hit> = base[..lock].to_vec();

    let base_top = base[0].score.abs().max(1e-12);
    let mh_max = mh_top.max(1e-12);
    let idx_of: HashMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let mh_norm = |id: &str| idx_of.get(id).map_or(0.0, |&i| mh[i] / mh_max);

    // MinHash 全序（同分按 id 升序，确定性），用于补齐候选。
    let mut order: Vec<usize> = (0..ids.len()).collect();
    order.sort_by(|&a, &b| {
        mh[b]
            .partial_cmp(&mh[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ids[a].cmp(&ids[b]))
    });
    let side = k.saturating_mul(SIDE_FACTOR).min(ids.len());

    let base_win: Vec<&Hit> = base.iter().skip(lock).take(k - lock).collect();
    // 可见性锚点：救回候选必须与 base[0] 同场景桶且未 retired，融合不得
    // 越过桶过滤/生命周期过滤（回归由 `bucket_filter_reduces_scan` 与
    // `retire_tombstones_and_filters_recall` 覆盖）。
    let anchor_bucket = cases
        .iter()
        .find(|c| c.id.as_str() == base[0].id.as_str())
        .map(|c| c.bucket.clone());
    let mut cand: Vec<Cand<'_>> = Vec::with_capacity(base_win.len() + side);
    for h in &base_win {
        let b = h.score.abs() / base_top;
        cand.push((
            h.id.as_str(),
            (1.0 - w_mh) * b + w_mh * mh_norm(&h.id),
            Some((h.score, h.sim)),
        ));
    }
    for &i in order.iter().take(side) {
        let id = ids[i].as_str();
        if base[..lock].iter().any(|h| h.id == id) || base_win.iter().any(|h| h.id == id) {
            continue;
        }
        if !visible(&cases[i], anchor_bucket.as_deref()) {
            continue;
        }
        cand.push((id, w_mh * mh_norm(id), None));
    }

    cand.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(b.0))
    });
    cand.truncate(k.saturating_sub(head.len()));

    let mut out = head;
    out.extend(cand.into_iter().map(|(id, _, pair)| {
        let (score, sim) = pair.unwrap_or((0.0, 0.0));
        Hit {
            id: id.to_string(),
            score,
            sim,
        }
    }));
    Ok(Some(out))
}

/// 中位数（用于 MinHash 背景基线；n 为库内 case 数，开销可忽略）。
fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s: Vec<f64> = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = s.len() / 2;
    if s.len() % 2 == 0 {
        (s[mid - 1] + s[mid]) / 2.0
    } else {
        s[mid]
    }
}
