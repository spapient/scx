//! 结构化分类：把一段证据文本转成稳定的检索标签。
//!
//! 两个纯函数子域共享同一份文本判定口径，集中在一个所有者里：
//! - 字段抽取：错误行 `key=val`、堆栈位置、符号、数字折叠指纹；
//! - 场景桶：db / network / disk / build / runtime / other 家族标签。
//!
//! 字段用于 BM25 之外的精确加权与实体边；场景桶用于候选过滤与跨域多标签。
//! 两者都只读文本、零 I/O、无随机源，摄取时算一次并随 case 落盘。
//!
//! 字段含义（与设计 v0.2 §2.2 一致）：
//! - error_code：错误行内 `key=val` 邻近 token（conn_id=9f2a、errno=28）
//! - file_loc：堆栈帧/行引用中的 `path:line`
//! - symbol：帧行内的方法/标识符词面
//! - num_fp：数字折叠后的行指纹（normalize 后含 `#` 的稳定串，跨日志去噪声）

use crate::analysis::{fails, frame, lineref};
use crate::analysis::{lowercase, normalize};
use crate::memory::store::Fields;

pub(crate) const CAP: usize = 16;

/// 四类结构化字段的稳定实体键（排序去重，供倒排与实体边共用）。
pub(crate) fn keys(f: &Fields) -> Vec<String> {
    let mut keys =
        Vec::with_capacity(f.error_code.len() + f.file_loc.len() + f.symbol.len() + f.num_fp.len());
    keys.extend(f.error_code.iter().cloned());
    keys.extend(f.file_loc.iter().cloned());
    keys.extend(f.symbol.iter().cloned());
    keys.extend(f.num_fp.iter().cloned());
    keys.sort();
    keys.dedup();
    keys
}

/// 从证据块抽取四类字段。逐行扫描，去重、截断、封顶。
///
/// 判定用归一化行（normalize，数字折叠），**提取用原始行**：norm 会把
/// `conn_id=9f2a` 折叠成 `conn_id=#f#a`、`Pool.java:214` 折叠成 `pool.java:#`，
/// 破坏 token/位置结构，故 token、位置、符号都从原始行取并统一小写。
pub(crate) fn extract(text: &str) -> Fields {
    let mut f = Fields::default();
    let mut seen: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut run = String::new();
    let mut ll = String::new();
    for raw in text.lines() {
        buf.clear();
        run.clear();
        normalize(raw, &mut buf, &mut run);
        ll.clear();
        lowercase(raw, &mut ll);
        if ll.trim().is_empty() {
            continue;
        }
        if fails(&ll) {
            tokens(raw, &mut f.error_code, &mut seen);
        }
        if frame(raw) || lineref(&ll) {
            location(raw, &mut f.file_loc, &mut seen);
            symbols(raw, &mut f.symbol, &mut seen);
        }
        if buf.contains('#') {
            push(&mut f.num_fp, &buf, &mut seen);
        }
    }
    f
}

/// 收集错误行中 `ident=num|hex` 形式的判别 token（统一小写）。
fn tokens(line: &str, out: &mut Vec<String>, seen: &mut Vec<String>) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        i = ident(line, bytes, i, out, seen);
    }
}

/// 扫描单个 ident；若其后接 `=` 则尝试收集值，否则推进到 ident 末尾。
fn ident(
    line: &str,
    bytes: &[u8],
    i: usize,
    out: &mut Vec<String>,
    seen: &mut Vec<String>,
) -> usize {
    if !start(bytes[i]) {
        return i + 1;
    }
    let s = i;
    let mut j = i;
    while j < bytes.len() && char(bytes[j]) {
        j += 1;
    }
    let key = &line[s..j];
    if j < bytes.len() && bytes[j] == b'=' {
        collect(line, bytes, j + 1, key, out, seen)
    } else {
        j
    }
}

/// 收集 `=` 后的值 token（含数字才入集），返回扫描终点。
fn collect(
    line: &str,
    bytes: &[u8],
    v: usize,
    key: &str,
    out: &mut Vec<String>,
    seen: &mut Vec<String>,
) -> usize {
    let mut j = v;
    while j < bytes.len() && value(bytes[j]) {
        j += 1;
    }
    if j > v && bytes[v..j].iter().any(|&b| b.is_ascii_digit()) {
        push(out, &format!("{key}={}", &line[v..j]).to_lowercase(), seen);
    }
    j
}

