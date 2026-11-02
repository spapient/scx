//! `xtask` 评估核心的 Python 参考输出回归测试（feature = "eval"）。
//! fixture 由 `tools/scx.py` 生成；本测试锁定 Rust 迁移后的逐字节一致性。

#![cfg(feature = "eval")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_xtask")
}

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/eval")
        .join(name)
        .to_string_lossy()
        .to_string()
}

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("scx-eval-{}-{name}", std::process::id()))
}

fn run(args: &[&str]) -> std::process::ExitStatus {
    Command::new(bin())
        .args(args)
        .status()
        .expect("spawn xtask")
}

#[test]
fn baseline_matches_python_reference() {
    let out = temp("baseline.json");
    let status = run(&[
        "baseline",
        "--csv",
        &fixture("cases.csv"),
        "--out",
        out.to_str().unwrap(),
        "--judge-by",
        "fixture-judge",
        "--judge-date",
        "2026-10-23T00:00:00Z",
    ]);
    assert!(status.success());
    let got = fs::read_to_string(&out).unwrap();
    let want = fs::read_to_string(fixture("baseline.json")).unwrap();
    assert_eq!(got, want);
    let _ = fs::remove_file(out);
}

#[test]
fn stats_matches_python_reference() {
    let out = temp("stats.csv");
    let status = run(&[
        "stats",
        "--baseline",
        &fixture("baseline.json"),
        "--verbose",
        &fixture("verbose.txt"),
        "--strata",
        &fixture("strata.csv"),
        "--out",
        out.to_str().unwrap(),
        "--boot",
        "200",
        "--perm",
        "200",
        "--seed",
        "7",
        "--mde",
        "0.05",
    ]);
    assert!(status.success());
    let got = fs::read_to_string(&out).unwrap();
    let want = fs::read_to_string(fixture("expected-stats.csv")).unwrap();
    assert_eq!(got, want);
    let _ = fs::remove_file(out);
}

#[test]
fn loo_gate_failure_exits_two() {
    let out = temp("gate.csv");
    let status = run(&[
        "stats",
        "--baseline",
        &fixture("baseline.json"),
        "--verbose",
        &fixture("verbose.txt"),
        "--strata",
        &fixture("strata.csv"),
        "--out",
        out.to_str().unwrap(),
        "--boot",
        "50",
        "--perm",
        "50",
        "--seed",
        "7",
        "--loo",
        "--loo-group",
        "1",
        "--loo-gate-delta",
        "1.0",
        "--gate",
    ]);
    assert_eq!(status.code(), Some(2));
    let _ = fs::remove_file(&out);
    let _ = fs::remove_file(out.with_extension("loo.json"));
}

fn corpus() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("config/contain5.corpus.json")
        .to_string_lossy()
        .to_string()
}

fn fixtures(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_string_lossy()
        .to_string()
}

fn fidelity_corpus() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("config/fidelity.corpus.json")
        .to_string_lossy()
        .to_string()
}

fn strip(value: &scx::json::Json) -> scx::json::Json {
    use scx::json::Json;
    match value {
        Json::Obj(entries) => Json::Obj(
            entries
                .iter()
                .filter(|(key, _)| {
                    key != "binary"
                        && key != "wall"
                        && !key.starts_with("wall_")
                        && !key.starts_with("mean_wall_")
                })
                .map(|(key, value)| (key.clone(), strip(value)))
                .collect(),
        ),
        Json::Arr(items) => Json::Arr(items.iter().map(strip).collect()),
        other => other.clone(),
    }
}

