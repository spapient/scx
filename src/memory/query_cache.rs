//! query 缓存：进程内会话快照 + 持久化 query 记忆图。
//!
//! 二进制布局（自产自销，零依赖；v2 起条目带库代际 gen，v3 起 recency 为 Unix 秒）：
//! ```text
//! [magic u32 "QMAP"][version u16]
//! (rec)*  每条： [qlen u16][norm(query) bytes][n u16][(idlen u8)(id bytes)(score f32)(sim f32)*n][recency u32][gen u64]
//! ```
//! 查询：归一化 query → 精确匹配直达（跳过 BM25，直接还原 score/sim 快照）。
//! 变体 query 不复用其它 query 的快照：缓存只保存 top-k，无法在不重扫全库的
//! 前提下保证变体排序正确，因此交给 BM25 重算。快照条数不足请求的 k 时也回退
//! BM25，避免小 k 查询截断后续大 k 查询。append-only：新条目追加；`--rebuild`
//! 时 `merge` 合并（同 query 取最新写入，同一秒内按文件顺序后者优先）。
//! 快照只在 1 小时内复用：时间衰减会随当前时钟缓慢改变排序，过期后重算。
//!
//! 代际失效：条目记录写入时的库 gen；lookup 时 `entry.gen < 当前 gen`
//! 的条目一律视为过期（新 case / 反馈 / 删除后，同 query 必须走 BM25 重算，
//! 快照不得遮挡新状态）。

use crate::memory::util::{fnv1a_parts, utc_now_secs};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const QMAP_MAGIC: u32 = 0x5041_4d51; // "QMAP"
const QMAP_VER: u16 = 3;
const HEADER_LEN: usize = 6;
/// 时间衰减的最短 τ 为 30 天；1 小时快照造成的相对分数漂移上限约 0.14%
/// （1h / (30d + 1h)），同时覆盖同一排障会话内的重复 CLI 查询。
const QMAP_TTL_SECS: u64 = 60 * 60;

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

pub(crate) fn qmap_path(root: &str) -> PathBuf {
    Path::new(root).join("index").join("qmap.bin")
}

/// 校验 append 输入边界（query/命中数/case id 长度）。
fn validate_append_inputs(q: &str, hits: &[(String, f64, f64)]) -> Result<(), std::io::Error> {
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
    Ok(())
}

/// 构建一条待追加的记录字节（不含 header）。
fn build_entry_buf(q: &str, hits: &[(String, f64, f64)], recency: u32, gen: u64) -> Vec<u8> {
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
    buf.extend_from_slice(&recency.to_le_bytes());
    buf.extend_from_slice(&gen.to_le_bytes());
    buf
}

/// 修复追加前的文件状态：坏 header 重建、残尾截掉、正常则定位到末尾。
fn repair_append_file(f: &mut fs::File, existing: &[u8], decoded: &Decoded) -> std::io::Result<()> {
    if !decoded.header_ok {
        rebuild_header(f)
    } else if decoded.end < existing.len() {
        truncate_to_end(f, decoded.end)
    } else {
        seek_end(f)
    }
}

fn rebuild_header(f: &mut fs::File) -> std::io::Result<()> {
    f.set_len(0)?;
    f.seek(SeekFrom::Start(0))?;
    write_header(f)
}

fn truncate_to_end(f: &mut fs::File, end: usize) -> std::io::Result<()> {
    f.set_len(end as u64)?;
    f.seek(SeekFrom::End(0))?;
    Ok(())
}

fn seek_end(f: &mut fs::File) -> std::io::Result<()> {
    f.seek(SeekFrom::End(0))?;
    Ok(())
}

/// 单条已解码记录是否与本次写入同 query/同 gen/新鲜且快照已覆盖。
fn matches_snapshot(e: &QEntry, q: &str, gen: u64, now: u64, hits: &[(String, f64, f64)]) -> bool {
    e.q == q && e.gen == gen && is_fresh(e.recency, now) && snapshot_covers(&e.hits, hits)
}

