//! 存储层：case.json 的读写与目录布局。
//!
//! 布局（与设计 v0.2 一致；P4.4 评审处置移除 vec.bin——无效产物不再落盘）：
//! ```text
//! <root>/
//! ├── meta.toml              # meta.rs：schema 版本、引擎版本、case 数、代际、参数
//! ├── cases/<id>/
//! │   └── case.json          # 文档 + 元数据（序列化在本文件）
//! └── index/                 # 索引缓存（P3 起）
//! ```
//!
//! 原则：cases/ 是资产（可迁移），index/ 是可丢弃缓存（rebuild 重建）。
//! 数据模型在 model.rs、哈希/时间在 util.rs、meta 在 meta.rs；本文件只负责
//! case 的 JSON 序列化、读写与库内 case 枚举。

#![cfg_attr(not(test), allow(dead_code))]

use crate::memory::json::{self, Val};
pub(crate) use crate::memory::meta::{ensure_root, load_meta, Meta};
pub(crate) use crate::memory::model::SCHEMA;
pub use crate::memory::model::{Case, CaseStats, Feedback, Fields};
pub(crate) use crate::memory::util::{case_id, fnv1a, utc_now};
use std::fs;
use std::path::{Path, PathBuf};

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
        Val::Obj(o) => o.clone(),
        _ => return Case::new("bad".into(), String::new(), Vec::new(), String::new()),
    };
    let id = get(&obj, "id").map(as_str).unwrap_or_default();
    let query = get(&obj, "query").map(as_str).unwrap_or_default();
    let terms = get(&obj, "terms").map(as_str_arr).unwrap_or_default();
    let text = get(&obj, "text").map(as_str).unwrap_or_default();
    let mut c = Case::new(id, query, terms, text);
    c.created_at = get(&obj, "created_at").map(as_str).unwrap_or_default();
    c.updated_at = get(&obj, "updated_at").map(as_str).unwrap_or_default();
    c.fp = get(&obj, "fp").map(as_str).unwrap_or_default();
    c.fix = get(&obj, "fix").map(as_str).unwrap_or_default(); // 缺键兼容旧 case
                                                              // P4.4+ 优化⑤：原始证据（缺键兼容旧 case——仅压缩结果快照，语义见 §11 评审④）。
    c.evidence_lno = get(&obj, "evidence_lno")
        .map(|v| match v {
            Val::Arr(items) => items.iter().map(|it| as_num(it) as usize).collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    c.evidence = get(&obj, "evidence").map(as_str_arr).unwrap_or_default();
    c.source = get(&obj, "source").map(as_str_arr).unwrap_or_default();
    c.tags = get(&obj, "tags").map(as_str_arr).unwrap_or_default();
    c.bucket = get(&obj, "bucket").map(as_str).unwrap_or_default();
    c.status = get(&obj, "status")
        .map(as_str)
        .unwrap_or_else(|| "open".to_string());
    c.forces = get(&obj, "forces").map(as_str_arr).unwrap_or_default();
    if let Some(Val::Obj(f)) = get(&obj, "fields") {
        c.fields.error_code = get(f, "error_code").map(as_str_arr).unwrap_or_default();
        c.fields.file_loc = get(f, "file_loc").map(as_str_arr).unwrap_or_default();
        c.fields.symbol = get(f, "symbol").map(as_str_arr).unwrap_or_default();
        c.fields.num_fp = get(f, "num_fp").map(as_str_arr).unwrap_or_default();
    }
    if let Some(Val::Obj(s)) = get(&obj, "stats") {
        c.stats.kept = as_num(get(s, "kept").unwrap_or(&Val::Num(0.0))) as usize;
        c.stats.total = as_num(get(s, "total").unwrap_or(&Val::Num(0.0))) as usize;
        c.stats.cut_pct = as_num(get(s, "cut_pct").unwrap_or(&Val::Num(0.0)));
        c.stats.risk = get(s, "risk")
            .map(as_str)
            .unwrap_or_else(|| "LOW".to_string());
    }
    if let Some(Val::Obj(f)) = get(&obj, "feedback") {
        c.feedback.good = as_num(get(f, "good").unwrap_or(&Val::Num(0.0))) as usize;
        c.feedback.bad = as_num(get(f, "bad").unwrap_or(&Val::Num(0.0))) as usize;
    }
    c.quality = as_num(get(&obj, "quality").unwrap_or(&Val::Num(0.0)));
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
/// P4.4 评审处置：不再生成 vec.bin（实际始终以空稀疏写入、召回从未使用，
/// 属无效产物，徒增 I/O 与 schema 维护成本）；每次写 case 视为库状态变化，
/// 自动 bump 库代际（qmap/会话缓存据此失效，保证新 case/反馈对同 query 召回可见）。
pub(crate) fn write_case(root: &str, c: &Case) -> std::io::Result<()> {
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

/// case id 合法性（P4.4 评审处置）：白名单字符集，禁路径分隔符与 `..` 逃逸。
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
    let p = case_path(root, id);
    let txt = fs::read_to_string(p)?;
    let v = json::parse(&txt).map_err(std::io::Error::other)?;
    Ok(val_to_case(&v))
}

/// 库内全部 case id（按目录列出，确定性排序）。
pub(crate) fn list_ids(root: &str) -> std::io::Result<Vec<String>> {
    let dir = Path::new(root).join("cases");
    let mut ids = Vec::new();
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            if let Some(name) = e.file_name().to_str() {
                if case_path(root, name).exists() {
                    ids.push(name.to_string());
                }
            }
        }
    }
    ids.sort();
    Ok(ids)
}

/// case_count 递增（新 case 写入后调用）；同时持久化回 meta.toml。
pub(crate) fn bump_case_count(root: &str) -> std::io::Result<()> {
    // 与 bump_gen 共用 meta 锁：case_count 是从目录重算的，但写回必须串行，
    // 否则并发的 rename 会互相覆盖/报错。
    let _g = crate::memory::lock::acquire(root, "meta.lock")?;
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

// ---- vec.bin（稀疏向量二进制）----
// P4.4 评审处置：vec.bin 始终以空稀疏写入、召回从未使用，属无效产物；
// 读写函数与 VEC_MAGIC 已整体移除，schema 2 兼容读（老库遗留文件被忽略）。

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
