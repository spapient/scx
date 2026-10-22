//! P4.3 实体边一跳扩展：case↔实体倒排（error_code/file_loc/symbol/num_fp 四类字段值
//! 各为一条实体键），召回沿边扩一跳取兄弟 case。
//!
//! 机制与边界（诚实口径）：实体键是 case 词面的子集（token 层无法"零词面共享"），
//! 实体边的真实收益有两条：
//!
//! 1) 跨桶兜底：query 判候选桶后，被桶过滤滤掉、但实体键相同的兄弟 case 被兜底并入候选；
//! 2) 实体加权：共享实体数 × edge_weight 直接进打分（fb），把实体强相关但词面弱的
//!    兄弟 case 提过截止线。
//!
//! 零依赖、确定性；实体表全库现算（小库口径）。
//!
//! P4.4+ 优化⑬（P1-5）：`build` 原本每次召回重建全库实体表（1 万条库 read_case×N
//! ≈ 0.85s/次，P3 性能回归）。改为进程内缓存（root+meta.toml mtime 键，与 index
//! 同模式）：store/rate/gc/rebuild 都 bump gen → meta.toml 变化 → 自动 miss 重建。
//! 库外进程无缓存，多进程共享 `--mem-dir` 仍是受支持场景（每次冷构建 ≤1 次）。

use crate::memory::store;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// 实体 → case id 倒排（实体键精确匹配）。
pub(crate) struct EdgeIndex {
    map: HashMap<String, Vec<String>>,
}

impl EdgeIndex {
    /// 实体命中的 case id 集合（去重、保序）。
    pub(crate) fn neighbors(&self, ents: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for e in ents {
            if let Some(ids) = self.map.get(e) {
                for id in ids {
                    if !out.contains(id) {
                        out.push(id.clone());
                    }
                }
            }
        }
        out
    }
}

/// 缓存键：root + meta.toml mtime（任何库变更都重写 meta.toml → 键失效）。
type EdgeCacheEntry = (String, Option<std::time::SystemTime>, Arc<EdgeIndex>);
static EDGE_CACHE: OnceLock<Mutex<Option<EdgeCacheEntry>>> = OnceLock::new();
fn cache() -> &'static Mutex<Option<EdgeCacheEntry>> {
    EDGE_CACHE.get_or_init(|| Mutex::new(None))
}

/// 全库构建实体表：每个 case 的四类字段值各作为一条实体键（去重）。
pub(crate) fn build(root: &str) -> std::io::Result<EdgeIndex> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for id in store::list_ids(root)? {
        let c = store::read_case(root, &id)?;
        let mut ents: Vec<String> = Vec::with_capacity(
            c.fields.error_code.len()
                + c.fields.file_loc.len()
                + c.fields.symbol.len()
                + c.fields.num_fp.len(),
        );
        ents.extend(c.fields.error_code.iter().cloned());
        ents.extend(c.fields.file_loc.iter().cloned());
        ents.extend(c.fields.symbol.iter().cloned());
        ents.extend(c.fields.num_fp.iter().cloned());
        ents.sort();
        ents.dedup();
        for e in ents {
            map.entry(e).or_default().push(id.clone());
        }
    }
    Ok(EdgeIndex { map })
}

/// 进程内缓存的实体表（见模块注释）。mtime 读取失败 → 每次重建（保守正确）。
fn cached_build(root: &str) -> std::io::Result<Arc<EdgeIndex>> {
    let p = std::path::Path::new(root).join("meta.toml");
    let mt = p.metadata().and_then(|m| m.modified()).ok();
    let mut g = cache().lock().unwrap();
    if let Some((r, t, idx)) = g.as_ref() {
        if r == root && *t == mt {
            return Ok(idx.clone());
        }
    }
    let idx = Arc::new(build(root)?);
    *g = Some((root.to_string(), mt, idx.clone()));
    Ok(idx)
}

/// 桥 case 的召回上下文（与 recall::DocCtx 同构，由调用方转写）。
pub(crate) struct BridgeCase {
    pub id: String,
    pub terms: Vec<String>,
    pub query_terms: Vec<String>,
    pub fields: store::Fields,
    pub status: String,
    pub quality: f64,
    pub updated_at: String,
    pub edge_shared: usize,
}

