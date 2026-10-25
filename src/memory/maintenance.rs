//! 库维护：索引重建、过期清理、case 删除与 `--stats` 盘点。
//!
//! - rebuild：全量重建 postings.bin（index::build）+ qmap 合并（同指纹取 recency
//!   最新）+ df 快照写回 meta.toml [df]（评估与召回共用的 IDF 数据源）+ 写
//!   不改变索引启用策略（索引只是可丢弃缓存）。
//! - gc：expire 双条件（meta [params] gc_ttl_regressed=180 / gc_ttl_stale=365）：
//!   regressed 且超期，或从未反馈（quality=0）且陈旧 → 物理删除 + 索引作废。
//! - delete_case：物理删除 + 惰性作废索引（删 postings.bin，回退线性；重建由
//!   rebuild/自动构建恢复）。
//!
//! 索引是可丢弃缓存（index/）：任何维护操作不触碰 cases/ 资产；坏段由
//! index::load 校验报告，重建覆盖。

use crate::memory::index;
use crate::memory::query_cache;
use crate::memory::store;
use crate::memory::util::{parse_utc_secs, utc_now};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

/// 全量重建索引 + qmap 合并 + df 快照。返回 (n_docs, n_terms)。
pub(crate) fn rebuild(root: &str) -> std::io::Result<(usize, usize)> {
    store::ensure_root(root)?;
    // 1) 倒排索引全量重建（确定性：case id 排序 + term 字典序）；df 快照随建随取。
    let (n_docs, n_terms, df) = index::build(root)?;
    // 2) qmap 合并：同归一化 query 只留 recency 最新一条。
    query_cache::merge(root)?;
    // 3) df 快照写 meta.toml [df]；不改 index_force（重建不得改变召回语义）。
    write_meta_index(root, n_docs, &df);
    Ok((n_docs, n_terms))
}

/// 过期清理：删除满足 expire 双条件的 case。返回删除数。
pub(crate) fn gc(root: &str) -> std::io::Result<usize> {
    let meta = store::load_meta(root)?;
    let ttl_regressed = meta.param("gc_ttl_regressed");
    let ttl_stale = meta.param("gc_ttl_stale");
    let now_s = parse_utc_secs(&utc_now());
    let mut removed = 0usize;
    for id in store::list_ids(root)? {
        let Ok(c) = store::read_case(root, &id) else {
            continue;
        };
        if case_expired(&c, now_s, ttl_regressed, ttl_stale)
            && delete_case(root, &id).unwrap_or(false)
        {
            removed += 1;
        }
    }
    Ok(removed)
}

/// 过期双条件：regressed 超短 TTL，或从未反馈且无 fix 且陈旧。
fn case_expired(c: &store::Case, now_s: u64, ttl_regressed: f64, ttl_stale: f64) -> bool {
    let age_days = now_s.saturating_sub(parse_utc_secs(&c.updated_at)) as f64 / 86_400.0;
    (c.status == "regressed" && age_days > ttl_regressed)
        || (c.quality == 0.0 && c.fix.is_empty() && age_days > ttl_stale)
}

/// 删除一个 case（物理删除 + 惰性作废索引 + 库代际前进）。
/// id 先过白名单校验，禁止 `..` 与路径分隔符逃逸。
/// 返回是否存在。
pub(crate) fn delete_case(root: &str, id: &str) -> std::io::Result<bool> {
    // 锁序契约（docs/locking.md）：case → index → meta → qmap，细粒度在前。
    // acquire_all 按固定序一次性取齐，任一把失败即整体放弃（已持锁逆序释放），
    // 绝不半持锁返回；meta 的写入用 *_locked 变体避免自死锁。
    let _guards = crate::memory::lock::acquire_all(
        root,
        &[
            &format!("cases/{id}.lock"),
            "index.lock",
            "meta.lock",
            "qmap.lock",
        ],
    )?;
    let dir = Path::new(root).join("cases").join(id);
    if !dir.exists() {
        return Ok(false);
    }
    fs::remove_dir_all(dir)?;
    index::invalidate_locked(root);
    store::bump_case_count_locked(root)?;
    crate::memory::ingest::invalidate_fp_index(root);
    crate::memory::meta::bump_gen_locked(root)?;
    let _ = fs::remove_file(query_cache::qmap_path(root));
    Ok(true)
}

