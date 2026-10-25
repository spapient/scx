//! 存储层：case.json 的读写与目录布局。
//!
//! 布局：
//! ```text
//! <root>/
//! ├── meta.toml              # meta.rs：schema 版本、引擎版本、case 数、代际、参数
//! ├── cases/<id>/
//! │   └── case.json          # 文档 + 元数据（序列化在本文件）
//! └── index/                 # 可丢弃的索引缓存
//! ```
//!
//! 原则：cases/ 是资产（可迁移），index/ 是可丢弃缓存（rebuild 重建）。
//! case 数据模型、JSON 序列化、读写与库内枚举由本文件统一拥有；哈希/时间在
//! util.rs，meta 在 meta.rs。

use crate::memory::json::{self, Val};
pub(crate) use crate::memory::meta::SCHEMA;
pub(crate) use crate::memory::meta::{ensure_root, load_meta, Meta};
#[cfg(test)]
pub(crate) use crate::memory::util::fnv1a;
pub(crate) use crate::memory::util::{case_id, fnv1a_parts, utc_now};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// `case.json` 读取上限：正文 1 MiB 加上证据副本与最坏 JSON 转义后仍有余量，
/// 同时把损坏/恶意文件的读取内存封顶，避免无界 `read_to_string`。
pub(crate) const MAX_CASE_JSON: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Fields {
    pub error_code: Vec<String>,
    pub file_loc: Vec<String>,
    pub symbol: Vec<String>,
    pub num_fp: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseStats {
    pub kept: usize,
    pub total: usize,
    pub cut_pct: f64,
    pub risk: String,
}

/// 测试夹具：与生产无关的固定统计形状；集中一处避免各模块复制。
#[cfg(test)]
pub(crate) fn sample_stats() -> CaseStats {
    CaseStats {
        kept: 5,
        total: 100,
        cut_pct: 95.0,
        risk: "HIGH".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Feedback {
    pub good: usize,
    pub bad: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub query: String,
    pub terms: Vec<String>,
    pub text: String,
    pub fix: String,
    pub evidence_lno: Vec<usize>,
    pub evidence: Vec<String>,
    pub fp: String,
    pub source: Vec<String>,
    pub tags: Vec<String>,
    pub bucket: String,
    pub fields: Fields,
    pub status: String,
    pub stats: CaseStats,
    pub forces: Vec<String>,
    pub feedback: Feedback,
    pub quality: f64,
}

impl Case {
    pub fn new(id: String, query: String, terms: Vec<String>, text: String) -> Case {
        let now = utc_now();
        let mut c = Self::empty(id, query, terms, text);
        c.created_at = now.clone();
        c.updated_at = now;
        c
    }

    /// 反序列化专用空壳：不读取时钟，时间字段由 `case.json` 覆盖。
    fn empty(id: String, query: String, terms: Vec<String>, text: String) -> Case {
        Case {
            id,
            created_at: String::new(),
            updated_at: String::new(),
            query,
            terms,
            text,
            fix: String::new(),
            evidence_lno: Vec::new(),
            evidence: Vec::new(),
            fp: String::new(),
            source: Vec::new(),
            tags: Vec::new(),
            bucket: String::new(),
            fields: Fields::default(),
            status: "open".to_string(),
            stats: CaseStats {
                kept: 0,
                total: 0,
                cut_pct: 0.0,
                risk: "LOW".to_string(),
            },
            forces: Vec::new(),
            feedback: Feedback { good: 0, bad: 0 },
            quality: 0.0,
        }
    }
}

// ---- case 读写（JSON 子集）----

pub(crate) fn case_path(root: &str, id: &str) -> PathBuf {
    Path::new(root).join("cases").join(id).join("case.json")
}

/// 将 Case 序列化为 Val（自产自销 schema；排序固定，字节稳定）。
pub(crate) fn case_to_val(c: &Case) -> Val {
    Val::Obj(vec![
        ("schema".into(), Val::Num(SCHEMA as f64)),
        ("id".into(), Val::Str(c.id.clone())),
        ("created_at".into(), Val::Str(c.created_at.clone())),
        ("updated_at".into(), Val::Str(c.updated_at.clone())),
        ("query".into(), Val::Str(c.query.clone())),
        (
            "terms".into(),
            Val::Arr(c.terms.iter().map(|t| Val::Str(t.clone())).collect()),
        ),
        ("text".into(), Val::Str(c.text.clone())),
        ("fix".into(), Val::Str(c.fix.clone())),
        (
            "evidence_lno".into(),
            Val::Arr(c.evidence_lno.iter().map(|n| Val::Num(*n as f64)).collect()),
        ),
        (
            "evidence".into(),
            Val::Arr(c.evidence.iter().map(|s| Val::Str(s.clone())).collect()),
        ),
        ("fp".into(), Val::Str(c.fp.clone())),
        (
            "source".into(),
            Val::Arr(c.source.iter().map(|s| Val::Str(s.clone())).collect()),
        ),
        (
            "tags".into(),
            Val::Arr(c.tags.iter().map(|t| Val::Str(t.clone())).collect()),
        ),
        ("bucket".into(), Val::Str(c.bucket.clone())),
        (
            "fields".into(),
            Val::Obj(vec![
                (
                    "error_code".into(),
                    Val::Arr(
                        c.fields
                            .error_code
                            .iter()
                            .map(|s| Val::Str(s.clone()))
                            .collect(),
                    ),
                ),
                (
                    "file_loc".into(),
                    Val::Arr(
                        c.fields
                            .file_loc
                            .iter()
                            .map(|s| Val::Str(s.clone()))
                            .collect(),
                    ),
                ),
                (
                    "symbol".into(),
                    Val::Arr(
                        c.fields
                            .symbol
                            .iter()
                            .map(|s| Val::Str(s.clone()))
                            .collect(),
                    ),
                ),
                (
                    "num_fp".into(),
                    Val::Arr(
                        c.fields
                            .num_fp
                            .iter()
                            .map(|s| Val::Str(s.clone()))
                            .collect(),
                    ),
                ),
            ]),
        ),
        ("status".into(), Val::Str(c.status.clone())),
        (
            "stats".into(),
            Val::Obj(vec![
                ("kept".into(), Val::Num(c.stats.kept as f64)),
                ("total".into(), Val::Num(c.stats.total as f64)),
                ("cut_pct".into(), Val::Num(c.stats.cut_pct)),
                ("risk".into(), Val::Str(c.stats.risk.clone())),
            ]),
        ),
        (
            "forces".into(),
            Val::Arr(c.forces.iter().map(|s| Val::Str(s.clone())).collect()),
        ),
        (
            "feedback".into(),
            Val::Obj(vec![
                ("good".into(), Val::Num(c.feedback.good as f64)),
                ("bad".into(), Val::Num(c.feedback.bad as f64)),
            ]),
        ),
        ("quality".into(), Val::Num(c.quality)),
    ])
}

fn get<'a>(obj: &'a [(String, Val)], k: &str) -> Option<&'a Val> {
    obj.iter().find(|(key, _)| key == k).map(|(_, v)| v)
}
fn as_str(v: &Val) -> String {
    match v {
        Val::Str(s) => s.clone(),
        _ => String::new(),
    }
}
fn as_num(v: &Val) -> f64 {
    match v {
        Val::Num(n) => *n,
        _ => 0.0,
    }
}
fn as_str_arr(v: &Val) -> Vec<String> {
    match v {
        Val::Arr(items) => items.iter().map(as_str).collect(),
        _ => Vec::new(),
    }
}

/// 从 Val 反解 Case（未知/缺失键容错为默认值；schema 版本不符仍尽力读取）。
pub(crate) fn val_to_case(v: &Val) -> Case {
    let obj = match v {
        Val::Obj(o) => o,
        _ => return Case::new("bad".into(), String::new(), Vec::new(), String::new()),
    };
    let id = get(obj, "id").map(as_str).unwrap_or_default();
    let query = get(obj, "query").map(as_str).unwrap_or_default();
    let terms = get(obj, "terms").map(as_str_arr).unwrap_or_default();
    let text = get(obj, "text").map(as_str).unwrap_or_default();
    let mut c = Case::empty(id, query, terms, text);
    c.created_at = get(obj, "created_at").map(as_str).unwrap_or_default();
    c.updated_at = get(obj, "updated_at").map(as_str).unwrap_or_default();
    c.fp = get(obj, "fp").map(as_str).unwrap_or_default();
    // fix 与原始证据字段缺键时兼容旧 case。
    c.fix = get(obj, "fix").map(as_str).unwrap_or_default();
    c.evidence_lno = get(obj, "evidence_lno")
        .map(|v| match v {
            Val::Arr(items) => items.iter().map(|it| as_num(it) as usize).collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    c.evidence = get(obj, "evidence").map(as_str_arr).unwrap_or_default();
    c.source = get(obj, "source").map(as_str_arr).unwrap_or_default();
    c.tags = get(obj, "tags").map(as_str_arr).unwrap_or_default();
    c.bucket = get(obj, "bucket").map(as_str).unwrap_or_default();
    c.status = get(obj, "status")
        .map(as_str)
        .unwrap_or_else(|| "open".to_string());
    c.forces = get(obj, "forces").map(as_str_arr).unwrap_or_default();
    if let Some(Val::Obj(f)) = get(obj, "fields") {
        c.fields.error_code = get(f, "error_code").map(as_str_arr).unwrap_or_default();
        c.fields.file_loc = get(f, "file_loc").map(as_str_arr).unwrap_or_default();
        c.fields.symbol = get(f, "symbol").map(as_str_arr).unwrap_or_default();
        c.fields.num_fp = get(f, "num_fp").map(as_str_arr).unwrap_or_default();
    }
    if let Some(Val::Obj(s)) = get(obj, "stats") {
        c.stats.kept = as_num(get(s, "kept").unwrap_or(&Val::Num(0.0))) as usize;
        c.stats.total = as_num(get(s, "total").unwrap_or(&Val::Num(0.0))) as usize;
        c.stats.cut_pct = as_num(get(s, "cut_pct").unwrap_or(&Val::Num(0.0)));
        c.stats.risk = get(s, "risk")
            .map(as_str)
            .unwrap_or_else(|| "LOW".to_string());
    }
    if let Some(Val::Obj(f)) = get(obj, "feedback") {
        c.feedback.good = as_num(get(f, "good").unwrap_or(&Val::Num(0.0))) as usize;
        c.feedback.bad = as_num(get(f, "bad").unwrap_or(&Val::Num(0.0))) as usize;
    }
    c.quality = as_num(get(obj, "quality").unwrap_or(&Val::Num(0.0)));
    c
}

pub(crate) use crate::memory::lock::atomic_write;

/// 单 case 读改写锁：rate / 去重刷新 / delete 必须先持有同一把锁，
/// 否则并发反馈会互相覆盖（最后写入者的旧快照胜出）。
pub(crate) fn lock_case(root: &str, id: &str) -> std::io::Result<crate::memory::lock::FileLock> {
    if !valid_id(id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid case id: {id}"),
        ));
    }
    crate::memory::lock::acquire(root, &format!("cases/{id}.lock"))
}

/// 写一个 case 到库（原子写 case.json）。
/// 每次写入推进库代际，使 qmap 与会话缓存失效，保证新 case/反馈对同 query 可见。
pub(crate) fn write_case(root: &str, c: &Case) -> std::io::Result<()> {
    if !valid_id(&c.id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid case id: {}", c.id),
        ));
    }
    // 索引锁覆盖“写 case → 删旧索引 → bump gen”整个窗口：索引构建要么看到
    // 写前状态并随后被失效，要么看到写后状态；不能在新 gen 下写回旧索引。
    let _idx = crate::memory::index::lock(root)?;
    let dir = Path::new(root).join("cases").join(&c.id);
    crate::memory::lock::create_dir_all(&dir).map_err(|e| {
        std::io::Error::new(e.kind(), format!("create case dir {}: {e}", dir.display()))
    })?;
    let mut json_txt = json::write(&case_to_val(c));
    json_txt.push('\n');
    atomic_write(&dir.join("case.json"), json_txt.as_bytes())
        .map_err(|e| std::io::Error::new(e.kind(), format!("write case {}: {e}", c.id)))?;
    // 索引是缓存：case 新增/更新后必须失效，否则大库会继续用旧 postings
    // 漏掉新 case（小库走线性路径看不出来）。
    crate::memory::index::invalidate_locked(root);
    crate::memory::meta::bump_gen(root)
}

