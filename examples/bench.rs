//! 零依赖性能基准入口。
//!
//! ```text
//! cargo run --release --locked --example bench            # 压缩全路径（default/structure/max-tokens/policy）
//! cargo run --release --locked --example bench -- memory  # 10k case 记忆库
//! ```
//!
//! 压缩基准报告 9 次采样的中位耗时（微秒），含四条路径：默认配置、--structure 1
//! 结构感知邻域、--max-tokens 硬上限、--policy-file 字段覆盖（conf 解析为一次性
//! 开销，稳态压缩延迟与字段覆盖等价——完整 CLI 映射在 args，此处为字段级近似）。
//! 记忆库基准构造短日志与长堆栈混合的 1 万条 case，分别报告线性冷启动、索引
//! 首次加载、稳态倒排召回与全候选宽查询。

use scx::memory::CaseStats;
use scx::memory::{rebuild, recall_cases, store_case, StoreInput};
use scx::{compress, Config};
use std::path::Path;
use std::time::Instant;

fn main() {
    match std::env::args().nth(1).as_deref() {
        None | Some("compress") => compress_bench(),
        Some("memory") => memory_bench(),
        Some(other) => {
            eprintln!("bench: unknown mode {other:?}; expected compress or memory");
            std::process::exit(2);
        }
    }
}

fn compress_bench() {
    let lines: Vec<String> = (0..50_000)
        .map(|i| {
            if i % 100 == 0 {
                format!("ERROR db timeout conn_id={} pool exhausted", i)
            } else {
                format!("INFO heartbeat seq={} ok pool=3 latency=1ms", i)
            }
        })
        .collect();
    let text = lines.join("\n");
    let cfg = Config::default();
    let query = "timeout conn_id pool";
    let repeats = bench_repeats();

    bench_median(&text, query, &cfg, "compress", repeats);
    structure_bench(repeats);
    max_tokens_bench(&text, query, repeats);
    policy_bench(&text, query, repeats);
}

/// 单路径中位耗时：预热一次后采样，报告 p50 与全样本。
fn bench_median(text: &str, query: &str, cfg: &Config, label: &str, repeats: usize) {
    let _ = compress(text, query, cfg);
    let mut samples: Vec<u128> = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        let t = Instant::now();
        let outcome = compress(text, query, cfg);
        samples.push(t.elapsed().as_micros());
        assert!(outcome.seeds > 0, "benchmark corpus must hit a seed");
    }
    samples.sort_unstable();
    println!(
        "{label}: 50k lines ({} chars) median {} us | samples {:?} us | kept={}",
        text.chars().count(),
        samples[samples.len() / 2],
        samples,
        compress(text, query, cfg).kept,
    );
}

/// 结构感知路径：结构富语料（import 块 / stack at 链 / fence 块 + 噪声）在
/// `structure=false` 与 `true` 下分别计时，报告开启的增量开销。
fn structure_bench(repeats: usize) {
    let text = structure_text();
    let query = "compute module";
    let mut off = Config::default();
    off.structure = false;
    let mut on = Config::default();
    on.structure = true;
    bench_median(&text, query, &off, "structure:off", repeats);
    bench_median(&text, query, &on, "structure:on", repeats);
}

/// --max-tokens 硬上限路径：默认语料 + `max_tokens=2000`，报告模式（应为
/// overflow 或 sieve，取决于证据地板与预算交互）。
fn max_tokens_bench(text: &str, query: &str, repeats: usize) {
    let mut cfg = Config::default();
    cfg.max_tokens = Some(2000);
    let _ = compress(text, query, &cfg);
    let mut samples: Vec<u128> = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        let t = Instant::now();
        let outcome = compress(text, query, &cfg);
        samples.push(t.elapsed().as_micros());
        assert!(outcome.seeds > 0);
    }
    samples.sort_unstable();
    let out = compress(text, query, &cfg);
    println!(
        "max-tokens: 50k lines median {} us | samples {:?} us | kept={} mode={}",
        samples[samples.len() / 2],
        samples,
        out.kept,
        out.mode,
    );
}

