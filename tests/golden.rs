//! golden 差分门：与冻结基线逐字节对比（stdout / stderr / 退出码）。
//!
//! 用例清单与期望输出统一存放在 `config/tests/assets.json`：`golden.manifest`
//! 保存名称 + CLI 参数，`golden.expected` 保存 stdout / stderr / 退出码。
//!
//! 流程：
//! 1) 首次或有意变更语义时重冻基线：
//!    `cargo test --test golden update_baselines -- --ignored --nocapture`，
//!    重建 config/tests/assets.json 的 golden.expected。
//! 2) 常规回归（CI 执行）：`cargo test --test golden`。
//!
//! 语义约束：本测试只对比当前实现与冻结基线的字节一致性，不判断对错；
//! 基线合理性由人工审查 + 库/CLI 单元测试共同保证。`case=` 后的
//! `YYYY-MM-DD_` 前缀是摄取当天的元数据，不属于输出语义，比较前统一归一化。

#[cfg(feature = "memory")]
use std::collections::{BTreeMap, HashMap};
#[cfg(feature = "memory")]
use std::io::Write;
#[cfg(feature = "memory")]
use std::path::Path;
#[cfg(feature = "memory")]
use std::process::{Command, Stdio};
#[cfg(feature = "memory")]
use std::sync::OnceLock;

#[cfg(feature = "memory")]
const ASSETS: &str = include_str!("../config/tests/assets.json");

#[cfg(feature = "memory")]
const SEED_LOG: &str = "INFO start service\nERROR db timeout conn_id=9f2a pool exhausted\nat com.ingest.Pool.acquire(Pool.java:214)\nINFO end\n";

#[cfg(feature = "memory")]
const SEED_DISK: &str =
    "INFO start\nWARN disk usage 97% mount /var\nERROR errno=28 no space left\n";

#[cfg(feature = "memory")]
struct Expected {
    stdout: String,
    stderr: String,
    code: i32,
}

#[cfg(feature = "memory")]
fn baseline_key() -> &'static str {
    // 语义特性会确定性改变保留决策，须与默认构建分别冻结，避免单基线跨特性强等。
    if cfg!(feature = "semantic") {
        "semantic_expected"
    } else {
        "expected"
    }
}

#[cfg(feature = "memory")]
fn expected() -> &'static HashMap<String, Expected> {
    static CASES: OnceLock<HashMap<String, Expected>> = OnceLock::new();
    CASES.get_or_init(|| {
        let value = scx::json::parse(ASSETS).expect("test assets JSON 非法");
        let Some(scx::json::Json::Obj(entries)) = value
            .get("golden")
            .and_then(|value| value.get(baseline_key()))
        else {
            panic!("golden.{} 必须是对象", baseline_key())
        };
        entries
            .iter()
            .map(|(name, value)| {
                let stdout = value
                    .get("stdout")
                    .and_then(scx::json::Json::str)
                    .unwrap_or_default()
                    .to_string();
                let stderr = value
                    .get("stderr")
                    .and_then(scx::json::Json::str)
                    .unwrap_or_default()
                    .to_string();
                let code = value
                    .get("code")
                    .and_then(scx::json::Json::num)
                    .unwrap_or(0.0) as i32;
                (
                    name.clone(),
                    Expected {
                        stdout,
                        stderr,
                        code,
                    },
                )
            })
            .collect()
    })
}

