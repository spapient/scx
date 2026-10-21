//! scx 引擎：本地查询感知证据筛选压缩（纯函数、无 I/O、零依赖、确定性）。
//!
//! 语义（v0.4）：种子不变量（证据行与 ±context（默认 1，可设 0）最小邻域无条件保留，`ratio` 只删可选）；证据仅
//! 来自 IDF 加权查询命中与 `forces`；种子区域门控（可选预算只填种子邻域区域：±window
//! 扩展，间隔 <= window*3 合并；种子为零回退全图）；邻域权重只排序；context=0 完全跳过可选填充（针尖模式）；无信号 fail-open；
//! `risk_gate` 在风险 HIGH 时保护失败级错误行(warn/警告 级除外)与事故标记行(root cause / fix hint / mitigation / recommendation / action / assertionerror)。只删不改、保留行逐字原文；省略以单行 "..." 标记。实现注：无 CR 输入零拷贝行切片；按行归一化与大小写折叠复用缓冲，无逐行分配。
use std::collections::HashMap;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_RATIO: f64 = 0.35;
const MIN_RATIO: f64 = 0.02;
const MAX_CONTEXT: i64 = 4;
const MAX_WINDOW: i64 = 8;
const MAX_QUERY_TERMS: usize = 256;
const MAX_FRAME_EXT_CHARS: usize = 8;
const STOP: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "your", "are", "was", "were", "not",
    "you", "use", "using", "的", "了", "是", "在", "和", "与", "或", "把", "被", "我", "你", "请",
    "帮", "一下", "一个", "这个", "那个", "怎么", "如何", "什么",
];
const ERR_NEEDLES: &[&str] = &[
    "error",
    "fail",
    "failed",
    "failure",
    "exception",
    "panic",
    "assert",
    "denied",
    "refused",
    "timeout",
    "timed out",
    "cannot",
    "unable",
    "undefined",
    "segfault",
    "fatal",
    "warn",
    "traceback",
    "npm err",
    "✗",
    "❌",
    "错误",
    "失败",
    "报错",
    "异常",
    "超时",
    "崩溃",
    "警告",
];
const INCIDENT_MARKERS: &[&str] = &[
    "root cause:",
    "fix hint:",
    "mitigation:",
    "recommendation:",
    "action:",
    "assertionerror:",
];
// 守卫专用错误判据:失败级(与 ERR_NEEDLES 同表,唯去 warn/警告)。warn 级行在噪声日志中常成批出现,
// 风险守卫不应整批保留;行内另有失败级词面者仍受守护。评分/折叠仍用含 warn 的 ERR_NEEDLES。
const GUARD_NEEDLES: &[&str] = &[
    "error",
    "fail",
    "failed",
    "failure",
    "exception",
    "panic",
    "assert",
    "denied",
    "refused",
    "timeout",
    "timed out",
    "cannot",
    "unable",
    "undefined",
    "segfault",
    "fatal",
    "traceback",
    "npm err",
    "✗",
    "❌",
    "错误",
    "失败",
    "报错",
    "异常",
    "超时",
    "崩溃",
];
const EXTS: &[&str] = &[
    "c", "cc", "cpp", "h", "hpp", "cu", "cuh", "rs", "py", "js", "ts", "go", "java", "mm", "m",
    "swift", "zig",
];

/// 引擎配置。`ratio` 为可选内容预算占输入 token 估算的比例；入口会做边界规范化。
pub struct Config {
    pub ratio: f64,
    pub context: i64,
    pub window: i64,
    pub elide: bool,
    pub risk_gate: bool,
    /// `--force` 值按 `|` 拆分后的原始子串（大小写不敏感字面匹配）。
    pub forces: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ratio: 0.35,
            context: 1,
            window: 2,
            elide: true,
            risk_gate: true,
            forces: Vec::new(),
        }
    }
}

/// 压缩结果与统计。`seeds == evidence` 恒成立（结构性不变量，可断言）。
pub struct Outcome {
    /// 压缩后的文本；passthrough / fail-open 时为输入原文。
    pub text: String,
    pub kept: usize,
    pub total: usize,
    pub chars_in: usize,
    pub chars_out: usize,
    pub cut_pct: f64,
    pub dup: usize,
    pub seeds: usize,
    pub evidence: usize,
    pub risk: &'static str,
    pub guarded: u8,
    pub regions: usize,
    pub window: i64,
    pub terms: usize,
    pub forced: usize,
    pub mode: &'static str,
}

