//! 语料生命周期（feature = "eval"）：`extract` 从 Claude Code JSONL 提取压缩
//! 语料，`seed` 校验语料并写入记忆库 cases 与 eval CSV。两者共享语料校验、
//! 二进制定位与原子写入约定。
//!
//! 迁移自 `tools/scx.py seed`：语料校验、安全清理记忆目录、并发调用
//! `scx --store`，按语料顺序输出 CSV。文本以字节写 stdin，保持跨平台逐字节稳定。

// 语料校验分支线性；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::io;
use super::{case_tail, default_binary, fail, home, load_json as load, Fail};
use scx::json::Json;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const LAYERS: [&str; 3] = ["literal", "paraphrase", "semantic"];
const MAX_JOBS: usize = 32;

struct SeedOpts {
    corpus: String,
    mem_dir: String,
    csv: Option<String>,
    binary: Option<String>,
    jobs: usize,
    force: bool,
}

struct Item {
    source: String,
    root_cause: String,
    fix: String,
    store_query: String,
    text_query: String,
    eval_query: String,
    layer: String,
}

struct Row {
    query: String,
    layer: String,
    case_id: String,
    note: String,
}

pub(crate) fn seed(args: &[String]) -> Result<(), Fail> {
    let opts = parse_seed(args)?;
    if opts.jobs < 1 || opts.jobs > MAX_JOBS {
        return Err(fail(format!("seed: --jobs must be in 1..={MAX_JOBS}")));
    }
    let value = load(Path::new(&opts.corpus))?;
    let layers = validate_corpus(&value).map_err(fail)?;
    let binary = opts.binary.clone().or_else(default_binary).ok_or_else(|| {
        fail("seed: missing release binary; run `cargo build --release --locked` first")
    })?;
    if !Path::new(&binary).is_file() {
        return Err(fail(
            "seed: missing release binary; run `cargo build --release --locked` first",
        ));
    }
    let mem_dir = prepare_mem_dir(&opts.mem_dir, opts.force).map_err(fail)?;
    let items = items(&value);
    if items.is_empty() {
        return Err(fail("seed: corpus has no cases"));
    }
    let rows = run_items(&binary, &mem_dir, &items, opts.jobs)?;
    if let Some(csv) = &opts.csv {
        let rows: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                vec![
                    row.query.clone(),
                    row.layer.clone(),
                    row.case_id.clone(),
                    row.note.clone(),
                ]
            })
            .collect();
        let fields = vec![
            "query".to_string(),
            "layer".to_string(),
            "case_id".to_string(),
            "note".to_string(),
        ];
        io::write_csv(csv, &fields, &rows).map_err(fail)?;
    }
    let name = value.get("name").and_then(Json::str).unwrap_or("None");
    println!(
        "seed: corpus={name} cases={} literal={} paraphrase={} semantic={} jobs={} mem={}",
        rows.len(),
        layers[0],
        layers[1],
        layers[2],
        opts.jobs,
        opts.mem_dir
    );
    if let Some(csv) = &opts.csv {
        println!("seed: wrote {csv}");
    }
    Ok(())
}

fn parse_seed(args: &[String]) -> Result<SeedOpts, Fail> {
    let mut opts = SeedOpts {
        corpus: String::new(),
        mem_dir: String::new(),
        csv: None,
        binary: None,
        jobs: 4,
        force: false,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--mem-dir" | "--csv" | "--binary" | "--jobs" => {
                let flag = args[i].as_str();
                i += 1;
                let value = args
                    .get(i)
                    .cloned()
                    .ok_or_else(|| usage_seed(format!("{flag} expects a value")))?;
                match flag {
                    "--mem-dir" => opts.mem_dir = value,
                    "--csv" => opts.csv = Some(value),
                    "--binary" => opts.binary = Some(value),
                    "--jobs" => {
                        opts.jobs = value
                            .parse()
                            .map_err(|_| usage_seed(format!("invalid --jobs: {value}")))?
                    }
                    _ => {}
                }
            }
            "--force" => opts.force = true,
            "-h" | "--help" => return Err(usage_seed(String::new())),
            other if other.starts_with('-') => {
                return Err(usage_seed(format!("unrecognized argument: {other}")))
            }
            other if opts.corpus.is_empty() => opts.corpus = other.to_string(),
            other => return Err(usage_seed(format!("unexpected argument: {other}"))),
        }
        i += 1;
    }
    if opts.corpus.is_empty() {
        return Err(usage_seed("the following arguments are required: corpus"));
    }
    if opts.mem_dir.is_empty() {
        return Err(usage_seed(
            "the following arguments are required: --mem-dir",
        ));
    }
    Ok(opts)
}

