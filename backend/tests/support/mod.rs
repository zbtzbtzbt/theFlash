//! A stand-in for detail.damai.cn that replays real responses saved in
//! testdata/damai. No test reaches the real damai.cn.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
    hits: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MockDamai {
    pub fn start(routes: &[(&'static str, &'static str)]) -> Self {
        Self::serve(fixtures(routes), None)
    }

    /// Routes to bodies given in full, such as a fixture with one field changed.
    pub fn start_bodies(routes: Vec<(String, String)>) -> Self {
        Self::serve(routes, None)
    }

    /// `damai_ms` is damai.cn's clock, ms since the epoch, when the server starts.
    pub fn start_with_clock(routes: &[(&'static str, &'static str)], damai_ms: i64) -> Self {
        Self::serve(fixtures(routes), Some((Instant::now(), damai_ms)))
    }

    fn serve(routes: Vec<(String, String)>, clock: Option<(Instant, i64)>) -> Self {
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

    /// "METHOD target" of every request so far.
    pub fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
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
    routes: &[(String, String)],
    hits: &Mutex<Vec<String>>,
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
    hits.lock().unwrap().push(format!("{method} {target}"));
    let (status, body) = match routes.iter().find(|(m, _)| target.contains(m.as_str())) {
        Some((_, body)) => ("200 OK", body.as_str()),
        None => ("404 Not Found", "not found"),
    };
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
        "HTTP/1.1 {status}\r\n{date}Content-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn fixtures(routes: &[(&'static str, &'static str)]) -> Vec<(String, String)> {
    routes
        .iter()
        .map(|(m, f)| (m.to_string(), fixture(f)))
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

/// The theflash binary with its damai.cn requests sent to `base_url` and
/// the machine's proxies cleared.
pub fn theflash(base_url: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_theflash"));
    c.env("THEFLASH_DAMAI_BASE_URL", base_url);
    for v in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        c.env_remove(v);
    }
    c
}
