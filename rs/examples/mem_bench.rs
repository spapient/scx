//! 记忆库性能基准（P3 阶段门）：1 万条 case 库，倒排路径召回中位耗时 < 5ms。
//!
//! 运行（与 README 一致的 target-dir 策略）：
//!   Windows:   cargo run --release --example mem_bench --locked --target-dir "%TEMP%\scx-cargo-target"
//!   macOS/Linux: cargo run --release --example mem_bench --locked --target-dir "$TMPDIR/scx-cargo-target"
//!
//! 构造：200 条字母唯一种子（norm 折叠数字会导致指纹全同去重，必须用字母 token）
//! → 复制 50 份改名 → 1 万条；文本长度混合真实分布（短日志行 ≈120B + 长堆栈 ≈2KB）。
//! 输出 store/build/recall 三段耗时 + 冷启动（无索引线性路径）耗时 + p50/p95/p99 + P3 门判据。
//!
//! P4.4+ 优化⑬（P1-5）：真实分布、百分位（不只中位数）、冷启动段（索引未建时的
//! 线性路径首查）——M4 基准数字站得住：p95 反映真实长尾，冷启动反映"开箱即用"。

use scx::memory::CaseStats;
use scx::memory::{rebuild, recall_cases, store_case, StoreInput};
use std::path::Path;
use std::time::Instant;

/// 数字 → 全字母串（base26，避免 norm 数字折叠导致指纹去重）。
fn letters(mut n: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'a' + (n % 26) as u8) as char);
        n /= 26;
        if n == 0 {
            break;
        }
    }
    s
}

/// p50/p95/p99（升序切片；n 小时用最近邻）。
fn percentiles(sorted: &[u128]) -> (u128, u128, u128) {
    fn pct(a: &[u128], q: usize) -> u128 {
        if a.is_empty() {
            return 0;
        }
        let idx = ((a.len() - 1) * q / 100).min(a.len() - 1);
        a[idx]
    }
    (pct(sorted, 50), pct(sorted, 95), pct(sorted, 99))
}

