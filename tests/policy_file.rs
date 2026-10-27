//! `--policy-file` 集成测试（P3.4）。
//! 验证：文本策略覆盖 DEFAULT；优先级 CLI 显式参数 > 文件 > DEFAULT；
//! 非法文件报错含行号；不传时默认路径逐字节不变（golden 37 已覆盖）。

use std::io::Write;
use std::process::{Command, Stdio};

fn scx() -> &'static str {
    env!("CARGO_BIN_EXE_scx")
}

fn tmp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("scx-p34-{}-{}", name, std::process::id()))
}

fn write_tmp(name: &str, body: &str) -> String {
    let p = tmp(name);
    std::fs::write(&p, body).unwrap();
    p.to_str().unwrap().to_string()
}

fn corpus() -> String {
    let mut out = Vec::new();
    for i in 0..300 {
        out.push(format!("INFO heartbeat seq={i} pool=3 latency=1ms ok"));
    }
    out.push("ERROR conn9f2a broken db crash".to_string());
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

/// 统计行字段提取（compressed:/scxjson: 两形态）。
fn stat_field(out: &str, key: &str) -> String {
    let line = out
        .lines()
        .find(|l| l.contains("mode=") || l.contains("\"mode\":"))
        .expect("stats line");
    if let Some(f) = line.split(&format!("{key}=")).nth(1) {
        // `kept=5/301 lines` / `mode=overflow:evidence limit=1`：取首段。
        f.split(['/', ' ']).next().unwrap().trim().to_string()
    } else {
        line.split(&format!("\"{key}\":"))
            .nth(1)
            .and_then(|s| s.split([',', '}']).next())
            .unwrap_or("")
            .trim()
            .to_string()
    }
}

#[test]
fn policy_file_ratio_equals_explicit_cli_ratio() {
    // 文件 ratio 与显式 `--ratio` 等价（证明文件覆盖 DEFAULT 生效）。
    let pf = write_tmp("ratio.toml", "ratio = 0.9\n");
    let (ok, out_f, _e) = run(&["--query", "conn broken", "--policy-file", &pf], &corpus());
    assert!(ok);
    let (ok2, out_c, _e2) = run(&["--query", "conn broken", "--ratio", "0.9"], &corpus());
    assert!(ok2);
    assert_eq!(out_f, out_c, "文件 ratio 必须与 CLI --ratio 0.9 等价");
    assert_eq!(stat_field(&out_f, "kept"), stat_field(&out_c, "kept"));
}

#[test]
fn cli_explicit_arg_wins_over_file() {
    let pf = write_tmp("prio.toml", "ratio = 0.9\n");
    // 文件 ratio=0.9，但 CLI --ratio 0.1 显式 → CLI 赢（保留更少）。
    let (ok, out, _e) = run(
        &[
            "--query",
            "conn broken",
            "--policy-file",
            &pf,
            "--ratio",
            "0.1",
        ],
        &corpus(),
    );
    assert!(ok);
    let kept: usize = stat_field(&out, "kept").parse().unwrap();
    assert!(kept < 5, "CLI ratio must win, kept={kept}: {out}");
}

#[test]
fn policy_file_max_tokens_overflow_evidence() {
    // 文件 max_tokens=1：证据地板超限 → mode=overflow:evidence（证据不丢）。
    let pf = write_tmp("maxtok.toml", "max_tokens = 1\n");
    let (ok, out, _e) = run(&["--query", "conn broken", "--policy-file", &pf], &corpus());
    assert!(ok, "out: {out}");
    assert_eq!(stat_field(&out, "mode"), "overflow:evidence", "out: {out}");
    // 证据行保留（conn9f2a 行原文存活）。
    assert!(out.contains("conn9f2a broken db crash"));
}

#[test]
fn policy_file_window_zero_normalizes_like_cli() {
    // 文档契约：`--window` 范围 0..=8，0 归一化为 1；策略文件必须同口径
    // （此前文件路径拒绝 0，与 CLI 不一致）。
    let pf = write_tmp("window0.toml", "window = 0\n");
    let (ok, out_f, err) = run(&["--query", "conn broken", "--policy-file", &pf], &corpus());
    assert!(ok, "policy window=0 must be accepted: {err}");
    let (ok2, out_c, _e2) = run(&["--query", "conn broken", "--window", "0"], &corpus());
    assert!(ok2);
    assert_eq!(stat_field(&out_f, "window"), "1", "out: {out_f}");
    assert_eq!(out_f, out_c, "策略文件 window=0 必须与 --window 0 等价");
}

#[test]
fn invalid_policy_reports_line_number() {
    let cases = [
        ("ratio = 99\n", "ratio must be in 0.02..=1.0"),
        ("bogus_key = 1\n", "unknown policy key `bogus_key`"),
        ("ratio = abc\n", "ratio must be a finite f64"),
        ("context = 99\n", "context must be 0..=4"),
        (
            "[section]\nratio = 0.5\n",
            "section [section] not supported",
        ),
    ];
    for (body, expect) in cases {
        let pf = write_tmp("bad.toml", body);
        let (ok, _out, err) = run(&["--query", "x", "--policy-file", &pf], "line one\n");
        assert!(!ok, "must reject: {body:?}");
        // 报错必须含行号与原因。
        assert!(
            err.contains(':') && err.contains(expect),
            "err: {err} expect {expect:?}"
        );
    }
}

#[test]
fn missing_policy_file_rejected() {
    let (ok, _out, err) = run(
        &[
            "--query",
            "x",
            "--policy-file",
            "/nonexistent/scx-policy.toml",
        ],
        "line one\n",
    );
    assert!(!ok);
    assert!(err.contains("cannot read"), "err: {err}");
}
