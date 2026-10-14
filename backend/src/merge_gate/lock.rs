//! 合并锁：同一时间只有一个合并在跑。
//!
//! Several agent sessions run the gate at once, so two merges could race
//! and land two unreviewed-against-each-other commits on main (#15 asks
//! for a queue). The lock is a directory: creating one either succeeds for
//! exactly one process or fails for the others, on macOS and on Linux, with
//! no daemon.
//!
//! What the first version of this got wrong, and what each piece here is
//! for (审查记录 b1b42f0 的第 3 条):
//!
//! - **The kernel owns mutual exclusion.** Every session opens the same
//!   stable file and takes `flock(LOCK_EX | LOCK_NB)`. The kernel releases
//!   it when the process exits, so there is no stale-lock takeover
//!   algorithm and no read/claim ABA window.
//! - **Holder metadata is diagnostic only and atomically replaced.** A
//!   reader sees either the old complete holder or the new complete
//!   holder, never the truncate/write gap of `fs::write`.
//! - **The lock path is never removed or replaced.** Advisory locks attach
//!   to an inode; replacing that inode would let a second session lock the
//!   new file while the first still holds the old one.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a holder promises to be done in. The lease is written inside
/// the lock, so it is the holder's own clock reading and not the
/// filesystem's. A lease that ran out does not release the lock by itself:
/// it only means the next session is allowed to check whether the holder
/// is still there.
pub const LEASE: Duration = Duration::from_secs(30 * 60);

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
#[derive(Debug)]
pub struct Lock {
    path: PathBuf,
    /// Kept open for the lifetime of the lock. Closing it releases flock.
    _guard: File,
    /// This holder's token, which is what makes releasing safe.
    token: String,
    what: String,
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
        Lock::acquire_in(&state_dir(), what)
    }

    /// The same, with the state directory given, so a test locks inside
    /// its own temporary directory instead of the real one.
    pub fn acquire_in(dir: &Path, what: &str) -> Result<Lock, Busy> {
        let path = dir.join("merge.lock");
        std::fs::create_dir_all(&path).map_err(|e| Busy {
            holder: format!("建锁目录失败 {}：{e}", path.display()),
        })?;
        #[cfg(test)]
        race_tests::pause_before_guard_acquire(what);
        let guard_path = path.join("kernel-lock");
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&guard_path)
            .map_err(|e| Busy {
                holder: format!("打开合并锁失败 {}：{e}", guard_path.display()),
            })?;
        if let Err(e) = try_lock(&guard) {
            let held = Holder::read(&path);
            return Err(Busy {
                holder: if is_lock_busy(&e) {
                    held.describe()
                } else {
                    format!("锁定 {} 失败：{e}", guard_path.display())
                },
            });
        }
        let previous = Holder::read(&path);
        let lock = Lock {
            path: path.clone(),
            _guard: guard,
            token: token(),
            what: what.to_string(),
        };
        lock.renew().map_err(|e| Busy {
            holder: format!("写合并锁持有者失败 {}：{e}", path.display()),
        })?;
        if previous.pid.is_some_and(|pid| !is_running(pid)) {
            eprintln!(
                "接管已经退出的会话留下的锁（原持有者：{}）",
                previous.describe()
            );
        }
        Ok(lock)
    }

    /// Pushes the lease out, so a merge that takes a while is not mistaken
    /// for one that stopped. Called between the steps of a merge and
    /// between the pull requests of a queue run.
    pub fn renew(&self) -> std::io::Result<()> {
        let until = SystemTime::now() + LEASE;
        write_holder(
            self.path.join("holder"),
            &self.token,
            format!(
                "who: {}\npid: {}\ntoken: {}\nlease-until: {}\n",
                self.what,
                std::process::id(),
                self.token,
                seconds(until)
            ),
        )
    }
}

fn write_holder(path: PathBuf, token: &str, contents: String) -> std::io::Result<()> {
    let temporary = path.with_file_name(format!("holder.{token}.new"));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        #[cfg(test)]
        race_tests::pause_during_holder_write(&contents);
        use std::io::Write;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Only this holder's own lock. A session whose lock was taken
        // over while it was gone must not delete the lock of the session
        // that is merging now.
        if Holder::read(&self.path).token.as_deref() != Some(self.token.as_str()) {
            return;
        }
        let _ = std::fs::remove_file(self.path.join("holder"));
        // `guard` is dropped after this method and releases the kernel lock.
    }
}

/// Who holds a lock, as the lock itself says.
#[derive(Default)]
struct Holder {
    who: String,
    pid: Option<u32>,
    token: Option<String>,
    lease_until: Option<SystemTime>,
}

impl Holder {
    fn read(path: &Path) -> Holder {
        let text = std::fs::read_to_string(path.join("holder")).unwrap_or_default();
        let mut h = Holder::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim().to_string();
            match key.trim() {
                "who" => h.who = value,
                "pid" => h.pid = value.parse().ok(),
                "token" => h.token = Some(value),
                "lease-until" => {
                    h.lease_until = value
                        .parse()
                        .ok()
                        .map(|s| UNIX_EPOCH + Duration::from_secs(s));
                }
                _ => {}
            }
        }
        if h.who.is_empty() {
            h.who = text.trim().to_string();
        }
        h
    }

    fn describe(&self) -> String {
        let mut said = if self.who.is_empty() {
            "另一个会话".to_string()
        } else {
            self.who.clone()
        };
        if let Some(pid) = self.pid {
            said.push_str(&format!(" pid {pid}"));
            if is_running(pid) {
                said.push_str("，进程还活着");
            }
        }
        if self
            .lease_until
            .is_some_and(|until| SystemTime::now() > until)
        {
            said.push_str(&format!(
                "。它说好 {} 分钟内做完，已经超时：去看看那个会话是不是卡住了，\
                 确认它结束了再删掉锁目录",
                LEASE.as_secs() / 60
            ));
        }
        said
    }
}

