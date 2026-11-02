//! `xtask ops`：运维六合一。
//!
//! 迁移自原 Python 工具的运维入口：run / install / setup / doctor / check / perf。
//! 路径与命令语义保持原样；构建仍落在 `SCX_TARGET_DIR`（默认 tmp/scx-cargo-target），
//! `check` 仍调用 Python `scx.py check a2` 与 `audit`（这两项尚未迁移）。

// 运维编排分支线性；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::io;
use super::{fail, Fail};
use scx::json::Json;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
use scx::VERSION;

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let argv: Vec<String> = if args.is_empty() {
        vec!["run".to_string()]
    } else {
        args.to_vec()
    };
    match argv[0].as_str() {
        "-h" | "--help" => {
            usage();
            Err(Fail {
                msg: String::new(),
                code: 2,
            })
        }
        "install" => install(),
        "setup" => execute(Some("setup"), &argv[1..]),
        "doctor" => doctor(),
        "check" => check(),
        "perf" => perf(),
        "run" => execute(None, &argv[1..]),
        _ => execute(None, &argv),
    }
}

fn usage() {
    eprintln!("usage: xtask ops [run] [ARGS...] | install | setup | doctor | check | perf");
    eprintln!("  run      自举构建后转发参数（默认；非子命令首参即 CLI 参数）");
    eprintln!("  install  release 构建 + 软链到 ~/.local/bin/scx");
    eprintln!("  setup    注册 MCP 配置（Claude/Codex/Cursor，幂等+备份+自检）");
    eprintln!("  doctor   scx mcp stdio 握手自检");
    eprintln!(
        "  check    评测闭环门禁（cargo test/clippy/fmt + manifest 冻结 + A2 + 冒烟 + 审计）"
    );
    eprintln!("  perf     压缩性能基线（lossless/r0.3/r0.2 三档 ×3 次最优）");
}

fn target_dir() -> PathBuf {
    std::env::var_os("SCX_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("scx-cargo-target"))
}

fn bin_path() -> PathBuf {
    let name = if cfg!(windows) { "scx.exe" } else { "scx" };
    target_dir().join("release").join(name)
}

fn bin_dir() -> PathBuf {
    std::env::var_os("SCX_BIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/bin"))
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn require_scx() -> Result<(), Fail> {
    let bin = bin_path();
    if !stale(&bin) {
        return Ok(());
    }
    eprintln!("[scx] release build (--locked) ...");
    let manifest = Path::new(ROOT)
        .join("Cargo.toml")
        .to_string_lossy()
        .to_string();
    let target = target_dir().to_string_lossy().to_string();
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--quiet",
            "--locked",
            "--manifest-path",
            &manifest,
            "--target-dir",
            &target,
        ])
        .status()
        .map_err(|e| fail(format!("cargo build: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Fail {
            msg: String::new(),
            code: status.code().unwrap_or(1),
        })
    }
}

fn stale(bin: &Path) -> bool {
    let Ok(bin_time) = fs::metadata(bin).and_then(|m| m.modified()) else {
        return true;
    };
    let mut sources = vec![
        Path::new(ROOT).join("Cargo.toml"),
        Path::new(ROOT).join("Cargo.lock"),
    ];
    walk_rs(&Path::new(ROOT).join("src"), &mut sources);
    sources.iter().any(|path| {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|time| time > bin_time)
            .unwrap_or(false)
    })
}

fn walk_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn execute(prefix: Option<&str>, args: &[String]) -> Result<(), Fail> {
    require_scx()?;
    let bin = bin_path();
    let mut command = Command::new(&bin);
    if let Some(prefix) = prefix {
        command.arg(prefix);
    }
    command.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        Err(fail(format!("exec {}: {error}", bin.display())))
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .map_err(|e| fail(format!("exec {}: {e}", bin.display())))?;
        if status.success() {
            Ok(())
        } else {
            Err(Fail {
                msg: String::new(),
                code: status.code().unwrap_or(1),
            })
        }
    }
}

fn install() -> Result<(), Fail> {
    require_scx()?;
    let dir = bin_dir();
    fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    let link = dir.join("scx");
    if fs::symlink_metadata(&link).is_ok() {
        fs::remove_file(&link).map_err(|e| fail(format!("{}: {e}", link.display())))?;
    }
    let bin = bin_path();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&bin, &link)
        .map_err(|e| fail(format!("{}: {e}", link.display())))?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&bin, &link)
        .map_err(|e| fail(format!("{}: {e}", link.display())))?;
    let output = Command::new(&bin)
        .arg("--version")
        .output()
        .map_err(|e| fail(format!("{}: {e}", bin.display())))?;
    println!("{}", String::from_utf8_lossy(&output.stdout).trim());
    println!(
        "installed: {}\n提示：若 {} 不在 PATH，可追加 export PATH=\"{}:$PATH\"",
        link.display(),
        dir.display(),
        dir.display()
    );
    Ok(())
}

