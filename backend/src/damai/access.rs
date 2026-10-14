//! The one way out to damai.cn. Every request waits for this process's next
//! slot here, and a risk-control page recorded here stops every theFlash
//! process on this machine until the user clears it. Why it works this way:
//! docs/decisions/0005-damai-access-gate.md. What the state file holds and
//! how the breaker is cleared: docs/subsystems/damai-access.md.

use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use super::Error;

/// At most one request this often, counting every caller in this process.
/// A calendar event has a session a day (18 on 1084537866279), and
/// damai.cn's search answered its first anonymous request with a slider
/// challenge, so even a single query paces itself.
pub const MIN_REQUEST_GAP: Duration = Duration::from_millis(300);

/// The breaker, and the lock taken around reading and changing it.
const STOP_FILE: &str = "risk-stop.json";
const LOCK_FILE: &str = "risk-stop.lock";

/// What damai.cn serves in place of the page asked for when it wants a
/// slider solved. `x5secdata` also appears in the URL it redirects to.
const RISK_MARKERS: [&str; 2] = ["_____tmd_____", "x5secdata"];

/// The marker in a response body or redirect URL that makes it damai.cn's
/// slider challenge rather than the page asked for.
pub fn risk_control_marker(text: &str) -> Option<&'static str> {
    RISK_MARKERS.into_iter().find(|m| text.contains(m))
}

/// The risk control one theFlash process ran into. Written in full so the
/// user, and the owner deciding what to do next, can see what happened
/// without the process that saw it still being around.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskStop {
    /// Beijing time, as everything else theFlash prints.
    pub at: String,
    /// The request that got the slider challenge.
    pub url: String,
    /// Which marker gave it away.
    pub marker: String,
    /// The command that ran into it, without its arguments.
    pub by: String,
    pub pid: u32,
}

impl std::fmt::Display for RiskStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}（pid {}）的请求 {} 收到滑块验证页，命中标记 {}",
            self.at, self.by, self.pid, self.url, self.marker
        )
    }
}

/// What `Gate::resume` cleared.
#[derive(Debug, PartialEq, Eq)]
pub enum Resumed {
    /// There was no breaker.
    Nothing,
    Cleared(Box<RiskStop>),
    /// The breaker was there but unreadable, so what it said is lost. Its
    /// path, for telling the user which file went away.
    ClearedUnreadable(String),
}

/// What the gate needs from time. Tests inject a virtual clock so pacing is
/// checked without waiting on the wall clock.
pub trait Clock: Send + Sync {
    /// ms on a monotonic clock.
    fn now_ms(&self) -> i64;
    /// Waits `ms` on that clock; `ms` of 0 or less returns at once.
    fn sleep_ms(&self, ms: i64) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// This machine's monotonic clock.
pub struct SystemClock(Instant);

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock(Instant::now())
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        self.0.elapsed().as_millis() as i64
    }

    fn sleep_ms(&self, ms: i64) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            if ms > 0 {
                tokio::time::sleep(Duration::from_millis(ms as u64)).await;
            }
        })
    }
}

/// The rate limit and the cross-process breaker in front of damai.cn.
pub struct Gate {
    dir: PathBuf,
    gap_ms: i64,
    clock: Arc<dyn Clock>,
    /// The earliest monotonic ms the next request may leave. Every caller
    /// in this process takes its slot from here.
    next_slot_ms: Mutex<i64>,
    by: String,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("dir", &self.dir)
            .field("gap_ms", &self.gap_ms)
            .field("by", &self.by)
            .finish_non_exhaustive()
    }
}

impl Gate {
    /// The gate every command uses: this user's state directory, this
    /// machine's clock, and the rate limit this run asks for.
    ///
    /// One per process, however many callers ask for it: a second gate
    /// would come with a request budget of its own, and two default
    /// `Client`s would then leave at twice the rate the user asked for
    /// (#74). Built on the first call, so a process that never requests
    /// damai.cn still never touches the state directory.
    pub fn for_user() -> Result<Arc<Gate>, Error> {
        static SHARED: Mutex<Option<Arc<Gate>>> = Mutex::new(None);
        let mut shared = SHARED.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(gate) = shared.as_ref() {
            return Ok(gate.clone());
        }
        let gate = Arc::new(Gate::new(
            state_dir()?,
            request_gap()?,
            Arc::new(SystemClock::default()),
            running_command(),
        )?);
        *shared = Some(gate.clone());
        Ok(gate)
    }

