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

use std::process::Output;

use support::{Home, MockDamai};

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

#[test]
fn a_breaker_file_that_cannot_be_read_stops_queries_and_says_why() {
    let damai = on_sale();
    for broken in ["", "{", "{\"at\": \"2026-10-14\"}", "not json at all"] {
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