fn same(left: &scx::json::Json, right: &scx::json::Json) -> bool {
    use scx::json::Json;
    match (left, right) {
        (Json::Null, Json::Null) => true,
        (Json::Bool(a), Json::Bool(b)) => a == b,
        (Json::Num(a), Json::Num(b)) => a.to_bits() == b.to_bits(),
        (Json::Str(a), Json::Str(b)) => a == b,
        (Json::Arr(a), Json::Arr(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
        }
        (Json::Obj(a), Json::Obj(b)) => {
            a.len() == b.len()
                && a.iter().all(|(key, value)| {
                    b.iter()
                        .find(|(other, _)| other == key)
                        .is_some_and(|(_, other)| same(value, other))
                })
        }
        _ => false,
    }
}

#[test]
fn contain_matches_python_reference() {
    let out = temp("contain.json");
    let output = Command::new(bin())
        .args([
            "contain",
            &corpus(),
            "--binary",
            env!("CARGO_BIN_EXE_scx"),
            "--timing-rounds",
            "1",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("spawn xtask contain");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("contain: scx   evidence retention = 70/70 = 100.00%"),
        "{stdout}"
    );
    assert!(
        stdout.contains("contain: trunc evidence retention = 0/70 = 0.00%"),
        "{stdout}"
    );
    let got = scx::json::parse(&fs::read_to_string(&out).unwrap()).unwrap();
    let want =
        scx::json::parse(&fs::read_to_string(fixture("contain-expected.json")).unwrap()).unwrap();
    assert!(same(&strip(&got), &strip(&want)));
    let _ = fs::remove_file(out);
}

#[test]
fn fidelity_matches_python_reference() {
    let out = temp("fidelity.json");
    let output = Command::new(bin())
        .args([
            "fidelity",
            &fidelity_corpus(),
            "--binary",
            env!("CARGO_BIN_EXE_scx"),
            "--domain-boot",
            "10000",
            "--domain-gate-p",
            "0.05",
            "--gate",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("spawn xtask fidelity");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("fidelity: evidence retention = 65/65 = 100.00%"),
        "{stdout}"
    );
    assert!(stdout.contains("fidelity: gate PASS"), "{stdout}");
    let got = scx::json::parse(&fs::read_to_string(&out).unwrap()).unwrap();
    let want =
        scx::json::parse(&fs::read_to_string(fixture("fidelity-expected.json")).unwrap()).unwrap();
    assert!(same(&strip(&got), &strip(&want)));
    let _ = fs::remove_file(out);
}

#[test]
fn bench_budget_matches_python_reference() {
    let out = temp("budget.csv");
    let status = run(&["bench", "budget", "--out", out.to_str().unwrap()]);
    assert!(status.success());
    let got = fs::read_to_string(&out).unwrap().replace("\r\n", "\n");
    let want = fs::read_to_string(fixtures("eval/budget-expected.csv")).unwrap();
    assert_eq!(got, want);
    let _ = fs::remove_file(out);
}

#[test]
fn bench_real_matches_python_reference() {
    let output = Command::new(bin())
        .args([
            "bench",
            "real",
            "--dumps",
            &fixtures("bench/dumps"),
            "--binary",
            env!("CARGO_BIN_EXE_scx"),
            "--query",
            "cache timeout",
            "--ratio",
            "0.35",
        ])
        .output()
        .expect("spawn xtask bench real");
    assert!(output.status.success());
    let got = String::from_utf8_lossy(&output.stdout);
    let want = fs::read_to_string(fixtures("bench/real-expected.out")).unwrap();
    assert_eq!(got, want);
}

#[test]
fn gen_matches_frozen_artifacts() {
    let dir = temp("gen");
    fs::create_dir_all(&dir).unwrap();
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cases = [
        ("cuda", "bench_cuda60.csv"),
        ("js", "bench_js60.csv"),
        ("layers", "bench_layers15.csv"),
        ("shape", "bench_shape30.csv"),
    ];
    for (kind, file) in cases {
        assert!(run(&["gen", kind, "--out", dir.to_str().unwrap()]).success());
        assert_eq!(
            fs::read(dir.join(file)).unwrap(),
            fs::read(base.join("config/bench/datasets").join(file)).unwrap(),
            "{kind}"
        );
    }
    assert!(run(&["gen", "bench", "--out", dir.to_str().unwrap()]).success());
    for lang in ["c", "py", "rs", "cpp", "java"] {
        let file = format!("bench_{lang}60.csv");
        assert_eq!(
            fs::read(dir.join(&file)).unwrap(),
            fs::read(base.join("config/bench/datasets").join(&file)).unwrap(),
            "{lang}"
        );
    }
    let fid = dir.join("fidelity.json");
    assert!(run(&["gen", "fid", "--out", fid.to_str().unwrap()]).success());
    assert_eq!(
        fs::read(&fid).unwrap(),
        fs::read(base.join("config/fidelity.corpus.json")).unwrap()
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ops_help_exits_two() {
    assert_eq!(run(&["ops", "--help"]).code(), Some(2));
}

#[test]
fn audit_passes_repository() {
    let output = Command::new(bin())
        .arg("audit")
        .output()
        .expect("spawn audit");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("audit_unwrap: OK"), "{stdout}");
}

#[test]
fn a2_passes_frozen_thresholds() {
    let output = Command::new(bin()).arg("a2").output().expect("spawn a2");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OK: A2 保真断言全部通过"), "{stdout}");
}

#[test]
fn manifest_verify_passes() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/eval-manifest.json");
    let output = Command::new(bin())
        .args(["manifest", "--expect", manifest.to_str().unwrap()])
        .output()
        .expect("spawn manifest");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("manifest: PASS (10 files frozen)"),
        "{stdout}"
    );
}

#[test]
fn report_check_passes() {
    let output = Command::new(bin())
        .args([
            "report",
            "--stats",
            &fixtures("eval/report/ci-stats.csv"),
            "--stats-recall3",
            &fixtures("eval/report/ci-stats-recall3.csv"),
            "--loo-json",
            &fixtures("eval/report/ci-stats-loo.loo.json"),
            "--fidelity",
            &fixtures("eval/report/ci-fidelity.json"),
            "--check",
            &fixtures("eval/report/expected.md"),
        ])
        .output()
        .expect("spawn report");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("report: PASS"), "{stdout}");
}

#[test]
fn freeze_matches_python_reference() {
    let manifest = temp("freeze.json");
    fs::write(&manifest, "{}\n").unwrap();
    let output = Command::new(bin())
        .args([
            "freeze",
            "--results",
            &fixtures("eval/freeze/results"),
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .output()
        .expect("spawn freeze");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("frozen=2  added=2  updated=0"), "{stdout}");
    assert_eq!(
        fs::read(&manifest).unwrap(),
        fs::read(fixtures("eval/freeze/expected.json")).unwrap()
    );
    let _ = fs::remove_file(manifest);
}

#[test]
fn subset_matches_frozen_output() {
    let out = temp("subset.json");
    let status = run(&["subset", "--out", out.to_str().unwrap()]);
    assert!(status.success());
    let frozen =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("config/bench/results/p12_6_c2_subset.json");
    assert_eq!(fs::read(&out).unwrap(), fs::read(frozen).unwrap());
    let _ = fs::remove_file(out);
}

#[test]
fn seed_parallel_matches_serial() {
    let dir = temp("seed");
    fs::create_dir_all(&dir).unwrap();
    let corpus = fixtures("eval/seed/corpus.json");
    let binary = env!("CARGO_BIN_EXE_scx");
    let mut outputs = Vec::new();
    for (index, jobs) in ["2", "1"].iter().enumerate() {
        let mem = dir.join(format!("m{index}"));
        let csv = dir.join(format!("c{index}.csv"));
        let output = Command::new(bin())
            .args([
                "seed",
                &corpus,
                "--mem-dir",
                mem.to_str().unwrap(),
                "--csv",
                csv.to_str().unwrap(),
                "--binary",
                binary,
                "--force",
                "--jobs",
                jobs,
            ])
            .output()
            .expect("spawn seed");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("cases=2 literal=1 paraphrase=1 semantic=0"),
            "{stdout}"
        );
        outputs.push(fs::read_to_string(&csv).unwrap());
    }
    assert_eq!(normalize_dates(&outputs[0]), normalize_dates(&outputs[1]));
    let _ = fs::remove_dir_all(dir);
}

fn normalize_dates(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i + 11 <= bytes.len() {
        let date = bytes[i..i + 4].iter().all(u8::is_ascii_digit)
            && bytes[i + 4] == b'-'
            && bytes[i + 5..i + 7].iter().all(u8::is_ascii_digit)
            && bytes[i + 7] == b'-'
            && bytes[i + 8..i + 10].iter().all(u8::is_ascii_digit)
            && bytes[i + 10] == b'_';
        if date {
            out.push_str("<date>_");
            i += 11;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out.push_str(&text[i..]);
    out
}

#[test]
fn selftest_passes() {
    let output = Command::new(bin())
        .arg("selftest")
        .output()
        .expect("spawn selftest");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("selftest: PASS"), "{stdout}");
}

fn repo(relative: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(relative)
        .to_string_lossy()
        .to_string()
}

#[test]
fn verify_matches_frozen_packet() {
    let output = Command::new(bin())
        .args([
            "verify",
            "--packet",
            &repo("config/gauge.blind40.packet.tsv"),
        ])
        .output()
        .expect("spawn verify");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("rows=384"), "{stdout}");
    assert!(
        stdout.contains("sha256=b8eebf2fcb78d0fb745c9760b55dd34d832a9bb976b377f3dfb8c7cea264093f"),
        "{stdout}"
    );
}

#[test]
fn blind_progress_and_check_match() {
    let progress = Command::new(bin())
        .args([
            "blind",
            "progress",
            "--packet",
            &repo("config/gauge.blind40.packet.tsv"),
        ])
        .output()
        .expect("spawn progress");
    assert_eq!(progress.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&progress.stdout).contains("graded=0/384"),
        "{}",
        String::from_utf8_lossy(&progress.stdout)
    );
    let check = Command::new(bin())
        .args([
            "blind",
            "check",
            "--baseline",
            &repo("config/gauge.human.labels.json"),
        ])
        .output()
        .expect("spawn check");
    assert!(check.status.success());
    assert!(
        String::from_utf8_lossy(&check.stdout).contains("cases=40 items=384 incomplete=0"),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
}

#[test]
fn blind_agree_matches() {
    let output = Command::new(bin())
        .args([
            "blind",
            "agree",
            "--a",
            &repo("config/gauge.ai.labels.json"),
            "--b",
            &repo("config/gauge.human.labels.json"),
        ])
        .output()
        .expect("spawn agree");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("common=384 raw=0.344 kappa=0.017"),
        "{stdout}"
    );
    assert!(
        stdout.contains("matrix(rows=A,cols=B) 2,2=70 1,1=10 0,0=52 off-diag=252"),
        "{stdout}"
    );
}

