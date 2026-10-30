//! 压缩管线·规划阶段（compress/plan）：切行统计、折叠判定、权重打分、
//! 种子与结构区域识别。纯函数、无 I/O；全部 `pub(crate)` 供管线阶段互调。

use super::*;
use crate::analysis::lowercase;
use crate::analysis::{
    closes, diff, fence, flags, frame, heading, import, lineref, marker, meta, stack,
};
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

/// 并行版 tally：分块估 token 与求和（estimate 逐行独立），供大输入复用。
fn tally_par(
    lines: &[&str],
    total: usize,
    small_tok: f64,
    do_tok: bool,
    n_t: usize,
    chunk: usize,
) -> (Vec<f64>, f64) {
    if !do_tok {
        return (Vec::new(), small_tok);
    }
    let mut tok = vec![0.0; total];
    let mut parts = split_parts(&mut tok, chunk);
    let sums: Vec<f64> = std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(n_t);
        for (k, part) in parts.drain(..).enumerate() {
            let base = k * chunk;
            handles.push(s.spawn(move || {
                let mut sum = 0.0;
                for (j, slot) in part.iter_mut().enumerate() {
                    *slot = crate::terms::estimate(lines[base + j]);
                    sum += *slot;
                }
                sum
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    (tok, sums.iter().sum())
}

/// 并行分块阈值：低于此行数走串行（避免线程启动开销）。
const PAR_MIN: usize = 20_000;
/// 供其它阶段（如 render trim 预计算）复用相同阈值。
pub(crate) const PAR_MIN_REF: usize = PAR_MIN;

/// 把 `v` 切成多个不重叠的可变切片（每片 ≤ `size` 元素）。
/// 借用链独立：前一借用的切出即终结，可全部收集后 move 分派。
fn split_parts<T>(v: &mut [T], size: usize) -> Vec<&mut [T]> {
    let mut rest = v;
    let mut out = Vec::new();
    while !rest.is_empty() {
        let take = size.min(rest.len());
        let (a, b) = rest.split_at_mut(take);
        out.push(a);
        rest = b;
    }
    out
}

/// 逐行计算共享入参：词表、强制模式、掩码字宽与严格旗标。
struct RowCtx<'a> {
    terms: &'a [String],
    forces: &'a [Force],
    words: usize,
    precise: bool,
}

/// 逐行输出槽：掩码行、错误/标记/守卫/固定行标记。
struct RowOut<'a> {
    mask: &'a mut [u64],
    errs: &'a mut [bool],
    marked: &'a mut [bool],
    guards: &'a mut [bool],
    pinned: &'a mut [bool],
}

/// 逐行计算一行的小写掩码与标记（掩码、错误/守卫/标记/固定行）。
/// 每行完全独立（无共享可变状态），供 `masks` 并行分块复用。
fn mask_row(i: usize, line: &str, ctx: &RowCtx<'_>, low: &mut String, out: &mut RowOut<'_>) {
    low.clear();
    lowercase(line, low);
    let ll = low.as_str();
    let (err, guard) = flags(ll);
    out.errs[i] = err;
    out.guards[i] = if ctx.precise { error_line(ll) } else { guard };
    out.marked[i] = marker(ll);
    out.pinned[i] = ctx.forces.iter().any(|p| match p {
        Force::Sub(s) => ll.contains(s.as_str()),
        Force::Re(n) => n.matches(ll),
    });
    let base = i * ctx.words;
    for (k, t) in ctx.terms.iter().enumerate() {
        if ll.contains(t.as_str()) {
            out.mask[base + k / 64] |= 1u64 << (k % 64);
        }
    }
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
    // 并行分块：行间逐行独立（含 tally），分块写不重叠区间，输出保序零漂移。
    // 线程数按 18 逻辑核折半，避免能效核与线程切换损耗。
    let n_t = if total >= PAR_MIN { 6 } else { 1 };
    let chunk = total.div_ceil(n_t);
    let (tok, total_tok) = if n_t > 1 {
        tally_par(lines, total, small_tok, do_tok, n_t, chunk)
    } else {
        tally(lines, total, small_tok, do_tok)
    };
    if n_t == 1 {
        let mut low = String::new();
        let ctx = RowCtx {
            terms,
            forces,
            words,
            precise,
        };
        let mut out = RowOut {
            mask: &mut mask,
            errs: &mut errs,
            marked: &mut marked,
            guards: &mut guards,
            pinned: &mut pinned,
        };
        for (i, &line) in lines.iter().enumerate() {
            mask_row(i, line, &ctx, &mut low, &mut out);
        }
    } else {
        // 一次性切出不重叠块（借用链独立），move 进各线程写各自区间。
        let mask_parts = split_parts(&mut mask, chunk * words);
        let errs_parts = split_parts(&mut errs, chunk);
        let marked_parts = split_parts(&mut marked, chunk);
        let guards_parts = split_parts(&mut guards, chunk);
        let pinned_parts = split_parts(&mut pinned, chunk);
        let ctx = RowCtx {
            terms,
            forces,
            words,
            precise,
        };
        std::thread::scope(|s| {
            let n_p = mask_parts.len().min(errs_parts.len());
            let mut iter_m = mask_parts.into_iter();
            let mut iter_e = errs_parts.into_iter();
            let mut iter_mk = marked_parts.into_iter();
            let mut iter_g = guards_parts.into_iter();
            let mut iter_p = pinned_parts.into_iter();
            for t in 0..n_p {
                let (Some(m), Some(e), Some(mk), Some(g), Some(p)) = (
                    iter_m.next(),
                    iter_e.next(),
                    iter_mk.next(),
                    iter_g.next(),
                    iter_p.next(),
                ) else {
                    break;
                };
                // m 为整块 mask（len=rows*words），mask_row 用块内偏移 j 索引。
                let rows = e.len();
                let ctx_ref = &ctx;
                let mut out_ref = RowOut {
                    mask: m,
                    errs: e,
                    marked: mk,
                    guards: g,
                    pinned: p,
                };
                s.spawn(move || {
                    let mut low = String::new();
                    for j in 0..rows {
                        mask_row(j, lines[t * chunk + j], ctx_ref, &mut low, &mut out_ref);
                    }
                });
            }
        });
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

// ---------------------------------------------------------------------------
// 去重键：单趟归一化指纹（P14）。
//
// 旧实现为每行 `normalize` 物化完整归一化 String 作 HashMap 键，并在唯一行
// 插入时 `clone`（P12 后 p30m 实测 dedup 占 44-52%，为第一热点）。本实现改为
// 边扫描原文边滚动更新两条独立 64 位哈希（合计 128 位）+ 归一化长度，
// 不物化字符串，键为 `(h1, h2, len)`。折叠语义与 `analysis::normalize`
// 逐字节一致（`norm_hash_matches_normalize_fingerprint` 差分测试守门）；
// 128 位指纹 + 长度下不同归一化串碰撞概率 ~2^-128，确定性不受影响。
// ---------------------------------------------------------------------------

/// 指纹通道 A 种子（与 `compress::hash::Hasher` 同源），通道 B 独立种子。
const FINGER_A: u64 = 0x517c_c1b7_2722_0a95;
const FINGER_B: u64 = 0x9e37_79b9_7f4a_7c15;

/// 滚动双通道指纹：字节级复刻 `hash::Hasher::write` 的 8 字节分块语义。
#[derive(Clone, Copy)]
struct Finger {
    a: u64,
    b: u64,
    buf: [u8; 8],
    n: usize,
}

impl Finger {
    fn new() -> Self {
        Finger {
            a: 0,
            b: 0,
            buf: [0; 8],
            n: 0,
        }
    }
    #[inline]
    fn push(&mut self, byte: u8) {
        self.buf[self.n] = byte;
        self.n += 1;
        if self.n == 8 {
            self.flush();
        }
    }
    #[inline]
    fn flush(&mut self) {
        let w = u64::from_ne_bytes(self.buf);
        self.a = (self.a.rotate_left(5) ^ w).wrapping_mul(FINGER_A);
        self.b = (self.b.rotate_left(5) ^ w).wrapping_mul(FINGER_B);
        self.n = 0;
    }
    /// 收尾（含不足 8 字节的剩余，与 `Hasher::write` 补齐语义一致）。
    fn finish(mut self) -> (u64, u64) {
        if self.n > 0 {
            let mut last = [0u8; 8];
            last[..self.n].copy_from_slice(&self.buf[..self.n]);
            let w = u64::from_ne_bytes(last);
            self.a = (self.a.rotate_left(5) ^ w).wrapping_mul(FINGER_A);
            self.b = (self.b.rotate_left(5) ^ w).wrapping_mul(FINGER_B);
        }
        (self.a, self.b)
    }
}

/// 单趟归一化指纹：`(h1, h2, 归一化长度)`。
/// 归一化长度口径与 `short` 一致：ASCII 输入按字节、非 ASCII 输入按字符。
fn norm_hash(s: &str) -> (u64, u64, u32) {
    if s.is_ascii() {
        norm_hash_ascii(s)
    } else {
        norm_hash_unicode(s)
    }
}

/// ASCII 快路径：与 `analysis::normalize::ascii` 的折叠语义逐字节一致。
fn norm_hash_ascii(s: &str) -> (u64, u64, u32) {
    let bytes = s.as_bytes();
    let mut f = Finger::new();
    let mut n_out = 0u32;
    let mut pending = false;
    let mut has = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_hexdigit() {
            let end = hex_run_end(bytes, i);
            emit_space(&mut pending, &mut has, &mut f, &mut n_out);
            hex_run(&bytes[i..end], &mut f, &mut n_out);
            has = true;
            i = end;
        } else if blank_byte(c) {
            i = blank_run_end(bytes, i);
            pending = true;
        } else {
            emit_space(&mut pending, &mut has, &mut f, &mut n_out);
            f.push(c);
            n_out += 1;
            has = true;
            i += 1;
        }
    }
    let (a, b) = f.finish();
    (a, b, n_out)
}

/// 连续空白 run 终点（与 `analysis::normalize::ascii` 的跳过语义一致）。
fn blank_run_end(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && blank_byte(bytes[i]) {
        i += 1;
    }
    i
}

/// 连续十六进制 run 终点（与 `analysis::normalize::ascii` 一致）。
fn hex_run_end(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
        i += 1;
    }
    i
}

/// 待写空格：`pending && 已有内容` 时写一个空格，随后复位（与 `normalize::space` 一致）。
fn emit_space(pending: &mut bool, has: &mut bool, f: &mut Finger, n: &mut u32) {
    if *pending && *has {
        f.push(b' ');
        *n += 1;
    }
    *pending = false;
}

/// 十六进制 run 折叠（ASCII 字节版本；与 `analysis::normalize::bytes` 逐字节一致）。
fn hex_run(run: &[u8], f: &mut Finger, n: &mut u32) {
    if run.len() >= 8 && run.iter().any(|b| b.is_ascii_digit()) {
        f.push(b'#');
        *n += 1;
        return;
    }
    let mut j = 0usize;
    while j < run.len() {
        if run[j].is_ascii_digit() {
            while j < run.len() && run[j].is_ascii_digit() {
                j += 1;
            }
            f.push(b'#');
            *n += 1;
        } else {
            f.push(run[j]);
            *n += 1;
            j += 1;
        }
    }
}

/// 与 `analysis::normalize::blank` 一致（含垂直制表 U+000B）。
fn blank_byte(b: u8) -> bool {
    b == b'\x0b' || b.is_ascii_whitespace()
}

/// Unicode 路径：与 `analysis::normalize::unicode` 的折叠语义逐字符一致。
fn norm_hash_unicode(s: &str) -> (u64, u64, u32) {
    let mut f = Finger::new();
    let mut n_out = 0u32;
    let mut pending = false;
    let mut has = false;
    let mut it = s.char_indices().peekable();
    while let Some((start, c)) = it.next() {
        if c.is_ascii_hexdigit() {
            let end = hex_end(&mut it, start, c);
            emit_space(&mut pending, &mut has, &mut f, &mut n_out);
            emit_hex_run(&s[start..end], &mut f, &mut n_out);
            has = true;
        } else if c.is_ascii_digit() {
            skip_digits(&mut it);
            emit_space(&mut pending, &mut has, &mut f, &mut n_out);
            f.push(b'#');
            n_out += 1;
            has = true;
        } else if c.is_whitespace() {
            skip_ws(&mut it);
            pending = true;
        } else {
            emit_space(&mut pending, &mut has, &mut f, &mut n_out);
            let end = plain_end(&mut it, start, c);
            emit_plain(&s[start..end], &mut f, &mut n_out);
            has = true;
        }
    }
    let (a, b) = f.finish();
    (a, b, n_out)
}

/// 十六进制 run：含数字 → `fold_run`，否则整段原样入指纹。
fn emit_hex_run(run: &str, f: &mut Finger, n: &mut u32) {
    if run.bytes().any(|b| b.is_ascii_digit()) {
        fold_run(run, f, n);
    } else {
        for &b in run.as_bytes() {
            f.push(b);
            *n += 1;
        }
    }
}

/// 普通字符 run 原样入指纹（长度按字符计数，与 `short` 的非 ASCII 口径一致）。
fn emit_plain(run: &str, f: &mut Finger, n: &mut u32) {
    for ch in run.chars() {
        let mut b4 = [0u8; 4];
        let bs = ch.encode_utf8(&mut b4);
        for &b in bs.as_bytes() {
            f.push(b);
        }
        *n += 1;
    }
}

/// 跳过连续数字字符（与 `analysis::normalize::skip` 一致）。
fn skip_digits(it: &mut Cursor) {
    while it.peek().is_some_and(|&(_, n)| n.is_ascii_digit()) {
        it.next();
    }
}

/// 跳过连续空白字符。
fn skip_ws(it: &mut Cursor) {
    while it.peek().is_some_and(|&(_, n)| n.is_whitespace()) {
        it.next();
    }
}

/// 连续十六进制 run 的终点（首字符已消费；与 `analysis::normalize::hex` 一致）。
fn hex_end(it: &mut Cursor, start: usize, first: char) -> usize {
    let mut end = start + first.len_utf8();
    while let Some(&(j, n)) = it.peek() {
        if !n.is_ascii_hexdigit() {
            break;
        }
        end = j + n.len_utf8();
        it.next();
    }
    end
}

/// 普通字符 run 的终点（非十六进制/空白；与 `analysis::normalize::plain` 一致）。
fn plain_end(it: &mut Cursor, start: usize, first: char) -> usize {
    let mut end = start + first.len_utf8();
    while let Some(&(j, d)) = it.peek() {
        if d.is_ascii_hexdigit() || d.is_whitespace() {
            break;
        }
        end = j + d.len_utf8();
        it.next();
    }
    end
}

/// 十六进制 run 折叠：>=8 位且含数字 → `#`；否则数字段 → `#`、其余保留。
/// 与 `analysis::normalize::fold` 逐字节一致。
fn fold_run(run: &str, f: &mut Finger, n_out: &mut u32) {
    if run.len() >= 8 && run.bytes().any(|b| b.is_ascii_digit()) {
        f.push(b'#');
        *n_out += 1;
        return;
    }
    let mut it = run.chars().peekable();
    while let Some(rc) = it.next() {
        if rc.is_ascii_digit() {
            while it.peek().is_some_and(|&n| n.is_ascii_digit()) {
                it.next();
            }
            f.push(b'#');
            *n_out += 1;
        } else {
            let mut b4 = [0u8; 4];
            let bs = rc.encode_utf8(&mut b4);
            for &b in bs.as_bytes() {
                f.push(b);
                *n_out += 1;
            }
        }
    }
}

/// 归一化扫描游标（与 `analysis::normalize` 同型）。
type Cursor<'a> = std::iter::Peekable<std::str::CharIndices<'a>>;

/// 并行扫描段产物：每行归一化指纹 + 与上一行原文相等标记。
struct ScanFp {
    fp: Vec<(u64, u64, u32)>,
    prev_same: Vec<bool>,
}

pub(crate) fn dedup(
    lines: &[&str],
    mask: &[u64],
    words: usize,
    err: &[bool],
    force: &[bool],
    code_keep: bool,
) -> (Vec<bool>, usize) {
    let scan = scan_fp(lines);
    dedup_insert(lines, mask, words, err, force, code_keep, &scan)
}

/// 并行扫描段：每行单趟归一化指纹 + 与上一行原文相等标记。
/// 实测（p30m）：旧串行 dedup 117ms 中 74% 是 normalize 扫描本身，
/// HashMap 插入仅 ~30ms。指纹/相等标记均行间独立，按 masks 同款
/// 分块并行（6 线程 / PAR_MIN 阈值），串行段只剩共享 HashMap 插入。
fn scan_fp(lines: &[&str]) -> ScanFp {
    let total = lines.len();
    let n_t = if total >= PAR_MIN { 6 } else { 1 };
    let chunk = total.div_ceil(n_t);
    let mut fp = vec![(0u64, 0u64, 0u32); total];
    let mut prev_same = vec![false; total];
    if n_t > 1 {
        scan_fp_par(lines, chunk, &mut fp, &mut prev_same);
    } else {
        scan_fp_ser(lines, &mut fp, &mut prev_same);
    }
    ScanFp { fp, prev_same }
}

fn scan_fp_par(lines: &[&str], chunk: usize, fp: &mut [(u64, u64, u32)], prev_same: &mut [bool]) {
    let fp_parts = split_parts(fp, chunk);
    let same_parts = split_parts(prev_same, chunk);
    std::thread::scope(|s| {
        for (k, (fp_part, same_part)) in fp_parts.into_iter().zip(same_parts).enumerate() {
            let base = k * chunk;
            s.spawn(move || {
                let mut prev: Option<&str> = None;
                for (j, slot) in fp_part.iter_mut().enumerate() {
                    let line = lines[base + j];
                    if prev == Some(line) {
                        same_part[j] = true;
                    } else {
                        *slot = norm_hash(line);
                    }
                    prev = Some(line);
                }
            });
        }
    });
}

fn scan_fp_ser(lines: &[&str], fp: &mut [(u64, u64, u32)], prev_same: &mut [bool]) {
    let mut prev: Option<&str> = None;
    for i in 0..lines.len() {
        if prev == Some(lines[i]) {
            prev_same[i] = true;
        } else {
            fp[i] = norm_hash(lines[i]);
        }
        prev = Some(lines[i]);
    }
}

/// 串行插入段：共享 HashMap 不可并行。连续重复 run 的指纹取 run 首行。
fn dedup_insert(
    lines: &[&str],
    mask: &[u64],
    words: usize,
    err: &[bool],
    force: &[bool],
    code_keep: bool,
    scan: &ScanFp,
) -> (Vec<bool>, usize) {
    let total = lines.len();
    let mut seen: hash::Hash<(u64, u64, u32), usize> =
        hash::Hash::with_capacity_and_hasher(total, Default::default());
    let mut dup = vec![false; total];
    let mut folds = 0usize;
    let mut run_fp = (0u64, 0u64, 0u32);
    // 围栏感知（code_keep 时）：围栏区间内代码行禁折叠。
    let fence = if code_keep {
        fence_mask(lines)
    } else {
        vec![false; total]
    };
    for i in 0..total {
        let f = if scan.prev_same[i] {
            run_fp
        } else {
            scan.fp[i]
        };
        run_fp = f;
        if f.2 < 6 {
            continue;
        }
        match seen.get_mut(&f) {
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
                seen.insert(f, 1);
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
    // 逐行打分工作量小（约 20-30ms/30MB），并行线程开销大于收益，保持串行。
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

/// 窗口内证据行计数（≥ `thr` 的高分行，dup 行不计）。
fn evidence_hits(scores: &[f64], dup: &[bool], i: usize, window: usize, thr: f64) -> usize {
    let total = scores.len();
    let mut hit = 0usize;
    for d in -(window as i64)..=window as i64 {
        let j = i as i64 + d;
        if j >= 0 && (j as usize) < total && !dup[j as usize] && scores[j as usize] >= thr {
            hit += 1;
        }
    }
    hit
}

/// P17-E 证据簇密度加权（`--cluster 1`，默认关 = 现状逐字节一致）：
/// boost 窗口内高分行聚集处（≥2 证据行）整簇加权翻倍，保留完整上下文块
/// （同一错误栈/同一段日志）。受 `boost_cap` 约束，确定性、无副作用。
pub(crate) fn cluster_signal(
    cfg: &Config,
    scores: &[f64],
    dup: &[bool],
    boost: &mut [f64],
    window: usize,
    tbl: &Weights,
) {
    if !cfg.cluster {
        return;
    }
    let thr = tbl.heading_bonus;
    for i in 0..boost.len() {
        if !dup[i] && evidence_hits(scores, dup, i, window, thr) >= 2 {
            boost[i] = (boost[i] * 2.0).min(tbl.boost_cap);
        }
    }
}

/// P17-C 熵近似评分（`--entropy 1`，默认关 = 零影响）：
/// 无模型的统计信息密度信号——字符分布熵（高熵=信息密集）加权，
/// 字符重复率（zipf 高频填充）降权。对无 query 命中的信息密集行召回补偿。
pub(crate) fn entropy_signal(cfg: &Config, lines: &[&str], dup: &[bool], scores: &mut [f64]) {
    if !cfg.entropy {
        return;
    }
    for (i, line) in lines.iter().enumerate() {
        if dup[i] || line.bytes().any(|b| b.is_ascii_digit()) {
            // 数字保真绝对优先：含数字行不参与熵竞争（消融评测证明熵加分
            // 会整体挤掉数字证据行，log 档 num_cov 0.74→0.67）。
            continue;
        }
        scores[i] += ent_bonus(line);
    }
}

/// 行信息密度加分 ∈ [0, 1)：字符分布熵归一化 × 重复降权。
fn ent_bonus(line: &str) -> f64 {
    let mut cnt = [0u32; 128];
    let mut n = 0usize;
    for &b in line.as_bytes() {
        if b < 128 {
            cnt[b as usize] += 1;
            n += 1;
        }
    }
    if n < 2 {
        return 0.0;
    }
    // 香农熵 H = -Σ p·log2(p)，上限 7 bit。
    let mut h = 0.0;
    for &c in cnt.iter() {
        if c > 0 {
            let p = c as f64 / n as f64;
            h -= p * p.log2();
        }
    }
    let h_norm = (h / 7.0).min(1.0);
    // 重复率：出现过 ≥2 次的字符占比（重复越多信息量越低）。
    let reps = cnt.iter().filter(|&&c| c > 1).count() as f64 / 128.0;
    // 平局决胜权重（0.01 上限）：熵信号只打破同分带内的次序，绝不翻转由
    // query 命中/数字/结构锚主导的保留排序。消融评测证明更高量级会在
    // log 档挤出数字证据行（num_cov 0.74→0.67）——保真优先铁律。
    h_norm * (1.0 - 0.5 * reps) * 0.01
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::Hasher as Digest;

    /// streaming 指纹必须与「normalize + 真实 Hasher」逐位一致（ASCII/非 ASCII）。
    #[test]
    fn norm_hash_matches_normalize_fingerprint() {
        let cases = [
            "服务器　超时  42",
            "a\tb\x0bc\r\nd",
            "deadbeef deadbee abc123 0xDEADBEEF",
            "错误 42 naïve 123456789",
            "conn 9f2a ",
            "纯中文无数字的普通文本",
            "🙂 emoji 混合 ＡＢＣ",
            "abc def",
            "",
            "  abc  ",
            "8080/9090 errno=28",
            "0xDEADBEEF 0xdeadbeef 0xDEADBEE",
            "aaaaaaaaaaaaaaaaaaaa",
            "1234567890",
            "\x1b[31mERROR\x1b[0m 42",
        ];
        for s in cases {
            assert_fp(s);
        }
        // LCG 生成覆盖：ASCII 字母/数字/标点/空白 + 中文 + emoji 边界。
        let mut state = 0x9e37_79b9u32;
        for _ in 0..8_000 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let len = (state as usize >> 8) % 48;
            let mut input = String::with_capacity(len);
            for _ in 0..len {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let pick = (state as usize >> 16) % 3;
                match pick {
                    0 => {
                        let abc = b" abcdefABCDEF0123456789[]()=:/.-_\\\t\r\n\x0b";
                        input.push(abc[(state as usize >> 20) % abc.len()] as char);
                    }
                    1 => {
                        let zh = "错误服务器超时失败崩溃警告连接拒绝";
                        input.push(zh.chars().nth((state as usize >> 20) % 15).unwrap());
                    }
                    _ => input.push('🙂'),
                }
            }
            assert_fp(&input);
        }
    }

    fn assert_fp(s: &str) {
        let mut key = String::new();
        crate::analysis::normalize(s, &mut key);
        let mut h = super::super::hash::Hasher::default();
        h.write(key.as_bytes());
        let want_len = if key.is_ascii() {
            key.len()
        } else {
            key.chars().count()
        };
        let (h1, h2, len) = norm_hash(s);
        assert_eq!(h1, h.finish(), "lane A mismatch for {s:?}");
        assert_eq!(len as usize, want_len, "len mismatch for {s:?}");
        // 双通道确定性：两次计算必须一致。
        let (h1b, h2b, lenb) = norm_hash(s);
        assert_eq!(
            (h1b, h2b, lenb),
            (h1, h2, len),
            "non-deterministic for {s:?}"
        );
    }

    #[test]
    fn entropy_ranks_dense_over_repetitive() {
        // 信息密集（字符多样）行熵加分 > 重复/单调行。
        let dense = "panic: index out of bounds at src/ops/query.rs:42";
        let mono = "aaaa aaaa aaaa aaaa aaaa aaaa aaaa aaaa";
        assert!(
            ent_bonus(dense) > ent_bonus(mono),
            "dense={} mono={}",
            ent_bonus(dense),
            ent_bonus(mono)
        );
        // 空/单字符无信号。
        assert_eq!(ent_bonus(""), 0.0);
        assert_eq!(ent_bonus("x"), 0.0);
        // 上界：不超过 heading_bonus(1.0) 量级。
        assert!(ent_bonus(dense) < 1.0);
    }

    #[test]
    fn entropy_signal_off_is_noop() {
        let lines = ["alpha beta", "gamma delta", "epsilon zeta"];
        let dup = [false, false, false];
        let mut cfg = crate::Config::default();
        let mut sc = vec![1.0f64; 3];
        let before = sc.clone();
        entropy_signal(&cfg, &lines, &dup, &mut sc);
        assert_eq!(sc, before, "默认关 = 零影响");
        cfg.entropy = true;
        entropy_signal(&cfg, &lines, &dup, &mut sc);
        assert!(sc.iter().all(|v| *v > 1.0), "开启后非 dup 行加分: {sc:?}");
    }

    #[test]
    fn entropy_skips_digit_lines() {
        // 数字保真绝对优先：含数字行不参与熵竞争（消融评测证据：
        // 熵加分整体挤掉数字证据行，log 档 num_cov 0.74→0.67）。
        let lines = ["alpha beta", "took 42ms id=7", "alpha beta"];
        let dup = [false, false, false];
        let cfg = crate::Config {
            entropy: true,
            ..crate::Config::default()
        };
        let mut sc = vec![1.0f64; 3];
        entropy_signal(&cfg, &lines, &dup, &mut sc);
        assert_eq!(sc[0], sc[2], "相同文本行公平加分");
        assert!(sc[0] > 1.0);
        assert_eq!(sc[1], 1.0, "含数字行不加分（保真锚不被挤出）");
    }

    #[test]
    fn cluster_off_is_noop_and_on_weights_dense_evidence() {
        let mut cfg = crate::Config::default();
        // 两处证据簇：行 1-2 密集（≥2 高分行），行 9 孤立高分行。
        let scores = [1.5, 1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.5, 0.0, 0.0, 0.0];
        let dup = [false; 12];
        let tbl = &crate::policy::DEFAULT_SCORES;
        let mut boost = crate::compress::plan::boost(&scores, &dup, 1, tbl);
        let before = boost.clone();
        cluster_signal(&cfg, &scores, &dup, &mut boost, 1, tbl);
        assert_eq!(boost, before, "默认关 = 零影响");
        cfg.cluster = true;
        cluster_signal(&cfg, &scores, &dup, &mut boost, 1, tbl);
        // 簇内行（0/1/2 窗口内均见 ≥2 高分行）加权翻倍。
        assert!(
            boost[0] > before[0] && boost[1] > before[1],
            "簇内加权: before={before:?} after={boost:?}"
        );
        // 孤立高分行（9）仅自身 1 个 → 不加权。
        assert!(
            (boost[9] - before[9]).abs() < 1e-12,
            "孤立行不加权: {} vs {}",
            boost[9],
            before[9]
        );
        // 受 boost_cap 约束。
        assert!(boost.iter().all(|v| *v <= tbl.boost_cap));
    }
}