/// 收集 `path.ext:line` 形式的位置（统一小写）。
fn location(line: &str, out: &mut Vec<String>, seen: &mut Vec<String>) {
    // 从行尾向前找 `:digits` 之后逆推 `path.ext`。
    let bytes = line.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        if bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let mut j = i;
            while j > 0 && path(bytes[j - 1]) {
                j -= 1;
            }
            let loc = &line[j..(i + 1 + digits(line, i + 1))];
            if loc.contains('.') {
                push(out, &loc.to_lowercase(), seen);
            }
        }
    }
}

/// 收集帧行内标识符（至少 3 字符，非纯路径段；统一小写）。
fn symbols(line: &str, out: &mut Vec<String>, seen: &mut Vec<String>) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if start(bytes[i]) {
            let s = i;
            while i < bytes.len() && char(bytes[i]) {
                i += 1;
            }
            let w = &line[s..i];
            if w.len() >= 3 && !w.contains('.') && !file(w) {
                push(out, &w.to_lowercase(), seen);
            }
        } else {
            i += 1;
        }
    }
}

fn digits(line: &str, start: usize) -> usize {
    line.as_bytes()[start..]
        .iter()
        .take_while(|&&b| b.is_ascii_digit())
        .count()
}

fn push(out: &mut Vec<String>, v: &str, seen: &mut Vec<String>) {
    if out.len() >= CAP {
        return;
    }
    if v.len() > 128 {
        return;
    }
    if !seen.contains(&v.to_string()) {
        seen.push(v.to_string());
        out.push(v.to_string());
    }
}

fn start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}
fn char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
fn value(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_' || b == b':'
}
fn path(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'/' || b == b'\\' || b == b'-' || b == b'_'
}
fn file(w: &str) -> bool {
    w.contains('/') || w.ends_with(".rs") || w.ends_with(".java") || w == "at"
}

#[cfg(test)]
mod field_tests {
    use super::*;

    #[test]
    fn field_extract_smoke() {
        let text = concat!(
            "ERROR db timeout conn_id=9f2a errno=28\n",
            "    at Pool.java:214 (getConnection)\n",
            "    at rs/lib.rs:88 connect_timeout\n",
            "Root cause: pool exhausted\n",
        );
        let f = extract(text);
        assert!(
            f.error_code.iter().any(|t| t == "conn_id=9f2a"),
            "错误行应抽取 key=val：{:?}",
            f.error_code
        );
        assert!(
            f.error_code.iter().any(|t| t == "errno=28"),
            "errno 应入错误码：{:?}",
            f.error_code
        );
        assert!(
            f.file_loc.iter().any(|t| t == "pool.java:214"),
            "应抽取文件位置：{:?}",
            f.file_loc
        );
        assert!(
            f.file_loc.iter().any(|t| t == "rs/lib.rs:88"),
            "应抽取相对路径位置：{:?}",
            f.file_loc
        );
        assert!(
            f.symbol.iter().any(|t| t == "getconnection"),
            "帧内标识符应入 symbol：{:?}",
            f.symbol
        );
        assert!(
            f.symbol.iter().any(|t| t == "connect_timeout"),
            "帧内方法名应入 symbol：{:?}",
            f.symbol
        );
        assert!(
            f.num_fp.iter().any(|t| t.contains('#')),
            "数字折叠指纹应含 #：{:?}",
            f.num_fp
        );
        // 封顶与去重
        let big = "ERROR x=1\n".repeat(50);
        let fb = extract(&big);
        assert!(fb.error_code.len() <= CAP);
        assert_eq!(fb.error_code.len(), 1, "重复 token 必须去重");
    }
}

// ---- 场景桶 ----

