//! 冲突仲裁：写入时的 minhash 近重复检测 + 证据/质量裁决 + related 互链 + log。
//!
//! 语义：同一事故的不同描述（query 不同、text 高度相似）不应静默堆积为孤立
//! 记忆。新 case 落盘前对库内候选（同桶优先、上限 `conflict_max`）做加权
//! Jaccard：sim ≥ `conflict_sim` 即触发仲裁。裁决规则（**不删除数据**）：
//!   - 证据更全（evidence.len 更大）者胜；平局比 quality；再平局先到先得
//!     （已存在 case 保留，新 case 后到败）；
//!   - 败者 `related` 链向胜者，胜者 `related` 也链回败者（双向互链）；
//!   - 每次仲裁 append 一行记录到 `index/conflicts.log`（TSV，确定性，
//!     无时钟随机源，时间戳用写入时刻）。

use crate::memory::store::{self, Case, Meta};
use std::io;

pub(crate) const CONFLICT_LOG: &str = "index/conflicts.log";

/// 冲突候选：近重复 case 的裁决依据快照（id + sim + 证据数 + 质量）。
pub(crate) struct Conflict {
    pub id: String,
    pub sim: f64,
    pub evidence: usize,
    pub quality: f64,
}

/// 近重复检测：新 case 与库内候选的加权 Jaccard（复用 `minhash::scores`）。
/// 候选 = 同桶优先（index 快照零额外 I/O）；同桶不足 `conflict_max` 时按库
/// 顺序补齐。返回按 sim 降序、且 sim ≥ conflict_sim 的冲突列表。
pub(crate) fn detect(root: &str, meta: &Meta, c: &Case) -> io::Result<Vec<Conflict>> {
    let th = meta.param("conflict_sim");
    if th <= 0.0 || c.text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let max = meta.param("conflict_max").max(1.0) as usize;
    let cands = candidates(root, c, max)?;
    if cands.is_empty() {
        return Ok(Vec::new());
    }
    // 逐候选 load（≤ conflict_max 次；同桶优先命中同场景，成本受控）。
    let mut peers = Vec::with_capacity(cands.len());
    let mut texts: Vec<String> = Vec::with_capacity(cands.len());
    for id in &cands {
        let p = store::load(root, id)?;
        texts.push(p.text.clone());
        peers.push(p);
    }
    let sims = crate::memory::minhash::scores(
        &c.text,
        &texts.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let mut out: Vec<Conflict> = peers
        .iter()
        .zip(sims)
        .filter(|(_, s)| *s >= th)
        .map(|(p, s)| Conflict {
            id: p.id.clone(),
            sim: s,
            evidence: p.evidence.len(),
            quality: p.quality,
        })
        .collect();
    out.sort_by(|a, b| {
        b.sim
            .partial_cmp(&a.sim)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(out)
}

/// 候选 id：同桶（主桶相等）优先；不足补全库（截断到 max）。
fn candidates(root: &str, c: &Case, max: usize) -> io::Result<Vec<String>> {
    if let Some(idx) = crate::memory::index::load(root)? {
        // 快照一趟分出同桶与其余，同桶优先取到 max，不足再补其余。
        let mut same = Vec::new();
        let mut rest = Vec::new();
        for (di, rec) in idx.docs.iter().enumerate() {
            let id = idx.ids[di].as_ref();
            if rec.bucket == c.bucket {
                same.push(id);
            } else {
                rest.push(id);
            }
        }
        return Ok(take(&same, &rest, max));
    }
    // 无索引（空库/缓存缺失）：直接按库序截断。
    Ok(take_str(&store::list(root)?, max))
}

/// 按序取候选：primary 优先，不足补 secondary，总长 ≤ max。
fn take<'a>(primary: &[&'a str], secondary: &[&'a str], max: usize) -> Vec<String> {
    primary
        .iter()
        .chain(secondary)
        .take(max)
        .map(|s| s.to_string())
        .collect()
}

/// 库序截断候选（无索引兜底路径）。
fn take_str(ids: &[String], max: usize) -> Vec<String> {
    ids.iter().take(max).cloned().collect()
}

/// 仲裁并双向互链。幂等：已链过的冲突（c.related 含对方 id）跳过，不重复写 log。
/// 返回本轮新增的冲突对数（供调用方观测）。
pub(crate) fn arbitrate(root: &str, c: &mut Case, peers: &[Conflict]) -> io::Result<usize> {
    let mut added = 0usize;
    for p in peers {
        if c.related.contains(&p.id) {
            continue;
        }
        // 裁决：证据多者胜 → quality 高者胜 → 先到先得（已存在 case 胜）。
        let c_wins = match c.evidence.len().cmp(&p.evidence) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => c.quality > p.quality,
        };
        let row = Row::new(c, p, c_wins);
        // 双向互链：库内 case p 的 related 追加 c.id（败者链向胜者 /
        // 胜者链回败者，二者对 p 而言都是记录与 c 的冲突）；c 是新 case，
        // 其 related 由下方内存 push 承担（insert 持锁，不能对 c 再次加锁）。
        attach(root, &c.id, &p.id)?;
        log(root, &row)?;
        c.related.push(p.id.clone());
        added += 1;
    }
    Ok(added)
}

/// 一次仲裁的记录行：胜负双方 id 与裁决依据（证据数/质量/sim）。
struct Row {
    winner: String,
    loser: String,
    w_ev: usize,
    l_ev: usize,
    w_q: f64,
    l_q: f64,
    sim: f64,
}

impl Row {
    fn new(c: &Case, p: &Conflict, c_wins: bool) -> Row {
        let (winner, loser, w_ev, l_ev, w_q, l_q) = if c_wins {
            (
                c.id.clone(),
                p.id.clone(),
                c.evidence.len(),
                p.evidence,
                c.quality,
                p.quality,
            )
        } else {
            (
                p.id.clone(),
                c.id.clone(),
                p.evidence,
                c.evidence.len(),
                p.quality,
                c.quality,
            )
        };
        Row {
            winner,
            loser,
            w_ev,
            l_ev,
            w_q,
            l_q,
            sim: p.sim,
        }
    }
}

/// 给 to（库内 case）追加 from 到 related（幂等；不删除数据，只改 related 字段）。
fn attach(root: &str, from: &str, to: &str) -> io::Result<()> {
    let _lock = store::lock(root, to)?;
    let mut t = store::load(root, to)?;
    if !t.related.contains(&from.to_string()) {
        t.related.push(from.to_string());
        store::save(root, &t)?;
    }
    Ok(())
}

/// append 一行仲裁记录到 conflicts.log（原子追加；TSV 8 列）。
fn log(root: &str, row: &Row) -> io::Result<()> {
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{:.3}\n",
        store::stamp(),
        row.winner,
        row.loser,
        row.w_ev,
        row.l_ev,
        row.w_q,
        row.l_q,
        row.sim,
    );
    crate::memory::lock::atomic(
        &std::path::Path::new(root).join(CONFLICT_LOG),
        line.as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::meta;
    use crate::memory::store::{self, Stats};
    use crate::memory::Input;

    fn tmp(tag: &str) -> String {
        let root = std::env::temp_dir().join(format!("scx-conflict-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root.to_str().unwrap().to_string()
    }

    fn seed(root: &str, query: &str, text: &str) -> String {
        let stats = Stats {
            kept: 1,
            total: 1,
            cut: 0.0,
            risk: "LOW".to_string(),
        };
        crate::memory::save(Input::new(root, query, text, &stats))
            .unwrap()
            .1
    }

    #[test]
    fn detect_flags_near_duplicate_above_threshold() {
        let root = tmp("detect");
        let id = seed(
            &root,
            "db timeout",
            "db timeout conn pool exhausted\nroot cause: pool exhausted\nfix: raise pool size",
        );
        let c = store::load(&root, &id).unwrap();
        meta::ensure(&root).unwrap();
        let meta = meta::load(&root).unwrap();
        // 同事故近重复文本（措辞略异）：sim 应 ≥ conflict_sim(0.8)。
        let mut near = c.clone();
        near.query = "database connection timeout".to_string();
        near.text =
            "db timeout conn pool exhausted\nroot cause: pool exhausted\nfix: raise pool size"
                .to_string();
        let hits = detect(&root, &meta, &near).unwrap();
        assert!(!hits.is_empty(), "近重复文本必须触发冲突检测");
        assert!(hits.iter().any(|h| h.id == id), "命中必须含库内对应 case");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn arbitrate_links_related_and_writes_log() {
        let root = tmp("arb");
        let id_a = seed(&root, "disk full", "disk full errno=28\nfix: clean up tmp");
        meta::ensure(&root).unwrap();
        // 构造新 case（证据更多 → 胜者）。
        let stats = Stats {
            kept: 1,
            total: 1,
            cut: 0.0,
            risk: "LOW".to_string(),
        };
        let (_, id_b) = crate::memory::save(Input {
            revise: None,
            root: &root,
            query: "disk is full",
            text: "disk full errno=28\nfix: clean up tmp",
            source: &[],
            stats: &stats,
            forces: &[],
            fix: "clean up tmp",
            evidence_lno: &[1, 2],
            evidence: &[
                "line 1: disk full errno=28".to_string(),
                "line 2: fix".to_string(),
            ],
            max_bytes: None,
        })
        .unwrap();
        let c_b = store::load(&root, &id_b).unwrap();
        let mut c_b_mut = c_b.clone();
        // save(id_b) 已走 ingest→insert→detect→arbitrate 自动互链。
        assert!(
            c_b_mut.related.contains(&id_a),
            "新 case 必须链向库内近重复 case"
        );
        let c_a2 = store::load(&root, &id_a).unwrap();
        assert!(c_a2.related.contains(&id_b), "库内 case 必须链回新 case");
        // log 落盘。
        let body = std::fs::read_to_string(std::path::Path::new(&root).join(CONFLICT_LOG)).unwrap();
        assert!(
            body.contains(&id_a) && body.contains(&id_b),
            "log 必须含双方 id"
        );
        // 幂等：重复仲裁不新增。
        let peers = vec![Conflict {
            id: id_a.clone(),
            sim: 0.9,
            evidence: 0,
            quality: 0.0,
        }];
        let n = arbitrate(&root, &mut c_b_mut, &peers).unwrap();
        assert_eq!(n, 0, "已链过的冲突不得重复仲裁");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unlinked_old_cases_roundtrip_related() {
        let root = tmp("rt");
        let id = seed(&root, "oom", "oom killer\nfix: reduce memory");
        let c = store::load(&root, &id).unwrap();
        assert!(c.related.is_empty(), "旧 case 无 related 字段必须兼容为空");
        let _ = std::fs::remove_dir_all(&root);
    }
}
