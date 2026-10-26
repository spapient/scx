//! scx setup：一键注册到 Claude / Codex / Cursor 的 MCP 配置。
//!
//! 行为契约：
//! - 注册名统一小写 `scx`，命令为当前二进制绝对路径 + 子命令 `mcp`（stdio）。
//! - Claude / Codex 优先调用官方 CLI（`claude mcp add` / `codex mcp add`），
//!   CLI 不可用或失败时兜底直接写配置文件。
//! - 幂等：已存在 `scx` 条目则更新 command/args，绝不重复追加。
//! - 写前备份原配置到 `<path>.scx.bak`（每次覆盖最新一份，可回滚）。
//! - 写后自检：spawn 自身 `mcp` 做 tools/list 握手并打印验证结果。
//! - 零第三方 crate、零网络：全部 std 实现；配置文件路径以 `$HOME` 为准。
//!
//! 配置写入目标：
//! - Claude：`claude mcp add` 优先；兜底 `~/.claude.json` 顶层 `mcpServers`。
//! - Codex：`codex mcp add` 优先；兜底 `~/.codex/config.toml` `[mcp_servers.scx]`。
//! - Cursor：`~/.cursor/mcp.json`（`mcpServers` 顶层）；若当前目录存在
//!   `.cursor/mcp.json` 则优先写项目配置。

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::json::{parse, Json};

/// 主入口：`scx setup [--claude|--codex|--cursor] [--project-cursor] [--help]`。
/// 返回进程退出码（0 = 全部成功）。
pub fn run(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return 0;
    }
    let only_claude = args.iter().any(|a| a == "--claude");
    let only_codex = args.iter().any(|a| a == "--codex");
    let only_cursor = args.iter().any(|a| a == "--cursor");
    let proj_cursor = args.iter().any(|a| a == "--project-cursor");
    let exe = match exe_path() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("scx setup: {e}");
            return 1;
        }
    };
    let mut bad = 0;
    if !only_codex && !only_cursor {
        bad += reg_and_check("Claude", claude_reg(&exe), &exe);
    }
    if !only_claude && !only_cursor {
        bad += reg_and_check("Codex", codex_reg(&exe), &exe);
    }
    if !only_claude && !only_codex {
        bad += reg_and_check("Cursor", cursor_reg(&exe, proj_cursor), &exe);
    }
    if bad > 0 {
        eprintln!("scx setup: {bad} agent(s) failed (see above; backups kept at *.scx.bak)");
        1
    } else {
        println!("scx setup: all agents registered (server name: scx)");
        0
    }
}

fn print_usage() {
    println!(
        "usage: scx setup [--claude] [--codex] [--cursor] [--project-cursor] [--help]\n\
         \n\
         Register the scx MCP stdio server into agent configs (idempotent).\n\
         --claude / --codex / --cursor  limit registration to that agent.\n\
         --project-cursor                write Cursor config to ./.cursor/mcp.json\n\
                                        (default: ~/.cursor/mcp.json).\n\
         Config files are backed up to <path>.scx.bak before writing;\n\
         a tools/list handshake runs after each successful registration."
    );
}

/// 注册 + 写后 tools/list 握手自检，汇总为退出码贡献。
fn reg_and_check(name: &str, r: Result<String, String>, exe: &Path) -> i32 {
    let line = match r {
        Ok(line) => line,
        Err(e) => {
            eprintln!("scx setup: [{name}] FAILED: {e}");
            return 1;
        }
    };
    println!("scx setup: [{name}] {line}");
    match handshake(exe) {
        Ok(h) => {
            println!("scx setup: [{name}] {h}");
            0
        }
        Err(e) => {
            eprintln!("scx setup: [{name}] handshake FAILED: {e}");
            1
        }
    }
}

fn exe_path() -> Result<PathBuf, String> {
    env::current_exe().map_err(|e| format!("cannot locate own binary: {e}"))
}

fn home_dir() -> Result<PathBuf, String> {
    match env::var("HOME") {
        Ok(h) if !h.is_empty() => Ok(PathBuf::from(h)),
        _ => Err("HOME is not set".to_string()),
    }
}

// ---------- Claude ----------

fn claude_reg(exe: &Path) -> Result<String, String> {
    if Command::new("claude").arg("--version").output().is_ok() {
        let out = Command::new("claude")
            .args(["mcp", "add", "scx", "--"])
            .arg(exe)
            .arg("mcp")
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                return Ok("registered via claude mcp add".to_string());
            }
        }
    }
    let path = home_dir()?.join(".claude.json");
    let entry = entry_json(exe);
    json_reg(&path, "mcpServers", "scx", &entry)?;
    Ok(format!("wrote {}", path.display()))
}

// ---------- Codex ----------

fn codex_reg(exe: &Path) -> Result<String, String> {
    if Command::new("codex").arg("--version").output().is_ok() {
        let out = Command::new("codex")
            .args(["mcp", "add", "scx", "--"])
            .arg(exe)
            .arg("mcp")
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                return Ok("registered via codex mcp add".to_string());
            }
        }
    }
    let dir = home_dir()?.join(".codex");
    let path = dir.join("config.toml");
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    toml_reg(&path, exe)?;
    Ok(format!("wrote {}", path.display()))
}

