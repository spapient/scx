//! `xtask verify` / `xtask blind`：盲审包哈希与逐候选盲审协议。
//!
//! 迁移自原 Python 工具的 `verify` 与 `blind make/import/check/progress/agree/review`。
//! TSV 协议、标签 JSON、Cohen's kappa 与提示文案保持与 Python 版一致。

// 盲审协议分支线性；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::io;
use super::meta;
use super::{fail, Fail};
use scx::json::Json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self as stdio, Write};
use std::path::{Path, PathBuf};

const PACKET_HEAD: [&str; 7] = [
    "row",
    "case_index",
    "layer",
    "query",
    "candidate",
    "hint",
    "grade",
];
const GRADE_HELP: &str = "2=同事故/同根因或直接修复  1=同故障族/部分有用  0=无关";

pub(crate) fn verify(args: &[String]) -> Result<(), Fail> {
    validate(args, &["--packet", "--sha256"], &[])?;
    let packet = require(args, "--packet")?;
    let expected = value(args, "--sha256");
    let actual = meta::sha256_file(packet, false)?;
    if let Some(expected) = expected {
        if actual.to_lowercase() != expected.to_lowercase() {
            return Err(fail(format!(
                "verify: packet drifted: {actual} != {expected}"
            )));
        }
    }
    let (_, rows) = read_tsv(packet)?;
    println!(
        "verify: packet={packet} rows={} sha256={actual}",
        rows.len()
    );
    Ok(())
}

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(usage("missing blind subcommand"));
    };
    match command {
        "make" => make(&args[1..]),
        "import" => import(&args[1..]),
        "check" => check(&args[1..]),
        "progress" => progress(&args[1..]),
        "agree" => agree(&args[1..]),
        "review" => review(&args[1..]),
        "-h" | "--help" => {
            print!("{}", blind_usage());
            Ok(())
        }
        other => Err(usage(format!("unknown blind subcommand: {other}"))),
    }
}

fn blind_usage() -> String {
    "\
usage: xtask blind <command> [options]

commands:
  make      baseline + memory -> blind packet TSV + manifest
  import    graded packet -> baseline labels JSON
  check     label coverage / materialized packet drift
  progress  packet grading progress
  agree     Cohen's kappa between two label files
  review    interactive packet grading
"
    .to_string()
}

fn usage(message: impl Into<String>) -> Fail {
    Fail {
        msg: format!("{}\n\n{}", message.into(), blind_usage()),
        code: 2,
    }
}

fn value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

fn require<'a>(args: &'a [String], name: &str) -> Result<&'a str, Fail> {
    value(args, name).ok_or_else(|| usage(format!("{name} is required")))
}

fn validate(args: &[String], values: &[&str], flags: &[&str]) -> Result<(), Fail> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if flags.contains(&arg) {
            i += 1;
            continue;
        }
        if values.contains(&arg) {
            if args.get(i + 1).is_none() {
                return Err(usage(format!("{arg} expects a value")));
            }
            i += 2;
            continue;
        }
        return Err(usage(format!("unrecognized argument: {arg}")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// TSV / JSON helpers
// ---------------------------------------------------------------------------

fn read_tsv(path: &str) -> Result<(Vec<String>, Vec<Vec<String>>), Fail> {
    let text = fs::read_to_string(path).map_err(|e| fail(format!("{path}: {e}")))?;
    let mut lines = text.split('\n');
    let head: Vec<String> = lines
        .next()
        .unwrap_or("")
        .trim_end_matches('\r')
        .split('\t')
        .map(str::to_string)
        .collect();
    if head != PACKET_HEAD.map(str::to_string) {
        return Err(fail(format!("blind: bad header: {}", py_list(&head))));
    }
    let mut rows = Vec::new();
    let mut number = 1usize;
    for line in lines {
        number += 1;
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let row: Vec<String> = line.split('\t').map(str::to_string).collect();
        if row.len() != head.len() {
            return Err(fail(format!(
                "blind: row {number} has {} columns, expected {}",
                row.len(),
                head.len()
            )));
        }
        rows.push(row);
    }
    Ok((head, rows))
}

fn tsv_text(head: &[String], rows: &[Vec<String>]) -> String {
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(head.join("\t"));
    for row in rows {
        lines.push(row.join("\t"));
    }
    lines.join("\n") + "\n"
}

fn read_packet(path: &str) -> Result<Vec<HashMap<String, String>>, Fail> {
    let (head, rows) = read_tsv(path)?;
    Ok(rows
        .into_iter()
        .map(|row| head.iter().cloned().zip(row).collect())
        .collect())
}

fn write_text(path: &str, text: &str) -> Result<(), Fail> {
    let target = Path::new(path);
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| fail(format!("{path}: {e}")))?;
        }
    }
    let tmp = format!("{path}.tmp");
    fs::write(&tmp, text).map_err(|e| fail(format!("{path}: {e}")))?;
    fs::rename(&tmp, target).map_err(|e| fail(format!("{path}: {e}")))
}

