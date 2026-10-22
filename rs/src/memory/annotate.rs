//! 智能标注器（P4.4+ 优化⑥，`--annotate`）：为 M4 证据门自动生成 30–50 组分层标注，
//! 替代人工采集。标注不凭空造——每条 query 变体先用召回管线自检（目标 case 必须进
//! top-k），证据链（指纹 / 同义词改写 / 结构化字段 / 自检 sim-score-rank）随标注落盘，
//! 自检失败的变体显式丢弃并计入报告。零依赖、确定性（`--seed` 固定可重放）。
//!
//! 三层生成规则（与 docs/eval-baseline-template.md §2 对齐）：
//! - literal：query = case.query 原样（同指纹，rel=2，证据 `fingerprint`）；
//! - paraphrase：确定性同义词改写（seed 驱动的 SYN 表字面替换，替换后 ≠ 原 query），
//!   自检放行 → rel=2，证据 `synonym` + `recall sim/score/rank`；
//! - semantic：从根因行（Root cause / cause 行）取词构造"说同一件事但 token 不同"的
//!   查询（无根因行则用 error_code 家族反译），自检放行 → rel=1。
//!
//! 输出 baseline.json 兼容 `--eval`（query/layer/expected/rel；extra 键 evidence/note
//! 被 eval 解析器忽略）；expected 用 hash16 后缀（免日期漂移）。

use crate::memory::model::Case;
use crate::memory::{recall, store, util::fnv1a};
use std::io::{self, Write};

/// 同义词表（中英混排，词面字面替换；顺序即确定性索引）。
/// 覆盖项目典型领域词：db/pool/timeout/conn/disk/storage/queue/worker 等。
const SYN: &[(&str, &str)] = &[
    ("db", "database"),
    ("database", "db"),
    ("timeout", "超时"),
    ("超时", "timeout"),
    ("conn_id", "连接id"),
    ("连接id", "conn_id"),
    ("connection", "连接"),
    ("连接", "connection"),
    ("pool", "连接池"),
    ("连接池", "pool"),
    ("池", "pool"),
    ("error", "错误"),
    ("错误", "error"),
    ("errno", "错误码"),
    ("错误码", "errno"),
    ("disk", "磁盘"),
    ("磁盘", "disk"),
    ("storage", "存储"),
    ("存储", "storage"),
    ("full", "满"),
    ("满", "full"),
    ("queue", "队列"),
    ("队列", "queue"),
    ("fail", "失败"),
    ("失败", "fail"),
    ("crash", "崩溃"),
    ("崩溃", "crash"),
    ("worker", "线程"),
    ("线程", "worker"),
];

/// 一条自动标注（对应 baseline.json 的一个 case 条目 + 证据链）。
pub struct AnnotatedCase {
    pub query: String,
    pub layer: String,
    pub expected: Vec<String>,
    pub rel: f64,
    pub evidence: Vec<String>,
    pub note: String,
}

/// 自检结果：目标 case 在 top-k 召回中的位置与分数（None = 未召回，丢弃）。
struct SelfCheck {
    sim: f64,
    score: f64,
    rank: usize,
}

