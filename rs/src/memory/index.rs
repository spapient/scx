//! 倒排索引（P3.1）：postings.bin 全量构建 / 加载校验（P3.2）/ 候选提取。
//!
//! 布局（自产自销，零依赖，v2 起 doc 表自包含打分快照）：
//! ```text
//! [magic u32 "POST"][ver u16][body_len u64][body][crc32 u32]
//! body:
//!   [n_docs u32][total_len u64][n_terms u32]
//!   doc 表（case id 字典序）：
//!     [idlen u8][id]
//!     [nterms u32][(tlen u8)(term)]*nterms
//!     [nqterms u32][(tlen u8)(term)]*nqterms
//!     [nbuckets u32][(blen u8)(bucket)]*nbuckets
//!     [n_fp u32][(vlen u8)(value)]*n_fp   × 4 字段（error_code/file_loc/symbol/num_fp）
//!     [slen u8][status][quality f32][ulen u8][updated_at]
//!   term 表（term 字典序）：[tlen u16][term][df u32][delta-varint doc_idx 序列]
//! ```
//!
//! 设计要点：
//! - 索引是可丢弃缓存（index/ 目录），`build` 全量重建、确定性：case id 排序 +
//!   term 字典序 → 同库同字节。新增/删除 case 使索引过期 → 调用方惰性作废（删文件，
//!   回退线性扫描）；`--rebuild`/首次自动构建恢复。
//! - v2 起 doc 表自包含打分快照（词表/字段/状态/质量/时间），候选打分零 case.json
//!   读取（P3 性能门：1 万条库召回 <5ms）；df 快照随 build 返回，供 meta [df] 落盘。
//! - P3.2 崩溃一致性：body_len 边界校验 + CRC-32（IEEE 查表）段尾校验；坏段 →
//!   `load` 返回 Err（corrupt），调用方自动重建或回退线性扫描，不崩溃。
//! - 口径说明：索引路径 df/avgdl 为全库口径（区别于线性路径的候选桶内口径），
//!   同一路径内确定；P4 评估统一以索引路径为准。

use crate::memory::store::{self, Meta};
use crate::terms::build_mem_terms;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

/// 自动启用倒排的规模阈值（库 case 数）；`meta [params] index_force=1` 可强制小库启用。
pub(crate) const INDEX_MIN_DOCS: usize = 10_000;

const POST_MAGIC: u32 = 0x5453_4f50; // "POST"
const POST_VER: u16 = 3;

pub(crate) fn postings_path(root: &str) -> PathBuf {
    Path::new(root).join("index").join("postings.bin")
}

/// 写 postings.bin：header + body + crc32（段尾校验，P3.2）。
fn write_postings(root: &str, body: &[u8]) -> std::io::Result<()> {
    let p = postings_path(root);
    if let Some(d) = p.parent() {
        crate::memory::lock::create_dir_all(d)?;
    }
    let crc = crc32(body);
    let mut buf = Vec::with_capacity(14 + body.len() + 4);
    buf.extend_from_slice(&POST_MAGIC.to_le_bytes());
    buf.extend_from_slice(&POST_VER.to_le_bytes());
    buf.extend_from_slice(&(body.len() as u64).to_le_bytes());
    buf.extend_from_slice(body);
    buf.extend_from_slice(&crc.to_le_bytes());
    crate::memory::lock::atomic_write(&p, &buf)
}

