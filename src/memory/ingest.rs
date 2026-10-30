//! 摄取：压缩产物（Outcome）→ 记忆 case。
//!
//! 摄取即过滤：入库的不是原始日志，而是 compress 筛出的高密度证据块。
//! 去重：归一化指纹 `fp = fnv1a(norm(query) ‖ norm(text))`，已存在则只更新
//! updated_at 与反馈相关字段，不新增（设计 v0.2 §6）。
//!
//! 红线：本模块是 I/O 侧（memory），compress 零改动；摄取结果不影响压缩输出，
//! 只沉淀旁路资产。
//!
use crate::memory::store::{self, Case, Stats};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 单 case 文本上限：64 MiB 输入不得直接变 64 MiB case；
/// 超过上限拒绝摄取并明确报错（旁路，不影响压缩主流程）。
pub(crate) const MAX_CASE_TEXT: usize = 1024 * 1024; // 1 MiB

/// 修复结论上限；`--fix` 只保存用户摘要文本。
pub(crate) const MAX_FIX_LEN: usize = 4096; // 4 KiB

const FP_INDEX: &str = "index/fingerprints.tsv";
const FP_READY: &str = "index/fingerprints.ready";
const FP_LOCK: &str = "index/fingerprints.lock";

/// 计算去重指纹（确定性、无随机源）。
///
/// 身份口径：query 保留数字（8080/9090、errno=28/29 是不同事故），text 仍按
/// `normalize` 折叠数字（日志里的 PID/时间戳/行号不应制造重复 case）。
/// 这修复了旧实现"query 数字也被折叠 → 不同端口事故互相去重"的问题。
pub(crate) fn fingerprint(query: &str, text: &str) -> String {
    let mut q = String::new();
    crate::analysis::canonical(query, &mut q);
    let mut t = String::new();
    crate::analysis::normalize(text, &mut t);
    let h = store::parts(&[q.as_bytes(), b"\0", t.as_bytes()]);
    format!("{:016x}", h)
}

fn index(root: &str) -> PathBuf {
    Path::new(root).join(FP_INDEX)
}

fn ready(root: &str) -> PathBuf {
    Path::new(root).join(FP_READY)
}

fn total(root: &str) -> Option<usize> {
    fs::read_to_string(ready(root)).ok()?.trim().parse().ok()
}

/// 惰性构建去重索引：一次性读取旧库 case，之后只看一个小文件。
fn ensure(root: &str) -> io::Result<()> {
    let count = crate::memory::meta::load(root)?.case_count;
    if total(root) == Some(count) && index(root).exists() {
        return Ok(());
    }
    let _lock = crate::memory::lock::acquire(root, FP_LOCK)?;
    let count = crate::memory::meta::load(root)?.case_count;
    if total(root) == Some(count) && index(root).exists() {
        return Ok(());
    }
    rebuild(root)
}

fn rebuild(root: &str) -> io::Result<()> {
    let ids = store::list(root)?;
    let mut body = String::with_capacity(ids.len().saturating_mul(40));
    for id in &ids {
        let c = store::load(root, id)?;
        body.push_str(&fingerprint(&c.query, &c.text));
        body.push('\t');
        body.push_str(id);
        body.push('\n');
    }
    crate::memory::lock::atomic(&index(root), body.as_bytes())?;
    crate::memory::lock::atomic(&ready(root), ids.len().to_string().as_bytes())
}

/// 追加一条索引并把 ready 对齐到**索引实际行数**（不是库 case_count：并发写库时
/// case_count 可能已包含别的进程刚写入、本索引尚无行的 case，拿它会误判完整）。
///
/// 幂等：同一 (fp, id) 若已被并发 rebuild 收录，则不再追加重复行——否则行数会
/// 超过 case 数，ready 与 case_count 永远对不上，触发无谓的全表重建。
fn remember(root: &str, fp: &str, id: &str) -> io::Result<()> {
    let _lock = crate::memory::lock::acquire(root, FP_LOCK)?;
    // ready 缺失（删除后失效/尚未构建）或索引不存在：整表重建。
    if total(root).is_none() {
        return rebuild(root);
    }
    let Some(body) = read(root)? else {
        return rebuild(root);
    };
    let mut lines = body.lines().count();
    let already = body.lines().any(|l| l.split_once('\t') == Some((fp, id)));
    if !already {
        let mut file = fs::OpenOptions::new().append(true).open(index(root))?;
        writeln!(file, "{fp}\t{id}")?;
        lines += 1;
    }
    crate::memory::lock::atomic(&ready(root), lines.to_string().as_bytes())
}

