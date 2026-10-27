//! golden 差分门：与冻结基线逐字节对比（stdout / stderr / 退出码）。
//!
//! 流程：
//! 1) 首次或有意变更语义时重冻基线：
//!    `cargo test --test golden update_baselines -- --ignored --nocapture`，
//!    生成 tests/golden/expected/<case>.{out,err,code}。
//! 2) 常规回归（CI 执行）：`cargo test --test golden`。
//!
//! 语义约束：本测试只对比当前实现与冻结基线的字节一致性，不判断对错；
//! 基线合理性由人工审查 + 库/CLI 单元测试共同保证。`case=` 后的
//! `YYYY-MM-DD_` 前缀是摄取当天的元数据，不属于输出语义，比较前统一归一化。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn expected_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/expected")
}

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

fn cases() -> Vec<(String, Vec<&'static str>, String)> {
    let lf = char::from(10u8).to_string();
    let mut v: Vec<(String, Vec<&str>, String)> = Vec::new();

    // 记忆用例的库目录：固定、隔离、每次运行前重置。
    let store_dir = std::env::temp_dir().join("scx-golden-mem-store");
    let recall_dir = std::env::temp_dir().join("scx-golden-mem-recall");
    let _ = std::fs::remove_dir_all(&store_dir);
    let _ = std::fs::remove_dir_all(&recall_dir);
    let store = store_dir.to_string_lossy().into_owned();
    let recall = recall_dir.to_string_lossy().into_owned();

    // 1) 小输入自动 passthrough
    let small: String = (0..10)
        .map(|i| format!("line {}", i))
        .collect::<Vec<_>>()
        .join(&lf);
    v.push(("small_passthrough".into(), vec!["--query", "line"], small));

    // 2) 超长行 passthrough
    let mut long_lines = vec!["x".repeat(150_000)];
    long_lines.extend((0..60).map(|i| format!("filler error kvwrite line {}", i)));
    v.push((
        "longline_passthrough".into(),
        vec!["--query", "kvwrite"],
        long_lines.join(&lf),
    ));

    // 3) 零信号 fail-open（原样保留）
    let nohit: String = (0..200)
        .map(|i| format!("line {} with padding text alpha beta gamma", i))
        .collect::<Vec<_>>()
        .join(&lf);
    v.push((
        "zero_signal_failopen".into(),
        vec!["--query", "绝对不存在的词zz"],
        nohit,
    ));

    // 4) 预算选不出任何行 → passthrough:no-kept
    let fat_line = format!("alpha {}", "x".repeat(5000));
    let no_kept: String = (0..40)
        .map(|_| fat_line.clone())
        .collect::<Vec<_>>()
        .join(&lf);
    v.push((
        "no_kept_fallback".into(),
        vec!["--query", "alpha", "--ratio", "0.02"],
        no_kept,
    ));

    // 5) 默认档 sieve（英文日志 + 错误守卫）
    let mut eng: Vec<String> = (0..200)
        .map(|i| format!("INFO heartbeat seq={} ok pool=3 latency=1ms", i))
        .collect();
    eng[150] = "ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a)".to_string();
    eng[151] = "at com.ingest.Pool.acquire(Pool.java:214)".to_string();
    eng[152] = "Root cause: connection pool size 4 < peak concurrency 11".to_string();
    v.push((
        "sieve_default".into(),
        vec!["--query", "what failed db timeout pool conn_id"],
        eng.join(&lf),
    ));

    // 6) 针尖档 CJK 错误守卫
    let mut cjk: Vec<String> = (0..300)
        .map(|i| format!("INFO 心跳 seq={} 状态=正常 latency=1ms", i))
        .collect();
    cjk[150] = "错误：数据库连接超时 (conn_id=9f2a)".to_string();
    cjk[151] = "错误：磁盘已满 errno=28".to_string();
    cjk[152] = "警告：队列深度=40".to_string();
    v.push((
        "sieve_needle_cjk".into(),
        vec!["--query", "数据库 超时 conn_id", "--context", "0"],
        cjk.join(&lf),
    ));

    // 7) ANSI 彩色日志剥离（分析缓冲）
    let mut ansi: Vec<String> = (0..200)
        .map(|i| {
            format!(
                "\u{1b}[32mINFO heartbeat {} ok\u{1b}[0m filler padding text",
                i
            )
        })
        .collect();
    ansi[150] = "\u{1b}[31mERROR db timeout pool exhausted (conn_id=9f2a)\u{1b}[0m".to_string();
    ansi[151] = "\u{1b}[33mRoot cause: pool size 4 < peak concurrency 11\u{1b}[0m".to_string();
    v.push((
        "ansi_strip".into(),
        vec!["--query", "timeout conn_id", "--context", "0"],
        ansi.join(&lf),
    ));

    // 8) CRLF 输入 sieve（输出 LF 归一化）
    let mut crlf: Vec<String> = (0..200)
        .map(|i| format!("INFO line {} heartbeat ok filler padding", i))
        .collect();
    crlf[100] = "ERROR db timeout pool exhausted conn_id=9f2a".to_string();
    v.push((
        "crlf_sieve".into(),
        vec!["--query", "timeout conn_id"],
        crlf.join("\r\n"),
    ));

    // 9) 统一 diff 正文识别
    let diff: Vec<String> = vec![
        "diff --git a/src/lib.rs b/src/lib.rs".to_string(),
        "index 1389ef2..f5c5c61 100644".to_string(),
        "--- a/src/lib.rs".to_string(),
        "+++ b/src/lib.rs".to_string(),
        "@@ -1,6 +1,9 @@".to_string(),
        " //! scx 引擎".to_string(),
        "+use std::collections::HashMap;".to_string(),
        "+const DEFAULT_RATIO: f64 = 0.35;".to_string(),
        "-pub const VERSION: &str = \"0.3.9\";".to_string(),
        "+pub const VERSION: &str = env!(\"CARGO_PKG_VERSION\");".to_string(),
        " fn is_cjk(c: char) -> bool {".to_string(),
        "+    let u = c as u32;".to_string(),
        " filler line".to_string(),
        " filler line".to_string(),
        " filler line".to_string(),
    ];
    v.push((
        "unified_diff".into(),
        vec!["--query", "DEFAULT_RATIO VERSION"],
        diff.join(&lf),
    ));

    // 10) --force 重复命中行全部保留
    let mut dupf: Vec<String> = (0..200)
        .map(|i| format!("INFO filler {} alpha beta", i))
        .collect();
    for idx in [150usize, 151, 152, 153] {
        dupf[idx] = "SPECIAL_FORCE_LINE".to_string();
    }
    v.push((
        "force_duplicates".into(),
        vec![
            "--force",
            "SPECIAL_FORCE_LINE",
            "--context",
            "0",
            "--ratio",
            "0.02",
        ],
        dupf.join(&lf),
    ));

    // 11) --json（正文+统计走 stdout，scxjson 走 stderr）
    v.push((
        "json_stats".into(),
        vec!["--query", "timeout conn_id", "--json"],
        eng.join(&lf),
    ));

    // 12) --stat 只输出统计行
    v.push((
        "stat_only".into(),
        vec!["--query", "timeout conn_id", "--stat"],
        eng.join(&lf),
    ));

    // 13) --no-stats 只输出正文
    v.push((
        "no_stats_body".into(),
        vec!["--query", "timeout conn_id", "--no-stats"],
        eng.join(&lf),
    ));

    // 14) --lineno 行号前缀（针尖档）
    v.push((
        "lineno_mode".into(),
        vec![
            "--query",
            "timeout conn_id",
            "--context",
            "0",
            "--lineno",
            "1",
            "--no-stats",
        ],
        eng.join(&lf),
    ));

    // 15) --elide 0（不输出省略标记）
    v.push((
        "elide_off".into(),
        vec!["--query", "timeout conn_id", "--elide", "0"],
        eng.join(&lf),
    ));

    // 16) --window 1 针尖档
    v.push((
        "window_needle".into(),
        vec![
            "--query",
            "timeout conn_id",
            "--window",
            "1",
            "--context",
            "0",
        ],
        eng.join(&lf),
    ));

    // 17) 记忆摄取旁路（--store 1：正文/统计不变，库内新增 case）
    let seed_log = "INFO start service\nERROR db timeout conn_id=9f2a pool exhausted\nat com.ingest.Pool.acquire(Pool.java:214)\nINFO end\n";
    v.push((
        "memory_store".into(),
        vec![
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(store.clone().into_boxed_str()),
        ],
        seed_log.to_string(),
    ));

    // 18) 记忆召回旁路（--recall-only：只输出 mem 段；先预热同 query 库）
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(recall.clone().into_boxed_str()),
        ],
        seed_log,
    );
    v.push((
        "memory_recall".into(),
        vec![
            "--query",
            "db timeout conn_id",
            "--recall-only",
            "--recall",
            "2",
            "--mem-dir",
            Box::leak(recall.into_boxed_str()),
        ],
        String::new(),
    ));

    // 19) 反馈标注旁路（--rate good <case-id>：状态机 + 质量分，不影响正文）
    let rate_dir = std::env::temp_dir().join("scx-golden-mem-rate");
    let _ = std::fs::remove_dir_all(&rate_dir);
    let rate = rate_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(rate.clone().into_boxed_str()),
        ],
        seed_log,
    );
    let rate_id = {
        let cases = Path::new(&rate).join("cases");
        let id = std::fs::read_dir(&cases)
            .unwrap()
            .find_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .unwrap_or_default();
        id
    };
    v.push((
        "memory_rate".into(),
        vec![
            "--rate",
            "good",
            Box::leak(rate_id.into_boxed_str()),
            "--mem-dir",
            Box::leak(rate.into_boxed_str()),
        ],
        String::new(),
    ));

    // 20) 索引重建旁路（--rebuild：postings 全量重建 + qmap 合并 + index_force=1）
    let rebuild_dir = std::env::temp_dir().join("scx-golden-mem-rebuild");
    let _ = std::fs::remove_dir_all(&rebuild_dir);
    let rebuild = rebuild_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(rebuild.clone().into_boxed_str()),
        ],
        seed_log,
    );
    v.push((
        "memory_rebuild".into(),
        vec![
            "--rebuild",
            "--mem-dir",
            Box::leak(rebuild.into_boxed_str()),
        ],
        String::new(),
    ));

    // 21) 过期清理旁路（--gc：无过期 case → removed=0 报告行）
    let gc_dir = std::env::temp_dir().join("scx-golden-mem-gc");
    let _ = std::fs::remove_dir_all(&gc_dir);
    let gc = gc_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(gc.clone().into_boxed_str()),
        ],
        seed_log,
    );
    v.push((
        "memory_gc".into(),
        vec!["--gc", "--mem-dir", Box::leak(gc.into_boxed_str())],
        String::new(),
    ));

    // 22) 坏段自愈：store + --rebuild 建索引 → 截断 postings.bin 尾部 →
    //     普通召回自动重建；随后 recall-only 输出确定且不写库。
    let corrupt_dir = std::env::temp_dir().join("scx-golden-mem-corrupt");
    let _ = std::fs::remove_dir_all(&corrupt_dir);
    let corrupt = corrupt_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(corrupt.clone().into_boxed_str()),
        ],
        seed_log,
    );
    let _ = run_scx(
        &[
            "--rebuild",
            "--mem-dir",
            Box::leak(corrupt.clone().into_boxed_str()),
        ],
        "",
    );
    {
        let p = Path::new(&corrupt).join("index").join("postings.bin");
        let b = std::fs::read(&p).unwrap();
        std::fs::write(&p, &b[..b.len() - 5]).unwrap(); // 截断 crc+body 尾部 → 校验失败
    }
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--recall",
            "1",
            "--mem-dir",
            Box::leak(corrupt.clone().into_boxed_str()),
        ],
        "",
    );
    v.push((
        "memory_corrupt_heal".into(),
        vec![
            "--query",
            "db timeout conn_id",
            "--recall-only",
            "--mem-dir",
            Box::leak(corrupt.into_boxed_str()),
        ],
        String::new(),
    ));

    // 23) qmap 记忆图重放：store + 一次普通 recall 预热（qmap 回流落盘）→
    //     二次同 query recall-only 走 qmap 直达，输出逐字节稳定。
    let qmap_dir = std::env::temp_dir().join("scx-golden-mem-qmap");
    let _ = std::fs::remove_dir_all(&qmap_dir);
    let qmap = qmap_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(qmap.clone().into_boxed_str()),
        ],
        seed_log,
    );
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--recall",
            "1",
            "--mem-dir",
            Box::leak(qmap.clone().into_boxed_str()),
        ],
        "",
    );
    v.push((
        "memory_qmap_replay".into(),
        vec![
            "--query",
            "db timeout conn_id",
            "--recall-only",
            "--mem-dir",
            Box::leak(qmap.into_boxed_str()),
        ],
        String::new(),
    ));

    // 24) 评估系统旁路（--eval：A 完整管线 vs B 朴素 BM25，分层报告行确定性）。
    //     baseline.json 的 expected 用 hash8 后缀（免日期漂移）。
    let eval_dir = std::env::temp_dir().join("scx-golden-mem-eval");
    let _ = std::fs::remove_dir_all(&eval_dir);
    let eval = eval_dir.to_string_lossy().into_owned();
    let ev_seeds: [(&str, &str); 3] = [
        ("db timeout conn_id", seed_log),
        (
            "disk full errno",
            "ERROR disk full errno=28\nat fs.write(Fs.java:88)\nINFO end\n",
        ),
        (
            "queue depth",
            "WARN queue depth=40 backlog\nat Queue.push(Queue.java:9)\nINFO end\n",
        ),
    ];
    for (q, t) in &ev_seeds {
        let _ = run_scx(
            &[
                "--query",
                q,
                "--store",
                "1",
                "--mem-dir",
                Box::leak(eval.clone().into_boxed_str()),
            ],
            t,
        );
    }
    let ev_id = |q: &str| -> String {
        let cases = Path::new(&eval).join("cases");
        let mut out = String::new();
        for e in std::fs::read_dir(&cases).unwrap().filter_map(|e| e.ok()) {
            let id = e.file_name().to_string_lossy().into_owned();
            let cj = std::fs::read_to_string(e.path().join("case.json")).unwrap_or_default();
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
    let baseline_path = {
        let p = std::env::temp_dir().join("scx-golden-eval-baseline.json");
        let js = format!(
            "{{\"cases\":[{{\"query\":\"db timeout conn_id\",\"layer\":\"literal\",\"expected\":[\"{h0}\"],\"rel\":{{\"{h0}\":2}}}},{{\"query\":\"磁盘连接超时 conn_id 池\",\"layer\":\"paraphrase\",\"expected\":[\"{h0}\"]}},{{\"query\":\"存储空间不足 errno 磁盘\",\"layer\":\"semantic\",\"expected\":[\"{h1}\"]}}]}}"
        );
        std::fs::write(&p, js).unwrap();
        p.to_string_lossy().into_owned()
    };
    v.push((
        "memory_eval".into(),
        vec![
            "--eval",
            Box::leak(baseline_path.clone().into_boxed_str()),
            "--mem-dir",
            Box::leak(eval.clone().into_boxed_str()),
        ],
        String::new(),
    ));

    // 25) --eval-verbose 逐 case 诊断：A/B top-10 与 [hit]/[miss]。
    //     复用 memory_eval 的库与基线，默认 --eval 输出不变。
    v.push((
        "memory_eval_verbose".into(),
        vec![
            "--eval",
            Box::leak(baseline_path.into_boxed_str()),
            "--eval-verbose",
            "--mem-dir",
            Box::leak(eval.into_boxed_str()),
        ],
        String::new(),
    ));

    // 26) L1 要点摘要旁路（--recall-detail：mem 段追加 summary 行，字段+证据首行）。
    let summary_dir = std::env::temp_dir().join("scx-golden-mem-summary");
    let _ = std::fs::remove_dir_all(&summary_dir);
    let summary = summary_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(summary.clone().into_boxed_str()),
        ],
        seed_log,
    );
    v.push((
        "memory_summary".into(),
        vec![
            "--query",
            "db timeout conn_id",
            "--recall-only",
            "--recall",
            "1",
            "--recall-detail",
            "--mem-dir",
            Box::leak(summary.into_boxed_str()),
        ],
        String::new(),
    ));

    // 27) L1 本地直答：--store 配 --fix 保存修复结论，recall-detail 输出 fix 行。
    let fix_dir = std::env::temp_dir().join("scx-golden-mem-fix");
    let _ = std::fs::remove_dir_all(&fix_dir);
    let fix = fix_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--fix",
            "enlarge pool to 64 conns, restart daemon",
            "--mem-dir",
            Box::leak(fix.clone().into_boxed_str()),
        ],
        seed_log,
    );
    v.push((
        "memory_fix".into(),
        vec![
            "--query",
            "db timeout conn_id",
            "--recall-only",
            "--recall",
            "1",
            "--recall-detail",
            "--mem-dir",
            Box::leak(fix.into_boxed_str()),
        ],
        String::new(),
    ));

    // 28) 库盘点：--stats 输出 case/桶/状态/fix 覆盖率/质量/磁盘。
    // 两个 case 用隔离日志：db 场景（seed_log）+ 磁盘场景（专用日志），
    // 让基线展示多桶（failopen 全文保留会把无关家族词带进 text，同库会互相污染桶归属）。
    let stats_dir = std::env::temp_dir().join("scx-golden-mem-stats");
    let _ = std::fs::remove_dir_all(&stats_dir);
    let stats = stats_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--fix",
            "enlarge pool to 64 conns",
            "--mem-dir",
            Box::leak(stats.clone().into_boxed_str()),
        ],
        seed_log,
    );
    let seed_disk = "INFO start\nWARN disk usage 97% mount /var\nERROR errno=28 no space left\n";
    let _ = run_scx(
        &[
            "--query",
            "磁盘空间不足",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(stats.clone().into_boxed_str()),
        ],
        seed_disk,
    );
    v.push((
        "memory_stats".into(),
        vec!["--stats", "--mem-dir", Box::leak(stats.into_boxed_str())],
        String::new(),
    ));

    // 29) 智能标注器：--annotate 三层标注与自检证据，仅用于回归集。
    let annotate_dir = std::env::temp_dir().join("scx-golden-mem-annotate");
    let _ = std::fs::remove_dir_all(&annotate_dir);
    let annotate = annotate_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--fix",
            "enlarge pool to 64 conns",
            "--mem-dir",
            Box::leak(annotate.clone().into_boxed_str()),
        ],
        seed_log,
    );
    let _ = run_scx(
        &[
            "--query",
            "磁盘空间不足 errno",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(annotate.clone().into_boxed_str()),
        ],
        seed_disk,
    );
    v.push((
        "memory_annotate".into(),
        vec![
            "--annotate",
            "--seed",
            "0",
            "--mem-dir",
            Box::leak(annotate.into_boxed_str()),
        ],
        String::new(),
    ));

    // 30) --max-tokens：证据地板（证据 + 最小邻域）超限 → overflow:evidence，
    //     证据行仍全保留，统计行追加 limit/tokens 记账字段。
    let mut mt1: Vec<String> = (0..200)
        .map(|i| format!("INFO heartbeat seq={} ok pool=3 latency=1ms", i))
        .collect();
    mt1[100] = "conn 9f2a broken db crash".to_string();
    v.push((
        "max_tokens_overflow_evidence".into(),
        vec!["--query", "alpha conn broken", "--max-tokens", "5"],
        mt1.join(&lf),
    ));

    // 31) --max-tokens：风险守卫追加证据性错误行后总估算超限 → overflow:fill。
    let mut mt2: Vec<String> = (0..200)
        .map(|i| format!("pad alpha line {} beta gamma", i))
        .collect();
    mt2[5] = "conn 9f2a broken".to_string();
    mt2[6] = "error alpha pipe burst 7a1c now".to_string();
    v.push((
        "max_tokens_overflow_fill".into(),
        vec![
            "--query",
            "alpha conn broken",
            "--max-tokens",
            "5",
            "--context",
            "0",
        ],
        mt2.join(&lf),
    ));

    // 32) --max-tokens：上限充足 → 正常 sieve，统计行与默认格式逐字节一致。
    v.push((
        "max_tokens_sieve_capped".into(),
        vec!["--query", "alpha conn broken", "--max-tokens", "1000"],
        mt1.join(&lf),
    ));

    // 36) --json：删除区间逐段记账（dropped 字段，1-based 闭区间 + reason；
    //     旧字段顺序不变，向后兼容）。
    v.push((
        "json_dropped".into(),
        vec!["--query", "alpha conn broken", "--json"],
        mt1.join(&lf),
    ));

    // 37) --json + --max-tokens：容量超限删除记 reason=max-tokens。
    v.push((
        "json_dropped_overflow".into(),
        vec![
            "--query",
            "alpha conn broken",
            "--json",
            "--max-tokens",
            "5",
        ],
        mt1.join(&lf),
    ));

    // 33) --structure 1：markdown fence 配对块整段并入种子区域（块内上下文随预算填充）。
    let mut sf: Vec<String> = vec!["```".to_string()];
    sf.push("def run():".to_string());
    sf.push("    conn 9f2a broken db crash".to_string());
    sf.push("    return pool".to_string());
    sf.push("```".to_string());
    sf.extend((0..150).map(|i| format!("INFO heartbeat seq={} ok pool=3 latency=1ms", i)));
    v.push((
        "structure_fence".into(),
        vec!["--query", "alpha conn broken", "--structure", "1"],
        sf.join(&lf),
    ));

    // 34) --structure 1：连续 import 块整段并入（块内含证据行，import 前缀连续）。
    let mut si: Vec<String> = vec![
        "import os".to_string(),
        "import sys".to_string(),
        "import conn_pool".to_string(),
        "import threading".to_string(),
    ];
    si.extend((0..150).map(|i| format!("INFO heartbeat seq={} ok pool=3 latency=1ms", i)));
    v.push((
        "structure_import".into(),
        vec!["--query", "alpha conn", "--structure", "1"],
        si.join(&lf),
    ));

    // 35) --structure 1：堆栈 at 链 + Caused by: 延续整段并入
    //     （query 用唯一稀有词 sqlexception 触发证据种子，演示区域并入）。
    let mut ss: Vec<String> = vec![
        "  at com.a.Pool.acquire(Pool.java:214)".to_string(),
        "  at com.b.Runner.main(Runner.java:88)".to_string(),
        "Caused by: java.sql.SQLException: db crash conn 9f2a".to_string(),
        "  at com.c.Conn.open(Conn.java:9)".to_string(),
    ];
    ss.extend((0..150).map(|i| format!("INFO heartbeat seq={} ok pool=3 latency=1ms", i)));
    v.push((
        "structure_stack".into(),
        vec!["--query", "alpha conn sqlexception", "--structure", "1"],
        ss.join(&lf),
    ));

    v
}

