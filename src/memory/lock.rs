//! 跨进程写锁与原子写（零依赖）。
//!
//! 记忆库允许多个 `scx` 进程共享同一 `--mem-dir`（例如并行 agent / 终端）。
//! 原先的 `fs::write(tmp) + rename` 使用固定临时名，且 `meta.toml` 的
//! read-modify-write 没有跨进程互斥：并发写会互相 rename、丢 gen 更新。
//!
//! 本模块提供两件互补的机制：
//! - `atomic`：每次写入使用唯一临时名 + rename，保证读者只看到旧文件
//!   或完整新文件（防半写）；Windows 上对短暂 sharing violation 做有限重试。
//! - `acquire`：`create_new` 锁文件 + 超时/陈旧锁回收，保护 meta/qmap/index
//!   这类 read-modify-write 或追加写。锁内容带唯一 token，Drop 只删除自己的锁，
//!   避免“陈旧锁被抢走后误删新持有者”的竞态。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// 等锁上限：meta/qmap 写都是毫秒级，10s 足够覆盖磁盘抖动。
const LOCK_WAIT: Duration = Duration::from_secs(10);
/// 陈旧锁阈值：持锁超过 15s 视为进程崩溃遗留（正常写远小于此）。
const LOCK_STALE: Duration = Duration::from_secs(15);
const RETRY: Duration = Duration::from_millis(10);

static TOKEN_SEQ: AtomicU64 = AtomicU64::new(0);

/// 建目录（Windows 杀毒/索引器短暂占用时重试 PermissionDenied）。
pub(crate) fn mkdir(path: &Path) -> io::Result<()> {
    let mut last: Option<io::Error> = None;
    for attempt in 0..3 {
        match fs::create_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let transient = e.kind() == io::ErrorKind::PermissionDenied;
                last = Some(e);
                if !transient || attempt == 2 {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(last.expect("create_dir_all always records an error"))
}

/// 锁守卫：作用域结束自动释放（只删自己 token 的锁文件）。
pub(crate) struct Lock {
    path: PathBuf,
    token: String,
}

impl Drop for Lock {
    fn drop(&mut self) {
        // 被判定陈旧后可能已被新持有者替换：只删自己的 token。
        if let Ok(txt) = fs::read_to_string(&self.path) {
            if txt.trim() == self.token {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

/// 获取 `<root>/<name>` 锁；超时返回 `WouldBlock`。
pub(crate) fn acquire(root: &str, name: &str) -> io::Result<Lock> {
    let path = Path::new(root).join(name);
    if let Some(dir) = path.parent() {
        mkdir(dir)?;
    }
    let seq = TOKEN_SEQ.fetch_add(1, Ordering::Relaxed);
    let token = format!("{}-{seq}", std::process::id());
    let deadline = Instant::now() + LOCK_WAIT;
    let mut permission_retries = 0u32;
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return finish(&path, f, &token),
            Err(e) if contended(e.kind()) => {
                wait(&path, deadline, &e, &mut permission_retries)?;
            }
            Err(e) => return Err(e),
        }
    }
}

fn contended(kind: io::ErrorKind) -> bool {
    kind == io::ErrorKind::AlreadyExists || kind == io::ErrorKind::PermissionDenied
}

fn finish(path: &Path, mut f: fs::File, token: &str) -> io::Result<Lock> {
    // token 落盘后其它进程才可能看到锁；写失败则移除，避免假锁。
    if let Err(e) = writeln!(f, "{token}") {
        let _ = fs::remove_file(path);
        return Err(e);
    }
    Ok(Lock {
        path: path.to_path_buf(),
        token: token.to_string(),
    })
}

fn wait(
    path: &Path,
    deadline: Instant,
    original: &io::Error,
    permission_retries: &mut u32,
) -> io::Result<()> {
    let kind = original.kind();
    // Windows：锁文件 pending-delete 时 CreateFile(CREATE_NEW) 返回
    // ACCESS_DENIED；给 1s 让删除完成，超过按真实权限错误返回。
    if kind == io::ErrorKind::PermissionDenied {
        *permission_retries += 1;
        if *permission_retries > 100 {
            return Err(io::Error::new(kind, format!("memory lock: {original}")));
        }
    }
    if stale(path) {
        let _ = fs::remove_file(path);
        return Ok(());
    }
    if Instant::now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("memory lock busy: {}", path.display()),
        ));
    }
    thread::sleep(RETRY);
    Ok(())
}

/// 按固定序一次性获取多把锁（锁序契约见 `docs/locking.md`）。
///
/// 任一锁获取失败时，已持有的锁随守卫 `Drop` 逆序自动释放——绝不半持锁返回。
/// 调用方必须按契约顺序传 `names`（细粒度在前、粗粒度在后），禁止自行穿插。
pub(crate) fn locks(root: &str, names: &[&str]) -> io::Result<Vec<Lock>> {
    let mut guards = Vec::with_capacity(names.len());
    for name in names {
        guards.push(acquire(root, name)?);
    }
    Ok(guards)
}

fn stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > LOCK_STALE)
}

