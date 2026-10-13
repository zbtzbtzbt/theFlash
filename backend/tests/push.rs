//! Real watch + local ntfy, with real saved Damai fixtures. No third-party requests.
mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Reply = Box<dyn FnMut(&str, &str) -> (u16, String) + Send>;

struct Server {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<(String, String, String)>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn start(reply: Reply) -> Self {
        Self::with_clock(reply, None)
    }

    fn with_clock(mut reply: Reply, clock: Option<i64>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let requests = requests.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                for stream in listener.incoming().flatten() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    let mut headers = String::new();
                    let mut len = 0;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                            break;
                        }
                        if let Some(n) = line.to_lowercase().strip_prefix("content-length:") {
                            len = n.trim().parse::<usize>().unwrap();
                        }
                        headers.push_str(&line);
                    }
                    let mut bytes = vec![0; len];
                    reader.read_exact(&mut bytes).unwrap();
                    let body = String::from_utf8(bytes).unwrap();
                    requests
                        .lock()
                        .unwrap()
                        .push((first.clone(), headers, body.clone()));
                    let (code, response) = reply(&first, &body);
                    let mut stream = stream;
                    let location = if code == 307 {
                        format!("Location: {response}\r\n")
                    } else {
                        String::new()
                    };
                    let date = clock
                        .map(|ms| {
                            let now = chrono::DateTime::from_timestamp_millis(
                                ms + started.elapsed().as_millis() as i64,
                            )
                            .unwrap();
                            format!("Date: {}\r\n", now.format("%a, %d %b %Y %H:%M:%S GMT"))
                        })
                        .unwrap_or_default();
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {code} Test\r\n{date}{location}Content-Length: {}\r\nConnection: close\r\n\r\n{response}",
                        response.len()
                    );
                }
            })
        };
        Self {
            addr,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn wait_for(&self, n: usize, seconds: u64) {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while self.count() < n && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            self.count() >= n,
            "expected {n} requests, got {}",
            self.count()
        );
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        self.thread.take().unwrap().join().unwrap();
    }
}

struct Running {
    child: Child,
    home: PathBuf,
    output: Arc<Mutex<String>>,
    reader: Option<JoinHandle<()>>,
}

impl Running {
    fn start(damai: &Server, push: &Server, config: Option<serde_json::Value>, mode: u32) -> Self {
        Self::start_args(
            damai,
            push,
            config,
            mode,
            &["1086791513717", "--session", "10-15", "--tier", "C-178"],
        )
    }