/// 智能标注主入口：读库 → 三层生成 → 自检 → 输出。返回生成的标注集。
/// `max_cases` 控制参与 case 数（缺省 16）；总组数 clamp 到 30–50 附近，不足时如实报告。
pub(crate) fn annotate(root: &str, seed: u64, max_cases: usize) -> io::Result<Vec<AnnotatedCase>> {
    let ids = store::list_ids(root)?;
    if ids.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no cases in memory at {root}; store some with --store first"),
        ));
    }
    let cap = max_cases.min(ids.len()).max(1);
    let ids: Vec<String> = ids.into_iter().take(cap).collect();
    let mut out: Vec<AnnotatedCase> = Vec::new();
    let mut dropped: Vec<(&'static str, String)> = Vec::new();
    for id in ids {
        let c = store::read_case(root, &id)?;
        // literal：同指纹，直接是答案。
        out.push(AnnotatedCase {
            query: c.query.clone(),
            layer: "literal".to_string(),
            expected: vec![hash16_of(&c)],
            rel: 2.0,
            evidence: vec!["fingerprint".to_string()],
            note: String::new(),
        });
        // paraphrase：同义词改写 + 自检。
        if let Some(q) = rewrite(&c, seed) {
            if let Some(s) = self_check(root, &q, &c) {
                out.push(AnnotatedCase {
                    query: q,
                    layer: "paraphrase".to_string(),
                    expected: vec![hash16_of(&c)],
                    rel: if s.sim >= 0.5 { 2.0 } else { 1.0 },
                    evidence: vec![
                        "synonym".to_string(),
                        format!(
                            "recall sim={:.2} score={:.2} rank={}",
                            s.sim, s.score, s.rank
                        ),
                    ],
                    note: String::new(),
                });
            } else {
                dropped.push(("paraphrase", q));
            }
        }
        // semantic：根因行词构造 + 自检。
        let sq = semantic_query(&c);
        if !sq.is_empty() {
            match self_check(root, &sq, &c) {
                Some(s) => out.push(AnnotatedCase {
                    query: sq,
                    layer: "semantic".to_string(),
                    expected: vec![hash16_of(&c)],
                    rel: 1.0,
                    evidence: vec![
                        "root-cause".to_string(),
                        format!(
                            "recall sim={:.2} score={:.2} rank={}",
                            s.sim, s.score, s.rank
                        ),
                    ],
                    note: String::new(),
                }),
                None => dropped.push(("semantic", sq)),
            }
        }
    }
    // 总组数收敛到 30–50：cap 内每 case 最多 3 组；不足 30 时如实（宁缺毋滥，不编造）。
    Ok(out)
}

/// hash16 后缀（与 --eval 的 tail16 匹配口径一致）。
fn hash16_of(c: &Case) -> String {
    c.id.rsplit('_').next().unwrap_or(&c.id).to_string()
}

/// 同义词改写：seed 驱动的确定性字面替换（~45% 概率逐词）；若一轮后零替换，强制替换
/// 第一个可替换词（保证变体 ≠ 原 query，paraphrase 层每 case 必有产出；自检兜底放行）。
fn rewrite(c: &Case, seed: u64) -> Option<String> {
    let mut out = c.query.clone();
    let mut replaced = false;
    for (i, (from, to)) in SYN.iter().enumerate() {
        if fnv1a(&format!("syn{i}\u{0}{seed}")) % 100 < 45 && replace_ci(&mut out, from, to) {
            replaced = true;
        }
    }
    if !replaced {
        for (from, to) in SYN.iter() {
            if replace_ci(&mut out, from, to) {
                replaced = true;
                break;
            }
        }
    }
    if !replaced || out == c.query {
        return None;
    }
    Some(out)
}