fn doctor() -> Result<(), Fail> {
    require_scx()?;
    let bin = bin_path();
    for (method, want) in [
        ("initialize", "\"serverInfo\""),
        ("tools/list", "compress_context"),
    ] {
        let request =
            format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"{method}\",\"params\":{{}}}}");
        let output = mcp_request(&bin, &request)?;
        if !output.contains(want) {
            eprintln!("fail: {method} 握手失败");
            return Err(Fail {
                msg: String::new(),
                code: 1,
            });
        }
    }
    println!("ok: scx mcp ready (stdio)");
    Ok(())
}

fn mcp_request(bin: &Path, request: &str) -> Result<String, Fail> {
    let mut child = Command::new(bin)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| fail(format!("{}: {e}", bin.display())))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "{request}");
    }
    let output = child
        .wait_with_output()
        .map_err(|e| fail(format!("{}: {e}", bin.display())))?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn check() -> Result<(), Fail> {
    let mut failed: Vec<String> = Vec::new();
    for (label, args) in [
        ("lib tests", vec!["test", "--locked", "--lib"]),
        (
            "semantic tests",
            vec!["test", "--locked", "--features", "semantic", "--lib"],
        ),
        ("golden tests", vec!["test", "--locked", "--test", "golden"]),
    ] {
        step(&format!("cargo {}", args.join(" ")));
        if !cargo_tests(&args) {
            failed.push(label.to_string());
        }
    }
    step("clippy --all-targets");
    if !clippy_clean() {
        failed.push("clippy".to_string());
    }
    step("fmt --check");
    if !fmt_clean() {
        failed.push("fmt not clean".to_string());
    }
    step("eval-manifest 冻结校验");
    if !check_manifest(
        &Path::new(ROOT).join("config/bench/eval-manifest.json"),
        &Path::new(ROOT).join("config/bench/results"),
    ) {
        failed.push("manifest integrity".to_string());
    }
    step("A2 保真断言（fidelity_thresholds 分档）");
    if super::checks::a2().is_err() {
        failed.push("A2 fidelity".to_string());
    }
    step("新开关冒烟（budget/fold/auto-anchor/cluster：rc + stats + 双跑确定性）");
    if let Some(label) = smoke() {
        failed.push(label);
    }
    step("unwrap/panic 审计（生产裸 unwrap=0，消息非空）");
    if super::checks::audit().is_err() {
        failed.push("audit".to_string());
    }
    if !failed.is_empty() {
        for label in failed {
            println!("FAIL: {label}");
        }
        println!("\nscx check: 存在失败项");
        return Err(Fail {
            msg: String::new(),
            code: 1,
        });
    }
    println!("\nscx check: 全部通过 ✓");
    Ok(())
}

fn step(name: &str) {
    println!("\n== {name} ==");
}

