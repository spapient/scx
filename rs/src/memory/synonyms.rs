//! 运维领域确定性同义词表。
//!
//! 两个使用方：
//! - `annotate`：按 seed 做字面改写，生成 paraphrase 标注；
//! - `recall`：A 管线把 query 词扩展为同义 token，补足 `replication/replica`、
//!   `delay/lag` 这类无字面交集的释义查询；B 基线保持纯词面，不做扩展。
//!
//! 只做精确 token 匹配，不做子串替换；表顺序固定，输出确定。

/// 双向词对（中英混排）；新增词对必须同时覆盖两个方向，避免检索不对称。
pub(crate) const PAIRS: &[(&str, &str)] = &[
    ("db", "database"),
    ("database", "db"),
    ("timeout", "超时"),
    ("超时", "timeout"),
    ("conn_id", "连接id"),
    ("连接id", "conn_id"),
    ("conn", "connection"),
    ("connection", "conn"),
    ("connection", "连接"),
    ("连接", "connection"),
    ("pool", "连接池"),
    ("连接池", "pool"),
    ("池", "pool"),
    ("error", "错误"),
    ("错误", "error"),
    ("errno", "错误码"),
    ("错误码", "errno"),
    ("disk", "磁盘"),
    ("磁盘", "disk"),
    ("disk", "storage"),
    ("storage", "disk"),
    ("storage", "存储"),
    ("存储", "storage"),
    ("full", "满"),
    ("满", "full"),
    ("queue", "队列"),
    ("队列", "queue"),
    ("fail", "失败"),
    ("失败", "fail"),
    ("failure", "failed"),
    ("failed", "failure"),
    ("crash", "崩溃"),
    ("崩溃", "crash"),
    ("worker", "线程"),
    ("线程", "worker"),
    ("replication", "replica"),
    ("replica", "replication"),
    ("delay", "lag"),
    ("lag", "delay"),
    ("memory", "heap"),
    ("heap", "memory"),
    ("memory", "oom"),
    ("oom", "memory"),
    ("certificate", "cert"),
    ("cert", "certificate"),
    ("config", "configuration"),
    ("configuration", "config"),
    ("deploy", "deployment"),
    ("deployment", "deploy"),
    ("auth", "authentication"),
    ("authentication", "auth"),
    ("permission", "denied"),
    ("denied", "permission"),
];

/// 召回专用的高精度子集：只收无字面交集的领域释义对。
///
/// 不使用 `db/database`、`error/错误`、`timeout/超时` 这类高频宽词：
/// 它们会把 `db write` 扩成 `database`，让 database connection pool case
/// 反超真正的 db write 目标（104 条截断诊断中已实测 literal MRR 退化）。
const RECALL_PAIRS: &[(&str, &str)] = &[
    ("replication", "replica"),
    ("replica", "replication"),
    ("delay", "lag"),
    ("lag", "delay"),
    ("memory", "heap"),
    ("heap", "memory"),
    ("memory", "oom"),
    ("oom", "memory"),
    ("certificate", "cert"),
    ("cert", "certificate"),
    ("config", "configuration"),
    ("configuration", "config"),
    ("deploy", "deployment"),
    ("deployment", "deploy"),
    ("failure", "failed"),
    ("failed", "failure"),
    ("disk", "storage"),
    ("storage", "disk"),
    ("permission", "denied"),
    ("denied", "permission"),
    ("auth", "authentication"),
    ("authentication", "auth"),
    ("conn", "connection"),
    ("connection", "conn"),
];

/// 对每个精确命中的 token 追加召回用同义 token，并去重保序。
pub(crate) fn expand_terms(terms: &[String]) -> Vec<String> {
    let mut out = terms.to_vec();
    for t in terms {
        for (from, to) in RECALL_PAIRS {
            if t == from && !out.iter().any(|x| x == to) {
                out.push((*to).to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_only_exact_known_tokens() {
        let got = expand_terms(&["replication".into(), "delay".into(), "x".into()]);
        assert!(got.contains(&"replica".to_string()));
        assert!(got.contains(&"lag".to_string()));
        assert!(!got.contains(&"y".to_string()));
    }

    #[test]
    fn does_not_expand_broad_db_alias_in_recall() {
        assert_eq!(expand_terms(&["db".into()]), vec!["db".to_string()]);
    }

    #[test]
    fn expansion_is_bidirectional_and_deduplicated() {
        let got = expand_terms(&["memory".into(), "heap".into()]);
        assert_eq!(got.iter().filter(|t| *t == "memory").count(), 1);
        assert_eq!(got.iter().filter(|t| *t == "heap").count(), 1);
        assert!(got.contains(&"oom".to_string()));
    }
}
