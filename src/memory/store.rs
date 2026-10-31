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
pub(crate) use crate::memory::meta::{ensure, Meta};
#[cfg(test)]
pub(crate) use crate::memory::util::fnv1a;
pub(crate) use crate::memory::util::{id, parts, stamp};
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
pub struct Stats {
    pub kept: usize,
    pub total: usize,
    pub cut: f64,
    pub risk: String,
}

/// 测试夹具：与生产无关的固定统计形状；集中一处避免各模块复制。
#[cfg(test)]
pub(crate) fn sample() -> Stats {
    Stats {
        kept: 5,
        total: 100,
        cut: 95.0,
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
    pub stats: Stats,
    pub forces: Vec<String>,
    pub feedback: Feedback,
    pub quality: f64,
    /// 记忆分层：0=原始（L0，仅摄取）、1=候选（L1，证据完整）、
    /// 2=可信（L2，反馈累积证明）。旧 case 缺键兼容为 0。
    pub tier: u8,
    /// 最近一次晋升时间（UTC Unix 秒，字符串）。L0 或未晋升时为空。
    pub tier_at: String,
    /// 冲突互链：近重复 case id 列表（P2 仲裁不删除数据，双向挂链）。
    /// 旧 case 缺键兼容为空。
    pub related: Vec<String>,
    /// 生命周期版本：普通摄取=1；revise 继承旧 case 版本 +1。
    pub version: u32,
    /// 生命周期状态：`active | revised | retired`。命名与反馈状态机
    /// `status`（open/solved/regressed）区分，plan 稿的 status 实施为 life。
    /// 旧 case 缺键兼容为 active。
    pub life: String,
    /// revise 时被替换的旧 case id；普通摄取为空。
    pub supersedes: String,
    /// 被新版替换时的替换者 id；active 时为空。
    pub superseded_by: String,
    /// retire 原因（tombstone 保留审计）；仅 retired 非空。
    pub retire_reason: String,
}

impl Case {
    pub fn new(id: String, query: String, terms: Vec<String>, text: String) -> Case {
        let now = stamp();
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
            stats: Stats {
                kept: 0,
                total: 0,
                cut: 0.0,
                risk: "LOW".to_string(),
            },
            forces: Vec::new(),
            feedback: Feedback { good: 0, bad: 0 },
            quality: 0.0,
            tier: 0,
            tier_at: String::new(),
            related: Vec::new(),
            version: 1,
            life: "active".to_string(),
            supersedes: String::new(),
            superseded_by: String::new(),
            retire_reason: String::new(),
        }
    }
}

// ---- case 读写（JSON 子集）----

pub(crate) fn path(root: &str, id: &str) -> PathBuf {
    Path::new(root).join("cases").join(id).join("case.json")
}

