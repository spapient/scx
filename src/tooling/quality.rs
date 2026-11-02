//! `xtask audit` / `a2` / `selftest`：质量门禁与不变量自检（feature = "eval"）。
//!
//! - `audit`：扫描 `src/**/*.rs` 生产段（首个测试标记之前），禁止裸 unwrap、空 expect、空 panic/unreachable；`scope.join()` 后的 unwrap 豁免。
//! - `a2`：按 `config/fidelity-thresholds.json` 校验结果包中
//!   `compression-fidelity.json` 的 round-trip 保真分档。
//! - `selftest`：校验记忆目录安全、语料结构、随机采样、统计指标、
//!   manifest 冻结/漂移与 Holm-Bonferroni 决策。
//!
//! 所有检查共享同一质量模块，避免门禁实现分散。

// 扫描/门禁分支线性；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::{contain, corpus, fail, frozen, io, load_json as load, nanos, stats, Fail};
use scx::json::Json;
use std::fs;
use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
// 用 concat! 构造字面量，避免审计扫描到实现自身的模式字符串。
const UNWRAP: &str = concat!(".un", "wrap()");
const EXPECT_EMPTY: &str = concat!(".exp", "ect()");
const EXPECT_OPEN: &str = concat!(".exp", "ect(");
const PANIC: &str = concat!("pan", "ic!(");
const UNREACHABLE: &str = concat!("unreach", "able!(");
const JOIN_UNWRAP: &str = concat!(".join()", ".un", "wrap()");

