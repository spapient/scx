//! 零依赖性能基准入口。
//!
//! ```text
//! cargo run --release --locked --example bench            # 50k 行压缩
//! cargo run --release --locked --example bench -- memory  # 10k case 记忆库
//! ```
//!
//! 压缩基准报告 9 次采样的中位耗时（微秒）。记忆库基准构造短日志与长堆栈混合的
//! 1 万条 case，分别报告线性冷启动、索引首次加载、稳态倒排召回与全候选宽查询。

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

    let _ = compress(&text, "timeout conn_id pool", &cfg);
    let mut samples: Vec<u128> = Vec::new();
    for _ in 0..9 {
        let t = Instant::now();
        let outcome = compress(&text, "timeout conn_id pool", &cfg);
        samples.push(t.elapsed().as_micros());
        assert!(outcome.seeds > 0, "benchmark corpus must hit a seed");
    }
    samples.sort_unstable();
    println!(
        "compress: 50k lines ({} chars) median {} us | samples {:?} us | kept={}",
        text.chars().count(),
        samples[samples.len() / 2],
        samples,
        compress(&text, "timeout conn_id pool", &cfg).kept,
    );
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

    let mut recall = Vec::new();
    for i in 0..21 {
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
