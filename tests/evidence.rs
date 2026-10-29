//! 证据行端到端断言（P12.6，建议 5）：`--evidence` 声明的证据行必须逐字节
//! 出现在输出正文 `outcome.text`（stdout 中 stats 行之前的部分）。
//! 覆盖三模式：sieve 深压缩 / small 透传 / fail-open（kept==total 无裁剪）。
//! v0.6.4 缺陷：证据行被字符级 trim 摘词（marker 丢失）但报告称"全部保留"；
//! 本文件把该洞固化为红测——证据行一旦从正文消失即失败。

use std::io::Write;
use std::process::{Command, Stdio};

fn run_scx(args: &[&str], input: &str) -> (String, String, i32) {
    let bin = env!("CARGO_BIN_EXE_scx");
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn scx: {e}"));
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
        out.status.code().unwrap_or(-1),
    )
}

/// 正文 = stdout 去掉 stats 行（`compressed: kept=` 前缀）之前的部分。
fn body_of(stdout: &str) -> &str {
    stdout.split("\ncompressed: kept=").next().unwrap_or(stdout)
}

const EV_LINE: &str = "关键证据行 UNIQUE_EVIDENCE_XYZ 核心配置";
const EV_SUBSTR: &str = "UNIQUE_EVIDENCE_XYZ";

/// 60 行文档：仅第 11 行为证据行，其余为无关填充。query 命中证据行 +
/// trim 深压 → 无关行被裁剪（cut≈93%）。
fn sieve_doc() -> String {
    let mut lines: Vec<String> = (0..60)
        .map(|i| format!("无关填充行 {i} 其余内容"))
        .collect();
    lines[10] = EV_LINE.to_string();
    lines.join("\n")
}

#[test]
fn sieve_deep_cut_keeps_evidence_row_byte_for_byte() {
    let doc = sieve_doc();
    let (out, err, code) = run_scx(
        &[
            "--query",
            "核心配置",
            "--trim",
            "1",
            "--trim-ratio",
            "0.44",
            "--trim-op-keep",
            "1",
            "--num-line-keep",
            "1",
            "--evidence",
            EV_SUBSTR,
        ],
        &doc,
    );
    assert_eq!(code, 0, "退出码: {code}\nstderr: {err}");
    let body = body_of(&out);
    // 证据行整行逐字节出现在正文（修复前该行被裁为残句，此处必 FAIL）。
    assert!(
        body.contains(EV_LINE),
        "sieve 深压缩下证据行必须整行出现在正文\nbody={body}"
    );
    assert!(
        body.contains(EV_SUBSTR),
        "证据 marker 必须出现在正文\nbody={body}"
    );
    // 深压确实发生（cut 显著），断言非透传。
    assert!(
        out.contains("mode=sieve"),
        "应走 sieve 模式\nstats={}",
        out.lines().last().unwrap_or("")
    );
    // stderr 报告如实：1 条声明证据 1 保留 0 丢失。
    assert!(
        err.contains("证据召回") && err.contains("1 条中 1 条保留 · 0 丢失"),
        "报告须如实反映保留数\nstderr={err}"
    );
    // 建议 4：stats 行含实际序列化字节数。
    assert!(
        out.contains("ser_b="),
        "stats 行必须含实际序列化字节 ser_b\nstats={}",
        out.lines().last().unwrap_or("")
    );
}

#[test]
fn small_passthrough_keeps_evidence_row() {
    // <40 行且 <400 token → passthrough:small 原样透传，证据行必然在正文。
    let doc = format!("甲行\n乙行\n{EV_LINE}\n丙行\n");
    let (out, err, code) = run_scx(&["--query", "丙", "--evidence", EV_SUBSTR], &doc);
    assert_eq!(code, 0, "退出码: {code}\nstderr: {err}");
    let body = body_of(&out);
    assert!(
        body.contains(EV_LINE) && body.contains(EV_SUBSTR),
        "small 透传下证据行必须整行保留\nbody={body}"
    );
    assert!(
        out.contains("mode=passthrough:small"),
        "应走 small 透传\nstats={}",
        out.lines().last().unwrap_or("")
    );
}

#[test]
fn fail_open_keeps_evidence_row() {
    // 构造 terms=0 的 fail-open（query 分词不命中任何术语 → 引擎跳过压缩、
    // 几乎原样输出）：证据行在正文。
    let mut lines: Vec<String> = (0..60).map(|i| format!("行 {i} 通用词")).collect();
    lines[10] = EV_LINE.to_string();
    let doc = lines.join("\n");
    let (out, err, code) = run_scx(
        &[
            "--query",
            "甲乙", // 双字均不在术语表 → terms=0 → failopen
            "--evidence",
            EV_SUBSTR,
        ],
        &doc,
    );
    assert_eq!(code, 0, "退出码: {code}\nstderr: {err}");
    let body = body_of(&out);
    assert!(
        body.contains(EV_LINE) && body.contains(EV_SUBSTR),
        "fail-open 下证据行必须整行保留\nbody={body}",
    );
    assert!(
        out.contains("mode=failopen"),
        "应走 failopen 模式\nstats={}",
        out.lines().last().unwrap_or("")
    );
    // 未删任何行：证据行整行与原文一致。
    assert!(
        body.contains(EV_LINE),
        "fail-open 证据行整行保留\nbody={body}"
    );
}
