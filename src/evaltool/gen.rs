//! `xtask gen`：评测语料生成/物化（feature = "eval"）。
//!
//! 迁移策略：这些语料原先由 `tools/bench.py gen` 用内嵌数据表 + 外部工具执行
//! （node/clang/python）生成，产物已提交并被 `eval-manifest.json` 冻结。为保证
//! 跨机器可复现，生成器数据改为 canonical JSON（`config/bench/gen/`），本命令
//! 只做确定性物化，不再依赖外部工具链。
//!
//! 这不是行为退化：冻结产物的哈希是评测契约，重新执行工具链反而会引入版本差异。

use super::io::{self, J};
use super::{fail, Fail};
use crate::json::Json;
use std::fs;
use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

const CSV_KINDS: [(&str, &str, &str, usize); 4] = [
    ("cuda", "cuda.json", "bench_cuda60.csv", 60),
    ("js", "js.json", "bench_js60.csv", 60),
    ("layers", "layers.json", "bench_layers15.csv", 15),
    ("shape", "shape.json", "bench_shape30.csv", 30),
];
const BENCH_LANGS: [&str; 5] = ["c", "py", "rs", "cpp", "java"];

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let (kind, out) = parse(args)?;
    match kind.as_str() {
        "cuda" | "js" | "layers" | "shape" => csv_kind(&kind, &out),
        "bench" => bench(&out),
        "fid" => fid(&out),
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
    let out = match out {
        Some(path) => PathBuf::from(path),
        None if kind == "fid" => Path::new(ROOT).join("config/fidelity.corpus.json"),
        None => Path::new(ROOT).join("config/bench/datasets"),
    };
    Ok((kind, out))
}

fn usage(message: impl Into<String>) -> Fail {
    let text = message.into();
    let body = "usage: xtask gen <cuda|js|layers|shape|bench|fid> [--out PATH]\n";
    Fail {
        msg: if text.is_empty() {
            body.to_string()
        } else {
            format!("{text}\n\n{body}")
        },
        code: 2,
    }
}

fn data(kind: &str) -> Result<Json, Fail> {
    let path = Path::new(ROOT)
        .join("config/bench/gen")
        .join(format!("{kind}.json"));
    let text = fs::read_to_string(&path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    crate::json::parse(&text).map_err(|e| fail(format!("{}: {e}", path.display())))
}

fn csv_kind(kind: &str, out: &Path) -> Result<(), Fail> {
    let (_, _, file, expected) = CSV_KINDS
        .iter()
        .find(|(name, _, _, _)| *name == kind)
        .copied()
        .ok_or_else(|| usage(format!("unknown gen kind: {kind}")))?;
    let rows = rows(&data(kind)?, expected)?;
    let path = out.join(file);
    write_rows(&path, &rows)?;
    println!("gen {kind}: wrote {} ({} rows)", path.display(), rows.len());
    Ok(())
}

fn bench(out: &Path) -> Result<(), Fail> {
    let value = data("bench")?;
    let entries = match &value {
        Json::Obj(entries) => entries,
        _ => return Err(fail("bench.json: expected object")),
    };
    let mut total = 0usize;
    for lang in BENCH_LANGS {
        let rows_value = entries
            .iter()
            .find(|(key, _)| key == lang)
            .map(|(_, value)| value)
            .ok_or_else(|| fail(format!("bench.json: missing language {lang}")))?;
        let rows = rows(rows_value, 60)?;
        total += rows.len();
        let path = out.join(format!("bench_{lang}60.csv"));
        write_rows(&path, &rows)?;
        println!("gen bench: wrote {} ({} rows)", path.display(), rows.len());
    }
    println!(
        "gen bench: wrote {} files ({} rows)",
        BENCH_LANGS.len(),
        total
    );
    Ok(())
}

fn fid(out: &Path) -> Result<(), Fail> {
    let value = data("fid")?;
    let slices = value
        .get("slices")
        .and_then(|v| match v {
            Json::Arr(items) => Some(items.len()),
            _ => None,
        })
        .ok_or_else(|| fail("fid.json: missing slices array"))?;
    io::write_json_indent(&out.to_string_lossy(), &to_j(&value), 2).map_err(fail)?;
    println!("gen fid: wrote {} ({slices} slices)", out.display());
    Ok(())
}

fn rows(value: &Json, expected: usize) -> Result<Vec<Vec<String>>, Fail> {
    let items = match value {
        Json::Arr(items) => items,
        _ => return Err(fail("expected an array of rows")),
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let question = item
            .get("question")
            .and_then(Json::str)
            .ok_or_else(|| fail(format!("row {index}: missing question")))?;
        let answer = item
            .get("answer")
            .and_then(Json::str)
            .ok_or_else(|| fail(format!("row {index}: missing answer")))?;
        if question.is_empty() || answer.is_empty() {
            return Err(fail(format!("row {index}: empty question/answer")));
        }
        out.push(vec![question.to_string(), answer.to_string()]);
    }
    if out.len() != expected {
        return Err(fail(format!("rows={} expected {expected}", out.len())));
    }
    Ok(out)
}

fn write_rows(path: &Path, rows: &[Vec<String>]) -> Result<(), Fail> {
    let fields = vec!["question".to_string(), "answer".to_string()];
    io::write_csv_with(&path.to_string_lossy(), &fields, rows, "\r\n").map_err(fail)
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
    fn canonical_data_has_expected_shapes() {
        for (kind, _, _, expected) in CSV_KINDS {
            let rows = rows(&data(kind).unwrap(), expected).unwrap();
            assert_eq!(rows.len(), expected);
            assert!(rows.iter().all(|row| row.len() == 2));
        }
        let value = data("bench").unwrap();
        let Json::Obj(entries) = value else {
            panic!("bench.json object")
        };
        for lang in BENCH_LANGS {
            let value = &entries.iter().find(|(key, _)| key == lang).expect("lang").1;
            assert_eq!(rows(value, 60).unwrap().len(), 60);
        }
    }
}
