//! meta.toml 读写与库根初始化（零依赖行式子集）。
//!
//! 布局见 store.rs 头注释。meta 是"库元信息"（schema/引擎版本/case 数/DF 快照/参数），
//! 与 case 资产分离：损坏时整库仍可用，rebuild 可重建。

use crate::memory::lock;
/// case.json 的当前 schema 版本。旧版本读取按缺字段默认值兼容。
pub(crate) const SCHEMA: u32 = 2;
pub(crate) const ENG_VER: &str = "stat-v1";
use crate::memory::util::utc_now;
use std::fs;
use std::path::Path;

/// 建库根目录（含 cases/、index/）与初始 meta.toml。
pub(crate) fn ensure_root(root: &str) -> std::io::Result<()> {
    for sub in ["", "cases", "index"] {
        let p = Path::new(root).join(sub);
        if !p.exists() {
            lock::create_dir_all(&p)?;
        }
    }
    let meta_p = Path::new(root).join("meta.toml");
    if !meta_p.exists() {
        // 多进程同时首次打开：锁内二次检查，避免两边写坏同一 meta。
        let _g = lock::acquire(root, "meta.lock")?;
        if !meta_p.exists() {
            lock::atomic_write(&meta_p, meta_toml(0).as_bytes())?;
        }
    }
    Ok(())
}

pub(crate) struct Meta {
    /// 建库时间（UTC）。评估/自检用它冻结衰减时钟，保证同一库快照可重放；
    /// 线上召回仍使用当前时间。
    pub created_at: String,
    pub case_count: usize,
    /// 库代际：任何 case 写入 / 删除都 +1；
    /// qmap 快照与会话缓存条目记录写入时的 gen，`gen < 当前` 视为过期
    /// （新 case / 反馈 / 删除必须对同 query 召回可见）。
    pub gen: u64,
    pub df: Vec<(String, usize)>,
    pub params: Vec<(String, f64)>,
    /// 词级调权表：词 → 增量权重（Rocchio 简化版），无词条=0。
    pub word_w: Vec<(String, f64)>,
    /// L1 自适应：最近 top1_sim 滑动历史（上限 L1_HIST_CAP，存于 [l1] 节）。
    pub l1_hist: Vec<f64>,
}

impl Meta {
    /// 参数读取的唯一入口：库内值优先，缺省值来自同一张默认表。
    pub(crate) fn param(&self, key: &str) -> f64 {
        if let Some((_, value)) = self.params.iter().find(|(k, _)| k == key) {
            return *value;
        }
        debug_assert!(
            DEFAULT_PARAMS.iter().any(|(k, _)| *k == key),
            "unknown config key: {key}"
        );
        default_param(key)
    }
}

/// 默认参数查表；未知键返回 0.0（老库兼容），开发期由 `Meta::param` 的断言兜住。
pub(crate) fn default_param(key: &str) -> f64 {
    DEFAULT_PARAMS
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(0.0, |(_, value)| *value)
}

/// 新库参数模板，也是所有缺省读取的唯一来源。
pub(crate) const DEFAULT_PARAMS: &[(&str, f64)] = &[
    ("bm25_k1", 1.2),
    ("bm25_b", 0.75),
    ("field_weight_error_code", 3.0),
    ("field_weight_file_loc", 2.5),
    ("field_weight_symbol", 2.0),
    ("field_weight_num_fp", 2.0),
    ("decay_tau_days", 90.0),
    ("decay_tau_solved", 180.0),
    ("decay_tau_regressed", 30.0),
    ("mmr_lambda", 0.5),
    ("mmr_enable", 0.0),
    ("bucket_enable", 1.0),
    ("bucket_allow_other", 1.0),
    ("bucket_bypass_sim", 0.30),
    ("prf_enable", 1.0),
    ("confidence_high_sim", 0.99),
    ("index_force", 0.0),
    ("feedback_step", 0.1),
    ("feedback_step_bad", 0.15),
    ("word_w_limit", 0.5),
    ("session_cache_minutes", 30.0),
    ("l1_base", 0.55),
    ("l1_margin_min", 0.15),
    ("l1_adapt_min", 0.30),
    ("l1_adapt_max", 0.90),
    ("l1_adapt_min_history", 4.0),
    ("quality_prior", 0.5),
    ("gc_ttl_regressed", 180.0),
    ("gc_ttl_stale", 365.0),
    ("edge_enable", 1.0),
    ("edge_weight", 1.0),
    ("query_field_weight", 0.80),
    ("query_field_min_sim", 0.07),
    ("query_field_trigger_sim", 0.30),
    ("sibling_coherence", 1.0),
    ("sibling_min_sim", 0.30),
    ("synonyms_enable", 1.0),
    ("synonym_weight", 0.5),
];

