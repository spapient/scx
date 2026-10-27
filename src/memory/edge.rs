//! 实体边一跳扩展：case↔实体倒排（error_code/file_loc/symbol/num_fp 四类字段值
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
//! 倒排路径直接使用 `index::entity_neighbors`（postings 自带字段快照），本模块
//! 只服务 <10k 的线性召回路径。
//!
//! 实体表使用进程内缓存（root + meta.toml mtime 键，与索引同模式）；
//! store/rate/gc/rebuild 推进库代际后会触发自动 miss 与重建。
//! 库外进程无缓存，多进程共享 `--mem-dir` 仍是受支持场景（每次冷构建 ≤1 次）。

use crate::memory::store;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

/// 实体 → case id 倒排（实体键精确匹配）。
pub(crate) struct Edges {
    map: HashMap<String, Vec<String>>,
}

impl Edges {
    /// 实体命中的 case id 集合（去重、保序）。
    pub(crate) fn neighbors(&self, ents: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for e in ents {
            if let Some(ids) = self.map.get(e) {
                for id in ids {
                    if seen.insert(id.as_str()) {
                        out.push(id.clone());
                    }
                }
            }
        }
        out
    }
}

/// 缓存键：root + meta.toml mtime（任何库变更都重写 meta.toml → 键失效）。
type Entry = (String, Option<std::time::SystemTime>, Arc<Edges>);
static EDGE_CACHE: OnceLock<Mutex<Option<Entry>>> = OnceLock::new();
fn cache() -> &'static Mutex<Option<Entry>> {
    EDGE_CACHE.get_or_init(|| Mutex::new(None))
}

/// 全库构建实体表：每个 case 的四类字段值各作为一条实体键（去重）。
pub(crate) fn build(root: &str) -> std::io::Result<Edges> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for id in store::list(root)? {
        let c = store::load(root, &id)?;
        for e in crate::memory::classify::keys(&c.fields) {
            map.entry(e).or_default().push(id.clone());
        }
    }
    Ok(Edges { map })
}

/// 进程内缓存的实体表（见模块注释）。mtime 读取失败 → 每次重建（保守正确）。
fn cached(root: &str) -> std::io::Result<Arc<Edges>> {
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

/// 桥 case 的召回上下文（与 recall::Doc 同构，由调用方转写）。
pub(crate) struct Bridge {
    pub id: String,
    pub terms: std::sync::Arc<[String]>,
    pub query_terms: std::sync::Arc<[String]>,
    pub fields: std::sync::Arc<store::Fields>,
    pub status: std::sync::Arc<str>,
    pub quality: f64,
    pub updated_at: std::sync::Arc<str>,
    pub edge_shared: usize,
}

/// 桥并入：query 实体 → 兄弟 case（未在 `exist` 中）读出完整上下文与共享实体计数。
/// 桥只增加候选；调用方保持全库 df/avgdl 口径。仅当 edge_enable>0 且 query
/// 确有实体时触发。
pub(crate) fn bridge(
    root: &str,
    meta: &store::Meta,
    qf_ents: &[String],
    exist: &[String],
) -> std::io::Result<Vec<Bridge>> {
    if meta.param("edge_enable") <= 0.0 {
        return Ok(Vec::new());
    }
    let mut ents: Vec<String> = qf_ents.to_vec();
    ents.sort();
    ents.dedup();
    if ents.is_empty() {
        return Ok(Vec::new());
    }
    let idx = cached(root)?;
    let bridge = idx.neighbors(&ents);
    let mut out = Vec::new();
    for id in bridge {
        if exist.contains(&id) {
            continue;
        }
        let c = store::load(root, &id)?;
        let (query_terms, terms) = crate::terms::document(&c.query, &c.text);
        let shared = shared(&ents, &c.fields);
        out.push(Bridge {
            id,
            terms: terms.into(),
            query_terms: query_terms.into(),
            fields: std::sync::Arc::new(c.fields),
            status: c.status.into(),
            quality: c.quality,
            updated_at: c.updated_at.into(),
            edge_shared: shared,
        });
    }
    Ok(out)
}

fn shared(ents: &[String], fields: &store::Fields) -> usize {
    ents.iter().filter(|e| entity(e, fields)).count()
}

fn entity(e: &str, fields: &store::Fields) -> bool {
    fields.error_code.iter().any(|x| x == e)
        || fields.file_loc.iter().any(|x| x == e)
        || fields.symbol.iter().any(|x| x == e)
        || fields.num_fp.iter().any(|x| x == e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ingest::ingest;
    use crate::memory::recall::recall;
    use crate::memory::store::Stats;
    use crate::memory::util::root;
    use std::fs;

    fn stats() -> Stats {
        Stats {
            kept: 2,
            total: 20,
            cut: 90.0,
            risk: "LOW".to_string(),
        }
    }

    /// 实体边一跳（召回集成）：共享 file_loc 实体但主桶不同（build）的兄弟 case，
    /// 被 db/network 候选桶过滤掉后，由实体边兜底并入候选并加权召回；关桥则被滤。
    #[test]
    fn edge_bridge_recovers_cross_bucket_sibling() {
        let root = root("eb");
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
        let hits = recall(&root, q_open, 3, &crate::memory::meta::load(&root).unwrap()).unwrap();
        let b_id = {
            let ids = store::list(&root).unwrap();
            ids.iter()
                .find(|id| store::load(&root, id).unwrap().query == "compile fail")
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
        crate::memory::meta::set(&root, "edge_enable", 0.0).unwrap();
        // 隔离本测试：关闭低置信桶回退，确保 B 只能被实体边召回。
        crate::memory::meta::set(&root, "bucket_bypass_sim", 0.0).unwrap();
        let q_off = "数据库连接池 timeout at Pool.java:214 pool";
        let hits2 = recall(&root, q_off, 3, &crate::memory::meta::load(&root).unwrap()).unwrap();
        assert!(
            !hits2.iter().any(|h| h.id == b_id),
            "关桥后 B 应被桶过滤排除：{:?}",
            hits2.iter().map(|h| h.id.as_str()).collect::<Vec<_>>()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn build_and_neighbors_are_deterministic() {
        let root = root("bn");
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