/// case 删除后让去重索引在下一次写入时惰性重建。
pub(crate) fn invalidate(root: &str) {
    if let Err(e) = fs::remove_file(ready(root)) {
        // ready 标记本就可能不存在（首次写入/已失效），NotFound 属预期；
        // 其余失败（权限/占用）需如实告警，否则去重索引可能一直不重建。
        if e.kind() != io::ErrorKind::NotFound {
            eprintln!("[scx] warning: 去重索引作废失败: {e}");
        }
    }
}

/// 按身份指纹查库内是否已存在 case，返回其 id。
/// 索引按动态重算指纹构建，因此旧库无需迁移也能按当前身份口径去重。
pub(crate) fn find(root: &str, fp: &str) -> io::Result<Option<String>> {
    ensure(root)?;
    let Some(body) = read(root)? else {
        return Ok(None);
    };
    for line in body.lines() {
        if let Some(id) = matched(root, line, fp) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn read(root: &str) -> io::Result<Option<String>> {
    match fs::read_to_string(index(root)) {
        Ok(body) => Ok(Some(body)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn matched(root: &str, line: &str, fp: &str) -> Option<String> {
    let (stored, id) = line.split_once('\t')?;
    if stored != fp {
        return None;
    }
    if store::valid(id) && store::path(root, id).exists() {
        return Some(id.to_string());
    }
    invalidate(root);
    None
}

/// 已脱敏、已切词的存储字段。公开门面 `memory::save` 是唯一构造入口；
/// 本结构只负责持久化，避免每种可选字段各复制一套函数。
pub(crate) struct Payload<'a> {
    pub root: &'a str,
    pub query: &'a str,
    pub terms: Vec<String>,
    pub text: &'a str,
    pub source: &'a [String],
    pub stats: &'a Stats,
    pub forces: &'a [String],
    pub fix: &'a str,
    pub evidence_lno: &'a [usize],
    pub evidence: &'a [String],
    pub max_bytes: Option<u64>,
}

pub(crate) fn persist(input: Payload<'_>) -> std::io::Result<(bool, String)> {
    store::ensure(input.root)?;
    check(
        input.root,
        input.max_bytes,
        input.text,
        input.fix,
        input.evidence_lno,
        input.evidence,
    )?;
    let fp = fingerprint(input.query, input.text);
    // 同一指纹的"查重 + 写入"必须串行：两个并发 store 若都在 find 前进入，
    // 会跨秒生成两个 id，或在去重更新时用旧快照覆盖刚写入的反馈。
    let _ingest_lock =
        crate::memory::lock::acquire(input.root, &format!("cases/.ingest-{fp}.lock"))?;
    if let Some(existing) = find(input.root, &fp)? {
        return update(
            input.root,
            &existing,
            input.fix,
            input.evidence_lno,
            input.evidence,
        );
    }
    let id = insert(input, &fp)?;
    Ok((true, id))
}

fn update(
    root: &str,
    existing: &str,
    fix: &str,
    evidence_lno: &[usize],
    evidence: &[String],
) -> std::io::Result<(bool, String)> {
    let _case_lock = store::lock(root, existing)?;
    let mut c = store::load(root, existing)?;
    let now = store::stamp();
    c.updated_at = now;
    backfill(&mut c, fix, evidence_lno, evidence);
    store::save(root, &c)?;
    Ok((false, existing.to_string()))
}

fn insert(fields: Payload<'_>, fp: &str) -> std::io::Result<String> {
    let Payload {
        root,
        query,
        terms,
        text,
        source,
        stats,
        forces,
        fix,
        evidence_lno,
        evidence,
        max_bytes: _,
    } = fields;
    let now = store::stamp();
    let id = store::id(&now, query, text);
    let _case_lock = store::lock(root, &id)?;
    let mut c = Case::new(id.clone(), query.to_string(), terms, text.to_string());
    c.fp = fp.to_string();
    c.fix = fix.to_string();
    c.evidence_lno = evidence_lno.to_vec();
    c.evidence = evidence.to_vec();
    c.source = source.to_vec();
    c.stats = stats.clone();
    c.forces = forces.to_vec();
    // 摄取即抽取：判别字段随 case 写入，召回时无需重扫正文。
    c.fields = crate::memory::classify::extract(text);
    // 摄取即分桶：先用 case query 判家族，query 无证据再回退正文，
    // 避免根因里的宽域词把 query 已明确的场景带偏。
    let fvec = c.fields.error_code.clone();
    let qterms = crate::terms::memory(&c.query);
    c.bucket = crate::memory::classify::query(&fvec, &qterms, text);
    store::save(root, &c)?;
    store::bump(root)?;
    remember(root, fp, &id)?;
    Ok(id)
}

/// 入库前校验：配额 / 文本上限 / fix 上限 / 证据数组长度一致。
fn check(
    root: &str,
    max_bytes: Option<u64>,
    text: &str,
    fix: &str,
    evidence_lno: &[usize],
    evidence: &[String],
) -> std::io::Result<()> {
    if let Some(max) = max_bytes {
        let total = store::bytes(root)?;
        if total > max {
            return Err(std::io::Error::other(format!(
                "memory quota exceeded: {total} bytes > --max-bytes {max}"
            )));
        }
    }
    if text.len() > MAX_CASE_TEXT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "case text too large: {} bytes > {} (scx-memory MAX_CASE_TEXT)",
                text.len(),
                MAX_CASE_TEXT
            ),
        ));
    }
    if fix.len() > MAX_FIX_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "case fix too long: {} bytes > {} (scx-memory MAX_FIX_LEN)",
                fix.len(),
                MAX_FIX_LEN
            ),
        ));
    }
    if evidence_lno.len() != evidence.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "evidence_lno/evidence length mismatch",
        ));
    }
    Ok(())
}