/// 写 meta.toml [df] 快照（来自 index::build）。
/// 不写 `index_force`：索引只是缓存，重建不得改变召回语义
/// （线性/索引两条路径的 df/avgdl 均为全库口径，桶过滤只裁候选；强制索引只用于
/// A/B 对拍，不写回 meta）。索引启用只由规模阈值决定。失败静默旁路。
fn write_meta_index(root: &str, n_docs: usize, df: &[(String, usize)]) {
    let Ok(_g) = crate::memory::lock::acquire(root, "meta.lock") else {
        return;
    };
    let p = Path::new(root).join("meta.toml");
    let Ok(txt) = fs::read_to_string(&p) else {
        return;
    };
    let mut out: Vec<String> = Vec::new();
    let mut wrote_df = false;
    for line in txt.lines() {
        let t = line.trim();
        if is_section_line(t) {
            if t == "[df]" {
                // 跳过旧 df 节，统一在末尾重写。
                wrote_df = true;
                continue;
            }
            if should_insert_df(wrote_df, df) {
                out.extend(df_lines(df));
                wrote_df = true;
            }
        }
        if t.starts_with("case_count") {
            out.push(format!("case_count = {n_docs}"));
            continue;
        }
        out.push(line.to_string());
    }
    if should_insert_df(wrote_df, df) {
        out.extend(df_lines(df));
    }
    let mut body = out.join("\n");
    body.push('\n');
    let _ = crate::memory::lock::atomic_write(&p, body.as_bytes());
}

fn is_section_line(t: &str) -> bool {
    t.starts_with('[') && t.ends_with(']')
}

fn should_insert_df(wrote_df: bool, df: &[(String, usize)]) -> bool {
    !wrote_df && !df.is_empty()
}

fn df_lines(df: &[(String, usize)]) -> Vec<String> {
    let mut out = vec!["[df]".to_string()];
    for (k, v) in df {
        out.push(format!("{k} = {v}"));
    }
    out.push(String::new());
    out
}

