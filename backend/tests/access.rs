//! End-to-end checks of the layer every damai.cn request goes through
//! (issue #4, docs/decisions/0005-damai-access-gate.md): risk control seen
//! by one theflash process stops the others, only `theflash risk resume
//! --yes` clears it, and a breaker that cannot be read stops queries
//! instead of passing for "no risk control".
//!
//! Each test gets a HOME of its own and talks to a local server replaying
//! saved damai.cn responses. The risk-control path is only ever the saved
//! `risk_control.html`; no test reaches the real damai.cn, and nothing here
//! provokes real risk control.

mod support;

use std::process::{Command, Output};
use std::sync::Arc;
use std::time::Duration;

use support::{Home, MockDamai, Reply};
use theflash::damai::{Client, Error, Gate, MIN_REQUEST_GAP, SystemClock};

const ITEM: u64 = 1086791513717;
const SESSION: u64 = 286937789;

fn out(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn run(home: &Home, base_url: &str, args: &[&str]) -> Output {
    support::output_within(
        {
            let mut c = home.theflash(base_url);
            c.args(args);
            c
        },
        30,
    )
}

/// The saved slider challenge, served for the event's page.
fn risk_control() -> MockDamai {
    MockDamai::start(&[("/item.htm?id=1086791513717", "risk_control.html")])
}

/// Three sessions, all answered normally.
fn on_sale() -> MockDamai {
    MockDamai::start(&[
        ("/item.htm?id=1086791513717", "item_on_sale.html"),
        ("dataType=2&dataId=286937789", "sku_on_sale_286937789.jsonp"),
        ("dataType=2&dataId=286937790", "sku_on_sale_286937790.jsonp"),
        ("dataType=2&dataId=286937788", "sku_on_sale_286937788.jsonp"),
    ])
}

#[test]
fn risk_control_in_one_process_stops_the_next_process_before_it_asks() {
    let home = Home::new("cross-process");
    let tripped = {
        let damai = risk_control();
        let o = run(&home, &damai.base_url(), &["event", "1086791513717"]);
        assert!(!o.status.success());
        assert_eq!(damai.hits().len(), 1, "{:?}", damai.hits());
        o
    };
    assert!(out(&tripped).contains("风控"), "{}", out(&tripped));
    assert!(
        home.risk_stop_file().is_file(),
        "熔断状态没有写进 {}",
        home.risk_stop_file().display()
    );

    // A second process, pointed at a damai that answers normally.
    let damai = on_sale();
    let stopped = run(&home, &damai.base_url(), &["event", "1086791513717"]);
    assert!(!stopped.status.success());
    let text = out(&stopped);
    assert!(text.contains("风控熔断生效中"), "{text}");
    assert!(text.contains("这次请求没有发出"), "{text}");
    assert!(text.contains("theflash risk resume --yes"), "{text}");
    assert!(
        damai.hits().is_empty(),
        "熔断期间还在请求大麦：{:?}",
        damai.hits()
    );

    // So is a watch, which polls for as long as it runs.
    let watch = run(
        &home,
        &damai.base_url(),
        &["watch", "1086791513717", "--no-system-notify"],
    );
    assert!(!watch.status.success());
    assert!(
        out(&watch).contains("风控熔断生效，已停止盯票"),
        "{}",
        out(&watch)
    );
    assert!(
        damai.hits().is_empty(),
        "熔断期间盯票还在请求大麦：{:?}",
        damai.hits()
    );

    // And so is the browser UI's query command.
    let json = run(&home, &damai.base_url(), &["event-json", "1086791513717"]);
    let value: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&json.stdout))
        .unwrap_or_else(|e| panic!("{e}: {}", out(&json)));
    assert_eq!(value["error"]["code"], "risk-stopped");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("theflash risk resume --yes"),
        "{value}"
    );
    assert!(damai.hits().is_empty(), "{:?}", damai.hits());
}