    fn start_args(
        damai: &Server,
        push: &Server,
        config: Option<serde_json::Value>,
        mode: u32,
        args: &[&str],
    ) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let home = std::env::temp_dir().join(format!(
            "theflash-push-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let dir = home.join(".config/theflash");
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(mut config) = config {
            config["server"] = push.url().into();
            let path = dir.join("push.json");
            std::fs::write(&path, config.to_string()).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let mut child = support::theflash(&damai.url())
            .env("HOME", &home)
            .env_remove("XDG_CONFIG_HOME")
            .arg("watch")
            .args(args)
            .args(["--interval", "10", "--no-system-notify"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = Arc::new(Mutex::new(String::new()));
        let pipe = child.stdout.take().unwrap();
        let log = output.clone();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                let mut text = log.lock().unwrap();
                text.push_str(&line);
                text.push('\n');
            }
        });
        Self {
            child,
            home,
            output,
            reader: Some(reader),
        }
    }

    fn wait_text(&self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !self.output.lock().unwrap().contains(text) {
            assert!(Instant::now() < deadline, "watch did not print {text}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn stop(&mut self) -> String {
        assert!(
            std::process::Command::new("kill")
                .args(["-INT", &self.child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "watch did not stop");
            std::thread::sleep(Duration::from_millis(20));
        }
        self.reader.take().unwrap().join().unwrap();
        let mut out = self.output.lock().unwrap().clone();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        out
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

#[test]
fn restock_pushes_once_and_unchanged_availability_is_quiet() {
    let damai = damai(vec![false, true, true]);
    let push = Server::start(Box::new(|_, _| {
        (200, include_str!("../testdata/ntfy/received.json").into())
    }));
    let mut run = Running::start(&damai, &push, Some(config()), 0o600);
    push.wait_for(1, 25);
    damai.wait_for(8, 35); // next round proves the preceding unchanged round completed
    run.wait_text("时间：");
    let out = run.stop();
    assert_eq!(push.count(), 1, "unchanged state pushed twice");
    let payload: serde_json::Value =
        serde_json::from_str(&push.requests.lock().unwrap()[0].2).unwrap();
    assert_eq!(payload["title"], "有票了");
    let body = payload["message"].as_str().unwrap();
    assert!(body.contains("C-178元 · ¥178.00 · 可售"));
    for line in body.lines() {
        assert!(out.contains(&format!("  {line}")));
    }
    assert_no_secrets(&out);
}

#[test]
fn failed_push_is_not_retried_and_the_watch_reports_the_next_restock() {
    let damai = damai(vec![false, true, false, true]);
    let push = Server::start(Box::new(|_, body| {
        (503, format!("{body} test-token-placeholder"))
    })); // echoes secrets
    let mut run = Running::start(&damai, &push, Some(config()), 0o600);
    push.wait_for(2, 50);
    run.wait_text("手机推送失败");
    let out = run.stop();
    assert_eq!(push.count(), 2, "one attempt per transition, no retry");
    assert_eq!(out.matches("【提醒】有票了").count(), 2);
    assert!(out.contains("又没票了"));
    assert_no_secrets(&out);
}

#[test]
fn redirect_is_not_followed_and_cannot_forward_credentials() {
    let damai = damai(vec![true]);
    let target = Server::start(Box::new(|_, _| (200, "{}".into())));
    let destination = format!("{}/test-topic-placeholder", target.url());
    let push = Server::start(Box::new(move |_, _| (307, destination.clone())));
    let mut run = Running::start(&damai, &push, Some(config()), 0o600);
    run.wait_text("手机推送失败");
    let out = run.stop();
    assert_eq!(push.count(), 1);
    assert_eq!(
        target.count(),
        0,
        "redirect forwarded notification credentials"
    );
    assert_no_secrets(&out);
}

#[test]
fn network_failure_only_prints_a_safe_message_and_token_is_optional() {
    let damai = damai(vec![true]);
    let push = Server::start(Box::new(|_, _| {
        (0, "invalid HTTP response test-token-placeholder".into())
    }));
    let mut cfg = config();
    cfg.as_object_mut().unwrap().remove("token");
    let mut run = Running::start(&damai, &push, Some(cfg), 0o600);
    run.wait_text("网络失败或超时");
    let out = run.stop();
    assert_eq!(push.count(), 1);
    assert!(
        !push.requests.lock().unwrap()[0]
            .1
            .to_lowercase()
            .contains("bearer")
    );
    assert!(out.contains("【提醒】现在就可以买"));
    assert_no_secrets(&out);
    assert!(
        !out.contains(&push.url()),
        "network error leaked server URL"
    );
}

#[test]
fn missing_config_keeps_the_old_watch_output_and_sends_nothing() {
    let damai = damai(vec![true]);
    let push = Server::start(Box::new(|_, _| {
        (200, include_str!("../testdata/ntfy/received.json").into())
    }));
    let mut run = Running::start(&damai, &push, None, 0o600);
    run.wait_text("时间：");
    let out = run.stop();
    assert_eq!(push.count(), 0);
    assert!(!out.contains("手机推送"));
    assert!(out.contains("【提醒】现在就可以买"));
}

#[test]
fn unsafe_permissions_and_invalid_config_do_not_leak_or_stop_the_watch() {
    for (mut cfg, mode, why) in [
        (config(), 0o644, "0600"),
        (config(), 0o600, "配置 JSON 无效"),
        (config(), 0o600, "token 格式无效"),
    ] {
        if why == "配置 JSON 无效" {
            cfg["channel"] = "test-token-placeholder".into();
        }
        if why == "token 格式无效" {
            cfg["token"] = "test-token-placeholder\n".into();
        }
        let damai = damai(vec![true]);
        let push = Server::start(Box::new(|_, _| {
            (200, include_str!("../testdata/ntfy/received.json").into())
        }));
        let mut run = Running::start(&damai, &push, Some(cfg), mode);
        run.wait_text("时间：");
        let out = run.stop();
        assert!(out.contains(why));
        assert!(out.contains("【提醒】现在就可以买"));
        assert_no_secrets(&out);
        assert_eq!(push.count(), 0);
    }
}

#[test]
fn opening_push_distinguishes_time_due_from_confirmed_availability() {
    const SALE: i64 = 1_791_944_280_000;
    let page = support::fixture("item_not_yet_on_sale.html");
    let sku = support::fixture("sku_not_yet_on_sale_286966465.jsonp");
    let mut round = 0;
    let damai = Server::with_clock(
        Box::new(move |first, _| {
            if first.starts_with("HEAD") {
                return (200, String::new());
            }
            if first.contains("/item.htm") {
                return (200, page.clone());
            }
            round += 1;
            if round == 1 {
                return (200, sku.clone());
            }
            let mut data: serde_json::Value = serde_json::from_str(&sku[6..sku.len() - 1]).unwrap();
            data["itemBuyBtn"]["btnStatus"] = "204".into();
            for tier in data["perform"]["skuList"].as_array_mut().unwrap() {
                tier["frontEndStatus"] = "1".into();
                tier["skuSalable"] = "true".into();
            }
            (200, format!("__jp0({data})"))
        }),
        Some(SALE - 5_000),
    );
    let push = Server::start(Box::new(|_, _| {
        (200, include_str!("../testdata/ntfy/received.json").into())
    }));
    let mut run = Running::start_args(&damai, &push, Some(config()), 0o600, &["1092337348536"]);
    push.wait_for(2, 30);
    let out = run.stop();
    let requests = push.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let due: serde_json::Value = serde_json::from_str(&requests[0].2).unwrap();
    let confirmed: serde_json::Value = serde_json::from_str(&requests[1].2).unwrap();
    assert_eq!(due["title"], "开售时间到");
    assert!(
        due["message"]
            .as_str()
            .unwrap()
            .contains("正在确认能不能买")
    );
    assert_eq!(confirmed["title"], "已经可以买了");
    assert!(confirmed["message"].as_str().unwrap().contains("可售"));
    for payload in [&due, &confirmed] {
        for line in payload["message"].as_str().unwrap().lines() {
            assert!(out.contains(&format!("  {line}")));
        }
    }
    assert_no_secrets(&out);
}

fn config() -> serde_json::Value {
    // These are deliberately fake credentials, never used outside local servers.
    serde_json::json!({"channel":"ntfy", "topic":"test-topic-placeholder", "token":"test-token-placeholder"})
}

fn damai(states: Vec<bool>) -> Server {
    let page = support::fixture("item_on_sale.html");
    let sku = support::fixture("sku_on_sale_286937790.jsonp");
    let mut round = 0;
    Server::start(Box::new(move |first, _| {
        if first.contains("/item.htm") {
            return (200, page.clone());
        }
        let available = states[round.min(states.len() - 1)];
        round += 1;
        let mut data: serde_json::Value = serde_json::from_str(&sku[6..sku.len() - 1]).unwrap();
        let tier = &mut data["perform"]["skuList"][1];
        tier["frontEndStatus"] = if available { "1" } else { "2" }.into();
        tier["skuSalable"] = if available { "true" } else { "false" }.into();
        (200, format!("__jp0({data})"))
    }))
}

fn assert_no_secrets(out: &str) {
    assert!(!out.contains("test-topic-placeholder"), "topic leaked");
    assert!(!out.contains("test-token-placeholder"), "token leaked");
}

#[test]
fn available_at_start_pushes_the_desktop_title_and_body() {
    let damai = damai(vec![true]);
    let push = Server::start(Box::new(|_, _| {
        (200, include_str!("../testdata/ntfy/received.json").into())
    }));
    let mut run = Running::start(&damai, &push, Some(config()), 0o600);
    push.wait_for(1, 8);
    let out = run.stop();
    assert_no_secrets(&out);
    let requests = push.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "POST / HTTP/1.1\r\n");
    let payload: serde_json::Value = serde_json::from_str(&requests[0].2).unwrap();
    assert_eq!(payload["title"], "现在就可以买");
    assert_eq!(payload["topic"], "test-topic-placeholder");
    assert!(
        requests[0]
            .1
            .to_lowercase()
            .contains("bearer test-token-placeholder")
    );
    let body = payload["message"].as_str().unwrap();
    assert!(body.contains("2026-10-15 周四 17:00 · C-178元 · ¥178.00 · 可售"));
    assert!(body.contains("时间："));
    for line in body.lines() {
        assert!(
            out.contains(&format!("  {line}")),
            "desktop and phone differ"
        );
    }
}