fn varint_put(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn varint_get(buf: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0u32;
    loop {
        let b = *buf.get(*i)?;
        *i += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        table[i as usize] = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// doc 打分快照（与 case.json 同源，build 时落盘进 postings，避免召回期二次 IO）。
#[derive(Debug, Clone)]
pub(crate) struct DocRec {
    pub terms: Arc<[String]>,
    /// 原始 case query 词表（query-field 重排特征；与 terms 的 query∪text 不同）。
    pub query_terms: Arc<[String]>,
    /// 多标签场景桶（query/正文任一命中）；过滤用交集，stats 仍用主 bucket。
    pub buckets: Vec<String>,
    pub bucket: String,
    pub fields: Arc<crate::memory::store::Fields>,
    pub status: Arc<str>,
    pub quality: f64,
    pub updated_at: Arc<str>,
}

pub(crate) struct Index {
    pub n_docs: usize,
    pub total_len: usize,
    /// doc_idx → case id（字典序）。
    pub ids: Vec<String>,
    /// doc_idx → 打分快照（与 ids 对齐）。
    pub docs: Vec<DocRec>,
    /// term → (df, doc_idx 升序列表)。
    pub dict: HashMap<String, (usize, Vec<usize>)>,
    /// 实体键 → doc_idx 升序列表；由 doc 快照派生，避免召回时重读全库 case。
    pub entities: HashMap<String, Vec<usize>>,
}

/// 从 doc 字段快照派生实体倒排（实体键去重、doc_idx 升序）。
fn entity_map(docs: &[DocRec]) -> HashMap<String, Vec<usize>> {
    let mut entities: HashMap<String, Vec<usize>> = HashMap::new();
    for (di, rec) in docs.iter().enumerate() {
        for key in crate::memory::field::entity_keys(&rec.fields) {
            entities.entry(key).or_default().push(di);
        }
    }
    entities
}

/// 获取索引写锁。调用方在“case 资产变更 + 索引失效 + gen 前进”期间持锁，
/// 防止 index::build 在失效之后又把旧快照写回。
pub(crate) fn lock(root: &str) -> std::io::Result<crate::memory::lock::FileLock> {
    crate::memory::lock::acquire(root, "index.lock")
}

/// 持锁作废索引（调用方必须已持有 [`lock`] 返回的守卫）。
pub(crate) fn invalidate_locked(root: &str) {
    let p = postings_path(root);
    if p.exists() {
        let _ = fs::remove_file(&p);
    }
    // 进程内缓存随之失效：mtime 键已失（文件删除 → 下次 load 短路 None）。
    let mut g = cache().lock().unwrap();
    if let Some((r, _, _)) = g.as_ref() {
        if r == root {
            *g = None;
        }
    }
}

/// 惰性作废索引（新增/删除 case 后调用）：删 postings.bin，召回回退线性/按需重建。
/// 锁失败必须上抛：静默失败会让新 case 永远被旧索引漏掉。
#[cfg(test)]
pub(crate) fn invalidate(root: &str) -> std::io::Result<()> {
    let _g = lock(root)?;
    invalidate_locked(root);
    Ok(())
}

/// 进程内索引缓存（会话/守护进程多次召回免重复 parse；CLI 单次无副作用）。
/// 键 = (root, postings mtime)：invalidate 删文件/重建后 mtime 变化 → 自动 miss。
type CacheEntry = (String, SystemTime, Arc<Index>);
static INDEX_CACHE: OnceLock<Mutex<Option<CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CacheEntry>> {
    INDEX_CACHE.get_or_init(|| Mutex::new(None))
}

/// df 快照行：term 字典序 → 库内出现次数。
pub(crate) type Df = Vec<(String, usize)>;

/// 全量构建 postings.bin（读取全部 case 的 query∪text 词表）。
/// 返回 (n_docs, n_terms, df 快照按 term 字典序) —— df 供 meta [df] 节落盘。
pub(crate) fn build(root: &str) -> std::io::Result<(usize, usize, Df)> {
    // 跨进程串行构建：避免两个 rebuild/自动构建同时写 postings.bin。
    let _g = lock(root)?;
    let ids = store::list_ids(root)?;
    let mut docs: Vec<(String, DocRec)> = Vec::with_capacity(ids.len());
    let mut total_len = 0usize;
    for id in &ids {
        let c = store::read_case(root, id)?;
        let query_terms = build_mem_terms(&c.query);
        let mut terms = query_terms.clone();
        terms.extend(build_mem_terms(&c.text));
        terms.sort();
        terms.dedup();
        total_len += terms.len();
        let buckets = crate::memory::bucket::case_buckets(&[], &query_terms, &c.text);
        docs.push((
            id.clone(),
            DocRec {
                terms: terms.into(),
                query_terms: query_terms.into(),
                buckets,
                bucket: c.bucket,
                fields: Arc::new(c.fields),
                status: c.status.into(),
                quality: c.quality,
                updated_at: c.updated_at.into(),
            },
        ));
    }
    let mut dict: HashMap<String, Vec<usize>> = HashMap::new();
    for (di, (_, rec)) in docs.iter().enumerate() {
        for t in rec.terms.iter() {
            dict.entry(t.clone()).or_default().push(di);
        }
    }
    let mut body: Vec<u8> = Vec::with_capacity(32 + docs.len() * 96);
    body.extend_from_slice(&(docs.len() as u32).to_le_bytes());
    body.extend_from_slice(&(total_len as u64).to_le_bytes());
    body.extend_from_slice(&(dict.len() as u32).to_le_bytes());
    for (id, rec) in &docs {
        // doc 记录：id + 去重词表 + 4 字段 + 状态/质量/时间（打分快照）。
        body.push(id.len() as u8);
        body.extend_from_slice(id.as_bytes());
        body.extend_from_slice(&(rec.terms.len() as u32).to_le_bytes());
        for t in rec.terms.iter() {
            body.push(t.len() as u8);
            body.extend_from_slice(t.as_bytes());
        }
        body.extend_from_slice(&(rec.query_terms.len() as u32).to_le_bytes());
        for t in rec.query_terms.iter() {
            body.push(t.len() as u8);
            body.extend_from_slice(t.as_bytes());
        }
        body.extend_from_slice(&(rec.buckets.len() as u32).to_le_bytes());
        for b in &rec.buckets {
            body.push(b.len() as u8);
            body.extend_from_slice(b.as_bytes());
        }
        for f in [
            &rec.fields.error_code,
            &rec.fields.file_loc,
            &rec.fields.symbol,
            &rec.fields.num_fp,
        ] {
            body.extend_from_slice(&(f.len() as u32).to_le_bytes());
            for v in f {
                body.push(v.len() as u8);
                body.extend_from_slice(v.as_bytes());
            }
        }
        body.push(rec.bucket.len() as u8);
        body.extend_from_slice(rec.bucket.as_bytes());
        body.push(rec.status.len() as u8);
        body.extend_from_slice(rec.status.as_bytes());
        body.extend_from_slice(&(rec.quality as f32).to_le_bytes());
        body.push(rec.updated_at.len() as u8);
        body.extend_from_slice(rec.updated_at.as_bytes());
    }
    let mut term_keys: Vec<&String> = dict.keys().collect();
    term_keys.sort(); // term 字典序，确定性
    for t in &term_keys {
        let post = &dict[*t];
        body.extend_from_slice(&(t.len() as u16).to_le_bytes());
        body.extend_from_slice(t.as_bytes());
        body.extend_from_slice(&(post.len() as u32).to_le_bytes());
        let mut prev = 0usize;
        for &di in post {
            varint_put(&mut body, (di - prev) as u64);
            prev = di;
        }
    }
    write_postings(root, &body)?;
    let df: Vec<(String, usize)> = term_keys
        .iter()
        .map(|t| ((*t).clone(), dict[*t].len()))
        .collect();
    Ok((docs.len(), dict.len(), df))
}

/// 加载索引。不存在 → Ok(None)；校验失败 → Err(corrupt)（调用方重建或回退线性）。
/// 命中进程内缓存（root+mtime 键）直接复用 Arc，零重复 parse；否则整文件 fs::read
/// 一次读入解析（P4.4 评审处置：移除手写 unsafe mmap FFI——无文件锁前提下
/// 另一进程 rebuild/truncate 会使映射失效；1 万条库 postings 仅数 MB，fs::read
/// 在页缓存下开销可忽略，契约零 unsafe）。
pub(crate) fn load(root: &str) -> std::io::Result<Option<Arc<Index>>> {
    let p = postings_path(root);
    if !p.exists() {
        return Ok(None);
    }
    let mtime = p.metadata().and_then(|m| m.modified()).ok();
    if let Some(mt) = mtime {
        let g = cache().lock().unwrap();
        if let Some((r, t, idx)) = g.as_ref() {
            if r == root && *t == mt {
                return Ok(Some(idx.clone()));
            }
        }
    }
    let buf = fs::read(&p)?;
    let parsed: Option<Index> = parse(&buf).map(Some)?;
    let arc = parsed.map(Arc::new);
    if let (Some(mt), Some(idx)) = (mtime, arc.as_ref()) {
        let mut g = cache().lock().unwrap();
        *g = Some((root.to_string(), mt, idx.clone()));
    }
    Ok(arc)
}

/// 解析字节切片为内存索引（纯函数，输入来自 fs::read）。
pub(crate) fn parse(buf: &[u8]) -> std::io::Result<Index> {
    let corrupt = |m: &str| std::io::Error::other(format!("postings.bin corrupt: {m}"));
    if buf.len() < 14 {
        return Err(corrupt("too short"));
    }
    let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    if magic != POST_MAGIC {
        return Err(corrupt("bad magic"));
    }
    let ver = u16::from_le_bytes(buf[4..6].try_into().unwrap());
    if ver != POST_VER {
        return Err(corrupt("bad version"));
    }
    let body_len = usize::try_from(u64::from_le_bytes(buf[6..14].try_into().unwrap()))
        .map_err(|_| corrupt("body_len too large"))?;
    let body_end = 14usize
        .checked_add(body_len)
        .ok_or_else(|| corrupt("body_len overflow"))?;
    let total_len = body_end
        .checked_add(4)
        .ok_or_else(|| corrupt("file length overflow"))?;
    if total_len != buf.len() {
        return Err(corrupt("bad body_len"));
    }
    let body = &buf[14..body_end];
    let crc = u32::from_le_bytes(buf[body_end..total_len].try_into().unwrap());
    if crc32(body) != crc {
        return Err(corrupt("crc mismatch"));
    }
    let mut i = 0usize;
    let take_u32 = |i: &mut usize| -> Option<u32> {
        if *i + 4 > body.len() {
            return None;
        }
        let v = u32::from_le_bytes(body[*i..*i + 4].try_into().unwrap());
        *i += 4;
        Some(v)
    };
    let n_docs = take_u32(&mut i).ok_or_else(|| corrupt("no n_docs"))? as usize;
    if n_docs > body.len() {
        return Err(corrupt("n_docs exceeds body"));
    }
    if i + 8 > body.len() {
        return Err(corrupt("no total_len"));
    }
    let total_len = u64::from_le_bytes(body[i..i + 8].try_into().unwrap()) as usize;
    i += 8;
    let n_terms = take_u32(&mut i).ok_or_else(|| corrupt("no n_terms"))? as usize;
    if n_terms > body.len() {
        return Err(corrupt("n_terms exceeds body"));
    }
    let mut ids: Vec<String> = Vec::with_capacity(n_docs);
    let mut docs: Vec<DocRec> = Vec::with_capacity(n_docs);
    let take_str = |i: &mut usize, body: &[u8]| -> std::io::Result<String> {
        if *i >= body.len() {
            return Err(corrupt("str len truncated"));
        }
        let len = body[*i] as usize;
        *i += 1;
        if *i + len > body.len() {
            return Err(corrupt("str overflow"));
        }
        let s = String::from_utf8_lossy(&body[*i..*i + len]).into_owned();
        *i += len;
        Ok(s)
    };
    for _ in 0..n_docs {
        let id = take_str(&mut i, body)?;
        let nterms = take_u32(&mut i).ok_or_else(|| corrupt("no doc nterms"))? as usize;
        if nterms > body.len() {
            return Err(corrupt("doc nterms exceeds body"));
        }
        let mut terms = Vec::with_capacity(nterms);
        for _ in 0..nterms {
            terms.push(take_str(&mut i, body)?);
        }
        let nqterms = take_u32(&mut i).ok_or_else(|| corrupt("no doc nqterms"))? as usize;
        if nqterms > body.len() {
            return Err(corrupt("doc nqterms exceeds body"));
        }
        let mut query_terms = Vec::with_capacity(nqterms);
        for _ in 0..nqterms {
            query_terms.push(take_str(&mut i, body)?);
        }
        let nbuckets = take_u32(&mut i).ok_or_else(|| corrupt("no doc nbuckets"))? as usize;
        if nbuckets > body.len() {
            return Err(corrupt("doc nbuckets exceeds body"));
        }
        let mut buckets = Vec::with_capacity(nbuckets);
        for _ in 0..nbuckets {
            buckets.push(take_str(&mut i, body)?);
        }
        let take_vec = |i: &mut usize, body: &[u8]| -> std::io::Result<Vec<String>> {
            let n = take_u32(i).ok_or_else(|| corrupt("no field n"))? as usize;
            if n > body.len() {
                return Err(corrupt("field count exceeds body"));
            }
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(take_str(i, body)?);
            }
            Ok(v)
        };
        let fields = crate::memory::store::Fields {
            error_code: take_vec(&mut i, body)?,
            file_loc: take_vec(&mut i, body)?,
            symbol: take_vec(&mut i, body)?,
            num_fp: take_vec(&mut i, body)?,
        };
        let bucket = take_str(&mut i, body)?;
        let status = take_str(&mut i, body)?;
        if i + 4 > body.len() {
            return Err(corrupt("no quality"));
        }
        let quality = f32::from_le_bytes(body[i..i + 4].try_into().unwrap()) as f64;
        i += 4;
        let updated_at = take_str(&mut i, body)?;
        ids.push(id);
        docs.push(DocRec {
            terms: terms.into(),
            query_terms: query_terms.into(),
            buckets,
            bucket,
            fields: Arc::new(fields),
            status: status.into(),
            quality,
            updated_at: updated_at.into(),
        });
    }
    let mut dict: HashMap<String, (usize, Vec<usize>)> = HashMap::with_capacity(n_terms);
    for _ in 0..n_terms {
        if i + 2 > body.len() {
            return Err(corrupt("term len truncated"));
        }
        let tlen = u16::from_le_bytes(body[i..i + 2].try_into().unwrap()) as usize;
        i += 2;
        if i + tlen + 4 > body.len() {
            return Err(corrupt("term overflow"));
        }
        let term = String::from_utf8_lossy(&body[i..i + tlen]).into_owned();
        i += tlen;
        let df = u32::from_le_bytes(body[i..i + 4].try_into().unwrap()) as usize;
        i += 4;
        if df > body.len() {
            return Err(corrupt("df exceeds body"));
        }
        let mut post = Vec::with_capacity(df);
        let mut prev = 0usize;
        for _ in 0..df {
            let d = varint_get(body, &mut i).ok_or_else(|| corrupt("posting varint"))?;
            let doc = prev + d as usize;
            if doc >= n_docs {
                return Err(corrupt("doc idx overflow"));
            }
            post.push(doc);
            prev = doc;
        }
        dict.insert(term, (df, post));
    }
    let entities = entity_map(&docs);
    Ok(Index {
        n_docs,
        total_len,
        ids,
        docs,
        dict,
        entities,
    })
}

/// 候选：query 词命中 doc 的并集（去重、按 doc_idx 升序 → 确定性）。
/// 返回 doc_idx（配合 `idx.docs[di]` 零 case 文件 IO 打分）。
pub(crate) fn candidates(idx: &Index, qterms: &[String]) -> Vec<usize> {
    let mut seen: Vec<bool> = vec![false; idx.n_docs];
    let mut hit: Vec<usize> = Vec::new();
    for t in qterms {
        if let Some((_, post)) = idx.dict.get(t) {
            for &di in post {
                if !seen[di] {
                    seen[di] = true;
                    hit.push(di);
                }
            }
        }
    }
    hit.sort_unstable();
    hit
}

/// term 的全库 df（未收录 = 0）。
pub(crate) fn term_df(idx: &Index, t: &str) -> usize {
    idx.dict.get(t).map(|(df, _)| *df).unwrap_or(0)
}

/// query 实体命中的兄弟 doc（去重后按 doc_idx 升序），并给出共享实体数。
pub(crate) fn entity_neighbors(idx: &Index, ents: &[String]) -> Vec<(usize, usize)> {
    let mut keys: Vec<&str> = ents.iter().map(String::as_str).collect();
    keys.sort_unstable();
    keys.dedup();
    let mut shared: HashMap<usize, usize> = HashMap::new();
    for key in keys {
        if let Some(docs) = idx.entities.get(key) {
            for &di in docs {
                *shared.entry(di).or_insert(0) += 1;
            }
        }
    }
    let mut out: Vec<(usize, usize)> = shared.into_iter().collect();
    out.sort_unstable_by_key(|&(di, _)| di);
    out
}

/// 是否启用倒排路径：规模达标（meta 可覆盖）或 --rebuild 后强制（index_force=1）。
pub(crate) fn use_index(meta: &Meta) -> bool {
    let force = meta.param("index_force") > 0.0;
    force || meta.case_count >= INDEX_MIN_DOCS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ingest::ingest;
    use crate::memory::store::{self, CaseStats};
    use std::fs;

    fn tmp_root(tag: &str) -> String {
        let r = std::env::temp_dir().join(format!("scx-idx-{}-{}", tag, store::fnv1a("i") & 0xff));
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
    fn crc_and_varint_are_deterministic() {
        let a = crc32(b"hello world");
        assert_eq!(a, crc32(b"hello world"));
        assert_ne!(a, crc32(b"hello worle"));
        let mut buf = Vec::new();
        varint_put(&mut buf, 0);
        varint_put(&mut buf, 127);
        varint_put(&mut buf, 128);
        varint_put(&mut buf, 300);
        varint_put(&mut buf, 1 << 35);
        let mut i = 0;
        assert_eq!(varint_get(&buf, &mut i), Some(0));
        assert_eq!(varint_get(&buf, &mut i), Some(127));
        assert_eq!(varint_get(&buf, &mut i), Some(128));
        assert_eq!(varint_get(&buf, &mut i), Some(300));
        assert_eq!(varint_get(&buf, &mut i), Some(1 << 35));
        assert_eq!(varint_get(&buf, &mut i), None, "越界返回 None");
    }

    fn seed(root: &str) {
        let s = stats();
        ingest(
            root,
            "数据库连接超时",
            vec!["conn_id".into()],
            "ERROR db timeout conn_id=9f2a alpha",
            vec!["/a.log".into()],
            &s,
            vec![],
        )
        .unwrap();
        ingest(
            root,
            "磁盘空间不足",
            vec!["disk".into()],
            "ERROR disk full errno=28 beta",
            vec!["/b.log".into()],
            &s,
            vec![],
        )
        .unwrap();
        ingest(
            root,
            "编译失败",
            vec!["cargo".into()],
            "error: could not compile gamma",
            vec!["/c.log".into()],
            &s,
            vec![],
        )
        .unwrap();
    }

    #[test]
    fn build_parse_roundtrip() {
        let root = tmp_root("rt");
        seed(&root);
        let (n_docs, n_terms, df) = build(&root).unwrap();
        assert_eq!(n_docs, 3);
        assert!(n_terms > 0);
        assert_eq!(df.len(), n_terms, "df 快照覆盖全部 term");
        let idx = load(&root).unwrap().expect("index 应存在");
        assert_eq!(idx.n_docs, 3);
        assert_eq!(idx.ids.len(), 3);
        assert!(idx.ids.windows(2).all(|w| w[0] < w[1]), "id 字典序");
        assert!(
            idx.docs.iter().all(|d| !d.query_terms.is_empty()),
            "索引必须携带 case-query 词表，倒排路径才能执行 query-field 重排"
        );
        assert!(
            idx.docs.iter().all(|d| !d.buckets.is_empty()),
            "索引必须携带多标签桶"
        );
        // df 语义：df=posting 长度，且 posting 严格递增（确定性）。
        for (df, post) in idx.dict.values() {
            assert_eq!(*df, post.len());
            assert!(post.windows(2).all(|w| w[0] < w[1]), "doc_idx 升序");
        }
        // 跨 case 词 df>1："error"（空格分隔）出现在 A/B 两个 seed 文本；
        // C 的 "error: could not" 冒号并入 token → "error:" 独立（df=1）。
        assert_eq!(idx.dict.get("error").map(|(df, _)| *df), Some(2));
        assert_eq!(idx.dict.get("error:").map(|(df, _)| *df), Some(1));
        let entities = entity_neighbors(&idx, &["conn_id=9f2a".to_string()]);
        assert_eq!(entities.len(), 1, "实体倒排应直接来自 doc 快照");
        assert_eq!(entities[0].1, 1, "共享实体数必须参与实体加权");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn candidates_union_dedup_sorted() {
        let root = tmp_root("cand");
        seed(&root);
        build(&root).unwrap();
        let idx = load(&root).unwrap().unwrap();
        // 词命中两篇：conn_id 只中 A；error 中 A+B（db timeout 与 disk full 都含 error）。
        let one = candidates(&idx, &["conn_id".to_string()]);
        assert_eq!(one.len(), 1);
        let both = candidates(&idx, &["error".to_string(), "disk".to_string()]);
        assert_eq!(both.len(), 2, "A+B 并集应为 2（确定性排序）");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_tail_rejected_and_rebuild_recovers() {
        let root = tmp_root("cor");
        seed(&root);
        build(&root).unwrap();
        let p = postings_path(&root);
        let mut buf = fs::read(&p).unwrap();
        buf.truncate(buf.len() - 4); // 截断 crc 尾部 → body_len 不匹配
        fs::write(&p, &buf).unwrap();
        assert!(
            load(&root).is_err(),
            "坏段必须报 corrupt（Err），不静默吞掉"
        );
        // --rebuild 语义：build 覆盖坏文件后恢复。
        let (nd, _, _) = build(&root).unwrap();
        assert_eq!(nd, 3);
        assert!(load(&root).unwrap().is_some(), "重建后索引应可加载");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn invalidate_removes_index_file() {
        let root = tmp_root("inv");
        seed(&root);
        build(&root).unwrap();
        assert!(postings_path(&root).exists());
        invalidate(&root).unwrap();
        assert!(!postings_path(&root).exists());
        assert!(load(&root).unwrap().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_case_invalidates_cached_index() {
        let root = tmp_root("wcinv");
        seed(&root);
        build(&root).unwrap();
        assert!(postings_path(&root).exists());
        let c = store::Case::new(
            "newcase".to_string(),
            "new query".to_string(),
            vec![],
            "ERROR new evidence".to_string(),
        );
        store::write_case(&root, &c).unwrap();
        assert!(
            !postings_path(&root).exists(),
            "新 case 必须让旧索引失效，否则大库会漏召"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_rejects_overflowing_body_len() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&POST_MAGIC.to_le_bytes());
        buf.extend_from_slice(&POST_VER.to_le_bytes());
        buf.extend_from_slice(&u64::MAX.to_le_bytes());
        buf.extend_from_slice(&[0u8; 4]);
        let err = match parse(&buf) {
            Ok(_) => panic!("溢出 body_len 必须被拒绝"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("body_len"),
            "溢出长度必须明确报 corrupt: {err}"
        );
    }
}