fn load_json(path: &str) -> Result<Json, Fail> {
    let text = fs::read_to_string(path).map_err(|e| fail(format!("{path}: {e}")))?;
    scx::json::parse(&text).map_err(|e| fail(format!("{path}: {e}")))
}

fn dump_json(path: &str, value: &Json) -> Result<(), Fail> {
    io::write_json_indent(path, &to_j(value), 1).map_err(fail)
}

fn to_j(value: &Json) -> io::J {
    use io::J;
    match value {
        Json::Null => J::Null,
        Json::Bool(flag) => J::Bool(*flag),
        Json::Num(number) if number.fract() == 0.0 && number.abs() < 9e15 => J::Int(*number as i64),
        Json::Num(number) => J::Num(*number),
        Json::Str(text) => J::Str(text.clone()),
        Json::Arr(items) => J::Arr(items.iter().map(to_j).collect()),
        Json::Obj(entries) => J::Obj(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), to_j(value)))
                .collect(),
        ),
    }
}

fn sha256_text(text: &str) -> String {
    scx::hash::hex(&scx::hash::sha256(text.as_bytes()))
}

fn clean(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| if c == '\t' || c == '\r' { ' ' } else { c })
        .collect();
    replaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(220)
        .collect()
}

fn tail_of(key: &str) -> &str {
    key.rsplit('_').next().unwrap_or(key)
}