/// --policy-file 路径：conf::parse 为一次性开销（与 CLI 层等价），单独计时；
/// 字段覆盖后的稳态压缩延迟单独报告。完整 CLI 映射在 args::parse_policy_file，
/// 此处为字段级近似（ratio/context/max_tokens/structure 代表字段）。
fn policy_bench(text: &str, query: &str, repeats: usize) {
    let body = "ratio = 0.5\ncontext = 1\nmax_tokens = 4000\nstructure = 1\n";
    let entries = scx::conf::parse(body.lines());
    let mut cfg = Config::default();
    for e in &entries {
        match e.key {
            "ratio" => cfg.ratio = scx::conf::parse_f64(&e.value).unwrap(),
            "context" => cfg.context = scx::conf::parse_usize(&e.value).unwrap() as i64,
            "max_tokens" => cfg.max_tokens = scx::conf::parse_usize(&e.value),
            "structure" => cfg.structure = scx::conf::parse_usize(&e.value).unwrap() > 0,
            _ => {}
        }
    }
    let t = Instant::now();
    let _ = compress(text, query, &cfg);
    let parse_us = t.elapsed().as_micros();
    let mut samples: Vec<u128> = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        let t = Instant::now();
        let outcome = compress(text, query, &cfg);
        samples.push(t.elapsed().as_micros());
        assert!(outcome.seeds > 0);
    }
    samples.sort_unstable();
    let out = compress(text, query, &cfg);
    println!(
        "policy: conf parse {} us (one-shot, per-invocation) | compress median {} us | samples {:?} us | kept={} mode={}",
        parse_us,
        samples[samples.len() / 2],
        samples,
        out.kept,
        out.mode,
    );
}

/// 结构富语料：每 50 行一个 import 块（10 行）、stack at 链（10 行）、fence
/// 块（3 行）与噪声行交替；保证 structure 路径有可识别的结构区间。
fn structure_text() -> String {
    let mut v: Vec<String> = Vec::with_capacity(50_000);
    for i in 0..50_000 {
        match i % 50 {
            0..=9 => v.push(format!("import com.svc.module{};", i % 10)),
            10..=19 => v.push(format!(
                "  at com.svc.Class{}.method{}() line {}",
                i % 10,
                i % 20,
                100 + i
            )),
            20 => v.push("```rust".to_string()),
            21 => v.push(format!("let x = compute({});", i)),
            22 => v.push("```".to_string()),
            _ => v.push(format!("INFO heartbeat seq={} ok", i)),
        }
    }
    v.join("\n")
}

