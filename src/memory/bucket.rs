//! 场景桶（O6）：case 自动归类到场景桶，检索先判候选桶再扫。
//!
//! 桶家族：db / network / disk / build / runtime / other。判定用规范化小写
//! 子串匹配（与 norm 后词表同口径），宁多勿漏：query 侧返回候选桶集合，
//! case 侧取首个命中家族为主桶；检索只扫"主桶 ∈ 候选桶集合"的 case。
//!
//! 零依赖、确定性：纯函数，无 I/O、无随机源。

/// 家族关键词表：每个家族一组小写子串（匹配字段/词表任意位置）。
/// 顺序即优先级：主桶取首个命中的家族。network 放最后：`timeout` 是通用词，
/// 不能把同时带 `disk/storage`、`cargo/build` 或 `oom/thread` 的具体场景吞掉。
const FAMILIES: [(&str, &[&str]); 5] = [
    (
        "db",
        &[
            // 注意：裸 "db" 不在子串表里（"db.java:4" 这类文件实体会被子串误判为
            // db 场景桶，导致编译失败等场景被误归类、query "compile fail" 漏召）。
            // 裸 "db" 由 bucket_of / candidate_buckets 用词级精确匹配单独处理。
            "conn_id",
            "jdbc",
            "mysql",
            "redis",
            "postgres",
            "oracle",
            "mongodb",
            "sql",
            "database",
            "failover",
            "replica",
            "replication",
            "连接池",
            "数据库",
            "主库",
            "主从",
            "裂脑",
        ],
    ),
    (
        "disk",
        &[
            "disk",
            "storage",
            "mount",
            "filesystem",
            "ebs",
            "re-mirror",
            "blob",
            "volume",
            "空间不足",
            "磁盘",
            "云盘",
            "重新镜像",
        ],
    ),
    (
        "build",
        &[
            "build",
            "compile",
            "linker",
            "undefined reference",
            "gradle",
            "maven",
            "cargo",
            "cmake",
            "makefile",
            "构建",
            "编译",
        ],
    ),
    (
        "runtime",
        &[
            "runtime",
            "exception",
            "nullpointerexception",
            "panic",
            "thread",
            "stacktrace",
            "stack trace",
            "oom",
            "null",
            "空指针",
            "异常",
        ],
    ),
    (
        "network",
        &[
            "socket",
            "http",
            "http2",
            "http3",
            "tcp",
            "udp",
            "dns",
            "resolver",
            "servfail",
            "bgp",
            "router",
            "route",
            "prefix",
            "connect",
            "network",
            "网络",
            "全网",
            "传播",
            "路由",
            "骨干",
            "解析服务",
            "连接超时",
            "timeout",
        ],
    ),
];

/// case 主桶：按家族顺序取首个命中。
/// `text` 参与判定是为了覆盖 fields 未抽到的家族词（如 build 日志里只有 "cargo"）。
#[cfg(test)]
pub(crate) fn bucket_of(fields: &[String], terms: &[String], text: &str) -> String {
    bucket_of_query(fields, terms, text)
}

/// query-aware 主桶：先用 case query 词表与字段判家族，query 没有家族证据时
/// 再回退正文。根因正文常含比 query 更宽的域词（例如“数据库权限变更生成超大
/// 文件”），只用正文会把 query 明明写 network/file 的 case 归到 db，随后被
/// 候选桶过滤误删。旧库仍按已存 bucket 使用；新摄取走本函数。
pub(crate) fn bucket_of_query(fields: &[String], query_terms: &[String], text: &str) -> String {
    // 裸 "db" 词级精确匹配（terms 是 query∪text 的切词表，"db.java:4" 是单一
    // token ≠ "db"，不会被误归 db 场景桶）：修 query "db timeout" 漏召的同时
    // 避免文件实体把编译等场景误归类。
    if query_terms.iter().any(|t| t == "db") {
        return "db".to_string();
    }
    let mut hay: Vec<String> = fields.iter().map(|s| lower(s)).collect();
    hay.extend(query_terms.iter().map(|s| lower(s)));
    if let Some(fam) = match_family(&hay, true) {
        return fam;
    }
    // query 只有通用 network 症状（timeout/connection）时，正文里的具体家族
    // 证据优先；否则 `Jdbc.java ... timeout` 会被 network 吞掉。
    let mut combined = hay;
    combined.push(lower(text));
    if let Some(fam) = match_family(&combined, false) {
        return fam;
    }
    "other".to_string()
}