#[cfg(feature = "memory")]
fn run_scx(args: &[&str], input: &str) -> (String, String, i32) {
    let bin = env!("CARGO_BIN_EXE_scx");
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn scx: {e}"));
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// 归一化 `case=YYYY-MM-DD_<hash16>` 中的日期前缀。
/// 只处理已校验的日期与 16 位十六进制 hash，避免误改普通日志内容。
fn normalize_case_dates(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0usize;
    let mut i = 0usize;
    while i + 32 <= bytes.len() {
        let date = &bytes[i + 5..i + 15];
        let is_case_date = bytes[i..i + 5] == *b"case="
            && date[4] == b'-'
            && date[7] == b'-'
            && date
                .iter()
                .enumerate()
                .all(|(j, b)| j == 4 || j == 7 || b.is_ascii_digit())
            && bytes[i + 15] == b'_'
            && bytes[i + 16..i + 32].iter().all(|b| b.is_ascii_hexdigit());
        if is_case_date {
            out.push_str(&text[copied..i + 5]);
            out.push_str("<date>_");
            i += 16;
            copied = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(feature = "memory")]
struct Case {
    name: String,
    args: Vec<String>,
}

/// 解析 `args = ["a", "b"]`：逗号分隔，引号内的逗号不切分。
#[cfg(feature = "memory")]
fn list(value: &str) -> Vec<String> {
    let inner = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or_else(|| panic!("golden.toml: args 必须是字符串数组: {value}"));
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in inner.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                cur.push(ch);
            }
            ',' if !quoted => {
                out.push(scx::toml::strip(&cur).to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    out.push(scx::toml::strip(&cur).to_string());
    assert!(
        !out.iter().any(String::is_empty),
        "golden.toml: args 含空项: {value}"
    );
    out
}

#[cfg(feature = "memory")]
fn manifest() -> Vec<Case> {
    let mut out: Vec<Case> = Vec::new();
    let value = scx::json::parse(ASSETS).expect("test assets JSON 非法");
    let manifest = value
        .get("golden")
        .and_then(|value| value.get("manifest"))
        .and_then(scx::json::Json::str)
        .expect("golden.manifest");
    for entry in scx::toml::parse(manifest.lines()) {
        if entry.section != "[case]" {
            continue;
        }
        match entry.key {
            "name" => out.push(Case {
                name: scx::toml::strip(entry.value).to_string(),
                args: Vec::new(),
            }),
            "args" => {
                let case = out
                    .last_mut()
                    .unwrap_or_else(|| panic!("golden.toml: args 出现在 name 之前"));
                case.args = list(entry.value);
            }
            key => panic!("golden.toml: 未知键 {key}"),
        }
    }
    assert!(!out.is_empty(), "golden.toml: 无用例");
    for case in &out {
        assert!(
            !case.args.is_empty(),
            "golden.toml: {} 缺少 args",
            case.name
        );
    }
    out
}

#[cfg(feature = "memory")]
fn mem_dir(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("scx-golden-mem-{name}"))
        .to_string_lossy()
        .into_owned()
}

#[cfg(feature = "memory")]
fn eval_baseline() -> String {
    std::env::temp_dir()
        .join("scx-golden-eval-baseline.json")
        .to_string_lossy()
        .into_owned()
}

#[cfg(feature = "memory")]
#[derive(Default)]
struct Env {
    rate_id: String,
}

#[cfg(feature = "memory")]
fn resolve(raw: &str, env: &Env) -> String {
    if let Some(name) = raw.strip_prefix("${mem:").and_then(|s| s.strip_suffix('}')) {
        return mem_dir(name);
    }
    match raw {
        "${rate_id}" => env.rate_id.clone(),
        "${eval_baseline}" => eval_baseline(),
        _ => raw.to_string(),
    }
}

/// 记忆库预置：写入一条 case，可选附带修复结论。
#[cfg(feature = "memory")]
fn seed(dir: &str, query: &str, input: &str, fix: Option<&str>) {
    let mut args = vec!["--query", query, "--store", "1"];
    if let Some(fix) = fix {
        args.push("--fix");
        args.push(fix);
    }
    args.push("--mem-dir");
    args.push(dir);
    let _ = run_scx(&args, input);
}

/// 各记忆用例的前置步骤（重置隔离库、预热或制造坏段），在用例列表中按序执行。
#[cfg(feature = "memory")]
fn prepare(name: &str, env: &mut Env) {
    match name {
        "memory_store" => {
            let _ = std::fs::remove_dir_all(mem_dir("store"));
        }
        "memory_recall" => {
            let dir = mem_dir("recall");
            let _ = std::fs::remove_dir_all(&dir);
            seed(&dir, "db timeout conn_id", SEED_LOG, None);
        }
        "memory_rate" => {
            let dir = mem_dir("rate");
            let _ = std::fs::remove_dir_all(&dir);
            seed(&dir, "db timeout conn_id", SEED_LOG, None);
            let cases = Path::new(&dir).join("cases");
            env.rate_id = std::fs::read_dir(&cases)
                .unwrap()
                .find_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
                .unwrap_or_default();
        }
        "memory_rebuild" => {
            let dir = mem_dir("rebuild");
            let _ = std::fs::remove_dir_all(&dir);
            seed(&dir, "db timeout conn_id", SEED_LOG, None);
        }
        "memory_gc" => {
            let dir = mem_dir("gc");
            let _ = std::fs::remove_dir_all(&dir);
            seed(&dir, "db timeout conn_id", SEED_LOG, None);
        }
        "memory_corrupt_heal" => {
            let dir = mem_dir("corrupt");
            let _ = std::fs::remove_dir_all(&dir);
            seed(&dir, "db timeout conn_id", SEED_LOG, None);
            let _ = run_scx(&["--rebuild", "--mem-dir", dir.as_str()], "");
            let p = Path::new(&dir).join("index").join("postings.bin");
            let b = std::fs::read(&p).unwrap();
            std::fs::write(&p, &b[..b.len() - 5]).unwrap(); // 截断 crc+body 尾部 → 校验失败
            let _ = run_scx(
                &[
                    "--query",
                    "db timeout conn_id",
                    "--recall",
                    "1",
                    "--mem-dir",
                    dir.as_str(),
                ],
                "",
            );
        }
        "memory_qmap_replay" => {
            let dir = mem_dir("qmap");
            let _ = std::fs::remove_dir_all(&dir);
            seed(&dir, "db timeout conn_id", SEED_LOG, None);
            let _ = run_scx(
                &[
                    "--query",
                    "db timeout conn_id",
                    "--recall",
                    "1",
                    "--mem-dir",
                    dir.as_str(),
                ],
                "",
            );
        }
        "memory_eval" => {
            let dir = mem_dir("eval");
            let _ = std::fs::remove_dir_all(&dir);
            let seeds = [
                ("db timeout conn_id", SEED_LOG),
                (
                    "disk full errno",
                    "ERROR disk full errno=28\nat fs.write(Fs.java:88)\nINFO end\n",
                ),
                (
                    "queue depth",
                    "WARN queue depth=40 backlog\nat Queue.push(Queue.java:9)\nINFO end\n",
                ),
            ];
            for (q, t) in seeds {
                seed(&dir, q, t, None);
            }
            let ev_id = |q: &str| -> String {
                let cases = Path::new(&dir).join("cases");
                let mut out = String::new();
                for e in std::fs::read_dir(&cases).unwrap().filter_map(|e| e.ok()) {
                    let id = e.file_name().to_string_lossy().into_owned();
                    let cj =
                        std::fs::read_to_string(e.path().join("case.json")).unwrap_or_default();
                    if cj.contains(&format!("\"query\":\"{q}\"")) {
                        out = id;
                        break;
                    }
                }
                out
            };
            let t0 = ev_id("db timeout conn_id");
            let t1 = ev_id("disk full errno");
            let h0 = t0.rsplit('_').next().unwrap_or(&t0).to_string();
            let h1 = t1.rsplit('_').next().unwrap_or(&t1).to_string();
            let js = format!(
                "{{\"cases\":[{{\"query\":\"db timeout conn_id\",\"layer\":\"literal\",\"expected\":[\"{h0}\"],\"rel\":{{\"{h0}\":2}}}},{{\"query\":\"磁盘连接超时 conn_id 池\",\"layer\":\"paraphrase\",\"expected\":[\"{h0}\"]}},{{\"query\":\"存储空间不足 errno 磁盘\",\"layer\":\"semantic\",\"expected\":[\"{h1}\"]}}]}}"
            );
            std::fs::write(eval_baseline(), js).unwrap();
        }
        "memory_summary" => {
            let dir = mem_dir("summary");
            let _ = std::fs::remove_dir_all(&dir);
            seed(&dir, "db timeout conn_id", SEED_LOG, None);
        }
        "memory_fix" => {
            let dir = mem_dir("fix");
            let _ = std::fs::remove_dir_all(&dir);
            seed(
                &dir,
                "db timeout conn_id",
                SEED_LOG,
                Some("enlarge pool to 64 conns, restart daemon"),
            );
        }
        "memory_stats" => {
            let dir = mem_dir("stats");
            let _ = std::fs::remove_dir_all(&dir);
            seed(
                &dir,
                "db timeout conn_id",
                SEED_LOG,
                Some("enlarge pool to 64 conns"),
            );
            seed(&dir, "磁盘空间不足", SEED_DISK, None);
        }
        "memory_annotate" => {
            let dir = mem_dir("annotate");
            let _ = std::fs::remove_dir_all(&dir);
            seed(
                &dir,
                "db timeout conn_id",
                SEED_LOG,
                Some("enlarge pool to 64 conns"),
            );
            seed(&dir, "磁盘空间不足 errno", SEED_DISK, None);
        }
        _ => {}
    }
}

#[cfg(feature = "memory")]
fn eng() -> String {
    let mut lines: Vec<String> = (0..200)
        .map(|i| format!("INFO heartbeat seq={i} ok pool=3 latency=1ms"))
        .collect();
    lines[150] = "ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a)".to_string();
    lines[151] = "at com.ingest.Pool.acquire(Pool.java:214)".to_string();
    lines[152] = "Root cause: connection pool size 4 < peak concurrency 11".to_string();
    lines.join("\n")
}

#[cfg(feature = "memory")]
fn cjk() -> String {
    let mut lines: Vec<String> = (0..300)
        .map(|i| format!("INFO 心跳 seq={i} 状态=正常 latency=1ms"))
        .collect();
    lines[150] = "错误：数据库连接超时 (conn_id=9f2a)".to_string();
    lines[151] = "错误：磁盘已满 errno=28".to_string();
    lines[152] = "警告：队列深度=40".to_string();
    lines.join("\n")
}

#[cfg(feature = "memory")]
fn ansi() -> String {
    let mut lines: Vec<String> = (0..200)
        .map(|i| format!("\u{1b}[32mINFO heartbeat {i} ok\u{1b}[0m filler padding text"))
        .collect();
    lines[150] = "\u{1b}[31mERROR db timeout pool exhausted (conn_id=9f2a)\u{1b}[0m".to_string();
    lines[151] = "\u{1b}[33mRoot cause: pool size 4 < peak concurrency 11\u{1b}[0m".to_string();
    lines.join("\n")
}

#[cfg(feature = "memory")]
fn crlf() -> String {
    let mut lines: Vec<String> = (0..200)
        .map(|i| format!("INFO line {i} heartbeat ok filler padding"))
        .collect();
    lines[100] = "ERROR db timeout pool exhausted conn_id=9f2a".to_string();
    lines.join("\r\n")
}

#[cfg(feature = "memory")]
fn diff() -> String {
    [
        "diff --git a/src/lib.rs b/src/lib.rs",
        "index 1389ef2..f5c5c61 100644",
        "--- a/src/lib.rs",
        "+++ b/src/lib.rs",
        "@@ -1,6 +1,9 @@",
        " //! scx 引擎",
        "+use std::collections::HashMap;",
        "+const DEFAULT_RATIO: f64 = 0.35;",
        "-pub const VERSION: &str = \"0.3.9\";",
        "+pub const VERSION: &str = env!(\"CARGO_PKG_VERSION\");",
        " fn is_cjk(c: char) -> bool {",
        "+    let u = c as u32;",
        " filler line",
        " filler line",
        " filler line",
    ]
    .join("\n")
}

#[cfg(feature = "memory")]
fn dups() -> String {
    let mut lines: Vec<String> = (0..200)
        .map(|i| format!("INFO filler {i} alpha beta"))
        .collect();
    for idx in [150usize, 151, 152, 153] {
        lines[idx] = "SPECIAL_FORCE_LINE".to_string();
    }
    lines.join("\n")
}

#[cfg(feature = "memory")]
fn mt1() -> String {
    let mut lines: Vec<String> = (0..200)
        .map(|i| format!("INFO heartbeat seq={i} ok pool=3 latency=1ms"))
        .collect();
    lines[100] = "conn 9f2a broken db crash".to_string();
    lines.join("\n")
}

#[cfg(feature = "memory")]
fn mt2() -> String {
    let mut lines: Vec<String> = (0..200)
        .map(|i| format!("pad alpha line {i} beta gamma"))
        .collect();
    lines[5] = "conn 9f2a broken".to_string();
    lines[6] = "error alpha pipe burst 7a1c now".to_string();
    lines.join("\n")
}

#[cfg(feature = "memory")]
fn structure_fence() -> String {
    let mut lines: Vec<String> = vec![
        "```".to_string(),
        "def run():".to_string(),
        "    conn 9f2a broken db crash".to_string(),
        "    return pool".to_string(),
        "```".to_string(),
    ];
    lines.extend((0..150).map(|i| format!("INFO heartbeat seq={i} ok pool=3 latency=1ms")));
    lines.join("\n")
}

#[cfg(feature = "memory")]
fn structure_import() -> String {
    let mut lines: Vec<String> = vec![
        "import os".to_string(),
        "import sys".to_string(),
        "import conn_pool".to_string(),
        "import threading".to_string(),
    ];
    lines.extend((0..150).map(|i| format!("INFO heartbeat seq={i} ok pool=3 latency=1ms")));
    lines.join("\n")
}

#[cfg(feature = "memory")]
fn structure_stack() -> String {
    let mut lines: Vec<String> = vec![
        "  at com.a.Pool.acquire(Pool.java:214)".to_string(),
        "  at com.b.Runner.main(Runner.java:88)".to_string(),
        "Caused by: java.sql.SQLException: db crash conn 9f2a".to_string(),
        "  at com.c.Conn.open(Conn.java:9)".to_string(),
    ];
    lines.extend((0..150).map(|i| format!("INFO heartbeat seq={i} ok pool=3 latency=1ms")));
    lines.join("\n")
}

#[cfg(feature = "memory")]
fn input(name: &str) -> String {
    match name {
        "small_passthrough" => (0..10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
        "longline_passthrough" => {
            let mut lines = vec!["x".repeat(150_000)];
            lines.extend((0..60).map(|i| format!("filler error kvwrite line {i}")));
            lines.join("\n")
        }
        "zero_signal_failopen" => (0..200)
            .map(|i| format!("line {i} with padding text alpha beta gamma"))
            .collect::<Vec<_>>()
            .join("\n"),
        "no_kept_fallback" => {
            let fat = format!("alpha {}", "x".repeat(5000));
            (0..40).map(|_| fat.clone()).collect::<Vec<_>>().join("\n")
        }
        "sieve_default" | "json_stats" | "stat_only" | "no_stats_body" | "lineno_mode"
        | "elide_off" | "window_needle" => eng(),
        "sieve_needle_cjk" => cjk(),
        "ansi_strip" => ansi(),
        "crlf_sieve" => crlf(),
        "unified_diff" => diff(),
        "force_duplicates" => dups(),
        "memory_store" => SEED_LOG.to_string(),
        "max_tokens_overflow_evidence"
        | "max_tokens_sieve_capped"
        | "json_dropped"
        | "json_dropped_overflow" => mt1(),
        "max_tokens_overflow_fill" => mt2(),
        "structure_fence" => structure_fence(),
        "structure_import" => structure_import(),
        "structure_stack" => structure_stack(),
        name if name.starts_with("memory_") => String::new(),
        other => panic!("golden.toml: 未知用例 {other}（缺少输入生成器）"),
    }
}

#[cfg(feature = "memory")]
fn cases() -> Vec<(String, Vec<String>, String)> {
    let mut env = Env::default();
    let mut out = Vec::new();
    for case in manifest() {
        prepare(&case.name, &mut env);
        let args = case.args.iter().map(|a| resolve(a, &env)).collect();
        out.push((case.name.clone(), args, input(&case.name)));
    }
    out
}

#[test]
#[cfg(feature = "memory")]
fn golden_gate() {
    for (name, args, input) in cases() {
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let (out, err, code) = run_scx(&argv, &input);
        let expected = expected();
        let want = expected.get(&name).unwrap_or_else(|| {
            panic!(
                "缺失基线 {name} —— 先运行: cargo test --test golden update_baselines -- --ignored"
            )
        });
        assert_eq!(code, want.code, "[{name}] 退出码漂移");
        assert_eq!(
            normalize_case_dates(&err),
            normalize_case_dates(&want.stderr),
            "[{name}] stderr 漂移"
        );
        assert_eq!(
            normalize_case_dates(&out),
            normalize_case_dates(&want.stdout),
            "[{name}] stdout 漂移"
        );
    }
}

#[test]
fn case_date_normalization_is_scoped() {
    let older = "stored case=2026-10-22_123e7decc4c4a423 added=yes";
    let newer = "stored case=2026-10-23_123e7decc4c4a423 added=yes";
    assert_eq!(normalize_case_dates(older), normalize_case_dates(newer));
    assert_eq!(
        normalize_case_dates(older),
        "stored case=<date>_123e7decc4c4a423 added=yes"
    );
    let unrelated = "date=2026-10-22_123e7decc4c4a423";
    assert_eq!(normalize_case_dates(unrelated), unrelated);
}

#[test]
#[ignore]
#[cfg(feature = "memory")]
fn update_baselines() {
    let mut baselines = BTreeMap::new();
    for (name, args, input) in cases() {
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let (out, err, code) = run_scx(&argv, &input);
        baselines.insert(name.clone(), (out, err, code));
        println!("基线已写入: {name}");
    }
    let entries = baselines
        .iter()
        .map(|(name, (out, err, code))| {
            let mut fields = vec![("stdout".to_string(), scx::json::Json::Str(out.clone()))];
            if !err.is_empty() {
                fields.push(("stderr".to_string(), scx::json::Json::Str(err.clone())));
            }
            if *code != 0 {
                fields.push(("code".to_string(), scx::json::Json::Num(*code as f64)));
            }
            (name.clone(), scx::json::Json::Obj(fields))
        })
        .collect();
    let mut assets = scx::json::parse(ASSETS).expect("test assets JSON 非法");
    let scx::json::Json::Obj(root) = &mut assets else {
        panic!("test assets 顶层必须是对象");
    };
    let golden = root
        .iter_mut()
        .find(|(key, _)| key == "golden")
        .map(|(_, value)| value)
        .expect("golden");
    let scx::json::Json::Obj(entries_obj) = golden else {
        panic!("golden 必须是对象");
    };
    let key = baseline_key();
    match entries_obj.iter_mut().find(|(name, _)| name == key) {
        Some((_, value)) => *value = scx::json::Json::Obj(entries),
        None => entries_obj.push((key.to_string(), scx::json::Json::Obj(entries))),
    }
    std::fs::write(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("config/tests/assets.json"),
        render_assets(&assets),
    )
    .unwrap();
}

#[cfg(feature = "memory")]
fn render_assets(value: &scx::json::Json) -> String {
    fn render(value: &scx::json::Json, depth: usize, out: &mut String) {
        let pad = "  ".repeat(depth);
        let next = "  ".repeat(depth + 1);
        match value {
            scx::json::Json::Null => out.push_str("null"),
            scx::json::Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            scx::json::Json::Num(value) if value.fract() == 0.0 => {
                out.push_str(&format!("{}", *value as i64));
            }
            scx::json::Json::Num(value) => out.push_str(&value.to_string()),
            scx::json::Json::Str(value) => out.push_str(&scx::json::string(value)),
            scx::json::Json::Arr(items) => {
                out.push_str("[\n");
                for (index, item) in items.iter().enumerate() {
                    out.push_str(&next);
                    render(item, depth + 1, out);
                    out.push_str(if index + 1 == items.len() {
                        "\n"
                    } else {
                        ",\n"
                    });
                }
                out.push_str(&pad);
                out.push(']');
            }
            scx::json::Json::Obj(entries) => {
                out.push_str("{\n");
                for (index, (key, item)) in entries.iter().enumerate() {
                    out.push_str(&next);
                    out.push_str(&scx::json::string(key));
                    out.push_str(": ");
                    render(item, depth + 1, out);
                    out.push_str(if index + 1 == entries.len() {
                        "\n"
                    } else {
                        ",\n"
                    });
                }
                out.push_str(&pad);
                out.push('}');
            }
        }
    }
    let mut out = String::new();
    render(value, 0, &mut out);
    out.push('\n');
    out
}
