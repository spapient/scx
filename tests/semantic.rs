//! 语义增强集成测试（`cargo test --features semantic`）。
//! 验证：权重文件加载校验（SHA256 篡改拒绝）、缺文件报错、固定权重+固定输入
//! = 固定输出（确定性）、打分参与排序后输出仍可解析。

#![cfg(feature = "semantic")]

use std::fs;
use std::process::{Command, Stdio};

fn scx() -> &'static str {
    env!("CARGO_BIN_EXE_scx")
}

fn sample_weights() -> String {
    format!(
        "{}/config/semantic/sample-weights.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn corpus() -> String {
    (0..300)
        .map(|i| {
            if i == 150 {
                "conn 9f2a broken db crash".to_string()
            } else {
                format!("INFO heartbeat seq={i} ok pool=3 latency=1ms")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run(args: &[&str], stdin_txt: &str) -> (bool, String, String) {
    let mut child = Command::new(scx())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn scx");
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_txt.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (out.status.success(), stdout, stderr)
}

#[test]
fn semantic_load_deterministic_output() {
    let inp = corpus();
    let (ok1, out1, _e1) = run(
        &["--query", "conn broken", "--semantic", &sample_weights()],
        &inp,
    );
    assert!(ok1, "semantic run must succeed");
    assert!(out1.contains("compressed: kept="), "out: {out1}");
    let (ok2, out2, _e2) = run(
        &["--query", "conn broken", "--semantic", &sample_weights()],
        &inp,
    );
    assert!(ok2);
    assert_eq!(out1, out2, "固定权重+固定输入必须逐字节一致");
}

#[test]
fn semantic_tampered_sha_rejected() {
    let p = std::env::temp_dir().join(format!("scx-sem-tamper-{}.json", std::process::id()));
    let txt = fs::read_to_string(sample_weights()).unwrap();
    // 篡改 sha256 字段（换成全 0），其余不变。
    let line = txt
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("\"sha256\"") {
                "  \"sha256\": \"0000000000000000000000000000000000000000000000000000000000000000\","
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&p, line).unwrap();
    let (ok, _out, stderr) = run(
        &["--query", "conn", "--semantic", p.to_str().unwrap()],
        "conn 9f2a broken\nline two\n",
    );
    // 校验失败必须报错拒绝运行（stderr 含 sha256 mismatch 或 parse error）。
    assert!(
        !ok && stderr.contains("sha256"),
        "必须拒绝运行并报 sha256: {stderr}"
    );
    let _ = fs::remove_file(&p);
}

#[test]
fn semantic_missing_file_rejected() {
    let (ok, _out, stderr) = run(
        &[
            "--query",
            "conn",
            "--semantic",
            "/nonexistent/scx-sem-weights.json",
        ],
        "conn broken\n",
    );
    assert!(!ok, "缺失权重文件必须拒绝运行");
    assert!(stderr.contains("semantic"), "stderr: {stderr}");
}