#[test]
fn golden_gate() {
    for (name, args, input) in cases() {
        let (out, err, code) = run_scx(&args, &input);
        let dir = expected_dir();
        let exp_out = std::fs::read_to_string(dir.join(format!("{name}.out"))).unwrap_or_else(|_| {
            panic!("缺失基线 {name}.out —— 先运行: cargo test --test golden update_baselines -- --ignored")
        });
        let exp_err = std::fs::read_to_string(dir.join(format!("{name}.err"))).unwrap_or_else(|_| {
            panic!("缺失基线 {name}.err —— 先运行: cargo test --test golden update_baselines -- --ignored")
        });
        let exp_code = std::fs::read_to_string(dir.join(format!("{name}.code")))
            .unwrap_or_else(|_| "0".to_string())
            .trim()
            .parse::<i32>()
            .unwrap();
        assert_eq!(code, exp_code, "[{name}] 退出码漂移");
        assert_eq!(
            normalize_case_dates(&err),
            normalize_case_dates(&exp_err),
            "[{name}] stderr 漂移"
        );
        assert_eq!(
            normalize_case_dates(&out),
            normalize_case_dates(&exp_out),
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
fn update_baselines() {
    let dir = expected_dir();
    std::fs::create_dir_all(&dir).unwrap();
    for (name, args, input) in cases() {
        let (out, err, code) = run_scx(&args, &input);
        std::fs::write(dir.join(format!("{name}.out")), out.as_bytes()).unwrap();
        std::fs::write(dir.join(format!("{name}.err")), err.as_bytes()).unwrap();
        std::fs::write(
            dir.join(format!("{name}.code")),
            code.to_string().as_bytes(),
        )
        .unwrap();
        println!("基线已写入: {name}");
    }
}
