//! 场景桶（O6）：case 自动归类到场景桶，检索先判候选桶再扫。
//!
//! 桶家族：db / network / disk / build / runtime / other。判定用规范化小写
//! 子串匹配（与 norm 后词表同口径），宁多勿漏：query 侧返回候选桶集合，
//! case 侧取首个命中家族为主桶；检索只扫"主桶 ∈ 候选桶集合"的 case。
//!
//! 零依赖、确定性：纯函数，无 I/O、无随机源。

/// 家族关键词表：每个家族一组小写子串（匹配字段/词表任意位置）。
/// 顺序即优先级：主桶取首个命中的家族。
const FAMILIES: [(&str, &[&str]); 5] = [
    (
        "db",
        &[
            "conn_id",
            "jdbc",
            "mysql",
            "redis",
            "postgres",
            "oracle",
            "mongodb",
            "sql",
            "database",
            "连接池",
            "数据库",
        ],
    ),
    (
        "network",
        &[
            "socket",
            "http",
            "tcp",
            "udp",
            "dns",
            "connect",
            "network",
            "网络",
            "连接超时",
            "timeout",
        ],
    ),
    (
        "disk",
        &["disk", "storage", "mount", "filesystem", "空间不足", "磁盘"],
    ),
    (
        "build",
        &[
            "build", "compile", "link", "gradle", "maven", "cargo", "cmake", "makefile", "构建",
            "编译",
        ],
    ),
    (
        "runtime",
        &[
            "runtime",
            "exception",
            "panic",
            "thread",
            "stack",
            "oom",
            "null",
            "空指针",
            "异常",
        ],
    ),
];

/// case 主桶：按家族顺序取首个命中。
/// `text` 参与判定是为了覆盖 fields 未抽到的家族词（如 build 日志里只有 "cargo"）。
pub(crate) fn bucket_of(fields: &[String], terms: &[String], text: &str) -> String {
    let mut hay: Vec<String> = fields.iter().map(|s| lower(s)).collect();
    hay.extend(terms.iter().map(|s| lower(s)));
    hay.push(lower(text));
    for (fam, keys) in FAMILIES {
        if keys.iter().any(|k| hay.iter().any(|h| h.contains(k))) {
            return fam.to_string();
        }
    }
    "other".to_string()
}

/// query 候选桶集合：任一家族命中即入选（宁多勿漏，防漏召）。
pub(crate) fn candidate_buckets(query: &str) -> Vec<String> {
    let q = lower(query);
    FAMILIES
        .iter()
        .filter(|(_, keys)| keys.iter().any(|k| q.contains(k)))
        .map(|(fam, _)| fam.to_string())
        .collect()
}

/// 归一化小写（复用 norm 的 push_analysis_lower 口径，输入已是小写关键词表）。
fn lower(s: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    crate::norm::norm_into(s, &mut out, &mut run);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_of_detects_family() {
        assert_eq!(bucket_of(&["conn_id=9f2a".into()], &[], ""), "db");
        assert_eq!(bucket_of(&[], &["连接池".into()], ""), "db");
        assert_eq!(bucket_of(&[], &[], "socket timeout 网络"), "network");
        assert_eq!(bucket_of(&[], &[], "磁盘 空间不足"), "disk");
        assert_eq!(bucket_of(&[], &["cargo".into()], "error"), "build");
        assert_eq!(bucket_of(&[], &[], "NullPointerException stack"), "runtime");
        assert_eq!(bucket_of(&[], &[], "hello world"), "other");
    }

    #[test]
    fn candidate_buckets_are_wide() {
        let c = candidate_buckets("数据库连接超时 conn_id");
        assert!(c.contains(&"db".to_string()), "候选桶应含 db: {:?}", c);
        let c2 = candidate_buckets("cargo build 磁盘满");
        assert!(c2.contains(&"build".to_string()) && c2.contains(&"disk".to_string()));
        let c3 = candidate_buckets("普通问题");
        assert!(c3.is_empty(), "无家族命中时空候选桶 = 全量扫（防漏召）");
    }
}