/// `--stats` 盘点：case/桶/状态/fix 覆盖率/质量/磁盘占用。
pub(crate) fn stats(root: &str) -> io::Result<Vec<String>> {
    let meta = store::load_meta(root)?;
    let ids = store::list_ids(root)?;
    let mut buckets: HashMap<String, usize> = HashMap::new();
    let mut statuses: HashMap<String, usize> = HashMap::new();
    let mut fix_n = 0usize;
    let mut quality_sum = 0.0f64;
    let mut disk = 0u64;
    for id in &ids {
        accumulate_stats(
            root,
            id,
            &mut buckets,
            &mut statuses,
            &mut fix_n,
            &mut quality_sum,
            &mut disk,
        )?;
    }
    let n = ids.len();
    let mut lines = Vec::with_capacity(6);
    lines.push(format!("stats: cases={} gen={}", n, meta.gen));
    lines.push(format!("  bucket: {}", join_counts(&buckets)));
    lines.push(format!("  status: {}", join_counts(&statuses)));
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

fn accumulate_stats(
    root: &str,
    id: &str,
    buckets: &mut HashMap<String, usize>,
    statuses: &mut HashMap<String, usize>,
    fix_n: &mut usize,
    quality_sum: &mut f64,
    disk: &mut u64,
) -> io::Result<()> {
    let c = store::read_case(root, id)?;
    *buckets.entry(c.bucket.clone()).or_insert(0) += 1;
    *statuses.entry(c.status.clone()).or_insert(0) += 1;
    if !c.fix.is_empty() {
        *fix_n += 1;
    }
    *quality_sum += c.quality;
    let dir = store::case_path(root, id)
        .parent()
        .expect("case_path 必含 cases/<id>/case.json")
        .to_path_buf();
    *disk += store::dir_bytes(&dir)?;
    Ok(())
}

/// map 按 key 字典序输出（确定性）：`db=5 network=3 ...`。
fn join_counts(counts: &HashMap<String, usize>) -> String {
    let mut keys: Vec<&String> = counts.keys().collect();
    keys.sort();
    keys.iter()
        .map(|k| format!("{}={}", k, counts[*k]))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ingest::{ingest, ingest_with_fix};
    use crate::memory::store::{self, CaseStats};
    use crate::memory::util::tmp_root;
    use std::fs;

    fn stats() -> CaseStats {
        crate::memory::store::sample_stats()
    }

    #[test]
    fn rebuild_builds_index_without_forcing_flag() {
        let root = tmp_root("rb");
        let s = stats();
        ingest(
            &root,
            "数据库超时",
            vec!["conn_id".into()],
            "ERROR db timeout conn_id=9f2a alpha",
            vec!["/a.log".into()],
            &s,
            vec![],
        )
        .unwrap();
        crate::memory::meta::set_param(&root, "index_force", 1.0).unwrap();
        let (n_docs, n_terms) = rebuild(&root).unwrap();
        assert_eq!(n_docs, 1);
        assert!(n_terms > 0);
        assert!(index::load(&root).unwrap().is_some(), "索引已建");
        let m = store::load_meta(&root).unwrap();
        assert_eq!(
            m.param("index_force"),
            1.0,
            "rebuild 不得改写 index_force（索引只是缓存，重建不得改变召回语义）"
        );
        assert!(
            m.df.iter().any(|(k, _)| k == "alpha"),
            "rebuild 落 df 快照（领域 IDF 数据源）"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gc_removes_expired_only() {
        let root = tmp_root("gc");
        let s = stats();
        ingest(
            &root,
            "数据库超时",
            vec!["conn_id".into()],
            "ERROR db timeout conn_id=9f2a gamma",
            vec!["/a.log".into()],
            &s,
            vec![],
        )
        .unwrap();
        ingest(
            &root,
            "磁盘满",
            vec!["disk".into()],
            "ERROR disk full delta",
            vec!["/b.log".into()],
            &s,
            vec![],
        )
        .unwrap();
        // 把第一个 case 拨成 regressed + 超期（gc_ttl_regressed=180）。
        let ids = store::list_ids(&root).unwrap();
        let mut c = store::read_case(&root, &ids[0]).unwrap();
        c.status = "regressed".to_string();
        let now = store::utc_now();
        let older = format!("2020-01-01{}", &now[10..]);
        c.updated_at = older;
        store::write_case(&root, &c).unwrap();
        let removed = gc(&root).unwrap();
        assert_eq!(removed, 1, "regressed 超期应被清");
        assert_eq!(store::list_ids(&root).unwrap().len(), 1);
        // 索引已作废（惰性）。
        assert!(!index::load(&root).unwrap().is_some(), "gc 后索引作废");
        let _ = fs::remove_dir_all(&root);
    }

    /// 带 fix 的 case 不因 stale 过期（经验价值最高）。
    #[test]
    fn gc_keeps_stale_case_with_fix() {
        let root = tmp_root("gc-fix");
        let s = stats();
        let mut c = ingest_fix_direct(&root, &s);
        c.quality = 0.0; // 从未反馈
        let now = store::utc_now();
        let older = format!("2020-01-01{}", &now[10..]);
        c.updated_at = older; // 远超 gc_ttl_stale=365
        store::write_case(&root, &c).unwrap();
        let removed = gc(&root).unwrap();
        assert_eq!(removed, 0, "带 fix 的 case 不应被 stale GC 清理");
        assert_eq!(store::list_ids(&root).unwrap().len(), 1);
        // 去掉 fix 后同条件应被清（对照）。
        let mut c2 = store::read_case(&root, &store::list_ids(&root).unwrap()[0]).unwrap();
        c2.fix = String::new();
        store::write_case(&root, &c2).unwrap();
        assert_eq!(gc(&root).unwrap(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    /// 直接落一个带 fix 的 case（经 ingest_with_fix，目录/指纹/写盘齐全）。
    fn ingest_fix_direct(root: &str, s: &store::CaseStats) -> store::Case {
        ingest_with_fix(
            root,
            "磁盘满",
            vec!["disk".into()],
            "ERROR disk full delta",
            vec!["/b.log".into()],
            s,
            vec![],
            "扩容并清理 inode",
        )
        .unwrap();
        store::read_case(root, &store::list_ids(root).unwrap()[0]).unwrap()
    }

    #[test]
    fn delete_case_invalidates_index() {
        let root = tmp_root("del");
        let s = stats();
        ingest(
            &root,
            "数据库超时",
            vec!["conn_id".into()],
            "ERROR db timeout conn_id=9f2a zeta",
            vec!["/a.log".into()],
            &s,
            vec![],
        )
        .unwrap();
        rebuild(&root).unwrap();
        let id = store::list_ids(&root).unwrap()[0].clone();
        assert!(delete_case(&root, &id).unwrap());
        assert!(store::list_ids(&root).unwrap().is_empty());
        assert!(!index::load(&root).unwrap().is_some(), "删除后索引惰性作废");
        let m = store::load_meta(&root).unwrap();
        assert_eq!(m.case_count, 0);
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;
    use crate::memory::ingest::{ingest, ingest_with_fix};
    use crate::memory::store::CaseStats;
    use crate::memory::util::tmp_root;

    fn case_stats() -> CaseStats {
        CaseStats {
            kept: 3,
            total: 100,
            cut_pct: 90.0,
            risk: "LOW".to_string(),
        }
    }

    #[test]
    fn stats_reports_counts_and_coverage() {
        let root = tmp_root("t");
        let s = case_stats();
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
        ingest_with_fix(
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
        let lines = stats(&root).unwrap();
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