fn usage_seed(message: impl Into<String>) -> Fail {
    let body = "usage: xtask seed <corpus> --mem-dir DIR [--csv PATH] [--binary PATH] [--jobs N] [--force]\n";
    let message = message.into();
    Fail {
        msg: if message.is_empty() {
            body.to_string()
        } else {
            format!("{message}\n\n{body}")
        },
        code: 2,
    }
}

fn items(value: &Json) -> Vec<Item> {
    let mut out = Vec::new();
    let Some(Json::Arr(incidents)) = value.get("incidents") else {
        return out;
    };
    for incident in incidents {
        let source = field(incident, "source");
        let root_cause = field(incident, "root_cause");
        let fix = field(incident, "fix");
        let Some(Json::Arr(cases)) = incident.get("cases") else {
            continue;
        };
        for case in cases {
            let store_query = field(case, "store_query");
            let eval_query = field(case, "eval_query");
            let text_value = field(case, "text_query");
            let text_query = if text_value.is_empty() {
                store_query.clone()
            } else {
                text_value
            };
            out.push(Item {
                source: source.clone(),
                root_cause: root_cause.clone(),
                fix: fix.clone(),
                store_query,
                text_query,
                eval_query,
                layer: field(case, "layer"),
            });
        }
    }
    out
}