#[test]
fn the_breaker_says_what_happened_and_only_resume_yes_clears_it() {
    let home = Home::new("resume");
    let risky = risk_control();
    run(&home, &risky.base_url(), &["event", "1086791513717"]);

    let shown = run(&home, &risky.base_url(), &["risk"]);
    let text = out(&shown);
    assert_eq!(
        shown.status.code(),
        Some(1),
        "熔断生效时 `theflash risk` 要用非 0 退出码：{text}"
    );
    assert!(text.contains("大麦风控熔断生效中"), "{text}");
    assert!(text.contains("（北京时间）"), "{text}");
    assert!(text.contains("/item.htm?id=1086791513717"), "{text}");
    assert!(
        text.contains("_____tmd_____"),
        "命中的页面标记没写出来：{text}"
    );
    assert!(text.contains("theflash event"), "{text}");

    // Without --yes nothing is cleared: the owner has to have seen it.
    let asked = run(&home, &risky.base_url(), &["risk", "resume"]);
    assert_eq!(asked.status.code(), Some(2), "{}", out(&asked));
    assert!(out(&asked).contains("没有解除"), "{}", out(&asked));
    assert!(out(&asked).contains("owner"), "{}", out(&asked));
    assert!(home.risk_stop_file().is_file(), "没加 --yes 就把熔断解除了");

    let cleared = run(&home, &risky.base_url(), &["risk", "resume", "--yes"]);
    assert!(cleared.status.success(), "{}", out(&cleared));
    assert!(
        out(&cleared).contains("已解除这次熔断"),
        "{}",
        out(&cleared)
    );
    assert!(out(&cleared).contains("owner"), "{}", out(&cleared));
    assert!(!home.risk_stop_file().exists(), "解除后熔断状态还在");

    let after = run(&home, &risky.base_url(), &["risk"]);
    assert!(after.status.success(), "{}", out(&after));
    assert!(out(&after).contains("没有大麦风控熔断"), "{}", out(&after));

    // Queries work again.
    let damai = on_sale();
    let ok = run(&home, &damai.base_url(), &["event", "1086791513717"]);
    assert!(ok.status.success(), "{}", out(&ok));
    assert!(out(&ok).contains("售卖状态：在售"), "{}", out(&ok));
    assert_eq!(damai.hits().len(), 4, "{:?}", damai.hits());
}

// #83: a breaker shows up in the environment the unattended validation
// runs in, which is the one that sets THEFLASH_REQUEST_GAP_MS (0009). The
// risk commands only read and clear this machine's state, so a mistyped
// value there must not hide a breaker that is in effect or keep the owner
// from clearing it. The queries that do send requests still refuse, since
// they are the ones the gap would pace.
#[test]
fn a_mistyped_request_gap_still_shows_the_breaker_and_clears_it() {
    let home = Home::new("bad-gap");
    let risky = risk_control();
    let tripped = run(&home, &risky.base_url(), &["event", "1086791513717"]);
    assert!(!tripped.status.success(), "{}", out(&tripped));
    assert!(home.risk_stop_file().is_file(), "{}", out(&tripped));

    let with_bad_gap = |args: &[&str]| {
        let mut c = home.theflash(&risky.base_url());
        c.args(args).env("THEFLASH_REQUEST_GAP_MS", "bad");
        support::output_within(c, 30)
    };

    let shown = with_bad_gap(&["risk"]);
    let text = out(&shown);
    assert_eq!(shown.status.code(), Some(1), "{text}");
    assert!(text.contains("大麦风控熔断生效中"), "{text}");
    assert!(text.contains("/item.htm?id=1086791513717"), "{text}");
    assert!(text.contains("_____tmd_____"), "{text}");
    assert!(!text.contains("THEFLASH_REQUEST_GAP_MS"), "{text}");

    let asked = with_bad_gap(&["risk", "resume"]);
    assert_eq!(asked.status.code(), Some(2), "{}", out(&asked));
    assert!(out(&asked).contains("没有解除"), "{}", out(&asked));
    assert!(home.risk_stop_file().is_file(), "没加 --yes 就解除了");

    let cleared = with_bad_gap(&["risk", "resume", "--yes"]);
    assert!(cleared.status.success(), "{}", out(&cleared));
    assert!(
        out(&cleared).contains("已解除这次熔断"),
        "{}",
        out(&cleared)
    );
    assert!(!home.risk_stop_file().exists(), "解除后熔断状态还在");

    // The commands that do send requests are still stopped by the
    // mistyped gap (#51), and nothing asked damai.cn anything after the
    // one request that tripped the breaker.
    let query = with_bad_gap(&["event", "1086791513717"]);
    assert_eq!(query.status.code(), Some(1), "{}", out(&query));
    assert!(
        out(&query).contains("THEFLASH_REQUEST_GAP_MS"),
        "{}",
        out(&query)
    );
    assert_eq!(risky.hits().len(), 1, "{:?}", risky.hits());
}

