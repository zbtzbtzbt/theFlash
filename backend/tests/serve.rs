//! End-to-end checks of `theflash serve`: the standalone Rust HTTP server
//! serving the embedded browser UI and `/api/event` in-process (#26).
//! Every process starts through `support::theflash` or `support::Home` so
//! the offline tripwire and temporary HOME guards hold.

mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use support::{MockDamai, fixture};

struct RunningServer {
    child: Child,
    addr: SocketAddr,
    base: String,
}

impl RunningServer {
    fn start(mut cmd: Command) -> Self {
        cmd.args(["serve", "--no-open"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn theflash serve");
        let stdout = child.stdout.take().expect("stdout piped");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let _ = tx.send(line);
            // Keep draining stdout so the child never blocks on a full pipe.
            let mut sink = [0u8; 256];
            while let Ok(n) = reader.read(&mut sink) {
                if n == 0 {
                    break;
                }
            }
        });
        let line = match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(l) if !l.is_empty() => l,
            _ => {
                let _ = child.kill();
                let out = child.wait_with_output().unwrap();
                panic!(
                    "theflash serve did not print its loopback URL (exit {:?}):\nstdout: {}\nstderr: {}",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr),
                );
            }
        };
        let url = line
            .trim()
            .strip_prefix("theFlash 演出查询：")
            .unwrap_or_else(|| panic!("unexpected first stdout line from serve: {line:?}"))
            .to_string();
        let host_port = url
            .strip_prefix("http://")
            .unwrap_or_else(|| panic!("expected http:// URL, got {url:?}"));
        let addr: SocketAddr = host_port
            .parse()
            .unwrap_or_else(|e| panic!("invalid SocketAddr {host_port:?}: {e}"));
        RunningServer {
            child,
            addr,
            base: url,
        }
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("invalid JSON body ({e}): {:?}", self.body))
    }
}

fn raw_http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> HttpResponse {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw);
    let (head, rest) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("malformed HTTP response: {text:?}"));
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("bad status line: {status_line:?}"))
        .parse()
        .unwrap();
    let parsed_headers = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    HttpResponse {
        status,
        headers: parsed_headers,
        body: rest.to_string(),
    }
}

fn get(srv: &RunningServer, path: &str) -> HttpResponse {
    let host = format!("127.0.0.1:{}", srv.addr.port());
    raw_http(srv.addr, "GET", path, &[("Host", &host)], "")
}

fn post_event(srv: &RunningServer, input: &str) -> HttpResponse {
    let host = format!("127.0.0.1:{}", srv.addr.port());
    let body = serde_json::json!({ "input": input }).to_string();
    raw_http(
        srv.addr,
        "POST",
        "/api/event",
        &[
            ("Host", &host),
            ("Origin", &srv.base),
            ("Content-Type", "application/json"),
        ],
        &body,
    )
}

#[test]
fn serve_binds_loopback_and_serves_embedded_html_css_and_compiled_js() {
    let damai = MockDamai::start(&[]);
    let srv = RunningServer::start(support::theflash(&damai.base_url()));
    assert_eq!(srv.addr.ip().to_string(), "127.0.0.1");

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let want_html = std::fs::read_to_string(repo_root.join("frontend/public/index.html")).unwrap();
    let want_css = std::fs::read_to_string(repo_root.join("frontend/public/style.css")).unwrap();

    for (path, want_type) in [
        ("/", "text/html; charset=utf-8"),
        ("/style.css", "text/css; charset=utf-8"),
        ("/main.js", "text/javascript; charset=utf-8"),
        ("/app.js", "text/javascript; charset=utf-8"),
    ] {
        let res = get(&srv, path);
        assert_eq!(res.status, 200, "{path}: {}", res.body);
        assert_eq!(res.header("content-type"), Some(want_type), "{path}");
        assert_eq!(res.header("cache-control"), Some("no-store"), "{path}");
        assert_eq!(
            res.header("x-content-type-options"),
            Some("nosniff"),
            "{path}"
        );
        assert!(
            res.header("content-security-policy")
                .unwrap_or_default()
                .contains("frame-ancestors 'none'"),
            "{path}: missing CSP"
        );
        if path == "/" {
            assert_eq!(res.body, want_html);
        } else if path == "/style.css" {
            assert_eq!(res.body, want_css);
        } else {
            // Must be valid compiled JavaScript, not raw TypeScript with type syntax.
            for ts_only in [
                "import type",
                "querySelector<HTMLElement>",
                ": HTMLElement",
                ": EventView",
                "text?: string",
            ] {
                assert!(
                    !res.body.contains(ts_only),
                    "{path} still contains TypeScript syntax {ts_only:?}:\n{}",
                    res.body
                );
            }
        }
    }

    // Host header without port on loopback is also accepted.
    let res = raw_http(srv.addr, "GET", "/", &[("Host", "localhost")], "");
    assert_eq!(res.status, 200);

    // Unknown route or GET on /api/event returns 404 without touching upstream.
    let res = get(&srv, "/api/event?input=1086791513717");
    assert_eq!(res.status, 404);
    assert_eq!(res.json()["error"]["code"], "not-found");
    assert!(damai.hits().is_empty());
}