fn is_cjk(c: char) -> bool {
    let u = c as u32;
    (0x3400..=0x4DBF).contains(&u)
        || (0x4E00..=0x9FFF).contains(&u)
        || (0x3040..=0x30FF).contains(&u)
        || (0xAC00..=0xD7AF).contains(&u)
        || (0x20000..=0x2FA1F).contains(&u)
}
fn is_cjk_or_fullwidth(c: char) -> bool {
    let u = c as u32;
    is_cjk(c) || (0x3000..=0x303F).contains(&u) || (0xFF00..=0xFFEF).contains(&u)
}

fn build_terms(query: &str) -> Vec<String> {
    let stop = |s: &str| STOP.contains(&s);
    let is_tok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "_./\\:@-".contains(c);
    let mut terms: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in query.to_lowercase().chars() {
        if is_tok(c) {
            cur.push(c);
            continue;
        }
        if cur.chars().count() >= 2 && !stop(&cur) {
            terms.push(cur.clone());
        }
        cur.clear();
    }
    if cur.chars().count() >= 2 && !stop(&cur) {
        terms.push(cur);
    }
    let mut run = String::new();
    let mut runs: Vec<String> = Vec::new();
    for c in query.chars() {
        if is_cjk(c) {
            run.push(c);
            continue;
        }
        if run.chars().count() >= 2 {
            runs.push(run.clone());
        }
        run.clear();
    }
    if run.chars().count() >= 2 {
        runs.push(run);
    }
    for r in runs {
        // CJK 没有空格分词：保留整段，并保证尾部不因 6 字切块而丢掉单字。
        if stop(&r) {
            continue;
        }
        let chars: Vec<char> = r.chars().collect();
        if chars.len() <= 6 {
            if chars.len() >= 2 {
                terms.push(r);
            }
            continue;
        }
        let mut start = 0;
        while start < chars.len() {
            let end = (start + 6).min(chars.len());
            if end - start >= 2 {
                terms.push(chars[start..end].iter().collect());
            }
            start += 6;
        }
        if chars.len() % 6 == 1 {
            terms.push(chars[chars.len() - 2..].iter().collect());
        }
    }
    let mut uniq: Vec<String> = Vec::new();
    for t in terms {
        if uniq.len() >= MAX_QUERY_TERMS {
            break;
        }
        if !uniq.contains(&t) {
            uniq.push(t);
        }
    }
    uniq
}

fn token_est(s: &str) -> f64 {
    if s.is_ascii() {
        return s.len() as f64 / 4.0 + 1.0;
    }
    let mut cjk = 0usize;
    let mut len = 0usize;
    for c in s.chars() {
        len += 1;
        if is_cjk_or_fullwidth(c) {
            cjk += 1;
        }
    }
    (len - cjk) as f64 / 4.0 + cjk as f64 * 0.6 + 1.0
}

// 单趟归一化：8+ 位十六进制串折叠为 '#'；其余数字段折叠为 '#'；空白段折叠为单空格；首尾去空。
// 单趟归一化（零分配版，输出写入调用方缓冲）：8+ 位十六进制串折叠为 '#';
// 其余数字段折叠为 '#';空白段折叠为单空格并延后写出(天然实现首尾去空);run 为复用的十六进制
// 串中转缓冲。语义与旧版逐字节等价。
fn norm_into(s: &str, out: &mut String, run: &mut String) {
    let mut pending_space = false;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c.is_ascii_hexdigit() {
            run.clear();
            run.push(c);
            while it.peek().is_some_and(|&n| n.is_ascii_hexdigit()) {
                run.push(it.next().unwrap());
            }
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            if run.chars().count() >= 8 {
                out.push('#');
            } else {
                let mut ri = run.chars().peekable();
                while let Some(rc) = ri.next() {
                    if rc.is_ascii_digit() {
                        while ri.peek().is_some_and(|&n| n.is_ascii_digit()) {
                            ri.next();
                        }
                        out.push('#');
                    } else {
                        out.push(rc);
                    }
                }
            }
        } else if c.is_ascii_digit() {
            while it.peek().is_some_and(|&n| n.is_ascii_digit()) {
                it.next();
            }
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push('#');
        } else if c.is_whitespace() {
            while it.peek().is_some_and(|&n| n.is_whitespace()) {
                it.next();
            }
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        }
    }
}