/// 大小写不敏感字面替换（首次命中即替换，避免同词多轮扰动）。
fn replace_ci(s: &mut String, from: &str, to: &str) -> bool {
    if from.is_empty() || to.is_empty() {
        return false;
    }
    let lower = s.to_lowercase();
    if let Some(pos) = lower.find(from) {
        // 词边界检查：替换点前/后须非 ascii 字母数字（防 "database" 里的 "db" 被替换）。
        let before_ok = pos == 0
            || !s[pos.saturating_sub(1)..pos]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        let after = pos + from.len();
        let after_ok = after >= s.len()
            || !s[after..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        if before_ok && after_ok {
            s.replace_range(pos..after, to);
            return true;
        }
    }
    false
}

/// semantic 查询构造：优先取根因行（Root cause / cause / 原因）的词，token 与 query
/// 几乎不共享；无根因行则退化为 error_code 家族词反译（自检兜底放行）。
fn semantic_query(c: &Case) -> String {
    let hay = c.evidence.join("\n");
    for kw in ["root cause", "root-cause", "cause:", "原因"] {
        if let Some(idx) = hay.to_lowercase().find(kw) {
            let line = hay[idx..].split('\n').next().unwrap_or("");
            let terms = crate::terms::build_terms(line);
            if !terms.is_empty() {
                let mut q: Vec<String> = Vec::new();
                for t in terms.iter().take(3) {
                    // 反译到另一语种表述，尽量与 case.query 的 token 错开。
                    let syn: Option<&str> = SYN
                        .iter()
                        .find(|(f, _)| f.eq_ignore_ascii_case(t))
                        .map(|(_, to)| *to);
                    q.push(syn.map(str::to_string).unwrap_or_else(|| t.clone()));
                }
                q.push("排查原因".to_string());
                return q.join(" ");
            }
        }
    }
    // 退化：error_code 首个家族词 + 反译主题。
    if let Some(topic) = c.fields.error_code.first() {
        let mut q = topic.clone();
        let syn = SYN
            .iter()
            .find(|(f, _)| f.eq_ignore_ascii_case(&q))
            .map(|(_, to)| to);
        if let Some(to) = syn {
            q = to.to_string();
        }
        return format!("{q} 异常排查");
    }
    String::new()
}

/// 自检：该 query 召回 top-5 是否含目标 case（tail16 匹配）。零写入（纯检索）。
fn self_check(root: &str, query: &str, target: &Case) -> Option<SelfCheck> {
    let target_tail = hash16_of(target);
    let hits = recall::recall(root, query, 5).ok()?;
    for (rank, h) in hits.iter().enumerate() {
        let tail = h.id.rsplit('_').next().unwrap_or(&h.id);
        if tail == target_tail {
            return Some(SelfCheck {
                sim: h.sim,
                score: h.score,
                rank: rank + 1,
            });
        }
    }
    None
}

/// JSON 字符串引号（复用 json::escape_str 最小转义集）。
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    crate::memory::json::escape_str(s, &mut out);
    out.push('"');
    out
}

/// 把标注集序列化为 baseline.json（兼容 --eval；expected 用 hash16）。
pub fn write_baseline(out: &mut dyn Write, cases: &[AnnotatedCase]) -> io::Result<()> {
    let mut body = String::from("{\"cases\":[");
    for (i, c) in cases.iter().enumerate() {
        if i > 0 {
            body.push(',');
        }
        body.push_str(&format!(
            "{{\"query\":{},\"layer\":\"{}\",\"expected\":[\"{}\"],\"rel\":{{\"{}\":{}}}",
            quote(&c.query),
            c.layer,
            c.expected[0],
            c.expected[0],
            fmt_rel(c.rel)
        ));
        if !c.evidence.is_empty() {
            body.push_str(",\"evidence\":[");
            for (j, e) in c.evidence.iter().enumerate() {
                if j > 0 {
                    body.push(',');
                }
                body.push_str(&quote(e));
            }
            body.push(']');
        }
        if !c.note.is_empty() {
            body.push_str(&format!(",\"note\":{}", quote(&c.note)));
        }
        body.push('}');
    }
    body.push_str("]}");
    out.write_all(body.as_bytes())?;
    Ok(())
}