pub(crate) fn audit() -> Result<(), Fail> {
    let src = Path::new(ROOT).join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    files.sort();
    let (fails, totals) = audit_files(&files);
    println!(
        "生产路径统计: unwrap={} expect={} panic/unreachable={}",
        totals.0, totals.1, totals.2
    );
    if fails > 0 {
        println!("audit_unwrap: {fails} 处未达标");
        Err(Fail {
            msg: String::new(),
            code: 1,
        })
    } else {
        println!("audit_unwrap: OK（生产裸 unwrap=0，expect/panic 消息非空）");
        Ok(())
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn audit_files(files: &[PathBuf]) -> (usize, (usize, usize, usize)) {
    let mut fails = 0usize;
    let mut total_unwrap = 0usize;
    let mut total_expect = 0usize;
    let mut total_panic = 0usize;
    for path in files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let prod = prod_part(&text);
        let rel = path
            .strip_prefix(ROOT)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        for start in find_all(prod, UNWRAP) {
            let lower = char_boundary(prod, start.saturating_sub(40));
            let snippet = prod[lower..start + UNWRAP.len()].trim();
            if snippet.contains(JOIN_UNWRAP) {
                continue;
            }
            fails += 1;
            println!(
                "裸 unwrap（需 expect 语义消息）: {rel}:{}  {}",
                line(prod, start),
                py_repr(snippet)
            );
        }
        for start in find_all(prod, EXPECT_EMPTY) {
            fails += 1;
            println!("空 expect 消息: {rel}:{}", line(prod, start));
        }
        for (needle, _) in [(PANIC, "panic"), (UNREACHABLE, "unreachable")] {
            for start in empty_call(prod, needle) {
                fails += 1;
                println!("空 panic!/unreachable! 消息: {rel}:{}", line(prod, start));
            }
        }
        total_unwrap += find_all(prod, UNWRAP).len();
        total_expect += expect_with_space(prod).len();
        total_panic += empty_call(prod, PANIC).len() + empty_call(prod, UNREACHABLE).len();
    }
    (fails, (total_unwrap, total_expect, total_panic))
}

fn prod_part(text: &str) -> &str {
    let cut = ["#[cfg(test)]", "#[test]", "mod tests {"]
        .iter()
        .filter_map(|marker| text.find(marker))
        .min()
        .unwrap_or(text.len());
    &text[..cut]
}

fn find_all(text: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(index) = text[from..].find(needle) {
        let start = from + index;
        out.push(start);
        from = start + needle.len();
    }
    out
}

/// `.expect(` + 空白 + `)`：对应 Python `AU_EXPECT` 的统计口径。
fn expect_with_space(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(index) = text[from..].find(EXPECT_OPEN) {
        let start = from + index;
        let rest = &text[start + EXPECT_OPEN.len()..];
        let whitespace = rest.len() - rest.trim_start().len();
        if rest[whitespace..].starts_with(')') {
            out.push(start);
        }
        from = start + EXPECT_OPEN.len();
    }
    out
}

/// `panic!(` / `unreachable!(` + 空白 + `)`。
fn empty_call(text: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(index) = text[from..].find(needle) {
        let start = from + index;
        let rest = &text[start + needle.len()..];
        let whitespace = rest.len() - rest.trim_start().len();
        if rest[whitespace..].starts_with(')') {
            out.push(start);
        }
        from = start + needle.len();
    }
    out
}

fn line(text: &str, byte: usize) -> usize {
    text[..byte].matches('\n').count() + 1
}

fn char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Python `repr(str)` 近似：snippet 输出用。
fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

pub(crate) fn a2() -> Result<(), Fail> {
    let thresholds = Path::new(ROOT).join("config/fidelity-thresholds.json");
    let text = frozen::result_text("compression-fidelity.json")?;
    let res = scx::json::parse(&text).map_err(fail)?;
    let cfg = load(&thresholds)?;
    let table = match res.get("a2_roundtrip") {
        Some(Json::Arr(rows)) => rows,
        _ => return Err(fail("a2: missing a2_roundtrip array")),
    };
    let bands = match cfg.get("bands") {
        Some(Json::Obj(bands)) => bands,
        _ => return Err(fail("a2: missing bands object")),
    };
    let mut ok = true;
    for (band, spec) in bands {
        let sets = match spec.get("sets") {
            Some(Json::Arr(sets)) => sets,
            _ => continue,
        };
        let metrics: Vec<(&String, f64)> = match spec {
            Json::Obj(entries) => entries
                .iter()
                .filter(|(key, _)| matches!(key.as_str(), "num_cov" | "token_cov" | "bleu4_prec"))
                .filter_map(|(key, value)| value.num().map(|number| (key, number)))
                .collect(),
            _ => Vec::new(),
        };
        for set in sets {
            let Some(set) = set.str() else {
                continue;
            };
            let Some(row) = table
                .iter()
                .find(|row| row.get("set").and_then(Json::str) == Some(set))
            else {
                println!("  [fidelity:{band}] {set}: MISSING in a2_roundtrip");
                ok = false;
                continue;
            };
            for (metric, threshold) in &metrics {
                let got = row.get(metric.as_str()).and_then(Json::num).unwrap_or(-1.0);
                if got < *threshold {
                    println!(
                        "  [fidelity:{band}] {set} {metric}={got:.3} < {}  FAIL",
                        io::repr(*threshold)
                    );
                    ok = false;
                } else {
                    println!(
                        "  [fidelity:{band}] {set} {metric}={got:.3} >= {}  ok",
                        io::repr(*threshold)
                    );
                }
            }
        }
    }
    println!(
        "  {}",
        if ok {
            "OK: A2 保真断言全部通过"
        } else {
            "FAIL: A2 保真断言未通过"
        }
    );
    if ok {
        Ok(())
    } else {
        Err(Fail {
            msg: String::new(),
            code: 1,
        })
    }
}

pub(crate) fn selftest() -> Result<(), Fail> {
    match selftest_checks() {
        Ok(()) => {
            println!("selftest: PASS");
            Ok(())
        }
        Err(message) => {
            println!("selftest: FAIL: {message}");
            Err(Fail {
                msg: String::new(),
                code: 1,
            })
        }
    }
}

fn selftest_checks() -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("scx-selftest-{}-{}", std::process::id(), nanos()));
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let result = selftest_inner(&dir);
    let _ = fs::remove_dir_all(&dir);
    result
}

fn selftest_inner(dir: &Path) -> Result<(), String> {
    memory_checks(dir)?;
    corpus_checks()?;
    sampling_checks()?;
    stats_checks(dir)?;
    manifest_checks(dir)?;
    holm_checks()
}

