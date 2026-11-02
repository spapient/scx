//! `xtask selftest`：Rust 侧不变量自检。
//!
//! 替代原 Python 工具 `selftest` 中的断言：覆盖已迁移的
//! 记忆目录安全、语料校验、随机采样、统计指标、manifest 冻结/漂移与
//! Holm-Bonferroni。失败退出 1，成功打印 `selftest: PASS`。

use super::Fail;
use super::{contain, meta, seed, stats};
use scx::json::Json;
use std::fs;
use std::path::Path;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");

pub(crate) fn run() -> Result<(), Fail> {
    match checks() {
        Ok(()) => {
            println!("selftest: PASS");
            Ok(())
        }
        Err(message) => {
            println!("selftest: FAIL: {message}");
            Err(Fail {
                msg: String::new(),
                code: 1,
            })
        }
    }
}

fn checks() -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("scx-selftest-{}-{}", std::process::id(), nanos()));
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let result = inner(&dir);
    let _ = fs::remove_dir_all(&dir);
    result
}

fn inner(dir: &Path) -> Result<(), String> {
    memory_checks(dir)?;
    corpus_checks()?;
    sampling_checks()?;
    stats_checks(dir)?;
    manifest_checks(dir)?;
    holm_checks()?;
    Ok(())
}

fn memory_checks(dir: &Path) -> Result<(), String> {
    check(
        !seed::is_memory_dir(Path::new(ROOT)),
        "ROOT must not be a memory dir",
    )?;
    let path = dir.join("memory");
    check(!seed::is_memory_dir(&path), "empty dir is not memory")?;
    fs::create_dir_all(path.join("cases")).map_err(|e| e.to_string())?;
    check(!seed::is_memory_dir(&path), "cases dir alone is not memory")?;
    fs::write(path.join("meta.toml"), "schema = 2\n").map_err(|e| e.to_string())?;
    check(seed::is_memory_dir(&path), "meta+cases is memory")?;
    fs::write(path.join("stale"), "remove\n").map_err(|e| e.to_string())?;
    let prepared = seed::prepare_mem_dir(path.to_str().unwrap_or_default(), true)?;
    check(prepared.is_dir(), "prepared dir exists")?;
    check(
        fs::read_dir(&prepared)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false),
        "force clear removes contents",
    )?;
    let foreign = dir.join("foreign");
    fs::create_dir_all(&foreign).map_err(|e| e.to_string())?;
    fs::write(foreign.join("keep"), "keep\n").map_err(|e| e.to_string())?;
    let error = seed::prepare_mem_dir(foreign.to_str().unwrap_or_default(), true)
        .expect_err("--force must reject non-memory");
    check(error.contains("non-memory"), "non-memory rejection")?;
    check(
        fs::read_to_string(foreign.join("keep"))
            .map(|text| text == "keep\n")
            .unwrap_or(false),
        "foreign dir untouched",
    )
}

fn corpus_checks() -> Result<(), String> {
    for (name, expected) in [("gauge", 150usize), ("v2", 150), ("real", 40)] {
        let value = load(&Path::new(ROOT).join(format!("config/{name}.corpus.json")))?;
        let layers = seed::validate_corpus(&value)?;
        check(
            layers.iter().sum::<usize>() == expected,
            &format!("{name} corpus cases={expected}"),
        )?;
    }
    let fidelity = load(&Path::new(ROOT).join("config/fidelity.corpus.json"))?;
    let slices = match fidelity.get("slices") {
        Some(Json::Arr(items)) => items,
        _ => return Err("fidelity corpus missing slices".to_string()),
    };
    check(slices.len() == 40, "fidelity slices=40")?;
    for kind in [
        "build", "stack", "diff", "log", "test", "db", "network", "hardware",
    ] {
        let count = slices
            .iter()
            .filter(|slice| slice.get("kind").and_then(Json::str) == Some(kind))
            .count();
        check(count == 5, &format!("fidelity kind {kind}=5"))?;
    }
    Ok(())
}

fn sampling_checks() -> Result<(), String> {
    let sample = "alpha line\nbeta line\nseed line evidence\ngamma noise\n";
    let big = contain::random_sample(sample, sample.len(), "z", "s1");
    check(big == sample, "full budget returns verbatim input")?;
    let small = contain::random_sample(sample, 10, "z", "s1");
    check(small.len() == 10, "small budget is byte-exact")?;
    check(
        small == contain::random_sample(sample, 10, "z", "s1"),
        "sampling deterministic",
    )?;
    check(
        small != contain::random_sample(sample, 10, "z2", "s1"),
        "seed affects sampling",
    )?;
    let trunc = contain::trunc(sample, 10);
    check(
        trunc == String::from_utf8_lossy(&sample.as_bytes()[..10]).into_owned(),
        "trunc is byte prefix",
    )
}