    /// Fails now rather than at the first request: a state directory this
    /// user cannot write is a reason to stop, never a reason to assume no
    /// risk control happened.
    pub fn new(
        dir: PathBuf,
        gap: Duration,
        clock: Arc<dyn Clock>,
        by: String,
    ) -> Result<Gate, Error> {
        fs::create_dir_all(&dir).map_err(|e| state_error(&dir, "建不了状态目录", &e))?;
        lock_file(&dir)?;
        // Opening the lock file is not enough: a lock file left there by an
        // earlier run opens for writing even when the directory itself has
        // become read-only, and the breaker is written as a *new* file next
        // to it (#74). So write one now, before any request goes out,
        // rather than finding out when risk control arrives and the breaker
        // cannot be recorded.
        write_probe(&dir)?;
        Ok(Gate {
            dir,
            gap_ms: gap.as_millis() as i64,
            clock,
            next_slot_ms: Mutex::new(0),
            by,
        })
    }

    /// Waits for this process's next slot, then refuses to let the request
    /// out if any theFlash process has run into risk control.
    ///
    /// The slot is taken when the caller actually gets it, not when it
    /// starts waiting: callers that queued for 300 ms and 600 ms but were
    /// woken a second late (a loaded machine, a busy executor) would
    /// otherwise both leave at once, which is the burst the gap exists to
    /// prevent (#74). So the wait is recomputed after every sleep and the
    /// gap is counted from the request that really went out.
    pub async fn enter(&self) -> Result<(), Error> {
        loop {
            let wait = {
                let mut next = self
                    .next_slot_ms
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let now = self.clock.now_ms();
                if now >= *next {
                    *next = now + self.gap_ms;
                    0
                } else {
                    *next - now
                }
            };
            if wait <= 0 {
                break;
            }
            self.clock.sleep_ms(wait).await;
        }
        // Checked after the wait, so a breaker another process tripped
        // while this request was queued still stops it.
        match self.stop()? {
            Some(stop) => Err(Error::RiskStopped(Box::new(stop))),
            None => Ok(()),
        }
    }

    /// Records risk control so every other process stops too. Returns the
    /// error to report: the caller stops either way, but a breaker that
    /// could not be written must not read like "nothing happened".
    pub fn trip(&self, url: &str, marker: &str) -> Error {
        let stop = RiskStop {
            at: beijing_now(),
            url: url.to_string(),
            marker: marker.to_string(),
            by: self.by.clone(),
            pid: std::process::id(),
        };
        match self.write_stop(&stop) {
            Ok(()) => Error::RiskControl(url.to_string()),
            Err(reason) => Error::RiskNotRecorded {
                url: url.to_string(),
                path: self.stop_path().display().to_string(),
                reason,
            },
        }
    }

    /// The breaker in effect, if any.
    pub fn stop(&self) -> Result<Option<RiskStop>, Error> {
        let lock = lock_file(&self.dir)?;
        lock.lock_shared().map_err(|e| lock_error(&self.dir, &e))?;
        let path = self.stop_path();
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(state_error(&path, "读不了熔断状态文件", &e)),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| state_error(&path, "熔断状态文件的内容不是它该有的样子", &e))
    }

    /// Clears the breaker, saying what it cleared. Reading and removing are
    /// under the lock, so the answer is exactly what went away. An
    /// unreadable breaker is cleared too — it is what stops queries — and
    /// reported as unreadable rather than as nothing.
    pub fn resume(&self) -> Result<Resumed, Error> {
        let lock = lock_file(&self.dir)?;
        lock.lock().map_err(|e| lock_error(&self.dir, &e))?;
        let path = self.stop_path();
        let was = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(stop) => Resumed::Cleared(Box::new(stop)),
                Err(_) => Resumed::ClearedUnreadable(path.display().to_string()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Resumed::Nothing),
            Err(e) => return Err(state_error(&path, "读不了熔断状态文件", &e)),
        };
        fs::remove_file(&path).map_err(|e| state_error(&path, "删不掉熔断状态文件", &e))?;
        Ok(was)
    }

    fn stop_path(&self) -> PathBuf {
        self.dir.join(STOP_FILE)
    }

    /// Written to a temporary file and renamed, so a reader never sees half
    /// a breaker even without taking the lock.
    fn write_stop(&self, stop: &RiskStop) -> Result<(), String> {
        let lock = lock_file(&self.dir).map_err(|e| e.to_string())?;
        lock.lock().map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(stop).map_err(|e| e.to_string())?;
        let temp = self
            .dir
            .join(format!("{STOP_FILE}.{}.tmp", std::process::id()));
        fs::write(&temp, format!("{json}\n")).map_err(|e| e.to_string())?;
        fs::rename(&temp, self.stop_path()).map_err(|e| {
            let _ = fs::remove_file(&temp);
            e.to_string()
        })
    }
}