#[test]
fn a_breaker_file_that_cannot_be_read_stops_queries_and_says_why() {
    for broken in ["", "{", "{\"at\": \"2026-10-14\"}", "not json at all"] {
        // A damai of its own each round: the query after `resume --yes`
        // leaves requests behind, and the next round counts from zero.
        let damai = on_sale();
        let home = Home::new("corrupt");
        std::fs::create_dir_all(home.state_dir()).unwrap();
        std::fs::write(home.risk_stop_file(), broken).unwrap();
        let o = run(&home, &damai.base_url(), &["event", "1086791513717"]);
        let text = out(&o);
        assert!(
            !o.status.success(),
            "内容 {broken:?} 被当成没有熔断：{text}"
        );
        assert!(text.contains("读不到大麦风控熔断状态"), "{text}");
        assert!(
            text.contains(&home.risk_stop_file().display().to_string()),
            "没说是哪个文件：{text}"
        );
        assert!(text.contains("已停止"), "{text}");
        assert!(
            damai.hits().is_empty(),
            "状态文件读不懂还在请求大麦：{:?}",
            damai.hits()
        );

        // Clearing it says the breaker was unreadable instead of "there
        // was none", because queries were stopped by it.
        let cleared = run(&home, &damai.base_url(), &["risk", "resume", "--yes"]);
        assert!(cleared.status.success(), "{}", out(&cleared));
        assert!(
            out(&cleared).contains("已删掉读不懂的熔断状态文件"),
            "{}",
            out(&cleared)
        );
        let ok = run(&home, &damai.base_url(), &["event", "1086791513717"]);
        assert!(ok.status.success(), "{}", out(&ok));
    }
}

#[test]
fn a_state_directory_that_cannot_be_written_stops_queries_and_says_why() {
    let home = Home::new("unwritable");
    // A plain file where the state directory has to go: the directory
    // cannot be created, whoever is running the test.
    let dir = home.state_dir();
    std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
    std::fs::write(&dir, "not a directory").unwrap();
    let damai = on_sale();
    let o = run(&home, &damai.base_url(), &["event", "1086791513717"]);
    let text = out(&o);
    assert!(!o.status.success(), "{text}");
    assert!(text.contains("读不到大麦风控熔断状态"), "{text}");
    assert!(text.contains(&dir.display().to_string()), "{text}");
    assert!(text.contains("已停止"), "{text}");
    assert!(
        damai.hits().is_empty(),
        "状态目录不可写还在请求大麦：{:?}",
        damai.hits()
    );
}

// The rate limit is one per process and covers every request, not just two
// tier requests in a row: a three-session event's page and its three tier
// requests all wait for their slot. Measured at the server, and a lower
// bound with room to spare, so a loaded machine cannot fail it.
#[test]
fn every_request_of_one_query_waits_for_the_shared_rate_limit() {
    let home = Home::new("rate-limit");
    let damai = on_sale();
    let o = run(&home, &damai.base_url(), &["event", "1086791513717"]);
    assert!(o.status.success(), "{}", out(&o));
    assert_eq!(damai.hits().len(), 4, "{:?}", damai.hits());
    // Under the gap by more than a local request's round trip, so this is
    // about pacing and not about how fast the machine is.
    let least = theflash::damai::MIN_REQUEST_GAP.as_millis() as i64 - 50;
    let gaps = damai.arrival_gaps_ms();
    for (i, gap) in gaps.iter().enumerate() {
        assert!(
            *gap >= least,
            "第 {} 个和第 {} 个请求只隔了 {gap} 毫秒（至少 {least}）：\
             频率上限没覆盖整条查询\n{:?}\n{gaps:?}",
            i + 1,
            i + 2,
            damai.hits()
        );
    }
}

