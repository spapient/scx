//! golden 差分门：与冻结基线逐字节对比（stdout / stderr / 退出码）。
//!
//! 流程：
//! 1) 首次或有意变更语义时重冻基线：
//!    `cargo test --test golden update_baselines -- --ignored --nocapture`，
//!    生成 tests/golden/expected/<case>.{out,err,code}。
//! 2) 常规回归（CI 执行）：`cargo test --test golden`。
//!
//! 语义约束：本测试只对比当前实现与冻结基线的字节一致性，不判断对错；
//! 基线合理性由人工审查 + 库/CLI 单元测试共同保证。

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

fn cases() -> Vec<(String, Vec<&'static str>, String)> {
    let lf = char::from(10u8).to_string();
    let mut v: Vec<(String, Vec<&str>, String)> = Vec::new();

    // 记忆用例的库目录：固定、隔离、每次运行前重置。
    let mem_store_dir = std::env::temp_dir().join("scx-golden-mem-store");
    let mem_recall_dir = std::env::temp_dir().join("scx-golden-mem-recall");
    let _ = std::fs::remove_dir_all(&mem_store_dir);
    let _ = std::fs::remove_dir_all(&mem_recall_dir);
    let mem_store = mem_store_dir.to_string_lossy().into_owned();
    let mem_recall = mem_recall_dir.to_string_lossy().into_owned();

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
            Box::leak(mem_store.clone().into_boxed_str()),
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
            Box::leak(mem_recall.clone().into_boxed_str()),
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
            Box::leak(mem_recall.into_boxed_str()),
        ],
        String::new(),
    ));

    // 19) 反馈标注旁路（--rate good <case-id>：状态机 + 质量分，不影响正文）
    let mem_rate_dir = std::env::temp_dir().join("scx-golden-mem-rate");
    let _ = std::fs::remove_dir_all(&mem_rate_dir);
    let mem_rate = mem_rate_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(mem_rate.clone().into_boxed_str()),
        ],
        seed_log,
    );
    let rate_id = {
        let cases = Path::new(&mem_rate).join("cases");
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
            Box::leak(mem_rate.into_boxed_str()),
        ],
        String::new(),
    ));

    // 20) 索引重建旁路（--rebuild：postings 全量重建 + qmap 合并 + index_force=1）
    let mem_rb_dir = std::env::temp_dir().join("scx-golden-mem-rebuild");
    let _ = std::fs::remove_dir_all(&mem_rb_dir);
    let mem_rb = mem_rb_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(mem_rb.clone().into_boxed_str()),
        ],
        seed_log,
    );
    v.push((
        "memory_rebuild".into(),
        vec!["--rebuild", "--mem-dir", Box::leak(mem_rb.into_boxed_str())],
        String::new(),
    ));

    // 21) 过期清理旁路（--gc：无过期 case → removed=0 报告行）
    let mem_gc_dir = std::env::temp_dir().join("scx-golden-mem-gc");
    let _ = std::fs::remove_dir_all(&mem_gc_dir);
    let mem_gc = mem_gc_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(mem_gc.clone().into_boxed_str()),
        ],
        seed_log,
    );
    v.push((
        "memory_gc".into(),
        vec!["--gc", "--mem-dir", Box::leak(mem_gc.into_boxed_str())],
        String::new(),
    ));

    // 22) 坏段自愈旁路：store + --rebuild 建索引 → 截断 postings.bin 尾部 →
    //     recall-only 仍正常（自动重建，不崩溃、输出确定）。
    let mem_cor_dir = std::env::temp_dir().join("scx-golden-mem-corrupt");
    let _ = std::fs::remove_dir_all(&mem_cor_dir);
    let mem_cor = mem_cor_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(mem_cor.clone().into_boxed_str()),
        ],
        seed_log,
    );
    let _ = run_scx(
        &[
            "--rebuild",
            "--mem-dir",
            Box::leak(mem_cor.clone().into_boxed_str()),
        ],
        "",
    );
    {
        let p = Path::new(&mem_cor).join("index").join("postings.bin");
        let b = std::fs::read(&p).unwrap();
        std::fs::write(&p, &b[..b.len() - 5]).unwrap(); // 截断 crc+body 尾部 → 校验失败
    }
    v.push((
        "memory_corrupt_heal".into(),
        vec![
            "--query",
            "db timeout conn_id",
            "--recall-only",
            "--mem-dir",
            Box::leak(mem_cor.into_boxed_str()),
        ],
        String::new(),
    ));

    // 23) qmap 记忆图重放：store + 一次 recall-only 预热（qmap 回流落盘）→
    //     二次同 query recall-only 走 qmap 直达，输出逐字节稳定。
    let mem_qm_dir = std::env::temp_dir().join("scx-golden-mem-qmap");
    let _ = std::fs::remove_dir_all(&mem_qm_dir);
    let mem_qm = mem_qm_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(mem_qm.clone().into_boxed_str()),
        ],
        seed_log,
    );
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--recall-only",
            "--mem-dir",
            Box::leak(mem_qm.clone().into_boxed_str()),
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
            Box::leak(mem_qm.into_boxed_str()),
        ],
        String::new(),
    ));

    // 24) 评估系统旁路（--eval：A 完整管线 vs B 朴素 BM25，分层报告行确定性）。
    //     baseline.json 的 expected 用 hash8 后缀（免日期漂移）。
    let mem_ev_dir = std::env::temp_dir().join("scx-golden-mem-eval");
    let _ = std::fs::remove_dir_all(&mem_ev_dir);
    let mem_ev = mem_ev_dir.to_string_lossy().into_owned();
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
                Box::leak(mem_ev.clone().into_boxed_str()),
            ],
            t,
        );
    }
    let ev_id = |q: &str| -> String {
        let cases = Path::new(&mem_ev).join("cases");
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
            Box::leak(mem_ev.clone().into_boxed_str()),
        ],
        String::new(),
    ));

    // 28) --eval-verbose 逐 case 诊断（P4.4+ 优化④：A/B top-10 + [hit]/[miss]，
    //     服务 M4 人工标注校准；复用 memory_eval 的库与基线，默认 --eval 输出不变）。
    v.push((
        "memory_eval_verbose".into(),
        vec![
            "--eval",
            Box::leak(baseline_path.into_boxed_str()),
            "--eval-verbose",
            "--mem-dir",
            Box::leak(mem_ev.into_boxed_str()),
        ],
        String::new(),
    ));

    // 25) L1 要点摘要旁路（--recall-detail：mem 段追加 summary 行，字段+证据首行）。
    let mem_sm_dir = std::env::temp_dir().join("scx-golden-mem-summary");
    let _ = std::fs::remove_dir_all(&mem_sm_dir);
    let mem_sm = mem_sm_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--mem-dir",
            Box::leak(mem_sm.clone().into_boxed_str()),
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
            Box::leak(mem_sm.into_boxed_str()),
        ],
        String::new(),
    ));

    // 26) L1 本地直答（P4.4+ 优化：--store 配 --fix 保存修复结论，recall-detail 输出 fix 行）。
    let mem_fix_dir = std::env::temp_dir().join("scx-golden-mem-fix");
    let _ = std::fs::remove_dir_all(&mem_fix_dir);
    let mem_fix = mem_fix_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--fix",
            "enlarge pool to 64 conns, restart daemon",
            "--mem-dir",
            Box::leak(mem_fix.clone().into_boxed_str()),
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
            Box::leak(mem_fix.into_boxed_str()),
        ],
        String::new(),
    ));

    // 27) 库盘点（P4.4+ 优化：--stats 输出 case/桶/状态/fix 覆盖率/质量/磁盘）。
    // 两个 case 用隔离日志：db 场景（seed_log）+ 磁盘场景（专用日志），
    // 让基线展示多桶（failopen 全文保留会把无关家族词带进 text，同库会互相污染桶归属）。
    let mem_st_dir = std::env::temp_dir().join("scx-golden-mem-stats");
    let _ = std::fs::remove_dir_all(&mem_st_dir);
    let mem_st = mem_st_dir.to_string_lossy().into_owned();
    let _ = run_scx(
        &[
            "--query",
            "db timeout conn_id",
            "--store",
            "1",
            "--fix",
            "enlarge pool to 64 conns",
            "--mem-dir",
            Box::leak(mem_st.clone().into_boxed_str()),
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
            Box::leak(mem_st.clone().into_boxed_str()),
        ],
        seed_disk,
    );
    v.push((
        "memory_stats".into(),
        vec!["--stats", "--mem-dir", Box::leak(mem_st.into_boxed_str())],
        String::new(),
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
        assert_eq!(err, exp_err, "[{name}] stderr 漂移");
        assert_eq!(out, exp_out, "[{name}] stdout 漂移");
    }
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
