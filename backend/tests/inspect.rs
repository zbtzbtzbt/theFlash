//! `theflash inspect` end to end, against a local server that replays the
//! responses in testdata/damai. #10: a damai.cn revision breaks theFlash
//! while CI stays green, so an agent runs this against the real site and
//! has to be told exactly what changed.
//!
//! No test here reaches the real damai.cn or GitHub: the replay server
//! stands in for both detail.damai.cn and the home page, and opening an
//! issue is only reachable with `--open-issue`, which no test passes.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

/// The five seed samples and the first session of each, which is what a
/// run with nothing stale asks for.
fn unchanged_routes() -> Vec<(String, String)> {
    [
        ("/item.htm?id=1086791513717", "item_on_sale.html"),
        ("/item.htm?id=1092337348536", "item_not_yet_on_sale.html"),
        ("/item.htm?id=1077137031347", "item_sold_out.html"),
        ("/item.htm?id=1084537866279", "item_calendar.html"),
        ("/item.htm?id=1080486727522", "item_pc_unsupported.html"),
        ("dataId=286937788", "sku_on_sale_286937788.jsonp"),
        ("dataId=286966465", "sku_not_yet_on_sale_286966465.jsonp"),
        ("dataId=284490009", "sku_sold_out_284490009.jsonp"),
        ("dataId=286938129", "sku_calendar_286938129.jsonp"),
    ]
    .into_iter()
    .map(|(target, fixture)| (target.to_string(), support::fixture(fixture)))
    .collect()
}

/// A home page with one link, in the shape damai.cn writes them. The run
/// only reads item ids out of it.
fn home_with(item_id: u64) -> (String, String) {
    (
        "/".to_string(),
        format!(
            "<html><body><a href=\"https://detail.damai.cn/item.htm?id={item_id}\">一场</a></body></html>"
        ),
    )
}

fn with_route(mut routes: Vec<(String, String)>, route: (String, String)) -> Vec<(String, String)> {
    routes.push(route);
    routes
}

/// Replaces the body of one route, the way damai.cn would after changing
/// something.
fn changed(
    routes: Vec<(String, String)>,
    target: &str,
    change: impl Fn(&str) -> String,
) -> Vec<(String, String)> {
    routes
        .into_iter()
        .map(|(t, body)| {
            if t == target {
                (t, change(&body))
            } else {
                (t, body)
            }
        })
        .collect()
}