fn field(value: &Json, key: &str) -> String {
    value
        .get(key)
        .and_then(Json::str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub(crate) fn validate_corpus(value: &Json) -> Result<[usize; 3], String> {
    let incidents = match value.get("incidents") {
        Some(Json::Arr(items)) => items.as_slice(),
        _ => &[],
    };
    let expected_incidents = value
        .get("expected_incidents")
        .and_then(Json::num)
        .map(|n| n as usize)
        .unwrap_or(incidents.len());
    if incidents.len() != expected_incidents {
        return Err(format!(
            "corpus: incidents={} expected={expected_incidents}",
            incidents.len()
        ));
    }
    let mut seen_ids = std::collections::HashSet::new();
    let mut seen_store = std::collections::HashSet::new();
    let mut seen_eval = std::collections::HashSet::new();
    let mut layers = [0usize; 3];
    let mut total = 0usize;
    for incident in incidents {
        if !matches!(incident, Json::Obj(_)) {
            return Err("corpus: every incident must be an object".to_string());
        }
        let ident = field(incident, "id");
        let source = field(incident, "source");
        let root_cause = field(incident, "root_cause");
        let fix = field(incident, "fix");
        if ident.is_empty() || !seen_ids.insert(ident.clone()) {
            return Err(format!(
                "corpus: missing or duplicate incident id: {ident:?}"
            ));
        }
        if !(source.starts_with("http://") || source.starts_with("https://")) {
            return Err(format!("corpus: incident {ident} has no public source URL"));
        }
        if root_cause.is_empty() || fix.is_empty() {
            return Err(format!("corpus: incident {ident} has empty root_cause/fix"));
        }
        for (label, text) in [
            ("id", &ident),
            ("source", &source),
            ("root_cause", &root_cause),
            ("fix", &fix),
        ] {
            if !is_line(text) {
                return Err(format!(
                    "corpus: incident {ident} {label}: must be one line without tabs or NUL"
                ));
            }
        }
        let Some(Json::Arr(cases)) = incident.get("cases") else {
            return Err(format!("corpus: incident {ident} has no cases"));
        };
        if cases.is_empty() {
            return Err(format!("corpus: incident {ident} has no cases"));
        }
        for case in cases {
            if !matches!(case, Json::Obj(_)) {
                return Err(format!("corpus: incident {ident} case must be an object"));
            }
            let layer = field(case, "layer");
            let store_query = field(case, "store_query");
            let eval_query = field(case, "eval_query");
            let text_value = field(case, "text_query");
            let text_query = if text_value.is_empty() {
                store_query.clone()
            } else {
                text_value
            };
            let Some(layer_index) = LAYERS.iter().position(|item| *item == layer) else {
                return Err(format!("corpus: incident {ident} has bad layer: {layer:?}"));
            };
            if store_query.is_empty() || eval_query.is_empty() || text_query.is_empty() {
                return Err(format!("corpus: incident {ident} has an empty query"));
            }
            for (label, text) in [
                ("store_query", &store_query),
                ("eval_query", &eval_query),
                ("text_query", &text_query),
            ] {
                if !is_line(text) {
                    return Err(format!(
                        "corpus: incident {ident} {label}: must be one line without tabs or NUL"
                    ));
                }
            }
            let normalized = store_query.to_lowercase();
            if !seen_store.insert(normalized) {
                return Err(format!("corpus: duplicate stored query: {store_query:?}"));
            }
            let normalized_eval = eval_query.to_lowercase();
            if !seen_eval.insert(normalized_eval) {
                return Err(format!("corpus: duplicate eval query: {eval_query:?}"));
            }
            layers[layer_index] += 1;
            total += 1;
        }
    }
    let expected_cases = value
        .get("expected_cases")
        .and_then(Json::num)
        .map(|n| n as usize)
        .unwrap_or(total);
    if total != expected_cases {
        return Err(format!("corpus: cases={total} expected={expected_cases}"));
    }
    Ok(layers)
}

fn is_line(text: &str) -> bool {
    !text.contains('\r') && !text.contains('\n') && !text.contains('\t') && !text.contains('\0')
}

pub(crate) fn is_memory_dir(path: &Path) -> bool {
    path.join("meta.toml").is_file() && path.join("cases").is_dir()
}

pub(crate) fn prepare_mem_dir(mem_dir: &str, force: bool) -> Result<PathBuf, String> {
    let path = if Path::new(mem_dir).exists() {
        fs::canonicalize(mem_dir).map_err(|e| format!("{mem_dir}: {e}"))?
    } else {
        std::path::absolute(mem_dir).map_err(|e| format!("{mem_dir}: {e}"))?
    };
    if path.exists() && !path.is_dir() {
        return Err(format!(
            "seed: memory path is not a directory: {}",
            path.display()
        ));
    }
    let non_empty = fs::read_dir(&path)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    if path.exists() && non_empty {
        if !force {
            return Err(format!(
                "seed: memory dir is not empty: {} (use --force)",
                path.display()
            ));
        }
        let roots = roots();
        if roots
            .iter()
            .any(|root| root == &path || root.starts_with(&path))
        {
            return Err(format!(
                "seed: refusing to clear a workspace or temp root: {}",
                path.display()
            ));
        }
        if !is_memory_dir(&path) {
            return Err(format!(
                "seed: refusing to clear a non-memory directory: {} (remove it manually)",
                path.display()
            ));
        }
        if !roots.iter().any(|root| path.starts_with(root)) {
            return Err(format!(
                "seed: refusing to clear outside workspace/temp roots: {}",
                path.display()
            ));
        }
        fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for path in [Path::new(ROOT).to_path_buf(), std::env::temp_dir()] {
        if let Ok(path) = fs::canonicalize(&path) {
            if !roots.contains(&path) {
                roots.push(path);
            }
        }
    }
    #[cfg(unix)]
    {
        if let Ok(path) = fs::canonicalize("/tmp") {
            if !roots.contains(&path) {
                roots.push(path);
            }
        }
    }
    roots
}

fn run_items(binary: &str, mem_dir: &Path, items: &[Item], jobs: usize) -> Result<Vec<Row>, Fail> {
    let workers = jobs.min(items.len()).max(1);
    let results: Mutex<Vec<Option<Row>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    let error: Mutex<Option<String>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if error.lock().map(|value| value.is_some()).unwrap_or(true) {
                    break;
                }
                let index = next.fetch_add(1, Ordering::SeqCst);
                if index >= items.len() {
                    break;
                }
                match seed_case(binary, mem_dir, &items[index]) {
                    Ok(row) => {
                        if let Ok(mut slots) = results.lock() {
                            slots[index] = Some(row);
                        }
                    }
                    Err(message) => {
                        if let Ok(mut slot) = error.lock() {
                            *slot = Some(message);
                        }
                        break;
                    }
                }
            });
        }
    });
    if let Some(message) = error.into_inner().unwrap_or(None) {
        return Err(fail(message));
    }
    let slots = results.into_inner().unwrap_or_default();
    let mut rows = Vec::with_capacity(slots.len());
    for slot in slots {
        match slot {
            Some(row) => rows.push(row),
            None => return Err(fail("seed: internal error: missing row")),
        }
    }
    Ok(rows)
}