fn is_err(ll: &str) -> bool {
    ERR_NEEDLES.iter().any(|n| ll.contains(n))
}
// 事故诊断标记行(行首、大小写不敏感):risk HIGH 且 risk_gate 时与错误行一同受守卫;不参与种子判定。
fn is_incident_marker(ll: &str) -> bool {
    let t = ll.trim_start();
    INCIDENT_MARKERS.iter().any(|m| t.starts_with(m))
}
fn is_guardable(ll: &str) -> bool {
    GUARD_NEEDLES.iter().any(|n| ll.contains(n))
}
fn is_heading(s: &str) -> bool {
    let t = s.trim_start();
    if t.is_empty() {
        return false;
    }
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && t.chars().nth(hashes).is_some_and(|c| c.is_whitespace()) {
        return true;
    }
    if t.starts_with("===") || t.starts_with("---") || t.starts_with("***") {
        return true;
    }
    t.starts_with('>') && t.chars().nth(1).is_some_and(|c| c.is_whitespace())
}
fn is_diffmeta(s: &str) -> bool {
    s.starts_with("@@")
        || s.starts_with("+++")
        || s.starts_with("---")
        || s.starts_with("diff --git")
        || s.starts_with("index ")
}
fn is_diffline(s: &str) -> bool {
    // 统一 diff 的正文行是 "+code" / "-code"，而不是只有第二个字符为空白才成立；
    // 三个相同前缀分别是文件头，交给 is_diffmeta 处理。
    (s.starts_with('+') && !s.starts_with("+++")) || (s.starts_with('-') && !s.starts_with("---"))
}

