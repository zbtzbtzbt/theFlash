//! 合并锁：同一时间只有一个合并在跑。
//!
//! Several agent sessions run the gate at once, so two merges could race
//! and land two unreviewed-against-each-other commits on main (#15 asks
//! for a queue). The lock is a directory: creating one either succeeds for
//! exactly one process or fails for the others, on macOS and on Linux, with
//! no daemon and no extra dependency. A holder that died leaves the
//! directory behind, so a lock older than [`STALE_AFTER`] is taken over.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long a lock can be held before another session takes it over. Long
/// enough for a merge that waits on CI, short enough that a crashed
/// session does not stop merging for the night.
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);

/// Where the lock lives. `THEFLASH_MERGE_GATE_STATE` points tests at a
/// temporary directory, so no test touches the real one.
pub fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("THEFLASH_MERGE_GATE_STATE") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".theflash")
}

/// Held while one merge runs; released when dropped, including on the
/// error paths, so a refusal does not leave the queue stuck.
pub struct Lock {
    path: PathBuf,
}

#[derive(Debug)]
pub struct Busy {
    pub holder: String,
}

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "另一个合并正在跑：{}", self.holder)
    }
}

impl Lock {
    /// Takes the lock, or says who holds it. `what` describes this holder
    /// and ends up in the message the next session sees.
    pub fn acquire(what: &str) -> Result<Lock, Busy> {
        let dir = state_dir();
        let path = dir.join("merge.lock");
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = std::fs::create_dir(&path) {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(Busy {
                    holder: format!("建锁失败 {}：{e}", path.display()),
                });
            }
            let holder = holder(&path);
            if age(&path).is_none_or(|a| a < STALE_AFTER) {
                return Err(Busy { holder });
            }
            // The holder is long gone; its lock would stop every merge
            // tonight, so take it over and say so.
            eprintln!(
                "接管超过 {} 分钟的旧锁（原持有者：{holder}）",
                STALE_AFTER.as_secs() / 60
            );
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir(&path).map_err(|e| Busy {
                holder: format!("接管旧锁失败：{e}"),
            })?;
        }
        let _ = std::fs::write(
            path.join("holder"),
            format!("{what} pid {}\n", std::process::id()),
        );
        Ok(Lock { path })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn holder(path: &Path) -> String {
    std::fs::read_to_string(path.join("holder"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn age(path: &Path) -> Option<Duration> {
    let created = std::fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(created).ok()
}
