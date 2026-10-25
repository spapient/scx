//! compress 引擎主体：证据筛选压缩的完整算法。
//!
//! 纯函数、无 I/O、无随机源；本文件不定义 Config/Outcome（在 crate 根），只实现
//! 从文本到压缩结果的单趟管线。独立成文件以维持 crate 根 <500 行门禁。

use crate::classify::{
    error_flags, is_diffline, is_diffmeta, is_frame, is_heading, is_incident_marker, is_lineref,
};
use crate::normalize::{norm_into, push_analysis_lower};
use crate::terms::{build_terms, token_est};
use crate::{push_lineno, Config, Outcome};
use std::collections::HashMap;

/// 字符数用于报告与压缩比；日志常见纯 ASCII，可跳过逐字符迭代。
fn char_count(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    }
}

/// 对 `text` 按 `query` 做证据筛选压缩。纯函数：无文件、环境与进程 I/O。
pub fn compress(text: &str, query: &str, cfg: &Config) -> Outcome {
    // 库是公开接口：配置在入口统一规范化，避免 CLI 之外的调用触发溢出或超大循环。
    let ratio = if cfg.ratio.is_nan() {
        Config::DEFAULT_RATIO
    } else {
        cfg.ratio.clamp(Config::MIN_RATIO, 1.0)
    };
    let context = cfg.context.clamp(0, Config::MAX_CONTEXT);
    let window = if cfg.window < Config::MIN_WINDOW {
        Config::MIN_WINDOW
    } else {
        cfg.window.min(Config::MAX_WINDOW)
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
    let total = lines.len();
    // token 估算只服务小输入直通与可选预算；针尖档大盘无需整份扫描。
    let total_tok: f64 = if context != 0 || total < 40 {
        lines.iter().map(|s| token_est(s)).sum()
    } else {
        0.0
    };
    let chars_a = char_count(original);

    let mk = |text_owned: String,
              kept: usize,
              chars_b: usize,
              cut: f64,
              dup: usize,
              seeds: usize,
              risk: &'static str,
              guarded: u8,
              regions: usize,
              kept_lno: Vec<usize>,
              mode: &'static str|
     -> Outcome {
        Outcome {
            text: text_owned,
            kept_lno,
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
            Vec::new(),
            "passthrough:small",
        );
    }
    // 100k 字符的行必然至少 100k 字节；先做 O(1) 字节检查，只有超长行才
    // 付 Unicode 字符计数成本（避免对每行都扫描一遍 chars）。
    if lines
        .iter()
        .any(|s| s.len() > 100_000 && s.chars().count() > 100_000)
    {
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
            Vec::new(),
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
        (err_v[i], guard_v[i]) = error_flags(ll);
        marker_v[i] = is_incident_marker(ll);
        force_v[i] = force_subs.iter().any(|p| ll.contains(p.as_str()));
        let base = i * words;
        for (k, t) in uniq_terms.iter().enumerate() {
            if ll.contains(t.as_str()) {
                mask_v[base + k / 64] |= 1u64 << (k % 64);
            }
        }
    }
    let has_hit = |i: usize| (0..words).any(|w| mask_v[i * words + w] != 0);

    // 去重：归一化后第 3 次起的重复行折叠（除非含查询命中、错误行或 force 命中）。
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
            Vec::new(),
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
    // 合成最终排序分，后续比较只读一次，避免 O(n log n) 次重复浮点加法。
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
        // 只排序具备独立入选资格的候选；区域外/零分行无论排序位置都不会
        // 进入预算，过滤后再排序可显著缩小默认配置下的比较规模。
        let mut order: Vec<(usize, f64)> = (0..total)
            .filter(|&i| {
                !dup[i] && !keep[i] && scores[i] > 0.0 && (!region_gate || in_region[i] || i < 2)
            })
            .map(|i| (i, scores[i] + boost[i]))
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
            let t = token_est(lines[i]);
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
            Vec::new(),
            "passthrough:no-kept",
        );
    }

    // 输出：零克隆直装；省略区间以单行 "..." 标记。保留行原始行号同步收集（i+1）。
    let mut out_text = String::with_capacity(original.len());
    let mut kept_lno = Vec::with_capacity(keep_count);
    let mut in_gap = false;
    for i in 0..total {
        if keep[i] {
            if in_gap && cfg.elide {
                out_text.push_str("...\n");
            }
            in_gap = false;
            if cfg.lineno {
                push_lineno(&mut out_text, i + 1);
            }
            out_text.push_str(lines[i]);
            out_text.push('\n');
            kept_lno.push(i + 1);
        } else {
            in_gap = true;
        }
    }
    if in_gap && cfg.elide {
        out_text.push_str("...\n");
    }
    let chars_b = if out_text.ends_with('\n') {
        char_count(&out_text).saturating_sub(1)
    } else {
        char_count(&out_text)
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
        kept_lno,
        "sieve",
    )
}