// ---------- Cursor ----------

fn cursor_reg(exe: &Path, project: bool) -> Result<String, String> {
    let path = if project {
        env::current_dir()
            .map_err(|e| format!("cannot read cwd: {e}"))?
            .join(".cursor/mcp.json")
    } else {
        home_dir()?.join(".cursor/mcp.json")
    };
    let entry = entry_json(exe);
    json_reg(&path, "mcpServers", "scx", &entry)?;
    Ok(format!("wrote {}", path.display()))
}

// ---------- 配置文件写入 ----------

/// 统一 MCP stdio 条目（type/command/args）。
fn entry_json(exe: &Path) -> Json {
    Json::Obj(vec![
        ("type".to_string(), Json::Str("stdio".to_string())),
        (
            "command".to_string(),
            Json::Str(exe.to_string_lossy().into_owned()),
        ),
        (
            "args".to_string(),
            Json::Arr(vec![Json::Str("mcp".to_string())]),
        ),
    ])
}

/// JSON 配置：读 → 树改写（key 存在则更新，否则插入）→ 备份 → 原子写。
fn json_reg(path: &Path, section: &str, key: &str, entry: &Json) -> Result<(), String> {
    let existing = match fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let mut root = match &existing {
        Some(t) => parse(t).map_err(|e| format!("{}: {e}", path.display()))?,
        None => Json::Obj(vec![]),
    };
    set_entry(&mut root, section, key, entry);
    let out = write_json(&root);
    if existing.as_deref() == Some(out.trim_end()) {
        return Ok(()); // 内容未变：幂等跳过，不写不备份。
    }
    backup(path)?;
    write_atomic(path, &out)?;
    Ok(())
}

/// TOML 配置（Codex config.toml）：文本级追加/更新 `[mcp_servers.scx]` 段。
fn toml_reg(path: &Path, exe: &Path) -> Result<(), String> {
    let existing = match fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let command = format!(
        "command = \"{}\"",
        exe.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    );
    let section = "[mcp_servers.scx]";
    let mut text = existing.unwrap_or_default();
    if text.contains(section) {
        // 已注册：原位更新 command 行（args 恒为 ["mcp"]，无需重写）。
        let updated = update_toml_command(&text, &command);
        if updated != text {
            backup(path)?;
            write_atomic(path, &updated)?;
        }
    } else {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(section);
        text.push('\n');
        text.push_str(&command);
        text.push('\n');
        text.push_str("args = [\"mcp\"]\n");
        backup(path)?;
        write_atomic(path, &text)?;
    }
    Ok(())
}