/// case 的多标签场景桶：query 与正文任一命中即入选。
///
/// 单主桶无法表达跨域事故（例如“数据库权限变更生成超大文件并传播全网”既是 db
/// 也是 network）。召回过滤用多标签做“任一交集即保留”，避免同一事故的不同问法
/// 因单桶优先级不同而互相漏召；`bucket` 主桶仍保留给 stats/兼容旧布局。
pub(crate) fn case_buckets(fields: &[String], query_terms: &[String], text: &str) -> Vec<String> {
    let mut hay: Vec<String> = fields.iter().map(|s| lower(s)).collect();
    hay.extend(query_terms.iter().map(|s| lower(s)));
    hay.push(lower(text));
    let mut out = Vec::new();
    if query_terms.iter().any(|t| t == "db") {
        out.push("db".to_string());
    }
    for (fam, keys) in FAMILIES {
        if keys.iter().any(|k| hay.iter().any(|h| contains_key(h, k)))
            && !out.iter().any(|f| f == fam)
        {
            out.push(fam.to_string());
        }
    }
    out
}

fn match_family(hay: &[String], query_only: bool) -> Option<String> {
    for (fam, keys) in FAMILIES {
        let hit = keys.iter().any(|k| hay.iter().any(|h| contains_key(h, k)));
        if !hit {
            continue;
        }
        // query 侧只有通用 network 词时不算“明确网络”，交给正文回退。
        if query_only && fam == "network" {
            let specific = [
                "socket", "http", "http2", "http3", "tcp", "udp", "dns", "network", "网络",
            ]
            .iter()
            .any(|k| hay.iter().any(|h| contains_key(h, k)));
            if !specific {
                continue;
            }
        }
        return Some(fam.to_string());
    }
    None
}