pub(crate) const L1_HIST_CAP: usize = 64;

pub(crate) fn load_meta(root: &str) -> std::io::Result<Meta> {
    let p = Path::new(root).join("meta.toml");
    let txt = fs::read_to_string(p)?;
    let mut meta = Meta {
        created_at: String::new(),
        case_count: 0,
        gen: 0,
        df: Vec::new(),
        params: Vec::new(),
        word_w: Vec::new(),
        l1_hist: Vec::new(),
    };
    // 行式解析统一由 scx::conf 提供（节感知 TOML 子集），语义与历史实现一致。
    for e in crate::conf::parse(txt.lines()) {
        apply_meta_entry(e, &mut meta);
    }
    Ok(meta)
}

fn apply_meta_entry(e: crate::conf::Entry<'_>, meta: &mut Meta) {
    match e.section {
        "" => apply_scalar(e.key, e.value, meta),
        "df" => push_usize(&mut meta.df, e.key, e.value),
        "params" => push_f64(&mut meta.params, e.key, e.value),
        "word_w" => push_f64(&mut meta.word_w, e.key, e.value),
        "l1" if e.key == "hist" => push_hist(e.value, &mut meta.l1_hist),
        _ => {}
    }
}

/// 无节标量（created_at / case_count / gen）：未知键忽略（老库缺字段按默认）。
fn apply_scalar(key: &str, value: &str, meta: &mut Meta) {
    match key {
        "created_at" => meta.created_at = crate::conf::unquote(value).to_string(),
        "case_count" => meta.case_count = crate::conf::parse_usize(value).unwrap_or(0),
        "gen" => meta.gen = crate::conf::parse_u64(value).unwrap_or(0),
        _ => {}
    }
}

fn push_usize(out: &mut Vec<(String, usize)>, key: &str, value: &str) {
    if let Some(n) = crate::conf::parse_usize(value) {
        out.push((key.to_string(), n));
    }
}

fn push_f64(out: &mut Vec<(String, f64)>, key: &str, value: &str) {
    if let Some(f) = crate::conf::parse_f64(value) {
        out.push((key.to_string(), f));
    }
}

/// 逗号分隔的 L1 历史串；每条 hist 行解析后截断到上限（保持逐行截断口径）。
fn push_hist(value: &str, hist: &mut Vec<f64>) {
    let body = crate::conf::unquote(value);
    for tok in body.split(',') {
        if let Some(f) = crate::conf::parse_f64(tok.trim()) {
            hist.push(f);
        }
    }
    hist.truncate(L1_HIST_CAP);
}

/// 库代际 +1 并原子写回（case 写入/删除的兜底失效信号；失败返回 Err，
/// 由调用方决定是否致命——case 已写盘，gen 落后只会多一次重算，不丢数据）。
/// 已持 `meta.lock` 时调用（delete 等多锁路径用 `acquire_all` 一次取齐后调此变体）。
pub(crate) fn bump_gen_locked(root: &str) -> std::io::Result<()> {
    let p = Path::new(root).join("meta.toml");
    let txt = fs::read_to_string(&p)?;
    let mut next: Option<u64> = None;
    let mut out: Vec<String> = Vec::new();
    for line in txt.lines() {
        let t = line.trim();
        if is_comment_or_section(t) {
            out.push(line.to_string());
            continue;
        }
        if let Some(v) = gen_value(t) {
            next = Some(v.saturating_add(1));
            out.push(format!("gen = {}", next.unwrap()));
            continue;
        }
        out.push(line.to_string());
    }
    let gen = next.unwrap_or(1);
    if next.is_none() {
        insert_gen_line(&mut out, gen);
    }
    let mut body = out.join("\n");
    body.push('\n');
    lock::atomic_write(&p, body.as_bytes())
}

fn is_comment_or_section(t: &str) -> bool {
    t.starts_with('[') || t.starts_with('#')
}

fn gen_value(t: &str) -> Option<u64> {
    let eq = t.find('=')?;
    if t[..eq].trim() != "gen" {
        return None;
    }
    t[eq + 1..].trim().parse::<u64>().ok()
}