/// case id 合法性：白名单字符集，禁路径分隔符与 `..` 逃逸。
/// 任何以 id 拼接路径的入口（--delete/--rate/gc）必须先过此校验。
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !id.contains("..")
}

/// 读一个 case。
pub(crate) fn read_case(root: &str, id: &str) -> std::io::Result<Case> {
    if !valid_id(id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid case id: {id}"),
        ));
    }
    let p = case_path(root, id);
    let mut txt = String::new();
    fs::File::open(&p)
        .and_then(|f| f.take(MAX_CASE_JSON + 1).read_to_string(&mut txt))
        .map_err(|e| std::io::Error::new(e.kind(), format!("read case {}: {e}", p.display())))?;
    if txt.len() as u64 > MAX_CASE_JSON {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "case {} too large: {} bytes > {MAX_CASE_JSON}",
                p.display(),
                txt.len()
            ),
        ));
    }
    let v = json::parse(&txt).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("parse case {}: {e}", p.display()),
        )
    })?;
    let mut c = val_to_case(&v);
    // 路径 id 是资产身份的唯一所有者；旧文件缺 id 或内容被篡改时也不能让
    // 同一份 case 在 qmap/去重索引里冒充另一条记录。
    if c.id != id {
        c.id = id.to_string();
    }
    Ok(c)
}

