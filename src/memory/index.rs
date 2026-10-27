//! 倒排索引：postings.bin 全量构建、加载校验与候选提取。
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
//!   读取（1 万条库召回 p50 < 5 ms）；df 快照随 build 返回，供 meta [df] 落盘。
//! - 崩溃一致性：body_len 边界校验 + CRC-32（IEEE 查表）段尾校验；坏段 →
//!   `load` 返回 Err（corrupt），调用方自动重建或回退线性扫描，不崩溃。
//! - 口径说明：线性与索引两条路径的 df/avgdl 均为全库口径，桶过滤只裁候选；
//!   同一 query 在两条路径下使用同一 BM25 统计量，A/B 对拍要求输出一致。

use crate::memory::store::{self, Meta};
use crate::terms::document;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

/// 自动启用倒排的规模阈值（库 case 数）；`meta [params] index_force=1` 可强制小库启用。
pub(crate) const INDEX_MIN_DOCS: usize = 10_000;

const POST_MAGIC: u32 = 0x5453_4f50; // "POST"
const POST_VER: u16 = 3;

pub(crate) fn postings(root: &str) -> PathBuf {
    Path::new(root).join("index").join("postings.bin")
}

/// 写 postings.bin：header + body + crc32 段尾校验。
fn write(root: &str, body: &[u8]) -> std::io::Result<()> {
    let p = postings(root);
    if let Some(d) = p.parent() {
        crate::memory::lock::mkdir(d)?;
    }
    let digest = checksum(body);
    let mut buf = Vec::with_capacity(14 + body.len() + 4);
    buf.extend_from_slice(&POST_MAGIC.to_le_bytes());
    buf.extend_from_slice(&POST_VER.to_le_bytes());
    buf.extend_from_slice(&(body.len() as u64).to_le_bytes());
    buf.extend_from_slice(body);
    buf.extend_from_slice(&digest.to_le_bytes());
    crate::memory::lock::atomic(&p, &buf)
}