/// 是否已存在可复用的同 query/同 gen 快照（无需追加）。
fn already_cached(
    decoded: &Decoded,
    q: &str,
    gen: u64,
    now: u64,
    hits: &[(String, f64, f64)],
) -> bool {
    decoded.header_ok
        && decoded
            .entries
            .iter()
            .rev()
            .any(|e| matches_snapshot(e, q, gen, now, hits))
}

/// 追加一条记忆图记录（不查重，rebuild 时合并）；记录写入时的库代际 `gen`。
pub(crate) fn append(
    root: &str,
    query: &str,
    hits: &[(String, f64, f64)],
    gen: u64,
) -> std::io::Result<()> {
    let q = norm_query(query);
    validate_append_inputs(&q, hits)?;
    let recency = u32::try_from(utc_now_secs()).unwrap_or(u32::MAX);
    let buf = build_entry_buf(&q, hits, recency, gen);

    // append-only 文件也必须串行：两个进程同时写 header/记录会让读者看到交错段。
    let _g = crate::memory::lock::acquire(root, "qmap.lock")?;
    let p = qmap_path(root);
    // 读-修复-追加都在 qmap.lock 内完成。上次进程若在 write_all 中途退出，
    // 残尾必须截掉再追加；否则 load_all 永远停在第一处残段，后续记录全部不可见。
    // header 损坏则整文件重建（qmap 只是可丢弃缓存）。
    ensure_parent(&p)?;
    let (mut f, decoded) = prepare_qmap(&p)?;
    // 同一 query/代际已有覆盖当前结果（相同或更大 top-k 且前缀一致）时无需追加：
    // 重复 CLI 召回不会让 append-only 文件无限膨胀。分数不同的重算仍会追加，
    // 因而参数或库状态变化不会被跳过。
    let now = recency as u64;
    if already_cached(&decoded, &q, gen, now, hits) {
        return Ok(());
    }
    f.write_all(&buf)?;
    Ok(())
}

fn ensure_parent(p: &std::path::Path) -> std::io::Result<()> {
    if let Some(dir) = p.parent() {
        crate::memory::lock::create_dir_all(dir)?;
    }
    Ok(())
}

fn prepare_qmap(p: &std::path::Path) -> std::io::Result<(fs::File, Decoded)> {
    let mut f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(p)?;
    let mut existing = Vec::new();
    f.read_to_end(&mut existing)?;
    let decoded = decode(&existing);
    repair_append_file(&mut f, &existing, &decoded)?;
    Ok((f, decoded))
}

fn snapshot_covers(existing: &[QHit], incoming: &[(String, f64, f64)]) -> bool {
    existing.len() >= incoming.len()
        && existing
            .iter()
            .zip(incoming)
            .all(|(old, (id, score, sim))| {
                old.id == *id && old.score == *score as f32 && old.sim == *sim as f32
            })
}

fn is_fresh(recency: u32, now: u64) -> bool {
    now.saturating_sub(recency as u64) <= QMAP_TTL_SECS
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
    if !header_ok(buf) {
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
        if let Some(entry) = parse_entry(buf, &mut i) {
            out.push(entry);
        } else {
            i = rec_start;
            break;
        }
    }
    Decoded {
        entries: out,
        end: i,
        header_ok: true,
    }
}

fn header_ok(buf: &[u8]) -> bool {
    if buf.len() < HEADER_LEN {
        return false;
    }
    let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    let ver = u16::from_le_bytes(buf[4..6].try_into().unwrap());
    magic == QMAP_MAGIC && ver == QMAP_VER
}

fn parse_entry(buf: &[u8], i: &mut usize) -> Option<QEntry> {
    if *i + 2 > buf.len() {
        return None;
    }
    let qlen = u16::from_le_bytes(buf[*i..*i + 2].try_into().unwrap()) as usize;
    *i += 2;
    if *i + qlen > buf.len() {
        return None;
    }
    let q = String::from_utf8_lossy(&buf[*i..*i + qlen]).into_owned();
    *i += qlen;
    if *i + 2 > buf.len() {
        return None;
    }
    let n = u16::from_le_bytes(buf[*i..*i + 2].try_into().unwrap()) as usize;
    *i += 2;
    let mut hits = Vec::with_capacity(n.min(64));
    for _ in 0..n {
        if *i >= buf.len() {
            return None;
        }
        let ilen = buf[*i] as usize;
        *i += 1;
        if *i + ilen + 8 > buf.len() {
            return None;
        }
        let id = String::from_utf8_lossy(&buf[*i..*i + ilen]).into_owned();
        *i += ilen;
        let score = f32::from_le_bytes(buf[*i..*i + 4].try_into().unwrap());
        let sim = f32::from_le_bytes(buf[*i + 4..*i + 8].try_into().unwrap());
        *i += 8;
        hits.push(QHit { id, score, sim });
    }
    if *i + 12 > buf.len() {
        return None;
    }
    let recency = u32::from_le_bytes(buf[*i..*i + 4].try_into().unwrap());
    let gen = u64::from_le_bytes(buf[*i + 4..*i + 12].try_into().unwrap());
    *i += 12;
    Some(QEntry {
        q,
        hits,
        recency,
        gen,
    })
}