#[test]
fn serve_rejects_foreign_hosts_and_cross_site_origins_before_any_upstream_request() {
    let damai = MockDamai::start(&[
        ("/item.htm?id=1086791513717", "item_on_sale.html"),
        ("dataType=2&dataId=286937789", "sku_on_sale_286937789.jsonp"),
    ]);
    let srv = RunningServer::start(support::theflash(&damai.base_url()));
    let port = srv.addr.port();
    let host = format!("127.0.0.1:{port}");
    let evil_port = format!("evil.example:{port}");
    let evil_origin_port = format!("http://evil.example:{port}");
    let body = r#"{"input":"1086791513717"}"#;

    let get_bad_host = raw_http(srv.addr, "GET", "/", &[("Host", "evil.example")], "");
    assert_eq!(get_bad_host.status, 403);
    assert_eq!(get_bad_host.json()["error"]["code"], "forbidden");

    let cases: &[&[(&str, &str)]] = &[
        &[
            ("Host", &host),
            ("Origin", "https://example.com"),
            ("Content-Type", "application/json"),
        ],
        &[
            ("Host", "evil.example"),
            ("Origin", "http://evil.example"),
            ("Content-Type", "application/json"),
        ],
        &[
            ("Host", &evil_port),
            ("Origin", &evil_origin_port),
            ("Content-Type", "application/json"),
        ],
        &[
            ("Host", "evil.example"),
            ("Origin", &srv.base),
            ("Content-Type", "application/json"),
        ],
        &[("Host", &host), ("Content-Type", "application/json")],
        &[
            ("Host", &host),
            ("Origin", ""),
            ("Content-Type", "application/json"),
        ],
        &[
            ("Host", &host),
            ("Origin", &srv.base),
            ("Content-Type", "text/plain"),
        ],
        &[
            ("Host", &host),
            ("Origin", &srv.base),
            ("Content-Type", "application/json"),
            ("Sec-Fetch-Site", "cross-site"),
        ],
    ];

    for headers in cases {
        let res = raw_http(srv.addr, "POST", "/api/event", headers, body);
        assert_eq!(res.status, 403, "headers {headers:?}: {}", res.body);
        assert_eq!(res.json()["error"]["code"], "forbidden");
    }
    assert!(damai.hits().is_empty(), "requests: {:?}", damai.hits());
}