/// The four requests theFlash sends to damai.cn. Each one has to go
/// through the gate, redirects included.
#[derive(Clone, Copy, Debug)]
enum Exit {
    ItemPage,
    TierPage,
    HomePage,
    ClockReading,
}

impl Exit {
    fn name(self) -> &'static str {
        match self {
            Exit::ItemPage => "详情页",
            Exit::TierPage => "票档接口",
            Exit::HomePage => "首页",
            Exit::ClockReading => "读大麦时钟的 HEAD",
        }
    }

    async fn call(self, client: &Client) -> Result<(), Error> {
        match self {
            Exit::ItemPage => client.item_page(ITEM).await.map(|_| ()),
            Exit::TierPage => client.tier_page(ITEM, SESSION).await.map(|_| ()),
            Exit::HomePage => client.home_page().await.map(|_| ()),
            Exit::ClockReading => client.clock_reading(ITEM).await.map(|_| ()),
        }
    }
}

/// A client whose breaker lives in `home`, talking to `damai` and nowhere
/// else: the home page is the same local server, as everywhere in these
/// tests.
fn client_at(home: &Home, damai: &MockDamai, gap: Duration) -> (Arc<Gate>, Client) {
    let gate = Arc::new(
        Gate::new(
            home.state_dir(),
            gap,
            Arc::new(SystemClock::default()),
            "theflash test".into(),
        )
        .unwrap(),
    );
    let client = Client::with_gate(damai.base_url(), gate.clone())
        .unwrap()
        .with_home(damai.base_url());
    (gate, client)
}

/// damai.cn answers a request with a redirect to its slider challenge
/// rather than with the challenge in the body of the page asked for. The
/// hop after it must not be sent: it would leave without a slot and
/// without the breaker being looked at, and a challenge that redirects to
/// a page theFlash can read (an event that is gone) would go unrecorded
/// altogether, leaving the next query free to ask again (#74).
#[tokio::test]
async fn a_redirect_to_the_slider_challenge_is_recorded_and_not_followed() {
    for exit in [
        Exit::ItemPage,
        Exit::TierPage,
        Exit::HomePage,
        Exit::ClockReading,
    ] {
        let home = Home::new(&format!("redirect-challenge-{exit:?}"));
        // Every request is sent to the challenge; the challenge itself
        // replays an event that is gone, which says nothing about risk
        // control in its body.
        let damai = MockDamai::start_replies(vec![
            (
                "_____tmd_____".to_string(),
                Reply::ok(support::fixture("item_not_found.html")),
            ),
            (
                "/".to_string(),
                Reply::redirect("/_____tmd_____/punish?x5secdata=sample"),
            ),
        ]);
        let (gate, client) = client_at(&home, &damai, Duration::ZERO);

        let e = exit
            .call(&client)
            .await
            .expect_err(&format!("{}：滑块验证页被当成正常答复", exit.name()));
        assert!(matches!(e, Error::RiskControl(_)), "{}：{e}", exit.name());
        assert_eq!(
            damai.hits().len(),
            1,
            "{}：跟着重定向又发了请求 {:?}",
            exit.name(),
            damai.hits()
        );

        let stop = gate
            .stop()
            .unwrap()
            .unwrap_or_else(|| panic!("{}：熔断没有写下来", exit.name()));
        assert_eq!(stop.marker, "_____tmd_____", "{}", exit.name());
        assert!(
            home.risk_stop_file().is_file(),
            "{}：{} 不在",
            exit.name(),
            home.risk_stop_file().display()
        );

        // And the breaker it wrote stops the next request, which is the
        // whole point of recording it.
        let again = exit
            .call(&client)
            .await
            .expect_err(&format!("{}：熔断之后还在放行", exit.name()));
        assert!(
            matches!(again, Error::RiskStopped(_)),
            "{}: {again}",
            exit.name()
        );
        assert_eq!(damai.hits().len(), 1, "{:?}", damai.hits());
    }
}