/// ASCII 关键词按词边界匹配，避免 `ebs` 命中 `website`、`link` 命中 `links`；
/// CJK 关键词保持子串匹配（中文没有空格分词）。
fn contains_key(hay: &str, key: &str) -> bool {
    if !key.is_ascii() {
        return hay.contains(key);
    }
    let bytes = hay.as_bytes();
    let kb = key.as_bytes();
    if kb.is_empty() || kb.len() > bytes.len() {
        return false;
    }
    for i in 0..=bytes.len() - kb.len() {
        if &bytes[i..i + kb.len()] != kb {
            continue;
        }
        let before_ok = i == 0 || !is_word_byte(bytes[i - 1]);
        let after_ok = i + kb.len() == bytes.len() || !is_word_byte(bytes[i + kb.len()]);
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// query 候选桶集合：任一家族命中即入选（宁多勿漏，防漏召）。
pub(crate) fn candidate_buckets(query: &str) -> Vec<String> {
    let q = lower(query);
    let mut out: Vec<String> = FAMILIES
        .iter()
        .filter(|(_, keys)| keys.iter().any(|k| contains_key(&q, k)))
        .map(|(fam, _)| fam.to_string())
        .collect();
    // 裸 "db" 词级精确匹配（与 bucket_of 同口径）：query 含独立词 "db" 即
    // 候选 db 桶（子串会误判 "kubectl/obd" 等，宁多勿漏但别乱扩桶）。
    if q.split_whitespace().any(|w| w == "db") && !out.contains(&"db".to_string()) {
        out.push("db".to_string());
    }
    // `timeout`/`connection` 是跨 db/network/disk/build 的通用症状。query 只有
    // 这类通用词时硬筛 network 会把 `database query timeout` / `storage timeout`
    // 直接滤掉；返回空候选桶 = 全量扫，宁多勿漏。出现 socket/dns/tcp 等网络
    // 专用词时才筛 network。
    let only_generic_network = out.len() == 1
        && out[0] == "network"
        && ![
            "socket", "http", "http2", "http3", "tcp", "udp", "dns", "network", "网络",
        ]
        .iter()
        .any(|k| contains_key(&q, k));
    if only_generic_network {
        return Vec::new();
    }
    out
}

/// 归一化小写（剥离 ANSI/CR 后做 Unicode 小写）。
///
/// 旧实现误用 `norm_into`：它不做小写，`Jdbc.java`/`DATABASE` 等大写词
/// 匹配不到小写关键词表，导致本该归 db 的 case 被 timeout 抢先归到 network。
fn lower(s: &str) -> String {
    // 来源 URL 里的 `https` 含 network 关键词 `http`，会把所有带公开复盘
    // 链接的 case 误归 network 桶。桶判定只看正文/实体，不看 URL。
    let mut cleaned = String::new();
    for (i, tok) in s.split_whitespace().enumerate() {
        if i > 0 {
            cleaned.push(' ');
        }
        let cut = tok.find("http://").or_else(|| tok.find("https://"));
        match cut {
            Some(pos) => cleaned.push_str(&tok[..pos]),
            None => cleaned.push_str(tok),
        }
    }
    let mut out = String::new();
    crate::analysis::push_analysis_lower(&cleaned, &mut out);
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
        // 数据库 HA 拓扑词必须归 db：主库 failover / 主从复制 / 裂脑。
        assert_eq!(bucket_of(&[], &["failover".into()], ""), "db");
        assert_eq!(bucket_of(&[], &["replication".into()], ""), "db");
        assert_eq!(bucket_of(&[], &["主从".into()], ""), "db");
        // 大小写不能改变桶判定：`Jdbc.java` 必须先命中 db，而不是被
        // 后面的 timeout 抢先归到 network。
        assert_eq!(
            bucket_of(&[], &["query".into(), "timeout".into()], "at Jdbc.java:88"),
            "db"
        );
        assert_eq!(bucket_of(&[], &[], "DISK FULL"), "disk");
        // 来源 URL 的 `https` 不是 network 证据：带复盘链接的 case 仍按正文分类。
        assert_eq!(
            bucket_of(
                &[],
                &[],
                "升级配置冲突\n公开复盘, 来源: https://example.com/post"
            ),
            "other"
        );
        assert_eq!(
            bucket_of(
                &[],
                &[],
                "数据库连接失败\n公开复盘, 来源: https://example.com/post"
            ),
            "db"
        );
        // EBS 重新镜像属于存储域；根因里出现“网络事件”不能把它归 network。
        assert_eq!(
            bucket_of(&[], &[], "网络事件引发 EBS re-mirroring 风暴与容量耗尽"),
            "disk"
        );
        // DNS resolver / SERVFAIL / BGP 路由 / Blob 存储在复盘文本里可能没有
        // 通用词 dns/storage，仍须归到对应家族，避免桶过滤漏召。
        assert_eq!(
            bucket_of(
                &[],
                &[],
                "an internal software error made the public resolver return servfail"
            ),
            "network"
        );
        assert_eq!(
            bucket_of(&[], &[], "亚特兰大路由器一行配置错误导致 BGP 路由泄漏"),
            "network"
        );
        assert_eq!(
            bucket_of(
                &[],
                &[],
                "reverting the metadata change restored blob lookups"
            ),
            "disk"
        );
        assert_eq!(
            bucket_of(&[], &[], "网络事件引发 cloud 云盘 volume 大规模重新镜像"),
            "disk"
        );
        // `timeout` 不能盖过具体存储场景：disk/storage 优先于通用 network。
        assert_eq!(
            bucket_of(&[], &["disk".into(), "read".into(), "timeout".into()], ""),
            "disk"
        );
        // P4.4+ 优化：裸 "db" 词级精确匹配——terms 含独立 "db" 归 db 桶；
        // "db.java:4" 是单一 token，不得误归 db 场景桶（编译失败场景归 build）。
        assert_eq!(bucket_of(&[], &["db".into()], ""), "db");
        let got = bucket_of(&[], &["db.java:4".into()], "compile fail");
        assert_eq!(got, "build", "文件实体不得把编译场景误归 db 桶");
        // query 明确写 network 时，根因里的“数据库”不得把 case 带偏到 db。
        assert_eq!(
            bucket_of_query(
                &[],
                &["oversized".into(), "file".into(), "network".into()],
                "机器人检测数据库权限变更生成超大文件"
            ),
            "network"
        );
        // query 没有家族证据时才回退正文。
        assert_eq!(
            bucket_of_query(&[], &["hello".into()], "数据库连接失败"),
            "db"
        );
        assert_eq!(
            bucket_of_query(
                &[],
                &[
                    "website".into(),
                    "access".into(),
                    "email".into(),
                    "delivery".into(),
                    "cutover".into()
                ],
                "Enom 计划迁移到新云基础设施，cutover 复杂导致数据复制、网络路由和 DNS 解析问题"
            ),
            "network"
        );
        // 跨域事故：单主桶是 db（query 先命中），但正文的“传播/全网”提供 network
        // 证据；多标签必须同时保留两者，否则 network 问法会把该 case 硬筛掉。
        let multi = case_buckets(
            &[],
            &[
                "database".into(),
                "permission".into(),
                "file".into(),
                "larger".into(),
            ],
            "机器人检测数据库权限变更生成超过软件限制的文件，文件传播全网导致系统级宕机",
        );
        assert!(multi.contains(&"db".to_string()), "{multi:?}");
        assert!(multi.contains(&"network".to_string()), "{multi:?}");
    }

    #[test]
    fn candidate_buckets_are_wide() {
        let c = candidate_buckets("数据库连接超时 conn_id");
        assert!(c.contains(&"db".to_string()), "候选桶应含 db: {:?}", c);
        let c2 = candidate_buckets("cargo build 磁盘满");
        assert!(c2.contains(&"build".to_string()) && c2.contains(&"disk".to_string()));
        let c3 = candidate_buckets("普通问题");
        assert!(c3.is_empty(), "无家族命中时空候选桶 = 全量扫（防漏召）");
        // P4.4 评审处置：裸 "db" 词必须命中 db 家族（桶判定对称，防漏召）。
        let c4 = candidate_buckets("db timeout");
        assert!(
            c4.contains(&"db".to_string()),
            "裸 db 应命中 db 桶: {:?}",
            c4
        );
        // P4.4+ 优化：子串不误判（"kubectl" 含 "db" 子串但不含独立 "db" 词）。
        let c5 = candidate_buckets("kubectl get pods");
        assert!(
            !c5.contains(&"db".to_string()),
            "子串不得乱扩 db 桶: {:?}",
            c5
        );
        assert!(
            candidate_buckets("timeout").is_empty(),
            "通用 timeout 必须全量扫，不能硬筛 network"
        );
        assert!(
            candidate_buckets("connection").is_empty(),
            "通用 connection 必须全量扫"
        );
        assert!(candidate_buckets("socket timeout").contains(&"network".to_string()));
        assert!(candidate_buckets("storage timeout").contains(&"disk".to_string()));
        // 主库 failover 的 query 即使带 "network partition" 也必须保留 db 候选桶，
        // 否则 db 桶 case 被网络桶过滤、主库故障切换直接漏召。
        let c6 = candidate_buckets("primary failover during network partition");
        assert!(
            c6.contains(&"db".to_string()),
            "主库 failover query 应候选 db 桶: {:?}",
            c6
        );
    }
}