fn main() {
    let root = std::env::temp_dir().join("scx-mem-bench");
    let _ = std::fs::remove_dir_all(&root);
    let root = root.to_str().unwrap().to_string();
    let stats = CaseStats {
        kept: 5,
        total: 100,
        cut_pct: 95.0,
        risk: "HIGH".to_string(),
    };

    // 1) 200 条字母唯一种子（约 1-2s）；文本长度混合：短日志行 ≈120B / 长堆栈 ≈2KB。
    let t = Instant::now();
    for v in 0..200 {
        let q = format!("issue fam{}", letters(v));
        let txt = if v % 4 == 0 {
            // 长堆栈：模拟真实异常栈（多帧 + 业务上下文），随机化各帧令牌防去重。
            let mut s = format!(
                "FATAL fam{} cid{} stack overflow alpha beta gamma theta",
                letters(v),
                letters(v)
            );
            for f in 0..8 {
                s.push_str(&format!(
                    " at com.svc.{}() called from {}() line {}",
                    letters((v * 31 + f * 7) % 26),
                    letters((v * 13 + f) % 26),
                    100 + (v * 3 + f) % 900
                ));
            }
            s
        } else {
            // 短日志行：真实单行错误 + 字段。
            let s = format!(
                "ERROR fam{} cid{} conn timeout pool exhausted retry=3 delta={}",
                letters(v),
                letters(v),
                (v * 7) % 1000
            );
            s
        };
        store_case(StoreInput::new(&root, &q, &txt, &stats)).expect("store_case 失败");
        if v == 0 {
            println!(
                "text sizes: short ~120B / long ~2KB (seed 0 len={})",
                txt.len()
            );
        }
    }
    println!("store: 200 seeds in {} ms", t.elapsed().as_millis());

    // 2) 复制 50 份改名 → 1 万条（目录名 + case.json id 同步改，fingerprint 不去重）。
    let t = Instant::now();
    let cases_dir = Path::new(&root).join("cases");
    let base: Vec<String> = std::fs::read_dir(&cases_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    for i in 1..50 {
        for id in &base {
            let new_id = format!("{id}_{i:02}");
            let src = cases_dir.join(id);
            let dst = cases_dir.join(&new_id);
            std::fs::create_dir(&dst).unwrap();
            std::fs::copy(src.join("case.json"), dst.join("case.json")).unwrap();
            // P4.4 评审处置：vec.bin 已移除（无效产物），不再复制。
            let p = dst.join("case.json");
            let txt = std::fs::read_to_string(&p).unwrap();
            std::fs::write(
                &p,
                txt.replace(&format!("\"id\":\"{id}\""), &format!("\"id\":\"{new_id}\"")),
            )
            .unwrap();
        }
    }
    println!(
        "replicate: 200 -> 10000 cases in {} ms",
        t.elapsed().as_millis()
    );

    // 3) 冷启动：索引未建时的线性路径首查（真实"开箱即用"体验；读模式不污染库）。
    let mut cold: Vec<u128> = Vec::new();
    for i in 0..5 {
        let q = format!("fam{} cid{} tail{}", letters(42), letters(42), letters(i));
        let t = Instant::now();
        let hits = recall_cases(&root, &q, 5, true).expect("冷启动召回失败");
        cold.push(t.elapsed().as_micros());
        assert!(!hits.is_empty(), "bench query 必须召回");
    }
    cold.sort_unstable();
    let (c50, c95, c99) = percentiles(&cold);
    println!("cold (no index, linear): p50={c50} p95={c95} p99={c99} us");

    // 4) 全量重建索引。
    let t = Instant::now();
    let (n_docs, n_terms) = rebuild(&root).expect("rebuild 失败");
    println!(
        "rebuild: docs={n_docs} terms={n_terms} in {} ms",
        t.elapsed().as_millis()
    );

    // 5) 索引路径召回基准：21 个词面不同的 query；只读模式（read_only=true）。
    //    报 p50/p95/p99（不只中位数）：p95 反映长尾抖动，p99 反映最坏一次。
    let mut samples: Vec<u128> = Vec::new();
    for i in 0..21 {
        let q = format!("fam{} cid{} tail{}", letters(42), letters(42), letters(i));
        let t = Instant::now();
        let hits = recall_cases(&root, &q, 5, true).expect("召回失败");
        samples.push(t.elapsed().as_micros());
        assert!(!hits.is_empty(), "bench query 必须召回");
    }
    samples.sort_unstable();
    let (p50, p95, p99) = percentiles(&samples);
    println!(
        "recall: p50={p50} p95={p95} p99={p99} us | samples={samples:?} us | top1 sim={}",
        recall_cases(
            &root,
            &format!("fam{} cid{}", letters(42), letters(42)),
            1,
            true
        )
        .unwrap()[0]
            .sim
    );
    if p50 < 5_000 {
        println!("P3 gate: PASS (10k recall p50 < 5ms)");
    } else {
        println!("P3 gate: FAIL (p50 {p50} us >= 5ms)");
        std::process::exit(2);
    }

    // 6) 宽查询：所有 case 都命中 `error`，候选集为全库。该路径覆盖字段/桶
    //    过滤与打分分配成本，防止只在稀疏命中查询上“看起来很快”。
    let mut broad: Vec<u128> = Vec::new();
    for i in 0..7 {
        let q = format!("ERROR timeout pool {}", letters(i));
        let t = Instant::now();
        let hits = recall_cases(&root, &q, 5, true).expect("宽查询召回失败");
        broad.push(t.elapsed().as_micros());
        assert!(!hits.is_empty());
    }
    broad.sort_unstable();
    let (b50, b95, b99) = percentiles(&broad);
    println!("broad recall: p50={b50} p95={b95} p99={b99} us | samples={broad:?} us");

    let _ = std::fs::remove_dir_all(&root);
}
