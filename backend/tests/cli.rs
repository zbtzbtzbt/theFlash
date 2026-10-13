//! End-to-end checks of `theflash event`: the built binary against a local
//! server that replays real damai.cn responses from testdata/damai. No test
//! reaches the real damai.cn.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// A stand-in for detail.damai.cn. Each route is a substring of the request
/// target and the fixture served for it; anything else gets a 404. The
/// server stops and its thread is joined when the value is dropped.
struct MockDamai {
    addr: SocketAddr,
    hits: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MockDamai {
    fn start(routes: &[(&'static str, &'static str)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let routes: Vec<(String, String)> = routes
            .iter()
            .map(|(m, f)| (m.to_string(), fixture(f)))
            .collect();
        let thread = {
            let hits = hits.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    serve(stream, &routes, &hits);
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

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn hits(&self) -> Vec<String> {
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

fn serve(mut stream: TcpStream, routes: &[(String, String)], hits: &Mutex<Vec<String>>) {
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
    let target = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();
    hits.lock().unwrap().push(target.clone());
    let (status, body) = match routes.iter().find(|(m, _)| target.contains(m.as_str())) {
        Some((_, body)) => ("200 OK", body.as_str()),
        None => ("404 Not Found", "not found"),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn fixture(name: &str) -> String {
    let path = format!("{}/testdata/damai/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn theflash(base_url: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_theflash"))
        .args(args)
        .env("THEFLASH_DAMAI_BASE_URL", base_url)
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("HTTPS_PROXY")
        .env_remove("https_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

// What detail.damai.cn showed for item 1086791513717 on 2026-10-14 00:50
// +0800, read off the page in a browser: sessions in date order, and each
// session's tiers with 缺货登记 where the page marked it.
#[test]
fn on_sale_event_lists_every_session_and_tier_as_the_page_shows_them() {
    let damai = MockDamai::start(&[
        ("/item.htm?id=1086791513717", "item_on_sale.html"),
        ("dataType=2&dataId=286937789", "sku_on_sale_286937789.jsonp"),
        ("dataType=2&dataId=286937790", "sku_on_sale_286937790.jsonp"),
        ("dataType=2&dataId=286937788", "sku_on_sale_286937788.jsonp"),
    ]);
    let out = theflash(
        &damai.base_url(),
        &["event", "https://detail.damai.cn/item.htm?id=1086791513717"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let want = "\
演出：【北京】2026德玛西亚杯国际邀请赛
城市：北京市
场馆：北京智慧电竞赛事中心
售卖状态：在售

场次：2026-10-14 周三 17:00
  D-118元 · ¥118.00 · 可售
  C-178元 · ¥178.00 · 可售
  B-238元 · ¥238.00 · 可售
  A-288元 · ¥288.00 · 可售
  SVIP-498元 · ¥498.00 · 缺货

场次：2026-10-15 周四 17:00
  D-118元 · ¥118.00 · 缺货
  C-178元 · ¥178.00 · 可售
  B-238元 · ¥238.00 · 可售
  A-288元 · ¥288.00 · 可售
  SVIP-498元 · ¥498.00 · 缺货

场次：2026-10-17 周六 17:00
  D-198元 · ¥198.00 · 缺货
  C-268元 · ¥268.00 · 可售
  B-328元 · ¥328.00 · 可售
  A-398元 · ¥398.00 · 可售
  SVIP-598元 · ¥598.00 · 缺货
";
    assert_eq!(stdout(&out), want);
}

// Item 1092337348536 before its sale opened: the page showed a countdown to
// 10月14日 10:18 and every tier as 可预约, though the tier fields read like
// a tier on sale (skuSalable true, frontEndStatus 1).
#[test]
fn not_yet_on_sale_event_shows_its_sale_time_and_no_tier_as_available() {
    let damai = MockDamai::start(&[
        ("/item.htm?id=1092337348536", "item_not_yet_on_sale.html"),
        (
            "dataType=2&dataId=286966465",
            "sku_not_yet_on_sale_286966465.jsonp",
        ),
    ]);
    let out = theflash(&damai.base_url(), &["event", "1092337348536"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let got = stdout(&out);
    assert!(got.contains("售卖状态：未开售\n"), "{got}");
    assert!(
        got.contains("开售时间：2026-10-14 10:18（北京时间）\n"),
        "{got}"
    );
    // damai.cn's data spells it "2027-01-10  周日 15:00"; the page shows
    // one space, as HTML collapses the run.
    assert!(got.contains("场次：2027-01-10 周日 15:00\n"), "{got}");
    assert!(
        got.contains("  单人早鸟126元（原价180元） · ¥126.00 · 未开售\n"),
        "{got}"
    );
    assert!(got.contains("  480元 · ¥480.00 · 未开售\n"), "{got}");
    assert!(
        !got.contains("可售"),
        "a tier before the sale reads as 可售:\n{got}"
    );
}

// Item 1084537866279 picks the day on a calendar: 18 sessions from 10-16 to
// 11-08. On 2026-10-14 the page showed 10-17 14:30's tiers as V, L and I
// 缺货登记, F, D and C to pick. Only that session's tiers are served here.
#[test]
fn calendar_event_lists_every_session() {
    let damai = MockDamai::start(&[
        ("/item.htm?id=1084537866279", "item_calendar.html"),
        (
            "dataType=2&dataId=286938129",
            "sku_calendar_286938129.jsonp",
        ),
    ]);
    let out = theflash(&damai.base_url(), &["event", "1084537866279"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let got = stdout(&out);
    assert_eq!(got.matches("\n场次：").count(), 18, "{got}");
    assert!(!got.contains("没有场次信息"), "{got}");
    assert!(
        got.contains(
            "场次：2026-10-17 星期六 14:30
  V（180元） · ¥180.00 · 缺货
  L（180元） · ¥180.00 · 缺货
  I（280元） · ¥280.00 · 缺货
  F（380元） · ¥380.00 · 可售
  D（480元） · ¥480.00 · 可售
  C（580元） · ¥580.00 · 可售
"
        ),
        "{got}"
    );
    assert_eq!(
        damai.hits().len(),
        19,
        "one page and one tier request per session"
    );
}

// Without dataType=2 damai.cn answers with its default session, whatever
// dataId asks for. Tiers of another session must not be shown as this one's.
#[test]
fn tiers_answered_for_another_session_are_not_shown_as_this_ones() {
    let damai = MockDamai::start(&[
        ("/item.htm?id=1086791513717", "item_on_sale.html"),
        ("dataType=2&dataId=286937789", "sku_on_sale_286937789.jsonp"),
        ("dataType=2&dataId=286937790", "sku_on_sale_286937789.jsonp"),
        ("dataType=2&dataId=286937788", "sku_on_sale_286937788.jsonp"),
    ]);
    let out = theflash(&damai.base_url(), &["event", "1086791513717"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let got = stdout(&out);
    assert!(
        got.contains("场次：2026-10-15 周四 17:00\n  票档：获取失败（接口返回的是另一个场次"),
        "{got}"
    );
    assert!(got.contains("  SVIP-598元 · ¥598.00 · 缺货\n"), "{got}");
}

#[test]
fn sold_out_event_shows_every_tier_sold_out() {
    let damai = MockDamai::start(&[
        ("/item.htm?id=1077137031347", "item_sold_out.html"),
        (
            "dataType=2&dataId=284490009",
            "sku_sold_out_284490009.jsonp",
        ),
    ]);
    let out = theflash(&damai.base_url(), &["event", "1077137031347"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let got = stdout(&out);
    assert!(got.contains("售卖状态：已售罄\n"), "{got}");
    assert!(got.contains("场次：2026-10-17 周六 13:00\n"), "{got}");
    assert!(got.contains("  158元 · ¥158.00 · 已售罄\n"), "{got}");
    assert!(
        got.contains("  1大1小亲子票237元（原价158*2） · ¥237.00 · 已售罄\n"),
        "{got}"
    );
}

#[test]
fn event_the_pc_site_does_not_sell_says_so_instead_of_listing_nothing() {
    let damai = MockDamai::start(&[("/item.htm?id=1080486727522", "item_pc_unsupported.html")]);
    let out = theflash(&damai.base_url(), &["event", "1080486727522"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let got = stdout(&out);
    assert!(
        got.contains("演出：【北京】2026动力火车“一路向前”演唱会-北京站\n"),
        "{got}"
    );
    assert!(got.contains("售卖状态：电脑版不支持购票\n"), "{got}");
    assert!(
        got.contains("场次和票档：大麦电脑版不展示这场演出的场次和票档"),
        "{got}"
    );
    assert_eq!(damai.hits().len(), 1, "requests: {:?}", damai.hits());
}

#[test]
fn missing_event_is_reported_as_missing() {
    let damai = MockDamai::start(&[("/item.htm?id=99999999999999", "item_not_found.html")]);
    let out = theflash(&damai.base_url(), &["event", "99999999999999"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("演出 99999999999999 不存在或已下架"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn input_without_an_event_id_is_refused_before_any_request() {
    let damai = MockDamai::start(&[]);
    for input in ["abc", "https://detail.damai.cn/item.htm", ""] {
        let out = theflash(&damai.base_url(), &["event", input]);
        assert_eq!(out.status.code(), Some(2), "input {input:?}");
        assert!(
            stderr(&out).contains("找不到演出 ID"),
            "input {input:?}: {}",
            stderr(&out)
        );
    }
    assert!(damai.hits().is_empty(), "requests: {:?}", damai.hits());
}

#[test]
fn network_failure_is_explained() {
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let out = theflash(&format!("http://{closed}"), &["event", "1086791513717"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("网络请求失败"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn risk_control_on_the_page_stops_after_one_request() {
    let damai = MockDamai::start(&[("/item.htm?id=1086791513717", "risk_control.html")]);
    let out = theflash(&damai.base_url(), &["event", "1086791513717"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("风控"), "stderr: {}", stderr(&out));
    assert_eq!(damai.hits().len(), 1, "requests: {:?}", damai.hits());
}

#[test]
fn risk_control_on_a_tier_request_stops_the_rest() {
    let damai = MockDamai::start(&[
        ("/item.htm?id=1086791513717", "item_on_sale.html"),
        ("dataId=", "risk_control.html"),
    ]);
    let out = theflash(&damai.base_url(), &["event", "1086791513717"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("风控"), "stderr: {}", stderr(&out));
    assert_eq!(damai.hits().len(), 2, "requests: {:?}", damai.hits());
}
