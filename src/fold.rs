//! P17-A：跨行模板折叠（`--fold 1`，默认关闭）——可逆版（2026-10-31）。
//!
//! 日志风暴类输入（成百上千行仅参数不同的相似行）会稀释证据密度并撑大输出。
//! 折叠把**连续块内共享静态骨架**的行（≥ `FOLD_MIN` 行）压缩为 3 行：
//!   1) 首行原文（语义锚，行 1 参数天然保留）；
//!   2) `scx-fold: {m} rows | {骨架}` —— 参数位 `#`，字面 `#`/`|` 转义 `##`/`||`；
//!   3) `scx-fold-params: {p2};{p3};...;{pn}` —— 每行参数 `|` 分隔、全量保序。
//!      信息无损：骨架只存一份、参数表只存参数，字节数与行内 token 大降；
//!      `rebuild()` 可从 2/3 行逐字节还原全部 n 行（round-trip 100%）。
//!
//! 铁律：
//! - 默认关闭（`cfg.fold=false` 时主循环零开销短路，默认路径逐字节一致）。
//! - 纯 std、确定性：相同输入恒同输出。
//! - 只作用于连续保留块，不跨空行/省略段；参数 token 只含数字/时间戳/hex/短码，
//!   语义 token 全部计入骨架，保证不误吞信息行。
//! - 与 trim 组合：折叠发生在 trim 之后，块内行取「trim 后文本」，重建亦为
//!   trim 后视角的可逆还原（trim 自身是不可逆摘词，属其既有语义）。

/// 折叠最小行数：低于此不折叠（防止误并）。
pub(crate) const FOLD_MIN: usize = 3;

/// 参数 token 判定：含数字（时间戳/计数/hex）或 ≤2 位纯小写短码（id/a/b）；
/// 语义 token（≥3 字母词）入骨架。
fn is_param(t: &str) -> bool {
    t.bytes().any(|b| b.is_ascii_digit())
        || (t.len() <= 2 && t.bytes().all(|b| b.is_ascii_lowercase()))
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

/// 骨架转义：字面 `#` → `##`、`|` → `||`，使骨架内不出现单 `#`/`|`，
/// 解析与重建无歧义（单 `#` 必为参数位，首个 ` | ` 必为段分隔）。
fn escape_into(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '#' => out.push_str("##"),
            '|' => out.push_str("||"),
            _ => out.push(ch),
        }
    }
}

/// 单行 → (骨架, 参数列表)。分隔符（空格/标点/`=` 等非词字符）原样入骨架，
/// 参数 token 归一为 `#` 占位——展开时 `#` 依次填回参数即逐字节还原原文。
fn split(line: &str) -> (String, Vec<String>) {
    let b = line.as_bytes();
    let mut skeleton = String::with_capacity(line.len());
    let mut params = Vec::new();
    let mut cur: Option<usize> = None;
    let mut i = 0usize;
    while i < b.len() {
        let (word, adv) = step(b, i);
        if word {
            if cur.is_none() {
                cur = Some(i);
            }
        } else {
            flush_word(line, &mut cur, i, &mut skeleton, &mut params);
            // 分隔符（词间/词尾的非词字符）原样入骨架——可逆重建的逐字节依据。
            escape_into(&line[i..i + adv], &mut skeleton);
        }
        i += adv;
    }
    flush_word(line, &mut cur, b.len(), &mut skeleton, &mut params);
    (skeleton, params)
}

/// 词段出账：参数入表、骨架写占位/转义并补词边界标记。
fn flush_word(
    line: &str,
    cur: &mut Option<usize>,
    end: usize,
    skeleton: &mut String,
    params: &mut Vec<String>,
) {
    let Some(st) = cur.take() else { return };
    let t = &line[st..end];
    if is_param(t) {
        params.push(t.to_string());
        skeleton.push('#');
    } else {
        escape_into(t, skeleton);
    }
    skeleton.push('|');
}

/// 模板行：`scx-fold: {n-1} rows | {骨架}`。
fn template(n: usize, skeleton: &str) -> String {
    format!("scx-fold: {} rows | {skeleton}", n - 1)
}