/// 归一化 query（保留数字：qmap 是精确键，8080/9090 不能共用快照）。
fn norm_query(query: &str) -> String {
    let mut out = String::new();
    crate::analysis::norm_identity_into(query, &mut out);
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

/// 精确记忆图命中（取最新的可用快照；append-only 顺序即写入顺序）。
/// `meta_gen` = 当前库代际：`entry.gen < meta_gen` 的旧快照视为过期（跳过），
/// 保证 store/rate/delete 后的库变化不被快照遮挡。
///
/// `k` = 本次请求的结果上限。快照只保存当时 `truncate(k)` 后的 top-k，因此
/// 只有 `hits.len() >= k` 才能复用：否则无法证明快照包含本次需要的全部结果。
/// 例如先用 `--recall 1` 暖缓存、再用 `--recall 5`，必须回到 BM25 重算，
/// 不能把 1 条结果当成"库里只有 1 条"。反向扫描也允许命中更早写入的更大快照。
/// 返回 (命中的归一化 query, hits)。
pub(crate) fn lookup(
    root: &str,
    query: &str,
    k: usize,
    meta_gen: u64,
) -> std::io::Result<Option<(String, Vec<QHit>)>> {
    let q = norm_query(query);
    let entries = load_all(root)?;
    let now = utc_now_secs();
    for e in entries.into_iter().rev() {
        if e.gen >= meta_gen && e.q == q && is_fresh(e.recency, now) && e.hits.len() >= k {
            let mut hits = e.hits;
            hits.truncate(k);
            return Ok(Some((e.q, hits)));
        }
    }
    Ok(None)
}

/// 同归一化 query 去重：保留最新写入；同一秒内后写者（文件顺序）覆盖先写者。
fn dedup_entries(entries: Vec<QEntry>) -> std::collections::HashMap<String, QEntry> {
    let mut best: std::collections::HashMap<String, QEntry> = std::collections::HashMap::new();
    for e in entries {
        match best.get_mut(&e.q) {
            // 同一秒内 recency 相等时，文件顺序更精确：后写者在后，必须覆盖先写者，
            // 与 lookup 的"取最新可用快照"语义保持一致。
            Some(cur) if e.recency >= cur.recency => *cur = e,
            Some(_) => {}
            None => {
                best.insert(e.q.clone(), e);
            }
        }
    }
    best
}

/// 序列化合并后的 qmap（header + 按 query 字典序的条目）。
fn serialize_qmap(sorted: &[QEntry]) -> Vec<u8> {
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
    buf
}

/// 合并重写（--rebuild）：同归一化 query 只留最新一条（同秒按写入顺序取后者）；
/// 按 query 字典序写回。
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
    let best = dedup_entries(entries);
    let mut sorted: Vec<QEntry> = best.into_values().collect();
    sorted.sort_by(|a, b| a.q.cmp(&b.q));
    let p = qmap_path(root);
    if let Some(d) = p.parent() {
        crate::memory::lock::create_dir_all(d)?;
    }
    let buf = serialize_qmap(&sorted);
    crate::memory::lock::atomic_write(&p, &buf)
}

/// 进程内会话缓存容量：同会话连续追问只需保留最近少量 query。
const SESSION_CAP: usize = 64;

#[derive(Debug, Clone, PartialEq)]
struct SessionHit {
    /// id + score + sim；L1 过滤可直接复用完整快照。
    hits: Vec<(String, f64, f64)>,
    born: u64,
    gen: u64,
}