/// 库内全部 case id（按目录列出，确定性排序）。
pub(crate) fn list_ids(root: &str) -> std::io::Result<Vec<String>> {
    let dir = Path::new(root).join("cases");
    let mut ids = Vec::new();
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            if let Some(name) = e.file_name().to_str() {
                if valid_id(name) && case_path(root, name).exists() {
                    ids.push(name.to_string());
                }
            }
        }
    }
    ids.sort();
    Ok(ids)
}

/// 库总字节数（cases + meta + index）。用于写入前的软配额检查。
pub(crate) fn bytes_total(root: &str) -> std::io::Result<u64> {
    let root = Path::new(root);
    if root.exists() {
        dir_bytes(root)
    } else {
        Ok(0)
    }
}

/// 目录递归字节数；符号链接按文件元数据计，不跟随目录链接。
pub(crate) fn dir_bytes(dir: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                total += entry.metadata()?.len();
            }
        }
    }
    Ok(total)
}

/// case_count 递增（新 case 写入后调用）；同时持久化回 meta.toml。
/// 已持 `meta.lock` 时调用（多锁路径用 `acquire_all` 一次取齐后调此变体）。
pub(crate) fn bump_case_count_locked(root: &str) -> std::io::Result<()> {
    // 与 bump_gen 共用 meta 锁：case_count 是从目录重算的，但写回必须串行，
    // 否则并发的 rename 会互相覆盖/报错。
    let n = list_ids(root)?.len();
    let mut meta = load_meta(root)?;
    meta.case_count = n;
    // 重写 meta.toml：保留 df/params 节，仅更新 case_count。
    let p = Path::new(root).join("meta.toml");
    let txt = fs::read_to_string(&p)?;
    let mut out_lines: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in txt.lines() {
        let t = line.trim();
        if t.starts_with("case_count") {
            out_lines.push(format!("case_count = {n}"));
            replaced = true;
        } else {
            out_lines.push(line.to_string());
        }
    }
    if !replaced {
        out_lines.push(format!("case_count = {n}"));
    }
    let mut body = out_lines.join("\n");
    body.push('\n');
    atomic_write(&p, body.as_bytes())?;
    Ok(())
}