const FAMILIES: [(&str, &[&str]); 5] = [
    (
        "db",
        &[
            // 注意：裸 "db" 不在子串表里（"db.java:4" 这类文件实体会被子串误判为
            // db 场景桶，导致编译失败等场景被误归类、query "compile fail" 漏召）。
            // 裸 "db" 由 bucket / seeds 用词级精确匹配单独处理。
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
pub(crate) fn bucket(fields: &[String], terms: &[String], text: &str) -> String {
    query(fields, terms, text)
}

/// query-aware 主桶：先用 case query 词表与字段判家族，query 没有家族证据时
/// 再回退正文。根因正文常含比 query 更宽的域词（例如“数据库权限变更生成超大
/// 文件”），只用正文会把 query 明明写 network/file 的 case 归到 db，随后被
/// 候选桶过滤误删。旧库仍按已存 bucket 使用；新摄取走本函数。
pub(crate) fn query(fields: &[String], query_terms: &[String], text: &str) -> String {
    // 裸 "db" 词级精确匹配（terms 是 query∪text 的切词表，"db.java:4" 是单一
    // token ≠ "db"，不会被误归 db 场景桶）：修 query "db timeout" 漏召的同时
    // 避免文件实体把编译等场景误归类。
    if query_terms.iter().any(|t| t == "db") {
        return "db".to_string();
    }
    let mut hay: Vec<String> = fields.iter().map(|s| fold(s)).collect();
    hay.extend(query_terms.iter().map(|s| fold(s)));
    if let Some(fam) = family(&hay, true) {
        return fam;
    }
    // query 只有通用 network 症状（timeout/connection）时，正文里的具体家族
    // 证据优先；否则 `Jdbc.java ... timeout` 会被 network 吞掉。
    let mut combined = hay;
    combined.push(fold(text));
    if let Some(fam) = family(&combined, false) {
        return fam;
    }
    "other".to_string()
}

/// case 的多标签场景桶：query 与正文任一命中即入选。
///
/// 单主桶无法表达跨域事故（例如“数据库权限变更生成超大文件并传播全网”既是 db
/// 也是 network）。召回过滤用多标签做“任一交集即保留”，避免同一事故的不同问法
/// 因单桶优先级不同而互相漏召；`bucket` 主桶仍保留给 stats/兼容旧布局。
pub(crate) fn buckets(fields: &[String], query_terms: &[String], text: &str) -> Vec<String> {
    let mut hay: Vec<String> = fields.iter().map(|s| fold(s)).collect();
    hay.extend(query_terms.iter().map(|s| fold(s)));
    hay.push(fold(text));
    let mut out = Vec::new();
    if query_terms.iter().any(|t| t == "db") {
        out.push("db".to_string());
    }
    for (fam, keys) in FAMILIES {
        if keys.iter().any(|k| hay.iter().any(|h| contains(h, k))) && !out.iter().any(|f| f == fam)
        {
            out.push(fam.to_string());
        }
    }
    out
}

fn family(hay: &[String], query_only: bool) -> Option<String> {
    for (fam, keys) in FAMILIES {
        let hit = keys.iter().any(|k| hay.iter().any(|h| contains(h, k)));
        if !hit {
            continue;
        }
        // query 侧只有通用 network 词时不算“明确网络”，交给正文回退。
        if query_only && fam == "network" {
            let specific = [
                "socket", "http", "http2", "http3", "tcp", "udp", "dns", "network", "网络",
            ]
            .iter()
            .any(|k| hay.iter().any(|h| contains(h, k)));
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
fn contains(hay: &str, key: &str) -> bool {
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
        let before_ok = i == 0 || !word(bytes[i - 1]);
        let after_ok = i + kb.len() == bytes.len() || !word(bytes[i + kb.len()]);
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

fn word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// query 候选桶集合：任一家族命中即入选（宁多勿漏，防漏召）。
pub(crate) fn seeds(query: &str) -> Vec<String> {
    let q = fold(query);
    let mut out: Vec<String> = FAMILIES
        .iter()
        .filter(|(_, keys)| keys.iter().any(|k| contains(&q, k)))
        .map(|(fam, _)| fam.to_string())
        .collect();
    // 裸 "db" 词级精确匹配（与 bucket 同口径）：query 含独立词 "db" 即
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
        .any(|k| contains(&q, k));
    if only_generic_network {
        return Vec::new();
    }
    out
}

/// 归一化小写（剥离 ANSI/CR 后做 Unicode 小写）。
///
/// 旧实现误用 `normalize`：它不做小写，`Jdbc.java`/`DATABASE` 等大写词
/// 匹配不到小写关键词表，导致本该归 db 的 case 被 timeout 抢先归到 network。
fn fold(s: &str) -> String {
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
    crate::analysis::lowercase(&cleaned, &mut out);
    out
}

#[cfg(test)]
mod bucket_tests {
    use super::*;

    #[test]
    fn bucket_detects_family() {
        assert_eq!(bucket(&["conn_id=9f2a".into()], &[], ""), "db");
        assert_eq!(bucket(&[], &["连接池".into()], ""), "db");
        assert_eq!(bucket(&[], &[], "socket timeout 网络"), "network");
        assert_eq!(bucket(&[], &[], "磁盘 空间不足"), "disk");
        assert_eq!(bucket(&[], &["cargo".into()], "error"), "build");
        assert_eq!(bucket(&[], &[], "NullPointerException stack"), "runtime");
        assert_eq!(bucket(&[], &[], "hello world"), "other");
        // 数据库 HA 拓扑词必须归 db：主库 failover / 主从复制 / 裂脑。
        assert_eq!(bucket(&[], &["failover".into()], ""), "db");
        assert_eq!(bucket(&[], &["replication".into()], ""), "db");
        assert_eq!(bucket(&[], &["主从".into()], ""), "db");
        // 大小写不能改变桶判定：`Jdbc.java` 必须先命中 db，而不是被
        // 后面的 timeout 抢先归到 network。
        assert_eq!(
            bucket(&[], &["query".into(), "timeout".into()], "at Jdbc.java:88"),
            "db"
        );
        assert_eq!(bucket(&[], &[], "DISK FULL"), "disk");
        // 来源 URL 的 `https` 不是 network 证据：带复盘链接的 case 仍按正文分类。
        assert_eq!(
            bucket(
                &[],
                &[],
                "升级配置冲突\n公开复盘, 来源: https://example.com/post"
            ),
            "other"
        );
        assert_eq!(
            bucket(
                &[],
                &[],
                "数据库连接失败\n公开复盘, 来源: https://example.com/post"
            ),
            "db"
        );
        // EBS 重新镜像属于存储域；根因里出现“网络事件”不能把它归 network。
        assert_eq!(
            bucket(&[], &[], "网络事件引发 EBS re-mirroring 风暴与容量耗尽"),
            "disk"
        );
        // DNS resolver / SERVFAIL / BGP 路由 / Blob 存储在复盘文本里可能没有
        // 通用词 dns/storage，仍须归到对应家族，避免桶过滤漏召。
        assert_eq!(
            bucket(
                &[],
                &[],
                "an internal software error made the public resolver return servfail"
            ),
            "network"
        );
        assert_eq!(
            bucket(&[], &[], "亚特兰大路由器一行配置错误导致 BGP 路由泄漏"),
            "network"
        );
        assert_eq!(
            bucket(
                &[],
                &[],
                "reverting the metadata change restored blob lookups"
            ),
            "disk"
        );
        assert_eq!(
            bucket(&[], &[], "网络事件引发 cloud 云盘 volume 大规模重新镜像"),
            "disk"
        );
        // `timeout` 不能盖过具体存储场景：disk/storage 优先于通用 network。
        assert_eq!(
            bucket(&[], &["disk".into(), "read".into(), "timeout".into()], ""),
            "disk"
        );
        // 裸 "db" 走词级精确匹配：独立 token "db" 归 db 桶；
        // "db.java:4" 是单一 token，不得误归 db 场景桶（编译失败场景归 build）。
        assert_eq!(bucket(&[], &["db".into()], ""), "db");
        let got = bucket(&[], &["db.java:4".into()], "compile fail");
        assert_eq!(got, "build", "文件实体不得把编译场景误归 db 桶");
        // query 明确写 network 时，根因里的“数据库”不得把 case 带偏到 db。
        assert_eq!(
            query(
                &[],
                &["oversized".into(), "file".into(), "network".into()],
                "机器人检测数据库权限变更生成超大文件"
            ),
            "network"
        );
        // query 没有家族证据时才回退正文。
        assert_eq!(query(&[], &["hello".into()], "数据库连接失败"), "db");
        assert_eq!(
            query(
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
        let multi = buckets(
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
    fn seeds_are_wide() {
        let c = seeds("数据库连接超时 conn_id");
        assert!(c.contains(&"db".to_string()), "候选桶应含 db: {:?}", c);
        let c2 = seeds("cargo build 磁盘满");
        assert!(c2.contains(&"build".to_string()) && c2.contains(&"disk".to_string()));
        let c3 = seeds("普通问题");
        assert!(c3.is_empty(), "无家族命中时空候选桶 = 全量扫（防漏召）");
        // 裸 "db" 必须命中 db 家族，保持桶判定对称并防漏召。
        let c4 = seeds("db timeout");
        assert!(
            c4.contains(&"db".to_string()),
            "裸 db 应命中 db 桶: {:?}",
            c4
        );
        // 子串不得误判："kubectl" 含 "db" 子串，但不含独立 token "db"。
        let c5 = seeds("kubectl get pods");
        assert!(
            !c5.contains(&"db".to_string()),
            "子串不得乱扩 db 桶: {:?}",
            c5
        );
        assert!(
            seeds("timeout").is_empty(),
            "通用 timeout 必须全量扫，不能硬筛 network"
        );
        assert!(seeds("connection").is_empty(), "通用 connection 必须全量扫");
        assert!(seeds("socket timeout").contains(&"network".to_string()));
        assert!(seeds("storage timeout").contains(&"disk".to_string()));
        // 主库 failover 的 query 即使带 "network partition" 也必须保留 db 候选桶，
        // 否则 db 桶 case 被网络桶过滤、主库故障切换直接漏召。
        let c6 = seeds("primary failover during network partition");
        assert!(
            c6.contains(&"db".to_string()),
            "主库 failover query 应候选 db 桶: {:?}",
            c6
        );
    }
}