#[test]
fn serve_api_event_matches_cli_and_maps_error_codes_and_breaker() {
    let damai = MockDamai::start(&[
        ("/item.htm?id=1086791513717", "item_on_sale.html"),
        ("dataType=2&dataId=286937789", "sku_on_sale_286937789.jsonp"),
        ("dataType=2&dataId=286937790", "sku_on_sale_286937790.jsonp"),
        ("dataType=2&dataId=286937788", "sku_on_sale_286937788.jsonp"),
        ("/item.htm?id=99999999999999", "item_not_found.html"),
    ]);
    let home = support::Home::new("serve-api");
    let srv = RunningServer::start(home.theflash(&damai.base_url()));

    // 200 OK matches CLI report
    let ok = post_event(&srv, "1086791513717");
    assert_eq!(ok.status, 200, "{}", ok.body);
    let data = ok.json();
    let cli = home
        .theflash(&damai.base_url())
        .args(["event", "1086791513717"])
        .output()
        .unwrap();
    assert!(cli.status.success());
    assert_eq!(
        data["report"].as_str(),
        Some(String::from_utf8_lossy(&cli.stdout).as_ref())
    );
    assert_eq!(data["sessions"].as_array().unwrap().len(), 3);

    // 400 invalid input (bad link, empty input, oversized body)
    let bad = post_event(&srv, "https://www.damai.cn/");
    assert_eq!(bad.status, 400);
    assert_eq!(bad.json()["error"]["code"], "invalid-input");
    assert!(
        bad.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("找不到演出 ID")
    );

    let empty = post_event(&srv, "   ");
    assert_eq!(empty.status, 400);
    assert_eq!(empty.json()["error"]["code"], "invalid-input");

    let huge = post_event(&srv, &"x".repeat(10_000));
    assert_eq!(huge.status, 400);
    assert_eq!(huge.json()["error"]["code"], "invalid-input");

    // 404 not found
    let missing = post_event(&srv, "99999999999999");
    assert_eq!(missing.status, 404);
    assert_eq!(missing.json()["error"]["code"], "not-found");
    assert!(
        missing.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("不存在或已下架")
    );

    // 500 unreadable risk-stop.json
    std::fs::create_dir_all(home.state_dir()).unwrap();
    std::fs::write(home.risk_stop_file(), "{").unwrap();
    let corrupt = post_event(&srv, "1086791513717");
    assert_eq!(corrupt.status, 500);
    assert_eq!(corrupt.json()["error"]["code"], "risk-state");
    std::fs::remove_file(home.risk_stop_file()).unwrap();

    // 500 invalid-config when THEFLASH_REQUEST_GAP_MS is invalid
    let bad_cfg_home = support::Home::new("serve-bad-cfg");
    let mut bad_cfg_cmd = bad_cfg_home.theflash(&damai.base_url());
    bad_cfg_cmd.env("THEFLASH_REQUEST_GAP_MS", "bad");
    let bad_cfg_srv = RunningServer::start(bad_cfg_cmd);
    let bad_cfg = post_event(&bad_cfg_srv, "1086791513717");
    assert_eq!(bad_cfg.status, 500);
    assert_eq!(bad_cfg.json()["error"]["code"], "invalid-config");
    assert!(
        bad_cfg.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("THEFLASH_REQUEST_GAP_MS")
    );

    #[cfg(target_os = "linux")]
    {
        let no_ca_home = support::Home::new("serve-no-ca");
        let mut no_ca_cmd = no_ca_home.theflash(&damai.base_url());
        no_ca_cmd
            .env("SSL_CERT_FILE", "/dev/null")
            .env_remove("SSL_CERT_DIR");
        let no_ca_srv = RunningServer::start(no_ca_cmd);
        let no_ca = post_event(&no_ca_srv, "1086791513717");
        assert_eq!(no_ca.status, 500);
        assert_eq!(no_ca.json()["error"]["code"], "setup");
        assert!(
            no_ca.json()["error"]["message"]
                .as_str()
                .unwrap()
                .contains("没有 CA 证书")
        );
    }

    // Invalid --port flag exits with usage code 2.
    let bad_flag = support::output_within(
        {
            let mut c = support::theflash(&damai.base_url());
            c.args(["serve", "--port", "not-a-port"]);
            c
        },
        5,
    );
    assert_eq!(bad_flag.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad_flag.stderr).contains("theflash serve"));

    // 502 network failure when upstream is closed
    let closed_addr = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let net_home = support::Home::new("serve-net-err");
    let net_srv = RunningServer::start(net_home.theflash(&format!("http://{closed_addr}")));
    let net_err = post_event(&net_srv, "1086791513717");
    assert_eq!(net_err.status, 502);
    assert_eq!(net_err.json()["error"]["code"], "network");
    assert!(
        net_err.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("网络请求失败")
    );

    // 403 risk-control trips the shared breaker, and subsequent queries stop with risk-stopped
    let risk_damai = MockDamai::start(&[("/item.htm?id=1086791513717", "risk_control.html")]);
    let risk_home = support::Home::new("serve-risk");
    let risk_srv = RunningServer::start(risk_home.theflash(&risk_damai.base_url()));
    let first = post_event(&risk_srv, "1086791513717");
    assert_eq!(first.status, 403);
    assert_eq!(first.json()["error"]["code"], "risk-control");
    assert!(risk_home.risk_stop_file().exists());

    let second = post_event(&risk_srv, "1086791513717");
    assert_eq!(second.status, 403);
    assert_eq!(second.json()["error"]["code"], "risk-stopped");
    assert!(
        second.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("theflash risk resume --yes")
    );
    assert_eq!(risk_damai.hits().len(), 1);
}

