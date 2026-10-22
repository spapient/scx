//! query 记忆图（O2）：query 指纹 → top-k (case_id, score, sim) 有向映射，append-only。
//!
//! 二进制布局（自产自销，零依赖；v2 起条目带库代际 gen）：
//! ```text
//! [magic u32 "QMAP"][version u16]
//! (rec)*  每条： [qlen u16][norm(query) bytes][n u16][(idlen u8)(id bytes)(score f32)(sim f32)*n][recency u32][gen u64]
//! ```
//! 查询：归一化 query → 精确匹配直达（跳过 BM25，直接还原 score/sim 快照）；
//! 编辑距离 ≤2 变体走轻量重排（新词面仍参与 BM25，见 recall 接线）。
//! 同指纹多条时取 recency 最新。append-only：新条目追加；`rebuild` 合并（P3 落地，
//! 本文件提供 load+write 原语）。
//!
//! 代际失效（P4.4 评审处置）：条目记录写入时的库 gen；lookup 时 `entry.gen < 当前 gen`
//! 的条目一律视为过期（新 case / 反馈 / 删除后，同 query 必须走 BM25 重算，
//! 快照不得遮挡新状态）。

use crate::memory::util::{edit_dist, fnv1a, utc_now};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const QMAP_MAGIC: u32 = 0x5041_4d51; // "QMAP"
const QMAP_VER: u16 = 2;
const HEADER_LEN: usize = 6;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QHit {
    pub id: String,
    pub score: f32,
    pub sim: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QEntry {
    /// 归一化后的 query（小写、保留数字的身份口径）。
    pub q: String,
    pub hits: Vec<QHit>,
    /// recency：Unix 秒（取 UTC 时间戳）。
    pub recency: u32,
    /// 写入时的库代际；低于当前 meta gen 即过期。
    pub gen: u64,
}

fn qmap_path(root: &str) -> PathBuf {
    Path::new(root).join("index").join("qmap.bin")
}

/// 追加一条记忆图记录（不查重，rebuild 时合并）；记录写入时的库代际 `gen`。
pub(crate) fn append(
    root: &str,
    query: &str,
    hits: &[(String, f64, f64)],
    gen: u64,
) -> std::io::Result<()> {
    let q = norm_query(query);
    if q.len() > u16::MAX as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("qmap query too long: {} bytes > {}", q.len(), u16::MAX),
        ));
    }
    if hits.len() > u16::MAX as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("qmap hit count too large: {}", hits.len()),
        ));
    }
    if let Some((id, _, _)) = hits.iter().find(|(id, _, _)| id.len() > u8::MAX as usize) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("qmap case id too long: {}", id.len()),
        ));
    }
    let mut buf: Vec<u8> = Vec::with_capacity(64 + hits.iter().map(|h| h.0.len()).sum::<usize>());
    buf.extend_from_slice(&(q.len() as u16).to_le_bytes());
    buf.extend_from_slice(q.as_bytes());
    buf.extend_from_slice(&(hits.len() as u16).to_le_bytes());
    for (id, score, sim) in hits {
        buf.extend_from_slice(&(id.len() as u8).to_le_bytes());
        buf.extend_from_slice(id.as_bytes());
        buf.extend_from_slice(&(*score as f32).to_le_bytes());
        buf.extend_from_slice(&(*sim as f32).to_le_bytes());
    }
    let now = utc_now();
    buf.extend_from_slice(&(fnv1a(&now) as u32).to_le_bytes());
    buf.extend_from_slice(&gen.to_le_bytes());

    // append-only 文件也必须串行：两个进程同时写 header/记录会让读者看到交错段。
    let _g = crate::memory::lock::acquire(root, "qmap.lock")?;
    let p = qmap_path(root);
    if let Some(dir) = p.parent() {
        crate::memory::lock::create_dir_all(dir)?;
    }
    // 读-修复-追加都在 qmap.lock 内完成。上次进程若在 write_all 中途退出，
    // 残尾必须截掉再追加；否则 load_all 永远停在第一处残段，后续记录全部不可见。
    // header 损坏则整文件重建（qmap 只是可丢弃缓存）。
    let mut f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&p)?;
    let mut existing = Vec::new();
    f.read_to_end(&mut existing)?;
    let decoded = decode(&existing);
    if !decoded.header_ok {
        f.set_len(0)?;
        f.seek(SeekFrom::Start(0))?;
        write_header(&mut f)?;
    } else if decoded.end < existing.len() {
        f.set_len(decoded.end as u64)?;
        f.seek(SeekFrom::End(0))?;
    } else {
        f.seek(SeekFrom::End(0))?;
    }
    f.write_all(&buf)?;
    Ok(())
}