// 分析用小写缓冲：剥离 ANSI 转义并折叠裸 CR。输出仍保留原始行，因此只影响匹配/排序，
// 不影响“保留行逐字原文”的承诺。进度条常用裸 \r 回写，按空白处理避免污染词面。
fn push_analysis_lower(s: &str, out: &mut String) {
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            match it.peek().copied() {
                Some('[') => {
                    it.next();
                    for esc in it.by_ref() {
                        if ('@'..='~').contains(&esc) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    it.next();
                    while let Some(esc) = it.next() {
                        if esc == '\u{7}' {
                            break;
                        }
                        if esc == '\u{1b}' && it.peek() == Some(&'\\') {
                            it.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    it.next();
                }
                None => {}
            }
            continue;
        }
        if c == '\r' {
            out.push(' ');
            continue;
        }
        for lc in c.to_lowercase() {
            out.push(lc);
        }
    }
}
// 带前导空白的行且形如堆栈帧：at ...／File "／#N ／含 path.ext:line。
fn is_frame(s: &str) -> bool {
    let t = s.trim_start();
    if t.len() == s.len() {
        return false;
    }
    if t.starts_with("at ") || t.starts_with("File \"") {
        return true;
    }
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if hashes >= 1
        && t.chars().nth(hashes).is_some_and(|c| c.is_ascii_digit())
        && t.chars().skip(hashes).any(|c| c.is_whitespace())
    {
        return true;
    }
    let cs: Vec<char> = t.chars().collect();
    for i in 1..cs.len() {
        if cs[i] == '.' && !cs[i - 1].is_whitespace() {
            let mut j = i + 1;
            let limit = (i + 1 + MAX_FRAME_EXT_CHARS).min(cs.len());
            while j < limit && !cs[j].is_whitespace() && cs[j] != ':' {
                j += 1;
            }
            if j < cs.len() && cs[j] == ':' {
                let ext = &cs[i + 1..j];
                if EXTS.iter().any(|e| e.chars().eq(ext.iter().copied()))
                    && j + 1 < cs.len()
                    && cs[j + 1].is_ascii_digit()
                {
                    return true;
                }
            }
        }
    }
    false
}
fn is_lineref(s: &str) -> bool {
    let mut it = s.chars().rev();
    let mut cur = it.next();
    if cur == Some(')') {
        cur = it.next();
    }
    let mut digits = 0usize;
    while cur.is_some_and(|c| c.is_ascii_digit()) {
        digits += 1;
        cur = it.next();
    }
    digits > 0 && cur == Some(':')
}

/// 对 `text` 按 `query` 做证据筛选压缩。纯函数：无文件、环境与进程 I/O。
pub fn compress(text: &str, query: &str, cfg: &Config) -> Outcome {
    // 库是公开接口：配置在入口统一规范化，避免 CLI 之外的调用触发溢出或超大循环。
    let ratio = if cfg.ratio.is_nan() {
        DEFAULT_RATIO
    } else {
        cfg.ratio.clamp(MIN_RATIO, 1.0)
    };
    let context = cfg.context.clamp(0, MAX_CONTEXT);
    let window = if cfg.window <= 0 {
        1
    } else {
        cfg.window.min(MAX_WINDOW)
    };

    // 分析使用 LF 归一化文本；passthrough/fail-open 返回输入原文，保持“原样保留”语义。
    let original = text;
    // 零拷贝快路径：无 CR 时直接借用原文本；行以 &str 切片保存，避免整文复制与逐行分配。
    let normalized;
    let text: &str = if original.contains('\r') {
        normalized = original.replace("\r\n", "\n");
        normalized.as_str()
    } else {
        original
    };
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let uniq_terms = build_terms(query);
    let mut force_subs: Vec<String> = Vec::new();
    for f in &cfg.forces {
        for part in f.split('|') {
            let p = part.trim().to_lowercase();
            if !p.is_empty() {
                force_subs.push(p);
            }
        }
    }
    let chars_a = original.chars().count();
    let total = lines.len();
    let tok_v: Vec<f64> = lines.iter().map(|s| token_est(s)).collect();
    let total_tok: f64 = tok_v.iter().sum();

    let mk = |text_owned: String,
              kept: usize,
              chars_b: usize,
              cut: f64,
              dup: usize,
              seeds: usize,
              risk: &'static str,
              guarded: u8,
              regions: usize,
              mode: &'static str|
     -> Outcome {
        Outcome {
            text: text_owned,
            kept,
            total,
            chars_in: chars_a,
            chars_out: chars_b,
            cut_pct: cut,
            dup,
            seeds,
            evidence: seeds,
            risk,
            guarded,
            regions,
            window,
            terms: uniq_terms.len(),
            forced: force_subs.len(),
            mode,
        }
    };

    if total < 40 && total_tok < 1200.0 {
        return mk(
            original.to_string(),
            total,
            chars_a,
            0.0,
            0,
            0,
            "LOW",
            0,
            0,
            "passthrough:small",
        );
    }
    if lines.iter().any(|s| s.chars().count() > 100000) {
        return mk(
            original.to_string(),
            total,
            chars_a,
            0.0,
            0,
            0,
            "LOW",
            0,
            0,
            "passthrough:longline",
        );
    }

    // 一次逐行×逐词扫描出命中位掩码；错误/标记/守卫/force 判定复用同一小写缓冲（无逐行分配）。
    let words = uniq_terms.len().div_ceil(64).max(1);
    let mut mask_v = vec![0u64; total * words];
    let mut err_v = vec![false; total];
    let mut marker_v = vec![false; total];
    let mut guard_v = vec![false; total];
    let mut force_v = vec![false; total];
    let mut low = String::new();
    for i in 0..total {
        low.clear();
        push_analysis_lower(lines[i], &mut low);
        let ll = low.as_str();
        err_v[i] = is_err(ll);
        marker_v[i] = is_incident_marker(ll);
        guard_v[i] = is_guardable(ll);
        force_v[i] = force_subs.iter().any(|p| ll.contains(p.as_str()));
        let base = i * words;
        for (k, t) in uniq_terms.iter().enumerate() {
            if ll.contains(t.as_str()) {
                mask_v[base + k / 64] |= 1u64 << (k % 64);
            }
        }
    }
    let has_hit = |i: usize| (0..words).any(|w| mask_v[i * words + w] != 0);

    // 去重：归一化后第 3 次起的重复行折叠（除非含查询命中或错误行）。
    let mut seen_count: HashMap<String, usize> = HashMap::new();
    let mut dup = vec![false; total];
    let mut dup_n = 0usize;
    let mut key = String::new();
    let mut run = String::new();
    for i in 0..total {
        key.clear();
        norm_into(lines[i], &mut key, &mut run);
        if key.chars().count() < 6 {
            continue;
        }
        if let Some(n) = seen_count.get_mut(key.as_str()) {
            *n += 1;
            if *n > 2 && !has_hit(i) && !err_v[i] && !force_v[i] {
                dup[i] = true;
                dup_n += 1;
            }
        } else {
            seen_count.insert(key.clone(), 1);
        }
    }

    // 词频（df）与 IDF 权重：只统计非折叠行。
    let live: usize = (0..total).filter(|&i| !dup[i]).count();
    let n_live = live.max(1) as f64;
    let mut df = vec![0usize; uniq_terms.len()];
    for (i, &is_dup) in dup.iter().enumerate() {
        if is_dup {
            continue;
        }
        let base = i * words;
        for w in 0..words {
            let mut bits = mask_v[base + w];
            while bits != 0 {
                let k = w * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if k < uniq_terms.len() {
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

    // 逐行证据判定：种子只来自查询证据（≥2 命中或稀有单词命中）与 --force。
    let mut hits_v = vec![0usize; total];
    let mut sumw_v = vec![0.0f64; total];
    let mut seed = vec![false; total];
    let mut hit_lines = 0usize;
    let mut force_any = false;
    for i in 0..total {
        if dup[i] {
            continue;
        }
        let base = i * words;
        let mut hits = 0usize;
        let mut sumw = 0.0f64;
        let mut lone = usize::MAX;
        for w in 0..words {
            let mut bits = mask_v[base + w];
            while bits != 0 {
                let k = w * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if k >= uniq_terms.len() {
                    continue;
                }
                hits += 1;
                sumw += weights[k];
                lone = if hits == 1 { k } else { usize::MAX };
            }
        }
        let fm = force_v[i];
        force_any |= fm;
        hits_v[i] = hits;
        sumw_v[i] = sumw;
        seed[i] = hits >= 2 || (hits == 1 && lone != usize::MAX && df[lone] <= rare_thr) || fm;
        if hits > 0 {
            hit_lines += 1;
        }
    }
    let seeds_total = (0..total).filter(|&i| seed[i]).count();

    // 无信号 fail-open：零命中且无 --force 命中 => 原样保留。
    if hit_lines == 0 && !force_any {
        return mk(
            original.to_string(),
            total,
            chars_a,
            0.0,
            dup_n,
            seeds_total,
            "LOW",
            0,
            0,
            "failopen:zero-signal",
        );
    }

    // 种子区域：±window 扩展，间隔 <= window*3 合并；区域外素材不参与可选预算。
    let w = window as usize;
    let maxgap = (window * 3).max(2) as usize;
    let seed_idx: Vec<usize> = (0..total).filter(|&i| seed[i]).collect();
    let mut in_region = vec![false; total];
    let mut region_count = 0usize;
    if !seed_idx.is_empty() {
        let mut a = seed_idx[0].saturating_sub(w);
        let mut b = (seed_idx[0] + w).min(total - 1);
        for &s in &seed_idx[1..] {
            if s <= b + maxgap {
                b = (s + w).min(total - 1);
            } else {
                for slot in in_region.iter_mut().take(b + 1).skip(a) {
                    *slot = true;
                }
                region_count += 1;
                a = s.saturating_sub(w);
                b = (s + w).min(total - 1);
            }
        }
        for slot in in_region.iter_mut().take(b + 1).skip(a) {
            *slot = true;
        }
        region_count += 1;
    }
    let region_gate = !seed_idx.is_empty();

    // 逐行打分：基础 = IDF 加权命中；结构加分只提升排序价值。
    let mut scores = vec![0.0f64; total];
    for i in 0..total {
        if dup[i] {
            continue;
        }
        let s = lines[i];
        let mut sc = sumw_v[i] * 3.0;
        if hits_v[i] >= 2 {
            sc += 2.0;
        }
        if err_v[i] {
            sc += 4.0;
        }
        if is_heading(s) {
            sc += 1.0;
        }
        if is_diffmeta(s) {
            sc += 2.0;
        }
        if is_diffline(s) && hits_v[i] > 0 {
            sc += 2.0;
        }
        if is_frame(s) || is_lineref(s) {
            sc += 2.0;
        }
        if force_v[i] {
            sc += 100.0;
        }
        if i < 2 || i >= total.saturating_sub(3) {
            sc += 2.0;
        }
        scores[i] = sc;
    }
    // 邻域权重：±window 范围内的高分给排序加权（不授予独立入选资格）。
    let mut boost = vec![0.0f64; total];
    for (i, b) in boost.iter_mut().enumerate() {
        let mut m = 0.0f64;
        for d in -window..=window {
            let j = i as i64 + d;
            if j >= 0 && (j as usize) < total && !dup[j as usize] {
                m = m.max(scores[j as usize]);
            }
        }
        *b = (m / 4.0).min(3.0);
    }

    // 先落种子与最小可读邻域；预算只约束其后的可选内容。
    let mut keep = vec![false; total];
    let mut keep_count = 0usize;
    for (i, &is_seed) in seed.iter().enumerate() {
        if !is_seed {
            continue;
        }
        for d in -context..=context {
            let j = i as i64 + d;
            if j >= 0 && (j as usize) < total && !keep[j as usize] {
                keep[j as usize] = true;
                keep_count += 1;
            }
        }
    }

    // 预算选择：种子区域门控（首两行身份行除外）；独立入选须真实得分（scores > 0）。
    // --context 0（针尖模式）：完全跳过可选预算填充，只保留证据行与守卫行。
    if context != 0 {
        let budget = (total_tok * ratio).max(200.0);
        let mut order: Vec<usize> = (0..total).filter(|&i| !dup[i] && !keep[i]).collect();
        order.sort_by(|&a, &b| {
            let ka = scores[a] + boost[a];
            let kb = scores[b] + boost[b];
            if kb != ka {
                kb.partial_cmp(&ka).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                a.cmp(&b)
            }
        });
        let mut used = 0.0f64;
        for &i in &order {
            if (region_gate && !in_region[i] && i >= 2) || scores[i] <= 0.0 {
                continue;
            }
            let t = tok_v[i];
            if used + t > budget {
                continue;
            }
            if !keep[i] {
                keep[i] = true;
                keep_count += 1;
            }
            used += t;
        }
    }

    // 风险审计与错误行保护（guarded）。
    let mut err_dropped = 0usize;
    let mut hit_dropped = 0usize;
    for i in 0..total {
        if keep[i] {
            continue;
        }
        if err_v[i] {
            err_dropped += 1;
        }
        if hits_v[i] > 0 {
            hit_dropped += 1;
        }
    }
    let risk_raw = (err_dropped + hit_dropped * 3) as f64 / keep_count.max(1) as f64;
    let risk = if risk_raw >= 0.35 {
        "HIGH"
    } else if risk_raw >= 0.1 {
        "MED"
    } else {
        "LOW"
    };
    let mut guarded = 0u8;
    if risk == "HIGH" && cfg.risk_gate {
        for i in 0..total {
            if keep[i] || dup[i] {
                continue;
            }
            if guard_v[i] || marker_v[i] {
                keep[i] = true;
                keep_count += 1;
                guarded = 1;
            }
        }
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
            region_count,
            "passthrough:no-kept",
        );
    }

    // 输出：零克隆直装；省略区间以单行 "..." 标记。
    let mut out_text = String::with_capacity(original.len());
    let mut in_gap = false;
    for i in 0..total {
        if keep[i] {
            if in_gap && cfg.elide {
                out_text.push_str("...\n");
            }
            in_gap = false;
            out_text.push_str(lines[i]);
            out_text.push('\n');
        } else {
            in_gap = true;
        }
    }
    if in_gap && cfg.elide {
        out_text.push_str("...\n");
    }
    let chars_b = if out_text.ends_with('\n') {
        out_text.chars().count().saturating_sub(1)
    } else {
        out_text.chars().count()
    };
    let cut_pct = if chars_a > 0 {
        100.0 * (1.0 - chars_b as f64 / chars_a as f64)
    } else {
        0.0
    };
    mk(
        out_text,
        keep_count,
        chars_b,
        cut_pct,
        dup_n,
        seeds_total,
        risk,
        guarded,
        region_count,
        "sieve",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            ratio: 0.35,
            ..Config::default()
        }
    }
    fn filler() -> String {
        "// filler line with enough tokens to pass the small guard".to_string()
    }

    #[test]
    fn invariant_seeds_kept() {
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "fn target() { return 1; }".to_string();
        let text = lines.join("\n");
        let o = compress(&text, "target", &cfg());
        assert_eq!(o.seeds, o.evidence);
        assert!(o.seeds >= 1);
        assert!(o.text.contains("fn target"));
    }

    #[test]
    fn zero_signal_failopen() {
        let text: String = (0..200)
            .map(|i| format!("line {} with padding text alpha beta gamma", i))
            .collect::<Vec<_>>()
            .join("\n");
        let o = compress(&text, "绝对不存在的词zz", &cfg());
        assert_eq!(o.mode, "failopen:zero-signal");
        assert_eq!(o.text, text);
    }

    #[test]
    fn warn_mass_not_guarded_in_needle() {
        let mut lines: Vec<String> = (0..300)
            .map(|i| format!("WARN queue depth={} consumer_lag=2s pool=3", i))
            .collect();
        lines[150] = "ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a)".to_string();
        lines[151] = "Root cause: connection pool size 4 < peak concurrency 11".to_string();
        let text = lines.join(&char::from(10u8).to_string());
        let needle = Config {
            ratio: 0.35,
            context: 0,
            ..Config::default()
        };
        let o = compress(&text, "what failed db timeout pool conn_id", &needle);
        assert!(o.text.contains("conn_id=9f2a"));
        assert!(o.text.contains("Root cause:"));
        assert!(
            !o.text.contains("WARN queue depth"),
            "warn 级行不应被风险守卫成批保留"
        );
        assert!(o.kept <= 4);
    }

    #[test]
    fn marker_guard_keeps_incident_marker_in_needle() {
        let mut lines: Vec<String> = (0..300)
            .map(|i| format!("INFO heartbeat seq={} ok pool=3 latency=1ms", i))
            .collect();
        lines[150] = "ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a)".to_string();
        lines[151] = "at com.ingest.Pool.acquire(Pool.java:214)".to_string();
        lines[152] = "Root cause: connection pool size 4 < peak concurrency 11".to_string();
        let text = lines.join(&char::from(10u8).to_string());
        let q = "what failed db timeout pool conn_id";
        let needle = Config {
            ratio: 0.35,
            context: 0,
            ..Config::default()
        };
        let o = compress(&text, q, &needle);
        assert_eq!(o.risk, "HIGH");
        assert_eq!(o.guarded, 1);
        assert!(
            o.text.contains("Root cause:"),
            "needle 档必须在 HIGH 风险下保留事故标记行"
        );
        assert!(o.text.contains("conn_id=9f2a"));
        let gated_off = Config {
            ratio: 0.35,
            context: 0,
            risk_gate: false,
            ..Config::default()
        };
        let o2 = compress(&text, q, &gated_off);
        assert!(
            !o2.text.contains("Root cause:"),
            "risk_gate=0 时标记行不被强制保留"
        );
        assert_eq!(o2.guarded, 0);
    }

    #[test]
    fn lineref_semantics() {
        assert!(is_lineref("foo.rs:10"));
        assert!(is_lineref("(bar.c:3)"));
        assert!(is_lineref(":123"));
        assert!(is_lineref("y:3)"));
        assert!(!is_lineref("abc:"));
        assert!(!is_lineref("12"));
        assert!(!is_lineref(":"));
        assert!(!is_lineref("x)y"));
    }

    #[test]
    fn unified_diff_content_lines_are_recognized() {
        assert!(is_diffline("+let value = 1;"));
        assert!(is_diffline("-old_value"));
        assert!(is_diffline("+"));
        assert!(is_diffline("-"));
        assert!(!is_diffline("+++ b/src/lib.rs"));
        assert!(!is_diffline("--- a/src/lib.rs"));
        assert!(!is_diffline("diff --git a/x b/x"));
    }

    #[test]
    fn analysis_strips_ansi_and_bare_carriage_returns() {
        let mut low = String::new();
        push_analysis_lower("\u{1b}[31mRoot cause:\u{1b}[0m 10%\rnext", &mut low);
        assert_eq!(low, "root cause: 10% next");
        assert!(is_incident_marker(&low));
        low.clear();
        push_analysis_lower("\u{1b}]8;;https://example.com\u{7}link", &mut low);
        assert_eq!(low, "link");
    }

    #[test]
    fn norm_into_semantics() {
        let n = |t: &str| {
            let mut o = String::new();
            let mut r = String::new();
            norm_into(t, &mut o, &mut r);
            o
        };
        assert_eq!(n("  a   b  "), "a b");
        assert_eq!(n("xyz123def"), "xyz#def");
        assert_eq!(n("abcdefab"), "#");
        assert_eq!(n("0xDEADBEEF1234"), "#x#");
        assert_eq!(n("a1b2c3"), "a#b#c#");
        assert_eq!(n("seq=42 ok"), "seq=# ok");
        assert_eq!(n(""), "");
        assert_eq!(n("汉字 42"), "汉字 #");
    }

    #[test]
    fn crlf_equivalence() {
        let lf = char::from(10u8).to_string();
        let crlf_sep = String::from(char::from(13u8)) + lf.as_str();
        let base: String = (0..200)
            .map(|i| format!("line {} alpha kvwrite beta gamma delta", i))
            .collect::<Vec<_>>()
            .join(lf.as_str());
        let crlf = base.replace(lf.as_str(), crlf_sep.as_str());
        let a = compress(&base, "kvwrite", &cfg());
        let b = compress(&crlf, "kvwrite", &cfg());
        assert_eq!(a.text, b.text);
        assert_eq!(a.kept, b.kept);
        assert_eq!(a.chars_out, b.chars_out);
        assert!(b.chars_in > a.chars_in, "CRLF 输入的原始字符数应包含 CR");
    }

    #[test]
    fn passthrough_modes() {
        let lf = char::from(10u8).to_string();
        let small: String = (0..10)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join(lf.as_str());
        let o = compress(&small, "line", &cfg());
        assert_eq!(o.mode, "passthrough:small");
        assert_eq!(o.text, small);
        let mut lines = vec!["x".repeat(150000)];
        lines.extend((0..60).map(|i| format!("filler error kvwrite line {}", i)));
        let text = lines.join(lf.as_str());
        let o2 = compress(&text, "kvwrite", &cfg());
        assert_eq!(o2.mode, "passthrough:longline");
        assert_eq!(o2.text, text);
    }

    #[test]
    fn cjk_error_words_guard_and_class() {
        let mut lines: Vec<String> = (0..300)
            .map(|i| format!("INFO 心跳 seq={} 状态=正常 latency=1ms", i))
            .collect();
        lines[150] = "错误：数据库连接超时 (conn_id=9f2a)".to_string();
        lines[151] = "错误：磁盘已满 errno=28".to_string();
        lines[152] = "警告：队列深度=40".to_string();
        let text = lines.join(&char::from(10u8).to_string());
        let needle = Config {
            ratio: 0.35,
            context: 0,
            ..Config::default()
        };
        let o = compress(&text, "数据库 超时 conn_id", &needle);
        assert!(o.text.contains("conn_id=9f2a"), "命中行作为种子保留");
        assert!(
            o.text.contains("磁盘已满"),
            "失败级 CJK 错误行在 HIGH 风险下受守卫"
        );
        assert!(!o.text.contains("队列深度"), "警告级 CJK 行不成批保留");
        assert_eq!(o.guarded, 1);
    }

    #[test]
    fn deterministic_output() {
        let text: String = (0..300)
            .map(|i| format!("l{} error alpha beta kvwrite", i % 7))
            .collect::<Vec<_>>()
            .join("\n");
        let a = compress(&text, "kvwrite", &cfg());
        let b = compress(&text, "kvwrite", &cfg());
        assert_eq!(a.text, b.text);
        assert_eq!(a.cut_pct.to_bits(), b.cut_pct.to_bits());
    }

    #[test]
    fn force_duplicates_are_all_kept() {
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("INFO filler {} alpha beta", i))
            .collect();
        for idx in [150usize, 151, 152, 153] {
            lines[idx] = "SPECIAL_FORCE_LINE".to_string();
        }
        let text = lines.join("\n");
        let needle = Config {
            ratio: 0.02,
            context: 0,
            forces: vec!["SPECIAL_FORCE_LINE".to_string()],
            ..Config::default()
        };
        let o = compress(&text, "zzzznomatch", &needle);
        assert_eq!(o.text.matches("SPECIAL_FORCE_LINE").count(), 4);
        assert_eq!(o.seeds, 4);
    }

    #[test]
    fn no_kept_falls_back_to_passthrough() {
        let line = format!("alpha {}", "x".repeat(5000));
        let text: String = (0..40).map(|_| line.clone()).collect::<Vec<_>>().join("\n");
        let needle = Config {
            ratio: 0.02,
            context: 1,
            ..Config::default()
        };
        let o = compress(&text, "alpha", &needle);
        assert_eq!(o.mode, "passthrough:no-kept");
        assert_eq!(o.text, text);
        assert_eq!(o.kept, o.total);
        assert_eq!(o.chars_out, o.chars_in);
        assert_eq!(o.cut_pct, 0.0);
    }

    #[test]
    fn config_values_are_normalized() {
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "fn target() { return 1; }".to_string();
        let text = lines.join("\n");
        let weird = Config {
            ratio: f64::NAN,
            context: -9,
            window: -1,
            ..Config::default()
        };
        let o = compress(&text, "target", &weird);
        assert_eq!(o.window, 1);
        assert!(o.text.contains("fn target"));

        let huge = Config {
            window: i64::MAX,
            context: i64::MAX,
            ..Config::default()
        };
        let o2 = compress(&text, "target", &huge);
        assert_eq!(o2.window, 8);
        assert!(o2.text.contains("fn target"));
    }

    #[test]
    fn small_guard_requires_few_tokens() {
        let line = format!("alpha {}", "x".repeat(5000));
        let text: String = (0..39).map(|_| line.clone()).collect::<Vec<_>>().join("\n");
        let o = compress(&text, "alpha", &cfg());
        assert_ne!(o.mode, "passthrough:small");
        assert!(o.seeds == 0);
    }

    #[test]
    fn budget_is_optional_only() {
        let mut lines: Vec<String> = (0..99)
            .map(|i| format!("alpha filler {} {}", i, "x".repeat(100)))
            .collect();
        lines.insert(50, format!("SPECIAL_FORCE {}", "x".repeat(10000)));
        let text = lines.join("\n");
        let needle = Config {
            ratio: 0.02,
            context: 1,
            forces: vec!["SPECIAL_FORCE".to_string()],
            ..Config::default()
        };
        let o = compress(&text, "alpha", &needle);
        assert_eq!(o.seeds, 1);
        assert!(o.kept > 3, "种子与上下文不应挤占可选内容预算");
    }

    #[test]
    fn cjk_stop_and_tail_terms() {
        assert!(build_terms("这个").is_empty());
        assert!(build_terms("数据库连接超时").contains(&"超时".to_string()));
        assert!(build_terms("エラー").contains(&"エラー".to_string()));
    }

    #[test]
    fn crlf_failopen_preserves_original() {
        let lines: Vec<String> = (0..200)
            .map(|i| format!("line {} alpha beta gamma delta", i))
            .collect();
        let text = lines.join("\r\n");
        let o = compress(&text, "zzzznomatch", &cfg());
        assert_eq!(o.mode, "failopen:zero-signal");
        assert_eq!(o.text, text);
        assert_eq!(o.chars_in, text.chars().count());
    }

    #[test]
    fn frame_extension_scan_is_bounded() {
        assert!(is_frame("  foo.rs:10"));
        assert!(!is_frame(&format!("  foo.{}:10", "a".repeat(50))));
    }
}
