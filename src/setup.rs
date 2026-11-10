//! scx setup：一键注册到 Claude / Codex / Cursor 的 MCP 配置与事件 hook。
//!
//! 行为契约：
//! - 注册名统一小写 `scx`，命令为当前二进制绝对路径 + 子命令 `mcp`（stdio）。
//! - Claude / Codex 优先调用官方 CLI（`claude mcp add` / `codex mcp add`），
//!   CLI 不可用或失败时兜底直接写配置文件。
//! - 幂等：已存在 `scx` 条目则更新 command/args，绝不重复追加；hook 组按
//!   自身二进制名原位替换，外部配置逐字节保留。
//! - 写前备份原配置到 `<path>.scx.bak`（每次覆盖最新一份，可回滚）。
//! - 写后自检：spawn 自身 `mcp` 做 tools/list 握手并打印验证结果。
//! - 零第三方 crate、零网络：全部 std 实现；配置文件路径优先按 HOME / USERPROFILE
//!   / HOMEDRIVE+HOMEPATH 解析，兼容 Windows 与 Unix 目录布局。
//!
//! 配置写入目标：
//! - Claude：MCP 兜底 `~/.claude.json` 顶层 `mcpServers`；hook 写
//!   `~/.claude/settings.json` 的 `hooks` 段。
//! - Codex：MCP 兜底 `~/.codex/config.toml` `[mcp_servers.scx]`；hook 写
//!   `~/.codex/hooks.json`（`CODEX_HOME` 可重定位），运行前需在 /hooks 信任。
//! - Cursor：`~/.cursor/mcp.json`（`mcpServers` 顶层）；产品无 hook 面，
//!   仅注册连接器；若当前目录存在 `.cursor/mcp.json` 则优先写项目配置。

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
        let _ = crate::ops::out_line("scx setup: all agents registered (server name: scx)");
        0
    }
}