/// 段内更新 command 行（保持文件其余内容逐字节不变）。
fn update_toml_command(text: &str, command: &str) -> String {
    let mut out = String::new();
    let mut in_scx = false;
    for line in text.lines() {
        let t = line.trim();
        if t == "[mcp_servers.scx]" {
            in_scx = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_scx && t.starts_with('[') && t.ends_with(']') {
            in_scx = false;
        }
        // command 键：允许任意空白后跟 `=`（`command =` / `command=` / `command  =`），
        // 注释行（`#` 开头）因 trim 后带 `#` 前缀不会误判。
        if in_scx && t.starts_with("command") {
            let rest = &t["command".len()..];
            if rest.trim_start().starts_with('=') {
                out.push_str(command);
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 树改写：section 对象内设置 key（已存在更新，否则追加）。
fn set_entry(root: &mut Json, section: &str, key: &str, entry: &Json) {
    let kv = obj_mut(root);
    let idx = kv.iter().position(|(k, _)| k == section);
    let mut sec = match idx {
        Some(i) => take_obj(&mut kv[i].1),
        None => Vec::new(),
    };
    upsert(&mut sec, key, entry);
    match idx {
        Some(i) => kv[i].1 = Json::Obj(sec),
        None => kv.push((section.to_string(), Json::Obj(sec))),
    }
}

/// 顶层对象视图（非对象就地重建为空对象）。
fn obj_mut(root: &mut Json) -> &mut Vec<(String, Json)> {
    if !matches!(root, Json::Obj(_)) {
        *root = Json::Obj(vec![]);
    }
    let Json::Obj(kv) = root else { unreachable!() };
    kv
}

/// 取走对象内容；非对象就地替换为空对象。
fn take_obj(v: &mut Json) -> Vec<(String, Json)> {
    match v {
        Json::Obj(s) => std::mem::take(s),
        _ => {
            *v = Json::Obj(vec![]);
            Vec::new()
        }
    }
}

/// 键存在则更新，否则追加。
fn upsert(sec: &mut Vec<(String, Json)>, key: &str, entry: &Json) {
    match sec.iter_mut().find(|(k, _)| k == key) {
        Some((_, v)) => *v = entry.clone(),
        None => sec.push((key.to_string(), entry.clone())),
    }
}

// ---------- 序列化 ----------

/// 紧凑两空格缩进 JSON 序列化（键序保持解析序）。
fn write_json(v: &Json) -> String {
    let mut s = String::new();
    write_node(v, &mut s, 0);
    s.push('\n');
    s
}

fn write_node(v: &Json, out: &mut String, indent: usize) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(bool_lit(*b)),
        Json::Num(n) => write_num(*n, out),
        Json::Str(s) => out.push_str(&crate::json::string(s)),
        Json::Arr(items) => write_arr(items, out, indent),
        Json::Obj(kv) => write_obj(kv, out, indent),
    }
}

fn bool_lit(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

fn write_num(n: f64, out: &mut String) {
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        out.push_str(&format!("{}", n as i64));
    } else {
        out.push_str(&format!("{n}"));
    }
}

fn write_arr(items: &[Json], out: &mut String, indent: usize) {
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[\n");
    for (i, it) in items.iter().enumerate() {
        pad(out, indent + 1);
        write_node(it, out, indent + 1);
        if i + 1 < items.len() {
            out.push(',');
        }
        out.push('\n');
    }
    pad(out, indent);
    out.push(']');
}

fn write_obj(kv: &[(String, Json)], out: &mut String, indent: usize) {
    if kv.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push_str("{\n");
    for (i, (k, val)) in kv.iter().enumerate() {
        pad(out, indent + 1);
        out.push_str(&crate::json::string(k));
        out.push_str(": ");
        write_node(val, out, indent + 1);
        if i + 1 < kv.len() {
            out.push(',');
        }
        out.push('\n');
    }
    pad(out, indent);
    out.push('}');
}

fn pad(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

// ---------- 文件安全 ----------

/// 备份：原文件复制为 `<path>.scx.bak`（覆盖最新一份）。
fn backup(path: &Path) -> Result<(), String> {
    let bak = backup_path(path);
    match fs::copy(path, &bak) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("backup {}: {e}", bak.display())),
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".scx.bak");
    path.with_file_name(name)
}

/// 原子写：临时文件 + rename（避免半写文件被 agent 读到）。
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let parent = match path.parent() {
        Some(p) if p.as_os_str().is_empty() => Path::new("."),
        Some(p) => p,
        None => return Err(format!("no parent for {}", path.display())),
    };
    if !parent.exists() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = parent.join(format!(".scx-setup-{}", std::process::id()));
    fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    // 保留原文件权限：fs::write 按 umask 创建 0644，会把原 0600 的配置放宽。
    // 原文件不存在（首次注册）时跳过，沿用默认 umask。
    if let Ok(meta) = fs::metadata(path) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))?;
    Ok(())
}

// ---------- 写后自检 ----------

/// stdio 握手：spawn 自身 `mcp`，initialize → tools/list，验证 compress_context。
pub fn handshake(exe: &Path) -> Result<String, String> {
    let mut child = Command::new(exe)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {exe:?} mcp: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "cannot take child stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "cannot take child stdout".to_string())?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let _ = stdin.write_all(
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"scx-setup\",\"version\":\"0\"}}}\n",
    );
    let _ = stdin.flush();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("initialize read: {e}"))?;
    let _ = stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n");
    let _ = stdin.flush();
    let mut list = String::new();
    reader
        .read_line(&mut list)
        .map_err(|e| format!("tools/list read: {e}"))?;
    // 关闭子进程 stdin（EOF）使其自然退出，避免 wait 死锁。
    drop(stdin);
    let _ = child.wait();
    if list.contains("compress_context") {
        Ok("tools/list handshake OK (compress_context present)".to_string())
    } else {
        Err(format!(
            "tools/list handshake failed: unexpected reply {list:?}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_toml_command_matches_any_whitespace_and_skips_comments() {
        // `command =` / `command=` / `command  =` 都应原位替换；注释里的
        // `[mcp_servers.scx]` 或 `command =` 不误判。
        let text = "[mcp_servers.scx]\ncommand = \"/old/bin\"\nargs = [\"mcp\"]\n# [mcp_servers.scx]\n# command = should-not-change\n";
        let out = update_toml_command(text, "command = \"/new/bin\"");
        assert!(
            out.contains("command = \"/new/bin\""),
            "command = 应替换: {out}"
        );
        assert!(!out.contains("\"/old/bin\""), "旧 command 应被替换: {out}");
        assert!(
            out.contains("# command = should-not-change"),
            "注释不得改动: {out}"
        );
        assert!(out.contains("args = [\"mcp\"]"), "args 保留: {out}");

        // 多空格 `command   =` 也应匹配。
        let multi = "[mcp_servers.scx]\ncommand   = \"/old\"\n";
        let out2 = update_toml_command(multi, "command = \"/new\"");
        assert!(
            out2.contains("command = \"/new\""),
            "多空格 command 应替换: {out2}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "scx-setup-perm-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&path, "new").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "原子写后应保留原 0600 权限，而非放宽到 0644");
        let _ = fs::remove_dir_all(&dir);
    }
}