fn memory_checks(dir: &Path) -> Result<(), String> {
    check(
        !corpus::is_memory_dir(Path::new(ROOT)),
        "ROOT must not be a memory dir",
    )?;
    let path = dir.join("memory");
    check(!corpus::is_memory_dir(&path), "empty dir is not memory")?;
    fs::create_dir_all(path.join("cases")).map_err(|e| e.to_string())?;
    check(
        !corpus::is_memory_dir(&path),
        "cases dir alone is not memory",
    )?;
    fs::write(path.join("meta.toml"), "schema = 2\n").map_err(|e| e.to_string())?;
    check(corpus::is_memory_dir(&path), "meta+cases is memory")?;
    fs::write(path.join("stale"), "remove\n").map_err(|e| e.to_string())?;
    let prepared = corpus::prepare_mem_dir(path.to_str().unwrap_or_default(), true)?;
    check(prepared.is_dir(), "prepared dir exists")?;
    check(
        fs::read_dir(&prepared)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false),
        "force clear removes contents",
    )?;
    let foreign = dir.join("foreign");
    fs::create_dir_all(&foreign).map_err(|e| e.to_string())?;
    fs::write(foreign.join("keep"), "keep\n").map_err(|e| e.to_string())?;
    let error = corpus::prepare_mem_dir(foreign.to_str().unwrap_or_default(), true)
        .expect_err("--force must reject non-memory");
    check(error.contains("non-memory"), "non-memory rejection")?;
    check(
        fs::read_to_string(foreign.join("keep"))
            .map(|text| text == "keep\n")
            .unwrap_or(false),
        "foreign dir untouched",
    )
}

fn corpus_checks() -> Result<(), String> {
    for (selector, expected) in [
        ("corpus:gauge", 150usize),
        ("corpus:v2", 150),
        ("corpus:real", 40),
    ] {
        let value = scx::assets::read_json(selector)?;
        let layers = corpus::validate_corpus(&value)?;
        check(
            layers.iter().sum::<usize>() == expected,
            &format!("{selector} corpus cases={expected}"),
        )?;
    }
    let fidelity = scx::assets::read_json("corpus:fidelity")?;
    let slices = match fidelity.get("slices") {
        Some(Json::Arr(items)) => items,
        _ => return Err("fidelity corpus missing slices".to_string()),
    };
    check(slices.len() == 40, "fidelity slices=40")?;
    for kind in [
        "build", "stack", "diff", "log", "test", "db", "network", "hardware",
    ] {
        let count = slices
            .iter()
            .filter(|slice| slice.get("kind").and_then(Json::str) == Some(kind))
            .count();
        check(count == 5, &format!("fidelity kind {kind}=5"))?;
    }
    Ok(())
}

fn sampling_checks() -> Result<(), String> {
    let sample = "alpha line\nbeta line\nseed line evidence\ngamma noise\n";
    let big = contain::random_sample(sample, sample.len(), "z", "s1");
    check(big == sample, "full budget returns verbatim input")?;
    let small = contain::random_sample(sample, 10, "z", "s1");
    check(small.len() == 10, "small budget is byte-exact")?;
    check(
        small == contain::random_sample(sample, 10, "z", "s1"),
        "sampling deterministic",
    )?;
    check(
        small != contain::random_sample(sample, 10, "z2", "s1"),
        "seed affects sampling",
    )?;
    let trunc = contain::trunc(sample, 10);
    check(
        trunc == String::from_utf8_lossy(&sample.as_bytes()[..10]).into_owned(),
        "trunc is byte prefix",
    )
}