/// The lock every read and change of the breaker takes. Its own file, so
/// taking it never creates or truncates the breaker itself.
fn lock_file(dir: &Path) -> Result<File, Error> {
    let path = dir.join(LOCK_FILE);
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| state_error(&path, "打不开熔断状态的锁文件", &e))
}

/// Proves this user can still create files in the state directory, which
/// is what recording a breaker needs. Named after the breaker's own
/// temporary file and removed again, so nothing is left behind.
fn write_probe(dir: &Path) -> Result<(), Error> {
    let path = dir.join(format!("{STOP_FILE}.{}.probe", std::process::id()));
    fs::write(&path, b"").map_err(|e| state_error(dir, "状态目录写不进东西", &e))?;
    let _ = fs::remove_file(&path);
    Ok(())
}

fn lock_error(dir: &Path, e: &std::io::Error) -> Error {
    state_error(&dir.join(LOCK_FILE), "锁不住熔断状态", e)
}

fn state_error(path: &Path, what: &str, e: &dyn std::fmt::Display) -> Error {
    Error::RiskState {
        path: path.display().to_string(),
        reason: format!("{what}：{e}"),
    }
}

/// The gap between requests this run uses. Unattended validation sets
/// THEFLASH_REQUEST_GAP_MS to something more conservative than the default
/// (#17, docs/decisions/0009-launchd-validation.md); it can only slow
/// requests down, never let them out faster than MIN_REQUEST_GAP.
fn request_gap() -> Result<Duration, Error> {
    match std::env::var("THEFLASH_REQUEST_GAP_MS") {
        Ok(value) => gap_from(Some(&value)),
        Err(std::env::VarError::NotPresent) => gap_from(None),
        Err(_) => Err(invalid_request_gap()),
    }
}

/// `None` is THEFLASH_REQUEST_GAP_MS unset. A value too large to represent,
/// or accidentally enormous, is refused rather than overflowing the slot
/// arithmetic and silently losing the pacing that was asked for.
fn gap_from(value: Option<&str>) -> Result<Duration, Error> {
    let Some(value) = value else {
        return Ok(MIN_REQUEST_GAP);
    };
    value
        .parse::<u64>()
        .ok()
        .filter(|ms| *ms <= 86_400_000)
        .map(|ms| Duration::from_millis(ms).max(MIN_REQUEST_GAP))
        .ok_or_else(invalid_request_gap)
}

fn invalid_request_gap() -> Error {
    Error::InvalidConfig {
        name: "THEFLASH_REQUEST_GAP_MS",
        expected: "必须是 0 到 86400000 毫秒的整数（含两端）",
    }
}

fn running_command() -> String {
    command_label(std::env::args())
}

/// `theflash watch`, without the arguments: a state file keeps no input of
/// the user's.
fn command_label(mut args: impl Iterator<Item = String>) -> String {
    let bin = args
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "theflash".into());
    match args.next() {
        Some(sub)
            if sub.starts_with(|c: char| c.is_ascii_alphanumeric())
                && sub.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') =>
        {
            format!("{bin} {sub}")
        }
        _ => bin,
    }
}

/// This machine's wall clock in Beijing time, to the second: what the user
/// and the owner read to find the risk control in their own timeline.
fn beijing_now() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    match (
        FixedOffset::east_opt(8 * 3600),
        DateTime::from_timestamp_millis(ms),
    ) {
        (Some(tz), Some(t)) => format!(
            "{}（北京时间）",
            t.with_timezone(&tz).format("%Y-%m-%d %H:%M:%S")
        ),
        _ => format!("{ms} 毫秒（Unix 时间）"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Os {
    MacOs,
    Linux,
}

/// theFlash supports macOS and Linux only, so there is no third branch.
const HOST: Os = if cfg!(target_os = "macos") {
    Os::MacOs
} else {
    Os::Linux
};

/// Where this machine keeps theFlash's state between runs.
pub fn state_dir() -> Result<PathBuf, Error> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty());
    let Some(home) = home else {
        return Err(Error::RiskState {
            path: "$HOME".into(),
            reason: "环境变量 HOME 没有设置，找不到用户状态目录".into(),
        });
    };
    Ok(dir_for(
        HOST,
        Path::new(&home),
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
    ))
}