/// 同指纹重复摄取：只补录空的 fix / evidence，已有结论绝不覆盖。
fn backfill(c: &mut Case, fix: &str, evidence_lno: &[usize], evidence: &[String]) {
    if c.fix.is_empty() && !fix.is_empty() {
        c.fix = fix.to_string();
    }
    if c.evidence.is_empty() && !evidence.is_empty() {
        c.evidence_lno = evidence_lno.to_vec();
        c.evidence = evidence.to_vec();
    }
}

/// 测试便捷入口：直接构造已切词字段，覆盖不带 fix/evidence 的旧调用形态。
#[cfg(test)]
pub(crate) fn ingest(
    root: &str,
    query: &str,
    terms: Vec<String>,
    text: &str,
    source: Vec<String>,
    stats: &Stats,
    forces: Vec<String>,
) -> std::io::Result<(bool, String)> {
    fix(root, query, terms, text, source, stats, forces, "")
}

/// 测试便捷入口：只扩展 fix，不改变既有测试的七参数调用形态。
#[cfg(test)]
#[allow(clippy::too_many_arguments)] // 测试夹具兼容：生产路径只用 Payload。
pub(crate) fn fix(
    root: &str,
    query: &str,
    terms: Vec<String>,
    text: &str,
    source: Vec<String>,
    stats: &Stats,
    forces: Vec<String>,
    fix: &str,
) -> std::io::Result<(bool, String)> {
    evidence(
        root,
        query,
        terms,
        text,
        source,
        stats,
        forces,
        fix,
        &[],
        &[],
    )
}

/// 测试便捷入口：完整可选字段。
#[cfg(test)]
#[allow(clippy::too_many_arguments)] // 测试夹具兼容：字段已由 Payload 统一。
pub(crate) fn evidence(
    root: &str,
    query: &str,
    terms: Vec<String>,
    text: &str,
    source: Vec<String>,
    stats: &Stats,
    forces: Vec<String>,
    fix: &str,
    evidence_lno: &[usize],
    evidence: &[String],
) -> std::io::Result<(bool, String)> {
    persist(Payload {
        root,
        query,
        terms,
        text,
        source: &source,
        stats,
        forces: &forces,
        fix,
        evidence_lno,
        evidence,
        max_bytes: None,
    })
}