fn cargo_tests(args: &[&str]) -> bool {
    let Ok(output) = Command::new("cargo").args(args).output() else {
        return false;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let ok = output.status.success() && tail(&stdout, 400).contains("0 failed");
    if !ok {
        println!("  rc={}", output.status.code().unwrap_or(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("  stderr tail:\n{}", tail(&stderr, 1500));
        println!("  stdout tail:\n{}", tail(&stdout, 1500));
    }
    ok
}

fn clippy_clean() -> bool {
    let Ok(output) = Command::new("cargo")
        .args(["clippy", "--locked", "--all-targets"])
        .output()
    else {
        return false;
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    !stderr
        .lines()
        .any(|line| line.starts_with("warning:") || line.starts_with("error:"))
}

fn fmt_clean() -> bool {
    Command::new("cargo")
        .args(["fmt", "--check"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn smoke() -> Option<String> {
    if require_scx().is_err() {
        return Some("smoke build".to_string());
    }
    let bin = bin_path();
    let dir = std::env::temp_dir().join(format!("scx-check-{}-{}", std::process::id(), nanos()));
    if fs::create_dir_all(&dir).is_err() {
        return Some("smoke tempdir".to_string());
    }
    let src = dir.join("in.txt");
    let text = "task start\n  log: build err=42 retry timeout\n  log: build ok step 2\n  log: flush cache err=7\n  a=1 b=2\n  summary: done\n";
    if fs::write(&src, text).is_err() {
        return Some("smoke input".to_string());
    }
    let version = Command::new(&bin).arg("--version").output();
    if !version
        .map(|out| String::from_utf8_lossy(&out.stdout).contains(VERSION))
        .unwrap_or(false)
    {
        return Some("smoke version".to_string());
    }
    for flags in [
        "--budget 0.5",
        "--fold 1",
        "--auto-anchor 1",
        "--cluster 1",
        "--budget 0.35 --cluster 1",
    ] {
        let mut args: Vec<String> = vec![
            src.to_string_lossy().to_string(),
            "--query".to_string(),
            "build timeout".to_string(),
        ];
        args.extend(flags.split_whitespace().map(str::to_string));
        args.push("--json".to_string());
        let first = Command::new(&bin).args(&args).output();
        let second = Command::new(&bin).args(&args).output();
        let ok = match (first, second) {
            (Ok(a), Ok(b)) => {
                a.status.success()
                    && String::from_utf8_lossy(&a.stdout).contains("compressed: kept=")
                    && a.stdout == b.stdout
            }
            _ => false,
        };
        if !ok {
            let _ = fs::remove_dir_all(&dir);
            return Some(format!("smoke {flags}"));
        }
    }
    let _ = fs::remove_dir_all(&dir);
    None
}

fn perf() -> Result<(), Fail> {
    require_scx()?;
    let mut blocks: Vec<String> = Vec::new();
    let mut sources = Vec::new();
    walk_rs(&Path::new(ROOT).join("src"), &mut sources);
    sources.sort();
    for path in sources {
        if let Ok(text) = fs::read_to_string(&path) {
            blocks.push(text);
        }
    }
    for path in ["docs/plan.md", "README.md"] {
        let path = Path::new(ROOT).join(path);
        if let Ok(text) = fs::read_to_string(path) {
            blocks.push(text);
        }
    }
    let datasets = Path::new(ROOT).join("config/bench/datasets");
    let mut files: Vec<PathBuf> = fs::read_dir(&datasets)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "csv"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    for path in files {
        if let Ok(records) = io::read_records(&path.to_string_lossy()) {
            for row in records {
                if row.len() >= 2 && !row[0].trim().is_empty() {
                    blocks.push(row[0].clone());
                }
            }
        }
    }
    let content = blocks.join("\n");
    let chars = content.len();
    let lines = content.lines().count();
    let tmp = std::env::temp_dir().join(format!("scx-perf-{}-{}.txt", std::process::id(), nanos()));
    fs::write(&tmp, &content).map_err(|e| fail(format!("{}: {e}", tmp.display())))?;
    println!("== scx perf baseline ==");
    println!("input: {chars} chars / {lines} lines");
    for (ratio, label) in [(1.0, "lossless"), (0.3, "r0.3"), (0.2, "r0.2")] {
        run_perf(&bin_path(), &tmp, ratio, label, chars);
    }
    let _ = fs::remove_file(&tmp);
    Ok(())
}

fn run_perf(bin: &Path, path: &Path, ratio: f64, label: &str, chars: usize) {
    let mut best: Option<(f64, usize)> = None;
    for _ in 0..3 {
        let start = Instant::now();
        let output = Command::new(bin)
            .args([
                "--query",
                "fn compress",
                "--ratio",
                &ratio.to_string(),
                "--trim",
                "1",
                "--trim-ratio",
                &ratio.to_string(),
                "--trim-op-keep",
                "1",
                "--num-line-keep",
                "1",
                path.to_str().unwrap_or_default(),
            ])
            .output();
        let elapsed = start.elapsed().as_secs_f64();
        if let Ok(output) = output {
            if output.status.success() && best.is_none_or(|(time, _)| elapsed < time) {
                best = Some((elapsed, output.stdout.len()));
            }
        }
    }
    match best {
        Some((seconds, output)) => println!(
            "{label:<12} best={seconds:.2}s  cut={:.1}%",
            (1.0 - output as f64 / chars as f64) * 100.0
        ),
        None => println!("{label:<12} best=n/a"),
    }
}

fn check_manifest(manifest: &Path, results: &Path) -> bool {
    let Ok(text) = fs::read_to_string(manifest) else {
        println!("  manifest: MISSING {}", manifest.display());
        return false;
    };
    let Ok(value) = scx::json::parse(&text) else {
        println!("  manifest: parse error {}", manifest.display());
        return false;
    };
    let mut ok = true;
    if let Some(Json::Obj(entries)) = value.get("frozen") {
        for (name, want) in entries {
            let path = results.join(name);
            let Ok(data) = fs::read(&path) else {
                println!("  frozen {name}: MISSING");
                ok = false;
                continue;
            };
            let got = [
                ("sha256", scx::hash::hex(&scx::hash::sha256(&data))),
                ("rows", rows(&data).to_string()),
            ];
            for (key, value) in got {
                let want = scalar(want.get(key));
                if want.as_deref() != Some(value.as_str()) {
                    println!(
                        "  frozen {name}: {key} want={} got={value}",
                        want.unwrap_or_else(|| "None".to_string())
                    );
                    ok = false;
                }
            }
        }
    }
    if let Some(Json::Arr(items)) = value.get("present") {
        for item in items {
            let file = item.get("file").and_then(Json::str).unwrap_or("");
            let min = item.get("min_rows").and_then(as_usize).unwrap_or(2);
            let path = results.join(file);
            match fs::read_to_string(&path) {
                Ok(text) if text.lines().count() >= min => {}
                Ok(_) => {
                    println!("  present {file}: rows 低于 min_rows");
                    ok = false;
                }
                Err(_) => {
                    println!("  present {file}: MISSING");
                    ok = false;
                }
            }
        }
    }
    println!(
        "  {}",
        if ok {
            "OK: frozen + present 全部匹配"
        } else {
            "FAIL"
        }
    );
    ok
}

fn rows(data: &[u8]) -> usize {
    data.iter().filter(|byte| **byte == b'\n').count() + if data.ends_with(b"\n") { 0 } else { 1 }
}

fn scalar(value: Option<&Json>) -> Option<String> {
    match value? {
        Json::Str(text) => Some(text.clone()),
        Json::Num(number) if number.fract() == 0.0 => Some((*number as i64).to_string()),
        Json::Num(number) => Some(number.to_string()),
        Json::Bool(flag) => Some(flag.to_string()),
        Json::Null => Some("None".to_string()),
        Json::Arr(_) | Json::Obj(_) => None,
    }
}

fn as_usize(value: &Json) -> Option<usize> {
    match value {
        Json::Num(number) if *number >= 0.0 => Some(*number as usize),
        Json::Str(text) => text.parse().ok(),
        _ => None,
    }
}

fn tail(text: &str, count: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(count)..].iter().collect()
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_check_matches_frozen_and_present() {
        let dir = crate::testsupport::tmpdir("ops-manifest");
        let results = PathBuf::from(&dir);
        fs::create_dir_all(&results).unwrap();
        fs::write(results.join("a.csv"), "x\n").unwrap();
        let sha = scx::hash::hex(&scx::hash::sha256(b"x\n"));
        let manifest = results.join("manifest.json");
        let value = Json::Obj(vec![
            (
                "frozen".to_string(),
                Json::Obj(vec![(
                    "a.csv".to_string(),
                    Json::Obj(vec![
                        ("sha256".to_string(), Json::Str(sha)),
                        ("rows".to_string(), Json::Num(1.0)),
                    ]),
                )]),
            ),
            (
                "present".to_string(),
                Json::Arr(vec![Json::Obj(vec![
                    ("file".to_string(), Json::Str("a.csv".to_string())),
                    ("min_rows".to_string(), Json::Num(1.0)),
                ])]),
            ),
        ]);
        let text = format!("{}\n", render(&value));
        fs::write(&manifest, text).unwrap();
        assert!(check_manifest(&manifest, &results));
        fs::write(results.join("a.csv"), "x\ny\n").unwrap();
        assert!(!check_manifest(&manifest, &results));
        let _ = fs::remove_dir_all(dir);
    }

    fn render(value: &Json) -> String {
        // 测试用极简 JSON 渲染：优先复用 tooling io 的缩进器。
        fn to_j(value: &Json) -> io::J {
            match value {
                Json::Null => io::J::Null,
                Json::Bool(b) => io::J::Bool(*b),
                Json::Num(n) if n.fract() == 0.0 => io::J::Int(*n as i64),
                Json::Num(n) => io::J::Num(*n),
                Json::Str(s) => io::J::Str(s.clone()),
                Json::Arr(items) => io::J::Arr(items.iter().map(to_j).collect()),
                Json::Obj(entries) => {
                    io::J::Obj(entries.iter().map(|(k, v)| (k.clone(), to_j(v))).collect())
                }
            }
        }
        let temp = std::env::temp_dir().join(format!("ops-manifest-{}.json", nanos()));
        io::write_json(&temp.to_string_lossy(), &to_j(value)).unwrap();
        let text = fs::read_to_string(&temp).unwrap();
        let _ = fs::remove_file(temp);
        text.trim_end().to_string()
    }
}
