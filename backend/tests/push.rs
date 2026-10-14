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
        Self::start_configured(damai, push, config, mode, args, |_, _| {})
    }

    fn start_configured(
        damai: &Server,
        push: &Server,
        config: Option<serde_json::Value>,
        mode: u32,
        args: &[&str],
        configure: impl FnOnce(&PathBuf, &mut std::process::Command),
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
        let mut command = support::theflash(&damai.url());
        command
            .env("HOME", &home)
            .env_remove("XDG_CONFIG_HOME")
            .arg("watch")
            .args(args)
            .args(["--interval", "10", "--no-system-notify"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure(&home, &mut command);
        let mut child = command.spawn().unwrap();
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
        self.wait_exit(0)
    }

    fn wait_exit(&mut self, code: i32) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert_eq!(status.code(), Some(code));
                break;
            }
            assert!(Instant::now() < deadline, "watch did not exit");
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
fn risk_control_push_finishes_before_the_watch_exits() {
    for code in [200, 503] {
        let page = support::fixture("risk_control.html");
        let damai = Server::start(Box::new(move |_, _| (200, page.clone())));
        let push = Server::start(Box::new(move |_, _| {
            // Hold the response so receipt alone cannot stand in for completion.
            // The rejected response must be reported before watch exits too.
            std::thread::sleep(Duration::from_millis(200));
            (code, include_str!("../testdata/ntfy/received.json").into())
        }));
        let mut run = Running::start_args(&damai, &push, Some(config()), 0o600, &["1086791513717"]);
        // Let risk control end watch itself; never wait for delivery then send SIGINT.
        let out = run.wait_exit(1);
        assert!(out.contains("【提醒】触发风控，已停止盯票"));
        assert_no_secrets(&out);
        assert!(
            !out.contains("手机推送失败（服务器拒绝推送；topic/token：***），继续盯票"),
            "an exiting watch must not claim it will continue watching"
        );
        assert_eq!(damai.count(), 1, "risk control must stop Damai requests");
        let requests = push.requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "risk-control alert was not pushed once");
        assert_eq!(requests[0].0, "POST / HTTP/1.1\r\n");
        let payload: serde_json::Value = serde_json::from_str(&requests[0].2).unwrap();
        assert_eq!(payload["title"], "触发风控，已停止盯票");
        let body = payload["message"].as_str().unwrap();
        assert!(body.contains("大麦触发了风控"));
        for line in body.lines() {
            assert!(
                out.contains(&format!("  {line}")),
                "desktop and phone differ"
            );
        }
        assert_eq!(
            out.contains("手机推送失败（服务器拒绝推送；topic/token：***）"),
            code == 503,
            "watch exited before the push result was reported"
        );
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

// Acceptance F1: the recorded calendar event has 18 sessions and six tiers.
// Only availability (and the opening clock in the second test) is changed.
fn calendar_damai(opening: bool) -> Server {
    const SALE: i64 = 1_791_944_280_000;
    let mut page = support::fixture("item_calendar.html");
    if opening {
        let start = page.find("id=\"dataDefault\"").unwrap();
        let start = start + page[start..].find('>').unwrap() + 1;
        let mut parsed =
            serde_json::Deserializer::from_str(&page[start..]).into_iter::<serde_json::Value>();
        let mut data = parsed.next().unwrap().unwrap();
        let end = start + parsed.byte_offset();
        data["buyBtnStatus"] = "106".into();
        data["sellStartTime"] = SALE.into();
        page.replace_range(start..end, &data.to_string());
    }
    let sku = support::fixture("sku_calendar_286938129.jsonp");
    let mut queried = 0;
    Server::with_clock(
        Box::new(move |first, _| {
            if first.starts_with("HEAD") {
                return (200, String::new());
            }
            if first.contains("/item.htm") {
                return (200, page.clone());
            }
            let id = first
                .split("dataId=")
                .nth(1)
                .unwrap()
                .split('&')
                .next()
                .unwrap();
            let mut data: serde_json::Value = serde_json::from_str(&sku[6..sku.len() - 1]).unwrap();
            data["perform"]["performId"] = id.into();
            data["itemBuyBtn"]["btnStatus"] = if opening && queried < 18 {
                "106"
            } else {
                "206"
            }
            .into();
            queried += 1;
            for tier in data["perform"]["skuList"].as_array_mut().unwrap() {
                tier["frontEndStatus"] = "1".into();
                tier["skuSalable"] = "true".into();
            }
            (200, format!("__jp0({data})"))
        }),
        opening.then_some(SALE - 15_000),
    )
}

fn size_limited_push() -> Server {
    Server::start(Box::new(|_, body| {
        let payload: serde_json::Value = serde_json::from_str(body).unwrap();
        // Reproduce ntfy's rejection at the protocol boundary, locally.
        if payload["message"].as_str().unwrap().len() > 4096 {
            (
                500,
                r#"{"code":50001,"http":500,"error":"internal server error"}"#.into(),
            )
        } else {
            (
                200,
                include_str!("../testdata/ntfy/received-calendar-summary.json").into(),
            )
        }
    }))
}

fn assert_calendar_summary(payload: &serde_json::Value, out: &str) {
    let title = payload["title"].as_str().unwrap();
    let after = out.split(&format!("【提醒】{title}\n")).nth(1).unwrap();
    let full: Vec<&str> = after
        .lines()
        .take_while(|line| line.starts_with("  "))
        .map(|line| line.strip_prefix("  ").unwrap())
        .collect();
    assert_eq!(full.len(), 110, "terminal must retain all 108 tiers");
    assert!(
        full.join("\n").len() > 4096,
        "fixture no longer reproduces F1"
    );
    let body = payload["message"].as_str().unwrap();
    assert!(
        body.len() <= 4096,
        "ntfy body has {} bytes, limit 4096",
        body.len()
    );
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines[0], full[0], "event name lost");
    assert_eq!(
        lines.last(),
        full.last(),
        "time or opening confirmation lost"
    );
    let kept = lines.len() - 3; // name, omission summary, time
    assert!(kept > 0 && kept < 108);
    assert_eq!(
        &lines[1..1 + kept],
        &full[1..1 + kept],
        "tier lines must stay complete"
    );
    assert_eq!(
        lines[1 + kept],
        format!("……另有 {} 个票档，详见终端", 108 - kept)
    );
}

#[test]
fn long_calendar_push_fits_ntfy_and_retains_the_full_terminal_alert() {
    let damai = calendar_damai(false);
    let push = size_limited_push();
    let mut run = Running::start_args(&damai, &push, Some(config()), 0o600, &["1084537866279"]);
    push.wait_for(1, 20);
    let out = run.stop();
    let requests = push.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let payload: serde_json::Value = serde_json::from_str(&requests[0].2).unwrap();
    assert_eq!(payload["title"], "现在就可以买");
    assert_calendar_summary(&payload, &out);
    assert!(!out.contains("手机推送失败"));
    assert_no_secrets(&out);
}

#[test]
fn long_calendar_opening_pushes_keep_due_and_confirmed_states() {
    let damai = calendar_damai(true);
    let push = size_limited_push();
    let mut run = Running::start_args(&damai, &push, Some(config()), 0o600, &["1084537866279"]);
    push.wait_for(2, 40);
    let out = run.stop();
    let requests = push.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for (request, title) in requests.iter().zip(["开售时间到", "已经可以买了"]) {
        let payload: serde_json::Value = serde_json::from_str(&request.2).unwrap();
        assert_eq!(payload["title"], title);
        assert_calendar_summary(&payload, &out);
        let body = payload["message"].as_str().unwrap();
        assert_eq!(body.contains("正在确认能不能买"), title == "开售时间到");
        assert_eq!(body.contains("可售"), title == "已经可以买了");
    }
    assert!(!out.contains("手机推送失败"));
    assert_no_secrets(&out);
}

#[test]
fn topic_length_is_checked_at_startup_and_64_characters_can_publish() {
    for length in [64, 65, 128, 129, 0] {
        let damai = damai(vec![true]);
        let push = size_limited_push();
        let mut cfg = config();
        let topic = "x".repeat(length);
        cfg["topic"] = topic.clone().into();
        let mut run = Running::start(&damai, &push, Some(cfg), 0o600);
        run.wait_text("时间：");
        if length == 64 {
            push.wait_for(1, 10);
        }
        let out = run.stop();
        assert_eq!(
            out.contains("topic 格式无效"),
            length != 64,
            "topic of {length} characters must be validated at startup"
        );
        assert_eq!(push.count(), usize::from(length == 64));
        if length > 0 {
            assert!(!out.contains(&topic));
        }
        assert!(out.contains("【提醒】现在就可以买"));
        assert_no_secrets(&out);
    }
}

#[test]
fn xdg_config_takes_precedence_and_relative_empty_and_missing_paths_are_handled() {
    for case in ["absolute", "relative", "empty", "missing"] {
        let damai = damai(vec![true]);
        let push = size_limited_push();
        let url = push.url();
        let mut run = Running::start_configured(
            &damai,
            &push,
            Some(config()),
            0o600,
            &["1086791513717", "--session", "10-15", "--tier", "C-178"],
            |home, command| match case {
                "absolute" => {
                    let dir = home.join("alternate/theflash");
                    std::fs::create_dir_all(&dir).unwrap();
                    let path = dir.join("push.json");
                    let mut cfg = config();
                    cfg["server"] = url.into();
                    cfg["topic"] = "xdg-test-placeholder".into();
                    std::fs::write(&path, cfg.to_string()).unwrap();
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                        .unwrap();
                    command.env("XDG_CONFIG_HOME", home.join("alternate"));
                }
                "relative" => {
                    command.env("XDG_CONFIG_HOME", "relative-path");
                }
                "empty" => {
                    command.env("XDG_CONFIG_HOME", "");
                }
                "missing" => {
                    command.env("XDG_CONFIG_HOME", home.join("missing"));
                }
                _ => unreachable!(),
            },
        );
        run.wait_text("时间：");
        if case != "missing" {
            push.wait_for(1, 10);
        }
        let out = run.stop();
        assert_no_secrets(&out);
        assert!(!out.contains("xdg-test-placeholder"));
        let requests = push.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            usize::from(case != "missing"),
            "XDG case {case}"
        );
        if let Some(request) = requests.first() {
            let payload: serde_json::Value = serde_json::from_str(&request.2).unwrap();
            assert_eq!(
                payload["topic"],
                if case == "absolute" {
                    "xdg-test-placeholder"
                } else {
                    "test-topic-placeholder"
                },
                "XDG case {case}"
            );
        }
        assert!(!out.contains("手机推送未启用"));
    }
}

#[test]
fn a_hanging_ntfy_request_times_out_and_the_next_alert_can_publish() {
    let damai = damai(vec![true, false, true]);
    let mut count = 0;
    let push = Server::start(Box::new(move |_, _| {
        count += 1;
        if count == 1 {
            // Keep the connection open beyond the 3-second client timeout.
            // With the timeout removed it eventually succeeds, so the test
            // fails on the missing error, not on a deadlocked test server.
            std::thread::sleep(Duration::from_secs(6));
        }
        (200, include_str!("../testdata/ntfy/received.json").into())
    }));
    let mut run = Running::start(&damai, &push, Some(config()), 0o600);
    push.wait_for(2, 40);
    run.wait_text("【提醒】有票了");
    let out = run.stop();
    assert!(
        out.contains("网络失败或超时"),
        "hanging ntfy request never reported a timeout"
    );
    assert!(out.contains("又没票了"));
    assert_eq!(
        push.count(),
        2,
        "timeout must not disable later delivery or retry the failed alert"
    );
    assert_eq!(out.matches("手机推送失败").count(), 1);
    assert_no_secrets(&out);
}