/// 按 id 删除一个 case，并让派生索引按库代际自动失效。
#[cfg(test)]
pub(crate) fn delete(root: &str, id: &str) -> std::io::Result<bool> {
    crate::memory::maintenance::delete(root, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store;
    use crate::memory::util::root;
    use std::fs;

    const TEXT: &str = "ERROR db timeout conn_id=9f2a\nat Pool.java:214\nRoot cause: pool size";

    #[test]
    fn ingest_dedup_by_fingerprint() {
        let root = root("dedup");
        let s1 = Stats {
            kept: 3,
            total: 200,
            cut: 94.2,
            risk: "HIGH".to_string(),
        };
        let a = ingest(
            &root,
            "数据库连接超时 conn_id",
            vec!["conn_id".into()],
            TEXT,
            vec!["/x/build.log".into()],
            &s1,
            vec![],
        )
        .unwrap();
        assert!(a.0, "首次摄取应为新增");
        let b = ingest(
            &root,
            "数据库连接超时 conn_id",
            vec!["conn_id".into()],
            TEXT,
            vec!["/x/build.log".into()],
            &s1,
            vec![],
        )
        .unwrap();
        assert!(!b.0, "同指纹重复摄取应去重，不新增");
        assert!(
            store::list(&root).unwrap().contains(&b.1),
            "去重也应返回既有 case id"
        );
        assert_eq!(store::list(&root).unwrap().len(), 1, "case 数必须保持 1");
        // 内容变体（证据块不同）应新增
        let s2 = Stats {
            kept: 1,
            total: 100,
            cut: 90.0,
            risk: "HIGH".to_string(),
        };
        let c = ingest(
            &root,
            "数据库连接超时 conn_id",
            vec!["conn_id".into()],
            "ERROR 磁盘已满 errno=28",
            vec!["/x/build.log".into()],
            &s2,
            vec![],
        )
        .unwrap();
        assert!(c.0, "证据块变体应新增");
        assert_eq!(store::list(&root).unwrap().len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ingest_with_fix_persists_and_rejects_oversize() {
        // --store 配 --fix 时修复结论随 case 入库（L1 本地直答）。
        let root = root("fix");
        let s1 = Stats {
            kept: 3,
            total: 200,
            cut: 94.2,
            risk: "HIGH".to_string(),
        };
        let added = fix(
            &root,
            "数据库连接超时 conn_id",
            vec!["conn_id".into()],
            TEXT,
            vec!["/x/build.log".into()],
            &s1,
            vec![],
            "enlarge pool to 64 conns, restart daemon",
        )
        .unwrap();
        assert!(added.0, "带 fix 首次摄取应为新增");
        let ids = store::list(&root).unwrap();
        let c = store::load(&root, &ids[0]).unwrap();
        assert_eq!(
            c.fix, "enlarge pool to 64 conns, restart daemon",
            "fix 必须落盘"
        );
        // 超长 fix 拒绝。
        let huge_fix = "x".repeat(MAX_FIX_LEN + 1);
        let err = fix(&root, "q", vec![], "t", vec![], &s1, vec![], &huge_fix).unwrap_err();
        assert!(
            err.to_string().contains("case fix too long"),
            "超长 fix 应明确拒绝: {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dedup_backfills_missing_fix_and_evidence_without_overwriting() {
        let root = root("backfill");
        let s = Stats {
            kept: 1,
            total: 10,
            cut: 90.0,
            risk: "LOW".to_string(),
        };
        let first = ingest(&root, "q", vec![], TEXT, vec![], &s, vec![]).unwrap();
        assert!(first.0);
        let second = evidence(
            &root,
            "q",
            vec![],
            TEXT,
            vec![],
            &s,
            vec![],
            "restart pool",
            &[1],
            &["ERROR db timeout".to_string()],
        )
        .unwrap();
        assert!(!second.0, "同指纹必须去重");
        let c = store::load(&root, &first.1).unwrap();
        assert_eq!(c.fix, "restart pool", "空 fix 可由后续 store 补录");
        assert_eq!(c.evidence_lno, vec![1], "空 evidence 可由后续 store 补录");
        assert_eq!(c.evidence, vec!["ERROR db timeout"]);
        // 已有结论不被后续重复摄取覆盖。
        fix(
            &root,
            "q",
            vec![],
            TEXT,
            vec![],
            &s,
            vec![],
            "different fix",
        )
        .unwrap();
        let c = store::load(&root, &first.1).unwrap();
        assert_eq!(c.fix, "restart pool", "已有修复结论不得被覆盖");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ingest_with_evidence_roundtrip_and_legacy_compat() {
        // 证据行区间与原始证据落盘；旧 case 缺键时读为空，不 panic。
        let root = root("ev");
        let s1 = Stats {
            kept: 3,
            total: 200,
            cut: 94.2,
            risk: "HIGH".to_string(),
        };
        let ev_lno = vec![3usize, 14, 37];
        let ev = vec![
            "ERROR db timeout".to_string(),
            "at Pool.java:214".to_string(),
            "fix hint".to_string(),
        ];
        let added = evidence(
            &root,
            "数据库连接超时 conn_id",
            vec!["conn_id".into()],
            TEXT,
            vec!["/x/build.log".into()],
            &s1,
            vec![],
            "enlarge pool",
            &ev_lno,
            &ev,
        )
        .unwrap();
        assert!(added.0, "带证据首次摄取应为新增");
        let ids = store::list(&root).unwrap();
        let c = store::load(&root, &ids[0]).unwrap();
        assert_eq!(c.evidence_lno, ev_lno, "证据行区间必须落盘");
        assert_eq!(c.evidence, ev, "原始证据行必须落盘");
        assert_eq!(c.fix, "enlarge pool", "fix 与 evidence 可同时落盘");
        // 旧 case 兼容：直接用 store::decode 模拟无键读取（无 evidence 字段）。
        let legacy = store::decode(
            &crate::memory::json::parse(
                "{\"id\":\"x\",\"query\":\"q\",\"terms\":[],\"text\":\"t\",\
                 \"fp\":\"f\",\"source\":[],\"tags\":[],\"fields\":{},\"stats\":{},\
                 \"forces\":[],\"feedback\":{\"good\":0,\"bad\":0},\"quality\":0.5}",
            )
            .unwrap(),
        );
        assert!(
            legacy.evidence_lno.is_empty(),
            "旧 case 无 evidence 键应读空"
        );
        assert!(legacy.evidence.is_empty(), "旧 case 无 evidence 键应读空");
        // 长度不匹配拒绝。
        let err = evidence(
            &root,
            "q",
            vec![],
            "t",
            vec![],
            &s1,
            vec![],
            "",
            &[1usize],
            &[],
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("evidence_lno/evidence length mismatch"),
            "证据数组长度不一致应拒绝: {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fingerprint_is_deterministic_and_sensitive() {
        let a = fingerprint("db timeout", TEXT);
        let b = fingerprint("db timeout", TEXT);
        assert_eq!(a, b);
        assert_ne!(
            a,
            fingerprint("db timeoutx", TEXT),
            "query 变体指纹必须不同"
        );
        assert_ne!(
            a,
            fingerprint("db timeout", "other text"),
            "文本变体指纹必须不同"
        );
    }

    #[test]
    fn fingerprint_keeps_query_numbers_but_folds_text_noise() {
        assert_ne!(
            fingerprint("port 8080 already in use", "ERROR port 8080 in use"),
            fingerprint("port 9090 already in use", "ERROR port 9090 in use"),
            "不同端口是不同事故，不能共享身份指纹"
        );
        assert_eq!(
            fingerprint("port 8080 in use", "ERROR pid=12345 port 8080"),
            fingerprint("port 8080 in use", "ERROR pid=67890 port 8080"),
            "同一 query 下 text 的 PID/时间戳噪声仍应折叠"
        );
    }

    #[test]
    fn fingerprint_index_is_lazily_rebuilt() {
        let root = root("fpidx");
        let s = Stats {
            kept: 1,
            total: 10,
            cut: 0.0,
            risk: "LOW".to_string(),
        };
        let (_, id) = ingest(&root, "q index", vec![], TEXT, vec![], &s, vec![]).unwrap();
        let fp = fingerprint("q index", TEXT);
        assert_eq!(find(&root, &fp).unwrap().as_deref(), Some(id.as_str()));

        fs::remove_file(index(&root)).unwrap();
        fs::remove_file(ready(&root)).unwrap();
        assert_eq!(find(&root, &fp).unwrap().as_deref(), Some(id.as_str()));
        assert!(index(&root).exists(), "重查应重建指纹索引");
    }

    #[test]
    fn fingerprint_index_rebuilds_after_delete() {
        let root = root("fpdel");
        let s = Stats {
            kept: 1,
            total: 10,
            cut: 0.0,
            risk: "LOW".to_string(),
        };
        let (_, first) = ingest(&root, "q alpha", vec![], TEXT, vec![], &s, vec![]).unwrap();
        let (_, second) = ingest(
            &root,
            "q beta",
            vec![],
            "FATAL other evidence",
            vec![],
            &s,
            vec![],
        )
        .unwrap();
        assert!(crate::memory::maintenance::delete(&root, &first).unwrap());
        assert!(find(&root, &fingerprint("q alpha", TEXT))
            .unwrap()
            .is_none());
        assert_eq!(
            find(&root, &fingerprint("q beta", "FATAL other evidence"))
                .unwrap()
                .as_deref(),
            Some(second.as_str())
        );
    }

    #[test]
    fn ingest_keeps_numeric_query_variants_and_dedups_legacy_fp() {
        let root = root("numid");
        let s = Stats {
            kept: 1,
            total: 10,
            cut: 0.0,
            risk: "LOW".to_string(),
        };
        let text = "ERROR port 8080 already in use\nat Sock.java:1";
        let a = ingest(
            &root,
            "port 8080 already in use",
            vec![],
            text,
            vec![],
            &s,
            vec![],
        )
        .unwrap();
        let b = ingest(
            &root,
            "port 9090 already in use",
            vec![],
            "ERROR port 9090 already in use\nat Sock.java:1",
            vec![],
            &s,
            vec![],
        )
        .unwrap();
        assert!(a.0 && b.0, "不同端口必须各自落库");
        assert_eq!(store::list(&root).unwrap().len(), 2);

        // 模拟旧库：fp 是旧"数字折叠"口径；动态重算仍应识别同一条记忆。
        let mut c = store::load(&root, &a.1).unwrap();
        c.fp = "0000000000000000".to_string();
        store::save(&root, &c).unwrap();
        let dup = ingest(
            &root,
            "port 8080 already in use",
            vec![],
            text,
            vec![],
            &s,
            vec![],
        )
        .unwrap();
        assert!(!dup.0, "旧 fp 也应通过动态身份键去重");
        assert_eq!(store::list(&root).unwrap().len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    /// 回归：remember 按索引行数推进 ready。并发进程若已写入新 case（case_count
    /// 变大、索引尚未含该行），ready 不得直接等于 case_count，否则 ensure 会信任
    /// 残缺索引而漏判重，重复 case 入库。
    /// remember 必须幂等：并发 rebuild 已收录同一 (fp, id) 时不得再追加重复行，
    /// 否则索引行数超过 case 数、ready 与 case_count 永远对不上。
    #[test]
    fn remember_is_idempotent_for_same_case() {
        let root = root("ready-idem");
        let (_, id) = ingest(
            &root,
            "qa conn",
            vec![],
            TEXT,
            vec!["/x/build.log".into()],
            &store::sample(),
            vec![],
        )
        .unwrap();
        let fp = fingerprint("qa conn", TEXT);
        // 模拟并发 rebuild 已收录该行后，迟到的 remember 再次执行。
        remember(&root, &fp, &id).unwrap();
        remember(&root, &fp, &id).unwrap();
        let body = fs::read_to_string(index(&root)).unwrap();
        assert_eq!(body.lines().count(), 1, "同一 (fp,id) 只能有一行");
        assert_eq!(
            fs::read_to_string(ready(&root)).unwrap().trim(),
            "1",
            "ready 必须等于索引真实行数"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn remember_does_not_claim_unindexed_cases() {
        let root = root("ready-race");
        let s = store::sample();
        let (_, id_a) = ingest(
            &root,
            "qa conn",
            vec![],
            TEXT,
            vec!["/x/build.log".into()],
            &s,
            vec![],
        )
        .unwrap();
        let base = store::load(&root, &id_a).unwrap();
        // 模拟并发：另外两个 case 已落库并推进 case_count，但索引尚无它们的行。
        let mut b = base.clone();
        b.id = "2026-01-01_bbbbbbbbbbbbbbbb".to_string();
        b.query = "qb conn".to_string();
        let mut c = base.clone();
        c.id = "2026-01-01_cccccccccccccccc".to_string();
        c.query = "qc conn".to_string();
        for case in [&b, &c] {
            store::save(&root, case).unwrap();
            store::bump(&root).unwrap();
        }
        // 迟到的 remember：只追加自己的一行，索引此时仍不覆盖 C。
        let fp_b = fingerprint(&b.query, &b.text);
        remember(&root, &fp_b, &b.id).unwrap();
        // 索引不完整 → ensure 必须重建；find(C) 应命中而不是漏判。
        let fp_c = fingerprint(&c.query, &c.text);
        assert_eq!(find(&root, &fp_c).unwrap().as_deref(), Some(c.id.as_str()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_removes_case_and_meta() {
        let root = root("del");
        let s1 = Stats {
            kept: 1,
            total: 10,
            cut: 0.0,
            risk: "LOW".to_string(),
        };
        ingest(&root, "q1", vec![], "t1", vec![], &s1, vec![]).unwrap();
        let id = store::list(&root).unwrap()[0].clone();
        assert!(delete(&root, &id).unwrap());
        assert!(store::list(&root).unwrap().is_empty());
        let m = crate::memory::meta::load(&root).unwrap();
        assert_eq!(m.case_count, 0);
        let _ = fs::remove_dir_all(&root);
    }
}
