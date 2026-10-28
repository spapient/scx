//! 压缩管线·规划阶段（compress/plan）：切行统计、折叠判定、权重打分、
//! 种子与结构区域识别。纯函数、无 I/O；全部 `pub(crate)` 供管线阶段互调。

use super::*;
use crate::analysis::{
    closes, diff, fence, flags, frame, heading, import, lineref, marker, meta, stack,
};
use crate::analysis::{lowercase, normalize};
use crate::policy::Weights;
use crate::Config;

pub(crate) struct Mask {
    pub(crate) mask: Vec<u64>,
    pub(crate) words: usize,
    pub(crate) errs: Vec<bool>,
    pub(crate) marked: Vec<bool>,
    pub(crate) guards: Vec<bool>,
    pub(crate) pinned: Vec<bool>,
    pub(crate) total_tok: f64,
    /// 每行 token 估算（空 = 未计算，调用方回退现算）。
    pub(crate) tok: Vec<f64>,
}

pub(crate) fn tally(lines: &[&str], total: usize, small_tok: f64, do_tok: bool) -> (Vec<f64>, f64) {
    if !do_tok {
        return (Vec::new(), small_tok);
    }
    let mut tok = vec![0.0; total];
    let mut sum = 0.0;
    for (i, line) in lines.iter().enumerate() {
        tok[i] = crate::terms::estimate(line);
        sum += tok[i];
    }
    (tok, sum)
}

pub(crate) fn masks(
    lines: &[&str],
    terms: &[String],
    forces: &[Force],
    small_tok: f64,
    do_tok: bool,
    precise: bool,
) -> Mask {
    let total = lines.len();
    let words = terms.len().div_ceil(64).max(1);
    let mut mask = vec![0u64; total * words];
    let mut errs = vec![false; total];
    let mut marked = vec![false; total];
    let mut guards = vec![false; total];
    let mut pinned = vec![false; total];
    // 每行 token 估算表：供填充与记账复用（此前同一批行会被 estimate 2–3 次）。
    // do_tok=false（小输入/针尖档）时为空表，调用方回退现算。
    let (tok, total_tok) = tally(lines, total, small_tok, do_tok);
    let mut low = String::new();
    for i in 0..total {
        low.clear();
        lowercase(lines[i], &mut low);
        let ll = low.as_str();
        let (err, guard) = flags(ll);
        errs[i] = err;
        guards[i] = if precise { error_line(ll) } else { guard };
        marked[i] = marker(ll);
        pinned[i] = forces.iter().any(|p| match p {
            Force::Sub(s) => ll.contains(s.as_str()),
            Force::Re(n) => n.matches(ll),
        });
        let base = i * words;
        for (k, t) in terms.iter().enumerate() {
            if ll.contains(t.as_str()) {
                mask[base + k / 64] |= 1u64 << (k % 64);
            }
        }
    }
    Mask {
        mask,
        words,
        errs,
        marked,
        guards,
        pinned,
        total_tok,
        tok,
    }
}

pub(crate) fn dedup(
    lines: &[&str],
    mask: &[u64],
    words: usize,
    err: &[bool],
    force: &[bool],
    code_keep: bool,
) -> (Vec<bool>, usize) {
    let total = lines.len();
    let mut seen: hash::Hash<String, usize> =
        hash::Hash::with_capacity_and_hasher(total, Default::default());
    let mut dup = vec![false; total];
    let mut folds = 0usize;
    let mut key = String::new();
    // 与上一行原文相同（日志里连续重复很常见）时，key 里的归一化结果仍然有效，
    // 省掉一次 normalize。字符串相等先比长度再比字节，差异行通常是廉价判断。
    let mut prev: Option<&str> = None;
    // 围栏感知（code_keep 时）：围栏区间内代码行禁折叠。
    let fence = if code_keep {
        fence_mask(lines)
    } else {
        vec![false; total]
    };
    for i in 0..total {
        if prev != Some(lines[i]) {
            key.clear();
            normalize(lines[i], &mut key);
            prev = Some(lines[i]);
        }
        if short(&key) {
            continue;
        }
        match seen.get_mut(key.as_str()) {
            Some(n) => {
                *n += 1;
                let is_dup = foldable(
                    n,
                    hit(mask, words, i),
                    err[i],
                    force[i],
                    code_keep,
                    fence[i],
                );
                dup[i] = is_dup;
                folds += usize::from(is_dup);
            }
            None => {
                seen.insert(key.clone(), 1);
            }
        }
    }
    (dup, folds)
}