fn print_usage() {
    let _ = crate::ops::out_line(
        "usage: scx setup [--claude] [--codex] [--cursor] [--project-cursor] [--help]\n\
         \n\
         Register the scx MCP stdio server and event hooks into agent configs\n\
         (idempotent). Claude hooks go to ~/.claude/settings.json; Codex hooks to\n\
         ~/.codex/hooks.json (or $CODEX_HOME/hooks.json); Cursor hooks to\n\
         ~/.cursor/hooks.json (official hooks schema).\n\
         --claude / --codex / --cursor  limit registration to that agent.\n\
         --project-cursor                write Cursor MCP config to ./.cursor/mcp.json\n\
                                         (default: ~/.cursor/mcp.json).\n\
         Config files are backed up to <path>.scx.bak before writing;\n\
         a tools/list handshake runs after each successful registration.\n\
         Codex hooks require trust review in the /hooks panel before they run.",
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
    let _ = crate::ops::out_line(&format!("scx setup: [{name}] {line}"));
    match handshake(exe) {
        Ok(h) => {
            let _ = crate::ops::out_line(&format!("scx setup: [{name}] {h}"));
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
    home_from(
        env::var("HOME").ok().as_deref(),
        env::var("USERPROFILE").ok().as_deref(),
        env::var("HOMEDRIVE").ok().as_deref(),
        env::var("HOMEPATH").ok().as_deref(),
        cfg!(windows),
    )
}

/// 纯函数版 home 解析：环境变量作为入参，便于无副作用测试。
/// 查找顺序：HOME → USERPROFILE（空白值跳过）；Windows 下两者都缺失时
/// 用 HOMEDRIVE+HOMEPATH 组合。
fn home_from(
    home: Option<&str>,
    userprofile: Option<&str>,
    homedrive: Option<&str>,
    homepath: Option<&str>,
    is_windows: bool,
) -> Result<PathBuf, String> {
    for value in [home, userprofile].into_iter().flatten() {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    if is_windows {
        let drive = homedrive.map(str::trim).filter(|v| !v.is_empty());
        let path = homepath.map(str::trim).filter(|v| !v.is_empty());
        if let (Some(drive), Some(path)) = (drive, path) {
            return Ok(PathBuf::from(format!("{drive}{path}")));
        }
    }
    Err("HOME / USERPROFILE / HOMEDRIVE+HOMEPATH is not set".to_string())
}

// ---------- Claude ----------

fn claude_reg(exe: &Path) -> Result<String, String> {
    let mcp = if Command::new("claude").arg("--version").output().is_ok() {
        let out = Command::new("claude")
            .args(["mcp", "add", "scx", "--"])
            .arg(exe)
            .arg("mcp")
            .output();
        match out {
            Ok(o) if o.status.success() => "registered via claude mcp add".to_string(),
            _ => {
                let path = home_dir()?.join(".claude.json");
                let entry = entry_json(exe);
                json_reg(&path, "mcpServers", "scx", &entry)?;
                format!("wrote {}", path.display())
            }
        }
    } else {
        let path = home_dir()?.join(".claude.json");
        let entry = entry_json(exe);
        json_reg(&path, "mcpServers", "scx", &entry)?;
        format!("wrote {}", path.display())
    };
    // 事件 hook 写入用户级 settings.json（安装时强制挂接）。
    let hook_path = home_dir()?.join(".claude/settings.json");
    hooks_reg(&hook_path, exe, false)?;
    Ok(format!("{mcp}; hooks wrote {}", hook_path.display()))
}

// ---------- Codex ----------

/// Codex MCP 注册：官方 CLI 优先，失败兜底写 config.toml。
fn codex_mcp(exe: &Path) -> Result<String, String> {
    let cli_ok = Command::new("codex").arg("--version").output().is_ok();
    if cli_ok {
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
    let dir = codex_home()?;
    let path = dir.join("config.toml");
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    toml_reg(&path, exe)?;
    Ok(format!("wrote {}", path.display()))
}

/// Codex 事件 hook 写入独立 hooks.json（CODEX_HOME 可重定位）。
fn codex_hooks(exe: &Path) -> Result<String, String> {
    let dir = codex_home()?;
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    let hook_path = dir.join("hooks.json");
    hooks_reg(&hook_path, exe, true)?;
    Ok(hook_path.display().to_string())
}

fn codex_reg(exe: &Path) -> Result<String, String> {
    let mcp = codex_mcp(exe)?;
    let hook = codex_hooks(exe)?;
    // Codex 按定义哈希建立信任：未审核前 hook 不执行，必须显式提示。
    eprintln!(
        "scx setup: [Codex] review and trust the new hooks in the /hooks panel \
(trust is keyed by the hook hash); until trusted, the hooks are skipped"
    );
    Ok(format!("{mcp}; hooks wrote {hook}"))
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
    // 官方 hooks.json：用户级事件 hook（project 模式不写，项目 hook 走仓库内文件）。
    let hooks = if project {
        String::new()
    } else {
        let hook_path = home_dir()?.join(".cursor/hooks.json");
        cursor_hooks_reg(&hook_path, exe)?;
        format!("; hooks wrote {}", hook_path.display())
    };
    Ok(format!("wrote {}{hooks}", path.display()))
}

/// Cursor 事件表：内部事件名 → 官方 camelCase 名与正则 matcher。
struct CursorHook {
    event: &'static str,
    matcher: &'static str,
}

const CURSOR_HOOKS: &[CursorHook] = &[
    CursorHook {
        event: "sessionStart",
        matcher: "",
    },
    CursorHook {
        event: "beforeSubmitPrompt",
        matcher: "",
    },
    CursorHook {
        event: "postToolUse",
        matcher: "Shell|Read|Grep|Glob",
    },
    CursorHook {
        event: "stop",
        matcher: "",
    },
];

/// 构造 Cursor 官方 hooks 树：`{"version":1,"hooks":{事件:[定义]}}`，定义为扁平结构。
fn cursor_hooks_tree(exe: &Path) -> Json {
    let command = format!("{} hook", exe.display());
    let mut events = Vec::new();
    for spec in CURSOR_HOOKS {
        let mut def = vec![
            ("command".to_string(), Json::Str(command.clone())),
            ("timeout".to_string(), Json::Num(10.0)),
        ];
        if !spec.matcher.is_empty() {
            def.push(("matcher".to_string(), Json::Str(spec.matcher.to_string())));
        }
        events.push((spec.event.to_string(), Json::Arr(vec![Json::Obj(def)])));
    }
    Json::Obj(vec![
        ("version".to_string(), Json::Num(1.0)),
        ("hooks".to_string(), Json::Obj(events)),
    ])
}

/// 判定一个扁平定义是否为 scx 写入（命令含自身二进制名且以 hook 结尾）。
fn is_scx_def(def: &Json, exe_name: &str) -> bool {
    def.get("command")
        .and_then(Json::str)
        .is_some_and(|c| c.contains(exe_name) && c.trim_end().ends_with(" hook"))
}

/// 合并 Cursor 扁平事件表：旧 scx 定义剔除，外部定义保留，标准定义追加。
fn merge_cursor_hooks(root: &mut Json, tree: &Json, exe_name: &str) {
    let top = obj_mut(root);
    // 官方 schema 版本号随标准树写入，仅在缺失时补（保持键序：version 在首）。
    if tree.get("version").is_some() && top.iter().all(|(k, _)| k != "version") {
        top.insert(0, ("version".to_string(), Json::Num(1.0)));
    }
    let Some(Json::Obj(canon_events)) = tree.get("hooks") else {
        return;
    };
    let hidx = top.iter().position(|(k, _)| k == "hooks");
    let mut hook_obj = match hidx {
        Some(i) => take_obj(&mut top[i].1),
        None => Vec::new(),
    };
    for (event, canon) in canon_events {
        merge_cursor_event(&mut hook_obj, event, canon, exe_name);
    }
    match hidx {
        Some(i) => top[i].1 = Json::Obj(hook_obj),
        None => top.push(("hooks".to_string(), Json::Obj(hook_obj))),
    }
}

/// 合并单个 cursor 事件：取出既有定义、剔除 scx 旧项、追加标准项后写回（幂等）。
fn merge_cursor_event(
    hook_obj: &mut Vec<(String, Json)>,
    event: &str,
    canon: &Json,
    exe_name: &str,
) {
    let eidx = hook_obj.iter().position(|(k, _)| k == event);
    let mut defs = match eidx {
        Some(i) => match &mut hook_obj[i].1 {
            Json::Arr(a) => std::mem::take(a),
            other => {
                take_obj(other);
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    defs.retain(|d| !is_scx_def(d, exe_name));
    if let Json::Arr(c) = canon {
        defs.extend(c.iter().cloned());
    }
    let arr = Json::Arr(defs);
    match eidx {
        Some(i) => hook_obj[i].1 = arr,
        None => hook_obj.push((event.to_string(), arr)),
    }
}

/// Cursor hooks.json 写入：读 → 合并 → 备份 → 原子写，幂等。
fn cursor_hooks_reg(path: &Path, exe: &Path) -> Result<(), String> {
    let existing = match fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let mut root = match &existing {
        Some(t) => parse(t).map_err(|e| format!("{}: {e}", path.display()))?,
        None => Json::Obj(vec![]),
    };
    let tree = cursor_hooks_tree(exe);
    let exe_name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    merge_cursor_hooks(&mut root, &tree, &exe_name);
    let out = write_json(&root);
    if existing.as_deref() == Some(out.trim_end()) {
        return Ok(());
    }
    backup(path)?;
    write_atomic(path, &out)?;
    Ok(())
}

// ---------- 事件 hook 注册（安装时写入，客户端侧强制触发） ----------

/// 事件规格：`cm` 为 Claude Code matcher（逗号枚举），`rx` 为 Codex matcher
/// （正则口径，同文件独立写入）。
struct HookSpec {
    event: &'static str,
    cm: &'static str,
    rx: &'static str,
}

const HOOKS: &[HookSpec] = &[
    HookSpec {
        event: "SessionStart",
        cm: "startup,resume,clear,compact,fork",
        rx: "startup|resume|clear|compact|fork",
    },
    HookSpec {
        event: "UserPromptSubmit",
        cm: "",
        rx: "",
    },
    HookSpec {
        event: "PostToolUse",
        cm: "Bash,Read,Grep,Glob",
        rx: "Bash|Read|Grep|Glob",
    },
    HookSpec {
        event: "Stop",
        cm: "",
        rx: "",
    },
];

/// 构造官方 hook 配置树（事件 → matcher 组 → command handler）。
fn hooks_tree(exe: &Path, codex: bool) -> Json {
    let command = format!("{} hook", exe.display());
    let mut events = Vec::new();
    for spec in HOOKS {
        let matcher = if codex { spec.rx } else { spec.cm };
        let handler = Json::Obj(vec![
            ("type".to_string(), Json::Str("command".to_string())),
            ("command".to_string(), Json::Str(command.clone())),
            ("timeout".to_string(), Json::Num(10.0)),
        ]);
        let group = Json::Obj(vec![
            ("matcher".to_string(), Json::Str(matcher.to_string())),
            ("hooks".to_string(), Json::Arr(vec![handler])),
        ]);
        events.push((spec.event.to_string(), Json::Arr(vec![group])));
    }
    Json::Obj(vec![("hooks".to_string(), Json::Obj(events))])
}

/// 判定一个 matcher 组是否为 scx 写入的组（按自身二进制名与 hook 子命令）。
fn is_scx_group(group: &Json, exe_name: &str) -> bool {
    let Some(handlers) = group.get("hooks") else {
        return false;
    };
    let Json::Arr(handlers) = handlers else {
        return false;
    };
    handlers.iter().any(|h| {
        h.get("command")
            .and_then(Json::str)
            .is_some_and(|c| c.contains(exe_name) && c.trim_end().ends_with(" hook"))
    })
}

/// 把 scx 的事件组合并进已有配置：旧 scx 组原位替换，其余配置逐字节保留。
/// 取走某事件现有组（非数组/非对象按空处理）。
fn take_groups(hook_obj: &mut [(String, Json)], eidx: usize) -> Vec<Json> {
    match &mut hook_obj[eidx].1 {
        Json::Arr(a) => std::mem::take(a),
        other => {
            take_obj(other);
            Vec::new()
        }
    }
}

/// 合并单个事件：剔除旧 scx 组，追加标准组，写回原位置或新键。
fn merge_event(
    hook_obj: &mut Vec<(String, Json)>,
    event: &str,
    canon_groups: &Json,
    exe_name: &str,
) {
    let eidx = hook_obj.iter().position(|(k, _)| k == event);
    let mut groups = match eidx {
        Some(i) => take_groups(hook_obj, i),
        None => Vec::new(),
    };
    groups.retain(|g| !is_scx_group(g, exe_name));
    if let Json::Arr(canon) = canon_groups {
        groups.extend(canon.iter().cloned());
    }
    let arr = Json::Arr(groups);
    match eidx {
        Some(i) => hook_obj[i].1 = arr,
        None => hook_obj.push((event.to_string(), arr)),
    }
}

fn merge_hooks(root: &mut Json, tree: &Json, exe_name: &str) {
    let top = obj_mut(root);
    let hidx = top.iter().position(|(k, _)| k == "hooks");
    let mut hook_obj = match hidx {
        Some(i) => take_obj(&mut top[i].1),
        None => Vec::new(),
    };
    let Some(Json::Obj(canon_events)) = tree.get("hooks") else {
        return;
    };
    for (event, canon_groups) in canon_events {
        merge_event(&mut hook_obj, event, canon_groups, exe_name);
    }
    match hidx {
        Some(i) => top[i].1 = Json::Obj(hook_obj),
        None => top.push(("hooks".to_string(), Json::Obj(hook_obj))),
    }
}

/// hook 配置文件写入：读 → 合并 → 备份 → 原子写，幂等。
fn hooks_reg(path: &Path, exe: &Path, codex: bool) -> Result<(), String> {
    let existing = match fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let mut root = match &existing {
        Some(t) => parse(t).map_err(|e| format!("{}: {e}", path.display()))?,
        None => Json::Obj(vec![]),
    };
    let tree = hooks_tree(exe, codex);
    let exe_name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    merge_hooks(&mut root, &tree, &exe_name);
    let out = write_json(&root);
    if existing.as_deref() == Some(out.trim_end()) {
        return Ok(());
    }
    backup(path)?;
    write_atomic(path, &out)?;
    Ok(())
}

/// Codex 配置目录：优先 `CODEX_HOME`，否则 `~/.codex`。
fn codex_home() -> Result<PathBuf, String> {
    if let Ok(v) = env::var("CODEX_HOME") {
        if !v.trim().is_empty() {
            return Ok(PathBuf::from(v));
        }
    }
    Ok(home_dir()?.join(".codex"))
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
        return Ok(());
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
    let mut text = existing.unwrap_or_default();
    let has_section = text.lines().any(|l| header(l.trim()));
    if has_section {
        let updated = update_toml_command(&text, &command);
        if updated != text {
            backup(path)?;
            write_atomic(path, &updated)?;
        }
    } else {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("[mcp_servers.scx]\n");
        text.push_str(&command);
        text.push('\n');
        text.push_str("args = [\"mcp\"]\n");
        backup(path)?;
        write_atomic(path, &text)?;
    }
    Ok(())
}

/// 段头判定：`[mcp_servers.scx]` 允许行尾注释（`# note`），但不匹配更长的段名。
fn header(t: &str) -> bool {
    t.strip_prefix("[mcp_servers.scx]").is_some_and(|rest| {
        let rest = rest.trim_start();
        rest.is_empty() || rest.starts_with('#')
    })
}

/// 段内更新 command 行（保持文件其余内容逐字节不变）。
fn update_toml_command(text: &str, command: &str) -> String {
    let mut out = String::new();
    let mut in_scx = false;
    for line in text.lines() {
        let t = line.trim();
        if header(t) {
            in_scx = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_scx && t.starts_with('[') && t.ends_with(']') {
            in_scx = false;
        }
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
    let Json::Obj(kv) = root else {
        unreachable!("obj_mut 已就地替换为非对象为空对象，必为 Obj")
    };
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
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path = resolved.as_path();
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
    fn home_from_prefers_unix_home_then_windows_userprofile() {
        let home = home_from(
            Some("/tmp/scx-home"),
            Some("C:/Users/test-user"),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(home, PathBuf::from("/tmp/scx-home"));

        let home = home_from(
            None,
            Some("C:/Users/test-user"),
            Some("C:"),
            Some("\\Users\\test-user"),
            true,
        )
        .unwrap();
        assert_eq!(home, PathBuf::from("C:/Users/test-user"));
    }

    #[test]
    fn home_from_composes_windows_drive_path_and_rejects_all_missing() {
        let home = home_from(None, None, Some("C:"), Some("\\Users\\test-user"), true).unwrap();
        assert_eq!(home, PathBuf::from("C:\\Users\\test-user"));

        let blank = home_from(
            Some("  "),
            None,
            Some("C:"),
            Some("\\Users\\test-user"),
            false,
        );
        assert!(blank.is_err(), "非 Windows 且前两个值为空时应报错");

        assert!(home_from(None, None, None, None, false).is_err());
    }

    #[test]
    fn header_accepts_trailing_comment_but_not_longer_names() {
        assert!(header("[mcp_servers.scx]"));
        assert!(header("[mcp_servers.scx] # keep this note"));
        assert!(header("[mcp_servers.scx]#note"));
        assert!(!header("[mcp_servers.scx2]"), "更长的段名不得误判");
        assert!(!header("# [mcp_servers.scx]"), "注释行不是段头");
    }

    #[test]
    fn commented_section_header_is_updated_in_place() {
        let dir = env::temp_dir().join(format!("scx-setup-hdr-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        fs::write(
            &cfg,
            "[mcp_servers.scx] # keep this note\ncommand = \"/old/bin\"\nargs = [\"mcp\"]\n",
        )
        .unwrap();
        toml_reg(&cfg, Path::new("/new/bin")).unwrap();
        let text = fs::read_to_string(&cfg).unwrap();
        assert_eq!(
            text.matches("[mcp_servers.scx]").count(),
            1,
            "不得追加重复段: {text}"
        );
        assert!(
            text.contains("command = \"/new/bin\""),
            "应原位更新: {text}"
        );
        assert!(text.contains("# keep this note"), "段头注释保留: {text}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_toml_command_matches_any_whitespace_and_skips_comments() {
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
        let dir = env::temp_dir().join(format!(
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

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("scx-hooks-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hooks_reg_writes_all_events_and_is_idempotent() {
        let dir = tmp_dir("write");
        let path = dir.join("settings.json");
        let exe = Path::new("/tmp/scx-bin");
        hooks_reg(&path, exe, false).unwrap();
        let once = fs::read_to_string(&path).unwrap();
        for event in ["SessionStart", "UserPromptSubmit", "PostToolUse", "Stop"] {
            assert!(once.contains(event), "缺少事件 {event}: {once}");
        }
        assert!(once.contains("scx-bin hook"));
        // 第二次运行内容不变（幂等）。
        hooks_reg(&path, exe, false).unwrap();
        let twice = fs::read_to_string(&path).unwrap();
        assert_eq!(once, twice);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hooks_reg_uses_regex_matcher_for_codex() {
        let dir = tmp_dir("codex");
        let path = dir.join("hooks.json");
        let exe = Path::new("/tmp/scx-bin");
        hooks_reg(&path, exe, true).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("Bash|Read|Grep|Glob"),
            "Codex 应使用正则 matcher: {text}"
        );
        assert!(!text.contains("Bash,Read"), "不应写入逗号枚举: {text}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_hooks_keeps_foreign_groups_and_replaces_scx_groups() {
        let dir = tmp_dir("merge");
        let path = dir.join("settings.json");
        let exe = Path::new("/tmp/scx-bin");
        // 预置：一个外部组（独立 group）+ 一个旧 scx 组。
        fs::write(
            &path,
            "{\n  \"hooks\": {\n    \"PostToolUse\": [\n      {\n        \"matcher\": \"Bash\",\n        \"hooks\": [\n          {\"type\": \"command\", \"command\": \"/tmp/other check\"}\n        ]\n      },\n      {\n        \"matcher\": \"Bash\",\n        \"hooks\": [\n          {\"type\": \"command\", \"command\": \"/tmp/scx-bin hook\"}\n        ]\n      }\n    ]\n  }\n}\n",
        )
        .unwrap();
        hooks_reg(&path, exe, false).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("/tmp/other check"), "外部组必须保留: {text}");
        // 标准树每个事件一条命令（共 4 条）；PostToolUse 内不得叠加旧组。
        assert_eq!(
            text.matches("scx-bin hook").count(),
            4,
            "旧 scx 组应被替换而非叠加: {text}"
        );
        assert!(
            text.contains("Bash,Read,Grep,Glob"),
            "PostToolUse 应替换为标准 matcher: {text}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cursor_hooks_reg_writes_native_schema_and_is_idempotent() {
        let dir = tmp_dir("cursor");
        let path = dir.join("hooks.json");
        let exe = Path::new("/tmp/scx-bin");
        cursor_hooks_reg(&path, exe).unwrap();
        let once = fs::read_to_string(&path).unwrap();
        assert!(once.contains("\"version\": 1"), "官方 schema 版本: {once}");
        for event in ["sessionStart", "beforeSubmitPrompt", "postToolUse", "stop"] {
            assert!(once.contains(event), "缺事件 {event}: {once}");
        }
        assert!(
            once.contains("Shell|Read|Grep|Glob"),
            "正则 matcher: {once}"
        );
        cursor_hooks_reg(&path, exe).unwrap();
        let twice = fs::read_to_string(&path).unwrap();
        assert_eq!(once, twice, "幂等写入无漂移");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cursor_merge_keeps_foreign_defs_and_replaces_scx_defs() {
        let dir = tmp_dir("cursor-merge");
        let path = dir.join("hooks.json");
        let exe = Path::new("/tmp/scx-bin");
        fs::write(
            &path,
            "{\n  \"version\": 1,\n  \"hooks\": {\n    \"postToolUse\": [\n      {\"command\": \"/tmp/other check\"},\n      {\"command\": \"/tmp/scx-bin hook\"}\n    ]\n  }\n}\n",
        )
        .unwrap();
        cursor_hooks_reg(&path, exe).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("/tmp/other check"),
            "外部定义必须保留: {text}"
        );
        assert_eq!(
            text.matches("scx-bin hook").count(),
            4,
            "旧 scx 定义应替换而非叠加: {text}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
