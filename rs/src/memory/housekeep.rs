//! 库维护（P3.3）：`--rebuild` 重建索引、`--gc` 过期清理、删除惰性作废。
//!
//! - rebuild：全量重建 postings.bin（index::build）+ qmap 合并（同指纹取 recency
//!   最新）+ df 快照写回 meta.toml [df]（领域 IDF 数据源，P4 评估用）+ 写
//!   不改变索引启用策略（索引只是可丢弃缓存）。
//! - gc：expire 双条件（meta [params] gc_ttl_regressed=180 / gc_ttl_stale=365）：
//!   regressed 且超期，或从未反馈（quality=0）且陈旧 → 物理删除 + 索引作废。
//! - delete_case：物理删除 + 惰性作废索引（删 postings.bin，回退线性；重建由
//!   rebuild/自动构建恢复）。
//!
//! 索引是可丢弃缓存（index/）：任何维护操作不触碰 cases/ 资产；坏段由
//! index::load 校验报告，重建覆盖。

use crate::memory::index;
use crate::memory::qmap;
use crate::memory::store;
use crate::memory::util::{parse_utc_secs, utc_now};
use std::fs;
use std::path::{Path, PathBuf};

/// 全量重建索引 + qmap 合并 + df 快照。返回 (n_docs, n_terms)。
pub(crate) fn rebuild(root: &str) -> std::io::Result<(usize, usize)> {
    store::ensure_root(root)?;
    // 1) 倒排索引全量重建（确定性：case id 排序 + term 字典序）；df 快照随建随取。
    let (n_docs, n_terms, df) = index::build(root)?;
    // 2) qmap 合并：同归一化 query 只留 recency 最新一条。
    qmap::merge(root)?;
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
        let c = match store::read_case(root, &id) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let upd_s = parse_utc_secs(&c.updated_at);
        let age_days = now_s.saturating_sub(upd_s) as f64 / 86_400.0;
        // P4.4+ 优化⑫（P1-4）：带 fix 的 case（已沉淀"怎么解决"）不因 stale 过期——
        // 经验价值最高，默认永不因"未反馈"被清；regressed 仍按短 TTL 清理。
        let expired = (c.status == "regressed" && age_days > ttl_regressed)
            || (c.quality == 0.0 && c.fix.is_empty() && age_days > ttl_stale);
        if expired && delete_case(root, &id).unwrap_or(false) {
            removed += 1;
        }
    }
    Ok(removed)
}

/// P4.4+ 优化⑫（P1-4）：库总字节数（cases + meta + index 文件递归求和）。
/// 供 `--max-bytes` 软配额检查（每个 store 前调用；1 万 case 目录枚举 <100ms）。
pub fn bytes_total(root: &str) -> std::io::Result<u64> {
    fn walk(dir: &std::path::Path) -> std::io::Result<u64> {
        let mut total = 0u64;
        let mut stack: Vec<std::path::PathBuf> = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d)? {
                let entry = entry?;
                let p = entry.path();
                let ft = entry.file_type()?;
                if ft.is_dir() {
                    stack.push(p);
                } else if ft.is_file() {
                    total += entry.metadata()?.len();
                }
            }
        }
        Ok(total)
    }
    if std::path::Path::new(root).exists() {
        walk(std::path::Path::new(root))
    } else {
        Ok(0)
    }
}

/// 删除一个 case（物理删除 + 惰性作废索引 + 库代际前进）。
/// id 先过白名单校验（P4.4 评审处置：禁 `..`/路径分隔符逃逸）。
/// 返回是否存在。
pub(crate) fn delete_case(root: &str, id: &str) -> std::io::Result<bool> {
    // case 锁串行化 rate/去重更新；index 锁覆盖“删资产 → 作废索引 → bump gen”，
    // 防止并发 rebuild 在失效后把已删除 case 写回。
    let _case_lock = store::lock_case(root, id)?;
    let _index_lock = index::lock(root)?;
    let dir = Path::new(root).join("cases").join(id);
    if !dir.exists() {
        return Ok(false);
    }
    fs::remove_dir_all(dir)?;
    index::invalidate_locked(root);
    store::bump_case_count(root)?;
    crate::memory::meta::bump_gen(root)?;
    if let Ok(_g) = crate::memory::lock::acquire(root, "qmap.lock") {
        let _ = fs::remove_file(qmap_path(root));
    }
    Ok(true)
}

fn qmap_path(root: &str) -> PathBuf {
    Path::new(root).join("index").join("qmap.bin")
}

/// 写 meta.toml [df] 快照（来自 index::build）。
/// P4.4 评审处置：**不再写 index_force** —— 索引只是缓存，重建不得改变召回语义
/// （线性路径 df/avgdl 为候选桶口径，索引路径为全库口径，强制切换会让排序跳变、
/// A/B 评估不可比）。索引启用只由规模阈值决定；强制索引走独立测试参数（预留）。
/// 失败静默旁路。
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
        if t.starts_with('[') && t.ends_with(']') {
            if t == "[df]" {
                // 跳过旧 df 节，统一在末尾重写。
                wrote_df = true;
                continue;
            }
            if !wrote_df && !df.is_empty() {
                out.push("[df]".to_string());
                for (k, v) in df {
                    out.push(format!("{k} = {v}"));
                }
                out.push("".to_string());
                wrote_df = true;
            }
        }
        if t.starts_with("case_count") {
            out.push(format!("case_count = {n_docs}"));
            continue;
        }
        out.push(line.to_string());
    }
    if !wrote_df && !df.is_empty() {
        out.push("[df]".to_string());
        for (k, v) in df {
            out.push(format!("{k} = {v}"));
        }
        out.push("".to_string());
    }
    let mut body = out.join("\n");
    body.push('\n');
    let _ = crate::memory::lock::atomic_write(&p, body.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ingest::{ingest, ingest_with_fix};
    use crate::memory::store::{self, CaseStats};
    use std::fs;

    fn tmp_root(tag: &str) -> String {
        let r = std::env::temp_dir().join(format!("scx-hk-{}-{}", tag, store::fnv1a("h") & 0xff));
        let _ = fs::remove_dir_all(&r);
        r.to_str().unwrap().to_string()
    }

    fn stats() -> CaseStats {
        CaseStats {
            kept: 5,
            total: 100,
            cut_pct: 95.0,
            risk: "HIGH".to_string(),
        }
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

    /// P4.4+ 优化⑫（P1-4）：带 fix 的 case 不因 stale 过期（经验价值最高）。
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
