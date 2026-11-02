//! `xtask audit` / `xtask a2`：质量门禁。
//!
//! - `audit`：扫描 `src/**/*.rs` 生产段（首个测试标记之前），禁止裸 unwrap、空 expect、空 panic/unreachable；`scope.join()` 后的 unwrap 豁免。
//! - `a2`：按 `config/bench/fidelity_thresholds.json` 校验
//!   `config/bench/results/p11_a_results.json` 的 round-trip 保真分档。
//!
//! 两者语义与原 Python 工具的 `audit` / `check a2` 对齐。

// 扫描/门禁分支线性；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::io;
use super::{fail, Fail};
use scx::json::Json;
use std::fs;
use std::path::{Path, PathBuf};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
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
    let results = Path::new(ROOT).join("config/bench/results/p11_a_results.json");
    let thresholds = Path::new(ROOT).join("config/bench/fidelity_thresholds.json");
    let res = load(&results)?;
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

fn load(path: &Path) -> Result<Json, Fail> {
    let text = fs::read_to_string(path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    scx::json::parse(&text).map_err(|e| fail(format!("{}: {e}", path.display())))
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
}