struct SessionCache {
    map: HashMap<u64, SessionHit>,
    /// 插入顺序队列（命中时移到队尾）。
    order: Vec<u64>,
}

impl SessionCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: Vec::new(),
        }
    }

    fn touch(&mut self, key: u64) {
        if let Some(pos) = self.order.iter().position(|k| *k == key) {
            self.order.remove(pos);
        }
        self.order.push(key);
        while self.order.len() > SESSION_CAP {
            let old = self.order.remove(0);
            self.map.remove(&old);
        }
    }

    fn remove(&mut self, key: u64) {
        self.map.remove(&key);
        if let Some(pos) = self.order.iter().position(|k| *k == key) {
            self.order.remove(pos);
        }
    }

    fn get(&mut self, key: u64, now: u64, gen: u64, window_secs: u64) -> Option<SessionHit> {
        if window_secs == 0 {
            return None;
        }
        let hit = self.map.get(&key)?;
        if hit.gen != gen || now.saturating_sub(hit.born) > window_secs {
            self.remove(key);
            return None;
        }
        let hit = hit.clone();
        self.touch(key);
        Some(hit)
    }

    fn put(&mut self, key: u64, hit: SessionHit) {
        // 同代际下保留更大的 top-k：一次小 k 查询不能覆盖此前更大的快照，
        // 否则同进程后续更大 k 的查询会被不必要的缓存 miss 拖慢。
        if let Some(old) = self.map.get_mut(&key) {
            if old.gen == hit.gen && old.hits.len() >= hit.hits.len() {
                old.born = hit.born;
                self.touch(key);
                return;
            }
        }
        self.map.insert(key, hit);
        self.touch(key);
    }
}

fn session_cache() -> &'static Mutex<SessionCache> {
    static CACHE: OnceLock<Mutex<SessionCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(SessionCache::new()))
}

/// 窗口内且库代际一致的会话快照；窗口 0 关闭缓存。
///
/// 与 qmap 相同，只有快照条数达到 `k` 才可复用；不足 `k` 时返回 `None`
/// 交给完整召回，避免小 k 查询污染后续大 k 查询。
pub(crate) fn session_get(
    root: &str,
    query: &str,
    gen: u64,
    window_secs: u64,
    k: usize,
) -> Option<Vec<(String, f64, f64)>> {
    let key = session_key(root, query);
    let hit = session_cache()
        .lock()
        .ok()
        .and_then(|mut cache| cache.get(key, utc_now_secs(), gen, window_secs))?;
    if hit.hits.len() < k {
        return None;
    }
    let mut hits = hit.hits;
    hits.truncate(k);
    Some(hits)
}

/// 写入会话快照；失败静默（缓存只是加速，不影响召回正确性）。
pub(crate) fn session_put(root: &str, query: &str, hits: Vec<(String, f64, f64)>, gen: u64) {
    let key = session_key(root, query);
    if let Ok(mut cache) = session_cache().lock() {
        cache.put(
            key,
            SessionHit {
                hits,
                born: utc_now_secs(),
                gen,
            },
        );
    }
}

