use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

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
