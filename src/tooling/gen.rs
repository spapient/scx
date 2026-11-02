//! `xtask gen`：从冻结数据集包物化评测 CSV（feature = "eval"）。
//!
//! `config/datasets.json` 是唯一数据源：每个值是 canonical CSV 字节，
//! 同文件内的 SHA-256 阻止手工编辑与生成器漂移。命令只做确定性物化，
//! 不依赖 node/clang/python，也不在仓库内生成中间目录。

use super::io::{self, J};
use super::{fail, Fail};
use scx::json::Json;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const BUNDLE: &str = "config/datasets.json";

const CSV_KINDS: [(&str, &str); 4] = [
    ("cuda", "code-cuda-60.csv"),
    ("js", "code-js-60.csv"),
    ("layers", "code-layers-15.csv"),
    ("shape", "code-shape-30.csv"),
];
const BENCH_FILES: [(&str, &str); 5] = [
    ("c", "code-c-60.csv"),
    ("py", "code-python-60.csv"),
    ("rs", "code-rust-60.csv"),
    ("cpp", "code-cpp-60.csv"),
    ("java", "code-java-60.csv"),
];

type Dataset = (Vec<String>, Vec<HashMap<String, String>>);

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let (kind, out) = parse(args)?;
    if !out.is_dir() {
        return Err(fail(format!(
            "gen: output directory does not exist: {}",
            out.display()
        )));
    }
    match kind.as_str() {
        "cuda" | "js" | "layers" | "shape" => csv_kind(&kind, &out),
        "bench" => bench(&out),
        other => Err(usage(format!("unknown gen kind: {other}"))),
    }
}

fn parse(args: &[String]) -> Result<(String, PathBuf), Fail> {
    let mut kind = None;
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                i += 1;
                out = Some(
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| usage("--out expects a path"))?,
                );
            }
            "-h" | "--help" => return Err(usage(String::new())),
            arg if arg.starts_with('-') => {
                return Err(usage(format!("unrecognized argument: {arg}")))
            }
            arg if kind.is_none() => kind = Some(arg.to_string()),
            arg => return Err(usage(format!("unexpected argument: {arg}"))),
        }
        i += 1;
    }
    let kind = kind.ok_or_else(|| usage("missing gen kind"))?;
    let out = out
        .map(PathBuf::from)
        .ok_or_else(|| usage("--out is required"))?;
    Ok((kind, out))
}

fn usage(message: impl Into<String>) -> Fail {
    let text = message.into();
    let body = "usage: xtask gen <cuda|js|layers|shape|bench> --out DIR\n";
    Fail {
        msg: if text.is_empty() {
            body.to_string()
        } else {
            format!("{text}\n\n{body}")
        },
        code: 2,
    }
}

fn data() -> Result<Json, Fail> {
    let path = Path::new(ROOT).join(BUNDLE);
    let text = fs::read_to_string(&path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    scx::json::parse(&text).map_err(|e| fail(format!("{}: {e}", path.display())))
}

pub(crate) fn dataset_names() -> Result<Vec<String>, Fail> {
    let data = data()?;
    let Some(Json::Obj(entries)) = data.get("datasets") else {
        return Err(fail(format!("{BUNDLE}: datasets must be an object")));
    };
    Ok(entries.iter().map(|(name, _)| name.clone()).collect())
}

pub(crate) fn dataset_text(name: &str) -> Result<String, Fail> {
    let data = data()?;
    let Some(Json::Obj(entries)) = data.get("datasets") else {
        return Err(fail(format!("{BUNDLE}: datasets must be an object")));
    };
    let text = entries
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| value.str())
        .ok_or_else(|| fail(format!("{BUNDLE}: missing dataset {name}")))?;
    let want = data
        .get("sha256")
        .and_then(|value| value.get(name))
        .and_then(Json::str)
        .ok_or_else(|| fail(format!("{BUNDLE}: missing sha256 for {name}")))?;
    let got = scx::hash::hex(&scx::hash::sha256(text.as_bytes()));
    if got != want {
        return Err(fail(format!(
            "{BUNDLE}: {name} sha256 mismatch: expected {want}, got {got}"
        )));
    }
    Ok(text.to_string())
}

pub(crate) fn dataset(name: &str) -> Result<Dataset, Fail> {
    let text = dataset_text(name)?;
    io::parse_csv(name, &text).map_err(fail)
}

pub(crate) fn dataset_records(name: &str) -> Result<Vec<Vec<String>>, Fail> {
    let text = dataset_text(name)?;
    io::parse_records(name, &text).map_err(fail)
}

fn materialize(name: &str, out: &Path) -> Result<usize, Fail> {
    let text = dataset_text(name)?;
    let path = out.join(name);
    io::write_text(&path.to_string_lossy(), &text).map_err(fail)?;
    let rows = io::parse_records(name, &text).map_err(fail)?.len();
    println!(
        "gen: wrote {} ({} rows)",
        path.display(),
        rows.saturating_sub(1)
    );
    Ok(rows.saturating_sub(1))
}

fn csv_kind(kind: &str, out: &Path) -> Result<(), Fail> {
    let (_, file) = CSV_KINDS
        .iter()
        .find(|(name, _)| *name == kind)
        .copied()
        .ok_or_else(|| usage(format!("unknown gen kind: {kind}")))?;
    materialize(file, out)?;
    Ok(())
}

fn bench(out: &Path) -> Result<(), Fail> {
    let mut total = 0usize;
    for (lang, file) in BENCH_FILES {
        total += materialize(file, out)?;
        println!("gen bench: {lang}");
    }
    println!(
        "gen bench: wrote {} files ({} rows)",
        BENCH_FILES.len(),
        total
    );
    Ok(())
}

/// `Json` → 可缩进输出的 `J`（保留对象键序；整数型 Num 还原为整数）。
pub(crate) fn to_j(value: &Json) -> J {
    match value {
        Json::Null => J::Null,
        Json::Bool(b) => J::Bool(*b),
        Json::Num(n) => {
            if n.fract() == 0.0 && n.abs() < 9e15 {
                J::Int(*n as i64)
            } else {
                J::Num(*n)
            }
        }
        Json::Str(s) => J::Str(s.clone()),
        Json::Arr(items) => J::Arr(items.iter().map(to_j).collect()),
        Json::Obj(entries) => J::Obj(entries.iter().map(|(k, v)| (k.clone(), to_j(v))).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_contains_all_frozen_datasets() {
        let names = dataset_names().unwrap();
        assert_eq!(names.len(), 11);
        for name in names {
            let (head, rows) = dataset(&name).unwrap();
            assert_eq!(head, ["question", "answer"], "{name}");
            assert!(!rows.is_empty(), "{name}");
        }
    }
}