/// 将 Case 序列化为 Val（自产自销 schema；排序固定，字节稳定）。
pub(crate) fn encode(c: &Case) -> Val {
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
        ("tier".into(), Val::Num(c.tier as f64)),
        ("tier_at".into(), Val::Str(c.tier_at.clone())),
        (
            "related".into(),
            Val::Arr(c.related.iter().map(|r| Val::Str(r.clone())).collect()),
        ),
        ("version".into(), Val::Num(c.version as f64)),
        ("life".into(), Val::Str(c.life.clone())),
        ("supersedes".into(), Val::Str(c.supersedes.clone())),
        ("superseded_by".into(), Val::Str(c.superseded_by.clone())),
        ("retire_reason".into(), Val::Str(c.retire_reason.clone())),
        (
            "stats".into(),
            Val::Obj(vec![
                ("kept".into(), Val::Num(c.stats.kept as f64)),
                ("total".into(), Val::Num(c.stats.total as f64)),
                ("cut_pct".into(), Val::Num(c.stats.cut)),
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
fn string(v: &Val) -> String {
    match v {
        Val::Str(s) => s.clone(),
        _ => String::new(),
    }
}
fn number(v: &Val) -> f64 {
    match v {
        Val::Num(n) => *n,
        _ => 0.0,
    }
}
fn strings(v: &Val) -> Vec<String> {
    match v {
        Val::Arr(items) => items.iter().map(string).collect(),
        _ => Vec::new(),
    }
}

/// tier 字段容错解析：缺失/越界一律回退 L0（旧 case 兼容），只接受 0/1/2。
fn tier(v: Option<&Val>) -> u8 {
    match v {
        Some(Val::Num(n)) => {
            let t = n.round();
            if (0.0..=2.0).contains(&t) {
                t as u8
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// 生命周期状态 → 单字节编码（0=active,1=revised,2=retired；未知回退 0）。
/// 供 postings.bin 快照使用，与 decode 的缺键兼容语义同源。
pub(crate) fn life_u8(s: &str) -> u8 {
    if s == "revised" {
        1
    } else if s == "retired" {
        2
    } else {
        0
    }
}

/// 从 Val 反解 Case（未知/缺失键容错为默认值；schema 版本不符仍尽力读取）。
pub(crate) fn decode(v: &Val) -> Case {
    let obj = match v {
        Val::Obj(o) => o,
        _ => return Case::new("bad".into(), String::new(), Vec::new(), String::new()),
    };
    let id = get(obj, "id").map(string).unwrap_or_default();
    let query = get(obj, "query").map(string).unwrap_or_default();
    let terms = get(obj, "terms").map(strings).unwrap_or_default();
    let text = get(obj, "text").map(string).unwrap_or_default();
    let mut c = Case::empty(id, query, terms, text);
    c.created_at = get(obj, "created_at").map(string).unwrap_or_default();
    c.updated_at = get(obj, "updated_at").map(string).unwrap_or_default();
    c.fp = get(obj, "fp").map(string).unwrap_or_default();
    // fix 与原始证据字段缺键时兼容旧 case。
    c.fix = get(obj, "fix").map(string).unwrap_or_default();
    c.evidence_lno = get(obj, "evidence_lno")
        .map(|v| match v {
            Val::Arr(items) => items.iter().map(|it| number(it) as usize).collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    c.evidence = get(obj, "evidence").map(strings).unwrap_or_default();
    c.source = get(obj, "source").map(strings).unwrap_or_default();
    c.tags = get(obj, "tags").map(strings).unwrap_or_default();
    c.bucket = get(obj, "bucket").map(string).unwrap_or_default();
    c.status = get(obj, "status")
        .map(string)
        .unwrap_or_else(|| "open".to_string());
    c.tier = tier(get(obj, "tier"));
    c.tier_at = get(obj, "tier_at").map(string).unwrap_or_default();
    c.related = get(obj, "related").map(strings).unwrap_or_default();
    c.version = number(get(obj, "version").unwrap_or(&Val::Num(1.0))) as u32;
    c.life = get(obj, "life")
        .map(string)
        .unwrap_or_else(|| "active".to_string());
    c.supersedes = get(obj, "supersedes").map(string).unwrap_or_default();
    c.superseded_by = get(obj, "superseded_by").map(string).unwrap_or_default();
    c.retire_reason = get(obj, "retire_reason").map(string).unwrap_or_default();
    c.forces = get(obj, "forces").map(strings).unwrap_or_default();
    if let Some(Val::Obj(f)) = get(obj, "fields") {
        c.fields.error_code = get(f, "error_code").map(strings).unwrap_or_default();
        c.fields.file_loc = get(f, "file_loc").map(strings).unwrap_or_default();
        c.fields.symbol = get(f, "symbol").map(strings).unwrap_or_default();
        c.fields.num_fp = get(f, "num_fp").map(strings).unwrap_or_default();
    }
    if let Some(Val::Obj(s)) = get(obj, "stats") {
        c.stats.kept = number(get(s, "kept").unwrap_or(&Val::Num(0.0))) as usize;
        c.stats.total = number(get(s, "total").unwrap_or(&Val::Num(0.0))) as usize;
        c.stats.cut = number(get(s, "cut_pct").unwrap_or(&Val::Num(0.0)));
        c.stats.risk = get(s, "risk")
            .map(string)
            .unwrap_or_else(|| "LOW".to_string());
    }
    if let Some(Val::Obj(f)) = get(obj, "feedback") {
        c.feedback.good = number(get(f, "good").unwrap_or(&Val::Num(0.0))) as usize;
        c.feedback.bad = number(get(f, "bad").unwrap_or(&Val::Num(0.0))) as usize;
    }
    c.quality = number(get(obj, "quality").unwrap_or(&Val::Num(0.0)));
    c
}

pub(crate) use crate::memory::lock::atomic;

/// 单 case 读改写锁：rate / 去重刷新 / delete 必须先持有同一把锁，
/// 否则并发反馈会互相覆盖（最后写入者的旧快照胜出）。
pub(crate) fn lock(root: &str, id: &str) -> std::io::Result<crate::memory::lock::Lock> {
    if !valid(id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid case id: {id}"),
        ));
    }
    crate::memory::lock::acquire(root, &format!("cases/{id}.lock"))
}

/// 写一个 case 到库（原子写 case.json）。
/// 每次写入推进库代际，使 qmap 与会话缓存失效，保证新 case/反馈对同 query 可见。
pub(crate) fn save(root: &str, c: &Case) -> std::io::Result<()> {
    if !valid(&c.id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid case id: {}", c.id),
        ));
    }
    // 索引锁覆盖“写 case → 删旧索引 → bump gen”整个窗口：索引构建要么看到
    // 写前状态并随后被失效，要么看到写后状态；不能在新 gen 下写回旧索引。
    let _idx = crate::memory::index::lock(root)?;
    let dir = Path::new(root).join("cases").join(&c.id);
    crate::memory::lock::mkdir(&dir).map_err(|e| {
        std::io::Error::new(e.kind(), format!("create case dir {}: {e}", dir.display()))
    })?;
    let mut json_txt = json::write(&encode(c));
    json_txt.push('\n');
    atomic(&dir.join("case.json"), json_txt.as_bytes())
        .map_err(|e| std::io::Error::new(e.kind(), format!("write case {}: {e}", c.id)))?;
    // 索引是缓存：case 新增/更新后必须失效，否则大库会继续用旧 postings
    // 漏掉新 case（小库走线性路径看不出来）。
    crate::memory::index::clear(root);
    crate::memory::meta::bump(root)
}

/// case id 合法性：白名单字符集，禁路径分隔符与 `..` 逃逸。
/// 任何以 id 拼接路径的入口（--delete/--rate/gc）必须先过此校验。
pub(crate) fn valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !id.contains("..")
}

/// 读一个 case。
pub(crate) fn load(root: &str, id: &str) -> std::io::Result<Case> {
    if !valid(id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid case id: {id}"),
        ));
    }
    let p = path(root, id);
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
    let mut c = decode(&v);
    // 路径 id 是资产身份的唯一所有者；旧文件缺 id 或内容被篡改时也不能让
    // 同一份 case 在 qmap/去重索引里冒充另一条记录。
    if c.id != id {
        c.id = id.to_string();
    }
    Ok(c)
}

/// 库内全部 case id（按目录列出，确定性排序）。
pub(crate) fn list(root: &str) -> std::io::Result<Vec<String>> {
    let dir = Path::new(root).join("cases");
    let mut ids = Vec::new();
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            if let Some(name) = e.file_name().to_str() {
                if valid(name) && path(root, name).exists() {
                    ids.push(name.to_string());
                }
            }
        }
    }
    ids.sort();
    Ok(ids)
}

/// 库总字节数（cases + meta + index）。用于写入前的软配额检查。
pub(crate) fn bytes(root: &str) -> std::io::Result<u64> {
    let root = Path::new(root);
    if root.exists() {
        size(root)
    } else {
        Ok(0)
    }
}

/// 目录递归字节数；符号链接按文件元数据计，不跟随目录链接。
pub(crate) fn size(dir: &Path) -> std::io::Result<u64> {
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

/// 重算并持久化 case_count；已持 `meta.lock` 时调用。
pub(crate) fn recount(root: &str) -> std::io::Result<()> {
    // 与 meta::advance 共用 meta 锁：case_count 是从目录重算的，但写回必须串行，
    // 否则并发的 rename 会互相覆盖/报错。
    let n = list(root)?.len();
    let mut meta = crate::memory::meta::load(root)?;
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
    atomic(&p, body.as_bytes())?;
    Ok(())
}

/// 取 `meta.lock` 后重算 case_count。
pub(crate) fn bump(root: &str) -> std::io::Result<()> {
    let _g = crate::memory::lock::acquire(root, "meta.lock")?;
    recount(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let root = std::env::temp_dir().join(format!("scx-mem-test-{}", fnv1a("t1") & 0xffff));
        let root = root.to_str().unwrap();
        let _ = fs::remove_dir_all(root);
        ensure(root).unwrap();

        let now = stamp();
        let text = "ERROR db timeout conn_id=9f2a\nat Pool.java:214\nRoot cause: pool size";
        let id = id(&now, "数据库连接超时 conn_id", text);
        let mut c = Case::new(
            id.clone(),
            "数据库连接超时 conn_id".to_string(),
            vec!["conn_id".to_string()],
            text.to_string(),
        );
        c.stats = Stats {
            kept: 3,
            total: 200,
            cut: 94.2,
            risk: "HIGH".to_string(),
        };
        c.status = "solved".to_string();
        c.feedback = Feedback { good: 2, bad: 0 };
        c.quality = 1.0;
        c.fix = "enlarge pool to 64 conns".to_string();

        save(root, &c).unwrap();
        let loaded = load(root, &id).unwrap();
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

        let ids = list(root).unwrap();
        assert_eq!(ids, vec![id]);
        let _ = fs::remove_dir_all(root);
    }

    /// P3 兼容：老库 case.json 缺 version/life 键 → 解码为 version=1/life=active。
    #[test]
    fn decode_missing_version_and_life_keys_fall_back() {
        let root = std::env::temp_dir().join(format!(
            "scx-mem-legacy-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = root.to_str().unwrap().to_string();
        ensure(&root).unwrap();
        let id = "2026-10-22_legacy01";
        let mut c = Case::new(id.into(), "q".into(), vec![], "t".into());
        c.stats = Stats {
            kept: 1,
            total: 1,
            cut: 0.0,
            risk: "LOW".to_string(),
        };
        let mut obj = match crate::memory::store::encode(&c) {
            crate::memory::json::Val::Obj(kvs) => kvs,
            _ => unreachable!("encode 必产出 Obj"),
        };
        obj.retain(|(k, _)| {
            ![
                "version",
                "life",
                "supersedes",
                "superseded_by",
                "retire_reason",
            ]
            .contains(&k.as_str())
        });
        let case_path = path(&root, id);
        fs::create_dir_all(case_path.parent().unwrap()).unwrap();
        fs::write(
            case_path,
            crate::memory::json::write(&crate::memory::json::Val::Obj(obj)),
        )
        .unwrap();
        let loaded = load(&root, id).unwrap();
        assert_eq!(loaded.version, 1, "缺 version 键回退 1");
        assert_eq!(loaded.life, "active", "缺 life 键回退 active");
        assert!(loaded.supersedes.is_empty());
        assert!(loaded.superseded_by.is_empty());
        assert!(loaded.retire_reason.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn case_id_is_deterministic_and_date_prefixed() {
        let a = id("2026-10-22T10:30:00Z", "q", "t");
        let b = id("2026-10-22T10:30:00Z", "q", "t");
        assert_eq!(a, b);
        assert!(a.starts_with("2026-10-22_"));
        let c = id("2026-10-22T10:30:00Z", "q2", "t");
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
        ensure(&root).unwrap();

        for bad in ["", "..", "../escape", "a/b", "a\\b", "with space"] {
            assert!(load(&root, bad).is_err(), "load must reject {bad:?}");
            let c = Case::new(bad.to_string(), "q".into(), vec![], "t".into());
            assert!(save(&root, &c).is_err(), "save must reject {bad:?}");
        }
        // 外来的非法目录即使含 case.json 也不能进入库枚举。
        let foreign = Path::new(&root).join("cases").join("with space");
        fs::create_dir_all(&foreign).unwrap();
        fs::write(foreign.join("case.json"), b"{}").unwrap();
        assert!(list(&root).unwrap().is_empty());

        let valid = Case::new("ok_1".into(), "q".into(), vec![], "t".into());
        save(&root, &valid).unwrap();
        assert_eq!(list(&root).unwrap(), vec!["ok_1"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_uses_the_validated_path_identity() {
        let root = crate::memory::util::root("case-id");
        ensure(&root).unwrap();
        let mut c = Case::new("expected_id".into(), "q".into(), vec![], "t".into());
        save(&root, &c).unwrap();

        c.id = "tampered_id".into();
        fs::write(path(&root, "expected_id"), json::write(&encode(&c))).unwrap();
        assert_eq!(load(&root, "expected_id").unwrap().id, "expected_id");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_rejects_an_oversized_asset() {
        let root = crate::memory::util::root("case-size");
        ensure(&root).unwrap();
        let id = "oversized";
        let dir = Path::new(&root).join("cases").join(id);
        fs::create_dir_all(&dir).unwrap();
        let file = fs::File::create(path(&root, id)).unwrap();
        file.set_len(MAX_CASE_JSON + 1).unwrap();
        let err = load(&root, id).unwrap_err();
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
        ensure(&root).unwrap();
        let mut handles = Vec::new();
        for t in 0..8 {
            let root = root.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..4 {
                    let id = format!("c{t}_{i}");
                    let c = Case::new(id.clone(), format!("q{t}_{i}"), vec![], "text".into());
                    save(&root, &c).unwrap();
                    bump(&root).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(list(&root).unwrap().len(), 32, "32 个 case 全部落盘");
        let m = crate::memory::meta::load(&root).unwrap();
        assert_eq!(m.case_count, 32, "case_count 不能丢更新");
        assert_eq!(m.gen, 32, "每次 save 的 gen 递增都必须保留");
        let _ = fs::remove_dir_all(&root);
    }
}