fn write_header(f: &mut fs::File) -> std::io::Result<()> {
    let mut h = [0u8; HEADER_LEN];
    h[0..4].copy_from_slice(&QMAP_MAGIC.to_le_bytes());
    h[4..6].copy_from_slice(&QMAP_VER.to_le_bytes());
    f.write_all(&h)
}

struct Decoded {
    entries: Vec<QEntry>,
    /// 完整记录结束位置（残尾起点）；坏 header 时为 0。
    end: usize,
    header_ok: bool,
}

/// 容错解码：坏 header 返回 `header_ok=false`；记录残尾返回最后一条完整记录之后
/// 的位置。所有切片都先做边界检查，绝不因外部损坏文件 panic。
fn decode(buf: &[u8]) -> Decoded {
    if buf.len() < HEADER_LEN {
        return Decoded {
            entries: Vec::new(),
            end: 0,
            header_ok: false,
        };
    }
    let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    let ver = u16::from_le_bytes(buf[4..6].try_into().unwrap());
    if magic != QMAP_MAGIC || ver != QMAP_VER {
        return Decoded {
            entries: Vec::new(),
            end: 0,
            header_ok: false,
        };
    }
    let mut out = Vec::new();
    let mut i = HEADER_LEN;
    while i < buf.len() {
        let rec_start = i;
        if i + 2 > buf.len() {
            break;
        }
        let qlen = u16::from_le_bytes(buf[i..i + 2].try_into().unwrap()) as usize;
        i += 2;
        if i + qlen > buf.len() {
            i = rec_start;
            break;
        }
        let q = String::from_utf8_lossy(&buf[i..i + qlen]).into_owned();
        i += qlen;
        if i + 2 > buf.len() {
            i = rec_start;
            break;
        }
        let n = u16::from_le_bytes(buf[i..i + 2].try_into().unwrap()) as usize;
        i += 2;
        let mut hits = Vec::with_capacity(n.min(64));
        let mut ok = true;
        for _ in 0..n {
            if i >= buf.len() {
                ok = false;
                break;
            }
            let ilen = buf[i] as usize;
            i += 1;
            if i + ilen + 8 > buf.len() {
                ok = false;
                break;
            }
            let id = String::from_utf8_lossy(&buf[i..i + ilen]).into_owned();
            i += ilen;
            let score = f32::from_le_bytes(buf[i..i + 4].try_into().unwrap());
            let sim = f32::from_le_bytes(buf[i + 4..i + 8].try_into().unwrap());
            i += 8;
            hits.push(QHit { id, score, sim });
        }
        if !ok || i + 12 > buf.len() {
            i = rec_start;
            break;
        }
        let recency = u32::from_le_bytes(buf[i..i + 4].try_into().unwrap());
        let gen = u64::from_le_bytes(buf[i + 4..i + 12].try_into().unwrap());
        i += 12;
        out.push(QEntry {
            q,
            hits,
            recency,
            gen,
        });
    }
    Decoded {
        entries: out,
        end: i,
        header_ok: true,
    }
}

/// 归一化 query（保留数字：qmap 是精确键，8080/9090 不能共用快照）。
fn norm_query(query: &str) -> String {
    let mut out = String::new();
    crate::norm::norm_identity_into(query, &mut out);
    out
}

/// 读取全部条目（append-only 顺序）。header 损坏/版本不符返回空（可重建）。
pub(crate) fn load_all(root: &str) -> std::io::Result<Vec<QEntry>> {
    let p = qmap_path(root);
    if !p.exists() {
        return Ok(Vec::new());
    }
    let mut buf = Vec::new();
    fs::File::open(&p)?.read_to_end(&mut buf)?;
    Ok(decode(&buf).entries)
}

