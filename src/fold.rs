//! P17-A：跨行模板折叠（`--fold 1`，默认关闭）。
//!
//! 日志风暴类输入（成百上千行仅参数不同的相似行）会稀释证据密度并撑大输出。
//! 折叠把**连续块内共享静态骨架**的行（≥ `FOLD_MIN` 行）压缩为首行 + 一行
//! 折叠标记（含差异参数摘要），压缩率显著提升，且模板首行保留原文证据。
//!
//! 铁律：
//! - 默认关闭（`cfg.fold=false` 时主循环零开销短路，默认路径逐字节一致）。
//! - 纯 std、确定性：相同输入恒同输出。
//! - 只作用于连续保留块，不跨空行/省略段；参数 token 只含数字/时间戳/hex/短码，
//!   语义 token 全部计入骨架，保证不误吞信息行。
//!
//! 与 trim 组合：折叠发生在 trim 之后，块内行取「trim 后文本」（若该行被 trim）。

/// 折叠最小行数：低于此不折叠（防止误并）。
pub(crate) const FOLD_MIN: usize = 3;
/// 标记行差异参数最多列出的个数；超出以 `+N` 计数。
const DIFFS_MAX: usize = 8;

/// 参数 token 判定：含数字（时间戳/计数/hex）或 ≤2 位纯小写短码（id/a/b）；
/// 语义 token（≥3 字母词）入骨架。
fn is_param(t: &str) -> bool {
    t.bytes().any(|b| b.is_ascii_digit())
        || (t.len() <= 2 && t.bytes().all(|b| b.is_ascii_lowercase()))
}

/// 单行 → 骨架 key（参数 token 归一为 `#` 占位）与参数 token 列表。
fn split(line: &str) -> (String, Vec<String>) {
    let mut skeleton = String::new();
    let mut params = Vec::new();
    for t in tokens(line) {
        if is_param(t) {
            params.push(t.to_string());
            skeleton.push('#');
        } else {
            skeleton.push_str(t);
        }
        skeleton.push('|');
    }
    (skeleton, params)
}

/// 单字节推进：返回 (是否词字符, 步长)。CJK 3 字节；ASCII 词字符 1 字节。
fn step(b: &[u8], i: usize) -> (bool, usize) {
    let c = b[i];
    if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b':' | b'/' | b'@' | b'\\') {
        (true, 1)
    } else if (0xE4..=0xE9).contains(&c) && i + 2 < b.len() {
        (true, 3)
    } else {
        (false, 1)
    }
}

/// 轻量分词：ASCII 字母数字连续段 + CJK 连续段，其余字符分隔。
fn tokens(line: &str) -> Vec<&str> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut cur: Option<usize> = None;
    let mut i = 0usize;
    while i < b.len() {
        let (word, adv) = step(b, i);
        if word {
            if cur.is_none() {
                cur = Some(i);
            }
        } else if let Some(st) = cur.take() {
            out.push(&line[st..i]);
        }
        i += adv;
    }
    if let Some(st) = cur.take() {
        out.push(&line[st..b.len()]);
    }
    out
}

/// 折叠标记行：`scx-fold: {n-1} rows folded (diffs: ...)`，n 为组内行数。
/// 全组参数与首行完全一致 → `(identical)`；否则差异参数去重保序，最多
/// `DIFFS_MAX` 个，超出补 `+N` 计数。
fn marker(n: usize, params: &[Vec<String>]) -> String {
    let first = &params[0];
    let detail = if params.iter().all(|p| p == first) {
        "(identical)".to_string()
    } else {
        let mut diffs: Vec<String> = Vec::new();
        for p in params.iter().skip(1) {
            for d in p {
                if !diffs.iter().any(|x| x == d) && diffs.len() < DIFFS_MAX {
                    diffs.push(d.clone());
                }
            }
        }
        let mut s = format!("(diffs: {}", diffs.join(", "));
        let total: usize = params.iter().skip(1).map(|p| p.len()).sum();
        if total > diffs.len() {
            s.push_str(&format!(" +{}", total - diffs.len()));
        }
        s.push(')');
        s
    };
    format!("scx-fold: {} rows folded {detail}", n - 1)
}

/// 折叠一个连续块的文本序列：骨架相同且行数 ≥ `min` 的子组压缩，
/// 其余原样。返回折叠后序列（首行原文保留）。
pub(crate) fn collapse(block: &[&str], min: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(block.len());
    let mut i = 0usize;
    while i < block.len() {
        let skel = split(block[i]).0;
        let mut j = i + 1;
        while j < block.len() && split(block[j]).0 == skel {
            j += 1;
        }
        if j - i >= min {
            let params: Vec<Vec<String>> = block[i..j].iter().map(|s| split(s).1).collect();
            out.push(block[i].to_string());
            out.push(marker(j - i, &params));
        } else {
            out.extend(block[i..j].iter().map(|s| s.to_string()));
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_lines_collapse() {
        let block = vec![
            "2026-10-30 12:00:01 ERROR conn timeout id=a1b",
            "2026-10-30 12:00:02 ERROR conn timeout id=b2c",
            "2026-10-30 12:00:03 ERROR conn timeout id=c3d",
        ];
        let out = collapse(&block, FOLD_MIN);
        assert_eq!(out.len(), 2, "3 行折叠为首行+标记");
        assert_eq!(out[0], block[0], "首行原文保留（含首行参数 a1b）");
        assert!(out[1].starts_with("scx-fold: 2 rows folded"));
        assert!(!out[1].contains("a1b"), "首行参数不重复进差异");
        assert!(out[1].contains("b2c"));
        assert!(out[1].contains("c3d"));
    }

    #[test]
    fn below_min_no_collapse() {
        let block = vec!["ERROR conn timeout id=abc", "ERROR conn timeout id=def"];
        let out = collapse(&block, FOLD_MIN);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], block[0]);
        assert_eq!(out[1], block[1]);
    }

    #[test]
    fn identical_params_marker() {
        let block = vec!["tick 5 ok", "tick 5 ok", "tick 5 ok"];
        let out = collapse(&block, FOLD_MIN);
        assert_eq!(out.len(), 2);
        assert!(out[1].contains("(identical)"), "{}", out[1]);
    }

    #[test]
    fn distinct_skeleton_no_collapse() {
        let block = vec!["first line alpha", "second line beta", "third line gamma"];
        let out = collapse(&block, FOLD_MIN);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn mixed_block_partial_fold() {
        let block = vec![
            "ERR conn timeout id=a",
            "ERR conn timeout id=b",
            "ERR conn timeout id=c",
            "unrelated info line",
        ];
        let out = collapse(&block, FOLD_MIN);
        assert_eq!(out.len(), 3, "前 3 折叠为 2，最后 1 原样");
        assert!(out[2].contains("unrelated"));
    }
}