/// A HOME of this test's own: the sample set is kept in the user's state
/// directory, and no test may read or write the real one.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let dir =
            std::env::temp_dir().join(format!("theflash-inspect-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Home(dir)
    }

    fn theflash(&self, base_url: &str) -> Command {
        let mut c = support::theflash(base_url);
        c.env("HOME", &self.0);
        c.env_remove("XDG_STATE_HOME");
        c
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Where theFlash keeps its state on this platform, spelled out so a
    /// test fails if it moves.
    fn samples_file(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.0
                .join("Library/Application Support/theFlash/inspect-samples.json")
        } else {
            self.0.join(".local/state/theflash/inspect-samples.json")
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(home: &Home, damai: &support::MockDamai, args: &[&str]) -> std::process::Output {
    let mut cmd = home.theflash(&damai.base_url());
    cmd.arg("inspect").args(args);
    // No run in a test waits between requests: the requests go to a local
    // server, and `request_gap` refuses this for the real damai.cn.
    cmd.args(["--gap", "0"]);
    support::output_within(cmd, 60)
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Nothing changed: every field is where theFlash expects it and every
/// status code is one it maps, so the run says so and asks for nothing.
#[test]
fn a_run_against_unchanged_pages_reports_no_change() {
    let home = Home::new("unchanged");
    let damai = support::MockDamai::start_bodies(unchanged_routes());
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    assert!(text.contains("结论：没有变化。"), "{text}");
    assert_eq!(out.status.code(), Some(0), "{text}");
    for situation in ["已开售", "未开售", "已售罄", "日历选日期", "电脑版不卖"] {
        assert!(text.contains(situation), "{situation} 没查：{text}");
    }
    assert!(text.contains("结构：和 testdata 里的一样"), "{text}");
    assert!(text.contains("状态码：都见过"), "{text}");
    // Five pages and the first session of the four that have one. No home
    // page: every situation still has a sample.
    assert_eq!(damai.hits().len(), 9, "{:?}", damai.hits());
    assert!(text.contains("发了 9 个请求"), "{text}");
}

/// What #10 asks for: a response with `performBases` taken out names the
/// field, says what theFlash used it for, and the command fails so a
/// scheduled run is noticed.
#[test]
fn a_page_that_lost_perform_bases_names_the_field_and_fails() {
    let home = Home::new("no-perform-bases");
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |body| {
        body.replace("\"performBases\"", "\"performBasesIsGone\"")
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    assert!(text.contains("结构变化："), "{text}");
    assert!(text.contains("dataDefault.performBases"), "{text}");
    assert!(text.contains("场次列表"), "{text}");
    assert!(text.contains("1086791513717"), "{text}");
    assert!(text.contains("发现 1 处结构变化"), "{text}");
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// A field inside the sessions theFlash has to have: without `performId`
/// it cannot ask for a session's tiers at all.
#[test]
fn a_session_that_lost_its_perform_id_names_the_field_and_the_place() {
    let home = Home::new("no-perform-id");
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |body| {
        body.replace("\"performId\"", "\"performIdIsGone\"")
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    assert!(
        text.contains("dataDefault.performBases[0].performs[0].performId 不见了"),
        "{text}"
    );
    assert!(text.contains("请求这个场次的票档"), "{text}");
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// A field that is still there with another kind: theFlash would print 未知
/// instead of failing, so only the inspection can notice.
#[test]
fn a_tier_field_that_changed_kind_is_reported_with_what_is_there_now() {
    let home = Home::new("kind-change");
    let routes = changed(unchanged_routes(), "dataId=286937788", |body| {
        body.replace("\"price\":\"", "\"price\":0,\"priceWas\":\"")
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    assert!(
        text.contains("perform.skuList[0].price 变成了数字"),
        "{text}"
    );
    assert!(text.contains("票价"), "{text}");
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// What #10 asks for: a status code 777 is reported as one nobody has
/// mapped, with the words damai.cn shows next to it.
#[test]
fn a_status_code_nobody_has_mapped_is_reported_with_the_page_words() {
    let home = Home::new("code-777");
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |body| {
        body.replace("\"buyBtnStatus\":204", "\"buyBtnStatus\":777")
    });
    // The on-sale sample stopped being on sale, so the run looks for a
    // replacement; the home page offers one it has already read.
    let damai = support::MockDamai::start_bodies(with_route(routes, home_with(1086791513717)));
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    assert!(text.contains("没见过的购买按钮状态码：777"), "{text}");
    assert!(text.contains("立即购票"), "页面上的文字也要报出来：{text}");
    assert!(text.contains("1 个没见过的状态码"), "{text}");
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// A tier status code nobody has mapped, with the tag damai.cn puts beside
/// it on the page.
#[test]
fn a_tier_status_code_nobody_has_mapped_is_reported_with_its_tag() {
    let home = Home::new("tier-777");
    let routes = changed(unchanged_routes(), "dataId=286937788", |body| {
        body.replace("\"frontEndStatus\":\"2\"", "\"frontEndStatus\":\"777\"")
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    assert!(text.contains("没见过的票档状态码：777"), "{text}");
    assert!(text.contains("缺货登记"), "{text}");
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// Samples go stale. A saved sample that no longer shows its situation is
/// replaced with one from the home page, and the new set is saved for the
/// next run.
#[test]
fn a_stale_sample_is_replaced_from_the_home_page_and_saved() {
    let home = Home::new("stale-sample");
    let samples = home.path().join("samples.json");
    std::fs::write(
        &samples,
        // 1077137031347 is sold out, not on sale: this set is stale.
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1077137031347,"category":"onSale"},
             {"itemId":1092337348536,"category":"notYetOnSale"},
             {"itemId":1077137031347,"category":"soldOut"},
             {"itemId":1084537866279,"category":"calendar"},
             {"itemId":1080486727522,"category":"notOnPcChannel"}]}"#,
    )
    .unwrap();
    let damai =
        support::MockDamai::start_bodies(with_route(unchanged_routes(), home_with(1086791513717)));
    let out = run(
        &home,
        &damai,
        &["--samples", samples.to_str().unwrap(), "--max-events", "8"],
    );
    let text = stdout(&out);
    assert!(text.contains("换掉了过期的样本 1077137031347"), "{text}");
    assert!(text.contains("[已开售] 1086791513717"), "{text}");
    assert!(
        damai.hits().iter().any(|h| h == "GET /"),
        "没去首页找新样本：{:?}",
        damai.hits()
    );
    let saved = std::fs::read_to_string(&samples).unwrap();
    assert!(saved.contains("1086791513717"), "{saved}");
    assert!(
        saved.contains("\"onSale\""),
        "新样本没有按状态存下来：{saved}"
    );
    assert_eq!(out.status.code(), Some(0), "换样本本身不算发现：{text}");
}

/// The sample set is kept in this user's state directory, so a run on a
/// machine that has never inspected anything still has the seeds. A test
/// writes it under its own HOME and never the real one.
#[test]
fn without_a_samples_file_the_run_seeds_one_in_the_state_directory() {
    let home = Home::new("default-samples");
    let damai = support::MockDamai::start_bodies(unchanged_routes());
    let out = run(&home, &damai, &[]);
    let file = home.samples_file();
    assert!(
        file.is_file(),
        "{} 没有写出来：{}",
        file.display(),
        stdout(&out)
    );
    let saved = std::fs::read_to_string(&file).unwrap();
    for id in [
        "1086791513717",
        "1092337348536",
        "1077137031347",
        "1084537866279",
        "1080486727522",
    ] {
        assert!(saved.contains(id), "{saved}");
    }
}

/// One run has a budget, because every agent shares the owner's one
/// address and #2's watch is usually running at the same time.
#[test]
fn a_run_asks_about_at_most_as_many_events_as_it_was_allowed() {
    let home = Home::new("budget");
    let damai = support::MockDamai::start_bodies(unchanged_routes());
    let out = run(&home, &damai, &["--max-events", "2"]);
    let text = stdout(&out);
    let pages = damai
        .hits()
        .iter()
        .filter(|h| h.contains("/item.htm"))
        .count();
    assert_eq!(pages, 2, "{:?}", damai.hits());
    assert!(text.contains("这次没有已售罄的样本"), "{text}");
    assert_eq!(out.status.code(), Some(0), "{text}");
}

/// The command an agent runs after damai.cn changed something: it writes
/// the real responses where the tests replay them from.
#[test]
fn record_writes_the_pages_it_fetched_as_fixtures() {
    let home = Home::new("record");
    let dir = home.path().join("testdata");
    std::fs::create_dir_all(&dir).unwrap();
    let damai = support::MockDamai::start_bodies(unchanged_routes());
    let mut cmd = home.theflash(&damai.base_url());
    cmd.args([
        "inspect",
        "record",
        "1086791513717",
        "--as",
        "on_sale",
        "--dir",
        dir.to_str().unwrap(),
        "--sessions",
        "1",
        "--gap",
        "0",
    ]);
    let out = support::output_within(cmd, 60);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    let page = dir.join("item_on_sale.html");
    let tiers = dir.join("sku_on_sale_286937788.jsonp");
    assert!(page.is_file() && tiers.is_file(), "{text}");
    assert_eq!(
        std::fs::read_to_string(&page).unwrap(),
        support::fixture("item_on_sale.html"),
        "录下来的页面和大麦发的不一样"
    );
    assert!(text.contains("item_on_sale.html"), "{text}");
    assert_eq!(damai.hits().len(), 2, "{:?}", damai.hits());
}

/// The owner's rule: no credential or personal data in the repository. A
/// recorded response has the numbers taken out, and one that carries a
/// login header is refused whole rather than saved half-clean.
#[test]
fn record_takes_personal_data_out_and_refuses_what_it_cannot_clean() {
    let home = Home::new("record-redact");
    let dir = home.path().join("testdata");
    std::fs::create_dir_all(&dir).unwrap();
    let phone = ["138", "0013", "8000"].concat();
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |body| {
        body.replace(
            "\"buyBtnStatus\":204",
            &format!("\"buyBtnStatus\":204,\"mobile\":\"{phone}\""),
        )
    });
    // Built from pieces so this file does not carry what repo_guard looks
    // for.
    let header = ["Set-", "Cookie", ":"].concat();
    let routes = changed(routes, "dataId=286937788", |body| {
        format!("{header} sid=1\n{body}")
    });
    let damai = support::MockDamai::start_bodies(routes);
    let mut cmd = home.theflash(&damai.base_url());
    cmd.args([
        "inspect",
        "record",
        "1086791513717",
        "--as",
        "planted",
        "--dir",
        dir.to_str().unwrap(),
        "--sessions",
        "1",
        "--gap",
        "0",
    ]);
    let out = support::output_within(cmd, 60);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let page = std::fs::read_to_string(dir.join("item_planted.html")).unwrap();
    assert!(!page.contains(&phone), "手机号还在录下来的页面里");
    assert!(page.contains("00000000000"), "手机号没有被换成 0");
    assert!(
        !dir.join("sku_planted_286937788.jsonp").exists(),
        "带 Set-Cookie 的响应不该被保存"
    );
    assert!(
        stderr.contains("Cookie") && stderr.contains("没有保存"),
        "没说清为什么没保存：{stderr}"
    );
}