fn stats_checks(dir: &Path) -> Result<(), String> {
    let verbose = dir.join("verbose.txt");
    fs::write(
        &verbose,
        "case 1/1 q=\"database timeout\"\n  A: 1) abcdef12 sim=0.90 score=9.00 [hit]\n  B: (no hits)\n",
    )
    .map_err(|e| e.to_string())?;
    let parsed = stats::parse_verbose(verbose.to_str().unwrap_or_default(), 1)?;
    check(
        parsed[0]
            .a
            .as_ref()
            .is_some_and(|ids| ids == &["abcdef12".to_string()]),
        "verbose A ids",
    )?;
    check(
        parsed[0].b.as_ref().is_some_and(|ids| ids.is_empty()),
        "verbose B empty",
    )?;
    check(stats::mrr(2) == 0.5, "mrr(2)=0.5")?;
    check(stats::recall_at(0, 1) == 0.0, "recall miss")?;
    check(stats::recall_at(1, 1) == 1.0, "recall hit")?;
    check(stats::recall_at(4, 3) == 0.0, "recall beyond k")?;
    check(stats::recall_at(10, 10) == 1.0, "recall at k")?;
    check(
        stats::jaccard("database timeout", "database timeout") == 1.0,
        "jaccard identical",
    )?;
    let case = stats::Case {
        query: "q".to_string(),
        layer: "literal".to_string(),
        expected: vec!["a".to_string()],
        rel: vec![("a".to_string(), 2.0), ("b".to_string(), 1.0)],
        case_index: None,
    };
    let ids = vec!["a".to_string(), "b".to_string()];
    check(stats::ndcg(&case, &ids) == 1.0, "ndcg ideal=1")?;
    let rows = [
        stats::Row {
            i: 0,
            layer: "literal".to_string(),
            tier: "hard".to_string(),
            a: 1.0,
            b: 0.0,
        },
        stats::Row {
            i: 1,
            layer: "literal".to_string(),
            tier: "hard".to_string(),
            a: 0.0,
            b: 0.0,
        },
    ];
    let refs: Vec<&stats::Row> = rows.iter().collect();
    let plan = stats::Plan {
        boot: 200,
        perm: 200,
        seed: 7,
        mde: 0.05,
    };
    let first = stats::analyze("all", &refs, plan).ok_or("analyze returned None")?;
    let second = stats::analyze("all", &refs, plan).ok_or("analyze returned None")?;
    check(
        first.delta == 0.5 && second.delta == first.delta,
        "analyze delta",
    )
}

fn manifest_checks(dir: &Path) -> Result<(), String> {
    let payload = dir.join("payload.json");
    fs::write(&payload, "{\"a\":1}\n").map_err(|e| e.to_string())?;
    let manifest = dir.join("manifest.json");
    let payload_text = payload.to_string_lossy().to_string();
    let manifest_text = manifest.to_string_lossy().to_string();
    meta::manifest(&[
        "--files".to_string(),
        payload_text,
        "--out".to_string(),
        manifest_text.clone(),
    ])
    .map_err(|failure| failure.msg)?;
    meta::manifest(&["--expect".to_string(), manifest_text.clone()])
        .map_err(|failure| failure.msg)?;
    fs::write(&payload, "{\"a\":2}\n").map_err(|e| e.to_string())?;
    let drifted = meta::manifest(&["--expect".to_string(), manifest_text]);
    check(
        drifted.err().map(|failure| failure.code) == Some(2),
        "manifest drift exits 2",
    )
}

fn holm_checks() -> Result<(), String> {
    let cases: [(&[f64], &[bool]); 5] = [
        (&[0.001, 0.04], &[true, true]),
        (&[0.01, 0.03], &[true, true]),
        (&[0.03, 0.04], &[false, false]),
        (&[0.02, 0.03], &[true, true]),
        (&[0.5, 0.001], &[false, true]),
    ];
    for (ps, want) in cases {
        check(
            meta::holm_bonferroni(ps).as_slice() == want,
            "holm decision",
        )?;
    }
    Ok(())
}

fn check(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_string())
    }
}

fn load(path: &Path) -> Result<Json, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    scx::json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn selftest_runs() {
        super::checks().expect("selftest");
    }
}
