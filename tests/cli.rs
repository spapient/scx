use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_scx")
}

fn write_temp(name: &str, text: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("scx-cli-{}-{}.txt", std::process::id(), name));
    fs::write(&path, text).unwrap();
    path
}

fn run(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().unwrap()
}

fn run_owned(args: &[String]) -> Output {
    Command::new(bin()).args(args).output().unwrap()
}

#[test]
fn force_keeps_all_duplicate_matches() {
    let mut lines: Vec<String> = (0..200)
        .map(|i| format!("INFO filler {} alpha beta", i))
        .collect();
    for idx in [150usize, 151, 152, 153] {
        lines[idx] = "SPECIAL_FORCE_LINE".to_string();
    }
    let path = write_temp("force", &lines.join("\n"));
    let out = run(&[
        "--query",
        "zzzznomatch",
        "--force",
        "SPECIAL_FORCE_LINE",
        "--context",
        "0",
        "--ratio",
        "0.02",
        path.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.matches("SPECIAL_FORCE_LINE").count(), 4);
    let _ = fs::remove_file(path);
}

#[test]
fn no_stats_returns_body_without_compressed_line() {
    let text: String = (0..100)
        .map(|i| format!("INFO filler {} alpha beta gamma delta epsilon", i))
        .collect::<Vec<_>>()
        .join("\n");
    let path = write_temp("nostats", &text);
    let out = run(&["--query", "alpha", "--no-stats", path.to_str().unwrap()]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("compressed:"));
    assert!(stdout.contains("alpha"));
    let _ = fs::remove_file(path);
}

#[test]
fn kind_flag_routes_bucket_type() {
    let text: String = (0..200)
        .map(|i| format!("INFO filler {} alpha beta", i))
        .collect::<Vec<_>>()
        .join("\n");
    let path = write_temp("kind", &text);
    // 显式类型：scxjson 单桶标签变为对应类型。
    let out = run(&[
        "--query",
        "alpha",
        "--kind",
        "static-contract",
        "--json",
        path.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("\"kind\":\"static-contract\""),
        "stderr: {err}"
    );
    // 缺省仍是单桶 tool-output（v0.4.1 行为不变）。
    let out2 = run(&["--query", "alpha", "--json", path.to_str().unwrap()]);
    let err2 = String::from_utf8_lossy(&out2.stderr);
    assert!(err2.contains("\"kind\":\"tool-output\""), "stderr: {err2}");
    // --kind auto：正文为日志 → 推断 tool-output。
    let out3 = run(&[
        "--query",
        "alpha",
        "--kind",
        "auto",
        "--json",
        path.to_str().unwrap(),
    ]);
    let err3 = String::from_utf8_lossy(&out3.stderr);
    assert!(err3.contains("\"kind\":\"tool-output\""), "stderr: {err3}");
    let _ = fs::remove_file(path);
}

#[test]
fn stat_returns_one_statistics_line() {
    let text: String = (0..100)
        .map(|i| format!("INFO filler {} alpha beta gamma delta epsilon", i))
        .collect::<Vec<_>>()
        .join("\n");
    let path = write_temp("stat", &text);
    let out = run(&["--query", "alpha", "--stat", path.to_str().unwrap()]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.lines().count(), 1);
    assert!(stdout.starts_with("compressed:"));
    let _ = fs::remove_file(path);
}

#[test]
fn invalid_option_and_no_kept_fallback_are_explicit() {
    let text: String = (0..100)
        .map(|i| format!("INFO filler {} alpha beta gamma delta epsilon", i))
        .collect::<Vec<_>>()
        .join("\n");
    let path = write_temp("errors", &text);
    let bad = run(&[
        "--bogus",
        "value",
        "--query",
        "alpha",
        path.to_str().unwrap(),
    ]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown option"));

    let long_line = format!("alpha {}", "x".repeat(5000));
    let long_text = (0..40)
        .map(|_| long_line.clone())
        .collect::<Vec<_>>()
        .join("\n");
    let long_path = write_temp("nokept", &long_text);
    let out = run(&[
        "--query",
        "alpha",
        "--ratio",
        "0.02",
        "--context",
        "1",
        long_path.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.matches("alpha").count(), 40);
    assert!(stdout.contains("mode=passthrough:no-kept"));
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(long_path);
}

#[test]
fn recall_only_does_not_create_a_missing_memory_directory() {
    let root = std::env::temp_dir().join(format!(
        "scx-cli-readonly-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let out = run(&[
        "--query",
        "missing library",
        "--recall-only",
        "--mem-dir",
        root.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!root.exists(), "--recall-only must stay read-only");
}

#[test]
fn recall_failure_is_reported_without_failing_compression() {
    let root = std::env::temp_dir().join(format!(
        "scx-cli-recall-missing-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let input = write_temp("recall-missing", "INFO alpha\n");
    let out = run(&[
        "--query",
        "alpha",
        "--recall",
        "1",
        "--mem-dir",
        root.to_str().unwrap(),
        input.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--recall skipped"),
        "missing memory must be reported, not silently dropped: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !root.exists(),
        "failed optional recall must not create a library"
    );
    let _ = fs::remove_file(input);
}

#[test]
fn recall_cache_never_truncates_a_larger_k() {
    let root = std::env::temp_dir().join(format!(
        "scx-cli-recall-k-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let query = "db timeout conn_id";
    let mut inputs = Vec::new();
    for word in ["alpha", "beta", "gamma", "delta", "epsilon"] {
        let path = write_temp(
            &format!("recall-k-{word}"),
            &format!("ERROR db timeout conn_id={word}\nRoot cause: pool {word}\n"),
        );
        let out = run(&[
            "--query",
            query,
            "--mem-dir",
            root.to_str().unwrap(),
            "--store",
            "1",
            "--force",
            "Root cause",
            path.to_str().unwrap(),
        ]);
        assert!(out.status.success(), "store failed: {out:?}");
        inputs.push(path);
    }
    let empty = write_temp("recall-k-empty", "");
    // 先用 k=1 暖 qmap：快照只有 1 条。
    let warm = run(&[
        "--query",
        query,
        "--mem-dir",
        root.to_str().unwrap(),
        "--recall",
        "1",
        empty.to_str().unwrap(),
    ]);
    assert!(warm.status.success(), "warm recall failed: {warm:?}");
    // 再请求 k=5：不足 5 条的旧快照必须失效，完整召回 5 条。
    let out = run(&[
        "--query",
        query,
        "--mem-dir",
        root.to_str().unwrap(),
        "--recall-only",
        "--recall",
        "5",
    ]);
    assert!(out.status.success(), "k=5 recall failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.lines().filter(|l| l.starts_with("mem: ")).count(),
        5,
        "k=1 的 qmap 快照不得截断 k=5 的结果: {stdout}"
    );
    for path in inputs {
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_file(empty);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn corrupt_recall_snapshot_does_not_fail_compression() {
    let root = std::env::temp_dir().join(format!(
        "scx-cli-recall-corrupt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let input = write_temp(
        "recall-corrupt",
        "ERROR unique_corrupt conn_id=9f2a timeout\nat Pool.java:214\n",
    );
    let args = |extra: &[&str]| -> Vec<String> {
        let mut v = vec![
            "--query".to_string(),
            "ERROR unique_corrupt timeout".to_string(),
            "--mem-dir".to_string(),
            root.to_str().unwrap().to_string(),
            input.to_str().unwrap().to_string(),
        ];
        v.extend(extra.iter().map(|s| s.to_string()));
        v
    };
    let stored = run_owned(&args(&["--store", "1"]));
    assert!(stored.status.success(), "store failed: {stored:?}");

    // 强制索引路径，让召回打分只读 postings；随后损坏 case.json，验证
    // 旁路详情读取失败不会把已经成功的压缩变成失败。
    let meta_path = root.join("meta.toml");
    let meta = fs::read_to_string(&meta_path).unwrap();
    fs::write(
        &meta_path,
        meta.replace("index_force = 0", "index_force = 1"),
    )
    .unwrap();
    let rebuilt = run(&["--rebuild", "--mem-dir", root.to_str().unwrap()]);
    assert!(rebuilt.status.success(), "rebuild failed: {rebuilt:?}");
    let case = fs::read_dir(root.join("cases"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("case.json");
    fs::write(&case, b"{corrupt").unwrap();

    let out = run_owned(&args(&["--recall", "1"]));
    assert!(
        out.status.success(),
        "optional recall detail must not fail compression: {out:?}"
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("compressed:"),
        "compression output missing: {text}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unreadable"),
        "corrupt recall detail must be reported: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let strict = run_owned(&args(&["--recall-only"]));
    assert_eq!(
        strict.status.code(),
        Some(2),
        "--recall-only is the primary operation and must surface corruption"
    );
    let _ = fs::remove_file(input);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn oversize_fix_file_is_rejected_before_storage() {
    let root = std::env::temp_dir().join(format!(
        "scx-cli-fix-cap-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let input = write_temp("fix-cap-input", "ERROR fix cap unique\n");
    let fix = write_temp("fix-cap-body", &"x".repeat(5000));
    let out = run(&[
        "--query",
        "ERROR fix cap",
        "--store",
        "1",
        "--fix-file",
        fix.to_str().unwrap(),
        "--mem-dir",
        root.to_str().unwrap(),
        input.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("4096-byte limit"));
    assert!(
        !root.join("cases").exists(),
        "oversize fix must fail before writing a case"
    );
    for p in [input, fix] {
        let _ = fs::remove_file(p);
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn storage_quota_rejects_before_writing_a_case() {
    let root = std::env::temp_dir().join(format!(
        "scx-cli-quota-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let input = write_temp(
        "quota",
        "ERROR quota test unique_alpha\nINFO filler unique_beta\n",
    );
    let out = run(&[
        "--query",
        "ERROR quota",
        "--store",
        "1",
        "--max-bytes",
        "1",
        "--mem-dir",
        root.to_str().unwrap(),
        input.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("memory quota exceeded"));
    let cases = root.join("cases");
    assert!(
        !cases.exists() || fs::read_dir(&cases).unwrap().next().is_none(),
        "quota failure must not write a case"
    );
    let _ = fs::remove_file(input);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn concurrent_store_processes_keep_all_cases_and_meta_updates() {
    let root = std::env::temp_dir().join(format!(
        "scx-cli-conc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let mut children = Vec::new();
    let mut inputs = Vec::new();
    for i in 0..8 {
        let p = root.join(format!("in{i}.txt"));
        fs::write(&p, format!("ERROR concurrent store {i} unique{i}\n")).unwrap();
        children.push(
            Command::new(bin())
                .args([
                    "--query",
                    &format!("concurrent q{i}"),
                    "--store",
                    "1",
                    "--mem-dir",
                    root.to_str().unwrap(),
                    p.to_str().unwrap(),
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        inputs.push(p);
    }
    for mut c in children {
        assert!(c.wait().unwrap().success(), "并发 store 子进程必须成功");
    }
    let ids: Vec<String> = fs::read_dir(root.join("cases"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(ids.len(), 8, "8 个进程的 case 不能丢");
    let meta = fs::read_to_string(root.join("meta.toml")).unwrap();
    assert!(
        meta.lines().any(|l| l.trim() == "case_count = 8"),
        "case_count 必须等于实际 case 数: {meta}"
    );
    assert!(
        meta.lines().any(|l| l.trim() == "gen = 8"),
        "每次写 case 的 gen 递增都必须保留: {meta}"
    );
    assert_eq!(
        fs::read_to_string(root.join("index/fingerprints.ready"))
            .unwrap()
            .trim(),
        "8",
        "并发去重索引必须覆盖全部 case"
    );
    for p in inputs {
        let _ = fs::remove_file(p);
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn wrap_passes_child_exit_code_and_keeps_body_on_stdout() {
    // 契约：退出码沿用 CMD；正文写 stdout，效果卡写 stderr。
    // 用一个跨平台子命令（scx 自身）：未知参数退出 2。
    let out = run(&["wrap", "--", bin(), "--no-such-flag"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "child exit code must pass through, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 小输出自动透传：正文仍在 stdout，效果卡在 stderr，退出码 0。
    let out = run(&["wrap", "--", bin(), "--version"]);
    assert!(
        out.status.success(),
        "wrap of a successful command must exit 0"
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("scx "),
        "stdout must carry the command output: {text}"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("SCX 压缩召回"),
        "effect card must be on stderr: {err}"
    );
}

#[test]
fn subcommands_tolerate_a_closed_stdout() {
    use std::io::Write;
    for args in [vec!["bucket"], vec!["hook"]] {
        let mut c = Command::new(bin())
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // 先关读端再喂输入：子进程写 stdout 时必然 EPIPE。
        drop(c.stdout.take());
        let mut si = c.stdin.take().unwrap();
        let _ = si.write_all(b"{\"query\":\"conn\",\"parts\":[]}\n");
        drop(si);
        let st = c.wait().unwrap();
        assert_eq!(
            st.code(),
            Some(0),
            "{args:?} must treat a closed stdout as end of output"
        );
    }
    let mut c = Command::new(bin())
        .args(["wrap", "--", bin(), "--version"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    drop(c.stdout.take());
    let st = c.wait().unwrap();
    assert_eq!(st.code(), Some(0), "wrap must tolerate a closed stdout");
}

#[test]
fn wrap_hands_its_stdin_to_the_child() {
    use std::io::Write;
    let mut c = Command::new(bin())
        .args(["wrap", "--query", "conn", "--", bin(), "--query", "conn"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    c.stdin
        .take()
        .unwrap()
        .write_all(b"hello conn boom\n")
        .unwrap();
    let out = c.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("hello"),
        "child stdin must be inherited: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[cfg(unix)]
#[test]
fn wrap_maps_signal_exit_to_128_plus_signal() {
    // SIGKILL（非 15/SIGTERM）：SIGKILL 不可忽略/不可捕获，
    // 不受 shell 继承 SIG_IGN 干扰（macOS bash 与 Linux dash 均稳定终止）。
    let out = Command::new(bin())
        .args(["wrap", "--", "sh", "-c", "kill -KILL $$"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(137), "SIGKILL must map to 128+9");
}

#[test]
fn bucket_record_requires_a_path() {
    let out = run(&["bucket", "--record"]);
    assert_eq!(out.status.code(), Some(2));
}