fn stats_checks(dir: &Path) -> Result<(), String> {
    let verbose = dir.join("verbose.txt");
    fs::write(
        &verbose,
        "case 1/1 q=\"database timeout\"\n  A: 1) abcdef12 sim=0.90 score=9.00 [hit]\n  B: (no hits)\n",
    )
    .map_err(|e| e.to_string())?;
    let parsed = stats::parse_verbose(verbose.to_str().unwrap_or_default(), 1)?;
    check(
        parsed[0]
            .a
            .as_ref()
            .is_some_and(|ids| ids == &["abcdef12".to_string()]),
        "verbose A ids",
    )?;
    check(
        parsed[0].b.as_ref().is_some_and(|ids| ids.is_empty()),
        "verbose B empty",
    )?;
    check(stats::mrr(2) == 0.5, "mrr(2)=0.5")?;
    check(stats::recall_at(0, 1) == 0.0, "recall miss")?;
    check(stats::recall_at(1, 1) == 1.0, "recall hit")?;
    check(stats::recall_at(4, 3) == 0.0, "recall beyond k")?;
    check(stats::recall_at(10, 10) == 1.0, "recall at k")?;
    check(
        stats::jaccard("database timeout", "database timeout") == 1.0,
        "jaccard identical",
    )?;
    let case = stats::Case {
        query: "q".to_string(),
        layer: "literal".to_string(),
        expected: vec!["a".to_string()],
        rel: vec![("a".to_string(), 2.0), ("b".to_string(), 1.0)],
        case_index: None,
    };
    let ids = vec!["a".to_string(), "b".to_string()];
    check(stats::ndcg(&case, &ids) == 1.0, "ndcg ideal=1")?;
    let rows = [
        stats::Row {
            i: 0,
            layer: "literal".to_string(),
            tier: "hard".to_string(),
            a: 1.0,
            b: 0.0,
        },
        stats::Row {
            i: 1,
            layer: "literal".to_string(),
            tier: "hard".to_string(),
            a: 0.0,
            b: 0.0,
        },
    ];
    let refs: Vec<&stats::Row> = rows.iter().collect();
    let plan = stats::Plan {
        boot: 200,
        perm: 200,
        seed: 7,
        mde: 0.05,
    };
    let first = stats::analyze("all", &refs, plan).ok_or("analyze returned None")?;
    let second = stats::analyze("all", &refs, plan).ok_or("analyze returned None")?;
    check(
        first.delta == 0.5 && second.delta == first.delta,
        "analyze delta",
    )
}

fn manifest_checks(dir: &Path) -> Result<(), String> {
    let payload = dir.join("payload.json");
    fs::write(&payload, "{\"a\":1}\n").map_err(|e| e.to_string())?;
    let manifest = dir.join("manifest.json");
    let payload_text = payload.to_string_lossy().to_string();
    let manifest_text = manifest.to_string_lossy().to_string();
    frozen::manifest(&[
        "--files".to_string(),
        payload_text,
        "--out".to_string(),
        manifest_text.clone(),
    ])
    .map_err(|failure| failure.msg)?;
    frozen::manifest(&["--expect".to_string(), manifest_text.clone()])
        .map_err(|failure| failure.msg)?;
    fs::write(&payload, "{\"a\":2}\n").map_err(|e| e.to_string())?;
    let drifted = frozen::manifest(&["--expect".to_string(), manifest_text]);
    check(
        drifted.err().map(|failure| failure.code) == Some(2),
        "manifest drift exits 2",
    )
}

fn holm_checks() -> Result<(), String> {
    let cases: [(&[f64], &[bool]); 5] = [
        (&[0.001, 0.04], &[true, true]),
        (&[0.01, 0.03], &[true, true]),
        (&[0.03, 0.04], &[false, false]),
        (&[0.02, 0.03], &[true, true]),
        (&[0.5, 0.001], &[false, true]),
    ];
    for (ps, want) in cases {
        check(
            frozen::holm_bonferroni(ps).as_slice() == want,
            "holm decision",
        )?;
    }
    Ok(())
}

fn check(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prod_part_cuts_at_first_test_marker() {
        let text = "fn a() { x.unwrap(); }\n#[cfg(test)]\nfn b() { y.unwrap(); }\n";
        assert_eq!(prod_part(text), "fn a() { x.unwrap(); }\n");
    }

    #[test]
    fn py_repr_matches_python_quoting() {
        assert_eq!(py_repr("x.unwrap()"), "'x.unwrap()'");
        assert_eq!(py_repr("a'b"), "\"a'b\"");
    }

    #[test]
    fn audit_counts_and_failures() {
        let dir = crate::testsupport::tmpdir("audit");
        let src = PathBuf::from(&dir);
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("a.rs"),
            "fn a() { x.unwrap(); scope.join().unwrap(); y.expect(); panic!(); }\n\
             #[cfg(test)]\nfn b() { z.unwrap(); }\n",
        )
        .unwrap();
        let mut files = Vec::new();
        walk(&src, &mut files);
        let (fails, totals) = audit_files(&files);
        assert_eq!(fails, 3); // bare unwrap + empty expect + bare panic
        assert_eq!(totals, (2, 1, 1)); // unwrap includes allowed .join()
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn selftest_runs() {
        super::selftest_checks().expect("selftest");
    }
}