/// An ordinary redirect is still followed — damai.cn moves a page and the
/// query has to arrive — but each hop waits for its own slot, measured
/// where it counts: when the request reaches the server.
#[tokio::test]
async fn an_ordinary_redirect_is_followed_one_slot_at_a_time() {
    let home = Home::new("redirect-paced");
    let damai = MockDamai::start_replies(vec![
        (
            "/item.htm?id=1086791513717&spm=moved".to_string(),
            Reply::ok(support::fixture("item_on_sale.html")),
        ),
        (
            "/item.htm?id=1086791513717".to_string(),
            Reply::redirect("/item.htm?id=1086791513717&spm=moved"),
        ),
    ]);
    let (_gate, client) = client_at(&home, &damai, MIN_REQUEST_GAP);

    let html = client.item_page(ITEM).await.expect("重定向后没拿到页面");
    assert!(html.contains("dataDefault"), "拿到的不是详情页");
    assert_eq!(damai.hits().len(), 2, "{:?}", damai.hits());
    let least = MIN_REQUEST_GAP.as_millis() as i64 - 50;
    let gaps = damai.arrival_gaps_ms();
    assert!(
        gaps[0] >= least,
        "重定向的下一跳只隔了 {} 毫秒（至少 {least}）：它没有排队{:?}",
        gaps[0],
        damai.hits()
    );
}

/// Redirects that never arrive anywhere stop, instead of spending the
/// whole run's requests on them.
#[tokio::test]
async fn a_redirect_that_never_lands_stops_after_a_few_hops() {
    let home = Home::new("redirect-loop");
    let mut routes: Vec<(String, Reply)> = (1..=6)
        .map(|n| {
            (
                format!("/hop{n}"),
                Reply::redirect(format!("/hop{}", n + 1)),
            )
        })
        .collect();
    routes.push(("/item.htm".to_string(), Reply::redirect("/hop1")));
    let damai = MockDamai::start_replies(routes);
    let (_gate, client) = client_at(&home, &damai, Duration::ZERO);

    let e = client
        .item_page(ITEM)
        .await
        .expect_err("一直重定向却说拿到了页面");
    assert!(
        e.to_string().contains("重定向超过"),
        "没说是重定向停下的：{e}"
    );
    assert_eq!(
        damai.hits().len(),
        6,
        "跟随的次数不是说好的那么多：{:?}",
        damai.hits()
    );
}

const TWO_CLIENTS_PROBE: &str = "two_default_clients_in_one_process_probe";

/// Two default clients in one process, which is what a command that
/// builds one for a query and one for a watch has. Ignored: the test
/// below runs it in a process whose HOME is a temporary directory, so the
/// breaker it builds is never the real user's.
#[test]
#[ignore = "run by two_default_clients_in_one_process_share_one_rate_limit"]
fn two_default_clients_in_one_process_probe() {
    let damai = on_sale();
    let left = Client::new(damai.base_url()).expect("第一个默认客户端没建起来");
    let right = Client::new(damai.base_url()).expect("第二个默认客户端没建起来");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (a, b) = rt.block_on(async { tokio::join!(left.item_page(ITEM), right.item_page(ITEM)) });
    a.expect("第一个客户端的请求失败");
    b.expect("第二个客户端的请求失败");
    assert_eq!(damai.hits().len(), 2, "{:?}", damai.hits());
    let least = MIN_REQUEST_GAP.as_millis() as i64 - 50;
    let gaps = damai.arrival_gaps_ms();
    assert!(
        gaps[0] >= least,
        "同一个进程里两个默认客户端的请求只隔了 {} 毫秒（至少 {least}）：\
         它们各有一份预算，绕开了共用的频率上限",
        gaps[0]
    );
}