fn insert_gen_line(out: &mut Vec<String>, gen: u64) {
    if let Some(ci) = out.iter().position(|l| l.trim().starts_with("case_count")) {
        out.insert(ci + 1, format!("gen = {gen}"));
    } else {
        out.push(format!("gen = {gen}"));
    }
}

/// 代际递增（包装 `bump_gen_locked`：取 `meta.lock` 后执行）。
pub(crate) fn bump_gen(root: &str) -> std::io::Result<()> {
    let _g = lock::acquire(root, "meta.lock")?;
    bump_gen_locked(root)
}

/// 追加一条 top1_sim 到 L1 历史并重写 meta.toml。
/// 失败静默（旁路：历史只是置信校准的输入，不影响召回正确性）。
pub(crate) fn push_l1_hist(root: &str, sim: f64) {
    let Ok(_g) = lock::acquire(root, "meta.lock") else {
        return;
    };
    let Ok(mut meta) = load_meta(root) else {
        return;
    };
    meta.l1_hist.push(sim);
    if meta.l1_hist.len() > L1_HIST_CAP {
        let drop = meta.l1_hist.len() - L1_HIST_CAP;
        meta.l1_hist.drain(..drop);
    }
    let hist_txt = meta
        .l1_hist
        .iter()
        .map(|f| format!("{f:.4}"))
        .collect::<Vec<_>>()
        .join(",");
    // 重写 meta.toml：保留 df/params/word_w 节，仅更新 [l1] hist。
    let p = Path::new(root).join("meta.toml");
    let Ok(txt) = fs::read_to_string(&p) else {
        return;
    };
    let mut out: Vec<String> = Vec::new();
    let mut in_l1 = false;
    let mut hist_written = false;
    for line in txt.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            in_l1 = t == "[l1]";
        } else if in_l1 && t.starts_with("hist") {
            out.push(format!("hist = \"{hist_txt}\""));
            hist_written = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !hist_written {
        // 无 [l1] 节：在文件末尾追加。
        out.push("[l1]".to_string());
        out.push(format!("hist = \"{hist_txt}\""));
    }
    let mut body = out.join("\n");
    body.push('\n');
    let _ = lock::atomic_write(&p, body.as_bytes());
}

/// 词级调权：good 词 +step / bad 词 −step（默认上下限 ±0.5，§3.9），重写 [word_w] 节。
pub(crate) fn bump_word_w(root: &str, terms: &[String], good: bool) {
    let Ok(_g) = lock::acquire(root, "meta.lock") else {
        return;
    };
    let Ok(mut meta) = load_meta(root) else {
        return;
    };
    let step = if good {
        meta.param("feedback_step")
    } else {
        -meta.param("feedback_step_bad")
    };
    let limit = meta.param("word_w_limit").max(0.0);
    apply_steps(&mut meta, terms, step, limit);
    let p = Path::new(root).join("meta.toml");
    let Ok(txt) = fs::read_to_string(&p) else {
        return;
    };
    let out = rewrite_word_w(&txt, &meta.word_w);
    let mut body = out.join("\n");
    body.push('\n');
    let _ = lock::atomic_write(&p, body.as_bytes());
}

/// 逐词应用调权：跳过单字符词；在表中则原位 clamp，否则追加。
fn apply_steps(meta: &mut Meta, terms: &[String], step: f64, limit: f64) {
    for t in terms {
        if t.len() < 2 {
            continue;
        }
        let cur = meta
            .word_w
            .iter()
            .find(|(k, _)| k == t)
            .map(|(_, v)| *v)
            .unwrap_or(0.0);
        let next = (cur + step).clamp(-limit, limit);
        if let Some(e) = meta.word_w.iter_mut().find(|(k, _)| k == t) {
            e.1 = next;
        } else {
            meta.word_w.push((t.clone(), next));
        }
    }
}

/// [word_w] 节重写：旧节整体跳过，在其后第一个节头前插入新节；
/// 文件无节头时在末尾追加。
fn rewrite_word_w(txt: &str, word_w: &[(String, f64)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut wrote_w = false;
    for line in txt.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if t == "[word_w]" {
                // 跳过旧节，统一在末尾重写。
                wrote_w = true;
                continue;
            }
            if !wrote_w && !word_w.is_empty() {
                out.extend(word_w_block(word_w));
                out.push("".to_string());
                wrote_w = true;
            }
        }
        out.push(line.to_string());
    }
    if !wrote_w {
        out.extend(word_w_block(word_w));
    }
    out
}

