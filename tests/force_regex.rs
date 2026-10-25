//! `--force` re: 正则子集集成测试（P3.3）。
//! 验证：`re:` 前缀进入手写有限正则（交替/字符类/量词/转义），命中即证据；
//! 非法正则 CLI 层报错（含支持子集提示）；字面子串行为与既有 golden 一致。

use std::io::Write;
use std::process::{Command, Stdio};

fn scx() -> &'static str {
    env!("CARGO_BIN_EXE_scx")
}

/// 300 行噪音 + 两个信号行（conn9f2a 数字连接 / connX 非数字）。
fn corpus() -> String {
    let mut out = Vec::new();
    for i in 0..300 {
        out.push(format!("INFO heartbeat seq={i} pool=3 latency=1ms ok"));
    }
    out.push("ERROR conn9f2a broken db crash".to_string());
    out.push("ERROR connX broken db crash".to_string());
    out.join("\n")
}

fn run(args: &[&str], stdin_txt: &str) -> (bool, String, String) {
    let mut child = Command::new(scx())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn scx");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_txt.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn forced(out: &str) -> usize {
    // 兼容两形态统计行：`compressed: ... forced=N` 与 `scxjson: {"forced":N,...}`。
    let line = out
        .lines()
        .find(|l| l.contains("forced=") || l.contains("\"forced\":"))
        .expect("stats line");
    if let Some(f) = line.split("forced=").nth(1) {
        f.split(' ').next().unwrap().parse().unwrap()
    } else {
        line.split("\"forced\":")
            .nth(1)
            .and_then(|s| s.split([',', '}']).next())
            .and_then(|s| s.trim().parse().ok())
            .expect("json forced")
    }
}

#[test]
fn re_pattern_hits_evidence_line() {
    let (ok, out, _e) = run(
        &[
            "--query",
            "no-match-zzz",
            "--force",
            "re:conn[0-9]+",
            "--json",
        ],
        &corpus(),
    );
    assert!(ok, "out: {out}");
    // conn9f2a 命中（数字），connX 不命中；forced = 1，sieve 模式。
    assert_eq!(forced(&out), 1, "out: {out}");
    let stat = out
        .lines()
        .find(|l| l.starts_with("scxjson:") || l.starts_with("compressed:"))
        .expect("stats line");
    assert!(
        stat.contains("forced=1") && stat.contains("mode=sieve"),
        "stat: {stat}"
    );
    // conn9f2a 行保留（含原文证据）。
    assert!(out.contains("conn9f2a broken db crash"));
}

#[test]
fn re_alt_and_quant_hits_both() {
    let (ok, out, _e) = run(
        &["--query", "no-match-zzz", "--force", "re:err[a-z]+|connX"],
        &corpus(),
    );
    assert!(ok);
    assert_eq!(forced(&out), 2, "out: {out}");
    assert!(out.contains("connX broken db crash"));
}

#[test]
fn invalid_regex_rejected_with_subset_hint() {
    // `re:a||b` 合法：顶层 `|` 拆分后每段独立（re:a + b 字面）。
    for bad in ["re:conn[", "re:[z-a]", "re:a**", "re:"] {
        let (ok, _out, err) = run(&["--query", "x", "--force", bad], "line one\n");
        assert!(!ok, "must reject {bad:?}");
        assert!(
            err.contains("subset")
                || err.contains("empty")
                || err.contains("must not be empty")
                || err.contains("inverted range"),
            "stderr for {bad:?}: {err}"
        );
        assert!(err.contains("--force"), "err names flag: {err}");
    }
}

#[test]
fn literal_substring_behavior_unchanged() {
    // 非 re: 条目：字面子串（小写化），行为与 v0.4.1 一致。
    let (ok, out, _e) = run(&["--query", "no-match-zzz", "--force", "connx"], &corpus());
    assert!(ok);
    // connx 字面命中 connX 行（小写匹配），conn9f2a 也含 conn 但不含 connx → forced=1。
    assert_eq!(forced(&out), 1, "out: {out}");
}
