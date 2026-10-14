//! End-to-end checks of `theflash event`: the built binary against a local
//! server that replays real damai.cn responses from testdata/damai. No test
//! reaches the real damai.cn.

mod support;

use std::net::TcpListener;
use std::process::{Output, Stdio};

use support::{MockDamai, fixture};

fn theflash(base_url: &str, args: &[&str]) -> Output {
    support::theflash(base_url).args(args).output().unwrap()
}

/// For the risk-control cases: tripping the breaker stops every process
/// sharing a HOME, so each takes one of its own. What the breaker does
/// across processes is checked in tests/access.rs.
fn theflash_at(home: &support::Home, base_url: &str, args: &[&str]) -> Output {
    home.theflash(base_url).args(args).output().unwrap()
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
    let home = support::Home::new("risk-on-page");
    let damai = MockDamai::start(&[("/item.htm?id=1086791513717", "risk_control.html")]);
    let out = theflash_at(&home, &damai.base_url(), &["event", "1086791513717"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("风控"), "stderr: {}", stderr(&out));
    assert_eq!(damai.hits().len(), 1, "requests: {:?}", damai.hits());
}

#[test]
fn risk_control_on_a_tier_request_stops_the_rest() {
    let home = support::Home::new("risk-on-tier");
    let damai = MockDamai::start(&[
        ("/item.htm?id=1086791513717", "item_on_sale.html"),
        ("dataId=", "risk_control.html"),
    ]);
    let out = theflash_at(&home, &damai.base_url(), &["event", "1086791513717"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("风控"), "stderr: {}", stderr(&out));
    assert_eq!(damai.hits().len(), 2, "requests: {:?}", damai.hits());
}

// Mutate exactly the real response and field used in the acceptance report.
fn sold_out_with_sku_list(value: Option<serde_json::Value>) -> MockDamai {
    let original = fixture("sku_sold_out_284490009.jsonp");
    let open = original.find('(').unwrap();
    let close = original.rfind(')').unwrap();
    let mut data: serde_json::Value = serde_json::from_str(&original[open + 1..close]).unwrap();
    let perform = data["perform"].as_object_mut().unwrap();
    match value {
        Some(value) => {
            perform.insert("skuList".into(), value);
        }
        None => {
            perform.remove("skuList");
        }
    }
    MockDamai::start_bodies(vec![
        (
            "/item.htm?id=1077137031347".into(),
            fixture("item_sold_out.html"),
        ),
        (
            "dataType=2&dataId=284490009".into(),
            format!("__jp0({data})"),
        ),
    ])
}

fn assert_unknown_sku_list(value: Option<serde_json::Value>) {
    let damai = sold_out_with_sku_list(value);
    let out = theflash(&damai.base_url(), &["event", "1077137031347"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let got = stdout(&out);
    assert!(
        got.contains("场次：2026-10-17 周六 13:00\n  票档：未知"),
        "{got}"
    );
    assert!(!got.contains("没有票档"), "{got}");
    assert_eq!(damai.hits().len(), 2);
}

#[test]
fn missing_sku_list_is_unknown() {
    assert_unknown_sku_list(None);
}

#[test]
fn null_sku_list_is_unknown() {
    assert_unknown_sku_list(Some(serde_json::Value::Null));
}

#[test]
fn explicitly_empty_sku_list_has_no_tiers() {
    let damai = sold_out_with_sku_list(Some(serde_json::json!([])));
    let out = theflash(&damai.base_url(), &["event", "1077137031347"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("  票档：没有票档\n"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn malformed_sku_list_is_a_failure_instead_of_empty() {
    for value in [
        serde_json::json!({}),
        serde_json::json!(""),
        serde_json::json!(false),
    ] {
        let damai = sold_out_with_sku_list(Some(value));
        let out = theflash(&damai.base_url(), &["event", "1077137031347"]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(stdout(&out).contains("票档：获取失败"), "{}", stdout(&out));
        assert!(!stdout(&out).contains("没有票档"), "{}", stdout(&out));
    }
}

#[test]
fn invalid_links_with_numeric_ids_are_refused_before_any_request() {
    let damai = MockDamai::start(&[
        ("/item.htm?id=1077137031347", "item_sold_out.html"),
        (
            "dataType=2&dataId=284490009",
            "sku_sold_out_284490009.jsonp",
        ),
    ]);
    for input in [
        "https:///detail.damai.cn/item.htm?id=1077137031347",
        "https://@detail.damai.cn/item.htm?id=1077137031347",
        "https://example.com/?id=1077137031347",
        "garbage?id=1077137031347",
        "https://detail.damai.cn.evil.example/item.htm?id=1077137031347",
        "https://damai.cn@evil.example/item.htm?id=1077137031347",
        "https://evil.example@detail.damai.cn/item.htm?id=1077137031347",
        "ftp://detail.damai.cn/item.htm?id=1077137031347",
        "https://detail.damai.cn/wrong.htm?id=1077137031347",
        "https://detail.damai.cn:444/item.htm?id=1077137031347",
        "https://detail.damai.cn/item.htm?itemId=1077137031347",
        "https://m.damai.cn/damai/detail/item.html?id=1077137031347",
        "https://detail.damai.cn/item.htm?id=1077137031347&id=123",
    ] {
        let out = theflash(&damai.base_url(), &["event", input]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "input {input:?}: {}",
            stdout(&out)
        );
        assert!(stderr(&out).contains("找不到演出 ID"), "{}", stderr(&out));
        assert!(stdout(&out).is_empty());
        assert!(
            damai.hits().is_empty(),
            "input {input:?}, requests: {:?}",
            damai.hits()
        );
    }
}

#[test]
fn supported_links_and_numeric_ids_query_the_same_event() {
    let damai = MockDamai::start(&[
        ("/item.htm?id=1077137031347", "item_sold_out.html"),
        (
            "dataType=2&dataId=284490009",
            "sku_sold_out_284490009.jsonp",
        ),
    ]);
    for input in [
        "1077137031347",
        "https://detail.damai.cn/item.htm?spm=share&id=1077137031347#top",
        "http://detail.damai.cn/item.htm?id=1077137031347",
        "https://m.damai.cn/damai/detail/item.html?itemId=1077137031347&from=share",
        "https://detail.damai.cn/item.htm?id=%31%30%37%37%31%33%37%30%33%31%33%34%37",
    ] {
        let out = theflash(&damai.base_url(), &["event", input]);
        assert!(out.status.success(), "input {input:?}: {}", stderr(&out));
        assert!(
            stdout(&out).contains("售卖状态：已售罄"),
            "{}",
            stdout(&out)
        );
    }
    assert_eq!(damai.hits().len(), 10);
}

/// What `theflash event` prints, put together from the fields of
/// `event-json` the browser shows: name, city, venue, the sale status and
/// time, the notices, and every session and tier in order. The page never
/// shows `report`, so `report` matching the CLI says nothing about the page
/// (#30, #48).
fn cli_text_from_event_json(event: &serde_json::Value) -> String {
    let text = |v: &serde_json::Value| -> String {
        v.as_str()
            .unwrap_or_else(|| panic!("expected text, got {v}"))
            .to_owned()
    };
    let mut out = format!(
        "演出：{}\n城市：{}\n场馆：{}\n售卖状态：{}\n",
        text(&event["name"]),
        text(&event["city"]),
        text(&event["venue"]),
        text(&event["sale"]["label"])
    );
    if !event["sellStart"].is_null() {
        out += &format!("开售时间：{}\n", text(&event["sellStart"]));
    }
    if !event["notice"].is_null() {
        let notice = text(&event["notice"]);
        let heading = if notice.starts_with("大麦电脑版") {
            "场次和票档"
        } else {
            "场次"
        };
        out += &format!("\n{heading}：{notice}\n");
    }
    for session in event["sessions"].as_array().unwrap() {
        out += &format!("\n场次：{}\n", text(&session["name"]));
        let tiers = session["tiers"].as_array().unwrap();
        if !session["error"].is_null() {
            out += &format!("  {}\n", text(&session["error"]));
        } else if !session["notice"].is_null() {
            out += &format!("  {}\n", text(&session["notice"]));
        } else if tiers.is_empty() {
            out += "  票档：没有票档\n";
        }
        for tier in tiers {
            out += &format!(
                "  {} · {} · {}\n",
                text(&tier["name"]),
                text(&tier["price"]),
                text(&tier["status"]["label"])
            );
        }
    }
    out
}

/// The style a CLI status word asks for, decided from the word alone.
fn status_kind(label: &str) -> &'static str {
    match label {
        "在售" | "可售" => "available",
        "缺货" => "out-of-stock",
        "未开售" => "not-yet-on-sale",
        "已售罄" => "sold-out",
        _ => "unknown",
    }
}

// The real responses the browser tests replay, plus an explicitly empty tier
// list. All the runs go at once: each process paces its own requests, and the
// calendar's 19 alone take seconds. Each is waited for before any assertion,
// so none outlives the test.
#[test]
fn browser_event_json_preserves_cli_content() {
    let cases = [
        (
            "on sale",
            "1086791513717",
            MockDamai::start(&[
                ("/item.htm?id=1086791513717", "item_on_sale.html"),
                ("dataType=2&dataId=286937789", "sku_on_sale_286937789.jsonp"),
                ("dataType=2&dataId=286937790", "sku_on_sale_286937790.jsonp"),
                ("dataType=2&dataId=286937788", "sku_on_sale_286937788.jsonp"),
            ]),
        ),
        (
            "not yet on sale",
            "1092337348536",
            MockDamai::start(&[
                ("/item.htm?id=1092337348536", "item_not_yet_on_sale.html"),
                (
                    "dataType=2&dataId=286966465",
                    "sku_not_yet_on_sale_286966465.jsonp",
                ),
            ]),
        ),
        (
            "sold out",
            "1077137031347",
            MockDamai::start(&[
                ("/item.htm?id=1077137031347", "item_sold_out.html"),
                (
                    "dataType=2&dataId=284490009",
                    "sku_sold_out_284490009.jsonp",
                ),
            ]),
        ),
        (
            "not sold on the PC site",
            "1080486727522",
            MockDamai::start(&[("/item.htm?id=1080486727522", "item_pc_unsupported.html")]),
        ),
        // Only one session's tiers are served: the other 17 fail, and say why.
        (
            "18 calendar sessions",
            "1084537866279",
            MockDamai::start(&[
                ("/item.htm?id=1084537866279", "item_calendar.html"),
                (
                    "dataType=2&dataId=286938129",
                    "sku_calendar_286938129.jsonp",
                ),
            ]),
        ),
        (
            "missing skuList",
            "1077137031347",
            sold_out_with_sku_list(None),
        ),
        (
            "empty skuList",
            "1077137031347",
            sold_out_with_sku_list(Some(serde_json::json!([]))),
        ),
    ];
    let started: Vec<_> = cases
        .iter()
        .map(|(case, id, damai)| {
            let start = |command: &str| {
                support::theflash(&damai.base_url())
                    .args([command, *id])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap()
            };
            (case, start("event"), start("event-json"))
        })
        .collect();
    let finished: Vec<_> = started
        .into_iter()
        .map(|(case, text, json)| {
            let text = text.wait_with_output().unwrap();
            (case, text, json.wait_with_output().unwrap())
        })
        .collect();
    for (case, text, json) in finished {
        assert!(text.status.success(), "{case}: {}", stderr(&text));
        assert!(
            json.status.success(),
            "{case}: 浏览器查询没有返回演出：{}",
            stderr(&json)
        );
        let cli = stdout(&text);
        let event: serde_json::Value = serde_json::from_str(&stdout(&json)).unwrap();
        assert_eq!(
            cli_text_from_event_json(&event),
            cli,
            "{case}: 浏览器拿到的字段和 CLI 说的不一样"
        );
        let tiers = event["sessions"].as_array().unwrap().iter();
        let tiers = tiers.flat_map(|s| s["tiers"].as_array().unwrap());
        for status in std::iter::once(&event["sale"]).chain(tiers.map(|t| &t["status"])) {
            let label = status["label"].as_str().unwrap();
            assert_eq!(
                status["kind"],
                status_kind(label),
                "{case}: “{label}”的样式语义和 CLI 的状态文字不符"
            );
        }
        assert_eq!(event["report"].as_str(), Some(cli.as_str()), "{case}");
    }
}

// Exercise the CLI environment wiring and every GET, including page -> first
// tier (which has no legacy 300 ms wait). Preserve the original fixture bytes.
#[test]
fn explicit_validation_gap_paces_every_get_and_preserves_default_output() {
    use std::time::Duration;
    let damai = MockDamai::start(&[
        ("/item.htm?id=1086791513717", "item_on_sale.html"),
        ("dataType=2&dataId=286937789", "sku_on_sale_286937789.jsonp"),
        ("dataType=2&dataId=286937790", "sku_on_sale_286937790.jsonp"),
        ("dataType=2&dataId=286937788", "sku_on_sale_286937788.jsonp"),
    ]);
    let run = |gap: Option<&str>| {
        let mut command = support::theflash(&damai.base_url());
        command.args(["event", "1086791513717"]);
        command.env_remove("THEFLASH_REQUEST_GAP_MS");
        if let Some(gap) = gap {
            command.env("THEFLASH_REQUEST_GAP_MS", gap);
        }
        support::output_within(command, 30)
    };
    let default = run(None);
    let zero = run(Some("0"));
    let paced = run(Some("1000"));
    for out in [&default, &zero, &paced] {
        assert!(out.status.success(), "{}", stderr(out));
        assert_eq!(stdout(out), stdout(&default));
    }
    let hits = damai.timed_hits();
    assert_eq!(hits.len(), 12, "{hits:?}");
    assert_eq!(damai.hits()[..4], damai.hits()[4..8]);
    assert_eq!(damai.hits()[..4], damai.hits()[8..]);
    // Only a lower bound; allow 150 ms for observation/scheduling jitter.
    // This remains far above the pre-existing 300 ms tier-only wait.
    for pair in hits[8..].windows(2) {
        let elapsed = pair[1].1.duration_since(pair[0].1);
        assert!(
            elapsed >= Duration::from_millis(850),
            "GET request gap missing: {} -> {} took {elapsed:?}",
            pair[0].0,
            pair[1].0
        );
    }
}

// A Linux without the ca-certificates package, like the ubuntu:24.04
// container in #37. rustls-platform-verifier finds no CA certificate while
// the client is being built, before any request, and every command stops
// there. It reads the system's certificates through rustls-native-certs,
// which reads only SSL_CERT_FILE and SSL_CERT_DIR when either is set, so
// an empty SSL_CERT_FILE and no SSL_CERT_DIR is a system with none. macOS
// verifies with its keychain, which can't be emptied like this, and always
// has certificates.
#[cfg(target_os = "linux")]
#[test]
fn without_ca_certificates_every_command_says_to_install_them_and_sends_nothing() {
    let damai = MockDamai::start(&[
        ("/item.htm?id=1077137031347", "item_sold_out.html"),
        (
            "dataType=2&dataId=284490009",
            "sku_sold_out_284490009.jsonp",
        ),
    ]);
    let run = |args: &[&str]| {
        let mut cmd = support::theflash(&damai.base_url());
        cmd.args(args)
            .env("SSL_CERT_FILE", "/dev/null")
            .env_remove("SSL_CERT_DIR");
        support::output_within(cmd, 15)
    };
    let says_to_install = |args: &[&str], said: &str| {
        for needle in ["没有 CA 证书", "ca-certificates", "apt-get install"] {
            assert!(said.contains(needle), "{args:?} 没有提到“{needle}”：{said}");
        }
        for wrong in ["（）", "网络请求失败", "builder error"] {
            assert!(!said.contains(wrong), "{args:?} 出现了“{wrong}”：{said}");
        }
    };

    for args in [
        &["event", "1077137031347"][..],
        &["watch", "1077137031347", "--no-system-notify"],
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
        says_to_install(args, &stderr(&out));
    }

    // The browser's query shows the message as it is (event-ui.md).
    let args = &["event-json", "1077137031347"][..];
    let out = run(args);
    assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stdout(&out));
    let error: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(error["error"]["code"], "setup", "{error}");
    says_to_install(args, error["error"]["message"].as_str().unwrap_or_default());

    assert!(damai.hits().is_empty(), "requests: {:?}", damai.hits());
}

// #51: configuration failures must not masquerade as a bad event link.
// Exercise the actual environment bytes and all four command entry points.
fn assert_invalid_request_gap_configuration(args: &[&str]) {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let command = args[0];
    let damai = MockDamai::start(&[]);
    let values = ["bad", "86400001", "-1", "18446744073709551616", "", "1.5"]
        .map(OsString::from)
        .into_iter()
        .chain([OsString::from_vec(vec![0xff])]);
    for value in values {
        let mut cmd = support::theflash(&damai.base_url());
        cmd.args(args).env("THEFLASH_REQUEST_GAP_MS", &value);
        if command == "watch" {
            cmd.arg("--no-system-notify");
        }
        let out = support::output_within(cmd, 10);
        assert_eq!(out.status.code(), Some(1), "{command}: {out:?}");
        assert!(damai.hits().is_empty(), "requests: {:?}", damai.hits());
        let message = if command == "event-json" {
            assert!(out.stderr.is_empty(), "{}", stderr(&out));
            let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(json["error"]["code"], "invalid-config", "{json}");
            json["error"]["message"].as_str().unwrap().to_string()
        } else {
            assert!(out.stdout.is_empty(), "{}", stdout(&out));
            stderr(&out)
        };
        for required in [
            "THEFLASH_REQUEST_GAP_MS",
            "无效",
            "0 到 86400000",
            "整数",
            "毫秒",
        ] {
            assert!(
                message.contains(required),
                "{command}: 缺少 {required}：{message}"
            );
        }
        assert!(!message.contains("演出 ID"), "{command}: {message}");
        assert!(!message.contains("bad"), "{command}: {message}");
    }
}

#[test]
fn event_reports_invalid_request_gap_configuration() {
    assert_invalid_request_gap_configuration(&["event", "1077137031347"]);
}

#[test]
fn event_json_reports_invalid_request_gap_configuration() {
    assert_invalid_request_gap_configuration(&["event-json", "1077137031347"]);
}

#[test]
fn watch_reports_invalid_request_gap_configuration() {
    assert_invalid_request_gap_configuration(&["watch", "1077137031347"]);
}

#[test]
fn risk_reports_invalid_request_gap_configuration() {
    for args in [
        &["risk"][..],
        &["risk", "resume"][..],
        &["risk", "resume", "--yes"][..],
    ] {
        assert_invalid_request_gap_configuration(args);
    }
}