/// macOS keeps per-user application state in `~/Library/Application
/// Support` (Apple's File System Programming Guide); Linux keeps state
/// that should survive a restart in `$XDG_STATE_HOME`, by default
/// `~/.local/state` (XDG Base Directory Specification). A relative
/// `XDG_STATE_HOME` is ignored, as the spec says.
fn dir_for(os: Os, home: &Path, xdg_state_home: Option<PathBuf>) -> PathBuf {
    match os {
        Os::MacOs => home.join("Library/Application Support/theFlash"),
        Os::Linux => match xdg_state_home.filter(|p| p.is_absolute()) {
            Some(xdg) => xdg.join("theflash"),
            None => home.join(".local/state/theflash"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    /// A clock the test moves by hand: a sleep returns only once the test
    /// has advanced past its deadline.
    struct TestClock(AtomicI64);

    impl TestClock {
        fn new() -> Arc<TestClock> {
            Arc::new(TestClock(AtomicI64::new(0)))
        }

        fn set(&self, ms: i64) {
            self.0.store(ms, Ordering::SeqCst);
        }
    }

    impl Clock for TestClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }

        fn sleep_ms(&self, ms: i64) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            let until = self.now_ms() + ms;
            Box::pin(async move {
                // Yields instead of waiting: the test task advances the
                // clock. The cap turns a clock that never moves into a
                // failure instead of a test that hangs.
                for _ in 0..100_000 {
                    if self.now_ms() >= until {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
                panic!("虚拟时钟没有走到 {until}，sleep 一直没返回");
            })
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("theflash-access-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn gate(dir: &Path, clock: Arc<dyn Clock>) -> Gate {
        Gate::new(
            dir.to_path_buf(),
            MIN_REQUEST_GAP,
            clock,
            "theflash test".into(),
        )
        .unwrap()
    }

    #[test]
    fn the_state_directory_is_the_one_each_platform_uses() {
        let home = Path::new("/home/someone");
        assert_eq!(
            dir_for(Os::Linux, home, None),
            Path::new("/home/someone/.local/state/theflash")
        );
        assert_eq!(
            dir_for(Os::Linux, home, Some("/run/state".into())),
            Path::new("/run/state/theflash")
        );
        assert_eq!(
            dir_for(Os::Linux, home, Some("relative/state".into())),
            Path::new("/home/someone/.local/state/theflash"),
            "XDG_STATE_HOME 不是绝对路径时按规范忽略"
        );
        assert_eq!(
            dir_for(Os::MacOs, Path::new("/Users/someone"), None),
            Path::new("/Users/someone/Library/Application Support/theFlash")
        );
        assert_eq!(
            dir_for(
                Os::MacOs,
                Path::new("/Users/someone"),
                Some("/run/state".into())
            ),
            Path::new("/Users/someone/Library/Application Support/theFlash"),
            "macOS 不用 XDG_STATE_HOME"
        );
    }

    #[tokio::test]
    async fn concurrent_callers_in_one_process_share_one_rate_limit() {
        let dir = temp_dir("rate-limit");
        let clock = TestClock::new();
        let gate = Arc::new(gate(&dir, clock.clone()));
        let sent = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            let (gate, sent) = (gate.clone(), sent.clone());
            tokio::spawn(async move {
                gate.enter().await.unwrap();
                sent.fetch_add(1, Ordering::SeqCst);
            });
        }
        let gap = MIN_REQUEST_GAP.as_millis() as i64;
        for (now, want) in [
            (0, 1),
            (gap - 1, 1),
            (gap, 2),
            (2 * gap - 1, 2),
            (2 * gap, 3),
        ] {
            clock.set(now);
            // Lets every ready task run before counting.
            for _ in 0..50 {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                sent.load(Ordering::SeqCst),
                want,
                "虚拟时钟 {now} 毫秒时发出的请求数"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A slot reserved for 300 ms is worth nothing if the task only gets
    /// back to it at 1000 ms: both queued callers would leave together,
    /// which is the burst the gap exists to prevent (#74). The slot is
    /// taken when a caller really gets it, so the gap counts from the
    /// request that went out. The delay is the injected clock jumping, so
    /// this never depends on how loaded the machine is.
    #[tokio::test]
    async fn callers_woken_up_late_still_leave_one_gap_apart() {
        let dir = temp_dir("late-wake-up");
        let clock = TestClock::new();
        let gate = Arc::new(gate(&dir, clock.clone()));
        let sent = Arc::new(AtomicUsize::new(0));
        // The first request leaves at 0 ms and takes the slot after it.
        gate.enter().await.unwrap();
        sent.fetch_add(1, Ordering::SeqCst);
        for _ in 0..2 {
            let (gate, sent) = (gate.clone(), sent.clone());
            tokio::spawn(async move {
                gate.enter().await.unwrap();
                sent.fetch_add(1, Ordering::SeqCst);
            });
        }
        // Both ask while the clock still says 0 ms, so both queue behind
        // the first request...
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(sent.load(Ordering::SeqCst), 1, "0 毫秒就放行了排队的请求");
        // ...and nothing runs them again until long after their turn.
        let gap = MIN_REQUEST_GAP.as_millis() as i64;
        for (now, want) in [(1_000, 2), (1_000 + gap - 1, 2), (1_000 + gap, 3)] {
            clock.set(now);
            for _ in 0..50 {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                sent.load(Ordering::SeqCst),
                want,
                "虚拟时钟 {now} 毫秒时发出的请求数：晚醒来的请求被一起放了出去"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A lock file an earlier run left behind opens for writing even in a
    /// directory that has since become read-only, and the breaker is
    /// written as a new file next to it. Without really writing one,
    /// theFlash would send its requests and only find out when risk
    /// control arrives and cannot be recorded (#74).
    #[test]
    fn a_lock_file_does_not_prove_the_breaker_can_be_written() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("read-only-with-lock");
        fs::create_dir_all(&dir).unwrap();
        File::create(dir.join(LOCK_FILE)).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        // root writes a read-only directory anyway, so there would be
        // nothing to see; say so instead of passing quietly.
        let writable_anyway = fs::write(dir.join("by-root"), b"").is_ok();
        let made = Gate::new(
            dir.clone(),
            MIN_REQUEST_GAP,
            Arc::new(SystemClock::default()),
            "theflash test".into(),
        );
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        if writable_anyway {
            eprintln!("跳过：这个用户（root？）连只读目录也能写，看不出区别");
        } else {
            let e = made.expect_err("只读的状态目录被当成可以用");
            assert!(matches!(e, Error::RiskState { .. }), "{e}");
            assert!(e.to_string().contains("已停止"), "{e}");
            assert!(e.to_string().contains(&dir.display().to_string()), "{e}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_breaker_another_process_wrote_stops_the_next_request() {
        let dir = temp_dir("breaker");
        let clock = TestClock::new();
        let one = gate(&dir, clock.clone());
        let two = gate(&dir, clock.clone());
        assert!(two.enter().await.is_ok());

        let tripped = one.trip("https://detail.damai.cn/item.htm?id=1", "x5secdata");
        assert!(
            matches!(tripped, Error::RiskControl(_)),
            "没写成熔断状态：{tripped}"
        );
        clock.set(10_000);
        let stopped = two.enter().await.expect_err("第二个客户端还在放行请求");
        let Error::RiskStopped(stop) = &stopped else {
            panic!("{stopped}");
        };
        assert_eq!(stop.marker, "x5secdata");
        assert_eq!(stop.url, "https://detail.damai.cn/item.htm?id=1");
        assert!(stop.at.contains("（北京时间）"), "{}", stop.at);
        assert!(
            stopped.to_string().contains("theflash risk resume --yes"),
            "{stopped}"
        );

        assert_eq!(two.resume().unwrap(), Resumed::Cleared(stop.clone()));
        clock.set(20_000);
        assert!(two.enter().await.is_ok(), "解除后仍然不放行");
        assert_eq!(
            two.resume().unwrap(),
            Resumed::Nothing,
            "解除后还有熔断状态"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_corrupt_breaker_stops_requests_instead_of_passing_for_no_breaker() {
        let dir = temp_dir("corrupt");
        let clock = TestClock::new();
        let gate = gate(&dir, clock.clone());
        for broken in ["", "{", "{\"at\":\"now\"}", "[]"] {
            fs::write(dir.join(STOP_FILE), broken).unwrap();
            clock.set(clock.now_ms() + 10_000);
            let e = gate
                .enter()
                .await
                .expect_err(&format!("内容 {broken:?} 被当成没有熔断"));
            assert!(matches!(e, Error::RiskState { .. }), "{e}");
            assert!(e.to_string().contains(STOP_FILE), "{e}");
            // Clearing it says the breaker was unreadable rather than
            // claiming there was none, and queries work again.
            assert!(matches!(
                gate.resume().unwrap(),
                Resumed::ClearedUnreadable(_)
            ));
            clock.set(clock.now_ms() + 10_000);
            assert!(gate.enter().await.is_ok(), "删掉坏文件后仍然不放行");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_state_directory_that_cannot_be_created_is_a_stop_with_a_reason() {
        let dir = temp_dir("unwritable");
        fs::create_dir_all(&dir).unwrap();
        let blocked = dir.join("state");
        fs::write(&blocked, "a file where the state directory goes").unwrap();
        let e = Gate::new(
            blocked,
            MIN_REQUEST_GAP,
            Arc::new(SystemClock::default()),
            "theflash test".into(),
        )
        .expect_err("不可写的状态目录没有停下来");
        assert!(matches!(e, Error::RiskState { .. }), "{e}");
        assert!(e.to_string().contains("已停止"), "{e}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_command_in_the_state_file_keeps_no_arguments() {
        let label = |args: &[&str]| command_label(args.iter().map(|a| a.to_string()));
        assert_eq!(
            label(&[
                "/usr/local/bin/theflash",
                "event",
                "https://detail.damai.cn/item.htm?id=1086791513717"
            ]),
            "theflash event"
        );
        assert_eq!(label(&["target/debug/theflash", "watch"]), "theflash watch");
        assert_eq!(label(&["theflash", "--version"]), "theflash");
        assert_eq!(label(&[]), "theflash");
    }

    #[test]
    fn an_asked_for_gap_only_slows_requests_down() {
        assert_eq!(gap_from(None).unwrap(), MIN_REQUEST_GAP);
        assert_eq!(gap_from(Some("2000")).unwrap(), Duration::from_secs(2));
        assert_eq!(
            gap_from(Some("86400000")).unwrap(),
            Duration::from_secs(86400)
        );
        for faster in ["0", "1", "299"] {
            assert_eq!(
                gap_from(Some(faster)).unwrap(),
                MIN_REQUEST_GAP,
                "{faster} 毫秒比共用的频率上限还快"
            );
        }
        for value in ["-1", "bad", "86400001", "18446744073709551615"] {
            assert!(gap_from(Some(value)).is_err(), "{value}");
        }
    }

    #[tokio::test]
    async fn an_asked_for_gap_paces_every_caller_in_this_process() {
        let dir = temp_dir("asked-for-gap");
        let clock = TestClock::new();
        let gate = Gate::new(
            dir.clone(),
            Duration::from_secs(2),
            clock.clone(),
            "theflash test".into(),
        )
        .unwrap();
        gate.enter().await.unwrap();
        let waiting = tokio::spawn({
            let gate = Arc::new(gate);
            async move { gate.enter().await }
        });
        clock.set(1_999);
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert!(!waiting.is_finished(), "1999 毫秒就放行了第二个请求");
        clock.set(2_000);
        waiting.await.unwrap().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_slider_challenge_is_recognised_by_which_marker_it_carries() {
        let dir = format!("{}/testdata/damai", env!("CARGO_MANIFEST_DIR"));
        let read = |n: &str| fs::read_to_string(format!("{dir}/{n}")).unwrap();
        assert_eq!(
            risk_control_marker(&read("risk_control.html")),
            Some("_____tmd_____")
        );
        assert_eq!(
            risk_control_marker("https://detail.damai.cn/_____tmd_____/punish?x5secdata=a"),
            Some("_____tmd_____")
        );
        assert_eq!(
            risk_control_marker("https://example.com/punish?x5secdata=a"),
            Some("x5secdata")
        );
        for name in [
            "item_on_sale.html",
            "item_not_yet_on_sale.html",
            "item_sold_out.html",
            "item_pc_unsupported.html",
            "item_not_found.html",
            "sku_on_sale_286937790.jsonp",
        ] {
            assert_eq!(risk_control_marker(&read(name)), None, "{name}");
        }
    }
}
