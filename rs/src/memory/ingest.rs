//! 摄取：压缩产物（Outcome）→ 记忆 case。
//!
//! 摄取即过滤：入库的不是原始日志，而是 compress 筛出的高密度证据块。
//! 去重：归一化指纹 `fp = fnv1a(norm(query) ‖ norm(text))`，已存在则只更新
//! updated_at 与反馈相关字段，不新增（设计 v0.2 §6）。
//!
//! 红线：本模块是 I/O 侧（memory），compress 零改动；摄取结果不影响压缩输出，
//! 只沉淀旁路资产。
//!
//! 阶段化开发过渡：P1.2 落地摄取，CLI 接线在 P1.5，非 test 构建暂放宽 dead_code。

#![cfg_attr(not(test), allow(dead_code))]

use crate::memory::store::{self, Case, CaseStats};

/// 单 case 文本上限（P4.4 评审处置）：64 MiB 输入不得直接变 64 MiB case；
/// 超过上限拒绝摄取并明确报错（旁路，不影响压缩主流程）。
pub(crate) const MAX_CASE_TEXT: usize = 1024 * 1024; // 1 MiB

/// 修复结论上限（P4.4+ 优化：--fix 随 --store 写入，属用户摘要文本）。
pub(crate) const MAX_FIX_LEN: usize = 4096; // 4 KiB

/// 计算去重指纹（确定性、无随机源）。
///
/// 身份口径：query 保留数字（8080/9090、errno=28/29 是不同事故），text 仍按
/// `norm_into` 折叠数字（日志里的 PID/时间戳/行号不应制造重复 case）。
/// 这修复了旧实现“query 数字也被折叠 → 不同端口事故互相去重”的问题。
pub(crate) fn fingerprint(query: &str, text: &str) -> String {
    let mut q = String::new();
    crate::norm::norm_identity_into(query, &mut q);
    let mut run = String::new();
    let mut t = String::new();
    crate::norm::norm_into(text, &mut t, &mut run);
    let h = store::fnv1a(&format!("{q}\u{0}{t}"));
    format!("{:016x}", h)
}