/// However many clients a process builds the default way, damai.cn sees
/// one process's rate (#74). The probe above runs in a process of its
/// own, because the gate every default client shares is built once per
/// process and reads HOME.
#[test]
fn two_default_clients_in_one_process_share_one_rate_limit() {
    let home = Home::new("two-default-clients");
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--ignored", "--exact", TWO_CLIENTS_PROBE, "--nocapture"]);
    // The same tripwire and empty state directory every test runs under,
    // with a HOME of this test's own: the probe writes a breaker there.
    support::offline(&mut cmd);
    cmd.env("HOME", home.path());
    cmd.env_remove("XDG_STATE_HOME");
    let o = support::output_within(cmd, 60);
    assert!(o.status.success(), "{}", out(&o));
}

/// The lock file an earlier run left behind opens for writing even after
/// the directory has become read-only, and the breaker is a new file next
/// to it: without really writing one, theFlash sends its requests and
/// only finds out when risk control arrives (#74).
#[test]
fn a_state_directory_that_only_looks_writable_stops_queries_before_any_request() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new("read-only-state-dir");
    let dir = home.state_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("risk-stop.lock"), b"").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let writable_anyway = std::fs::write(dir.join("by-root"), b"").is_ok();
    let damai = on_sale();
    let o = run(&home, &damai.base_url(), &["event", "1086791513717"]);
    let text = out(&o);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    if writable_anyway {
        eprintln!("跳过：这个用户（root？）连只读目录也能写，看不出区别");
        return;
    }
    assert!(!o.status.success(), "只读的状态目录下照样查询了：{text}");
    assert!(text.contains("读不到大麦风控熔断状态"), "{text}");
    assert!(text.contains(&dir.display().to_string()), "{text}");
    assert!(text.contains("已停止"), "{text}");
    assert!(
        damai.hits().is_empty(),
        "熔断写不进去还在请求大麦：{:?}",
        damai.hits()
    );
}

#[test]
fn nothing_reads_or_writes_the_state_directory_until_a_request_is_due() {
    let home = Home::new("no-state-for-bad-input");
    let damai = on_sale();
    let o = run(
        &home,
        &damai.base_url(),
        &["event", "https://www.damai.cn/"],
    );
    assert_eq!(o.status.code(), Some(2), "{}", out(&o));
    assert!(
        !home.state_dir().exists(),
        "输入都没通过就建了状态目录 {}",
        home.state_dir().display()
    );
    assert!(damai.hits().is_empty(), "{:?}", damai.hits());
}

/// If `Gate::enter` claims its rate-limit slot before acquiring the shared
/// lock in `stop()`, two callers that both block on `risk-stop.lock` see
/// their slot timestamps pass while waiting and burst out together as soon
/// as the lock is released (#110).
#[test]
fn a_delayed_breaker_read_cannot_release_two_slots_together() {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::mpsc;
    use theflash::damai::Clock;

    struct ClockProbe {
        now: AtomicI64,
        seen: mpsc::Sender<i64>,
    }

    impl Clock for ClockProbe {
        fn now_ms(&self) -> i64 {
            let n = self.now.load(Ordering::SeqCst);
            let _ = self.seen.send(n);
            n
        }

        fn sleep_ms(&self, ms: i64) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(async move {
                if ms > 0 {
                    tokio::time::sleep(Duration::from_millis(ms as u64)).await;
                    self.now.fetch_add(ms, Ordering::SeqCst);
                }
            })
        }
    }

    let home = Home::new("review100-blocked-stop");
    let dir = home.state_dir();
    let (tx, rx) = mpsc::channel();
    let clock = Arc::new(ClockProbe {
        now: AtomicI64::new(0),
        seen: tx,
    });
    let gate = Arc::new(
        Gate::new(
            dir.clone(),
            Duration::from_millis(300),
            clock.clone(),
            "review100".into(),
        )
        .unwrap(),
    );
    let held = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("risk-stop.lock"))
        .unwrap();
    held.lock().unwrap();
    let server = MockDamai::start(&[("/item.htm", "item_on_sale.html")]);
    let start = |g: Arc<Gate>, c: Arc<ClockProbe>| {
        let url = server.base_url();
        std::thread::spawn(move || {
            let client = Client::with_gate(url, g).unwrap();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(client.item_page(ITEM)).unwrap();
            c.now.load(Ordering::SeqCst)
        })
    };
    let a = start(gate.clone(), clock.clone());
    assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), 0);
    clock.now.store(1000, Ordering::SeqCst);
    let b = start(gate.clone(), clock.clone());
    assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), 1000);
    held.unlock().unwrap();
    let x = a.join().unwrap();
    let y = b.join().unwrap();
    let gaps = server.arrival_gaps_ms();
    assert!(
        y - x >= 300,
        "enter returned at {x}/{y} ms after breaker lock was released; reserved spacing was lost (gaps={gaps:?})"
    );
    let least = MIN_REQUEST_GAP.as_millis() as i64 - 50;
    assert!(
        gaps[0] >= least,
        "熔断锁释放后两次请求在服务器上只隔了 {} 毫秒（至少 {least}）：{:?}",
        gaps[0],
        server.hits()
    );
}

