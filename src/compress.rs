//! compress 引擎主体：证据筛选压缩的完整算法。
//!
//! 纯函数、无 I/O、无随机源；本文件不定义 Config/Outcome（在 crate 根），只实现
//! 从文本到压缩结果的单趟管线。管线拆分为独立阶段函数（切行 / 扫描 / 折叠 /
//! 权重 / 种子 / 区域 / 打分 / 邻域 / 落地 / 预算 / 审计 / 守卫 / 渲染），
//! 各阶段函数圈复杂度 < 10；可调参数与加分权重全部声明于 `policy` 表，
//! 不在函数体内硬编码。独立成文件以维持 crate 根 <500 行门禁。

use crate::analysis::{
    closes, diff, fence, flags, frame, heading, import, lineref, marker, meta, stack,
};
use crate::analysis::{lowercase, normalize};
use crate::policy::{Limits, Weights, DEFAULT_RISK, DEFAULT_SCORES};
use crate::terms::terms;
use crate::{prefix, Config, Mode, Outcome, Risk};

/// 确定性快速哈希（FxHash 风格）：压缩内核的查找/计数型 map 统一使用，
/// 避免 std SipHash 的随机种子破坏“同输入同输出”的确定性承诺，同时更快。
/// 使用方均为“先查表计数、按键引用或排序输出”，不依赖裸迭代顺序。
mod hash {
    use std::hash::{BuildHasherDefault, Hasher as Digest};

    /// rustc 内部 FxHasher 的等价实现（乘法散列，零依赖）。
    #[derive(Default)]
    pub(crate) struct Hasher {
        hash: u64,
    }

    const SEED: u64 = 0x517c_c1b7_2722_0a95;

    impl Hasher {
        #[inline]
        fn add(&mut self, word: u64) {
            self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
        }
    }

    impl Digest for Hasher {
        #[inline]
        fn finish(&self) -> u64 {
            self.hash
        }

        #[inline]
        fn write(&mut self, bytes: &[u8]) {
            let mut chunks = bytes.chunks_exact(8);
            for c in &mut chunks {
                self.add(u64::from_ne_bytes(c.try_into().unwrap()));
            }
            let rest = chunks.remainder();
            if !rest.is_empty() {
                let mut last = [0u8; 8];
                last[..rest.len()].copy_from_slice(rest);
                self.add(u64::from_ne_bytes(last));
            }
        }

        #[inline]
        fn write_u8(&mut self, i: u8) {
            self.add(i as u64);
        }

        #[inline]
        fn write_u16(&mut self, i: u16) {
            self.add(i as u64);
        }

        #[inline]
        fn write_u32(&mut self, i: u32) {
            self.add(i as u64);
        }

        #[inline]
        fn write_u64(&mut self, i: u64) {
            self.add(i);
        }

        #[inline]
        fn write_usize(&mut self, i: usize) {
            self.add(i as u64);
        }
    }

    pub(crate) type Build = BuildHasherDefault<Hasher>;
    pub(crate) type Hash<K, V> = std::collections::HashMap<K, V, Build>;
}

/// 字符数用于报告与压缩比；日志常见纯 ASCII，可跳过逐字符迭代。
fn chars(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    }
}

