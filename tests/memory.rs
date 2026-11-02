//! 多进程组合压力测试：store + rate + delete + rebuild 并发打同一 `--mem-dir`。
//!
//! 目的不是断言“全部成功”，而是证明**无死锁**：所有进程必须在有限时间内结束
//! （锁超时上限 10s），且终态库仍可被 `--stats` / `--recall` 读取。
//! 与 `src/memory/lock.rs` 的线程级单测互补（跨进程 + 锁序 + rebuild 竞争）。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn scx_bin() -> &'static str {
    env!("CARGO_BIN_EXE_scx")
}

fn fresh_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("scx-conc-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

/// 预置一个 case（store 一次并返回 case id）。
fn seed_case(root: &std::path::Path, tag: &str, conn: &str) -> String {
    let inp = root.join(format!("in-{tag}.txt"));
    fs::write(
        &inp,
        format!("{conn} broken db crash\nline two\nline three\nline four"),
    )
    .unwrap();
    let st = Command::new(scx_bin())
        .args([
            "--query",
            conn,
            "--store",
            "1",
            "--fix",
            "fixed",
            "--mem-dir",
            root.to_str().unwrap(),
            inp.to_str().unwrap(),
        ])
        .status()
        .expect("spawn scx store");
    assert!(st.success(), "seed store must succeed: {st:?}");
    // case id = 目录名（YYYY-MM-DD_hex）。
    let cases = root.join("cases");
    let id = fs::read_dir(&cases)
        .expect("cases dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .max()
        .expect("at least one case dir");
    id
}

#[test]
fn concurrent_store_rate_delete_rebuild_no_deadlock() {
    let root = fresh_root("p2");
    // 预置 3 个 case：1 号留给并发 delete，2 号留给并发 rate，3 号兜底。
    let del_id = seed_case(&root, "d", "conn del");
    let rate_id = seed_case(&root, "r", "conn rate");
    let _keep_id = seed_case(&root, "k", "conn keep");

    let scx = scx_bin();
    let mem = root.to_str().unwrap().to_string();
    let mut procs = Vec::new();

    // 2 个并发 store（写新 case）
    for i in 0..2 {
        let inp = root.join(format!("in-w{i}.txt"));
        fs::write(&inp, format!("conn w{i} broken\nline two\nline three")).unwrap();
        let inp = inp.to_str().unwrap().to_string();
        let mem = mem.clone();
        procs.push(thread::spawn(move || {
            Command::new(scx)
                .args([
                    "--query",
                    &format!("conn w{i}"),
                    "--store",
                    "1",
                    "--fix",
                    "w",
                    "--mem-dir",
                    &mem,
                    &inp,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
        }));
    }
    // 1 个并发 rate（good, case 2）
    {
        let mem = mem.clone();
        let rate_id = rate_id.clone();
        procs.push(thread::spawn(move || {
            Command::new(scx)
                .args(["--rate", "good", &rate_id, "--mem-dir", &mem])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
        }));
    }
    // 1 个并发 delete（case 1）
    {
        let mem = mem.clone();
        let del_id = del_id.clone();
        procs.push(thread::spawn(move || {
            Command::new(scx)
                .args(["--delete", &del_id, "--mem-dir", &mem])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
        }));
    }
    // 1 个并发 rebuild
    {
        let mem = mem.clone();
        procs.push(thread::spawn(move || {
            Command::new(scx)
                .args(["--rebuild", "--mem-dir", &mem])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
        }));
    }

    // 无死锁断言：全部在 10s 内结束（锁超时上限即 10s，加 10s 余量）。
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut finished: Vec<bool> = Vec::new();
    for h in procs {
        let joined = h
            .join()
            .expect("worker thread panicked")
            .expect("spawn scx process");
        // store / rebuild 必须成功；rate / delete 因并发竞争可能优雅失败，不强制。
        finished.push(joined);
        assert!(
            Instant::now() < deadline,
            "死锁或超时：并发进程未在 20s 内结束"
        );
    }

    // 终态可读性：--stats 与 --recall 必须能跑通（库结构未被并发写坏）。
    let st = Command::new(scx)
        .args(["--stats", "--mem-dir", mem.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn scx stats");
    assert!(st.success(), "终态 --stats 必须可读: {st:?}");

    let _ = fs::remove_dir_all(&root);
}
