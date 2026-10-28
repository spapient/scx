// 摘自 scx（Rust 零依赖 LLM 上下文压缩器）src/trim.rs —— token 级
// 重要性裁剪内核。删减了与核心打分无关的调用方，逻辑与上游一致。
//
// score 表（P6d 定稿）：数字 3 / 锚 4 / query 命中 2 / KEEP 2 /
// 停用词 0 / 普通词 1。trim_ratio ∈ (0,1]：低分 token 的保留比例。

struct Tokens {
    start: Vec<usize>,
    end: Vec<usize>,
    numeric: Vec<bool>,
    anchor: Vec<bool>,
    hit: Vec<bool>,
    stop: Vec<bool>,
    keep: Vec<bool>,
    punct: Vec<bool>,
}

fn score(t: &Tokens, i: usize) -> i32 {
    if t.anchor[i] {
        4
    } else if t.numeric[i] {
        3
    } else if t.hit[i] {
        2
    } else if t.keep[i] {
        2
    } else if t.stop[i] {
        0
    } else if t.punct[i] {
        0
    } else {
        1
    }
}

/// 裁剪一行：高重要（数字/命中/锚）与行首尾强制保留；
/// 低分 token 按 trim_ratio 保留，先保分数高的。
fn trim_line(t: &Tokens, trim_ratio: f64, line: &str) -> String {
    let total = t.start.len();
    if total <= 1 {
        return line.trim().to_string();
    }
    let mut keep = vec![false; total];
    for (i, k) in keep.iter_mut().enumerate() {
        if score(t, i) >= 2 || t.anchor[i] || i == 0 || i == total - 1 {
            *k = true;
        }
    }
    let low: Vec<usize> = (0..total).filter(|&i| !keep[i]).collect();
    if low.is_empty() {
        return line.trim().to_string();
    }
    let keep_low = ((low.len() as f64) * trim_ratio).ceil() as usize;
    let mut order: Vec<usize> = low.clone();
    order.sort_unstable_by(|&a, &b| {
        score(t, b)
            .partial_cmp(&score(t, a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    for &i in order.iter().take(keep_low) {
        keep[i] = true;
    }
    let mut out = String::new();
    for (i, &k) in keep.iter().enumerate() {
        if k {
            out.push_str(&line[t.start[i]..t.end[i]]);
        }
    }
    out
}
