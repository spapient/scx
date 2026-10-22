//! 记忆库性能基准（P3 阶段门）：1 万条 case 库，倒排路径召回中位耗时 < 5ms。
//!
//! 运行（与 README 一致的 target-dir 策略）：
//!   Windows:   cargo run --release --example mem_bench --locked --target-dir "%TEMP%\scx-cargo-target"
//!   macOS/Linux: cargo run --release --example mem_bench --locked --target-dir "$TMPDIR/scx-cargo-target"
//!
//! 构造：200 条字母唯一种子（norm 折叠数字会导致指纹全同去重，必须用字母 token）
//! → 复制 50 份改名 → 1 万条；--rebuild 建倒排 → 预热 → 5 次采样取中位。
//! 输出 store/build/recall 三段耗时 + P3 门判据。

use scx::memory::CaseStats;
use scx::memory::{rebuild, recall_cases, store_case};
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

    // 1) 200 条字母唯一种子（约 1-2s）。
    let t = Instant::now();
    for v in 0..200 {
        let q = format!("issue fam{}", letters(v));
        let txt = format!(
            "ERROR fam{} cid{} conn timeout alpha beta gamma",
            letters(v),
            letters(v)
        );
        store_case(&root, &q, &txt, vec![], &stats, vec![]).expect("store_case 失败");
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

    // 3) 全量重建索引。
    let t = Instant::now();
    let (n_docs, n_terms) = rebuild(&root).expect("rebuild 失败");
    println!(
        "rebuild: docs={n_docs} terms={n_terms} in {} ms",
        t.elapsed().as_millis()
    );

    // 4) 召回基准：5 个词面不同的 query（词面差异 >2 编辑距离，规避 qmap 直达，
    //    强制走倒排索引路径）；每个 query 命中同一子集（fam{..} 每份 50 条）。
    //    只读模式（read_only=true）：基准不得污染库状态（qmap/L1 历史）。
    let mut samples: Vec<u128> = Vec::new();
    for i in 0..5 {
        let q = format!("fam{} cid{} tail{}", letters(42), letters(42), letters(i));
        let t = Instant::now();
        let hits = recall_cases(&root, &q, 5, true).expect("召回失败");
        samples.push(t.elapsed().as_micros());
        assert!(!hits.is_empty(), "bench query 必须召回");
    }
    samples.sort_unstable();
    let med = samples[samples.len() / 2];
    println!(
        "recall: median {} us | samples {:?} us | top1 sim={}",
        med,
        samples,
        recall_cases(
            &root,
            &format!("fam{} cid{}", letters(42), letters(42)),
            1,
            true
        )
        .unwrap()[0]
            .sim
    );
    if med < 5_000 {
        println!("P3 gate: PASS (10k recall < 5ms)");
    } else {
        println!("P3 gate: FAIL (median {med} us >= 5ms)");
        std::process::exit(2);
    }
    let _ = std::fs::remove_dir_all(&root);
}