#[test]
fn extract_matches_python_reference() {
    let dir = temp("extract");
    fs::create_dir_all(&dir).unwrap();
    let output = Command::new(bin())
        .args([
            "extract",
            "--dir",
            &fixtures("eval/extract"),
            "--out",
            dir.to_str().unwrap(),
            "--extract",
            "1",
        ])
        .output()
        .expect("spawn extract");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("session-b.js   tool_use=   1 tool_result=   1 use_chars=       18 result_chars=       64"),
        "{stdout}"
    );
    assert!(stdout.contains("(2 lines)"), "{stdout}");
    assert_eq!(
        fs::read(dir.join("session-b.dump.txt")).unwrap(),
        fs::read(fixtures("eval/extract/expected.dump.txt")).unwrap()
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn calibrate_matches_reference() {
    let output = Command::new(bin())
        .args([
            "calibrate",
            "--corpus",
            &fixtures("eval/calibrate/corpus.json"),
        ])
        .output()
        .expect("spawn calibrate");
    assert!(output.status.success());
    let got = String::from_utf8_lossy(&output.stdout);
    let want = fs::read_to_string(fixtures("eval/calibrate/expected.out")).unwrap();
    assert_eq!(got, want);
}

#[test]
fn fit_matches_python_weights() {
    let out = temp("fit-weights.json");
    let output = Command::new(bin())
        .args(["fit", "--out", out.to_str().unwrap()])
        .output()
        .expect("spawn fit");
    assert!(output.status.success());
    let text = fs::read_to_string(&out).unwrap();
    assert!(text.contains("scx-sem-1"), "{text}");
    assert!(text.contains("\"dim\": 16"), "{text}");
    assert!(text.contains("sha256"), "{text}");
    let _ = fs::remove_file(out);
}

#[test]
fn mlp_cross_validation_runs() {
    let out = temp("mlp.json");
    let output = Command::new(bin())
        .args(["mlp", "--out", out.to_str().unwrap()])
        .output()
        .expect("spawn mlp");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fold 0: linear auc/ap="), "{stdout}");
    assert!(stdout.contains("fold 2: linear auc/ap="), "{stdout}");
    let json = fs::read_to_string(&out).unwrap();
    assert!(json.contains("\"dim\": 24"), "{json}");
    let _ = fs::remove_file(out);
}

#[test]
fn unknown_metric_exits_two() {
    let out = temp("bad.csv");
    let status = run(&[
        "stats",
        "--baseline",
        &fixture("baseline.json"),
        "--verbose",
        &fixture("verbose.txt"),
        "--strata",
        &fixture("strata.csv"),
        "--out",
        out.to_str().unwrap(),
        "--metric",
        "bogus",
    ]);
    assert_eq!(status.code(), Some(2));
    let _ = fs::remove_file(out);
}