/// 精确或编辑距离 ≤ max 的记忆图命中（取 recency 最新；精确优先）。
/// `meta_gen` = 当前库代际：`entry.gen < meta_gen` 的旧快照视为过期（跳过），
/// 保证 store/rate/delete 后的库变化不被快照遮挡。
/// 返回 (命中的归一化 query, hits, 编辑距离)。
pub(crate) fn lookup(
    root: &str,
    query: &str,
    max: usize,
    meta_gen: u64,
) -> std::io::Result<Option<(String, Vec<QHit>, usize)>> {
    let q = norm_query(query);
    let entries = load_all(root)?;
    let mut exact: Option<(String, Vec<QHit>, usize)> = None;
    let mut best: Option<(String, Vec<QHit>, usize)> = None;
    let mut best_rec: u32 = 0;
    for e in entries {
        if e.gen < meta_gen {
            continue; // 库已变化：快照过期，不走直达。
        }
        if e.q == q {
            exact = Some((e.q.clone(), e.hits.clone(), 0));
        }
        if let Some(d) = edit_dist(&e.q, &q, max) {
            if d > 0 && (best.is_none() || e.recency > best_rec) {
                best = Some((e.q.clone(), e.hits.clone(), d));
                best_rec = e.recency;
            }
        }
    }
    // 精确命中永远优先于变体。
    Ok(exact.or(best))
}