/// case_count 递增（包装 `bump_case_count_locked`：取 `meta.lock` 后执行）。
pub(crate) fn bump_case_count(root: &str) -> std::io::Result<()> {
    let _g = crate::memory::lock::acquire(root, "meta.lock")?;
    bump_case_count_locked(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let root = std::env::temp_dir().join(format!("scx-mem-test-{}", fnv1a("t1") & 0xffff));
        let root = root.to_str().unwrap();
        let _ = fs::remove_dir_all(root);
        ensure_root(root).unwrap();

        let now = utc_now();
        let text = "ERROR db timeout conn_id=9f2a\nat Pool.java:214\nRoot cause: pool size";
        let id = case_id(&now, "数据库连接超时 conn_id", text);
        let mut c = Case::new(
            id.clone(),
            "数据库连接超时 conn_id".to_string(),
            vec!["conn_id".to_string()],
            text.to_string(),
        );
        c.stats = CaseStats {
            kept: 3,
            total: 200,
            cut_pct: 94.2,
            risk: "HIGH".to_string(),
        };
        c.status = "solved".to_string();
        c.feedback = Feedback { good: 2, bad: 0 };
        c.quality = 1.0;
        c.fix = "enlarge pool to 64 conns".to_string();

        write_case(root, &c).unwrap();
        let loaded = read_case(root, &id).unwrap();
        assert_eq!(loaded, c, "case 往返必须逐字段一致");
        assert_eq!(loaded.fix, "enlarge pool to 64 conns", "fix 必须往返");
        assert!(
            !Path::new(root)
                .join("cases")
                .join(&id)
                .join("vec.bin")
                .exists(),
            "vec.bin 已移除：无效产物不再落盘"
        );

        let ids = list_ids(root).unwrap();
        assert_eq!(ids, vec![id]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn case_id_is_deterministic_and_date_prefixed() {
        let a = case_id("2026-10-22T10:30:00Z", "q", "t");
        let b = case_id("2026-10-22T10:30:00Z", "q", "t");
        assert_eq!(a, b);
        assert!(a.starts_with("2026-10-22_"));
        let c = case_id("2026-10-22T10:30:00Z", "q2", "t");
        assert_ne!(a, c, "query 变体必须产生不同 id");
    }

    #[test]
    fn invalid_case_ids_cannot_escape_the_cases_directory() {
        let root = std::env::temp_dir().join(format!(
            "scx-mem-id-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = root.to_str().unwrap().to_string();
        ensure_root(&root).unwrap();

        for bad in ["", "..", "../escape", "a/b", "a\\b", "with space"] {
            assert!(
                read_case(&root, bad).is_err(),
                "read_case must reject {bad:?}"
            );
            let c = Case::new(bad.to_string(), "q".into(), vec![], "t".into());
            assert!(
                write_case(&root, &c).is_err(),
                "write_case must reject {bad:?}"
            );
        }
        // 外来的非法目录即使含 case.json 也不能进入库枚举。
        let foreign = Path::new(&root).join("cases").join("with space");
        fs::create_dir_all(&foreign).unwrap();
        fs::write(foreign.join("case.json"), b"{}").unwrap();
        assert!(list_ids(&root).unwrap().is_empty());

        let valid = Case::new("ok_1".into(), "q".into(), vec![], "t".into());
        write_case(&root, &valid).unwrap();
        assert_eq!(list_ids(&root).unwrap(), vec!["ok_1"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn read_case_uses_the_validated_path_identity() {
        let root = crate::memory::util::tmp_root("case-id");
        ensure_root(&root).unwrap();
        let mut c = Case::new("expected_id".into(), "q".into(), vec![], "t".into());
        write_case(&root, &c).unwrap();

        c.id = "tampered_id".into();
        fs::write(
            case_path(&root, "expected_id"),
            json::write(&case_to_val(&c)),
        )
        .unwrap();
        assert_eq!(read_case(&root, "expected_id").unwrap().id, "expected_id");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn read_case_rejects_an_oversized_asset() {
        let root = crate::memory::util::tmp_root("case-size");
        ensure_root(&root).unwrap();
        let id = "oversized";
        let dir = Path::new(&root).join("cases").join(id);
        fs::create_dir_all(&dir).unwrap();
        let file = fs::File::create(case_path(&root, id)).unwrap();
        file.set_len(MAX_CASE_JSON + 1).unwrap();
        let err = read_case(&root, id).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("too large"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn concurrent_case_writes_are_lossless() {
        let root = std::env::temp_dir().join(format!(
            "scx-mem-conc-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = root.to_str().unwrap().to_string();
        ensure_root(&root).unwrap();
        let mut handles = Vec::new();
        for t in 0..8 {
            let root = root.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..4 {
                    let id = format!("c{t}_{i}");
                    let c = Case::new(id.clone(), format!("q{t}_{i}"), vec![], "text".into());
                    write_case(&root, &c).unwrap();
                    bump_case_count(&root).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(list_ids(&root).unwrap().len(), 32, "32 个 case 全部落盘");
        let m = load_meta(&root).unwrap();
        assert_eq!(m.case_count, 32, "case_count 不能丢更新");
        assert_eq!(m.gen, 32, "每次 write_case 的 gen 递增都必须保留");
        let _ = fs::remove_dir_all(&root);
    }
}