fn word_w_block(word_w: &[(String, f64)]) -> Vec<String> {
    let mut out = vec!["[word_w]".to_string()];
    for (k, v) in word_w {
        out.push(format!("{k} = {v:.3}"));
    }
    out
}

/// 写/更新 [params] 单个参数：节内有 key 原位替换，无则节内末尾插入，无节则文件尾建节。
/// 参数写入的唯一入口；建库模板、L1 自适应与测试共用。
pub(crate) fn set_param(root: &str, key: &str, val: f64) -> std::io::Result<()> {
    let _g = lock::acquire(root, "meta.lock")?;
    let p = Path::new(root).join("meta.toml");
    let txt = fs::read_to_string(&p)?;
    let key_line = format!("{key} = {val:.3}");
    let mut lines: Vec<String> = txt.lines().map(|s| s.to_string()).collect();
    if let Some(pi) = lines.iter().position(|l| l.trim() == "[params]") {
        if !replace_param_line(&mut lines, pi, key, &key_line) {
            insert_param_line(&mut lines, pi, key_line);
        }
    } else {
        lines.push("[params]".to_string());
        lines.push(key_line.clone());
    }
    let mut body = lines.join("\n");
    body.push('\n');
    lock::atomic_write(&p, body.as_bytes())
}

fn replace_param_line(lines: &mut [String], pi: usize, key: &str, key_line: &str) -> bool {
    for l in lines.iter_mut().skip(pi + 1) {
        let t = l.trim();
        if t.starts_with('[') {
            return false;
        }
        if line_key_is(t, key) {
            *l = key_line.to_string();
            return true;
        }
    }
    false
}

fn insert_param_line(lines: &mut Vec<String>, pi: usize, key_line: String) {
    let end = lines
        .iter()
        .enumerate()
        .skip(pi + 1)
        .find(|(_, l)| l.trim().starts_with('['))
        .map(|(i, _)| i)
        .unwrap_or(lines.len());
    lines.insert(end, key_line);
}

fn line_key_is(t: &str, key: &str) -> bool {
    t.find('=').is_some_and(|eq| t[..eq].trim() == key)
}

fn meta_toml(case_count: usize) -> String {
    let mut body = format!(
        "schema = {SCHEMA}\nembed_engine = \"{ENG_VER}\"\ncreated_at = \"{}\"\ncase_count = {case_count}\ngen = 0\n\n[params]\n",
        utc_now()
    );
    for (key, value) in DEFAULT_PARAMS {
        body.push_str(&format!("{key} = {value}\n"));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::util::tmp_root;
    use std::fs;

    #[test]
    fn meta_roundtrip() {
        let root = tmp_root("rt");
        ensure_root(&root).unwrap();
        let m = load_meta(&root).unwrap();
        assert_eq!(m.case_count, 0);
        assert!(
            m.created_at.ends_with('Z') && m.created_at.contains('T'),
            "建库时间必须可读，供评估冻结时钟: {:?}",
            m.created_at
        );
        assert!(m.df.is_empty(), "新库 df 快照应为空（摄取后才增量写入）");
        assert!(
            m.params
                .iter()
                .any(|(k, v)| k == "bm25_k1" && (*v - 1.2).abs() < 1e-9),
            "默认参数应含 bm25_k1=1.2"
        );
        assert!(
            m.params
                .iter()
                .any(|(k, v)| k == "query_field_weight" && (*v - 0.80).abs() < 1e-9),
            "新库默认必须启用降权 query-field 重排"
        );
        assert!(
            m.params
                .iter()
                .any(|(k, v)| k == "sibling_min_sim" && (*v - 0.30).abs() < 1e-9),
            "新库默认弱词面锚点不得启用同族收紧"
        );
        assert!(
            m.params
                .iter()
                .any(|(k, v)| k == "synonym_weight" && (*v - 0.5).abs() < 1e-9),
            "新库默认同义扩展必须降权"
        );
        assert!(
            m.params
                .iter()
                .any(|(k, v)| k == "bucket_bypass_sim" && (*v - 0.30).abs() < 1e-9),
            "新库默认低置信时必须绕过场景桶硬筛"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
