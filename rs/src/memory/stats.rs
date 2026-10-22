//! 库盘点（--stats，P4.4+ 优化）：case 数 / 桶分布 / 状态分布 / fix 覆盖率 /
//! 质量均值 / 磁盘占用。维护命令，线性全扫可接受（read_case 现算，零额外状态）。
//!
//! 用途：库膨胀监控（配合 --gc）、fix 补录缺口发现（哪些 solved 还没记修法）、
//! 桶倾斜观察（候选桶过滤是否有空桶问题）。确定性输出，无随机源。

use crate::memory::store;
use std::collections::HashMap;
use std::io;

/// 统计一行式输出（`stats: ...` + 4 行明细），供 CLI --stats 打印。
pub(crate) fn table(root: &str) -> io::Result<Vec<String>> {
    let meta = store::load_meta(root)?;
    let ids = store::list_ids(root)?;
    let mut buckets: HashMap<String, usize> = HashMap::new();
    let mut statuses: HashMap<String, usize> = HashMap::new();
    let mut fix_n = 0usize;
    let mut quality_sum = 0.0f64;
    let mut disk = 0u64;
    for id in &ids {
        let c = store::read_case(root, id)?;
        *buckets.entry(c.bucket.clone()).or_insert(0) += 1;
        *statuses.entry(c.status.clone()).or_insert(0) += 1;
        if !c.fix.is_empty() {
            fix_n += 1;
        }
        quality_sum += c.quality;
        // case 目录 = cases/<id>/（case.json 的父目录），dir_size 统计其递归大小。
        let dir = store::case_path(root, id)
            .parent()
            .expect("case_path 必含 cases/<id>/case.json")
            .to_path_buf();
        disk += dir_size(&dir)?;
    }
    let n = ids.len();
    let mut lines = Vec::with_capacity(6);
    lines.push(format!("stats: cases={} gen={}", n, meta.gen));
    lines.push(format!("  bucket: {}", join_map(&buckets)));
    lines.push(format!("  status: {}", join_map(&statuses)));
    lines.push(format!(
        "  fix: {}/{} ({:.0}%)",
        fix_n,
        n,
        if n == 0 {
            0.0
        } else {
            100.0 * fix_n as f64 / n as f64
        }
    ));
    lines.push(format!(
        "  quality: mean={:.2}",
        if n == 0 { 0.0 } else { quality_sum / n as f64 }
    ));
    lines.push(format!("  disk: {:.1} KiB", disk as f64 / 1024.0));
    Ok(lines)
}

/// map 按 key 字典序输出（确定性）：`db=5 network=3 ...`。
fn join_map(m: &HashMap<String, usize>) -> String {
    let mut ks: Vec<&String> = m.keys().collect();
    ks.sort();
    ks.iter()
        .map(|k| format!("{}={}", k, m[*k]))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 目录递归大小（case 目录：case.json，不含残留杂物）。
fn dir_size(p: &std::path::Path) -> io::Result<u64> {
    let mut total = 0u64;
    for e in std::fs::read_dir(p)? {
        let e = e?;
        let md = e.metadata()?;
        if md.is_dir() {
            total += dir_size(&e.path())?;
        } else {
            total += md.len();
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ingest::ingest;
    use crate::memory::store::CaseStats;
    use std::fs;

    fn tmp_root(tag: &str) -> String {
        let r =
            std::env::temp_dir().join(format!("scx-stats-{}-{}", tag, store::fnv1a("s") & 0xff));
        let _ = fs::remove_dir_all(&r);
        r.to_str().unwrap().to_string()
    }

    fn stats() -> CaseStats {
        CaseStats {
            kept: 3,
            total: 100,
            cut_pct: 90.0,
            risk: "LOW".to_string(),
        }
    }

    #[test]
    fn table_reports_counts_and_coverage() {
        let root = tmp_root("t");
        let s = stats();
        ingest(
            &root,
            "db timeout",
            vec!["conn_id".into()],
            "ERROR db timeout\nat Pool.java:214",
            vec!["/a.log".into()],
            &s,
            vec![],
        )
        .unwrap();
        crate::memory::ingest::ingest_with_fix(
            &root,
            "compile fail",
            vec!["compile".into()],
            "ERROR compile fail\nat Db.java:4",
            vec!["/b.log".into()],
            &s,
            vec![],
            "pin toolchain 1.82",
        )
        .unwrap();
        let lines = table(&root).unwrap();
        assert!(
            lines[0].starts_with("stats: cases=2 gen="),
            "{:?}",
            lines[0]
        );
        let joined = lines.join("\n");
        assert!(joined.contains("bucket: build=1 db=1"), "{joined}");
        assert!(joined.contains("status: open=2"), "{joined}");
        assert!(joined.contains("fix: 1/2 (50%)"), "{joined}");
        assert!(joined.contains("disk: "), "{joined}");
        let _ = fs::remove_dir_all(&root);
    }
}
