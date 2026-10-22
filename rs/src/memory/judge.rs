//! P4.4+ 优化⑩：盲测裁决工具（`--judge <baseline.json>`）。
//!
//! 消除"人工直接接受预填 rel = 自动标注自证"：`--annotate-raw` 输出的盲测集
//! 每条的 rel 为空（序列化为 `"judge": null`），`--eval` 遇到未裁决条目一律拒绝。
//! 本工具逐条打印 query + 盲测摘要（note: target/hint/fix），从 stdin 录入
//! 相关度裁决（2=强 / 1=弱 / 0=无关 / s=跳过），把 judge:null 替换为
//! rel 字典 + judge 元数据（by / date），原子写回文件。
//!
//! 纪律（防再引入偏差）：
//! - 裁决只看 query 与目标摘要，不打印任何 A/B 分数、不引用 --annotate 的放行结果；
//! - 未裁决条目（跳过）保持 judge:null，--eval 仍拒绝 → 报告必须声明"漏裁未纳入"。

use crate::memory::json::{self, Val};
use std::io::{self, BufRead, Write};

/// 逐条裁决：把每条未裁决 case 的 `"judge": null` 替换为
/// `"rel": {<expected>: value}` + `"judge": {"by": .., "date": ..}`，写回文件。
/// 已裁决（带 rel 或 judge 对象）的条目直接跳过；EOF 中止且不写回。
pub fn judge(path: &str, by: &str) -> io::Result<Vec<String>> {
    let txt = std::fs::read_to_string(path)?;
    let mut v = json::parse(&txt).map_err(io::Error::other)?;
    {
        let cases = match &mut v {
            Val::Obj(kvs) => match kvs.iter_mut().find(|(k, _)| k == "cases") {
                Some((_, Val::Arr(items))) => items,
                _ => return Err(io::Error::other("judge: baseline missing \"cases\" array")),
            },
            _ => return Err(io::Error::other("judge: baseline must be an object")),
        };
        if cases.is_empty() {
            return Err(io::Error::other("judge: no cases to adjudicate"));
        }
        let total = cases.len();
        let stdin = io::stdin();
        let mut lines = stdin.lock().lines();
        let mut judged = 0usize;
        let mut skipped = 0usize;
        let mut judged_rows: Vec<String> = Vec::new();
        for (i, item) in cases.iter_mut().enumerate() {
            let kvs = match item {
                Val::Obj(kvs) => kvs,
                _ => return Err(io::Error::other(format!("judge: cases[{i}] not an object"))),
            };
            // 已裁决（rel 字典或 judge 对象）→ 跳过。
            let already = kvs.iter().any(|(k, val)| match k.as_str() {
                "rel" => matches!(val, Val::Obj(_)),
                "judge" => matches!(val, Val::Obj(_)),
                _ => false,
            });
            if already {
                skipped += 1;
                continue;
            }
            let layer = get_str(kvs, "layer");
            let query = get_str(kvs, "query");
            let note = get_str(kvs, "note");
            let target = get_first_expected(kvs).unwrap_or_default();
            // 盲测展示：只给 query + 目标摘要，无任何 A/B 分数。
            println!("[{}/{}] layer={} query=\"{}\"", i + 1, total, layer, query);
            if !note.is_empty() {
                println!("  hint: {note}");
            }
            let rel = loop {
                print!("  rel? [2=strong 1=weak 0=unrelated s=skip] > ");
                io::stdout().flush()?;
                let buf = match lines.next() {
                    Some(Ok(l)) => l,
                    Some(Err(e)) => return Err(e),
                    None => return Err(io::Error::other(
                        "judge: EOF - nothing written; rerun to resume (unjudged stay judge:null)",
                    )),
                };
                match buf.trim() {
                    "2" => break Some(2.0),
                    "1" => break Some(1.0),
                    "0" => break Some(0.0),
                    "s" | "S" | "" => break None,
                    _ => println!("  (input: 2/1/0/s)"),
                }
            };
            match rel {
                Some(r) => {
                    // 替换/新增 rel 与 judge 键（原地，不重复 push）。
                    upsert(kvs, "rel", Val::Obj(vec![(target, Val::Num(r))]));
                    let date = crate::memory::util::utc_now();
                    upsert(
                        kvs,
                        "judge",
                        Val::Obj(vec![
                            ("by".to_string(), Val::Str(by.to_string())),
                            ("date".to_string(), Val::Str(date)),
                        ]),
                    );
                    judged += 1;
                    judged_rows.push(format!(
                        "  judged[{}] {} query=\"{}\"",
                        fmt_rel(r),
                        layer,
                        query
                    ));
                }
                None => {
                    skipped += 1;
                }
            }
        }
        let out = json::write(&v);
        std::fs::write(path, out)?;
        let mut lines_out = vec![
            format!("judge: judged={judged} skipped={skipped} total={total}"),
            format!("  by={by} date={}", crate::memory::util::utc_now()),
            format!("  out: {path} (skipped keep judge:null, --eval still rejects them)"),
        ];
        lines_out.extend(judged_rows);
        Ok(lines_out)
    }
}

/// 原地插入或替换键（保留键序稳定：替换在原位，新增在尾部）。
fn upsert(kvs: &mut Vec<(String, Val)>, key: &str, val: Val) {
    if let Some(pos) = kvs.iter().position(|(k, _)| k == key) {
        kvs[pos] = (key.to_string(), val);
    } else {
        kvs.push((key.to_string(), val));
    }
}

/// 读字符串键（缺失 → 空串）。
fn get_str(kvs: &[(String, Val)], key: &str) -> String {
    kvs.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| match v {
            Val::Str(s) => s.clone(),
            _ => String::new(),
        })
        .unwrap_or_default()
}

/// expected 数组首元素（hash16 目标）。
fn get_first_expected(kvs: &[(String, Val)]) -> Option<String> {
    kvs.iter()
        .find(|(k, _)| k == "expected")
        .and_then(|(_, v)| match v {
            Val::Arr(items) => items.iter().find_map(|e| match e {
                Val::Str(s) => Some(s.clone()),
                _ => None,
            }),
            _ => None,
        })
}

/// rel 打印：2.0 → "2"，1.0 → "1"。
fn fmt_rel(r: f64) -> String {
    if r == r.floor() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}