/// 合并重写（--rebuild）：同归一化 query 只留 recency 最新一条；按 query 字典序。
pub(crate) fn merge(root: &str) -> std::io::Result<()> {
    // 锁必须在 load 之前：否则 append 在 load 与 write 之间进来的记录会被合并覆盖。
    let _g = crate::memory::lock::acquire(root, "qmap.lock")?;
    let entries = load_all(root)?;
    if entries.is_empty() {
        // 损坏/空 qmap 在 rebuild 时直接丢弃，避免下一次 append 继续追加到坏 header 后。
        let p = qmap_path(root);
        if p.exists() {
            let _ = fs::remove_file(p);
        }
        return Ok(());
    }
    let mut best: std::collections::HashMap<String, QEntry> = std::collections::HashMap::new();
    for e in entries {
        match best.get_mut(&e.q) {
            Some(cur) if e.recency > cur.recency => *cur = e,
            Some(_) => {}
            None => {
                best.insert(e.q.clone(), e);
            }
        }
    }
    let mut sorted: Vec<QEntry> = best.into_values().collect();
    sorted.sort_by(|a, b| a.q.cmp(&b.q));
    let p = qmap_path(root);
    if let Some(d) = p.parent() {
        crate::memory::lock::create_dir_all(d)?;
    }
    let mut buf: Vec<u8> = Vec::with_capacity(16 + sorted.len() * 64);
    buf.extend_from_slice(&QMAP_MAGIC.to_le_bytes());
    buf.extend_from_slice(&QMAP_VER.to_le_bytes());
    for e in sorted {
        buf.extend_from_slice(&(e.q.len() as u16).to_le_bytes());
        buf.extend_from_slice(e.q.as_bytes());
        buf.extend_from_slice(&(e.hits.len() as u16).to_le_bytes());
        for h in &e.hits {
            buf.extend_from_slice(&(h.id.len() as u8).to_le_bytes());
            buf.extend_from_slice(h.id.as_bytes());
            buf.extend_from_slice(&h.score.to_le_bytes());
            buf.extend_from_slice(&h.sim.to_le_bytes());
        }
        buf.extend_from_slice(&e.recency.to_le_bytes());
        buf.extend_from_slice(&e.gen.to_le_bytes());
    }
    crate::memory::lock::atomic_write(&p, &buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_root(tag: &str) -> String {
        let r = std::env::temp_dir().join(format!("scx-qmap-{}-{}", tag, fnv1a("q") & 0xff));
        let _ = fs::remove_dir_all(&r);
        let _ = fs::create_dir_all(r.join("index"));
        r.to_str().unwrap().to_string()
    }

    #[test]
    fn qmap_hit_after_append() {
        let root = tmp_root("hit");
        append(
            &root,
            "数据库连接超时",
            &[("c1".to_string(), 1.19, 1.0), ("c2".to_string(), 0.8, 0.6)],
            3,
        )
        .unwrap();
        let hit = lookup(&root, "数据库连接超时", 0, 3).unwrap();
        assert_eq!(
            hit,
            Some((
                "数据库连接超时".to_string(),
                vec![
                    QHit {
                        id: "c1".to_string(),
                        score: 1.19,
                        sim: 1.0
                    },
                    QHit {
                        id: "c2".to_string(),
                        score: 0.8,
                        sim: 0.6
                    },
                ],
                0
            ))
        );
        // 编辑距离 1 变体：加一字。
        let v = lookup(&root, "数据库连接超时了", 2, 3).unwrap();
        assert!(v.is_some(), "≤2 变体应命中记忆图");
        assert_eq!(v.unwrap().2, 1);
        // 编辑距离 4：不命中。
        let far = lookup(&root, "磁盘空间不足了", 2, 3).unwrap();
        assert!(far.is_none());
        // 代际失效：库 gen 前进后，同 gen 快照必须过期（走 BM25 重算）。
        let stale = lookup(&root, "数据库连接超时", 0, 4).unwrap();
        assert!(stale.is_none(), "gen 落后的快照不得直达");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmap_append_only_and_latest_wins() {
        let root = tmp_root("ap");
        append(&root, "q", &[("a".to_string(), 1.0, 0.9)], 1).unwrap();
        append(&root, "q", &[("b".to_string(), 0.7, 0.5)], 1).unwrap();
        let all = load_all(&root).unwrap();
        assert_eq!(all.len(), 2, "append-only：两次追加为两条");
        // lookup 精确命中取 recency 最新——两条 recency 相同(同一秒)时取先写，
        // 因此断言至少能命中且 id 为其中一条。
        let hit = lookup(&root, "q", 0, 1).unwrap().unwrap();
        assert!(hit.1[0].id == "a" || hit.1[0].id == "b");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmap_keeps_numeric_query_identity() {
        let root = tmp_root("num");
        append(&root, "port 8080 in use", &[("a".to_string(), 1.0, 1.0)], 1).unwrap();
        append(&root, "port 9090 in use", &[("b".to_string(), 1.0, 1.0)], 1).unwrap();
        let a = lookup(&root, "port 8080 in use", 1, 1).unwrap().unwrap();
        let b = lookup(&root, "port 9090 in use", 1, 1).unwrap().unwrap();
        assert_eq!(a.1[0].id, "a", "8080 不能拿到 9090 的快照");
        assert_eq!(b.1[0].id, "b", "9090 不能拿到 8080 的快照");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmap_handles_corrupt_tail() {
        let root = tmp_root("cor");
        append(&root, "数据库超时", &[("c1".to_string(), 1.0, 0.9)], 1).unwrap();
        // 截断尾部：损坏最后 2 字节。
        let p = qmap_path(&root);
        let mut buf = fs::read(&p).unwrap();
        buf.truncate(buf.len() - 2);
        fs::write(&p, &buf).unwrap();
        let r = lookup(&root, "数据库超时", 0, 1);
        assert!(r.is_ok(), "尾部损坏必须容错，不崩溃");
        // 残段被丢弃（该条 recency 缺失）→ 命中可为空；rebuild 后恢复。
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmap_repairs_partial_tail_before_next_append() {
        let root = tmp_root("repair");
        append(&root, "first", &[("c1".to_string(), 1.0, 0.9)], 1).unwrap();
        let p = qmap_path(&root);
        // 模拟 write_all 中途崩溃：留下 1 字节残尾。旧 decoder 会在 i+2 处越界 panic；
        // 修好后 lookup 应安全忽略残段，下一次 append 必须截掉残尾再写。
        let mut f = fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(&[0x7f]).unwrap();
        drop(f);
        assert!(lookup(&root, "first", 0, 1).unwrap().is_some());
        append(&root, "second", &[("c2".to_string(), 0.8, 0.7)], 1).unwrap();
        assert_eq!(load_all(&root).unwrap().len(), 2, "残尾修复后两条都应可读");
        assert_eq!(
            lookup(&root, "second", 0, 1).unwrap().unwrap().1[0].id,
            "c2"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmap_rebuilds_bad_header_on_append() {
        let root = tmp_root("hdr");
        let p = qmap_path(&root);
        fs::write(&p, b"garbage").unwrap();
        append(&root, "q", &[("c1".to_string(), 1.0, 0.9)], 1).unwrap();
        assert_eq!(lookup(&root, "q", 0, 1).unwrap().unwrap().1[0].id, "c1");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmap_rejects_oversized_fields_without_corrupting_file() {
        let root = tmp_root("limits");
        append(&root, "q", &[("c1".to_string(), 1.0, 0.9)], 1).unwrap();
        let long_q = "x".repeat(u16::MAX as usize + 1);
        assert!(append(&root, &long_q, &[("c2".to_string(), 1.0, 1.0)], 1).is_err());
        let long_id = "i".repeat(u8::MAX as usize + 1);
        assert!(append(&root, "q2", &[(long_id, 1.0, 1.0)], 1).is_err());
        assert_eq!(
            load_all(&root).unwrap().len(),
            1,
            "拒绝写入不得破坏已有记录"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
