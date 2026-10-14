//! 调度程序记得什么：派过哪些会话、今天派了几个、定时任务跑过没有。
//!
//! The state is one JSON file, written by replacing it, so a dispatcher
//! killed mid-write leaves the previous state rather than half of a new
//! one. It is the only reason the dispatcher does not start the same work
//! twice, so every field in it is something a decision reads.

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::beijing;

/// What happened to a session, once it is no longer running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// The agent command exited with 0.
    Done,
    /// It exited with something else.
    Failed,
    /// It was still running after `session_timeout_minutes` and was stopped.
    TimedOut,
    /// The process is gone and the dispatcher never saw its exit code
    /// (the dispatcher itself was restarted while it ran).
    Vanished,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Done => "完成",
            Outcome::Failed => "失败",
            Outcome::TimedOut => "超时",
            Outcome::Vanished => "不知所终",
        }
    }
}

/// One agent session: a whole process, started once, never reused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// `accept-16-r1`, `review-33-91f0f06`: the same event always produces
    /// the same id, which is how the dispatcher avoids starting it twice.
    pub id: String,
    pub role: String,
    pub vendor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<u64>,
    #[serde(with = "rfc3339")]
    pub started_at: DateTime<Utc>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "rfc3339_option"
    )]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    /// The process group the session runs in, so stopping it stops the
    /// tools it started too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub log: PathBuf,
    #[serde(default)]
    pub worktree: PathBuf,
}

impl Session {
    pub fn is_running(&self) -> bool {
        self.finished_at.is_none()
    }

    pub fn minutes_since_start(&self, now: DateTime<Utc>) -> i64 {
        (now - self.started_at).num_minutes()
    }

    /// Whether the session's process is still there. A session whose
    /// process is gone is finished even if the dispatcher never saw it
    /// exit, which is what a restarted dispatcher sees.
    pub fn process_alive(&self) -> bool {
        let Some(pid) = self.pid else {
            return false;
        };
        Command::new(kill_bin())
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Every session this dispatcher started, newest last.
    pub sessions: Vec<Session>,
    /// Task ids the dispatcher has already explained on the issue, so a
    /// queued task does not add a comment every minute.
    pub explained: Vec<String>,
    /// `定时任务名 -> 最后跑的那个本地日期`, so a daily job runs once a day
    /// even if the dispatcher restarts.
    pub scheduled_ran: std::collections::BTreeMap<String, String>,
}

impl State {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("state.json")
    }

    /// The remembered state, or an empty one when nothing has run yet. A
    /// file that cannot be parsed is an error: continuing with an empty
    /// state would re-dispatch everything that is already running.
    pub fn read(dir: &Path) -> Result<State, String> {
        let path = State::path(dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("{} 不是预期的格式：{e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(format!("读 {} 失败：{e}", path.display())),
        }
    }

    pub fn write(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("建 {} 失败：{e}", dir.display()))?;
        let path = State::path(dir);
        let temp = path.with_extension("json.new");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&temp, text).map_err(|e| format!("写 {} 失败：{e}", temp.display()))?;
        std::fs::rename(&temp, &path).map_err(|e| format!("写 {} 失败：{e}", path.display()))
    }

    pub fn running(&self) -> impl Iterator<Item = &Session> {
        self.sessions.iter().filter(|s| s.is_running())
    }

    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn session_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    /// How many sessions started on `now`'s local day. The day is local
    /// because the ceiling is about the owner's day and money, not UTC.
    pub fn started_today(&self, now: DateTime<Utc>) -> usize {
        let today = local_day(now);
        self.sessions
            .iter()
            .filter(|s| local_day(s.started_at) == today)
            .count()
    }
}

/// `kill` by absolute path where there is one: the dispatcher asks it
/// whether a session's process is still there, and a PATH that does not
/// have it would make every session look dead.
pub fn kill_bin() -> &'static str {
    if std::path::Path::new("/bin/kill").exists() {
        "/bin/kill"
    } else {
        "kill"
    }
}

/// `YYYY-MM-DD` in Beijing time, which is the owner's day and damai.cn's
/// day. The project keeps every clock in Beijing time rather than the
/// machine's zone (`report::beijing_time`), so a ceiling called "today"
/// means the same thing in a log, on an issue and in a test.
pub fn local_day(at: DateTime<Utc>) -> String {
    at.with_timezone(&beijing()).format("%Y-%m-%d").to_string()
}

/// Times in the state file are RFC 3339 strings: they are read by agents
/// and by `jq`, and chrono's own serde support is not among the features
/// this project compiles chrono with.
mod rfc3339 {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(at: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&at.to_rfc3339())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        let text = String::deserialize(d)?;
        DateTime::parse_from_rfc3339(&text)
            .map(|t| t.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}

mod rfc3339_option {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(at: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
        match at {
            Some(at) => s.serialize_str(&at.to_rfc3339()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
        let Some(text) = Option::<String>::deserialize(d)? else {
            return Ok(None);
        };
        DateTime::parse_from_rfc3339(&text)
            .map(|t| Some(t.with_timezone(&Utc)))
            .map_err(serde::de::Error::custom)
    }
}

/// Only one dispatcher may act at a time: `run` in launchd and a `tick`
/// typed by hand would otherwise start the same work twice. The lock is a
/// file holding a pid; a file whose process is gone is taken over, so a
/// crash does not need a person to clean up.
pub struct Lock {
    path: PathBuf,
}

impl Lock {
    pub fn take(dir: &Path) -> Result<Lock, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("建 {} 失败：{e}", dir.display()))?;
        let path = dir.join("dispatch.pid");
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(pid) = text.trim().parse::<u32>()
            && pid != std::process::id()
            && alive(pid)
        {
            return Err(format!(
                "另一个调度程序正在跑（pid {pid}，锁在 {}）。先 scripts/dispatch status 看它在做什么。",
                path.display()
            ));
        }
        std::fs::write(&path, std::process::id().to_string())
            .map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
        Ok(Lock { path })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn alive(pid: u32) -> bool {
    Command::new(kill_bin())
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