/// 切行：CRLF 归一化到 LF 后按行切分并去掉末尾空行。
/// 含 CR 时归一化结果暂存于 `buf`（由调用者持有），返回行切片借自 `buf`；
/// 无 CR 时零拷贝直接借用输入。
pub(crate) fn lines<'b, 'a: 'b>(text: &'a str, buf: &'b mut Option<String>) -> Vec<&'b str> {
    let src: &str = if text.contains('\r') {
        buf.replace(text.replace("\r\n", "\n"));
        buf.as_ref().unwrap().as_str()
    } else {
        text
    };
    let mut lines: Vec<&str> = src.split('\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// `--force` 条目：默认字面子串，或 `re:` 前缀进入手写有限正则子集
/// （`|` 交替 / 字符类 / `*` `+` `?` 量词 / `\` 转义，见 `crate::regex`）。
/// `re:` 正则先于字面子串匹配；命中即证据。
#[derive(Debug)]
pub(crate) enum Force {
    /// 字面子串（大小写不敏感，对小写缓冲匹配）。
    Sub(String),
    /// 手写正则（已解析并预编译；非法模式不参与匹配，CLI 层解析时已报错）。
    Re(crate::regex::Compiled),
}

/// 解析 `--force`：`re:` 前缀条目整体作为单个正则（`|` 是正则交替，不拆分）；
/// 其余条目按 `|` 拆分为多个字面子串（v0.4.1 语义不变）。trim + 小写，跳过空项。
fn forces(forces: &[String]) -> Vec<Force> {
    let mut pats: Vec<Force> = Vec::new();
    for f in forces {
        let p = f.trim().to_lowercase();
        if p.starts_with("re:") {
            if !p.is_empty() {
                force(&mut pats, p);
            }
        } else {
            for part in f.split('|') {
                let p = part.trim().to_lowercase();
                if !p.is_empty() {
                    force(&mut pats, p);
                }
            }
        }
    }
    pats
}

/// 单个 `--force` 片段：`re:` 正则或字面子串。
fn force(pats: &mut Vec<Force>, p: String) {
    if let Some(rest) = p.strip_prefix("re:") {
        match crate::regex::parse(rest) {
            Ok(node) => pats.push(Force::Re(crate::regex::compile(&node))),
            Err(e) => {
                // CLI（args::parse_force）已前置校验；库内核绕过 CLI
                // 直接构造 Config 时，非法模式不参与匹配（debug 断言提示）。
                debug_assert!(false, "invalid re: force pattern: {e}");
            }
        }
    } else {
        pats.push(Force::Sub(p));
    }
}

/// 逐行 × 逐词位掩码扫描结果：命中掩码、错误/标记/守卫/force 判定与 token 估算。
struct Mask {
    mask_v: Vec<u64>,
    words: usize,
    err_v: Vec<bool>,
    marker_v: Vec<bool>,
    guard_v: Vec<bool>,
    force_v: Vec<bool>,
    total_tok: f64,
}

/// 一次逐行×逐词扫描出命中位掩码；错误/标记/守卫/force 判定复用同一小写缓冲（无逐行分配）。
/// `do_tok` 为真时在循环内累计 token 估算（大输入 + 非针尖档），避免为同一批行再走一趟。
fn masks(lines: &[&str], terms: &[String], forces: &[Force], small_tok: f64, do_tok: bool) -> Mask {
    let total = lines.len();
    let words = terms.len().div_ceil(64).max(1);
    let mut mask_v = vec![0u64; total * words];
    let mut err_v = vec![false; total];
    let mut marker_v = vec![false; total];
    let mut guard_v = vec![false; total];
    let mut force_v = vec![false; total];
    let mut total_tok = if do_tok { 0.0 } else { small_tok };
    let mut low = String::new();
    for i in 0..total {
        if do_tok {
            total_tok += crate::terms::estimate(lines[i]);
        }
        low.clear();
        lowercase(lines[i], &mut low);
        let ll = low.as_str();
        (err_v[i], guard_v[i]) = flags(ll);
        marker_v[i] = marker(ll);
        force_v[i] = forces.iter().any(|p| match p {
            Force::Sub(s) => ll.contains(s.as_str()),
            Force::Re(n) => n.matches(ll),
        });
        let base = i * words;
        for (k, t) in terms.iter().enumerate() {
            if ll.contains(t.as_str()) {
                mask_v[base + k / 64] |= 1u64 << (k % 64);
            }
        }
    }
    Mask {
        mask_v,
        words,
        err_v,
        marker_v,
        guard_v,
        force_v,
        total_tok,
    }
}

/// 去重：归一化后第 3 次起的重复行折叠（除非含查询命中、错误行或 force 命中）。
fn dedup(
    lines: &[&str],
    mask: &[u64],
    words: usize,
    err: &[bool],
    force: &[bool],
) -> (Vec<bool>, usize) {
    let total = lines.len();
    let mut seen_count: hash::Hash<String, usize> =
        hash::Hash::with_capacity_and_hasher(total, Default::default());
    let mut dup = vec![false; total];
    let mut dup_n = 0usize;
    let mut key = String::new();
    let mut run = String::new();
    for i in 0..total {
        key.clear();
        normalize(lines[i], &mut key, &mut run);
        if short(&key) {
            continue;
        }
        if let Some(n) = seen_count.get_mut(key.as_str()) {
            *n += 1;
            if foldable(n, mask, words, i, err, force) {
                dup[i] = true;
                dup_n += 1;
            }
        } else {
            seen_count.insert(key.clone(), 1);
        }
    }
    (dup, dup_n)
}

/// 归一化键是否过短（短键不参与折叠，保持与 v0.4.1 一致）。
fn short(key: &str) -> bool {
    let key_len = if key.is_ascii() {
        key.len()
    } else {
        key.chars().count()
    };
    key_len < 6
}

/// 某行是否命中任意查询词（仅用于重复折叠的豁免判定）。
fn hit(mask: &[u64], words: usize, i: usize) -> bool {
    (0..words).any(|w| mask[i * words + w] != 0)
}

/// 第 3 次起的重复行是否可折叠（无查询命中、无错误、无 force）。
fn foldable(n: &usize, mask: &[u64], words: usize, i: usize, err: &[bool], force: &[bool]) -> bool {
    *n > 2 && !hit(mask, words, i) && !err[i] && !force[i]
}

/// 词频（df）与 IDF 权重：只统计非折叠行。
struct Stat {
    df: Vec<usize>,
    weights: Vec<f64>,
    rare_thr: usize,
}

fn weights(mask: &[u64], words: usize, dup: &[bool], n_terms: usize, dup_n: usize) -> Stat {
    let total = dup.len();
    let live: usize = total - dup_n;
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

/// 逐行证据判定：种子只来自查询证据（≥2 命中或稀有单词命中）与 `--force`。
struct Seed {
    seed: Vec<bool>,
    hits: Vec<usize>,
    sumw: Vec<f64>,
    hit_lines: usize,
    force_any: bool,
}

fn marks(
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

/// 单行的命中计数：词位掩码展开为（命中数、权重和、唯一稀有词）。
fn count(
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

/// 种子区域：±window 扩展，间隔 <= window*3 合并；区域外素材不参与可选预算。
/// `structs` 为结构感知区域（`--structure 1`）：含种子的结构区域整段并入，
/// 超长区域按 `REGION_DEFAULT.max_span` 裁剪为种子邻域窗口。
struct Region {
    in_region: Vec<bool>,
    /// 仅结构区域（`--structure 1` 时 merge 并入的行）；默认关时全 false。
    struct_region: Vec<bool>,
    count: usize,
    gate: bool,
}

fn expand(seed: &[bool], window: usize, total: usize, structs: Option<&Shape>) -> Region {
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

/// 对给定种子索引做 ±w 扩展 + gap 合并，返回区间数（原 expand 主体）。
fn windows(seed_idx: &[usize], w: usize, total: usize, in_region: &mut [bool]) -> usize {
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

/// 结构区域并入：含种子的区域整段进入候选；超长区域裁剪为种子 ± half 窗口。
fn merge(
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

/// 统计 in_region 中连续区段数（结构并入后 count 与 mask 一致）。
fn regions(in_region: &[bool]) -> usize {
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

/// 连续段结构种类（fence 配对独立处理，不进入此枚举）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Import,
    Stack,
}

/// 单个结构区域（含起止行，闭区间）。
struct Span {
    start: usize,
    end: usize,
}

/// 结构区域表（`--structure 1` 时由 `structures` 预扫描生成）。
struct Shape {
    regions: Vec<Span>,
}

/// 单趟状态机：fence 配对（``` / ~~~ 含后缀开启、纯标记闭合）、
/// 连续 import/use/#include 块、连续堆栈链（at 帧 + Caused by: 延续）、
/// 大括号块（函数体/类/impl/try 等）。fence 内行不参与其余判定；
/// 未闭合 fence 与大括号块不并入（保守）。
fn structures(lines: &[&str]) -> Shape {
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

/// 大括号块追踪：从含 `{` 的行开始，到嵌套深度归零的 `}` 结束。
/// 保守实现：不计字符串/注释内的括号；单行 `{ }` 自成一段；未闭合不并入。
struct Brace {
    start: Option<usize>,
    depth: usize,
}

impl Brace {
    fn new() -> Self {
        Brace {
            start: None,
            depth: 0,
        }
    }

    /// 进入 fence 时丢弃未闭合的块（fence 内行不参与大括号判定）。
    fn reset(&mut self) {
        self.start = None;
        self.depth = 0;
    }

    /// 扫描一行的大括号，块闭合时追加 Span。
    fn scan(&mut self, i: usize, line: &str, regions: &mut Vec<Span>) {
        let opens = line.matches('{').count();
        let closes = line.matches('}').count();
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

/// 相邻行是否同属一段（同类结构且段未关闭）。
fn continues(cur: Option<(usize, Kind)>, kind: Option<Kind>) -> bool {
    match (cur, kind) {
        (Some((_, k)), Some(k2)) => k == k2,
        _ => false,
    }
}

/// 单行结构种类：import 块 / 堆栈链行。
fn kind(line: &str) -> Option<Kind> {
    if import(line) {
        Some(Kind::Import)
    } else if stack(line) {
        Some(Kind::Stack)
    } else {
        None
    }
}

/// 收尾当前 import/stack 段（空段忽略）。
fn close(regions: &mut Vec<Span>, cur: Option<(usize, Kind)>, end: usize) {
    if let Some((start, _)) = cur {
        if end >= start {
            regions.push(Span { start, end });
        }
    }
}

/// 单行结构加分：`tbl` 表驱动，判定顺序与 v0.4.1 硬编码 if 链一致（浮点加法顺序不变）。
fn score(
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

/// 状态类加分（命中数 / 错误 / force / 首尾身份行）。
fn state(hits: usize, err: bool, force: bool, i: usize, total: usize, tbl: &Weights) -> f64 {
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

/// 结构类加分（标题 / diff 元信息 / diff 命中 / 堆栈帧）。
fn region(line: &str, hits: usize, tbl: &Weights) -> f64 {
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

/// diff 内容行且命中（结构判定与命中判定合并为单布尔，降低 score 判定点）。
fn patch(line: &str, hits: usize) -> bool {
    diff(line) && hits > 0
}

/// 堆栈帧 / 行引用（score 的结构加分判定）。
fn trace(line: &str) -> bool {
    frame(line) || lineref(line)
}

/// 首 `edge_head` / 尾 `edge_tail` 身份行。
fn edge(i: usize, total: usize, tbl: &Weights) -> bool {
    i < tbl.edge_head || i >= total.saturating_sub(tbl.edge_tail)
}

/// 逐行打分：基础 = IDF 加权命中；结构加分只提升排序价值。
fn scores(
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

/// 邻域权重：±window 范围内的高分给排序加权（不授予独立入选资格）。
/// 合成最终排序分，后续比较只读一次，避免 O(n log n) 次重复浮点加法。
fn boost(scores: &[f64], dup: &[bool], window: usize, tbl: &Weights) -> Vec<f64> {
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

/// 相似度注入（可选）：`--embed-cmd` 预计算的 query×每行相似度叠加到排序分。
/// 只影响重排（boost 之前），不改变种子判定（seeds==evidence 不变量不变）。
fn apply_sims(cfg: &Config, scores: &mut [f64], dup: &[bool], total: usize) {
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

/// 逐行保留原因编码（`--explain` 用）：0=删除，其余见 `explain` 映射。
const R_SEED: u8 = 1;
const R_CONTEXT: u8 = 2;
const R_FILL: u8 = 3;
const R_GUARD: u8 = 4;
const R_ANCHOR: u8 = 5;

/// 先落种子与最小可读邻域；预算只约束其后的可选内容。
/// 两趟：先种子（`Seed`），再邻域（`Context`），保证种子行的原因不被邻域覆盖。
fn floor(seed: &[bool], context: i64, total: usize, reason: &mut [u8]) -> (Vec<bool>, usize) {
    let mut keep = vec![false; total];
    let mut keep_count = 0usize;
    mark_seeds(seed, &mut keep, &mut keep_count, reason);
    mark_context(seed, context, total, &mut keep, &mut keep_count, reason);
    (keep, keep_count)
}

/// 第一趟：种子行标记 `Seed`。
fn mark_seeds(seed: &[bool], keep: &mut [bool], keep_count: &mut usize, reason: &mut [u8]) {
    for (i, &is_seed) in seed.iter().enumerate() {
        if is_seed {
            keep[i] = true;
            *keep_count += 1;
            reason[i] = R_SEED;
        }
    }
}

/// 第二趟：种子 ±context 邻域标记 `Context`（已保留的种子行不覆盖）。
fn mark_context(
    seed: &[bool],
    context: i64,
    total: usize,
    keep: &mut [bool],
    keep_count: &mut usize,
    reason: &mut [u8],
) {
    for (i, &is_seed) in seed.iter().enumerate() {
        if !is_seed {
            continue;
        }
        for d in -context..=context {
            let j = i as i64 + d;
            if d != 0 && j >= 0 && (j as usize) < total && !keep[j as usize] {
                keep[j as usize] = true;
                *keep_count += 1;
                reason[j as usize] = R_CONTEXT;
            }
        }
    }
}

/// 结构锚点检测（P6）：标记结构边界行，默认空 kinds 全 false（现状行为）。
fn anchors(lines: &[&str], kinds: &[crate::AnchorKind]) -> Vec<bool> {
    let json = kinds.contains(&crate::AnchorKind::Json);
    let xml = kinds.contains(&crate::AnchorKind::Xml) || kinds.contains(&crate::AnchorKind::Idoc);
    let yaml = kinds.contains(&crate::AnchorKind::Yaml);
    let mut a = vec![false; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        a[i] = (json && json_anchor(t)) || (xml && tag_anchor(t)) || (yaml && yaml_anchor(t));
    }
    a
}

/// JSON 结构边界：去掉尾随逗号后恰为花括号/方括号，或行尾开括号/行首闭括号。
fn json_anchor(t: &str) -> bool {
    let s = t.trim_end_matches(',');
    matches!(s, "{" | "}" | "[" | "]")
        || s.ends_with('{')
        || s.ends_with('[')
        || s.starts_with('}')
        || s.starts_with(']')
}

/// XML/IDoc 起止标签：整行为单个 `<...>`/`</...>` 标签（非 `<a>text</a>` 叶行，
/// 避免把含内容的叶行也当结构边界整体保留）。
fn tag_anchor(t: &str) -> bool {
    if !t.starts_with('<') || !t.ends_with('>') {
        return false;
    }
    !t[1..t.len() - 1].contains('>')
}

/// YAML 缩进层级键行：映射键（`key:`）或列表项（`- `）；跳过注释/文档标记。
fn yaml_anchor(t: &str) -> bool {
    if t.starts_with('#') || t.starts_with("---") || t.starts_with("...") {
        return false;
    }
    t.ends_with(':') || t.starts_with("- ")
}

/// 把锚点行标记为保留（不覆盖已保留的种子/邻域/守卫）。
fn mark_anchors(anchors: &[bool], keep: &mut [bool], keep_count: &mut usize, reason: &mut [u8]) {
    for (i, &a) in anchors.iter().enumerate() {
        if a && !keep[i] {
            keep[i] = true;
            *keep_count += 1;
            reason[i] = R_ANCHOR;
        }
    }
}

/// 结构锚点标记入口（P6）：空 kinds 时为空操作（现状行为）；否则检测并标记锚点行。
fn mark_structure_anchors(
    cfg: &Config,
    lines: &[&str],
    keep: &mut [bool],
    keep_count: &mut usize,
    reason: &mut [u8],
) {
    if cfg.structure_anchors.is_empty() {
        return;
    }
    let anchor_mask = anchors(lines, &cfg.structure_anchors);
    mark_anchors(&anchor_mask, keep, keep_count, reason);
}

/// 预算填充候选资格：非折叠、未保留，且有真实得分或位于结构区间
/// （`--structure 1` 让「整段并入」名副其实）；种子区域门控（首两行身份行豁免）。
fn eligible(sel: &Select, cfg: &Fill, keep: &[bool], i: usize) -> bool {
    !sel.dup[i]
        && !keep[i]
        && (sel.scores[i] > 0.0 || sel.struct_region[i])
        && (!cfg.region_gate || sel.in_region[i] || i < cfg.edge_head)
}

/// 预算选择：种子区域门控（首两行身份行除外）；独立入选须真实得分（scores > 0）。
/// 预算填充的只读切片上下文（slice 借用归组，避免超长参数表）。
struct Select<'a> {
    lines: &'a [&'a str],
    scores: &'a [f64],
    boost: &'a [f64],
    dup: &'a [bool],
    in_region: &'a [bool],
    struct_region: &'a [bool],
}

/// 预算填充的标量配置。
struct Fill {
    region_gate: bool,
    total_tok: f64,
    ratio: f64,
    edge_head: usize,
    remaining: f64,
}

/// 预算选择：种子区域门控（首两行身份行除外）；独立入选须真实得分（scores > 0）。
/// 排序按 `scores + boost` 降序、行号升序（确定性 tie-break）。
/// `remaining` 为 `--max-tokens` 下可选填充的可分配余量（无上限时 +∞）；
/// 填充总量受 `min(budget, remaining)` 双重约束。
fn fill(
    sel: &Select,
    cfg: &Fill,
    keep: &mut [bool],
    keep_count: &mut usize,
    reason: &mut [u8],
) -> f64 {
    let total = sel.lines.len();
    let budget = (cfg.total_tok * cfg.ratio).max(crate::policy::BUDGET_MIN);
    let cap = budget.min(cfg.remaining);
    let mut order: Vec<(usize, f64)> = (0..total)
        .filter(|&i| eligible(sel, cfg, keep, i))
        .map(|i| (i, sel.scores[i] + sel.boost[i]))
        .collect();
    order.sort_unstable_by(|&(a, sa), &(b, sb)| {
        if sb != sa {
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            a.cmp(&b)
        }
    });
    let mut used = 0.0f64;
    for &(i, _) in &order {
        let t = crate::terms::estimate(sel.lines[i]);
        if used + t > cap {
            continue;
        }
        if !keep[i] {
            keep[i] = true;
            *keep_count += 1;
            reason[i] = R_FILL;
        }
        used += t;
    }
    used
}

/// 已保留行的 token 估算总和（`--max-tokens` 记账口径）。
fn tokens(lines: &[&str], keep: &[bool]) -> f64 {
    let mut sum = 0.0f64;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            sum += crate::terms::estimate(lines[i]);
        }
    }
    sum
}

/// 风险审计：统计被丢弃的失败级错误行与命中行，按 `keep_count` 归一化得出 risk_raw。
fn audit(keep: &[bool], err: &[bool], hits: &[usize], tbl: &Limits) -> Risk {
    let (err_dropped, hit_dropped) = dropped(keep, err, hits);
    let keep_count = keep.iter().filter(|&&k| k).count();
    let risk_raw = (err_dropped + hit_dropped * tbl.hit_drop_w) as f64 / keep_count.max(1) as f64;
    level(risk_raw, tbl)
}

/// 统计未保留行中被丢弃的失败级错误行与命中行。
fn dropped(keep: &[bool], err: &[bool], hits: &[usize]) -> (usize, usize) {
    let mut err_dropped = 0usize;
    let mut hit_dropped = 0usize;
    for i in 0..keep.len() {
        if keep[i] {
            continue;
        }
        if err[i] {
            err_dropped += 1;
        }
        if hits[i] > 0 {
            hit_dropped += 1;
        }
    }
    (err_dropped, hit_dropped)
}

/// 归一化风险 → 风险档。
fn level(raw: f64, tbl: &Limits) -> Risk {
    if raw >= tbl.high {
        Risk::High
    } else if raw >= tbl.med {
        Risk::Medium
    } else {
        Risk::Low
    }
}

/// 风险守卫：HIGH 时保护失败级错误行与事故标记行（warn/警告 级除外）。
fn guard(
    keep: &mut [bool],
    keep_count: &mut usize,
    dup: &[bool],
    guard: &[bool],
    marker: &[bool],
    region: Option<&[bool]>,
    reason: &mut [u8],
) -> u8 {
    let mut guarded = 0u8;
    for i in 0..keep.len() {
        if keep[i] || dup[i] {
            continue;
        }
        // 区域门控：只保护证据邻域内的错误/标记行（根因聚集在证据附近），
        // 丢弃远处无关错误行；None 时为现状全量保护。
        if region.is_some_and(|r| !r[i]) {
            continue;
        }
        if guard[i] || marker[i] {
            keep[i] = true;
            *keep_count += 1;
            reason[i] = R_GUARD;
            guarded = 1;
        }
    }
    guarded
}

/// 信息留存率（0..=1，确定性）。空输入恒 1.0。
/// - 显式证据：逐项校验（行号 ∈ 保留行号；子串 ∈ 输出文本）后取保留比例。
/// - 无证据：行保留率 kept/total（默认轻量代理，行级近似）。
fn ev_retention(
    evidence: &[crate::Evidence],
    numbers: &[usize],
    text: &str,
    kept: usize,
    total: usize,
) -> f64 {
    if total == 0 {
        return 1.0;
    }
    if evidence.is_empty() {
        return kept as f64 / total as f64;
    }
    let mut hit = 0usize;
    for e in evidence {
        let ok = match e {
            crate::Evidence::Line(n) => {
                // kept == total 表示未删任何行（passthrough/fail-open/no-kept），
                // 此时行号 1..=total 全部存活；否则按保留行号集合判定。
                if kept == total {
                    *n >= 1 && *n <= total
                } else {
                    numbers.contains(n)
                }
            }
            crate::Evidence::Substr(s) => text.contains(s.as_str()),
        };
        if ok {
            hit += 1;
        }
    }
    hit as f64 / evidence.len() as f64
}

/// 输出：零克隆直装；省略区间以单行 "..." 标记。保留行原始行号同步收集（i+1）。
fn render(lines: &[&str], keep: &[bool], elide: bool, lineno: bool) -> (String, Vec<usize>) {
    let mut out_text = String::new();
    let mut numbers = Vec::new();
    let mut in_gap = false;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            if in_gap && elide {
                out_text.push_str("...\n");
            }
            in_gap = false;
            if lineno {
                prefix(&mut out_text, i + 1);
            }
            out_text.push_str(lines[i]);
            out_text.push('\n');
            numbers.push(i + 1);
        } else {
            in_gap = true;
        }
    }
    if in_gap && elide {
        out_text.push_str("...\n");
    }
    (out_text, numbers)
}

/// 结构感知邻域表：`--structure 1` 时先扫描结构区域再并入种子邻域。
fn spans(cfg: &Config, seed: &[bool], window: usize, total: usize, lines: &[&str]) -> Region {
    if cfg.structure {
        let map = structures(lines);
        expand(seed, window, total, Some(&map))
    } else {
        expand(seed, window, total, None)
    }
}

/// 语义增强加分（仅 feature=semantic 且显式加载权重时生效）。
#[cfg(feature = "semantic")]
fn semantic(
    cfg: &Config,
    lines: &[&str],
    uniq_terms: &[String],
    total: usize,
    dup: &[bool],
    scores: &mut [f64],
) {
    if let Some(w) = &cfg.semantic {
        for i in 0..total {
            if dup[i] {
                continue;
            }
            scores[i] += w.score(&crate::semantic::feats(lines[i], uniq_terms, w.dim));
        }
    }
}

#[cfg(not(feature = "semantic"))]
fn semantic(
    _cfg: &Config,
    _lines: &[&str],
    _uniq_terms: &[String],
    _total: usize,
    _dup: &[bool],
    _scores: &mut [f64],
) {
}

/// 可选预算填充：`--context 0` 或已进入 overflow 时跳过。返回可选填充实际
/// 消耗的 token 估算（跳过时返回 0），供 `budget_used_pct` 记账。
fn optional(
    sel: &Select,
    cfg: &Fill,
    context: i64,
    overflow: Option<Mode>,
    keep: &mut [bool],
    keep_count: &mut usize,
    reason: &mut [u8],
) -> f64 {
    if context == 0 || overflow.is_some() {
        return 0.0;
    }
    fill(sel, cfg, keep, keep_count, reason)
}

/// 风险守卫触发条件（HIGH 且未关闭 `--risk-gate`）。
fn active(risk: Risk, cfg: &Config) -> bool {
    risk == Risk::High && cfg.gate
}

/// 对 `text` 按 `query` 做证据筛选压缩。纯函数：无文件、环境与进程 I/O。
/// 编排器只做阶段调用与决策合并（McCabe 判定点 < 10）；各阶段逻辑在
/// 独立小函数中（`config` / `shape` / `cap_*` 等）。
pub fn compress(text: &str, query: &str, cfg: &Config) -> Outcome {
    let (ratio, context, window) = config(cfg);
    let kind = cfg.kind;
    // --max-tokens 保守上界：可选填充预算按「用户上限 ÷ 安全系数」计（宁高估勿超预算）；
    // 证据地板/守卫溢出诊断仍按诚实上限（cfg.limit），不翻转模式标签。
    let budget_limit = cfg
        .limit
        .map(|m| m as f64 / crate::terms::BUDGET_SAFETY_FACTOR);
    // 分析使用 LF 归一化文本；passthrough/fail-open 返回输入原文，保持“原样保留”语义。
    let original = text;
    // 零拷贝快路径：无 CR 时直接借用原文本；行以 &str 切片保存，避免整文复制与逐行分配。
    let mut nl_buf: Option<String> = None;
    let lines = lines(original, &mut nl_buf);
    let uniq_terms = terms(query);
    let force_pats = forces(&cfg.forces);
    let total = lines.len();
    // token 估算只服务小输入直通与可选预算；针尖档大盘无需整份扫描。
    // 小输入先算一次即可（随后可能直接 passthrough）；大输入把估算并入
    // 逐行分析循环，避免为同一批行再走一趟缓存。
    let small_tok = survey(&lines, total);
    let chars_a = chars(original);
    let evidence = &cfg.evidence;
    // 输入 token 估算只依赖原文，且 Outcome.tokens_in 与单桶 tokens_in 同值：
    // 算一次共用（CJK 输入上 estimate 是整篇 O(n) 扫描）。
    let tokens_in = crate::terms::estimate(original);

    let mk = |text_owned: String,
              kept: usize,
              chars_b: usize,
              cut: f64,
              dup: usize,
              seeds: usize,
              risk: Risk,
              guarded: u8,
              regions: usize,
              numbers: Vec<usize>,
              mode: Mode,
              limit: Option<usize>,
              tokens: f64,
              dropped: Vec<crate::Dropped>|
     -> Outcome {
        let ev = ev_retention(evidence, &numbers, &text_owned, kept, total);
        let tokens_out = crate::terms::estimate(&text_owned);
        Outcome {
            text: text_owned,
            numbers,
            kept,
            total,
            input: chars_a,
            output: chars_b,
            cut,
            dup,
            seeds,
            evidence: seeds,
            risk,
            guarded,
            regions,
            window,
            terms: uniq_terms.len(),
            forced: force_pats.len(),
            mode,
            limit,
            tokens,
            tokens_in,
            dropped,
            budget_used_pct: 0.0,
            ev_retention: ev,
            buckets: vec![crate::BucketStat {
                kind,
                tokens_in,
                tokens_out,
                kept,
                total,
                mode,
            }],
            explain: Vec::new(),
        }
    };

    // 直通早退：小输入 / 超长行（无查询参与，原样返回）。
    if let Some(mode) = shape(total, small_tok, &lines) {
        return mk(
            original.to_string(),
            total,
            chars_a,
            0.0,
            0,
            0,
            Risk::Low,
            0,
            0,
            Vec::new(),
            mode,
            cfg.limit,
            0.0,
            Vec::new(),
        );
    }

    let do_tok = total >= crate::policy::SMALL_LINES && context != 0;
    let masks = masks(&lines, &uniq_terms, &force_pats, small_tok, do_tok);
    let (dup, dup_n) = dedup(
        &lines,
        &masks.mask_v,
        masks.words,
        &masks.err_v,
        &masks.force_v,
    );
    let stat = weights(&masks.mask_v, masks.words, &dup, uniq_terms.len(), dup_n);
    let seeds = marks(
        &masks.mask_v,
        masks.words,
        &dup,
        &stat.df,
        &stat.weights,
        stat.rare_thr,
        &masks.force_v,
    );
    let seeds_total = (0..total).filter(|&i| seeds.seed[i]).count();

    // 无信号 fail-open：零命中且无 --force 命中 => 原样保留。
    if failopen(seeds.hit_lines, seeds.force_any) {
        return mk(
            original.to_string(),
            total,
            chars_a,
            0.0,
            dup_n,
            seeds_total,
            Risk::Low,
            0,
            0,
            Vec::new(),
            Mode::FailOpenZeroSignal,
            cfg.limit,
            0.0,
            Vec::new(),
        );
    }

    // 结构感知邻域（--structure 1）：fence/import/堆栈区域整段并入种子区域。
    let regions = spans(cfg, &seeds.seed, window as usize, total, &lines);
    #[cfg_attr(not(feature = "semantic"), allow(unused_mut))]
    let mut scores = scores(
        &lines,
        &dup,
        &seeds.hits,
        &seeds.sumw,
        &masks.err_v,
        &masks.force_v,
        &DEFAULT_SCORES,
    );
    // 语义增强（可选）：feature=semantic 且显式加载权重时，对非 dup 候选行
    // 叠加 sigmoid 打分作为排序加分；不启用时与 v0.4.1 逐字节一致。
    semantic(cfg, &lines, &uniq_terms, total, &dup, &mut scores);
    apply_sims(cfg, &mut scores, &dup, total);
    let boost = boost(&scores, &dup, window as usize, &DEFAULT_SCORES);
    // 逐行保留原因（--explain 用）：0=删除，其余见 R_* 常量。
    let mut reason = vec![0u8; total];
    let (mut keep, mut keep_count) = floor(&seeds.seed, context, total, &mut reason);

    // 结构锚点（P6 骨架保护）：结构边界行无条件保留，优先级低于证据（seed/context）、
    // 高于可选填充。默认空 kinds 时为空操作，与现状逐字节一致。
    mark_structure_anchors(cfg, &lines, &mut keep, &mut keep_count, &mut reason);

    // --max-tokens 硬上限：证据地板最高优先。证据 + 最小邻域先落，其 token
    // 估算超限时直接进入 OverflowEvidence（保留全部证据，如实报告），不再
    // 填充可选内容；未超限时可选填充受 `min(budget, max - floor)` 双重约束。
    let tok_floor = tokens(&lines, &keep);
    let mut overflow = cap(tok_floor, cfg.limit);

    // --context 0（针尖模式）：完全跳过可选预算填充，只保留证据行与守卫行。
    let used_optional = optional(
        &Select {
            lines: &lines,
            scores: &scores,
            boost: &boost,
            dup: &dup,
            in_region: &regions.in_region,
            struct_region: &regions.struct_region,
        },
        &Fill {
            region_gate: regions.gate,
            total_tok: masks.total_tok,
            ratio,
            edge_head: DEFAULT_SCORES.edge_head,
            remaining: remaining(tok_floor, budget_limit),
        },
        context,
        overflow,
        &mut keep,
        &mut keep_count,
        &mut reason,
    );
    // 可选预算利用率（%）：实际填充 ÷ 预算上限，展示 `--ratio` 的实际作用。
    // 与 fill 同口径（budget = max(total_tok × ratio, BUDGET_MIN)）。
    let budget = (masks.total_tok * ratio).max(crate::policy::BUDGET_MIN);
    let budget_used_pct = if budget > 0.0 {
        (used_optional / budget * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    let risk = audit(&keep, &masks.err_v, &seeds.hits, &DEFAULT_RISK);
    let mut guarded = 0u8;
    if active(risk, cfg) {
        let guard_region = cfg.guard_region.then_some(regions.in_region.as_slice());
        guarded = guard(
            &mut keep,
            &mut keep_count,
            &dup,
            &masks.guard_v,
            &masks.marker_v,
            guard_region,
            &mut reason,
        );
    }

    // 守卫后最终检查：填充受上限约束，但风险守卫可能追加证据性行；如实报告 OverflowFill。
    let final_tok = tokens(&lines, &keep);
    if overflow.is_none() {
        overflow = last(final_tok, cfg.limit);
    }

    // 预算过紧或只有高频单查询命中时可能选不出任何行；此时回退原文，不返回静默空结果。
    if keep_count == 0 {
        return mk(
            original.to_string(),
            total,
            chars_a,
            0.0,
            dup_n,
            seeds_total,
            risk,
            0,
            regions.count,
            Vec::new(),
            Mode::PassNoKept,
            cfg.limit,
            0.0,
            Vec::new(),
        );
    }

    let (out_text, numbers) = render(&lines, &keep, cfg.elide, cfg.lineno);
    let (chars_b, cut) = cut(&out_text, chars_a);
    let mut o = mk(
        out_text,
        keep_count,
        chars_b,
        cut,
        dup_n,
        seeds_total,
        risk,
        guarded,
        regions.count,
        numbers,
        overflow.unwrap_or(Mode::Sieve),
        cfg.limit,
        final_tok,
        runs(&keep, &dup, overflow.is_some()),
    );
    o.budget_used_pct = budget_used_pct;
    o.explain = explain(&reason, &dup);
    o
}

/// 分桶压缩：把分类化载荷逐桶压缩（各桶按价值梯度 ratio），拼接正文，聚合分桶统计。
/// 全局 `kept/total/tokens/input/output` 为各桶之和；`mode` 恒为 `Sieve`（多桶摘要），
/// 逐桶 mode/kept/total 在 `Outcome.buckets`。默认单桶路径（`compress`）不受影响。
pub fn compress_bucketed(
    parts: &[(crate::ContentType, &str)],
    query: &str,
    cfg: &Config,
) -> Outcome {
    let mut out_text = String::new();
    let mut buckets = Vec::with_capacity(parts.len());
    let (mut tin, mut tout) = (0.0, 0.0);
    let (mut cin, mut cout) = (0usize, 0usize);
    let (mut kept, mut total) = (0usize, 0usize);
    for (kind, text) in parts {
        let mut c = cfg.clone();
        c.ratio = kind.default_ratio();
        let o = compress(text, query, &c);
        let tok_out = crate::terms::estimate(&o.text);
        buckets.push(crate::BucketStat {
            kind: *kind,
            tokens_in: o.tokens_in,
            tokens_out: tok_out,
            kept: o.kept,
            total: o.total,
            mode: o.mode,
        });
        tin += o.tokens_in;
        tout += tok_out;
        cin += o.input;
        cout += o.output;
        kept += o.kept;
        total += o.total;
        out_text.push_str(&o.text);
        if !o.text.is_empty() && !o.text.ends_with('\n') {
            out_text.push('\n');
        }
    }
    let cut = if cin > 0 {
        (cin as f64 - cout as f64) * 100.0 / cin as f64
    } else {
        0.0
    };
    Outcome {
        text: out_text,
        numbers: Vec::new(),
        kept,
        total,
        input: cin,
        output: cout,
        cut,
        dup: 0,
        seeds: 0,
        evidence: 0,
        risk: Risk::Low,
        guarded: 0,
        regions: 0,
        window: cfg.window,
        terms: 0,
        forced: 0,
        mode: Mode::Sieve,
        limit: cfg.limit,
        tokens: tout,
        tokens_in: tin,
        dropped: Vec::new(),
        budget_used_pct: 0.0,
        ev_retention: 1.0,
        buckets,
        explain: Vec::new(),
    }
}

/// 配置规范化（库是公开接口：入口统一钳制，避免 CLI 之外的调用触发溢出或超大循环）。
fn config(cfg: &Config) -> (f64, i64, i64) {
    let ratio = if cfg.ratio.is_nan() {
        crate::policy::DEFAULT_RATIO
    } else {
        cfg.ratio.clamp(crate::policy::MIN_RATIO, 1.0)
    };
    let context = cfg.context.clamp(0, crate::policy::MAX_CONTEXT);
    let window = if cfg.window < crate::policy::MIN_WINDOW {
        crate::policy::MIN_WINDOW
    } else {
        cfg.window.min(crate::policy::MAX_WINDOW)
    };
    (ratio, context, window)
}

/// 小输入 token 估算（仅 total < SMALL_LINES 时计算，其余为 0）。
fn survey(lines: &[&str], total: usize) -> f64 {
    if total < crate::policy::SMALL_LINES {
        lines.iter().map(|s| crate::terms::estimate(s)).sum()
    } else {
        0.0
    }
}

/// 超长行检测：字节与字符双口径（先 O(1) 字节检查，只有超长行才付字符计数成本）。
fn long(lines: &[&str]) -> bool {
    lines.iter().any(|s| {
        s.len() > crate::policy::LONGLINE_BYTES && s.chars().count() > crate::policy::LONGLINE_CHARS
    })
}

/// 直通判定：行数超限 / 小输入（行数与 token 估算双低）/ 超长行，返回对应模式；否则 None。
fn shape(total: usize, small_tok: f64, lines: &[&str]) -> Option<Mode> {
    // long() 是全行扫描，放在最便宜的两个判定之后：行数超限/小输入直接短路，
    // 不必为必然直通的输入再扫一遍全部行。
    if total >= crate::policy::MAX_LINES {
        Some(Mode::PassTooManyLines)
    } else if total < crate::policy::SMALL_LINES && small_tok < crate::policy::SMALL_TOK {
        Some(Mode::PassSmall)
    } else if long(lines) {
        Some(Mode::PassLongline)
    } else {
        None
    }
}

/// 无信号 fail-open 判定（零命中且无 --force 命中）。
fn failopen(signals: usize, force_any: bool) -> bool {
    signals == 0 && !force_any
}

/// 证据种子判定：≥2 命中，或单稀有命中（df <= rare_thr），或 --force 命中。
fn rare(h: usize, lone: usize, df: &[usize], rare_thr: usize, fm: bool) -> bool {
    h >= 2 || (h == 1 && lone != usize::MAX && df[lone] <= rare_thr) || fm
}

/// --max-tokens：证据地板（证据 + 最小邻域）已超上限 → OverflowEvidence。
fn cap(tok_floor: f64, limit: Option<usize>) -> Option<Mode> {
    match limit {
        Some(max) if tok_floor > max as f64 => Some(Mode::OverflowEvidence),
        _ => None,
    }
}

/// --max-tokens：可选填充可分配余量（无上限时为 +∞）。
fn remaining(tok_floor: f64, limit: Option<f64>) -> f64 {
    limit.map(|m| m - tok_floor).unwrap_or(f64::INFINITY)
}

/// --max-tokens：风险守卫追加证据性行后总估算仍超 → OverflowFill。
fn last(final_tok: f64, limit: Option<usize>) -> Option<Mode> {
    match limit {
        Some(max) if final_tok > max as f64 => Some(Mode::OverflowFill),
        _ => None,
    }
}

/// 删除区间：`keep` 中连续未保留段（1-based 闭区间）与原因。
/// `dup[i]` 的行记 `Dedup`（去重折叠）；其余 `overflow` 为真记 `MaxTokens`、
/// 否则记 `Budget`（含 `--structure 1` 超长结构区域裁剪——裁剪行为在 merge
/// 完成，此处统一按未保留段记录）。同段内原因变化时拆分为多区间。
fn runs(keep: &[bool], dup: &[bool], overflow: bool) -> Vec<crate::Dropped> {
    let mut out: Vec<crate::Dropped> = Vec::new();
    let mut start: Option<usize> = None;
    let mut cur = crate::Reason::Budget;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            if let Some(s) = start.take() {
                run(&mut out, s, i, cur);
            }
            continue;
        }
        let reason = drop_reason(dup[i], overflow);
        advance(&mut out, &mut start, &mut cur, i, reason);
    }
    if let Some(s) = start {
        run(&mut out, s, keep.len(), cur);
    }
    out
}

/// 未保留行的删除原因：去重折叠 > 硬上限 > 预算不足。
fn drop_reason(dup: bool, overflow: bool) -> crate::Reason {
    if dup {
        crate::Reason::Dedup
    } else if overflow {
        crate::Reason::MaxTokens
    } else {
        crate::Reason::Budget
    }
}

/// 推进删除区间：无起始则开新段，原因一致则延续，原因变化则收尾旧段再开新段。
fn advance(
    out: &mut Vec<crate::Dropped>,
    start: &mut Option<usize>,
    cur: &mut crate::Reason,
    i: usize,
    reason: crate::Reason,
) {
    match *start {
        None => {
            *start = Some(i);
            *cur = reason;
        }
        Some(s) if reason != *cur => {
            run(out, s, i, *cur);
            *start = Some(i);
            *cur = reason;
        }
        Some(_) => {}
    }
}

/// 追加一段未保留区间（1-based 闭区间）与原因。
fn run(out: &mut Vec<crate::Dropped>, start: usize, end: usize, reason: crate::Reason) {
    out.push(crate::Dropped {
        start: start + 1,
        end,
        reason,
    });
}

/// 逐行决策解释：保留原因来自 `reason` 编码，删除原因按去重折叠 vs 其余区分。
fn explain(reason: &[u8], dup: &[bool]) -> Vec<crate::Explain> {
    reason
        .iter()
        .enumerate()
        .map(|(i, &r)| match r {
            R_SEED => crate::Explain::Seed,
            R_CONTEXT => crate::Explain::Context,
            R_FILL => crate::Explain::Fill,
            R_GUARD => crate::Explain::Guard,
            R_ANCHOR => crate::Explain::Anchor,
            _ => {
                if dup[i] {
                    crate::Explain::Dedup
                } else {
                    crate::Explain::Dropped
                }
            }
        })
        .collect()
}

/// 输出统计：字符数（去掉尾随换行）与压减率。
fn cut(out_text: &str, chars_a: usize) -> (usize, f64) {
    let chars_b = if out_text.ends_with('\n') {
        chars(out_text).saturating_sub(1)
    } else {
        chars(out_text)
    };
    let cut = if chars_a > 0 {
        100.0 * (1.0 - chars_b as f64 / chars_a as f64)
    } else {
        0.0
    };
    (chars_b, cut)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_lines<'a>(v: &[&'a str]) -> Vec<&'a str> {
        v.to_vec()
    }

    #[test]
    fn drop_runs_empty_keep() {
        assert!(runs(&[], &[], false).is_empty());
        assert!(runs(&[true, true, true], &[false; 3], false).is_empty());
    }

    #[test]
    fn drop_runs_budget_runs() {
        let keep = vec![true, false, false, true, false, true];
        let dup = vec![false; 6];
        let dr = runs(&keep, &dup, false);
        assert_eq!(dr.len(), 2);
        assert_eq!((dr[0].start, dr[0].end), (2, 3));
        assert_eq!(dr[0].reason, crate::Reason::Budget);
        assert_eq!((dr[1].start, dr[1].end), (5, 5));
        assert_eq!(dr[1].reason, crate::Reason::Budget);
    }

    #[test]
    fn drop_runs_trailing_tail() {
        let keep = vec![true, false, false];
        let dr = runs(&keep, &[false; 3], false);
        assert_eq!(dr.len(), 1);
        assert_eq!((dr[0].start, dr[0].end), (2, 3));
    }

    #[test]
    fn drop_runs_overflow_reason() {
        let keep = vec![false, false, true];
        let dr = runs(&keep, &[false; 3], true);
        assert_eq!(dr.len(), 1);
        assert_eq!(dr[0].reason, crate::Reason::MaxTokens);
        assert_eq!(dr[0].reason.as_str(), "max-tokens");
        assert_eq!(crate::Reason::Budget.as_str(), "budget");
        assert_eq!(crate::Reason::Dedup.as_str(), "dedup");
    }

    #[test]
    fn drop_runs_dedup_splits_from_budget() {
        // dup 行记 Dedup，与相邻 budget 行拆分为不同区间。
        let keep = vec![true, false, false];
        let dup = vec![false, true, false];
        let dr = runs(&keep, &dup, false);
        assert_eq!(dr.len(), 2);
        assert_eq!((dr[0].start, dr[0].end), (2, 2));
        assert_eq!(dr[0].reason, crate::Reason::Dedup);
        assert_eq!((dr[1].start, dr[1].end), (3, 3));
        assert_eq!(dr[1].reason, crate::Reason::Budget);
        // 同一原因延续不拆分：两个相邻 dup 合并为一个 Dedup 区间。
        let keep2 = vec![true, false, false, false];
        let dup2 = vec![false, true, true, false];
        let dr2 = runs(&keep2, &dup2, false);
        assert_eq!(dr2.len(), 2);
        assert_eq!((dr2[0].start, dr2[0].end), (2, 3));
        assert_eq!(dr2[0].reason, crate::Reason::Dedup);
        assert_eq!((dr2[1].start, dr2[1].end), (4, 4));
        assert_eq!(dr2[1].reason, crate::Reason::Budget);
    }

    #[test]
    fn scan_structures_fence_pairing() {
        let lines = sample_lines(&["```", "import os", "conn 9f2a", "```", "x"]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 1);
        assert_eq!((map.regions[0].start, map.regions[0].end), (0, 3));
    }

    #[test]
    fn scan_structures_import_block() {
        let lines = sample_lines(&["import os", "import sys", "import conn_pool", "x"]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 1);
        assert_eq!((map.regions[0].start, map.regions[0].end), (0, 2));
    }

    #[test]
    fn nan_sims_do_not_poison_scores() {
        // 外部嵌入命令输出 NaN 时不得进入排序分（比较器会失去全序）。
        let text = (0..200)
            .map(|i| format!("line {i}: conn pool timeout db error 9f2a"))
            .collect::<Vec<_>>()
            .join("\n");
        let cfg = crate::Config {
            sims: Some(vec![f64::NAN; 200]),
            ..crate::Config::default()
        };
        let out = compress(&text, "conn timeout", &cfg);
        assert!(out.kept >= 1, "NaN 相似度不应破坏压缩结果");
        assert!(out.text.is_char_boundary(0));
    }

    #[test]
    fn scan_structures_import_block_closed_before_fence() {
        // 进入 fence 前必须先收尾未结束的结构段，否则 import 段整体丢失。
        let lines = sample_lines(&["import os", "import sys", "```", "code", "```", "x"]);
        let map = structures(&lines);
        let mut spans: Vec<(usize, usize)> = map.regions.iter().map(|r| (r.start, r.end)).collect();
        spans.sort();
        assert_eq!(spans, vec![(0, 1), (2, 4)]);
    }

    #[test]
    fn scan_structures_stack_chain_with_caused_by() {
        let lines = sample_lines(&[
            "  at com.a.Pool.acquire(Pool.java:214)",
            "Caused by: java.sql.SQLException: db crash conn 9f2a",
            "  at com.c.Conn.open(Conn.java:9)",
            "x",
        ]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 1);
        assert_eq!((map.regions[0].start, map.regions[0].end), (0, 2));
    }

    #[test]
    fn scan_structures_fence_shields_inner_import() {
        // fence 内行不参与 import/stack 判定。
        let lines = sample_lines(&["```", "import os", "```", "import sys"]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 2, "fence 区域 + fence 外 import 区域");
        let mut spans: Vec<(usize, usize)> = map.regions.iter().map(|r| (r.start, r.end)).collect();
        spans.sort();
        assert_eq!(spans, vec![(0, 2), (3, 3)]);
    }

    #[test]
    fn use_strict_not_import() {
        assert!(!import("use strict"));
        assert!(!import("use of"));
        assert!(import("use crate::policy::DEFAULT_RATIO"));
        assert!(import("#include <stdio.h>"));
        assert!(import("import os"));
    }

    #[test]
    fn scan_structures_brace_block() {
        // 大括号块（函数体 + 嵌套 if）：从含 { 的行到匹配的 } 整段并入。
        let lines = sample_lines(&[
            "fn connect() {",
            "  let conn = pool.acquire();",
            "  if conn.is_err() {",
            "    return Err(conn.err());",
            "  }",
            "  Ok(conn)",
            "}",
            "x",
        ]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 1, "单个函数体块");
        assert_eq!((map.regions[0].start, map.regions[0].end), (0, 6));
    }

    #[test]
    fn scan_structures_single_line_brace_block() {
        // 单行 { } 自成一段。
        let lines = sample_lines(&["fn empty() {}", "x"]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 1);
        assert_eq!((map.regions[0].start, map.regions[0].end), (0, 0));
    }

    #[test]
    fn scan_structures_fence_shields_brace_block() {
        // fence 内的代码块不参与大括号判定（重置 brace）。
        let lines = sample_lines(&[
            "```",
            "fn inside() {",
            "  return 1;",
            "}",
            "```",
            "fn outside() {",
            "  return 2;",
            "}",
        ]);
        let map = structures(&lines);
        // 只有 fence（0..4）和 fence 外的函数体（5..7）。
        assert_eq!(map.regions.len(), 2, "fence 区域 + fence 外函数体");
        let mut spans: Vec<(usize, usize)> = map.regions.iter().map(|r| (r.start, r.end)).collect();
        spans.sort();
        assert_eq!(spans, vec![(0, 4), (5, 7)]);
    }

    #[test]
    fn structure_default_off_region_shape_unchanged() {
        // --structure 默认关：expand 与 v0.4.1 完全一致（区间数与 mask）。
        let seed = vec![false, false, true, false, false, false];
        let a = expand(&seed, 2, 6, None);
        let b = expand(
            &seed,
            2,
            6,
            Some(&Shape {
                regions: Vec::new(),
            }),
        );
        assert_eq!(a.in_region, b.in_region);
        assert_eq!(a.count, b.count);
        assert_eq!(a.gate, b.gate);
    }

    #[test]
    fn structure_fence_merges_region_into_candidate() {
        // 结构并入是叠加：种子邻域基础窗 + 含种子的 fence 区域整段进入候选。
        let lines: Vec<&str> = vec![
            "```",
            "import os",
            "conn 9f2a broken db crash",
            "return pool",
            "```",
            "INFO filler",
        ];
        let seed = vec![false, false, true, false, false, false];
        let _base = expand(&seed, 1, lines.len(), None);
        let map = structures(&lines);
        let merged = expand(&seed, 1, lines.len(), Some(&map));
        assert!(merged.in_region[0], "fence 开启行并入");
        assert!(merged.in_region[4], "fence 闭合行并入");
        assert!(!merged.in_region[5], "fence 外行不并入");
    }

    #[test]
    fn too_many_lines_returns_passthrough() {
        assert_eq!(
            shape(crate::policy::MAX_LINES, 0.0, &[]),
            Some(Mode::PassTooManyLines),
            "行数达到上限应直通"
        );
        assert_eq!(
            shape(crate::policy::MAX_LINES - 1, 0.0, &[]),
            None,
            "上限内不触发行数直通"
        );
    }

    #[test]
    fn json_anchor_detects_braces_and_brackets() {
        assert!(json_anchor("{"));
        assert!(json_anchor("}"));
        assert!(json_anchor("["));
        assert!(json_anchor("]"));
        assert!(json_anchor("},"), "尾随逗号");
        assert!(json_anchor("\"key\": {"), "行尾开括号");
        assert!(!json_anchor("\"key\": 1"));
    }

    #[test]
    fn tag_anchor_detects_xml_tags() {
        assert!(tag_anchor("<root>"));
        assert!(tag_anchor("</root>"));
        assert!(tag_anchor("<tag attr=\"x\">"));
        assert!(!tag_anchor("plain text"));
        assert!(!tag_anchor("<item>data</item>"), "叶行非结构边界");
    }

    #[test]
    fn yaml_anchor_detects_keys_and_list_items() {
        assert!(yaml_anchor("key:"));
        assert!(yaml_anchor("- item"));
        assert!(!yaml_anchor("# comment"));
        assert!(!yaml_anchor("key: value"));
    }

    #[test]
    fn anchors_marks_only_enabled_kinds() {
        let lines = ["{", "<tag>", "key:", "plain"];
        let json_only = anchors(&lines, &[crate::AnchorKind::Json]);
        assert_eq!(json_only, vec![true, false, false, false]);
        let all = anchors(
            &lines,
            &[
                crate::AnchorKind::Json,
                crate::AnchorKind::Xml,
                crate::AnchorKind::Yaml,
            ],
        );
        assert_eq!(all, vec![true, true, true, false]);
    }

    #[test]
    fn guard_region_gates_far_errors() {
        // 全量守卫（None）保留所有错误行；区域门控（Some）只保留证据邻域内的错误行。
        let keep = vec![false; 5];
        let dup = vec![false; 5];
        let guard_lines = vec![true, true, true, true, true];
        let marker = vec![false; 5];
        let in_region = vec![false, false, true, false, false];
        let mut reason = vec![0u8; 5];

        let mut k_full = keep.clone();
        let mut kc = 0;
        guard(
            &mut k_full,
            &mut kc,
            &dup,
            &guard_lines,
            &marker,
            None,
            &mut reason,
        );
        assert_eq!(kc, 5, "全量守卫保留所有错误行");

        let mut k_region = keep.clone();
        let mut kc2 = 0;
        guard(
            &mut k_region,
            &mut kc2,
            &dup,
            &guard_lines,
            &marker,
            Some(&in_region),
            &mut reason,
        );
        assert_eq!(kc2, 1, "区域门控只保留证据邻域内的错误行");
        assert!(k_region[2], "in-region 错误行保留");
    }
}