/// 围栏掩码（code_keep 时）：``` 区间内的代码行标记，禁折叠。
fn fence_mask(lines: &[&str]) -> Vec<bool> {
    let mut fence = vec![false; lines.len()];
    let mut in_fence = false;
    for i in 0..lines.len() {
        if lines[i].trim_start().starts_with("```") {
            in_fence = !in_fence;
            fence[i] = true;
        } else if in_fence {
            fence[i] = true;
        }
    }
    fence
}

pub(crate) fn short(key: &str) -> bool {
    let key_len = if key.is_ascii() {
        key.len()
    } else {
        key.chars().count()
    };
    key_len < 6
}

pub(crate) fn hit(mask: &[u64], words: usize, i: usize) -> bool {
    (0..words).any(|w| mask[i * words + w] != 0)
}

pub(crate) fn foldable(
    n: &usize,
    hit: bool,
    err: bool,
    force: bool,
    code_keep: bool,
    fence: bool,
) -> bool {
    *n > 2 && (!hit || code_keep) && !err && !force && !fence
}

pub(crate) struct Stat {
    pub(crate) df: Vec<usize>,
    pub(crate) weights: Vec<f64>,
    pub(crate) rare_thr: usize,
}

pub(crate) fn weights(
    mask: &[u64],
    words: usize,
    dup: &[bool],
    n_terms: usize,
    folds: usize,
) -> Stat {
    let total = dup.len();
    let live: usize = total - folds;
    let n_live = live.max(1) as f64;
    let mut df = vec![0usize; n_terms];
    for (i, &is_dup) in dup.iter().enumerate() {
        if is_dup {
            continue;
        }
        let base = i * words;
        for w in 0..words {
            let mut bits = mask[base + w];
            while bits != 0 {
                let k = w * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if k < n_terms {
                    df[k] += 1;
                }
            }
        }
    }
    let weights: Vec<f64> = df
        .iter()
        .map(|&d| (1.0 + n_live / (1.0 + d as f64)).ln())
        .collect();
    let rare_thr = (live / 5).max(1);
    Stat {
        df,
        weights,
        rare_thr,
    }
}

pub(crate) struct Seed {
    pub(crate) seed: Vec<bool>,
    pub(crate) hits: Vec<usize>,
    pub(crate) sumw: Vec<f64>,
    pub(crate) hit_lines: usize,
    pub(crate) force_any: bool,
}

pub(crate) fn marks(
    mask: &[u64],
    words: usize,
    dup: &[bool],
    df: &[usize],
    weights: &[f64],
    rare_thr: usize,
    force: &[bool],
) -> Seed {
    let total = dup.len();
    let mut hits = vec![0usize; total];
    let mut sumw = vec![0.0f64; total];
    let mut seed = vec![false; total];
    let mut hit_lines = 0usize;
    let mut force_any = false;
    for i in 0..total {
        if dup[i] {
            continue;
        }
        let base = i * words;
        let (h, sw, lone) = count(mask, base, words, df.len(), weights);
        let fm = force[i];
        force_any |= fm;
        hits[i] = h;
        sumw[i] = sw;
        seed[i] = rare(h, lone, df, rare_thr, fm);
        if h > 0 {
            hit_lines += 1;
        }
    }
    Seed {
        seed,
        hits,
        sumw,
        hit_lines,
        force_any,
    }
}

pub(crate) fn count(
    mask: &[u64],
    base: usize,
    words: usize,
    df_len: usize,
    weights: &[f64],
) -> (usize, f64, usize) {
    let mut h = 0usize;
    let mut sw = 0.0f64;
    let mut lone = usize::MAX;
    for w in 0..words {
        let mut bits = mask[base + w];
        while bits != 0 {
            let k = w * 64 + bits.trailing_zeros() as usize;
            bits &= bits - 1;
            if k >= df_len {
                continue;
            }
            h += 1;
            sw += weights[k];
            lone = if h == 1 { k } else { usize::MAX };
        }
    }
    (h, sw, lone)
}

pub(crate) struct Region {
    pub(crate) in_region: Vec<bool>,
    /// 仅结构区域（`--structure 1` 时 merge 并入的行）；默认关时全 false。
    pub(crate) struct_region: Vec<bool>,
    pub(crate) count: usize,
    pub(crate) gate: bool,
}

pub(crate) fn expand(
    seed: &[bool],
    window: usize,
    total: usize,
    structs: Option<&Shape>,
) -> Region {
    let mut in_region = vec![false; total];
    let mut struct_region = vec![false; total];
    let seed_idx: Vec<usize> = (0..total).filter(|&i| seed[i]).collect();
    // 结构并入是“叠加”而非“替代”：基础种子邻域（±window + gap 合并）先行，
    // 含种子的结构区域整段并入其上；count 以合并后的 mask 重算。
    let count = if let Some(map) = structs {
        windows(&seed_idx, window, total, &mut in_region);
        merge(seed, window, total, map, &mut in_region, &mut struct_region);
        regions(&in_region)
    } else {
        windows(&seed_idx, window, total, &mut in_region)
    };
    Region {
        in_region,
        struct_region,
        count,
        gate: (0..total).any(|i| seed[i]),
    }
}