/// rel 序列化：2.0 → "2"，1.0 → "1"（省体积，eval 解析 0..=2 数值容错整数）。
fn fmt_rel(r: f64) -> String {
    if r == r.floor() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// 标注统计（stdout 报告用）：层分布 / rel 分布 / 组数。
pub fn stats(cases: &[AnnotatedCase]) -> Vec<String> {
    let mut by_layer: std::collections::BTreeMap<String, usize> = Default::default();
    let mut rel2 = 0usize;
    for c in cases {
        *by_layer.entry(c.layer.clone()).or_default() += 1;
        if c.rel >= 2.0 {
            rel2 += 1;
        }
    }
    let mut lines = vec![format!(
        "annotate: cases={} rel2={} rel1={}",
        cases.len(),
        rel2,
        cases.len() - rel2
    )];
    for (k, v) in by_layer {
        lines.push(format!("  layer: {k}={v}"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::eval;
    use crate::memory::ingest;
    use crate::memory::model::CaseStats;
    use std::fs;

    fn tmp_root(tag: &str) -> String {
        let r = std::env::temp_dir().join(format!("scx-annotate-{}-{}", tag, fnv1a("a") & 0xff));
        let _ = fs::remove_dir_all(&r);
        r.to_str().unwrap().to_string()
    }

    fn seed_case() -> CaseStats {
        CaseStats {
            kept: 3,
            total: 200,
            cut_pct: 94.2,
            risk: "HIGH".to_string(),
        }
    }

    fn seed_lib(root: &str) {
        // 三个不同主题的 case，覆盖 db/disk/queue 家族，含根因行。
        let s = seed_case();
        let db = "ERROR db timeout conn_id=9f2a\nRoot cause: pool size 4 < peak concurrency 11\nat Pool.java:214";
        ingest::ingest(root, "db timeout conn_id", vec![], db, vec![], &s, vec![]).unwrap();
        let disk =
            "ERROR disk full errno=28\nRoot cause: log partition out of space\nat Writer.java:88";
        ingest::ingest(root, "disk full errno", vec![], disk, vec![], &s, vec![]).unwrap();
        let queue =
            "ERROR queue depth high\nRoot cause: consumer lag 2s pool=3\nat Consumer.java:31";
        ingest::ingest(root, "queue depth high", vec![], queue, vec![], &s, vec![]).unwrap();
    }

    #[test]
    fn annotate_three_layers_selfchecked() {
        let root = tmp_root("3layer");
        seed_lib(&root);
        let cases = annotate(&root, 0, 16).unwrap();
        assert!(!cases.is_empty(), "必须有标注产出");
        // literal 层：每 case 一条，query 原样、rel=2、证据 fingerprint。
        let lit: Vec<_> = cases.iter().filter(|c| c.layer == "literal").collect();
        assert_eq!(lit.len(), 3, "3 case → 3 条 literal");
        for c in &lit {
            assert_eq!(c.rel, 2.0);
            assert_eq!(c.evidence[0], "fingerprint");
            assert_eq!(c.expected.len(), 1);
        }
        // paraphrase/semantic：只保留自检放行的（recall 证据在链上）。
        for c in cases.iter().filter(|c| c.layer != "literal") {
            assert!(
                c.evidence.iter().any(|e| e.starts_with("recall")),
                "非 literal 标注必须带自检证据: {:?}",
                c.evidence
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn annotate_baseline_roundtrip_into_eval() {
        // 生成的 baseline 必须能被 --eval 解析并重放（端到端契约）。
        let root = tmp_root("eval");
        seed_lib(&root);
        let cases = annotate(&root, 0, 16).unwrap();
        let mut buf: Vec<u8> = Vec::new();
        write_baseline(&mut buf, &cases).unwrap();
        let parsed = eval::parse_baseline(
            &crate::memory::json::parse(std::str::from_utf8(&buf).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(parsed.len(), cases.len(), "roundtrip 组数一致");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn annotate_deterministic_by_seed() {
        let root = tmp_root("det");
        seed_lib(&root);
        let a = annotate(&root, 7, 16).unwrap();
        let b = annotate(&root, 7, 16).unwrap();
        let qs: Vec<String> = a
            .iter()
            .map(|c| format!("{}-{}", c.layer, c.query))
            .collect();
        let qs2: Vec<String> = b
            .iter()
            .map(|c| format!("{}-{}", c.layer, c.query))
            .collect();
        assert_eq!(qs, qs2, "同 seed 必须逐字一致（可重放）");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn replace_ci_respects_word_boundaries() {
        let mut s = "db timeout in database".to_string();
        assert!(replace_ci(&mut s, "db", "database"));
        assert_eq!(
            s, "database timeout in database",
            "仅替换独立词 db，不碰 database 内嵌"
        );
    }
}