/// Does process `pid` exist? `kill(pid, 0)` sends no signal and only
/// answers that question; `EPERM` means it exists and belongs to someone
/// else. This is the one thing in the gate that needs libc: the
/// alternatives are a lock that cannot tell a crashed session from a slow
/// one, or shelling out to `kill` with `PATH` emptied under test.
fn is_running(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        // 0 and negative pids mean groups of processes to `kill`, never
        // one process, so they are not a holder we could ask about.
        return false;
    }
    // SAFETY: `kill` with signal 0 reads process state and changes
    // nothing; `pid` is positive, so it names one process.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn try_lock(file: &File) -> std::io::Result<()> {
    // SAFETY: flock only changes the advisory lock associated with this
    // open file description; the fd remains owned by `file`.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn is_lock_busy(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
    )
}

/// A string no other holder has: two sessions can share a wall-clock
/// reading or a pid, but not both at once.
fn token() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    )
}

fn seconds(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// A lock left behind by another session, for tests: the holder file as
/// that session would have written it, without running that session.
#[cfg(test)]
pub(crate) fn plant(dir: &Path, what: &str, pid: u32) {
    let path = dir.join("merge.lock");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(
        path.join("holder"),
        format!(
            "who: {what}\npid: {pid}\ntoken: planted-{pid}\nlease-until: {}\n",
            seconds(SystemTime::now() + LEASE)
        ),
    )
    .unwrap();
}

#[cfg(test)]
mod race_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, OnceLock};

    static TAKEOVER_BARRIERS: OnceLock<(Arc<Barrier>, Arc<Barrier>)> = OnceLock::new();
    static RENEW_BARRIERS: OnceLock<(Arc<Barrier>, Arc<Barrier>)> = OnceLock::new();
    static PAUSE_RENEW: AtomicBool = AtomicBool::new(false);

    pub(super) fn pause_before_guard_acquire(what: &str) {
        if what == "paused-old-contender" {
            let (read, resume) = TAKEOVER_BARRIERS.get().expect("test barriers");
            read.wait();
            resume.wait();
        }
    }

    pub(super) fn pause_during_holder_write(contents: &str) {
        if contents.starts_with("who: renewing-live-holder\n") && PAUSE_RENEW.load(Ordering::SeqCst)
        {
            let (read, resume) = RENEW_BARRIERS.get().expect("test barriers");
            read.wait();
            resume.wait();
        }
    }

    fn state(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/merge-gate-race-tests")
            .join(format!("{name}-{}", token()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stale_contender_cannot_take_over_a_recreated_live_lock() {
        let dir = state("stale-contender");
        plant(&dir, "dead-holder", 0x4000_0000);
        let read = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        TAKEOVER_BARRIERS
            .set((read.clone(), resume.clone()))
            .unwrap();

        let state = dir.clone();
        let stale = std::thread::spawn(move || Lock::acquire_in(&state, "paused-old-contender"));
        read.wait();
        let first = Lock::acquire_in(&dir, "first-takeover").unwrap();
        drop(first);
        let live = Lock::acquire_in(&dir, "new-live-holder").unwrap();
        resume.wait();

        let result = stale.join().unwrap();
        assert!(
            result.is_err(),
            "延迟的旧接管者覆盖了重建后的活锁，两个会话同时持锁"
        );
        drop(live);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn renewing_a_live_old_lock_cannot_enable_takeover() {
        let dir = state("renew");
        let live = Lock::acquire_in(&dir, "renewing-live-holder").unwrap();
        let old = std::fs::FileTimes::new()
            .set_modified(SystemTime::now() - LEASE - Duration::from_secs(60));
        std::fs::File::open(dir.join("merge.lock"))
            .unwrap()
            .set_times(old)
            .unwrap();
        let read = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        RENEW_BARRIERS.set((read.clone(), resume.clone())).unwrap();
        PAUSE_RENEW.store(true, Ordering::SeqCst);

        let renewer = std::thread::spawn(move || {
            live.renew().unwrap();
            live
        });
        read.wait();
        let contender = Lock::acquire_in(&dir, "contender-during-renew");
        resume.wait();
        let live = renewer.join().unwrap();

        assert!(
            contender.is_err(),
            "续租截空 holder 时，活锁被误接管，两个会话同时持锁"
        );
        drop(live);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn renew_reports_a_metadata_write_failure() {
        let dir = state("renew-error");
        let live = Lock::acquire_in(&dir, "holder").unwrap();
        std::fs::create_dir(live.path.join(format!("holder.{}.new", live.token))).unwrap();

        let error = live.renew().expect_err("刷新失败必须返回给调用方");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);

        drop(live);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