pub(crate) fn windows(seed_idx: &[usize], w: usize, total: usize, in_region: &mut [bool]) -> usize {
    let maxgap = (w * crate::policy::GAP_RATIO).max(2);
    if seed_idx.is_empty() {
        return 0;
    }
    let mut count = 0usize;
    let mut a = seed_idx[0].saturating_sub(w);
    let mut b = (seed_idx[0] + w).min(total - 1);
    for &s in &seed_idx[1..] {
        if s <= b + maxgap {
            b = (s + w).min(total - 1);
        } else {
            for slot in in_region.iter_mut().take(b + 1).skip(a) {
                *slot = true;
            }
            count += 1;
            a = s.saturating_sub(w);
            b = (s + w).min(total - 1);
        }
    }
    for slot in in_region.iter_mut().take(b + 1).skip(a) {
        *slot = true;
    }
    count + 1
}

pub(crate) fn merge(
    seed: &[bool],
    window: usize,
    total: usize,
    map: &Shape,
    in_region: &mut [bool],
    struct_region: &mut [bool],
) {
    for r in &map.regions {
        if !(r.start..=r.end).any(|i| seed[i]) {
            continue;
        }
        if r.end - r.start < crate::policy::REGION_DEFAULT.max_span {
            for (slot, sslot) in in_region
                .iter_mut()
                .zip(struct_region.iter_mut())
                .take(r.end + 1)
                .skip(r.start)
            {
                *slot = true;
                *sslot = true;
            }
        } else {
            let half = crate::policy::REGION_DEFAULT.max_span / 2;
            let idx: Vec<usize> = (r.start..=r.end).filter(|&i| seed[i]).collect();
            windows(&idx, half.max(window), total, in_region);
        }
    }
}

pub(crate) fn regions(in_region: &[bool]) -> usize {
    let mut count = 0usize;
    let mut prev = false;
    for &b in in_region {
        if b && !prev {
            count += 1;
        }
        prev = b;
    }
    count
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Import,
    Stack,
}

pub(crate) struct Span {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub(crate) struct Shape {
    pub(crate) regions: Vec<Span>,
}

pub(crate) fn structures(lines: &[&str]) -> Shape {
    let mut regions: Vec<Span> = Vec::new();
    let mut block: Option<usize> = None;
    let mut cur: Option<(usize, Kind)> = None;
    let mut brace = Brace::new();
    for (i, line) in lines.iter().enumerate() {
        if let Some(start) = block {
            if closes(line) {
                regions.push(Span { start, end: i });
                block = None;
                cur = None;
            }
            continue;
        }
        if fence(line) {
            // 进入 fence 前先收尾此前的连续结构段（如 import 块），否则该段
            // 起点会随 cur 一起被丢弃，`--structure 1` 保护不到它。
            close(&mut regions, cur, i.saturating_sub(1));
            block = Some(i);
            cur = None;
            brace.reset();
            continue;
        }
        brace.scan(i, line, &mut regions);
        let kind = kind(line);
        if !continues(cur, kind) {
            close(&mut regions, cur, i.saturating_sub(1));
            cur = kind.map(|k2| (i, k2));
        }
    }
    close(&mut regions, cur, lines.len().saturating_sub(1));
    Shape { regions }
}

pub(crate) struct Brace {
    pub(crate) start: Option<usize>,
    pub(crate) depth: usize,
}

impl Brace {
    fn new() -> Self {
        Brace {
            start: None,
            depth: 0,
        }
    }

    /// 进入 fence 时丢弃未闭合的块（fence 内行不参与大括号判定）。
    pub(crate) fn reset(&mut self) {
        self.start = None;
        self.depth = 0;
    }

