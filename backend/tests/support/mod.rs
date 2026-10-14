//! A stand-in for detail.damai.cn that replays real responses saved in
//! testdata/damai. No test reaches the real damai.cn.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use chrono::{DateTime, Utc};

/// Each route is a substring of the request target and the fixture served
/// for it; anything else gets a 404. With a clock, every response carries a
/// Date header from damai.cn's pretend clock, which starts at the given
/// instant and runs in real time. The server stops and its thread is
/// joined when the value is dropped.
pub struct MockDamai {
    addr: SocketAddr,
    hits: Arc<Mutex<Vec<(String, Instant)>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// One response the stand-in sends: the status line, the Location damai.cn
/// sends a browser to when it redirects, and the body.
#[derive(Clone)]
pub struct Reply {
    pub status: &'static str,
    pub location: Option<String>,
    pub body: String,
}

impl Reply {
    pub fn ok(body: impl Into<String>) -> Reply {
        Reply {
            status: "200 OK",
            location: None,
            body: body.into(),
        }
    }

    /// What damai.cn answers with when it sends the browser somewhere
    /// else, including to its slider challenge.
    pub fn redirect(location: impl Into<String>) -> Reply {
        Reply {
            status: "302 Found",
            location: Some(location.into()),
            body: String::new(),
        }
    }
}

impl MockDamai {
    pub fn start(routes: &[(&'static str, &'static str)]) -> Self {
        Self::serve(fixtures(routes), None)
    }

    /// Routes to bodies given in full, such as a fixture with one field changed.
    pub fn start_bodies(routes: Vec<(String, String)>) -> Self {
        Self::serve(
            routes.into_iter().map(|(t, b)| (t, Reply::ok(b))).collect(),
            None,
        )
    }

    /// Routes to responses given in full, for the redirects damai.cn
    /// answers with.
    pub fn start_replies(routes: Vec<(String, Reply)>) -> Self {
        Self::serve(routes, None)
    }

    /// `damai_ms` is damai.cn's clock, ms since the epoch, when the server starts.
    pub fn start_with_clock(routes: &[(&'static str, &'static str)], damai_ms: i64) -> Self {
        Self::serve(fixtures(routes), Some((Instant::now(), damai_ms)))
    }

    fn serve(routes: Vec<(String, Reply)>, clock: Option<(Instant, i64)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let hits = hits.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    answer(stream, &routes, &hits, clock);
                }
            })
        };
        MockDamai {
            addr,
            hits,
            stop,
            thread: Some(thread),
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Server arrival times, used to check actual GET/HEAD pacing.
    pub fn timed_hits(&self) -> Vec<(String, Instant)> {
        self.hits.lock().unwrap().clone()
    }

    /// "METHOD target" of every request so far.
    pub fn hits(&self) -> Vec<String> {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .map(|(request, _)| request.clone())
            .collect()
    }

    /// How long each request waited behind the one before it, measured
    /// where it matters: when the request actually arrived. A pacing that
    /// is only printed and not waited out shows up here as zero.
    pub fn gaps(&self) -> Vec<std::time::Duration> {
        let hits = self.hits.lock().unwrap();
        hits.windows(2).map(|w| w[1].1 - w[0].1).collect()
    }

    /// ms between the arrivals of each two requests in a row, for checking
    /// that the caller paced itself.
    pub fn arrival_gaps_ms(&self) -> Vec<i64> {
        self.timed_hits()
            .windows(2)
            .map(|w| w[1].1.saturating_duration_since(w[0].1).as_millis() as i64)
            .collect()
    }
}