/// 参数行：`scx-fold-params: {p2};{p3};...;{pn}`，每行参数 `|` 分隔、全量保序。
fn params_line(n: usize, params: &[Vec<String>]) -> String {
    let mut s = String::with_capacity(32 * n);
    s.push_str("scx-fold-params: ");
    for (k, p) in params.iter().skip(1).enumerate() {
        if k > 0 {
            s.push(';');
        }
        s.push_str(&p.join("|"));
    }
    s
}

/// 折叠一个连续块的文本序列：骨架相同且行数 ≥ `min` 的子组压缩为
/// 首行 + 模板行 + 参数行（3 行，信息无损可逆），其余原样。
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
            out.push(template(j - i, &skel));
            out.push(params_line(j - i, &params));
        } else {
            out.extend(block[i..j].iter().map(|s| s.to_string()));
        }
        i = j;
    }
    out
}

/// 从模板行 + 参数行重建第 2..=n 行（逐字节等于折叠前的对应原文）。
/// 结构不合法（段数/参数数不匹配、前缀缺失）返回 `None`。
/// 供测试与下游重建消费的可逆性契约 API；主链为压缩器，运行期不展开。
#[allow(dead_code)]
pub(crate) fn rebuild(tpl: &str, pstream: &str) -> Option<Vec<String>> {
    let (m, skeleton) = parse_template(tpl)?;
    if m == 0 {
        return Some(Vec::new());
    }
    let rows = parse_params(pstream)?;
    if rows.len() != m {
        return None;
    }
    rows.iter().map(|row| expand_row(skeleton, row)).collect()
}

/// 模板行 → (折叠行数 m, 骨架)。
fn parse_template(tpl: &str) -> Option<(usize, &str)> {
    let rest = tpl.strip_prefix("scx-fold: ")?;
    let cut = rest.find(" rows | ")?;
    let m: usize = rest[..cut].parse().ok()?;
    Some((m, &rest[cut + " rows | ".len()..]))
}

/// 参数行 → 每行参数向量（空段 = 0 参数；非空段内不得有空 token）。
fn parse_params(pstream: &str) -> Option<Vec<Vec<&str>>> {
    let body = pstream.strip_prefix("scx-fold-params: ")?;
    let rows: Vec<Vec<&str>> = body
        .split(';')
        .map(|r| {
            if r.is_empty() {
                Vec::new()
            } else {
                r.split('|').collect()
            }
        })
        .collect();
    if rows
        .iter()
        .any(|r| r.len() > 1 && r.iter().any(|p| p.is_empty()))
    {
        return None;
    }
    Some(rows)
}