fn seed_case(binary: &str, mem_dir: &Path, item: &Item) -> Result<Row, String> {
    let text = format!(
        "{}\n公开复盘, 来源: {}\n根因: {}",
        item.text_query, item.source, item.root_cause
    );
    let mut child = Command::new(binary)
        .args([
            "--query",
            &item.store_query,
            "--store",
            "1",
            "--fix",
            &item.fix,
            "--mem-dir",
            &mem_dir.to_string_lossy(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("seed: spawn {binary}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("seed: stdin: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("seed: wait: {e}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let case_id = parse_case_id(&text)
        .ok_or_else(|| format!("seed: store failed for {:?}\n{text}", item.store_query))?;
    if !output.status.success() {
        return Err(format!(
            "seed: store failed for {:?}\n{text}",
            item.store_query
        ));
    }
    Ok(Row {
        query: item.eval_query.clone(),
        layer: item.layer.clone(),
        case_id: case_id.clone(),
        note: format!("target={} source={}", case_tail(&case_id), item.source),
    })
}

fn parse_case_id(output: &str) -> Option<String> {
    let marker = "stored case=";
    let start = output.find(marker)? + marker.len();
    let rest = &output[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let case_id = &rest[..end];
    if case_id.is_empty() || !rest[end..].starts_with(" added=yes") {
        return None;
    }
    Some(case_id.to_string())
}

struct ExtractOpts {
    dir: String,
    out: String,
    extract: Option<usize>,
}

struct ExtractStat {
    path: PathBuf,
    use_n: usize,
    res_n: usize,
    use_c: usize,
    res_c: usize,
}

pub(crate) fn extract(args: &[String]) -> Result<(), Fail> {
    let opts = parse_extract(args)?;
    let mut files: Vec<PathBuf> = fs::read_dir(&opts.dir)
        .map_err(|e| fail(format!("{}: {e}", opts.dir)))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    files.sort();
    let mut stats = Vec::new();
    for path in files {
        let blocks = blocks(&path)?;
        let mut use_n = 0usize;
        let mut res_n = 0usize;
        let mut use_c = 0usize;
        let mut res_c = 0usize;
        for (kind, text) in &blocks {
            if kind == "use" {
                use_n += 1;
                use_c += text.chars().count();
            } else {
                res_n += 1;
                res_c += text.chars().count();
            }
        }
        stats.push(ExtractStat {
            path,
            use_n,
            res_n,
            use_c,
            res_c,
        });
    }
    stats.sort_by_key(|stat| std::cmp::Reverse(stat.res_c));
    for stat in &stats {
        let base: String = basename(&stat.path).chars().take(12).collect();
        println!(
            "{base:<14} tool_use={:>4} tool_result={:>4} use_chars={:>9} result_chars={:>9}",
            stat.use_n, stat.res_n, stat.use_c, stat.res_c
        );
    }
    if let Some(count) = opts.extract.filter(|count| *count > 0) {
        fs::create_dir_all(&opts.out).map_err(|e| fail(format!("{}: {e}", opts.out)))?;
        for stat in stats.iter().take(count) {
            let base = stat
                .path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            let out = Path::new(&opts.out).join(format!("{base}.dump.txt"));
            let mut text = String::new();
            for (_, block) in blocks(&stat.path)? {
                text.push_str(&block);
                if !block.is_empty() && !block.ends_with('\n') {
                    text.push('\n');
                }
            }
            fs::write(&out, &text).map_err(|e| fail(format!("{}: {e}", out.display())))?;
            let lines = fs::read_to_string(&out)
                .map(|text| text.lines().count())
                .unwrap_or(0);
            println!("  -> {} ({lines} lines)", out.display());
        }
    }
    Ok(())
}

fn parse_extract(args: &[String]) -> Result<ExtractOpts, Fail> {
    let mut opts = ExtractOpts {
        dir: home()
            .join(".claude/projects/-Users-fei-code-scx")
            .to_string_lossy()
            .to_string(),
        out: std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string()),
        extract: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" | "--out" | "--extract" => {
                let flag = args[i].as_str();
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| usage_extract(format!("{flag} expects a value")))?;
                match flag {
                    "--dir" => opts.dir = value.clone(),
                    "--out" => opts.out = value.clone(),
                    "--extract" => {
                        opts.extract =
                            Some(value.parse().map_err(|_| {
                                usage_extract(format!("invalid --extract: {value}"))
                            })?)
                    }
                    _ => {}
                }
            }
            "--scan" => {}
            "-h" | "--help" => return Err(usage_extract(String::new())),
            other => return Err(usage_extract(format!("unrecognized argument: {other}"))),
        }
        i += 1;
    }
    Ok(opts)
}

fn usage_extract(message: impl Into<String>) -> Fail {
    let body = "usage: xtask extract [--dir DIR] [--scan] [--extract N] [--out DIR]\n";
    let message = message.into();
    Fail {
        msg: if message.is_empty() {
            body.to_string()
        } else {
            format!("{message}\n\n{body}")
        },
        code: 2,
    }
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn blocks(path: &Path) -> Result<Vec<(String, String)>, Fail> {
    let text = fs::read_to_string(path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(value) = scx::json::parse(line) else {
            continue;
        };
        let Json::Obj(object) = &value else {
            continue;
        };
        let kind = block_field(object, "type");
        if !matches!(kind.as_str(), "user" | "assistant") {
            continue;
        }
        let message = match object.iter().find(|(key, _)| key == "message") {
            Some((_, Json::Obj(message))) => message,
            _ => continue,
        };
        let content = match message.iter().find(|(key, _)| key == "content") {
            Some((_, Json::Arr(content))) => content,
            _ => continue,
        };
        for block in content {
            let Json::Obj(block) = block else {
                continue;
            };
            match block_field(block, "type").as_str() {
                "tool_use" => {
                    let Some(input) = block
                        .iter()
                        .find(|(key, _)| key == "input")
                        .map(|(_, value)| value)
                    else {
                        continue;
                    };
                    match input {
                        Json::Obj(entries) => {
                            if let Some(command) = find_str(entries, "command") {
                                out.push(("use".to_string(), format!("command: {command}")));
                            } else if let Some(content) = find_str(entries, "content") {
                                out.push(("use".to_string(), content.to_string()));
                            } else {
                                out.push(("use".to_string(), dumps(input)));
                            }
                        }
                        Json::Str(text) => out.push(("use".to_string(), text.clone())),
                        _ => {}
                    }
                }
                "tool_result" => {
                    let Some(inner) = block
                        .iter()
                        .find(|(key, _)| key == "content")
                        .map(|(_, value)| value)
                    else {
                        continue;
                    };
                    match inner {
                        Json::Arr(items) => {
                            let mut parts = Vec::new();
                            for sub in items {
                                match sub {
                                    Json::Obj(entries)
                                        if find_str(entries, "type") == Some("text") =>
                                    {
                                        parts.push(
                                            find_str(entries, "text").unwrap_or("").to_string(),
                                        );
                                    }
                                    Json::Str(text) => parts.push(text.clone()),
                                    _ => {}
                                }
                            }
                            out.push(("result".to_string(), parts.join("\n")));
                        }
                        Json::Str(text) => out.push(("result".to_string(), text.clone())),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

fn block_field(object: &[(String, Json)], key: &str) -> String {
    find_str(object, key).unwrap_or("").to_string()
}

fn find_str<'a>(object: &'a [(String, Json)], key: &str) -> Option<&'a str> {
    object
        .iter()
        .find(|(name, _)| name == key)
        .and_then(|(_, value)| value.str())
}

/// Python `json.dumps(..., ensure_ascii=False)` 的紧凑格式。
fn dumps(value: &Json) -> String {
    match value {
        Json::Null => "null".to_string(),
        Json::Bool(flag) => flag.to_string(),
        Json::Num(number) if number.fract() == 0.0 && number.abs() < 9e15 => {
            (*number as i64).to_string()
        }
        Json::Num(number) => io::repr(*number),
        Json::Str(text) => io::json_string(text),
        Json::Arr(items) => {
            let body = items.iter().map(dumps).collect::<Vec<_>>().join(", ");
            format!("[{body}]")
        }
        Json::Obj(entries) => {
            let body = entries
                .iter()
                .map(|(key, value)| format!("{}: {}", io::json_string(key), dumps(value)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{body}}}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_parses_tool_blocks_and_compact_json() {
        let dir = crate::testsupport::tmpdir("extract");
        let path = PathBuf::from(&dir).with_extension("jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"input\":{\"command\":\"ls -la\"}}]}}\n",
                "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":[{\"type\":\"text\",\"text\":\"a\"},{\"type\":\"text\",\"text\":\"b\"}]}]}}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"input\":{\"foo\":\"bar\",\"n\":2}}]}}\n",
                "not json\n"
            ),
        )
        .unwrap();
        let blocks = blocks(&path).unwrap();
        assert_eq!(
            blocks[0],
            ("use".to_string(), "command: ls -la".to_string())
        );
        assert_eq!(blocks[1], ("result".to_string(), "a\nb".to_string()));
        assert_eq!(
            blocks[2],
            (
                "use".to_string(),
                "{\"foo\": \"bar\", \"n\": 2}".to_string()
            )
        );
        assert_eq!(
            dumps(&scx::json::parse("{\"a\":[1,2]}").unwrap()),
            "{\"a\": [1, 2]}"
        );
        let _ = fs::remove_file(path);
    }
    #[test]
    fn validates_seed_fixture() {
        let assets = scx::json::parse(include_str!("../../config/tests/assets.json")).unwrap();
        let text = assets
            .get("tooling")
            .and_then(|value| value.get("eval/seed/corpus.json"))
            .and_then(Json::str)
            .unwrap();
        let value = scx::json::parse(text).unwrap();
        assert_eq!(validate_corpus(&value).unwrap(), [1, 1, 0]);
    }

    #[test]
    fn rejects_duplicate_queries() {
        let value = scx::json::parse(
            r#"{"incidents":[{"id":"a","source":"https://x","root_cause":"r","fix":"f",
            "cases":[{"layer":"literal","store_query":"q","eval_query":"e"},
                     {"layer":"literal","store_query":"Q","eval_query":"e2"}]}]}"#,
        )
        .unwrap();
        assert!(validate_corpus(&value).is_err());
    }

    #[test]
    fn memory_dir_detection() {
        let dir = crate::testsupport::tmpdir("seed-mem");
        let path = PathBuf::from(&dir);
        assert!(!is_memory_dir(&path));
        fs::create_dir_all(path.join("cases")).unwrap();
        assert!(!is_memory_dir(&path));
        fs::write(path.join("meta.toml"), "schema = 2\n").unwrap();
        assert!(is_memory_dir(&path));
        let _ = fs::remove_dir_all(dir);
    }
}