impl Drop for MockDamai {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn answer(
    mut stream: TcpStream,
    routes: &[(String, Reply)],
    hits: &Mutex<Vec<(String, Instant)>>,
    clock: Option<(Instant, i64)>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line == "\r\n" => break,
            Ok(_) => {}
            Err(_) => return,
        }
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    hits.lock()
        .unwrap()
        .push((format!("{method} {target}"), Instant::now()));
    let not_found = Reply {
        status: "404 Not Found",
        location: None,
        body: "not found".to_string(),
    };
    let reply = match routes.iter().find(|(m, _)| target.contains(m.as_str())) {
        Some((_, reply)) => reply,
        None => &not_found,
    };
    let (status, body) = (reply.status, reply.body.as_str());
    let location = reply
        .location
        .as_ref()
        .map(|l| format!("Location: {l}\r\n"))
        .unwrap_or_default();
    let date = clock
        .map(|(started, ms)| {
            let now = ms + started.elapsed().as_millis() as i64;
            let t: DateTime<Utc> = DateTime::from_timestamp_millis(now).unwrap();
            format!("Date: {}\r\n", t.format("%a, %d %b %Y %H:%M:%S GMT"))
        })
        .unwrap_or_default();
    let body = if method == "HEAD" { "" } else { body };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\n{date}{location}Content-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn fixtures(routes: &[(&'static str, &'static str)]) -> Vec<(String, Reply)> {
    routes
        .iter()
        .map(|(m, f)| (m.to_string(), Reply::ok(fixture(f))))
        .collect()
}

pub fn fixture(name: &str) -> String {
    let path = format!("{}/testdata/damai/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Runs `cmd` to the end, failing the test if it is still running after
/// `secs`; the process is killed either way, so none outlives its test.
pub fn output_within(mut cmd: Command, secs: u64) -> std::process::Output {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let deadline = Instant::now() + Duration::from_secs(secs);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("still running after {secs} s");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    std::process::Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}

/// The theflash binary with its damai.cn requests sent to `base_url`,
/// every request that is not for this machine caught by `offline`, and a
/// HOME of this test binary's own, so nothing reads or writes the real
/// user's state directory.
pub fn theflash(base_url: &str) -> Command {
    at_home(base_url, shared_home())
}

fn at_home(base_url: &str, home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_theflash"));
    c.env("THEFLASH_DAMAI_BASE_URL", base_url);
    offline(&mut c);
    // offline() already points HOME at the shared test directory; this
    // test wants one of its own.
    c.env("HOME", home);
    c
}

/// One directory for every test in this binary that does not touch the
/// risk-control breaker. Emptied when the binary first uses it.
fn shared_home() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("theflash-test-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    })
}

/// A HOME of one test's own, removed when the test ends. A test that trips
/// the risk-control breaker needs one: the breaker stops every process
/// that shares a HOME, which would stop other tests' queries too.
pub struct Home(PathBuf);

impl Home {
    pub fn new(name: &str) -> Home {
        let dir =
            std::env::temp_dir().join(format!("theflash-test-home-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Home(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// `support::theflash`, with this directory as HOME.
    pub fn theflash(&self, base_url: &str) -> Command {
        at_home(base_url, &self.0)
    }

    /// Where theFlash keeps its state on this platform, spelled out here so
    /// a test fails if the directory moves. macOS and Linux differ.
    pub fn state_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.0.join("Library/Application Support/theFlash")
        } else {
            self.0.join(".local/state/theflash")
        }
    }

    pub fn risk_stop_file(&self) -> PathBuf {
        self.state_dir().join("risk-stop.json")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The dispatch binary with nothing to reach GitHub or an agent with:
/// PATH is `home/bin`, which a test fills with the stubs it wants, so a
/// test that forgets `--replay` fails instead of asking the real GitHub,
/// and no session is ever started by a real agent command line. Every
/// test that starts dispatch goes through here.
pub fn dispatch(home: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_dispatch"));
    std::fs::create_dir_all(home.join("bin")).unwrap();
    c.env("PATH", home.join("bin"));
    c.env("HOME", home);
    offline(&mut c);
    c
}

/// Writes an executable stub named `name` into the PATH `dispatch` uses,
/// so a test sees what the dispatcher asked a `gh`, `scripts/claims` or
/// agent command line to do, without any of them being real.
pub fn stub(home: &std::path::Path, name: &str, script: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = home.join("bin").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Makes a real tool from this machine (`git`) reachable through the
/// test's PATH, for the paths that genuinely need it.
pub fn real_tool(home: &std::path::Path, name: &str) {
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name}"))
        .output()
        .unwrap();
    let found = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(!found.is_empty(), "这台机器上找不到 {name}");
    let link = home.join("bin").join(name);
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(found, link).unwrap();
}

/// Makes `cmd` unable to reach the real damai.cn or the real user's files:
/// its proxies become a tripwire, and only this machine's addresses skip
/// the proxy. A test that forgets THEFLASH_DAMAI_BASE_URL sends its request
/// to the tripwire, which ends the test process with a message saying so.
/// HOME points at a test directory, because theflash keeps the
/// risk-control breaker under HOME (under XDG_STATE_HOME on Linux when it
/// is set) and a test must not read or write the user's own. Every test
/// that starts the binary goes through here (tests/repo_guard.rs checks
/// that), so both guarantees hold for all of them.
pub fn offline(cmd: &mut Command) {
    let proxy = tripwire();
    for v in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
        cmd.env(v, proxy);
        cmd.env(v.to_lowercase(), proxy);
    }
    cmd.env("NO_PROXY", "127.0.0.1,localhost,::1");
    cmd.env("no_proxy", "127.0.0.1,localhost,::1");
    cmd.env("THEFLASH_BROWSER", "");
    cmd.env("HOME", shared_home());
    cmd.env_remove("XDG_STATE_HOME");
}

/// The merge-gate binary with nothing to reach GitHub with: `gh` is gone
/// from PATH, so a test that forgets `--replay` fails instead of asking
/// the real GitHub, and the merge lock lives under `state` instead of the
/// real home directory. Every test that starts merge-gate goes through
/// here (tests/repo_guard.rs checks that).
pub fn merge_gate(state: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_merge-gate"));
    std::fs::create_dir_all(state).unwrap();
    c.env("PATH", empty_dir());
    c.env("HOME", state);
    c.env("THEFLASH_MERGE_GATE_STATE", state);
    offline(&mut c);
    c
}

/// The claims binary with nothing to reach GitHub with: `gh` is gone from
/// PATH, so a test that forgets `--replay` fails instead of asking the
/// real GitHub, and `home` stands in for the home directory. Every test
/// that starts claims goes through here (tests/repo_guard.rs checks that).
pub fn claims(home: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_claims"));
    std::fs::create_dir_all(home).unwrap();
    c.env("PATH", empty_dir());
    c.env("HOME", home);
    offline(&mut c);
    c
}

/// A directory with no executables in it, used as PATH. One per test
/// binary; it holds nothing, so nothing outlives the run.
fn empty_dir() -> &'static str {
    static DIR: OnceLock<String> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("theflash-empty-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.display().to_string()
    })
}

/// The address of a proxy that fails the test run for any request it
/// receives. One per test binary, started on first use; it ends with the
/// binary, and nothing a test starts depends on it.
fn tripwire() -> &'static str {
    static ADDR: OnceLock<String> = OnceLock::new();
    ADDR.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                tripped(stream);
            }
        });
        format!("http://{addr}")
    })
}

/// A request reached the tripwire, so something tried to leave this
/// machine. Panicking here would only end this thread, and the test that
/// caused it might still pass, so the whole run ends with exit code 101,
/// the code of a failed test. stderr is written directly because the test
/// harness hides what `eprintln!` prints from a passing or a killed test.
fn tripped(stream: TcpStream) {
    let mut request = String::new();
    // Read through a reference: the connection must stay open until the
    // process exits. Closed earlier, theflash fails on the dropped tunnel and
    // returns, and the test that started it can report a pass or its own
    // panic before the exit below ends the run.
    let _ = BufReader::new(&stream).read_line(&mut request);
    let message = format!(
        "\n测试试图访问真实大麦：theflash 发出了 `{}`，目标不是本机。\n\
         测试必须把 THEFLASH_DAMAI_BASE_URL 指向本地假服务器（MockDamai）。\
         测试进程已终止。\n",
        request.trim()
    );
    let _ = std::io::stderr().write_all(message.as_bytes());
    std::process::exit(101);
}