/// P4.3 桥并入：query 实体 → 兄弟 case（未在 `exist` 中）读出完整上下文 + 共享实体计数。
/// 返回 (桥 case 列表, 桥词长合计，供调用方 avgdl 重算)；df 口径由调用方保持。
/// 仅当 edge_enable>0 且 query 确有实体时触发。
pub(crate) fn bridge_cases(
    root: &str,
    params: &[(String, f64)],
    qf_ents: &[String],
    exist: &[String],
) -> std::io::Result<(Vec<BridgeCase>, usize)> {
    let edge_on = params
        .iter()
        .find(|(k, _)| k == "edge_enable")
        .map(|(_, v)| *v)
        .unwrap_or(1.0);
    if edge_on <= 0.0 {
        return Ok((Vec::new(), 0));
    }
    let mut ents: Vec<String> = qf_ents.to_vec();
    ents.sort();
    ents.dedup();
    if ents.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let idx = cached_build(root)?;
    let bridge = idx.neighbors(&ents);
    let mut out = Vec::new();
    let mut total = 0usize;
    for id in bridge {
        if exist.contains(&id) {
            continue;
        }
        let c = store::read_case(root, &id)?;
        let query_terms = crate::terms::build_mem_terms(&c.query);
        let mut terms = query_terms.clone();
        terms.extend(crate::terms::build_mem_terms(&c.text));
        terms.sort();
        terms.dedup();
        let shared = ents
            .iter()
            .filter(|e| {
                c.fields.error_code.contains(e)
                    || c.fields.file_loc.contains(e)
                    || c.fields.symbol.contains(e)
                    || c.fields.num_fp.contains(e)
            })
            .count();
        total += terms.len();
        out.push(BridgeCase {
            id,
            terms,
            query_terms,
            fields: c.fields,
            status: c.status,
            quality: c.quality,
            updated_at: c.updated_at,
            edge_shared: shared,
        });
    }
    Ok((out, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ingest::ingest;
    use crate::memory::recall::recall;
    use crate::memory::store::CaseStats;
    use std::fs;

    fn tmp_root(tag: &str) -> String {
        let r = std::env::temp_dir().join(format!("scx-edge-{}-{}", tag, store::fnv1a("e") & 0xff));
        let _ = fs::remove_dir_all(&r);
        r.to_str().unwrap().to_string()
    }

    fn stats() -> CaseStats {
        CaseStats {
            kept: 2,
            total: 20,
            cut_pct: 90.0,
            risk: "LOW".to_string(),
        }
    }

    /// P4.3 实体边一跳（召回集成）：共享 file_loc 实体但主桶不同（build）的兄弟 case，
    /// 被 db/network 候选桶过滤掉后，由实体边兜底并入候选并加权召回；关桥则被滤。
    #[test]
    fn edge_bridge_recovers_cross_bucket_sibling() {
        let root = tmp_root("eb");
        let s1 = stats();
        // A：db/network 桶（含 timeout），共享实体 pool.java:214。
        ingest(
            &root,
            "db timeout",
            vec!["conn_id".into()],
            "ERROR db timeout\nat Pool.java:214",
            vec!["/a.log".into()],
            &s1,
            vec![],
        )
        .unwrap();
        // B：build 桶（compile），词面弱（无 timeout），但共享实体 pool.java:214。
        ingest(
            &root,
            "compile fail",
            vec!["compile".into()],
            "ERROR compile fail\nat Pool.java:214",
            vec!["/b.log".into()],
            &s1,
            vec![],
        )
        .unwrap();
        // C：db/network 桶干扰 case，无共享实体。
        ingest(
            &root,
            "db timeout",
            vec!["errno".into()],
            "ERROR db timeout\nat Db.java:4",
            vec!["/c.log".into()],
            &s1,
            vec![],
        )
        .unwrap();
        // 桥开（默认 edge_enable=1）：q 实体 pool.java:214 → B 兜底召回。
        let q_open = "数据库连接池 timeout at Pool.java:214";
        let hits = recall(&root, q_open, 3).unwrap();
        let b_id = {
            let ids = store::list_ids(&root).unwrap();
            ids.iter()
                .find(|id| store::read_case(&root, id).unwrap().query == "compile fail")
                .cloned()
                .unwrap()
        };
        assert!(
            hits.iter().any(|h| h.id == b_id),
            "实体边应兜底召回跨桶兄弟 case：{}（B 桶=build 被 db/network 候选滤掉）",
            hits.iter()
                .map(|h| h.id.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
        // 关桥（edge_enable=0）：B 被桶过滤，不再出现在结果里。
        crate::memory::meta::set_param(&root, "edge_enable", 0.0).unwrap();
        // 隔离本测试：关闭低置信桶回退，确保 B 只能被实体边召回。
        crate::memory::meta::set_param(&root, "bucket_bypass_sim", 0.0).unwrap();
        let q_off = "数据库连接池 timeout at Pool.java:214 pool";
        let hits2 = recall(&root, q_off, 3).unwrap();
        assert!(
            !hits2.iter().any(|h| h.id == b_id),
            "关桥后 B 应被桶过滤排除：{:?}",
            hits2.iter().map(|h| h.id.as_str()).collect::<Vec<_>>()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn build_and_neighbors_are_deterministic() {
        let root = tmp_root("bn");
        ingest(
            &root,
            "db timeout",
            vec!["conn_id".into()],
            "ERROR db timeout conn_id=9f2a\nat Pool.java:214",
            vec!["/a.log".into()],
            &stats(),
            vec![],
        )
        .unwrap();
        ingest(
            &root,
            "compile fail",
            vec!["compile".into()],
            "ERROR compile fail\nat Pool.java:214",
            vec!["/b.log".into()],
            &stats(),
            vec![],
        )
        .unwrap();
        ingest(
            &root,
            "db errno",
            vec!["errno".into()],
            "ERROR db errno=28\nat Db.java:4",
            vec!["/c.log".into()],
            &stats(),
            vec![],
        )
        .unwrap();
        let idx = build(&root).unwrap();
        // 共享 file_loc 实体：Pool.java:214 → 两个 case；独立实体不串。
        let n = idx.neighbors(&["pool.java:214".to_string()]);
        assert_eq!(n.len(), 2, "Pool.java:214 应命中 2 个 case: {n:?}");
        let e = idx.neighbors(&["conn_id=9f2a".to_string()]);
        assert_eq!(e.len(), 1, "conn_id=9f2a 只在一个 case 里");
        // 无命中实体 → 空（不报错）。
        assert!(idx.neighbors(&["no_such_entity".to_string()]).is_empty());
        let _ = fs::remove_dir_all(&root);
    }
}