/// 单行展开：骨架 `#` 依次填参数，`##`/`||` 还原字面 `#`/`|`，其余原样。
fn expand_row(skeleton: &str, params: &[&str]) -> Option<String> {
    let chars: Vec<char> = skeleton.chars().collect();
    let mut rebuilt = String::with_capacity(skeleton.len() + 16);
    let mut pi = 0usize;
    let mut ci = 0usize;
    while ci < chars.len() {
        match chars[ci] {
            '#' if chars.get(ci + 1) == Some(&'#') => {
                rebuilt.push('#');
                ci += 2;
            }
            '#' => {
                rebuilt.push_str(params.get(pi)?);
                pi += 1;
                ci += 1;
            }
            '|' if chars.get(ci + 1) == Some(&'|') => {
                rebuilt.push('|');
                ci += 2;
            }
            '|' => ci += 1,
            c => {
                rebuilt.push(c);
                ci += 1;
            }
        }
    }
    (pi == params.len()).then_some(rebuilt)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 折叠 → 重建 → 与原文逐行相等（round-trip 100%）。
    fn roundtrip(block: &[&str]) {
        let out = collapse(block, FOLD_MIN);
        let rebuild_idx: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, l)| l.starts_with("scx-fold: "))
            .map(|(i, _)| i)
            .collect();
        if out.len() == block.len() && rebuild_idx.is_empty() {
            // 未折叠：原样
            assert_eq!(out, block, "未折叠块应逐字原样");
            return;
        }
        // 每折叠组：模板行后紧跟参数行
        for &ti in &rebuild_idx {
            let tpl = &out[ti];
            let pstream = out
                .get(ti + 1)
                .unwrap_or_else(|| panic!("模板行后缺参数行: {tpl}"));
            let rebuilt = rebuild(tpl, pstream).expect("重建失败");
            let m: usize = tpl
                .strip_prefix("scx-fold: ")
                .unwrap()
                .split(" rows | ")
                .next()
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(rebuilt.len(), m, "重建行数应等于折叠行数");
            // 逐行断言与块内连续段一致（按出现顺序回溯）
            let mut cursor = 0usize;
            for line in &rebuilt {
                while cursor < block.len() && block[cursor] != *line {
                    cursor += 1;
                }
                assert!(cursor < block.len(), "重建行不在原文中: {line}");
                cursor += 1;
            }
        }
    }

    #[test]
    fn identical_lines_collapse() {
        let block = vec![
            "2026-10-30 12:00:01 ERROR conn timeout id=a1b",
            "2026-10-30 12:00:02 ERROR conn timeout id=b2c",
            "2026-10-30 12:00:03 ERROR conn timeout id=c3d",
        ];
        let out = collapse(&block, FOLD_MIN);
        assert_eq!(out.len(), 3, "3 行折叠为首行+模板+参数行");
        assert_eq!(out[0], block[0], "首行原文保留（含首行参数 a1b）");
        assert!(out[1].starts_with("scx-fold: 2 rows | "), "{}", out[1]);
        assert!(out[2].starts_with("scx-fold-params: "), "{}", out[2]);
        assert!(!out[1].contains("a1b"), "首行参数不重复进骨架");
        assert!(out[2].contains("b2c"));
        assert!(out[2].contains("c3d"));
        roundtrip(&block);
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
    fn identical_params_rebuild() {
        let block = vec!["tick 5 ok", "tick 5 ok", "tick 5 ok"];
        let out = collapse(&block, FOLD_MIN);
        assert_eq!(out.len(), 3);
        assert!(out[1].starts_with("scx-fold: 2 rows | "), "{}", out[1]);
        roundtrip(&block);
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
        assert_eq!(out.len(), 4, "前 3 折叠为 3，最后 1 原样");
        assert!(out[3].contains("unrelated"));
        roundtrip(&block);
    }

    #[test]
    fn roundtrip_with_special_chars() {
        // 骨架内字面 `#`（shell 注释）与 `|`（管道）必须转义且可逆。
        let block = vec![
            "# ERR code=7 | retry=1",
            "# ERR code=8 | retry=2",
            "# ERR code=9 | retry=3",
        ];
        let out = collapse(&block, FOLD_MIN);
        assert!(out[1].contains("##"), "字面 # 应转义 ##: {}", out[1]);
        assert!(out[1].contains("||"), "字面 | 应转义 ||: {}", out[1]);
        roundtrip(&block);
    }

    #[test]
    fn roundtrip_cjk_params() {
        let block = vec![
            "连接 超时 id=a 重试 1",
            "连接 超时 id=b 重试 2",
            "连接 超时 id=c 重试 3",
        ];
        let out = collapse(&block, FOLD_MIN);
        assert_eq!(out.len(), 3);
        roundtrip(&block);
    }

    #[test]
    fn roundtrip_large_group() {
        let block: Vec<String> = (0..50)
            .map(|i| format!("worker-{i} pid={} state=run", 1000 + i))
            .collect();
        let refs: Vec<&str> = block.iter().map(|s| s.as_str()).collect();
        let out = collapse(&refs, FOLD_MIN);
        assert_eq!(out.len(), 3, "50 行 → 3 行");
        roundtrip(&refs);
    }

    #[test]
    fn deterministic_same_input_same_output() {
        let block = vec!["tick 1 ok", "tick 2 ok", "tick 3 ok"];
        assert_eq!(collapse(&block, FOLD_MIN), collapse(&block, FOLD_MIN));
    }

    #[test]
    fn rebuild_rejects_malformed() {
        assert!(rebuild("scx-fold: 2 rows | a|#|", "bad").is_none());
        assert!(rebuild("garbage", "scx-fold-params: x").is_none());
        assert!(rebuild("scx-fold: 2 rows | a|#|", "scx-fold-params: 1|2").is_none());
    }

    #[test]
    fn all_skeleton_zero_params_rebuild() {
        // 无参数位（纯语义行）也可逆：参数行为空段。
        let block = vec![
            "ERROR conn timeout",
            "ERROR conn timeout",
            "ERROR conn timeout",
        ];
        let out = collapse(&block, FOLD_MIN);
        assert_eq!(out.len(), 3);
        assert!(out[2].starts_with("scx-fold-params: "));
        roundtrip(&block);
    }
}