fn py_list(items: &[String]) -> String {
    let body = items
        .iter()
        .map(|item| format!("'{item}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{body}]")
}

fn resolve(path: &str) -> String {
    fs::canonicalize(path)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}

fn as_int(value: &Json) -> Option<i64> {
    match value {
        Json::Num(number) if number.fract() == 0.0 => Some(*number as i64),
        Json::Str(text) => text.parse().ok(),
        _ => None,
    }
}

fn as_grade(value: &Json) -> Option<i64> {
    as_int(value)
}

fn as_float(value: &Json) -> Option<f64> {
    match value {
        Json::Num(number) => Some(*number),
        Json::Str(text) => text.parse().ok(),
        _ => None,
    }
}

fn cases_of(value: &Json) -> &[Json] {
    match value.get("cases") {
        Some(Json::Arr(cases)) => cases,
        _ => &[],
    }
}

struct Entry {
    id: String,
    query: String,
    fix: String,
}

fn load_memory_cases(mem_dir: &str) -> Result<HashMap<String, Entry>, Fail> {
    let root = Path::new(mem_dir).join("cases");
    if !root.is_dir() {
        return Err(fail(format!(
            "blind: missing memory cases dir: {}",
            root.display()
        )));
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(&root)
        .map_err(|e| fail(format!("{}: {e}", root.display())))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    let mut by_tail = HashMap::new();
    for dir in dirs {
        let path = dir.join("case.json");
        if !path.is_file() {
            continue;
        }
        let value = load_json(&path.to_string_lossy())?;
        let fallback = dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let id = value
            .get("id")
            .and_then(Json::str)
            .filter(|text| !text.is_empty())
            .unwrap_or(fallback.as_str())
            .to_string();
        let query = value
            .get("query")
            .and_then(Json::str)
            .unwrap_or_default()
            .to_string();
        let fix = value
            .get("fix")
            .and_then(Json::str)
            .unwrap_or_default()
            .to_string();
        by_tail.insert(tail_of(&id).to_string(), Entry { id, query, fix });
    }
    Ok(by_tail)
}

// ---------------------------------------------------------------------------
// make
// ---------------------------------------------------------------------------

fn make(args: &[String]) -> Result<(), Fail> {
    validate(
        args,
        &[
            "--baseline",
            "--mem-dir",
            "--out",
            "--manifest",
            "--name",
            "--seed",
            "--per-layer",
        ],
        &[],
    )?;
    let baseline = require(args, "--baseline")?;
    let mem_dir = require(args, "--mem-dir")?;
    let out = require(args, "--out")?;
    let manifest_path = require(args, "--manifest")?;
    let name = value(args, "--name").unwrap_or("gauge-blind40");
    let seed = value(args, "--seed").unwrap_or("gauge-blind-v1");
    let per =
        per_layer(value(args, "--per-layer").unwrap_or("literal=14,paraphrase=13,semantic=13"))?;
    let baseline_value = load_json(baseline)?;
    let cases = cases_of(&baseline_value);
    let mut by_layer: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let layer = case.get("layer").and_then(Json::str).unwrap_or("");
        match by_layer.iter_mut().find(|(name, _)| name == layer) {
            Some((_, list)) => list.push(index),
            None => by_layer.push((layer.to_string(), vec![index])),
        }
    }
    let mut picks = Vec::new();
    for (layer, indexes) in &by_layer {
        let want = per.get(layer).copied().unwrap_or(0);
        if want > indexes.len() {
            return Err(fail(format!(
                "blind: layer {layer} has {} cases < {want}",
                indexes.len()
            )));
        }
        let mut ranked = indexes.clone();
        ranked.sort_by_key(|index| sha256_text(&format!("{seed}:{layer}:{index}")));
        picks.extend(ranked.into_iter().take(want));
    }
    picks.sort_by_key(|index| sha256_text(&format!("{seed}:case:{index}")));
    let memory = load_memory_cases(mem_dir)?;
    let mut rows: Vec<(usize, String, String, String, String)> = Vec::new();
    let mut manifest_cases = Vec::new();
    for case_index in picks {
        let case = &cases[case_index];
        let layer = case
            .get("layer")
            .and_then(Json::str)
            .unwrap_or("")
            .to_string();
        let rel: &[(String, Json)] = match case.get("rel") {
            Some(Json::Obj(entries)) => entries,
            _ => &[],
        };
        if rel.is_empty() {
            return Err(fail(format!("blind: case {case_index} has empty rel pool")));
        }
        let mut ordered: Vec<&String> = rel.iter().map(|(key, _)| key).collect();
        ordered.sort_by_key(|key| sha256_text(&format!("{seed}:{case_index}:{key}")));
        let mut filled = Vec::new();
        for key in ordered {
            let hit = memory.get(tail_of(key)).ok_or_else(|| {
                fail(format!(
                    "blind: candidate {key} (case {case_index}) missing in memory"
                ))
            })?;
            let hint = if hit.fix.is_empty() {
                hit.query.clone()
            } else {
                format!("{} || fix: {}", hit.query, hit.fix)
            };
            filled.push((key.to_string(), hit.id.clone(), clean(&hint)));
        }
        manifest_cases.push(Json::Obj(vec![
            ("case_index".to_string(), Json::Num(case_index as f64)),
            ("layer".to_string(), Json::Str(layer.clone())),
            ("candidates".to_string(), Json::Num(filled.len() as f64)),
        ]));
        let query = clean(case.get("query").and_then(Json::str).unwrap_or(""));
        for (key, _full, hint) in filled {
            rows.push((case_index, layer.clone(), query.clone(), key, hint));
        }
    }
    let packet_rows: Vec<Vec<String>> = rows
        .iter()
        .enumerate()
        .map(|(index, (ci, layer, query, key, hint))| {
            vec![
                (index + 1).to_string(),
                ci.to_string(),
                layer.clone(),
                query.clone(),
                key.clone(),
                hint.clone(),
                String::new(),
            ]
        })
        .collect();
    let head: Vec<String> = PACKET_HEAD.map(str::to_string).to_vec();
    write_text(out, &tsv_text(&head, &packet_rows))?;
    let manifest = Json::Obj(vec![
        ("name".to_string(), Json::Str(name.to_string())),
        ("seed".to_string(), Json::Str(seed.to_string())),
        ("baseline".to_string(), Json::Str(resolve(baseline))),
        (
            "baseline_sha256".to_string(),
            Json::Str(meta::sha256_file(baseline, false)?),
        ),
        ("memory_dir".to_string(), Json::Str(resolve(mem_dir))),
        ("packet".to_string(), Json::Str(resolve(out))),
        (
            "packet_sha256".to_string(),
            Json::Str(meta::sha256_file(out, false)?),
        ),
        ("protocol".to_string(), Json::Str(GRADE_HELP.to_string())),
        ("cases".to_string(), Json::Arr(manifest_cases.clone())),
        (
            "sample_cases".to_string(),
            Json::Num(manifest_cases.len() as f64),
        ),
        ("total_cases".to_string(), Json::Num(cases.len() as f64)),
        ("judgments".to_string(), Json::Num(rows.len() as f64)),
    ]);
    dump_json(manifest_path, &manifest)?;
    let packet_sha = meta::sha256_file(out, false)?;
    println!(
        "blind: cases={}/{} judgments={} packet={out}",
        manifest_cases.len(),
        cases.len(),
        rows.len()
    );
    println!("blind: packet_sha256={packet_sha}");
    println!("blind: {GRADE_HELP}");
    Ok(())
}

fn per_layer(text: &str) -> Result<HashMap<String, usize>, Fail> {
    let mut out = HashMap::new();
    for part in text.split(',') {
        if part.is_empty() {
            continue;
        }
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| usage(format!("invalid --per-layer entry: {part}")))?;
        let value: usize = value
            .parse()
            .map_err(|_| usage(format!("invalid --per-layer count: {value}")))?;
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// import / parse_grades
// ---------------------------------------------------------------------------

fn parse_grades(
    rows: &[HashMap<String, String>],
    allow_partial: bool,
) -> Result<HashMap<(i64, String), f64>, Fail> {
    if rows.is_empty() {
        return Err(fail("blind: empty packet"));
    }
    let mut grades = HashMap::new();
    for row in rows {
        let grade = row.get("grade").map(|value| value.trim()).unwrap_or("");
        if grade.is_empty() && allow_partial {
            continue;
        }
        if !matches!(grade, "0" | "1" | "2") {
            if !grade.is_empty() {
                return Err(fail(format!(
                    "blind: row {} grade={grade:?} must be 0, 1, or 2",
                    row.get("row").map(String::as_str).unwrap_or("")
                )));
            }
            return Err(fail(format!(
                "blind: row {} has an empty grade (fill every row or pass --allow-partial)",
                row.get("row").map(String::as_str).unwrap_or("")
            )));
        }
        let case_index: i64 = row
            .get("case_index")
            .ok_or_else(|| fail("blind: missing case_index"))?
            .parse()
            .map_err(|_| fail("blind: bad case_index"))?;
        let candidate = row.get("candidate").cloned().unwrap_or_default();
        let key = (case_index, candidate);
        if grades.contains_key(&key) {
            return Err(fail(format!(
                "blind: duplicate row for case {} candidate {}",
                key.0, key.1
            )));
        }
        grades.insert(key, grade.parse().unwrap_or(0.0));
    }
    if grades.is_empty() {
        return Err(fail("blind: no graded rows"));
    }
    Ok(grades)
}

fn import(args: &[String]) -> Result<(), Fail> {
    validate(
        args,
        &[
            "--baseline",
            "--packet",
            "--out",
            "--judge",
            "--date",
            "--name",
            "--method",
            "--note",
        ],
        &["--allow-partial"],
    )?;
    let baseline = require(args, "--baseline")?;
    let packet = require(args, "--packet")?;
    let out = require(args, "--out")?;
    let judge = require(args, "--judge")?;
    let date = require(args, "--date")?;
    let name = value(args, "--name").unwrap_or("gauge-blind40");
    let method = value(args, "--method").unwrap_or("per-item-blind-v1");
    let note_arg = value(args, "--note").unwrap_or("");
    let allow_partial = flag(args, "--allow-partial");
    let (head, table) = read_tsv(packet)?;
    let rows: Vec<HashMap<String, String>> = table
        .iter()
        .map(|row| head.iter().cloned().zip(row.iter().cloned()).collect())
        .collect();
    let grades = parse_grades(&rows, allow_partial)?;
    let baseline_value = load_json(baseline)?;
    let cases = cases_of(&baseline_value);
    let packet_sha = sha256_text(&tsv_text(&head, &table));
    let mut by_case: BTreeMap<i64, BTreeMap<String, f64>> = BTreeMap::new();
    for ((case_index, candidate), grade) in &grades {
        if *case_index < 0 || *case_index as usize >= cases.len() {
            return Err(fail(format!(
                "blind: case_index out of range: {case_index}"
            )));
        }
        by_case
            .entry(*case_index)
            .or_default()
            .insert(candidate.clone(), *grade);
    }
    let mut output = Vec::new();
    let mut violations: Vec<(usize, String, Option<f64>)> = Vec::new();
    for (case_index, grade_map) in &by_case {
        let case = &cases[*case_index as usize];
        let expected = match case.get("expected") {
            Some(Json::Arr(items)) if !items.is_empty() => items,
            _ => {
                return Err(fail(format!(
                    "blind: case {case_index} has no expected target"
                )))
            }
        };
        let rel: &[(String, Json)] = match case.get("rel") {
            Some(Json::Obj(entries)) => entries,
            _ => &[],
        };
        let pool: HashSet<&String> = rel.iter().map(|(key, _)| key).collect();
        let got: HashSet<&String> = grade_map.keys().collect();
        if pool != got {
            let mut missing: Vec<String> =
                pool.difference(&got).map(|key| (*key).clone()).collect();
            let mut extra: Vec<String> = got.difference(&pool).map(|key| (*key).clone()).collect();
            missing.sort();
            extra.sort();
            let missing = missing.iter().take(3).cloned().collect::<Vec<_>>();
            let extra = extra.iter().take(3).cloned().collect::<Vec<_>>();
            return Err(fail(format!(
                "blind: case {case_index} coverage mismatch missing={} extra={}",
                py_list(&missing),
                py_list(&extra)
            )));
        }
        let rel_json: Vec<(String, Json)> = rel
            .iter()
            .map(|(key, _)| (key.clone(), Json::Num(*grade_map.get(key).unwrap_or(&0.0))))
            .collect();
        let target = tail_of(expected[0].str().unwrap_or(""));
        let target_grade = rel_json
            .iter()
            .find(|(key, _)| tail_of(key) == target)
            .and_then(|(_, value)| value.num());
        let layer = case.get("layer").and_then(Json::str).unwrap_or("");
        if layer == "literal" && target_grade != Some(2.0) {
            violations.push((
                *case_index as usize + 1,
                target.chars().take(8).collect(),
                target_grade,
            ));
        }
        let mut note = format!(
            "per-item blind review; packet={}",
            Path::new(packet)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
        if !note_arg.is_empty() {
            note.push_str(&format!("; {note_arg}"));
        }
        output.push(Json::Obj(vec![
            (
                "query".to_string(),
                Json::Str(
                    case.get("query")
                        .and_then(Json::str)
                        .unwrap_or("")
                        .to_string(),
                ),
            ),
            ("layer".to_string(), Json::Str(layer.to_string())),
            ("expected".to_string(), Json::Arr(expected.clone())),
            ("case_index".to_string(), Json::Num(*case_index as f64)),
            ("rel".to_string(), Json::Obj(rel_json)),
            (
                "judge".to_string(),
                Json::Obj(vec![
                    ("by".to_string(), Json::Str(judge.to_string())),
                    ("date".to_string(), Json::Str(date.to_string())),
                    ("method".to_string(), Json::Str(method.to_string())),
                    ("packet_sha256".to_string(), Json::Str(packet_sha.clone())),
                    ("packet".to_string(), Json::Str(resolve(packet))),
                ]),
            ),
            (
                "judge_items".to_string(),
                Json::Obj(
                    rel.iter()
                        .map(|(key, _)| {
                            (
                                key.clone(),
                                Json::Obj(vec![
                                    ("by".to_string(), Json::Str(judge.to_string())),
                                    ("date".to_string(), Json::Str(date.to_string())),
                                ]),
                            )
                        })
                        .collect(),
                ),
            ),
            ("note".to_string(), Json::Str(note)),
        ]));
    }
    if !violations.is_empty() {
        for (case_number, target, grade) in violations.iter().take(8) {
            eprintln!(
                "blind: literal case {case_number} target {target} graded {} (must be 2)",
                grade.map(io::repr).unwrap_or_else(|| "None".to_string())
            );
        }
        return Err(fail(format!(
            "blind: {} literal self-match rows are not 2; nothing written",
            violations.len()
        )));
    }
    dump_json(
        out,
        &Json::Obj(vec![
            ("name".to_string(), Json::Str(name.to_string())),
            (
                "provenance".to_string(),
                Json::Str("per-item-blind-v1".to_string()),
            ),
            ("cases".to_string(), Json::Arr(output.clone())),
        ]),
    )?;
    let mut counts = [0usize; 3];
    for case in &output {
        if let Some(Json::Obj(entries)) = case.get("rel") {
            for (_, value) in entries {
                if let Some(grade) = value.num() {
                    if grade == 2.0 {
                        counts[2] += 1;
                    } else if grade == 1.0 {
                        counts[1] += 1;
                    } else if grade == 0.0 {
                        counts[0] += 1;
                    }
                }
            }
        }
    }
    println!(
        "blind: wrote {out} cases={} items={} rel2={} rel1={} rel0={}",
        output.len(),
        counts.iter().sum::<usize>(),
        counts[2],
        counts[1],
        counts[0]
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// filled packet / check
// ---------------------------------------------------------------------------

fn filled_packet(head: &[String], rows: &[Vec<String>], cases: &[Json]) -> Result<String, Fail> {
    let mut rel_by_case: BTreeMap<i64, Vec<(String, f64)>> = BTreeMap::new();
    for (index, case) in cases.iter().enumerate() {
        let case_index = case
            .get("case_index")
            .and_then(as_int)
            .unwrap_or(index as i64);
        if rel_by_case.contains_key(&case_index) {
            return Err(fail(format!(
                "blind: duplicate label case_index: {case_index}"
            )));
        }
        let rel = match case.get("rel") {
            Some(Json::Obj(entries)) => entries
                .iter()
                .filter_map(|(key, value)| as_float(value).map(|grade| (key.clone(), grade)))
                .collect(),
            _ => Vec::new(),
        };
        rel_by_case.insert(case_index, rel);
    }
    let mut filled = Vec::with_capacity(rows.len());
    for (number, row) in rows.iter().enumerate() {
        let n = number + 2;
        let fields: HashMap<&str, &str> = head
            .iter()
            .map(String::as_str)
            .zip(row.iter().map(String::as_str))
            .collect();
        let case_index: i64 = fields
            .get("case_index")
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| {
                fail(format!(
                    "blind: packet row {n} cannot be materialized from labels"
                ))
            })?;
        let candidate = fields.get("candidate").copied().unwrap_or("");
        let grade = rel_by_case
            .get(&case_index)
            .and_then(|rel| rel.iter().find(|(key, _)| key == candidate))
            .map(|(_, grade)| *grade)
            .ok_or_else(|| {
                fail(format!(
                    "blind: packet row {n} cannot be materialized from labels"
                ))
            })?;
        if !matches!(grade, 0.0 | 1.0 | 2.0) {
            return Err(fail(format!(
                "blind: packet row {n} grade {grade} outside 0..2"
            )));
        }
        let mut values = Vec::with_capacity(head.len());
        for key in head {
            let value = if key == "grade" {
                (grade as i64).to_string()
            } else {
                fields.get(key.as_str()).copied().unwrap_or("").to_string()
            };
            values.push(value);
        }
        filled.push(values);
    }
    Ok(tsv_text(head, &filled))
}

fn check(args: &[String]) -> Result<(), Fail> {
    validate(args, &["--baseline", "--packet", "--out"], &[])?;
    let baseline = require(args, "--baseline")?;
    let packet = value(args, "--packet");
    let out = value(args, "--out");
    if out.is_some() && packet.is_none() {
        return Err(fail("blind: --out requires --packet"));
    }
    let value = load_json(baseline)?;
    let cases = cases_of(&value);
    let mut bad = 0usize;
    let mut items = 0usize;
    for (index, case) in cases.iter().enumerate() {
        let pool: HashSet<&String> = match case.get("rel") {
            Some(Json::Obj(entries)) => entries.iter().map(|(key, _)| key).collect(),
            _ => HashSet::new(),
        };
        let got: HashSet<&String> = match case.get("judge_items") {
            Some(Json::Obj(entries)) => entries.iter().map(|(key, _)| key).collect(),
            _ => HashSet::new(),
        };
        items += got.len();
        if pool != got {
            bad += 1;
            println!(
                "blind: case {index} uncovered={} extra={}",
                pool.difference(&got).count(),
                got.difference(&pool).count()
            );
        }
    }
    println!(
        "blind: cases={} items={items} incomplete={bad}",
        cases.len()
    );
    let mut failed = bad > 0;
    if let Some(packet) = packet {
        let (head, rows) = read_tsv(packet)?;
        let body = filled_packet(&head, &rows, cases)?;
        let actual = sha256_text(&body);
        let mut recorded = HashSet::new();
        for case in cases {
            if let Some(Json::Obj(judge)) = case.get("judge") {
                if let Some(value) = judge
                    .iter()
                    .find(|(key, _)| key == "packet_sha256")
                    .and_then(|(_, value)| value.str())
                    .filter(|value| !value.is_empty())
                {
                    recorded.insert(value.to_string());
                }
            }
        }
        if recorded.len() > 1 {
            return Err(fail("blind: labels disagree on packet_sha256"));
        }
        let expected = recorded.into_iter().next().unwrap_or_default();
        if !expected.is_empty() && actual.to_lowercase() != expected.to_lowercase() {
            eprintln!("blind: materialized packet drifted: {actual} != {expected}");
            failed = true;
        }
        if let Some(out) = out {
            write_text(out, &body)?;
        }
        println!("blind: materialized rows={} sha256={actual}", rows.len());
    }
    if failed {
        Err(Fail {
            msg: String::new(),
            code: 1,
        })
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// progress
// ---------------------------------------------------------------------------

fn progress(args: &[String]) -> Result<(), Fail> {
    validate(args, &["--packet"], &[])?;
    let packet = require(args, "--packet")?;
    let rows = read_packet(packet)?;
    let done = rows
        .iter()
        .filter(|row| {
            matches!(
                row.get("grade").map(|value| value.trim()).unwrap_or(""),
                "0" | "1" | "2"
            )
        })
        .count();
    println!(
        "blind: graded={done}/{} remaining={}",
        rows.len(),
        rows.len() - done
    );
    if done == rows.len() {
        Ok(())
    } else {
        Err(Fail {
            msg: String::new(),
            code: 1,
        })
    }
}

// ---------------------------------------------------------------------------
// agreement / agree
// ---------------------------------------------------------------------------

struct Agreement {
    total: usize,
    raw: f64,
    kappa: f64,
    matrix: [[usize; 3]; 3],
    disagreements: Vec<(usize, String, String, i64, i64)>,
}

fn agreement(left: &[Json], right: &[Json]) -> Result<Agreement, Fail> {
    if left.len() != right.len() {
        return Err(fail(format!(
            "blind: case counts differ: {} vs {}",
            left.len(),
            right.len()
        )));
    }
    let mut matrix = [[0usize; 3]; 3];
    let mut disagreements = Vec::new();
    let mut total = 0usize;
    let mut agree = 0usize;
    for (index, (left, right)) in left.iter().zip(right).enumerate() {
        let rel_left = match left.get("rel") {
            Some(Json::Obj(entries)) => entries,
            _ => continue,
        };
        let rel_right = match right.get("rel") {
            Some(Json::Obj(entries)) => entries,
            _ => continue,
        };
        for (candidate, left_value) in rel_left {
            let Some(right_value) = rel_right
                .iter()
                .find(|(key, _)| key == candidate)
                .map(|(_, value)| value)
            else {
                continue;
            };
            let a = as_grade(left_value).ok_or_else(|| {
                fail(format!(
                    "blind: non-integer grade at case {} candidate {candidate}",
                    index + 1
                ))
            })?;
            let b = as_grade(right_value).ok_or_else(|| {
                fail(format!(
                    "blind: non-integer grade at case {} candidate {candidate}",
                    index + 1
                ))
            })?;
            if !(0..=2).contains(&a) || !(0..=2).contains(&b) {
                return Err(fail(format!(
                    "blind: grade outside 0..2 at case {} candidate {candidate}",
                    index + 1
                )));
            }
            matrix[a as usize][b as usize] += 1;
            total += 1;
            if a == b {
                agree += 1;
            } else {
                disagreements.push((
                    index + 1,
                    left.get("query")
                        .and_then(Json::str)
                        .unwrap_or("")
                        .to_string(),
                    candidate.clone(),
                    a,
                    b,
                ));
            }
        }
    }
    if total == 0 {
        return Ok(Agreement {
            total,
            raw: 0.0,
            kappa: 0.0,
            matrix,
            disagreements,
        });
    }
    let raw = agree as f64 / total as f64;
    let rows: [usize; 3] = [
        matrix[0].iter().sum(),
        matrix[1].iter().sum(),
        matrix[2].iter().sum(),
    ];
    let columns: [usize; 3] = [
        matrix[0][0] + matrix[1][0] + matrix[2][0],
        matrix[0][1] + matrix[1][1] + matrix[2][1],
        matrix[0][2] + matrix[1][2] + matrix[2][2],
    ];
    let expected = (0..3)
        .map(|index| rows[index] as f64 * columns[index] as f64)
        .sum::<f64>()
        / (total as f64 * total as f64);
    let kappa = if expected >= 1.0 {
        1.0
    } else {
        (raw - expected) / (1.0 - expected)
    };
    Ok(Agreement {
        total,
        raw,
        kappa,
        matrix,
        disagreements,
    })
}

fn agree(args: &[String]) -> Result<(), Fail> {
    validate(args, &["--a", "--b"], &[])?;
    let left_path = require(args, "--a")?;
    let right_path = require(args, "--b")?;
    let left = load_json(left_path)?;
    let right = load_json(right_path)?;
    let result = agreement(cases_of(&left), cases_of(&right))?;
    if result.total == 0 {
        println!("blind: no common adjudicated candidates");
        return Ok(());
    }
    let off_diag = (0..3)
        .flat_map(|a| (0..3).map(move |b| (a, b)))
        .filter(|(a, b)| a != b)
        .map(|(a, b)| result.matrix[a][b])
        .sum::<usize>();
    println!(
        "blind: common={} raw={:.3} kappa={:.3}",
        result.total, result.raw, result.kappa
    );
    println!(
        "  matrix(rows=A,cols=B) 2,2={} 1,1={} 0,0={} off-diag={off_diag}",
        result.matrix[2][2], result.matrix[1][1], result.matrix[0][0]
    );
    if result.disagreements.is_empty() {
        println!("  disagreements: 0");
    } else {
        println!("  disagreements:");
        for (case, query, candidate, a, b) in &result.disagreements {
            println!("    case={case} q=\"{query}\" cand={candidate} A={a} B={b}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// review (interactive)
// ---------------------------------------------------------------------------

fn review(args: &[String]) -> Result<(), Fail> {
    validate(args, &["--packet"], &[])?;
    let packet = require(args, "--packet")?;
    let (head, mut rows) = read_tsv(packet)?;
    let columns: HashMap<&str, usize> = head
        .iter()
        .enumerate()
        .map(|(index, key)| (key.as_str(), index))
        .collect();
    let grade_index = *columns
        .get("grade")
        .ok_or_else(|| fail("blind: packet missing grade column"))?;
    let todo: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row[grade_index].trim().is_empty())
        .map(|(index, _)| index)
        .collect();
    println!(
        "blind: {}/{} already graded; {} to go",
        rows.len() - todo.len(),
        rows.len(),
        todo.len()
    );
    println!("blind: {GRADE_HELP}  (s=skip  q=save&quit)");
    let mut stop = false;
    for index in todo {
        if stop {
            break;
        }
        let row = &rows[index];
        println!(
            "\n[row {}/{}] layer={}\nquery: {}\nhint:  {}",
            row[columns["row"]],
            rows.len(),
            row[columns["layer"]],
            row[columns["query"]],
            row[columns["hint"]]
        );
        loop {
            print!("grade? [2/1/0/s/q] > ");
            let _ = stdio::stdout().flush();
            let mut line = String::new();
            if stdio::stdin().read_line(&mut line).unwrap_or(0) == 0 {
                stop = true;
                break;
            }
            let answer = line.trim().to_lowercase();
            if answer == "q" {
                stop = true;
                break;
            }
            if matches!(answer.as_str(), "2" | "1" | "0") {
                rows[index][grade_index] = answer;
                write_text(packet, &tsv_text(&head, &rows))?;
                break;
            }
            if answer == "s" {
                break;
            }
        }
    }
    let graded = rows
        .iter()
        .filter(|row| matches!(row[grade_index].trim(), "0" | "1" | "2"))
        .count();
    println!("\nblind: graded={graded}/{} (saved)", rows.len());
    if graded == rows.len() {
        Ok(())
    } else {
        Err(Fail {
            msg: String::new(),
            code: 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv_roundtrip() {
        let dir = crate::testsupport::tmpdir("blind-tsv");
        let path = format!("{dir}.tsv");
        let head: Vec<String> = PACKET_HEAD.map(str::to_string).to_vec();
        let rows = vec![vec![
            "1".to_string(),
            "0".to_string(),
            "literal".to_string(),
            "q".to_string(),
            "c".to_string(),
            "h".to_string(),
            "2".to_string(),
        ]];
        write_text(&path, &tsv_text(&head, &rows)).unwrap();
        let (got_head, got_rows) = read_tsv(&path).unwrap();
        assert_eq!(got_head, head);
        assert_eq!(got_rows, rows);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn filled_packet_materializes_grades() {
        let value =
            scx::json::parse(r#"{"cases":[{"case_index":0,"rel":{"a":2,"b":1}}]}"#).unwrap();
        let head: Vec<String> = PACKET_HEAD.map(str::to_string).to_vec();
        let rows = vec![
            vec![
                "1".into(),
                "0".into(),
                "literal".into(),
                "q".into(),
                "a".into(),
                "h".into(),
                "".into(),
            ],
            vec![
                "2".into(),
                "0".into(),
                "literal".into(),
                "q".into(),
                "b".into(),
                "h".into(),
                "".into(),
            ],
        ];
        let body = filled_packet(&head, &rows, cases_of(&value)).unwrap();
        assert!(body.contains("\t2\n") || body.contains("\t2\r\n"));
        assert!(body.contains("\t1\n") || body.contains("\t1\r\n"));
    }

    #[test]
    fn agreement_is_identity_on_same_labels() {
        let value = scx::json::parse(
            r#"{"cases":[{"query":"q","rel":{"a":2,"b":1}},{"query":"q2","rel":{"c":2,"d":0}}]}"#,
        )
        .unwrap();
        let result = agreement(cases_of(&value), cases_of(&value)).unwrap();
        assert_eq!(result.total, 4);
        assert_eq!(result.raw, 1.0);
        assert_eq!(result.kappa, 1.0);
        assert_eq!(result.matrix[2][2], 2);
        assert!(result.disagreements.is_empty());
    }
}