#[test]
fn serve_rejects_concurrent_query_with_409_and_cancels_when_client_disconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let hits = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (tier_entered_tx, tier_entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let stop = Arc::new(AtomicBool::new(false));
    let release_rx = Arc::new(std::sync::Mutex::new(release_rx));

    let upstream_thread = {
        let hits = hits.clone();
        let stop = stop.clone();
        let release_rx = release_rx.clone();
        std::thread::spawn(move || {
            let item_html = fixture("item_on_sale.html");
            let sku_body = fixture("sku_on_sale_286937789.jsonp");
            for stream in listener.incoming().flatten() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut req_line = String::new();
                if reader.read_line(&mut req_line).is_err() {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if line == "\r\n" => break,
                        Ok(_) => {}
                    }
                }
                let target = req_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                hits.lock().unwrap().push(target.clone());
                let body = if target.contains("/item.htm") {
                    item_html.as_str()
                } else {
                    let _ = tier_entered_tx.send(());
                    let _ = release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10));
                    sku_body.as_str()
                };
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        })
    };

    let home = support::Home::new("serve-concurrency");
    let srv = RunningServer::start(home.theflash(&format!("http://{upstream_addr}")));
    let host = format!("127.0.0.1:{}", srv.addr.port());
    let body = r#"{"input":"1086791513717"}"#;

    // Start first query over a raw socket so we can disconnect it while held.
    let mut first_conn = TcpStream::connect(srv.addr).unwrap();
    write!(
        first_conn,
        "POST /api/event HTTP/1.1\r\nHost: {host}\r\nOrigin: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        srv.base,
        body.len()
    )
    .unwrap();

    // Wait until the server is holding inside the first tier request.
    tier_entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("first tier request should reach upstream");
    assert_eq!(hits.lock().unwrap().len(), 2);

    // Concurrent second query must immediately receive 409 busy.
    let busy_res = post_event(&srv, "1086791513717");
    assert_eq!(busy_res.status, 409, "{}", busy_res.body);
    assert_eq!(busy_res.json()["error"]["code"], "busy");
    assert!(
        busy_res.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("上一场还在查询中，请稍候")
    );

    // Disconnect the first client while the first tier response is still gated.
    drop(first_conn);

    // Wait briefly for the server to observe EOF on the disconnected socket,
    // then release the held upstream response and verify no further tier requests are made.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        // Probe with an invalid input: once the disconnected query has released the busy lock,
        // this returns 400 instead of 409.
        let probe = post_event(&srv, "not-an-id");
        if probe.status == 400 {
            break;
        }
    }
    let _ = release_tx.send(());
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        hits.lock().unwrap().len(),
        2,
        "disconnected query must not issue later tier requests"
    );

    stop.store(true, Ordering::SeqCst);
    let _ = TcpStream::connect(upstream_addr);
    let _ = upstream_thread.join();
}