fn bench_repeats() -> usize {
    std::env::var("SCX_BENCH_REPEAT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9)
}

fn memory_bench() {
    let root = std::env::temp_dir().join(format!("scx-mem-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let root = root.to_str().unwrap().to_string();
    let stats = CaseStats {
        kept: 5,
        total: 100,
        cut_pct: 95.0,
        risk: "HIGH".to_string(),
    };

    let t = Instant::now();
    for seed in 0..200 {
        let query = format!("issue fam{}", letters(seed));
        let text = bench_case(seed);
        store_case(StoreInput::new(&root, &query, &text, &stats)).expect("store_case failed");
    }
    println!("store: 200 seeds in {} ms", t.elapsed().as_millis());

    let t = Instant::now();
    let cases_dir = Path::new(&root).join("cases");
    let base: Vec<String> = std::fs::read_dir(&cases_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    for copy in 1..50 {
        for id in &base {
            let new_id = format!("{id}_{copy:02}");
            let src = cases_dir.join(id);
            let dst = cases_dir.join(&new_id);
            std::fs::create_dir(&dst).unwrap();
            let path = dst.join("case.json");
            std::fs::copy(src.join("case.json"), &path).unwrap();
            let body = std::fs::read_to_string(&path).unwrap();
            std::fs::write(
                &path,
                body.replace(&format!("\"id\":\"{id}\""), &format!("\"id\":\"{new_id}\"")),
            )
            .unwrap();
        }
    }
    println!(
        "replicate: 200 -> 10000 cases in {} ms",
        t.elapsed().as_millis()
    );

    let mut cold = Vec::new();
    for i in 0..5 {
        let query = bench_query(i);
        let t = Instant::now();
        let hits = recall_cases(&root, &query, 5, true).expect("cold recall failed");
        cold.push(t.elapsed().as_micros());
        assert!(!hits.is_empty(), "benchmark query must recall");
    }
    cold.sort_unstable();
    let (c50, c95, c99) = percentiles(&cold);
    println!("cold (no index, linear): p50={c50} p95={c95} p99={c99} us");

    let t = Instant::now();
    let (n_docs, n_terms) = rebuild(&root).expect("rebuild failed");
    println!(
        "rebuild: docs={n_docs} terms={n_terms} in {} ms",
        t.elapsed().as_millis()
    );

    // 单独报告索引首次加载，后续样本只衡量稳态召回；否则一个 10ms 冷加载样本
    // 会长期占据 p99，把查询分布与索引 I/O 混在一起。
    let t = Instant::now();
    let warm = recall_cases(&root, &bench_query(99), 5, true).expect("index warm failed");
    assert!(!warm.is_empty(), "index warm query must recall");
    println!("index warm: {} us", t.elapsed().as_micros());

    let recall_repeats = std::env::var("SCX_BENCH_RECALL_REPEAT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(21);
    let mut recall = Vec::new();
    for i in 0..recall_repeats {
        let query = bench_query(i);
        let t = Instant::now();
        let hits = recall_cases(&root, &query, 5, true).expect("recall failed");
        recall.push(t.elapsed().as_micros());
        assert!(!hits.is_empty(), "benchmark query must recall");
    }
    recall.sort_unstable();
    let (p50, p95, p99) = percentiles(&recall);
    let top1 = recall_cases(
        &root,
        &format!("fam{} cid{}", letters(42), letters(42)),
        1,
        true,
    )
    .unwrap()[0]
        .sim;
    println!("recall: p50={p50} p95={p95} p99={p99} us | samples={recall:?} us | top1 sim={top1}");
    // 5 ms is 25× the current ~0.2 ms p50: it catches order-of-magnitude
    // regressions without turning small CI host jitter into failures.
    if p50 >= 5_000 {
        println!("gate: FAIL (p50 {p50} us >= 5ms)");
        std::process::exit(2);
    }
    println!("gate: PASS (10k recall p50 < 5ms)");

    let mut broad = Vec::new();
    for i in 0..7 {
        let query = format!("ERROR timeout pool {}", letters(i));
        let t = Instant::now();
        let hits = recall_cases(&root, &query, 5, true).expect("broad recall failed");
        broad.push(t.elapsed().as_micros());
        assert!(!hits.is_empty());
    }
    broad.sort_unstable();
    let (b50, b95, b99) = percentiles(&broad);
    println!("broad recall: p50={b50} p95={b95} p99={b99} us | samples={broad:?} us");

    let _ = std::fs::remove_dir_all(&root);
}

fn bench_case(seed: usize) -> String {
    if seed % 4 == 0 {
        let mut text = format!(
            "FATAL fam{} cid{} stack overflow alpha beta gamma theta",
            letters(seed),
            letters(seed)
        );
        for frame in 0..8 {
            text.push_str(&format!(
                " at com.svc.{}() called from {}() line {}",
                letters((seed * 31 + frame * 7) % 26),
                letters((seed * 13 + frame) % 26),
                100 + (seed * 3 + frame) % 900
            ));
        }
        text
    } else {
        format!(
            "ERROR fam{} cid{} conn timeout pool exhausted retry=3 delta={}",
            letters(seed),
            letters(seed),
            (seed * 7) % 1000
        )
    }
}

fn bench_query(sample: usize) -> String {
    format!(
        "fam{} cid{} tail{}",
        letters(42),
        letters(42),
        letters(sample)
    )
}

/// 数字 → 全字母串（base26，避免 norm 数字折叠导致指纹去重）。
fn letters(mut n: usize) -> String {
    let mut out = String::new();
    loop {
        out.insert(0, (b'a' + (n % 26) as u8) as char);
        n /= 26;
        if n == 0 {
            return out;
        }
    }
}

fn percentiles(sorted: &[u128]) -> (u128, u128, u128) {
    fn pct(values: &[u128], q: usize) -> u128 {
        if values.is_empty() {
            return 0;
        }
        let index = ((values.len() - 1) * q / 100).min(values.len() - 1);
        values[index]
    }
    (pct(sorted, 50), pct(sorted, 95), pct(sorted, 99))
}