/// Deterministic reproduction of #108: when the first request is delayed
/// for 230 ms (either on `risk-stop.lock` or while connecting/sending
/// before the server receives the request), the next redirect hop or a
/// concurrent default client's request must still arrive at the server at
/// least one rate-limit slot after the first request arrived, rather than
/// having the 230 ms delay subtracted from the 300 ms gap (leaving only
/// 70 ms or 19 ms between arrivals).
#[tokio::test]
async fn a_delayed_first_hop_still_keeps_redirect_and_concurrent_requests_one_slot_apart() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    let least = MIN_REQUEST_GAP.as_millis() as i64 - 50;
    let body = support::fixture("item_on_sale.html");

    // 1. Redirect: first hop stalls 230 ms before the server reads and
    // timestamps it, then answers 302; second hop answers 200 immediately.
    {
        let home = Home::new("delayed-redirect-hop1");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = body.clone();
        let server = std::thread::spawn(move || {
            let mut arrivals = Vec::new();
            for hop in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                if hop == 0 {
                    std::thread::sleep(Duration::from_millis(230));
                }
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                arrivals.push(Instant::now());
                if hop == 0 {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 302 Found\r\nLocation: /item.htm?id=1086791513717&spm=moved\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                } else {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
            }
            arrivals[1]
                .saturating_duration_since(arrivals[0])
                .as_millis() as i64
        });

        let gate = Arc::new(
            Gate::new(
                home.state_dir(),
                MIN_REQUEST_GAP,
                Arc::new(SystemClock::default()),
                "theflash test".into(),
            )
            .unwrap(),
        );
        let client = Client::with_gate(format!("http://{addr}"), gate).unwrap();
        let html = client.item_page(ITEM).await.unwrap();
        assert!(html.contains("dataDefault"));
        let gap_ms = server.join().unwrap();
        assert!(
            gap_ms >= least,
            "第一跳延迟 230 毫秒后，重定向下一跳只隔了 {gap_ms} 毫秒（至少 {least}）"
        );
    }

    // 2. Two concurrent clients sharing one Gate: the first client's
    // request stalls 250 ms before the server reads and timestamps it.
    {
        let home = Home::new("delayed-concurrent-clients");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut arrivals = Vec::new();
            for req_idx in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                if req_idx == 0 {
                    std::thread::sleep(Duration::from_millis(250));
                }
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                arrivals.push(Instant::now());
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
            arrivals[1]
                .saturating_duration_since(arrivals[0])
                .as_millis() as i64
        });

        let gate = Arc::new(
            Gate::new(
                home.state_dir(),
                MIN_REQUEST_GAP,
                Arc::new(SystemClock::default()),
                "theflash test".into(),
            )
            .unwrap(),
        );
        let base = format!("http://{addr}");
        let left = Client::with_gate(&base, gate.clone()).unwrap();
        let right = Client::with_gate(&base, gate).unwrap();
        let (a, b) = tokio::join!(left.item_page(ITEM), right.item_page(ITEM));
        a.unwrap();
        b.unwrap();
        let gap_ms = server.join().unwrap();
        assert!(
            gap_ms >= least,
            "第一个客户端请求延迟 250 毫秒后，第二个客户端请求只隔了 {gap_ms} 毫秒（至少 {least}）"
        );
    }
}