    /// 扫描一行的大括号，块闭合时追加 Span。
    pub(crate) fn scan(&mut self, i: usize, line: &str, regions: &mut Vec<Span>) {
        let (opens, closes) = braces(line);
        if self.start.is_none() {
            if opens > 0 {
                self.start = Some(i);
                self.depth = opens.saturating_sub(closes);
                if self.depth == 0 {
                    regions.push(Span { start: i, end: i });
                    self.start = None;
                }
            }
        } else {
            self.depth = self.depth.saturating_add(opens).saturating_sub(closes);
            if self.depth == 0 {
                regions.push(Span {
                    start: self.start.unwrap(),
                    end: i,
                });
                self.start = None;
            }
        }
    }
}

pub(crate) fn braces(line: &str) -> (usize, usize) {
    let mut opens = 0usize;
    let mut closes = 0usize;
    for &b in line.as_bytes() {
        match b {
            b'{' => opens += 1,
            b'}' => closes += 1,
            _ => {}
        }
    }
    (opens, closes)
}

pub(crate) fn continues(cur: Option<(usize, Kind)>, kind: Option<Kind>) -> bool {
    match (cur, kind) {
        (Some((_, k)), Some(k2)) => k == k2,
        _ => false,
    }
}

pub(crate) fn kind(line: &str) -> Option<Kind> {
    if import(line) {
        Some(Kind::Import)
    } else if stack(line) {
        Some(Kind::Stack)
    } else {
        None
    }
}

pub(crate) fn close(regions: &mut Vec<Span>, cur: Option<(usize, Kind)>, end: usize) {
    if let Some((start, _)) = cur {
        if end >= start {
            regions.push(Span { start, end });
        }
    }
}

pub(crate) fn score(
    lines: &[&str],
    hits: &[usize],
    sumw: &[f64],
    err: &[bool],
    force: &[bool],
    i: usize,
    tbl: &Weights,
) -> f64 {
    let total = lines.len();
    let line = lines[i];
    let mut sc = sumw[i] * tbl.sumw_mul;
    sc += state(hits[i], err[i], force[i], i, total, tbl);
    sc += region(line, hits[i], tbl);
    sc
}

pub(crate) fn state(
    hits: usize,
    err: bool,
    force: bool,
    i: usize,
    total: usize,
    tbl: &Weights,
) -> f64 {
    let mut sc = 0.0;
    if hits >= 2 {
        sc += tbl.pair_bonus;
    }
    if err {
        sc += tbl.error_bonus;
    }
    if force {
        sc += tbl.force_bonus;
    }
    if edge(i, total, tbl) {
        sc += tbl.edge_bonus;
    }
    sc
}

pub(crate) fn region(line: &str, hits: usize, tbl: &Weights) -> f64 {
    let mut sc = 0.0;
    if heading(line) {
        sc += tbl.heading_bonus;
    }
    if meta(line) {
        sc += tbl.diffmeta_bonus;
    }
    if patch(line, hits) {
        sc += tbl.diffline_hit_bonus;
    }
    if trace(line) {
        sc += tbl.frame_bonus;
    }
    sc
}

pub(crate) fn patch(line: &str, hits: usize) -> bool {
    diff(line) && hits > 0
}

pub(crate) fn trace(line: &str) -> bool {
    frame(line) || lineref(line)
}

pub(crate) fn edge(i: usize, total: usize, tbl: &Weights) -> bool {
    i < tbl.edge_head || i >= total.saturating_sub(tbl.edge_tail)
}

pub(crate) fn scores(
    lines: &[&str],
    dup: &[bool],
    hits: &[usize],
    sumw: &[f64],
    err: &[bool],
    force: &[bool],
    tbl: &Weights,
) -> Vec<f64> {
    let total = lines.len();
    let mut scores = vec![0.0f64; total];
    for i in 0..total {
        if dup[i] {
            continue;
        }
        scores[i] = score(lines, hits, sumw, err, force, i, tbl);
    }
    scores
}

pub(crate) fn boost(scores: &[f64], dup: &[bool], window: usize, tbl: &Weights) -> Vec<f64> {
    let total = scores.len();
    let mut boost = vec![0.0f64; total];
    for (i, b) in boost.iter_mut().enumerate() {
        let mut m = 0.0f64;
        for d in -(window as i64)..=window as i64 {
            let j = i as i64 + d;
            if j >= 0 && (j as usize) < total && !dup[j as usize] {
                m = m.max(scores[j as usize]);
            }
        }
        *b = (m / tbl.boost_div).min(tbl.boost_cap);
    }
    boost
}

pub(crate) fn apply_sims(cfg: &Config, scores: &mut [f64], dup: &[bool], total: usize) {
    let Some(sims) = &cfg.sims else {
        return;
    };
    if cfg.sim_weight == 0.0 {
        return;
    }
    for i in 0..total {
        if !dup[i] && i < sims.len() {
            // 外部嵌入命令可能输出 NaN/inf（`f64::from_str` 接受 "NaN"）；
            // 一旦进入 scores，排序比较器将不再满足全序，std 会 panic 或乱序。
            if sims[i].is_finite() {
                scores[i] += sims[i] * cfg.sim_weight;
            }
        }
    }
}