fn put(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn get(buf: &[u8], i: &mut usize) -> Option<u64> {
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

fn checksum(data: &[u8]) -> u32 {
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
    let mut sum = 0xFFFF_FFFFu32;
    for &b in data {
        sum = table[((sum ^ u32::from(b)) & 0xFF) as usize] ^ (sum >> 8);
    }
    !sum
}

/// doc 打分快照（与 case.json 同源，build 时落盘进 postings，避免召回期二次 IO）。
#[derive(Debug, Clone)]
pub(crate) struct Record {
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
    /// `updated_at` 的 Unix 秒快照；召回热循环不再逐候选解析日期。
    pub updated_s: u64,
}

pub(crate) struct Index {
    pub n_docs: usize,
    pub total_len: usize,
    /// doc_idx → case id（字典序）；候选只克隆 `Arc`，不复制字符串。
    pub ids: Vec<Arc<str>>,
    /// doc_idx → 打分快照（与 ids 对齐）。
    pub docs: Vec<Record>,
    /// term → (df, doc_idx 升序列表)。
    pub dict: HashMap<String, (usize, Vec<usize>)>,
    /// 实体键 → doc_idx 升序列表；由 doc 快照惰性派生（无实体查询时不付构建成本）。
    entities: OnceLock<HashMap<String, Vec<usize>>>,
}

/// 从 doc 字段快照派生实体倒排（实体键去重、doc_idx 升序）。
fn entities(docs: &[Record]) -> HashMap<String, Vec<usize>> {
    let mut entities: HashMap<String, Vec<usize>> = HashMap::new();
    for (di, rec) in docs.iter().enumerate() {
        for key in crate::memory::classify::keys(&rec.fields) {
            entities.entry(key).or_default().push(di);
        }
    }
    entities
}

/// 获取索引写锁。调用方在"case 资产变更 + 索引失效 + gen 前进"期间持锁，
/// 防止 index::build 在失效之后又把旧快照写回。
pub(crate) fn lock(root: &str) -> std::io::Result<crate::memory::lock::Lock> {
    crate::memory::lock::acquire(root, "index.lock")
}

/// 持锁清除索引（调用方必须已持有 [`lock`] 返回的守卫）。
pub(crate) fn clear(root: &str) {
    let p = postings(root);
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
    clear(root);
    Ok(())
}

/// 进程内索引缓存（会话/守护进程多次召回免重复 parse；CLI 单次无副作用）。
/// 键 = (root, postings mtime)：invalidate 删文件/重建后 mtime 变化 → 自动 miss。
/// 缓存键：mtime + 文件长度（仅 mtime 在粗粒度文件系统上会误命中）。
type Key = (SystemTime, u64);
type Entry = (String, Key, Arc<Index>);
static INDEX_CACHE: OnceLock<Mutex<Option<Entry>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<Entry>> {
    INDEX_CACHE.get_or_init(|| Mutex::new(None))
}

/// df 快照行：term 字典序 → 库内出现次数。
pub(crate) type Df = Vec<(String, usize)>;

/// 读取全部 case 构建 doc 打分快照；返回 (docs, 全库词数 total_len)。
fn records(root: &str, ids: &[String]) -> std::io::Result<(Vec<(String, Record)>, usize)> {
    let mut docs: Vec<(String, Record)> = Vec::with_capacity(ids.len());
    let mut total_len = 0usize;
    for id in ids {
        let c = store::load(root, id)?;
        let (query_terms, terms) = document(&c.query, &c.text);
        let updated_s = crate::memory::util::parse(&c.updated_at);
        total_len += terms.len();
        let buckets = crate::memory::classify::buckets(&[], &query_terms, &c.text);
        docs.push((
            id.clone(),
            Record {
                terms: terms.into(),
                query_terms: query_terms.into(),
                buckets,
                bucket: c.bucket,
                fields: Arc::new(c.fields),
                status: c.status.into(),
                quality: c.quality,
                updated_at: c.updated_at.into(),
                updated_s,
            },
        ));
    }
    Ok((docs, total_len))
}

/// 由 doc 快照派生 term → doc_idx 列表（doc_idx 升序 = docs 迭代序）。
fn terms(docs: &[(String, Record)]) -> HashMap<String, Vec<usize>> {
    let mut dict: HashMap<String, Vec<usize>> = HashMap::new();
    for (di, (_, rec)) in docs.iter().enumerate() {
        for t in rec.terms.iter() {
            // 命中已存在的键时不再克隆字符串（10k 库可达百万次无谓分配）。
            match dict.get_mut(t.as_str()) {
                Some(v) => v.push(di),
                None => {
                    dict.insert(t.clone(), vec![di]);
                }
            }
        }
    }
    dict
}

/// 序列化一条字符串列表段：[len u32][(u8 len + bytes)*]。
fn strings(body: &mut Vec<u8>, list: &[String]) {
    body.extend_from_slice(&(list.len() as u32).to_le_bytes());
    for s in list {
        body.push(s.len() as u8);
        body.extend_from_slice(s.as_bytes());
    }
}

/// 写 doc 表（id + 词表 + 4 字段 + 状态/质量/时间快照）。
fn table(body: &mut Vec<u8>, docs: &[(String, Record)]) {
    for (id, rec) in docs {
        body.push(id.len() as u8);
        body.extend_from_slice(id.as_bytes());
        strings(body, &rec.terms);
        strings(body, &rec.query_terms);
        strings(body, &rec.buckets);
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
}

/// 写 term 表（term 字典序 + df + delta-varint doc_idx 序列）。
fn lexicon(body: &mut Vec<u8>, dict: &HashMap<String, Vec<usize>>) {
    let mut term_keys: Vec<&String> = dict.keys().collect();
    term_keys.sort(); // term 字典序，确定性
    for t in &term_keys {
        let post = &dict[*t];
        body.extend_from_slice(&(t.len() as u16).to_le_bytes());
        body.extend_from_slice(t.as_bytes());
        body.extend_from_slice(&(post.len() as u32).to_le_bytes());
        let mut prev = 0usize;
        for &di in post {
            put(body, (di - prev) as u64);
            prev = di;
        }
    }
}

/// 全量构建 postings.bin（读取全部 case 的 query∪text 词表）。
/// 返回 (n_docs, n_terms, df 快照按 term 字典序) —— df 供 meta [df] 节落盘。
pub(crate) fn build(root: &str) -> std::io::Result<(usize, usize, Df)> {
    // 跨进程串行构建：避免两个 rebuild/自动构建同时写 postings.bin。
    let _g = lock(root)?;
    let ids = store::list(root)?;
    let (docs, total_len) = records(root, &ids)?;
    let dict = terms(&docs);
    let mut body: Vec<u8> = Vec::with_capacity(32 + docs.len() * 96);
    body.extend_from_slice(&(docs.len() as u32).to_le_bytes());
    body.extend_from_slice(&(total_len as u64).to_le_bytes());
    body.extend_from_slice(&(dict.len() as u32).to_le_bytes());
    table(&mut body, &docs);
    lexicon(&mut body, &dict);
    write(root, &body)?;
    // df 快照按 term 字典序输出（与原 term_keys.sort() 同序）。
    let mut df: Vec<(String, usize)> = dict.iter().map(|(t, v)| (t.clone(), v.len())).collect();
    df.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((docs.len(), dict.len(), df))
}

/// 加载索引。不存在 → Ok(None)；校验失败 → Err(corrupt)（调用方重建或回退线性）。
/// 命中进程内缓存（root+mtime 键）直接复用 Arc，零重复 parse；否则整文件 fs::read
/// 一次读入解析。不使用 mmap：无文件锁时，另一进程 rebuild/truncate 会使映射失效；
/// 1 万条库的 postings 仅数 MB，`fs::read` 在页缓存下开销可忽略，且保持零 unsafe。
pub(crate) fn load(root: &str) -> std::io::Result<Option<Arc<Index>>> {
    let p = postings(root);
    // 缓存键 = mtime + 文件长度：粗粒度 mtime 文件系统（ext3/NFS/SMB）上同秒重建
    // 会得到相同 mtime，仅比 mtime 会长期复用旧索引并漏掉新 case。
    let mtime = match p.metadata() {
        Ok(m) => m.modified().ok().map(|t| (t, m.len())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => None,
    };
    if let Some(idx) = cached(root, mtime.as_ref()) {
        return Ok(Some(idx));
    }
    let buf = match fs::read(&p) {
        Ok(b) => b,
        // 读取与 stat 之间被并发 rebuild 删除：按"无索引"处理，不当作错误。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let parsed: Option<Index> = parse(&buf).map(Some)?;
    let arc = parsed.map(Arc::new);
    store(root, mtime, arc.as_ref());
    Ok(arc)
}

fn cached(root: &str, mtime: Option<&Key>) -> Option<Arc<Index>> {
    let mt = mtime?;
    let g = cache().lock().unwrap();
    let (r, t, idx) = g.as_ref()?;
    (r == root && t == mt).then(|| idx.clone())
}

fn store(root: &str, mtime: Option<Key>, idx: Option<&Arc<Index>>) {
    if let (Some(mt), Some(idx)) = (mtime, idx) {
        let mut g = cache().lock().unwrap();
        *g = Some((root.to_string(), mt, idx.clone()));
    }
}

/// 解析失败统一错误（postings.bin corrupt: ...）。
fn corrupt(m: &str) -> std::io::Error {
    std::io::Error::other(format!("postings.bin corrupt: {m}"))
}

/// 从游标语境取 u32（边界不足返回 None）。
fn word(body: &[u8], i: &mut usize) -> Option<u32> {
    if *i + 4 > body.len() {
        return None;
    }
    let v = u32::from_le_bytes(body[*i..*i + 4].try_into().unwrap());
    *i += 4;
    Some(v)
}

/// 从游标语境取一条 u8 长度前缀字符串。
fn text(body: &[u8], i: &mut usize) -> std::io::Result<String> {
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
}

/// 从游标语境取一个 u32 长度前缀的字符串列表（用于 4 个字段向量）。
fn list(body: &[u8], i: &mut usize) -> std::io::Result<Vec<String>> {
    let n = count(body, i, "no field n")?;
    batch(body, i, n)
}

fn count(body: &[u8], i: &mut usize, what: &str) -> std::io::Result<usize> {
    let n = word(body, i).ok_or_else(|| corrupt(what))? as usize;
    if n > body.len() {
        return Err(corrupt(&format!("{what} exceeds body")));
    }
    Ok(n)
}

fn batch(body: &[u8], i: &mut usize, n: usize) -> std::io::Result<Vec<String>> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(text(body, i)?);
    }
    Ok(v)
}

fn float(body: &[u8], i: &mut usize, what: &str) -> std::io::Result<f32> {
    if *i + 4 > body.len() {
        return Err(corrupt(what));
    }
    let v = f32::from_le_bytes(body[*i..*i + 4].try_into().unwrap());
    *i += 4;
    Ok(v)
}

/// 校验 header（magic/version/body_len/crc32），返回 body 切片。
fn open(buf: &[u8]) -> std::io::Result<&[u8]> {
    let (body_end, total_len) = valid(buf)?;
    let body = &buf[14..body_end];
    let stored = u32::from_le_bytes(buf[body_end..total_len].try_into().unwrap());
    if checksum(body) != stored {
        return Err(corrupt("crc mismatch"));
    }
    Ok(body)
}

fn valid(buf: &[u8]) -> std::io::Result<(usize, usize)> {
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
    let (body_end, total_len) = lengths(buf)?;
    if total_len != buf.len() {
        return Err(corrupt("bad body_len"));
    }
    Ok((body_end, total_len))
}

fn lengths(buf: &[u8]) -> std::io::Result<(usize, usize)> {
    let body_len = usize::try_from(u64::from_le_bytes(buf[6..14].try_into().unwrap()))
        .map_err(|_| corrupt("body_len too large"))?;
    let body_end = 14usize
        .checked_add(body_len)
        .ok_or_else(|| corrupt("body_len overflow"))?;
    let total_len = body_end
        .checked_add(4)
        .ok_or_else(|| corrupt("file length overflow"))?;
    Ok((body_end, total_len))
}

/// 解析 doc 表（n_docs 条打分快照），返回 (ids, docs)。
fn scan(
    body: &[u8],
    n_docs: usize,
    i: &mut usize,
) -> std::io::Result<(Vec<Arc<str>>, Vec<Record>)> {
    let mut ids: Vec<Arc<str>> = Vec::with_capacity(n_docs);
    let mut docs: Vec<Record> = Vec::with_capacity(n_docs);
    for _ in 0..n_docs {
        let (id, doc) = doc(body, i)?;
        ids.push(id);
        docs.push(doc);
    }
    Ok((ids, docs))
}

fn doc(body: &[u8], i: &mut usize) -> std::io::Result<(Arc<str>, Record)> {
    let (id, terms, query_terms, buckets) = split(body, i)?;
    let fields = values(body, i)?;
    let (bucket, status, quality, updated_at) = tail(body, i)?;
    let updated_s = crate::memory::util::parse(&updated_at);
    Ok((
        id.into(),
        Record {
            terms: terms.into(),
            query_terms: query_terms.into(),
            buckets,
            bucket,
            fields: Arc::new(fields),
            status: status.into(),
            quality,
            updated_at: updated_at.into(),
            updated_s,
        },
    ))
}

type Terms = (String, Vec<String>, Vec<String>, Vec<String>);

fn split(body: &[u8], i: &mut usize) -> std::io::Result<Terms> {
    let id = text(body, i)?;
    let nterms = count(body, i, "no doc nterms")?;
    let terms = batch(body, i, nterms)?;
    let nqterms = count(body, i, "no doc nqterms")?;
    let query_terms = batch(body, i, nqterms)?;
    let nbuckets = count(body, i, "no doc nbuckets")?;
    let buckets = batch(body, i, nbuckets)?;
    Ok((id, terms, query_terms, buckets))
}

fn values(body: &[u8], i: &mut usize) -> std::io::Result<crate::memory::store::Fields> {
    Ok(crate::memory::store::Fields {
        error_code: list(body, i)?,
        file_loc: list(body, i)?,
        symbol: list(body, i)?,
        num_fp: list(body, i)?,
    })
}

fn tail(body: &[u8], i: &mut usize) -> std::io::Result<(String, String, f64, String)> {
    let bucket = text(body, i)?;
    let status = text(body, i)?;
    let quality = float(body, i, "no quality")? as f64;
    let updated_at = text(body, i)?;
    Ok((bucket, status, quality, updated_at))
}

/// 解析 term 表（n_terms 条倒排），返回 term → (df, doc_idx 升序列表)。
fn dictionary(
    body: &[u8],
    n_terms: usize,
    n_docs: usize,
    i: &mut usize,
) -> std::io::Result<HashMap<String, (usize, Vec<usize>)>> {
    let mut dict: HashMap<String, (usize, Vec<usize>)> = HashMap::with_capacity(n_terms);
    for _ in 0..n_terms {
        let (term, entry) = term(body, n_docs, i)?;
        dict.insert(term, entry);
    }
    Ok(dict)
}

fn term(
    body: &[u8],
    n_docs: usize,
    i: &mut usize,
) -> std::io::Result<(String, (usize, Vec<usize>))> {
    if *i + 2 > body.len() {
        return Err(corrupt("term len truncated"));
    }
    let tlen = u16::from_le_bytes(body[*i..*i + 2].try_into().unwrap()) as usize;
    *i += 2;
    if *i + tlen + 4 > body.len() {
        return Err(corrupt("term overflow"));
    }
    let term = String::from_utf8_lossy(&body[*i..*i + tlen]).into_owned();
    *i += tlen;
    let df = u32::from_le_bytes(body[*i..*i + 4].try_into().unwrap()) as usize;
    *i += 4;
    if df > body.len() {
        return Err(corrupt("df exceeds body"));
    }
    let mut post = Vec::with_capacity(df);
    let mut prev = 0usize;
    for _ in 0..df {
        let d = get(body, i).ok_or_else(|| corrupt("posting varint"))?;
        let doc = prev + d as usize;
        if doc >= n_docs {
            return Err(corrupt("doc idx overflow"));
        }
        post.push(doc);
        prev = doc;
    }
    Ok((term, (df, post)))
}

/// 解析字节切片为内存索引（纯函数，输入来自 fs::read）。
pub(crate) fn parse(buf: &[u8]) -> std::io::Result<Index> {
    let body = open(buf)?;
    let mut i = 0usize;
    let (n_docs, total_len, n_terms) = header(body, &mut i)?;
    let (ids, docs) = scan(body, n_docs, &mut i)?;
    let dict = dictionary(body, n_terms, n_docs, &mut i)?;
    Ok(Index {
        n_docs,
        total_len,
        ids,
        docs,
        dict,
        entities: OnceLock::new(),
    })
}

fn header(body: &[u8], i: &mut usize) -> std::io::Result<(usize, usize, usize)> {
    let n_docs = word(body, i).ok_or_else(|| corrupt("no n_docs"))? as usize;
    if n_docs > body.len() {
        return Err(corrupt("n_docs exceeds body"));
    }
    if *i + 8 > body.len() {
        return Err(corrupt("no total_len"));
    }
    let total_len = u64::from_le_bytes(body[*i..*i + 8].try_into().unwrap()) as usize;
    *i += 8;
    let n_terms = word(body, i).ok_or_else(|| corrupt("no n_terms"))? as usize;
    if n_terms > body.len() {
        return Err(corrupt("n_terms exceeds body"));
    }
    Ok((n_docs, total_len, n_terms))
}

/// 候选：query 词命中 doc 的并集（去重、按 doc_idx 升序 → 确定性）。
/// 返回 doc_idx（配合 `idx.docs[di]` 零 case 文件 IO 打分）。
pub(crate) fn candidates(idx: &Index, qterms: &[String]) -> Vec<usize> {
    let mut seen: Vec<bool> = vec![false; idx.n_docs];
    for t in qterms {
        if let Some((_, post)) = idx.dict.get(t) {
            for &di in post {
                seen[di] = true;
            }
        }
    }
    seen.iter()
        .enumerate()
        .filter_map(|(di, &hit)| hit.then_some(di))
        .collect()
}

/// term 的全库 df（未收录 = 0）。
pub(crate) fn df(idx: &Index, t: &str) -> usize {
    idx.dict.get(t).map(|(df, _)| *df).unwrap_or(0)
}

/// case id → doc 下标（`ids` 已按字典序，二分查找）。
pub(crate) fn find(idx: &Index, id: &str) -> Option<usize> {
    idx.ids.binary_search_by(|x| x.as_ref().cmp(id)).ok()
}

/// query 实体命中的兄弟 doc（去重后按 doc_idx 升序），并给出共享实体数。
pub(crate) fn neighbors(idx: &Index, ents: &[String]) -> Vec<(usize, usize)> {
    let mut keys: Vec<&str> = ents.iter().map(String::as_str).collect();
    keys.sort_unstable();
    keys.dedup();
    let mut shared = vec![0usize; idx.n_docs];
    for key in keys {
        if let Some(docs) = idx.entities().get(key) {
            for &di in docs {
                shared[di] += 1;
            }
        }
    }
    shared
        .into_iter()
        .enumerate()
        .filter_map(|(di, n)| (n > 0).then_some((di, n)))
        .collect()
}

impl Index {
    fn entities(&self) -> &HashMap<String, Vec<usize>> {
        self.entities.get_or_init(|| entities(&self.docs))
    }
}

/// 是否启用倒排路径：规模达标（meta 可覆盖）或 --rebuild 后强制（index_force=1）。
pub(crate) fn enabled(meta: &Meta) -> bool {
    let force = meta.param("index_force") > 0.0;
    force || meta.case_count >= INDEX_MIN_DOCS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ingest::ingest;
    use crate::memory::store::{self, Stats};
    use crate::memory::util::root;
    use std::fs;

    fn stats() -> Stats {
        crate::memory::store::sample()
    }

    #[test]
    fn crc_and_varint_are_deterministic() {
        let a = checksum(b"hello world");
        assert_eq!(a, checksum(b"hello world"));
        assert_ne!(a, checksum(b"hello worle"));
        let mut buf = Vec::new();
        put(&mut buf, 0);
        put(&mut buf, 127);
        put(&mut buf, 128);
        put(&mut buf, 300);
        put(&mut buf, 1 << 35);
        let mut i = 0;
        assert_eq!(get(&buf, &mut i), Some(0));
        assert_eq!(get(&buf, &mut i), Some(127));
        assert_eq!(get(&buf, &mut i), Some(128));
        assert_eq!(get(&buf, &mut i), Some(300));
        assert_eq!(get(&buf, &mut i), Some(1 << 35));
        assert_eq!(get(&buf, &mut i), None, "越界返回 None");
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
        let root = root("rt");
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
        assert!(
            idx.docs.iter().all(|d| d.updated_s > 0),
            "索引加载时必须一次解析 updated_at，热循环不再逐候选解析日期"
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
        let entities = neighbors(&idx, &["conn_id=9f2a".to_string()]);
        assert_eq!(entities.len(), 1, "实体倒排应直接来自 doc 快照");
        assert_eq!(entities[0].1, 1, "共享实体数必须参与实体加权");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn candidates_union_dedup_sorted() {
        let root = root("cand");
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
        let root = root("cor");
        seed(&root);
        build(&root).unwrap();
        let p = postings(&root);
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
        let root = root("inv");
        seed(&root);
        build(&root).unwrap();
        assert!(postings(&root).exists());
        invalidate(&root).unwrap();
        assert!(!postings(&root).exists());
        assert!(load(&root).unwrap().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_case_invalidates_cached_index() {
        let root = root("wcinv");
        seed(&root);
        build(&root).unwrap();
        assert!(postings(&root).exists());
        let c = store::Case::new(
            "newcase".to_string(),
            "new query".to_string(),
            vec![],
            "ERROR new evidence".to_string(),
        );
        store::save(&root, &c).unwrap();
        assert!(
            !postings(&root).exists(),
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