/// 会话键必须包含库根：同进程可服务多个 `--mem-dir`，query+gen 相同也不能串库。
fn session_key(root: &str, query: &str) -> u64 {
    let query = norm_query(query);
    fnv1a_parts(&[root.as_bytes(), b"\0", query.as_bytes()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::util::tmp_root;
    use std::fs;

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
        let entry = load_all(&root).unwrap().pop().unwrap();
        assert!(
            entry.recency.abs_diff(utc_now_secs() as u32) <= 2,
            "recency 必须是 Unix 秒，而不是时间字符串的哈希"
        );
        let hit = lookup(&root, "数据库连接超时", 2, 3).unwrap();
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
                ]
            ))
        );
        // 变体不复用快照：缓存只有 top-k，交给 BM25 重算才不会被旧排序误导。
        assert!(lookup(&root, "数据库连接超时了", 2, 3).unwrap().is_none());
        // 代际失效：库 gen 前进后，同 gen 快照必须过期（走 BM25 重算）。
        let stale = lookup(&root, "数据库连接超时", 2, 4).unwrap();
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
        // append-only 顺序即写入顺序；精确命中取最后一条（最新写入）。
        let hit = lookup(&root, "q", 1, 1).unwrap().unwrap();
        assert_eq!(hit.1[0].id, "b");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmap_never_serves_fewer_hits_than_requested() {
        let root = tmp_root("k");
        // 先用小 k 写入 1 条，再写 2 条；大 k 查询必须回到更大的快照，
        // 不能让最新但更小的快照把结果截断。
        append(&root, "q", &[("a".to_string(), 1.0, 0.9)], 7).unwrap();
        assert!(lookup(&root, "q", 2, 7).unwrap().is_none());
        append(
            &root,
            "q",
            &[("a".to_string(), 1.0, 0.9), ("b".to_string(), 0.8, 0.7)],
            7,
        )
        .unwrap();
        let two = lookup(&root, "q", 2, 7).unwrap().unwrap().1;
        assert_eq!(two.len(), 2);
        // 再写一条更小的快照：反向扫描必须越过它，命中此前更大的快照。
        append(&root, "q", &[("c".to_string(), 0.9, 0.8)], 7).unwrap();
        let still_two = lookup(&root, "q", 2, 7).unwrap().unwrap().1;
        assert_eq!(still_two.len(), 2);
        assert!(lookup(&root, "q", 3, 7).unwrap().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmap_skips_redundant_snapshots_without_hiding_recomputed_scores() {
        let root = tmp_root("dedup");
        let hits = |id: &str, score: f64| vec![(id.to_string(), score, score / 2.0)];
        append(&root, "q", &hits("a", 1.0), 7).unwrap();
        append(&root, "q", &hits("a", 1.0), 7).unwrap();
        assert_eq!(load_all(&root).unwrap().len(), 1, "相同快照不应重复追加");

        append(&root, "q", &hits("a", 0.9), 7).unwrap();
        assert_eq!(load_all(&root).unwrap().len(), 2, "重算分数必须保留新快照");

        append(
            &root,
            "q",
            &[("a".to_string(), 1.0, 0.5), ("b".to_string(), 0.8, 0.4)],
            7,
        )
        .unwrap();
        append(&root, "q", &hits("a", 1.0), 7).unwrap();
        assert_eq!(
            load_all(&root).unwrap().len(),
            3,
            "较小 k 快照已被更大的同前缀快照覆盖，不应追加"
        );
        assert_eq!(
            lookup(&root, "q", 2, 7).unwrap().unwrap().1.len(),
            2,
            "复用大 k 结果时仍必须得到完整两条"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmap_expires_stale_time_decay_snapshots() {
        let root = tmp_root("ttl");
        let hits = vec![("a".to_string(), 1.0, 0.5)];
        append(&root, "q", &hits, 7).unwrap();
        let p = qmap_path(&root);
        let mut f = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&p)
            .unwrap();
        let len = f.metadata().unwrap().len();
        f.seek(SeekFrom::Start(len - 12)).unwrap();
        let stale = (utc_now_secs() - QMAP_TTL_SECS - 1) as u32;
        f.write_all(&stale.to_le_bytes()).unwrap();
        drop(f);

        assert!(
            lookup(&root, "q", 1, 7).unwrap().is_none(),
            "过期快照必须重算"
        );
        append(&root, "q", &hits, 7).unwrap();
        assert!(
            lookup(&root, "q", 1, 7).unwrap().is_some(),
            "新快照必须可复用"
        );
        assert_eq!(load_all(&root).unwrap().len(), 2, "过期快照不得阻止刷新");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmap_merge_prefers_later_write_on_equal_recency() {
        let root = tmp_root("merge");
        let p = qmap_path(&root);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut buf = Vec::new();
        buf.extend_from_slice(&QMAP_MAGIC.to_le_bytes());
        buf.extend_from_slice(&QMAP_VER.to_le_bytes());
        let now = utc_now_secs();
        for (id, score) in [("older", 0.1f32), ("newer", 0.9f32)] {
            buf.extend_from_slice(&1u16.to_le_bytes());
            buf.push(b'q');
            buf.extend_from_slice(&1u16.to_le_bytes());
            buf.push(id.len() as u8);
            buf.extend_from_slice(id.as_bytes());
            buf.extend_from_slice(&score.to_le_bytes());
            buf.extend_from_slice(&score.to_le_bytes());
            buf.extend_from_slice(&(now as u32).to_le_bytes());
            buf.extend_from_slice(&1u64.to_le_bytes());
        }
        fs::write(&p, buf).unwrap();
        merge(&root).unwrap();
        let hit = lookup(&root, "q", 1, 1).unwrap().unwrap();
        assert_eq!(hit.1[0].id, "newer", "同一秒内后写者必须胜出");
        assert_eq!(load_all(&root).unwrap().len(), 1, "merge 后只剩一条");
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
        let r = lookup(&root, "数据库超时", 1, 1);
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
        assert!(lookup(&root, "first", 1, 1).unwrap().is_some());
        append(&root, "second", &[("c2".to_string(), 0.8, 0.7)], 1).unwrap();
        assert_eq!(load_all(&root).unwrap().len(), 2, "残尾修复后两条都应可读");
        assert_eq!(
            lookup(&root, "second", 1, 1).unwrap().unwrap().1[0].id,
            "c2"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qmap_rebuilds_bad_header_on_append() {
        let root = tmp_root("hdr");
        let p = qmap_path(&root);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"garbage").unwrap();
        append(&root, "q", &[("c1".to_string(), 1.0, 0.9)], 1).unwrap();
        assert_eq!(lookup(&root, "q", 1, 1).unwrap().unwrap().1[0].id, "c1");
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

    #[test]
    fn session_cache_hit_within_window() {
        assert_eq!(
            session_get("/test/a", "缓存命中测试 query alpha", 1, 30 * 60, 2),
            None,
            "首次必 miss"
        );
        let hits = vec![("c1".to_string(), 1.5, 0.9), ("c2".to_string(), 1.2, 0.8)];
        session_put("/test/a", "缓存命中测试 query alpha", hits.clone(), 1);
        assert_eq!(
            session_get("/test/a", "缓存命中测试 query alpha", 1, 30 * 60, 2),
            Some(hits),
            "窗口内二次命中（快照逐字段一致）"
        );
        assert_eq!(
            session_get("/test/a", "缓存命中测试 query beta", 1, 30 * 60, 2),
            None
        );
        assert_eq!(
            session_get("/test/b", "缓存命中测试 query alpha", 1, 30 * 60, 2),
            None,
            "不同记忆库不得共享会话快照"
        );
        assert_eq!(
            session_get("/test/a", "缓存命中测试 query alpha", 1, 30 * 60, 3),
            None,
            "快照条数不足 k 时必须 miss"
        );
        // 小 k 查询不能覆盖已经缓存的更大快照。
        session_put(
            "/test/a",
            "缓存命中测试 query alpha",
            vec![("c1".to_string(), 1.5, 0.9)],
            1,
        );
        assert_eq!(
            session_get("/test/a", "缓存命中测试 query alpha", 1, 30 * 60, 2).map(|v| v.len()),
            Some(2),
            "较大快照必须保留"
        );
        assert_eq!(
            session_get("/test/a", "缓存命中测试 query alpha", 2, 30 * 60, 2),
            None,
            "gen 不符必须 miss"
        );
    }

    #[test]
    fn session_cache_expires_after_window() {
        let key = session_key("/test/expire", "过期测试 query");
        session_cache().lock().unwrap().put(
            key,
            SessionHit {
                hits: vec![("cx".to_string(), 1.0, 1.0)],
                born: utc_now_secs() - 30 * 60 - 1,
                gen: 0,
            },
        );
        assert_eq!(
            session_get("/test/expire", "过期测试 query", 0, 30 * 60, 1),
            None,
            "窗口外必须 miss"
        );
    }

    #[test]
    fn zero_window_disables_session_cache() {
        session_put(
            "/test/zero",
            "关闭缓存测试 query",
            vec![("c0".to_string(), 1.0, 1.0)],
            0,
        );
        assert_eq!(
            session_get("/test/zero", "关闭缓存测试 query", 0, 0, 1),
            None,
            "窗口 0 必须 miss"
        );
    }
}
