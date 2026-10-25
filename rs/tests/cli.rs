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