/// 唯一临时名 + rename 的原子写。
///
/// 说明：保证“不会读到半写文件”，不做 fsync 级别的掉电持久化；case/meta 都是
/// 可再生/可重建数据，崩溃后最坏回到上一个完整版本。每次写使用
/// `<name>.tmp.<pid>.<seq>`，并发写不会互相覆盖临时文件。
pub(crate) fn atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        mkdir(dir)?;
    }
    let base = path.file_name().and_then(|s| s.to_str()).unwrap_or("data");
    let mut last: Option<io::Error> = None;
    // Windows 杀毒/索引器会短暂锁住目标或临时文件；PermissionDenied 时换新
    // 临时名重试整次写，避免把可恢复的文件占用报成记忆写入失败。
    for attempt in 0..3 {
        let seq = TOKEN_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_file_name(format!("{base}.tmp.{}.{seq}", std::process::id()));
        let result = once(&tmp, path, bytes);
        match result {
            Ok(()) => return Ok(()),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                let transient = retry(e.kind());
                last = Some(io::Error::new(
                    e.kind(),
                    format!("atomic write {}: {e}", path.display()),
                ));
                if !transient || attempt == 2 {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(last.expect("atomic always records an error"))
}

fn once(tmp: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(tmp)?;
    f.write_all(bytes)?;
    drop(f);
    rename(tmp, path)
}

fn retry(kind: io::ErrorKind) -> bool {
    kind == io::ErrorKind::PermissionDenied || kind == io::ErrorKind::WouldBlock
}

/// Windows 上目标文件可能被短暂打开（杀毒/索引/读者），rename 做有限重试。
fn rename(from: &Path, to: &Path) -> io::Result<()> {
    let mut last: Option<io::Error> = None;
    for attempt in 0..10 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                if attempt < 9 {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
    Err(last.expect("rename retry always records an error"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::util::root;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn atomic_keeps_last_complete_value() {
        let root = root("atomic");
        let p = Path::new(&root).join("value.txt");
        fs::create_dir_all(&root).unwrap();
        let mut handles = Vec::new();
        for i in 0..8 {
            let p = p.clone();
            handles.push(thread::spawn(move || {
                atomic(&p, format!("value-{i}").as_bytes()).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let got = fs::read_to_string(&p).unwrap();
        assert!(got.starts_with("value-"), "必须是某次完整写入: {got}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn lock_serializes_and_releases() {
        let root = root("lock");
        fs::create_dir_all(&root).unwrap();
        let root = Arc::new(root);
        let mut handles = Vec::new();
        for _ in 0..4 {
            let root = root.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..10 {
                    let _g = acquire(&root, "test.lock").unwrap();
                    thread::sleep(Duration::from_millis(1));
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!(!Path::new(root.as_str()).join("test.lock").exists());
        let _ = fs::remove_dir_all(root.as_str());
    }

    #[test]
    fn locks_fixed_order_and_failure_releases() {
        let root = root("lock_all");
        fs::create_dir_all(&root).unwrap();
        // 固定序获取成功，Drop 逆序释放后锁文件全部消失。
        {
            let _g = locks(&root, &["a.lock", "b.lock", "c.lock"]).unwrap();
            assert!(Path::new(&root).join("a.lock").exists());
            assert!(Path::new(&root).join("b.lock").exists());
            assert!(Path::new(&root).join("c.lock").exists());
        }
        assert!(!Path::new(&root).join("a.lock").exists());
        // 中途失败：已持锁逆序释放，不留半持状态。
        let _held = acquire(&root, "b.lock").unwrap();
        let err = match locks(&root, &["a.lock", "b.lock"]) {
            Ok(_) => panic!("locks 必须失败"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        assert!(!Path::new(&root).join("a.lock").exists(), "a 必须已释放");
        let _ = fs::remove_dir_all(&root);
    }
}