/// 按身份指纹查库内是否已存在 case，返回其 id。
/// 规模小（P1 阶段 <1 万）时线性扫描；P3 起由索引/指纹表替代。
///
/// 同时比对动态重算值：旧 case 的 `fp` 是“数字折叠”口径，直接比字段会漏掉
/// 升级前的同一条记忆；动态值让旧库无需迁移即可按新口径去重。
pub(crate) fn find_by_fp(root: &str, fp: &str) -> std::io::Result<Option<String>> {
    for id in store::list_ids(root)? {
        let c = store::read_case(root, &id)?;
        if c.fp == fp || fingerprint(&c.query, &c.text) == fp {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// 摄取一个 case。返回 `(added, case_id)`：added=true 表示新增，false 表示命中去重
/// （仅刷新 updated_at）；case_id 始终为本次涉及的 case id（新增或既有）。
///
/// `terms`/`stats`/`forces` 由调用方（CLI 或后续记忆管道）从压缩结果提取；
/// 本函数负责 id 生成、指纹去重、写盘与 meta 维护。
/// 不带修复结论的便捷入口（测试与无 fix 调用点复用；fix 为空）。
pub(crate) fn ingest(
    root: &str,
    query: &str,
    terms: Vec<String>,
    text: &str,
    source: Vec<String>,
    stats: &CaseStats,
    forces: Vec<String>,
) -> std::io::Result<(bool, String)> {
    ingest_with_fix(root, query, terms, text, source, stats, forces, "")
}

/// 摄取带修复结论的 case（P4.4+ 优化：--store 配 --fix）。
/// 8 参为保持 ingest 7 参签名（30+ 测试/调用点零改动）而接受的局部扩展，
/// fix 是末尾可选参数（空串等价 ingest）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn ingest_with_fix(
    root: &str,
    query: &str,
    terms: Vec<String>,
    text: &str,
    source: Vec<String>,
    stats: &CaseStats,
    forces: Vec<String>,
    fix: &str,
) -> std::io::Result<(bool, String)> {
    ingest_impl(
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

/// P4.4+ 优化⑤：带原始证据的摄取（评审缺口 B——case 四要素落地）。
/// `evidence_lno[i]` = 证据行原始 1-based 行号（证据行区间），`evidence[i]` = 对应行原文
/// （无 `N:` 前缀、无 `...` 省略标记）。main.rs --store 从压缩结果 kept_lno + 输入提取；
/// 旧 case/无证据调用（ingest/ingest_with_fix）传空，行为完全等价。
#[allow(clippy::too_many_arguments)]
pub(crate) fn ingest_with_evidence(
    root: &str,
    query: &str,
    terms: Vec<String>,
    text: &str,
    source: Vec<String>,
    stats: &CaseStats,
    forces: Vec<String>,
    fix: &str,
    evidence_lno: &[usize],
    evidence: &[String],
) -> std::io::Result<(bool, String)> {
    ingest_impl(
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
    )
}

#[allow(clippy::too_many_arguments)]
fn ingest_impl(
    root: &str,
    query: &str,
    terms: Vec<String>,
    text: &str,
    source: Vec<String>,
    stats: &CaseStats,
    forces: Vec<String>,
    fix: &str,
    evidence_lno: &[usize],
    evidence: &[String],
) -> std::io::Result<(bool, String)> {
    store::ensure_root(root)?;
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
    let fp = fingerprint(query, text);
    // 同一指纹的“查重 + 写入”必须串行：两个并发 store 若都在 find 前进入，
    // 会跨秒生成两个 id，或在去重更新时用旧快照覆盖刚写入的反馈。
    let _ingest_lock = crate::memory::lock::acquire(root, &format!("cases/.ingest-{fp}.lock"))?;
    if let Some(existing) = find_by_fp(root, &fp)? {
        let _case_lock = store::lock_case(root, &existing)?;
        let mut c = store::read_case(root, &existing)?;
        let now = store::utc_now();
        c.updated_at = now;
        // 反馈/状态等历史字段保留；空修复结论/空证据可由后续 --store 补录，
        // 已有结论绝不覆盖（同指纹重复摄取以首条有效结论为准）。
        if c.fix.is_empty() && !fix.is_empty() {
            c.fix = fix.to_string();
        }
        if c.evidence.is_empty() && !evidence.is_empty() {
            c.evidence_lno = evidence_lno.to_vec();
            c.evidence = evidence.to_vec();
        }
        store::write_case(root, &c)?;
        return Ok((false, existing));
    }
    let now = store::utc_now();
    let id = store::case_id(&now, query, text);
    let _case_lock = store::lock_case(root, &id)?;
    let mut c = Case::new(id.clone(), query.to_string(), terms, text.to_string());
    c.fp = fp;
    c.fix = fix.to_string();
    c.evidence_lno = evidence_lno.to_vec();
    c.evidence = evidence.to_vec();
    c.source = source;
    c.stats = stats.clone();
    c.forces = forces;
    // O1：摄取即抽取，判别字段写入 case（召回时字段加权直接用，无需重扫）。
    c.fields = crate::memory::field::extract(text);
    // O6：摄取即分桶（error_code 家族哈希），检索时按候选桶过滤扫描。
    let fvec = c.fields.error_code.clone();
    c.bucket = crate::memory::bucket::bucket_of(&fvec, &c.terms, text);
    store::write_case(root, &c)?;
    store::bump_case_count(root)?;
    Ok((true, id))
}

/// 按 id 删除一个 case（tombstone 化前的简单物理删除，P3 换惰性标记）。
pub(crate) fn delete(root: &str, id: &str) -> std::io::Result<bool> {
    crate::memory::housekeep::delete_case(root, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store;
    use std::fs;

    fn tmp_root(tag: &str) -> String {
        let r =
            std::env::temp_dir().join(format!("scx-ingest-{}-{}", tag, store::fnv1a("x") & 0xff));
        let _ = fs::remove_dir_all(&r);
        r.to_str().unwrap().to_string()
    }

    const TEXT: &str = "ERROR db timeout conn_id=9f2a\nat Pool.java:214\nRoot cause: pool size";

    #[test]
    fn ingest_dedup_by_fingerprint() {
        let root = tmp_root("dedup");
        let s1 = CaseStats {
            kept: 3,
            total: 200,
            cut_pct: 94.2,
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
            store::list_ids(&root).unwrap().contains(&b.1),
            "去重也应返回既有 case id"
        );
        assert_eq!(
            store::list_ids(&root).unwrap().len(),
            1,
            "case 数必须保持 1"
        );
        // 内容变体（证据块不同）应新增
        let s2 = CaseStats {
            kept: 1,
            total: 100,
            cut_pct: 90.0,
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
        assert_eq!(store::list_ids(&root).unwrap().len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ingest_with_fix_persists_and_rejects_oversize() {
        // P4.4+ 优化：--store 配 --fix 时修复结论随 case 入库（L1 本地直答）。
        let root = tmp_root("fix");
        let s1 = CaseStats {
            kept: 3,
            total: 200,
            cut_pct: 94.2,
            risk: "HIGH".to_string(),
        };
        let added = ingest_with_fix(
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
        let ids = store::list_ids(&root).unwrap();
        let c = store::read_case(&root, &ids[0]).unwrap();
        assert_eq!(
            c.fix, "enlarge pool to 64 conns, restart daemon",
            "fix 必须落盘"
        );
        // 超长 fix 拒绝。
        let huge_fix = "x".repeat(MAX_FIX_LEN + 1);
        let err =
            ingest_with_fix(&root, "q", vec![], "t", vec![], &s1, vec![], &huge_fix).unwrap_err();
        assert!(
            err.to_string().contains("case fix too long"),
            "超长 fix 应明确拒绝: {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dedup_backfills_missing_fix_and_evidence_without_overwriting() {
        let root = tmp_root("backfill");
        let s = CaseStats {
            kept: 1,
            total: 10,
            cut_pct: 90.0,
            risk: "LOW".to_string(),
        };
        let first = ingest(&root, "q", vec![], TEXT, vec![], &s, vec![]).unwrap();
        assert!(first.0);
        let second = ingest_with_evidence(
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
        let c = store::read_case(&root, &first.1).unwrap();
        assert_eq!(c.fix, "restart pool", "空 fix 可由后续 store 补录");
        assert_eq!(c.evidence_lno, vec![1], "空 evidence 可由后续 store 补录");
        assert_eq!(c.evidence, vec!["ERROR db timeout"]);
        // 已有结论不被后续重复摄取覆盖。
        ingest_with_fix(
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
        let c = store::read_case(&root, &first.1).unwrap();
        assert_eq!(c.fix, "restart pool", "已有修复结论不得被覆盖");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ingest_with_evidence_roundtrip_and_legacy_compat() {
        // P4.4+ 优化⑤：证据行区间 + 原始证据落盘；旧 case（无键）读为空，不 panic。
        let root = tmp_root("ev");
        let s1 = CaseStats {
            kept: 3,
            total: 200,
            cut_pct: 94.2,
            risk: "HIGH".to_string(),
        };
        let ev_lno = vec![3usize, 14, 37];
        let ev = vec![
            "ERROR db timeout".to_string(),
            "at Pool.java:214".to_string(),
            "fix hint".to_string(),
        ];
        let added = ingest_with_evidence(
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
        let ids = store::list_ids(&root).unwrap();
        let c = store::read_case(&root, &ids[0]).unwrap();
        assert_eq!(c.evidence_lno, ev_lno, "证据行区间必须落盘");
        assert_eq!(c.evidence, ev, "原始证据行必须落盘");
        assert_eq!(c.fix, "enlarge pool", "fix 与 evidence 可同时落盘");
        // 旧 case 兼容：直接用 store::val_to_case 模拟无键读取（无 evidence 字段）。
        let legacy = store::val_to_case(
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
        let err = ingest_with_evidence(
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
    fn ingest_keeps_numeric_query_variants_and_dedups_legacy_fp() {
        let root = tmp_root("numid");
        let s = CaseStats {
            kept: 1,
            total: 10,
            cut_pct: 0.0,
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
        assert_eq!(store::list_ids(&root).unwrap().len(), 2);

        // 模拟旧库：fp 是旧“数字折叠”口径；动态重算仍应识别同一条记忆。
        let mut c = store::read_case(&root, &a.1).unwrap();
        c.fp = "0000000000000000".to_string();
        store::write_case(&root, &c).unwrap();
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
        assert_eq!(store::list_ids(&root).unwrap().len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_removes_case_and_meta() {
        let root = tmp_root("del");
        let s1 = CaseStats {
            kept: 1,
            total: 10,
            cut_pct: 0.0,
            risk: "LOW".to_string(),
        };
        ingest(&root, "q1", vec![], "t1", vec![], &s1, vec![]).unwrap();
        let id = store::list_ids(&root).unwrap()[0].clone();
        assert!(delete(&root, &id).unwrap());
        assert!(store::list_ids(&root).unwrap().is_empty());
        let m = store::load_meta(&root).unwrap();
        assert_eq!(m.case_count, 0);
        let _ = fs::remove_dir_all(&root);
    }
}
