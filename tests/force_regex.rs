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
    // re: 是单个正则：| 是正则交替，不拆分为多个 --force 模式。
    assert_eq!(forced(&out), 1, "out: {out}");
    assert!(
        out.contains("conn9f2a broken db crash"),
        "err[a-z]+ 分支命中 error 行"
    );
    assert!(out.contains("connX broken db crash"), "connX 分支命中");
}

#[test]
fn re_alt_is_not_split_by_outer_bar() {
    // 回归：re: 条目整体是单个正则，| 是正则交替，不得被外层拆成多模式
    // （否则 `panic in|assert in` 会被拆成 `re:panic in` + 字面 `assert in`，
    // forced 变成 2，交替语义被破坏）。>40 行触发 sieve 而非 passthrough:small。
    let mut lines: Vec<String> = (0..40)
        .map(|i| format!("INFO heartbeat seq={i} ok"))
        .collect();
    lines.push("assert in policy check".to_string());
    lines.push("panic in worker thread".to_string());
    let text = lines.join("\n");
    let (ok, out, _e) = run(
        &[
            "--query",
            "no-match-zzz",
            "--force",
            "re:panic in|assert in",
        ],
        &text,
    );
    assert!(ok);
    assert_eq!(forced(&out), 1, "re: 是单个模式: {out}");
    assert!(out.contains("assert in policy check"), "交替左分支命中");
    assert!(out.contains("panic in worker thread"), "交替右分支命中");
}

#[test]
fn re_class_bar_is_not_split() {
    // 字符类内的 | 是类成员；re: 不拆分后 `[bc|d]` 是单个类，不再误报未闭合。
    // --context 0 只保留 force 命中行；>40 行触发 sieve。
    let mut lines: Vec<String> = (0..40)
        .map(|i| format!("INFO heartbeat seq={i} ok"))
        .collect();
    lines.extend([
        "ab".to_string(),
        "ac".to_string(),
        "ad".to_string(),
        "ax".to_string(),
    ]);
    let text = lines.join("\n");
    let (ok, out, _e) = run(&["--context", "0", "--force", "re:a[bc|d]"], &text);
    assert!(ok, "stderr: {_e}");
    assert_eq!(forced(&out), 1);
    assert!(out.contains("ab") && out.contains("ac") && out.contains("ad"));
    assert!(!out.contains("ax"), "ax 不匹配 a[bc|d]");
}

#[test]
fn invalid_regex_rejected_with_subset_hint() {
    // re: 不再按顶层 | 拆分；空交替 `a||b` 现在也是非法（empty alternative）。
    for bad in ["re:conn[", "re:[z-a]", "re:a**", "re:", "re:a||b"] {
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
