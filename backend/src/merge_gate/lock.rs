//! 合并锁：同一时间只有一个合并在跑。
//!
//! Several agent sessions run the gate at once, so two merges could race
//! and land two unreviewed-against-each-other commits on main (#15 asks
//! for a queue). The lock is a directory: creating one either succeeds for
//! exactly one process or fails for the others, on macOS and on Linux, with
//! no daemon and no extra dependency.
//!
//! What the first version of this got wrong, and what each piece here is
//! for (审查记录 b1b42f0 的第 3 条):
//!
//! - **A lock is only taken over when its holder is gone.** The holder
//!   writes its pid into the lock, and a session that finds the lock asks
//!   the operating system whether that process still exists. A merge that
//!   has been running for an hour is a session to go look at, not a lock
//!   to take: taking it over is exactly the two-merges-at-once the lock
//!   exists to prevent. Deciding by the lock's age alone also trusted the
//!   filesystem's clock, which `touch` can set to anything.
//! - **Taking over is atomic.** The stale lock is moved aside with a
//!   single `rename`, which succeeds for one session and fails with
//!   "not found" for every other session racing it; whoever moved it then
//!   creates the lock again, which is itself atomic.
//! - **A session only releases its own lock.** Each holder writes a token
//!   no other holder has, and releasing checks the token is still there.
//!   A session whose lock was taken over while it was away must not
//!   delete the lock of the session that took it over.

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
        let _ = std::fs::create_dir_all(dir);
        let lock = Lock {
            path: path.clone(),
            token: token(),
            what: what.to_string(),
        };
        match std::fs::create_dir(&path) {
            Ok(()) => {
                lock.renew();
                return Ok(lock);
            }
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(Busy {
                    holder: format!("建锁失败 {}：{e}", path.display()),
                });
            }
            Err(_) => {}
        }
        let held = Holder::read(&path);
        if !held.is_gone() {
            return Err(Busy {
                holder: held.describe(),
            });
        }
        // The holder is gone; its lock would stop every merge tonight, so
        // take it over. The lock directory stays the same directory —
        // nobody removes it and makes it again, because a session that
        // read the old holder a moment ago would then delete the lock of
        // whoever took over first. Instead each taking-over session tries
        // to create one file named after the holder it is replacing, and
        // creating a file that must not exist yet succeeds for exactly
        // one of them, on macOS and on Linux.
        let claim = path.join(format!("claim-{}", held.id()));
        if let Err(e) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&claim)
        {
            return Err(Busy {
                holder: format!(
                    "{}（另一个会话正在接管它：{e}）",
                    Holder::read(&path).describe()
                ),
            });
        }
        eprintln!(
            "接管已经退出的会话留下的锁（原持有者：{}）",
            held.describe()
        );
        lock.renew();
        Ok(lock)
    }

    /// Pushes the lease out, so a merge that takes a while is not mistaken
    /// for one that stopped. Called between the steps of a merge and
    /// between the pull requests of a queue run.
    pub fn renew(&self) {
        let until = SystemTime::now() + LEASE;
        let _ = std::fs::write(
            self.path.join("holder"),
            format!(
                "who: {}\npid: {}\ntoken: {}\nlease-until: {}\n",
                self.what,
                std::process::id(),
                self.token,
                seconds(until)
            ),
        );
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Only this holder's own lock. A session whose lock was taken
        // over while it was gone must not delete the lock of the session
        // that is merging now.
        if Holder::read(&self.path).token.as_deref() != Some(self.token.as_str()) {
            return;
        }
        let aside = self
            .path
            .with_file_name(format!("merge.lock.released.{}", self.token));
        if std::fs::rename(&self.path, &aside).is_ok() {
            let _ = std::fs::remove_dir_all(&aside);
        }
    }
}

/// Who holds a lock, as the lock itself says.
#[derive(Default)]
struct Holder {
    who: String,
    pid: Option<u32>,
    token: Option<String>,
    lease_until: Option<SystemTime>,
    /// The lock's own age, used only when the holder file is unreadable:
    /// a session killed between creating the lock and writing the file
    /// leaves a lock nothing else can account for.
    age: Option<Duration>,
}

impl Holder {
    fn read(path: &Path) -> Holder {
        let text = std::fs::read_to_string(path.join("holder")).unwrap_or_default();
        let mut h = Holder {
            age: age(path),
            ..Holder::default()
        };
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

    /// Is the session that took this lock gone? Only then may the lock be
    /// taken over. A holder that is still running keeps its lock however
    /// long it has had it; a lock whose holder cannot be identified at
    /// all (no pid, because it was killed in the moment between creating
    /// the lock and writing into it) is taken over once it is older than
    /// the lease.
    fn is_gone(&self) -> bool {
        match self.pid {
            Some(pid) => !is_running(pid),
            None => self.age.is_none_or(|a| a > LEASE),
        }
    }

    /// Which holder this is, for the name of the file that claims its
    /// lock: the token when it wrote one, and otherwise a fixed name, so
    /// two sessions replacing the same unidentifiable holder still race
    /// for one file.
    fn id(&self) -> String {
        self.token.clone().unwrap_or_else(|| "unknown".to_string())
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

fn age(path: &Path) -> Option<Duration> {
    let created = std::fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(created).ok()
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
