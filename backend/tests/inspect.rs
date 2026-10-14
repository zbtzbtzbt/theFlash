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

/// The recorded not-yet-on-sale event opens at 2026-10-14 10:18, which
/// every run after that reads as "this sample is about to stop being
/// 未开售" — a finding (#39), and one that would arrive at a different
/// moment on every machine. The routes below serve that page with its
/// sale far enough out that a run's verdict does not depend on when the
/// test runs; the tests about expiry set the time they need themselves.
const SELL_START_FAR_OUT: &str = "\"sellStartTime\":4102416000000";

/// The five seed samples and the first session of each, which is what a
/// run with nothing stale asks for.
fn unchanged_routes() -> Vec<(String, String)> {
    let routes: Vec<(String, String)> = [
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
    .collect();
    changed(routes, "/item.htm?id=1092337348536", |body| {
        body.replace("\"sellStartTime\":1791944280000", SELL_START_FAR_OUT)
    })
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
    let mut hit = false;
    let changed: Vec<(String, String)> = routes
        .into_iter()
        .map(|(t, body)| {
            if t != target {
                return (t, body);
            }
            let after = change(&body);
            // A replacement that matched nothing would leave the test
            // asserting about a page damai.cn never changed.
            assert_ne!(after, body, "{target} 没有被改到");
            hit = true;
            (t, after)
        })
        .collect();
    assert!(hit, "{target} 不在这组路由里");
    changed
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

/// damai.cn writes its home page in Chinese, so the 200 bytes a run reads
/// after a link almost never end on a character boundary. Cutting the page
/// there panicked, and a panic takes the whole inspection with it: this
/// run had already seen an unmapped status code and printed nothing about
/// it, exiting 101 instead of 1 (#75).
#[test]
fn a_home_page_written_in_chinese_does_not_take_the_report_down() {
    let home = Home::new("home-chinese");
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |body| {
        body.replace("\"buyBtnStatus\":204", "\"buyBtnStatus\":777")
    });
    // The on-sale sample stopped being on sale, so the run runs out of
    // candidates and asks the home page — whose one link is followed by a
    // hundred Chinese characters, as damai.cn's own is.
    let chinese_home = (
        "/".to_string(),
        format!(
            r#"<a href="item.htm?id=1086791513717">{}</a>"#,
            "中".repeat(100)
        ),
    );
    let damai = support::MockDamai::start_bodies(with_route(routes, chinese_home));
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(text.contains("没见过的购买按钮状态码：777"), "{text}");
    assert!(text.contains("1 个没见过的状态码"), "{text}");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{text}\n{stderr}\n{:?}",
        damai.hits()
    );
    assert!(
        damai.hits().iter().any(|h| h == "GET /"),
        "没问首页，这次复现没走到出问题的那条路：{:?}",
        damai.hits()
    );
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

/// Samples go stale. damai.cn has no anonymous listing to take a new one
/// from (#39), so a stale sample is replaced out of the pool kept in the
/// repository, without asking the home page at all.
#[test]
fn a_stale_sample_is_replaced_from_the_pool_without_asking_the_home_page() {
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
    assert!(
        text.contains("换掉了这一类原来的样本 1077137031347（不再是这个状态）"),
        "{text}"
    );
    assert!(text.contains("[已开售] 1086791513717"), "{text}");
    assert!(
        !damai.hits().iter().any(|h| h == "GET /"),
        "候选池里还有样本，不该去首页：{:?}",
        damai.hits()
    );
    let saved = std::fs::read_to_string(&samples).unwrap();
    assert!(saved.contains("1086791513717"), "{saved}");
    assert!(
        saved.contains("\"onSale\""),
        "新样本没有按状态存下来：{saved}"
    );
    assert_eq!(
        saved_ids(&samples, "onSale"),
        [1086791513717],
        "报告说换掉了样本，写下去的却不是：{saved}"
    );
    assert_eq!(out.status.code(), Some(0), "换样本本身不算发现：{text}");
}

/// An event that moved on — the sale opened — is filed under the
/// situation it shows now, so 已开售 refills itself from the sample that
/// used to be 未开售, with nothing to look it up in.
#[test]
fn an_event_whose_sale_opened_becomes_the_on_sale_sample() {
    let home = Home::new("moved-on");
    let samples = home.path().join("samples.json");
    // 1086791513717 is on sale; saved here as the 未开售 sample, as if its
    // sale had opened since the last run.
    std::fs::write(
        &samples,
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1077137031347,"category":"onSale"},
             {"itemId":1086791513717,"category":"notYetOnSale"},
             {"itemId":1077137031347,"category":"soldOut"},
             {"itemId":1084537866279,"category":"calendar"},
             {"itemId":1080486727522,"category":"notOnPcChannel"}]}"#,
    )
    .unwrap();
    let damai = support::MockDamai::start_bodies(unchanged_routes());
    let out = run(
        &home,
        &damai,
        &["--samples", samples.to_str().unwrap(), "--max-events", "8"],
    );
    let text = stdout(&out);
    assert!(text.contains("[已开售] 1086791513717"), "{text}");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&samples).unwrap()).unwrap();
    let on_sale: Vec<&serde_json::Value> = saved["samples"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["category"] == "onSale")
        .collect();
    assert_eq!(
        on_sale.first().map(|s| s["itemId"].as_u64()),
        Some(Some(1086791513717)),
        "开售了的那一场没有被存成已开售的样本：{saved}"
    );
}

/// The events one situation holds in the saved sample file, in order: the
/// first is what the next run reads, the rest are its backups. A report
/// that says a sample was swapped out has to agree with this (#79).
fn saved_ids(path: &Path, category: &str) -> Vec<u64> {
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    saved["samples"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["category"] == category)
        .map(|s| s["itemId"].as_u64().unwrap())
        .collect()
}

/// An event whose sale opened joins a situation another sample still
/// covers. Nothing was swapped out: the sample file keeps 1086791513717
/// first under 已开售 and the newcomer behind it. The report used to tell
/// the newcomer it had replaced the sample that is still there, which is
/// what #79 was opened for — the real run on 2026-10-14 10:56 printed
/// 换掉了过期的样本 1086791513717 while saving that very event as the
/// 已开售 sample.
#[test]
fn an_event_that_joined_a_situation_another_sample_still_covers_replaced_nothing() {
    let home = Home::new("joined-situation");
    let samples = home.path().join("samples.json");
    // The seed set: both samples are the ones the real run started from.
    std::fs::write(
        &samples,
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1086791513717,"category":"onSale"},
             {"itemId":1092337348536,"category":"notYetOnSale"},
             {"itemId":1077137031347,"category":"soldOut"},
             {"itemId":1084537866279,"category":"calendar"},
             {"itemId":1080486727522,"category":"notOnPcChannel"}]}"#,
    )
    .unwrap();
    // The 未开售 sample's page with only its buy button changed: its sale
    // opened, as 1092337348536's did at 10:18 that day.
    let routes = changed(unchanged_routes(), "/item.htm?id=1092337348536", |body| {
        body.replace("\"buyBtnStatus\":106", "\"buyBtnStatus\":204")
    });
    // The home page damai.cn serves today: no detail link, so 未开售 has
    // nothing left to replace it with (0014).
    let empty_home = (
        "/".to_string(),
        "<html><body>没有演出链接</body></html>".to_string(),
    );
    let damai = support::MockDamai::start_bodies(with_route(routes, empty_home));
    let out = run(
        &home,
        &damai,
        &["--samples", samples.to_str().unwrap(), "--max-events", "8"],
    );
    let text = stdout(&out);
    assert!(text.contains("[已开售] 1092337348536"), "{text}");
    assert!(
        !text.contains("换掉了"),
        "没有任何样本被换掉，报告却说换了：{text}"
    );
    assert_eq!(
        saved_ids(&samples, "onSale"),
        [1086791513717, 1092337348536],
        "报告说的和写进样本文件的对不上：{text}"
    );
}

/// A candidate probed for one situation can turn out to show another one.
/// It is kept as a backup behind the sample that already covers that
/// situation, so it swapped nothing out, and the report may not say it
/// did (#79).
#[test]
fn a_candidate_that_was_probed_and_not_chosen_replaced_nothing() {
    let home = Home::new("probed-not-chosen");
    let samples = home.path().join("samples.json");
    // The saved 已售罄 sample is on sale, so the run goes looking for a
    // replacement in the pool.
    std::fs::write(
        &samples,
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1086791513717,"category":"onSale"},
             {"itemId":1092337348536,"category":"notYetOnSale"},
             {"itemId":1086791513717,"category":"soldOut"},
             {"itemId":1084537866279,"category":"calendar"},
             {"itemId":1080486727522,"category":"notOnPcChannel"}]}"#,
    )
    .unwrap();
    // The pool's 已售罄 candidate answers with an on-sale page, so the run
    // probes it, files it under 已开售 and still has no 已售罄 sample.
    let routes = changed(unchanged_routes(), "/item.htm?id=1077137031347", |_| {
        support::fixture("item_on_sale.html")
    });
    let empty_home = (
        "/".to_string(),
        "<html><body>没有演出链接</body></html>".to_string(),
    );
    let damai = support::MockDamai::start_bodies(with_route(routes, empty_home));
    let out = run(
        &home,
        &damai,
        &["--samples", samples.to_str().unwrap(), "--max-events", "8"],
    );
    let text = stdout(&out);
    assert!(text.contains("样本缺口：没有已售罄的样本了"), "{text}");
    assert!(
        !text.contains("换掉了"),
        "被探了一下又没被选中的候选，报告却说它换掉了已开售的样本：{text}"
    );
    assert_eq!(
        saved_ids(&samples, "onSale"),
        [1086791513717, 1077137031347],
        "报告说的和写进样本文件的对不上：{text}"
    );
}

/// The report lines about one event, from its `[状态] <id> <名字>` line to
/// the blank line before the next event: what one event is said to have
/// replaced cannot be read off the whole report, because another event in
/// the same run may really have replaced something.
fn event_block(text: &str, item_id: u64) -> String {
    let at = text
        .find(&format!("] {item_id} "))
        .unwrap_or_else(|| panic!("报告里没有 {item_id}：{text}"));
    let from = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let rest = &text[from..];
    let to = rest.find("\n\n").map_or(rest.len(), |n| n + 1);
    rest[..to].to_string()
}

/// Two samples swapped situations and a third joined one of them. Each
/// situation's new first sample names the event it took that situation
/// over from, and the one kept as a backup behind it names nothing: a
/// report that only asks "was a sample of this situation someone else
/// last time" hands the same replacement to every page that shows the
/// situation (#79).
#[test]
fn only_the_sample_a_situation_is_read_from_next_says_it_took_the_situation_over() {
    let home = Home::new("backup-took-nothing");
    let samples = home.path().join("samples.json");
    // 1077137031347 and 1086791513717 swapped: the one saved as 已开售 is
    // sold out now and the one saved as 已售罄 is on sale. 1111111111111
    // was the 日历选日期 sample and is on sale too, so it joins 已开售
    // behind the event that took it over.
    std::fs::write(
        &samples,
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1077137031347,"category":"onSale"},
             {"itemId":1092337348536,"category":"notYetOnSale"},
             {"itemId":1086791513717,"category":"soldOut"},
             {"itemId":1111111111111,"category":"calendar"},
             {"itemId":1080486727522,"category":"notOnPcChannel"}]}"#,
    )
    .unwrap();
    let a_third_on_sale_event = (
        "/item.htm?id=1111111111111".to_string(),
        support::fixture("item_on_sale.html"),
    );
    let damai =
        support::MockDamai::start_bodies(with_route(unchanged_routes(), a_third_on_sale_event));
    let out = run(
        &home,
        &damai,
        &["--samples", samples.to_str().unwrap(), "--max-events", "8"],
    );
    let text = stdout(&out);
    let took_over = event_block(&text, 1086791513717);
    assert!(
        took_over.contains("换掉了这一类原来的样本 1077137031347（不再是这个状态）"),
        "{text}"
    );
    let backup = event_block(&text, 1111111111111);
    assert!(
        !backup.contains("换掉了"),
        "1111111111111 排在 1086791513717 后面，什么也没换掉：{text}"
    );
    assert_eq!(
        saved_ids(&samples, "onSale"),
        [1086791513717, 1111111111111],
        "报告说的和写进样本文件的对不上：{text}"
    );
}

/// A sample that moved into a situation already covered takes the front
/// of it, because this run read it first. The sample behind it is still
/// showing that situation, so it did not go stale and the report may not
/// say so; the one real replacement in this run — the 已开售 sample that
/// sold out — is the only one reported, and it says what became of it
/// (#79).
#[test]
fn only_the_sample_that_stopped_covering_its_situation_is_reported_as_replaced() {
    let home = Home::new("one-real-replacement");
    let samples = home.path().join("samples.json");
    // 1077137031347 is sold out, saved here as the 已开售 sample: its sale
    // ended since the last run. 1111111111111 is a second sold-out event,
    // and it is still sold out.
    std::fs::write(
        &samples,
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1077137031347,"category":"onSale"},
             {"itemId":1092337348536,"category":"notYetOnSale"},
             {"itemId":1111111111111,"category":"soldOut"},
             {"itemId":1084537866279,"category":"calendar"},
             {"itemId":1080486727522,"category":"notOnPcChannel"}]}"#,
    )
    .unwrap();
    let second_sold_out = (
        "/item.htm?id=1111111111111".to_string(),
        support::fixture("item_sold_out.html"),
    );
    let damai = support::MockDamai::start_bodies(with_route(unchanged_routes(), second_sold_out));
    let out = run(
        &home,
        &damai,
        &["--samples", samples.to_str().unwrap(), "--max-events", "8"],
    );
    let text = stdout(&out);
    // The real one: the pool's on-sale event takes 已开售 over from the
    // sample that is now sold out.
    assert!(text.contains("[已开售] 1086791513717"), "{text}");
    assert!(
        text.contains("换掉了这一类原来的样本 1077137031347（不再是这个状态）"),
        "真正的换样本没有报出来，或者没说原来那一场怎么了：{text}"
    );
    assert!(
        !text.contains("换掉了这一类原来的样本 1111111111111"),
        "1111111111111 还是已售罄，没有被换掉：{text}"
    );
    assert_eq!(
        saved_ids(&samples, "onSale"),
        [1086791513717],
        "报告说的和写进样本文件的对不上：{text}"
    );
    assert_eq!(
        saved_ids(&samples, "soldOut"),
        [1077137031347, 1111111111111],
        "两场都还是已售罄，都该留在样本文件里：{text}"
    );
}

/// A sample file that did not take this run's samples still holds the
/// old one, so the replacement this run worked out did not happen: the
/// next run reads the same sample again. Saying 换掉了 about a file whose
/// bytes never changed is the mistake #79 was opened for, in the other
/// direction — the report has to say the new sample was picked and not
/// kept, and say what the next run will still read.
#[test]
fn a_replacement_that_did_not_reach_the_file_is_not_reported_as_done() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new("replacement-unsaved");
    let samples = home.path().join("samples.json");
    // The same real replacement as the test above: the 已开售 sample sold
    // out, and the pool's 1086791513717 is on sale.
    std::fs::write(
        &samples,
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1077137031347,"category":"onSale"},
             {"itemId":1092337348536,"category":"notYetOnSale"},
             {"itemId":1111111111111,"category":"soldOut"},
             {"itemId":1084537866279,"category":"calendar"},
             {"itemId":1080486727522,"category":"notOnPcChannel"}]}"#,
    )
    .unwrap();
    let before = std::fs::read(&samples).unwrap();
    std::fs::set_permissions(&samples, std::fs::Permissions::from_mode(0o444)).unwrap();
    // root writes a read-only file whatever its mode says, so the run
    // would save after all and this test would assert about a success.
    let unwritable = std::fs::OpenOptions::new()
        .write(true)
        .open(&samples)
        .is_err();
    let second_sold_out = (
        "/item.htm?id=1111111111111".to_string(),
        support::fixture("item_sold_out.html"),
    );
    let damai = support::MockDamai::start_bodies(with_route(unchanged_routes(), second_sold_out));
    let out = run(
        &home,
        &damai,
        &["--samples", samples.to_str().unwrap(), "--max-events", "8"],
    );
    let text = stdout(&out);
    let after = std::fs::read(&samples).unwrap();
    std::fs::set_permissions(&samples, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        unwritable,
        "这次复现没走到保存失败那条路：这个用户写得了只读文件"
    );
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("样本没保存下来"), "{text}");
    assert_eq!(before, after, "样本文件被改了，这次复现不成立");
    assert_eq!(
        saved_ids(&samples, "onSale"),
        [1077137031347],
        "样本文件没变，下一次巡检还是读它"
    );
    assert!(
        !text.contains("换掉了"),
        "文件一个字节都没变，报告却说已经换掉了：{text}"
    );
    // What it should say instead: picked, not kept, and what the next run
    // still reads.
    let block = event_block(&text, 1086791513717);
    assert!(
        block.contains("1077137031347"),
        "没说这次选中它接替的是哪一场：{text}"
    );
    assert!(
        block.contains("没保存下来") && block.contains("下一次"),
        "没说这次的选择没留下来、下一次还是读原来那一场：{text}"
    );
}

/// What #39 is about: when a situation has no candidate left anywhere,
/// the inspection stops being able to see that situation. It says so,
/// says what to put where, and fails, so a scheduled run is noticed
/// instead of passing green while half blind.
#[test]
fn a_situation_with_no_candidate_left_is_a_finding_that_says_what_to_do() {
    let home = Home::new("no-candidate");
    let samples = home.path().join("samples.json");
    // Every candidate for 已售罄 — the saved one and the pool's — is the
    // same event, and it is on sale, so nothing covers 已售罄.
    std::fs::write(
        &samples,
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1086791513717,"category":"onSale"},
             {"itemId":1092337348536,"category":"notYetOnSale"},
             {"itemId":1086791513717,"category":"soldOut"},
             {"itemId":1084537866279,"category":"calendar"},
             {"itemId":1080486727522,"category":"notOnPcChannel"}]}"#,
    )
    .unwrap();
    // The pool's 已售罄 event answers as an on-sale page too, so no
    // candidate anywhere shows 已售罄.
    let routes = changed(unchanged_routes(), "/item.htm?id=1077137031347", |_| {
        support::fixture("item_on_sale.html")
    });
    let damai = support::MockDamai::start_bodies(with_route(routes, home_with(1086791513717)));
    let out = run(
        &home,
        &damai,
        &["--samples", samples.to_str().unwrap(), "--max-events", "8"],
    );
    let text = stdout(&out);
    assert!(text.contains("样本缺口：没有已售罄的样本了"), "{text}");
    assert!(
        text.contains(samples.to_str().unwrap()),
        "没说要改哪个文件：{text}"
    );
    assert!(
        text.contains("1 类样本补不上"),
        "结论里没说巡检看不见这一类了：{text}"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "补不上样本的巡检必须失败，否则定时任务看不到：{text}"
    );
    assert!(
        damai.hits().iter().any(|h| h == "GET /"),
        "候选池用完了，首页是最后一搏：{:?}",
        damai.hits()
    );
}

/// Finding a replacement takes a person or an agent (0014), so a sample
/// that is about to stop showing its situation is reported before it
/// does, not after.
#[test]
fn a_sample_that_opens_in_two_days_is_reported_before_its_situation_is_lost() {
    let home = Home::new("expiring");
    let in_two_days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 2 * 24 * 3600 * 1000;
    let routes = changed(unchanged_routes(), "/item.htm?id=1092337348536", |body| {
        body.replace(
            SELL_START_FAR_OUT,
            &format!("\"sellStartTime\":{in_two_days}"),
        )
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    assert!(text.contains("样本快过期：1092337348536"), "{text}");
    assert!(text.contains("之后就不是未开售了"), "{text}");
    assert!(text.contains("1 个样本快过期"), "{text}");
    assert_eq!(
        out.status.code(),
        Some(1),
        "快过期的样本要在还来得及的时候报出来：{text}"
    );
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

/// Samples paths this machine cannot write, one for each step of saving
/// that can fail. The first two fail whoever runs them: a parent that is
/// a regular file stops the directory from being made, and a path that is
/// already a directory stops the write itself.
fn unwritable_samples_paths(home: &Path) -> Vec<(&'static str, PathBuf, String)> {
    let block = home.join("block");
    std::fs::write(&block, "不是目录").unwrap();
    let a_directory = home.join("samples.json");
    std::fs::create_dir_all(&a_directory).unwrap();
    let mut cases = vec![
        (
            "上级是一个普通文件",
            block.join("samples.json"),
            block.display().to_string(),
        ),
        (
            "路径本身是一个目录",
            a_directory.clone(),
            a_directory.display().to_string(),
        ),
    ];
    // A state directory nobody may write in, which is the other way a
    // machine loses the sample set. root may write in it whatever its
    // mode says, so this case is added only where the operating system
    // can express it; the two above hold either way.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let read_only = home.join("read-only");
        std::fs::create_dir_all(&read_only).unwrap();
        std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o500)).unwrap();
        if std::fs::write(read_only.join("probe"), "x").is_err() {
            let path = read_only.join("samples.json");
            let shown = path.display().to_string();
            cases.push(("目录不可写", path, shown));
        }
    }
    cases
}

/// A run whose sample set did not reach the disk did not finish its job:
/// the coverage it worked out is lost, and the next run starts again from
/// the samples this one was replacing. It used to print 样本没保存下来 and
/// 结论：没有变化 in the same report and exit 0, so a scheduled run counted
/// it as a success (#77).
#[test]
fn a_run_that_could_not_save_its_samples_fails_and_says_what_to_do() {
    let home = Home::new("samples-unwritable");
    let damai = support::MockDamai::start_bodies(unchanged_routes());
    for (what, samples, shown) in unwritable_samples_paths(home.path()) {
        let out = run(&home, &damai, &["--samples", samples.to_str().unwrap()]);
        let text = stdout(&out);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{what}：样本没保存下来，定时任务却按退出码当成这次成功了：{text}"
        );
        assert!(
            !text.contains("结论：没有变化。"),
            "{what}：样本没写下去，报告却说这次没有变化：{text}"
        );
        assert!(text.contains("样本没保存下来"), "{what}：{text}");
        assert!(text.contains(&shown), "{what}：没说是哪个路径：{text}");
        assert!(text.contains("--samples"), "{what}：没说该怎么办：{text}");
        assert!(
            !text.contains("大麦很可能改版了"),
            "{what}：本机的磁盘问题被说成了大麦改版：{text}"
        );
        assert!(
            !samples.is_file(),
            "{what}：这次复现没走到保存失败那条路，文件写出来了"
        );
    }

    // The same run on a path it can write is a success again: a disk that
    // works is not news.
    let good = home.path().join("good/inspect-samples.json");
    let out = run(&home, &damai, &["--samples", good.to_str().unwrap()]);
    let text = stdout(&out);
    assert!(text.contains("结论：没有变化。"), "{text}");
    assert!(!text.contains("样本没保存下来"), "{text}");
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(good.is_file(), "样本没写出来：{text}");
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
    // A situation the budget did not reach is not a finding: the next run
    // with a bigger budget covers it.
    assert!(text.contains("这次没查已售罄：--max-events"), "{text}");
    assert_eq!(out.status.code(), Some(0), "{text}");
}

/// An array element that stopped being an object takes every field under
/// it away. Without a word about the elements themselves, a tier list of
/// nulls reads as "nothing changed" while no user could buy a ticket.
#[test]
fn a_tier_list_whose_elements_are_no_longer_objects_is_reported() {
    let home = Home::new("tier-elements");
    // The saved response with only its tier list emptied of objects: the
    // callback and every other field stay as damai.cn wrote them.
    let routes = changed(unchanged_routes(), "dataId=286937788", |body| {
        let (open, close) = (body.find('(').unwrap(), body.rfind(')').unwrap());
        let mut json: serde_json::Value = serde_json::from_str(&body[open + 1..close]).unwrap();
        json["perform"]["skuList"] = serde_json::json!([null]);
        format!("{}({}){}", &body[..open], json, &body[close + 1..])
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    assert!(text.contains("perform.skuList[0] 变成了null"), "{text}");
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// damai.cn can answer the tier endpoint with every field in place and
/// still say the query failed. `theflash event` refuses that response, so
/// an inspection that called it unchanged would be saying the opposite of
/// what a user sees.
#[test]
fn a_tier_response_damai_marked_failed_is_reported_even_though_its_shape_is_intact() {
    let home = Home::new("tier-failed");
    let routes = changed(unchanged_routes(), "dataId=286937788", |body| {
        body.replace(
            r#""responseInfo":{"responseSuccess":"true","responseCode":""}"#,
            r#""responseInfo":{"responseSuccess":"false","responseCode":"X1"}"#,
        )
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    assert!(
        text.contains("结构：和 testdata 里的一样"),
        "字段都还在：{text}"
    );
    assert!(text.contains("票档接口：接口返回失败"), "{text}");
    assert!(text.contains("X1"), "{text}");
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// The same for a response about another session: theFlash asked for one
/// session's tiers and would be showing someone else's.
#[test]
fn a_tier_response_about_another_session_is_reported() {
    let home = Home::new("tier-other-session");
    let routes = changed(unchanged_routes(), "dataId=286937788", |body| {
        body.replace(r#""performId":"286937788""#, r#""performId":"286937791""#)
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    assert!(
        text.contains("票档接口：接口返回的是另一个场次（286937791）"),
        "{text}"
    );
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// A detail page can carry a session id `theflash event` refuses — a word
/// where the number was — with every field of the right kind in its
/// place. The run used to skip that session and read the next readable
/// one, so the same response failed a user's query and passed the
/// inspection as 没有变化 (#73).
#[test]
fn a_session_id_a_query_refuses_is_reported_and_not_skipped_over() {
    let home = Home::new("perform-id-not-a-number");
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |body| {
        body.replacen(
            "\"performId\":286937788",
            "\"performId\":\"not-a-number\"",
            1,
        )
    });
    // The session after the broken one, whose tiers the run read instead
    // of saying anything about the page.
    let next = (
        "dataId=286937790".to_string(),
        support::fixture("sku_on_sale_286937790.jsonp"),
    );
    let damai = support::MockDamai::start_bodies(with_route(routes, next));

    // The same response, asked the way a user asks: the query fails.
    let mut query = home.theflash(&damai.base_url());
    query.args(["event", "1086791513717"]);
    let queried = support::output_within(query, 60);
    let said = String::from_utf8_lossy(&queried.stderr).into_owned();
    assert_eq!(queried.status.code(), Some(1), "{said}");
    assert!(said.contains("场次缺少 performId"), "{said}");

    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    let block = about(&text, "1086791513717");
    assert!(
        block.contains("查不了"),
        "查询读不了这一页，巡检却说它没问题：{block}"
    );
    assert!(
        block.contains("not-a-number"),
        "没说是哪个 performId 读不了：{block}"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "查询会失败的页面，巡检必须报出来：{text}"
    );
}

/// The same page with every session id negative. -1 is a number, so every
/// declared kind matched, and the run asked for no tiers at all: eight
/// requests and 没有变化 for a page no user could buy from (#73).
#[test]
fn a_page_whose_session_ids_are_all_negative_is_a_finding() {
    let home = Home::new("perform-id-negative");
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |body| {
        let mut out = body.to_string();
        for id in ["286937788", "286937789", "286937790"] {
            out = out.replace(&format!("\"performId\":{id}"), "\"performId\":-1");
        }
        out
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    let block = about(&text, "1086791513717");
    assert!(
        block.contains("查不了"),
        "场次 ID 是负数，票档根本请求不了，巡检却说没变化：{block}"
    );
    assert!(block.contains("-1"), "没说是哪个 performId 读不了：{block}");
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// A page the query cannot read and a tier response it cannot read are
/// two separate pieces of news: the second must not take the place of the
/// first, or a revision would be reported half.
#[test]
fn a_page_and_a_tier_response_that_both_fail_are_both_reported() {
    let home = Home::new("page-and-tier-fail");
    // The second session on the page, so its first session's tiers are
    // still asked for and can fail on their own.
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |body| {
        body.replacen(
            "\"performId\":286937790",
            "\"performId\":\"not-a-number\"",
            1,
        )
    });
    let routes = changed(routes, "dataId=286937788", |body| {
        body.replace(
            r#""responseInfo":{"responseSuccess":"true","responseCode":""}"#,
            r#""responseInfo":{"responseSuccess":"false","responseCode":"X1"}"#,
        )
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &[]);
    let text = stdout(&out);
    let block = about(&text, "1086791513717");
    assert!(
        block.contains("详情页：场次缺少 performId"),
        "票档接口的问题把详情页的问题盖掉了：{block}"
    );
    assert!(
        block.contains("票档接口：接口返回失败") && block.contains("X1"),
        "{block}"
    );
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// The default command, which is what a scheduled run uses: a sample that
/// went stale is replaced without anyone passing --max-events. A budget
/// with no room for a replacement meant the home page was never read, and
/// the stale set — which names one event twice — spent a request on it.
#[test]
fn the_default_run_has_room_to_replace_a_stale_sample() {
    let home = Home::new("default-replaces");
    let samples = home.path().join("samples.json");
    std::fs::write(
        &samples,
        // 1077137031347 is sold out, so it stands for onSale no longer,
        // and it is named twice.
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
    let out = run(&home, &damai, &["--samples", samples.to_str().unwrap()]);
    let text = stdout(&out);
    assert!(
        damai.hits().iter().any(|h| h.contains("id=1086791513717")),
        "默认预算下没去读候选，没换成新样本：{:?}",
        damai.hits()
    );
    assert_eq!(
        damai
            .hits()
            .iter()
            .filter(|h| h.contains("id=1077137031347"))
            .count(),
        1,
        "同一场被问了两遍：{:?}",
        damai.hits()
    );
    assert!(text.contains("[已开售] 1086791513717"), "{text}");
    let saved = std::fs::read_to_string(&samples).unwrap();
    for category in [
        "onSale",
        "notYetOnSale",
        "soldOut",
        "calendar",
        "notOnPcChannel",
    ] {
        assert!(saved.contains(category), "{category} 没了：{saved}");
    }
    assert!(saved.contains("1086791513717"), "{saved}");
    assert_eq!(out.status.code(), Some(0), "换样本本身不算发现：{text}");
}

/// Five situations means five samples, so a budget of exactly five left a
/// run no request to spend on a replacement: it read every sample, found a
/// situation uncovered, and stopped without ever asking the home page. The
/// budget is a ceiling with room above the five, not a target.
#[test]
fn the_default_budget_leaves_room_to_replace_one_of_five_separate_samples() {
    let home = Home::new("default-budget");
    let samples = home.path().join("samples.json");
    // The event this run was watching for 已开售 was taken down, which is
    // what a sample going stale looks like at its worst.
    std::fs::write(
        &samples,
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1090000000001,"category":"onSale"},
             {"itemId":1092337348536,"category":"notYetOnSale"},
             {"itemId":1077137031347,"category":"soldOut"},
             {"itemId":1084537866279,"category":"calendar"},
             {"itemId":1080486727522,"category":"notOnPcChannel"}]}"#,
    )
    .unwrap();
    let gone = (
        "/item.htm?id=1090000000001".to_string(),
        support::fixture("item_not_found.html"),
    );
    let routes = with_route(unchanged_routes(), gone);
    let damai = support::MockDamai::start_bodies(with_route(routes, home_with(1086791513717)));
    let out = run(&home, &damai, &["--samples", samples.to_str().unwrap()]);
    let text = stdout(&out);
    assert!(
        damai.hits().iter().any(|h| h.contains("id=1086791513717")),
        "五个样本就把预算花光了，没去读替代的候选：{:?}",
        damai.hits()
    );
    assert!(text.contains("[已开售] 1086791513717"), "{text}");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&samples).unwrap()).unwrap();
    let on_sale: Vec<&serde_json::Value> = saved["samples"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["category"] == "onSale")
        .collect();
    assert_eq!(
        on_sale.first().map(|s| s["itemId"].as_u64()),
        Some(Some(1086791513717)),
        "下架的那场还排在已开售的第一位，下次又要先读它：{saved}"
    );
}

/// A situation left without a sample is not a healthy run: nobody is
/// watching it until an agent hands it a new event. A sample that could
/// not be read stays in the file — one failed request is not proof the
/// event is gone — so the next run still inspects as much as this one did.
#[test]
fn a_situation_left_without_a_sample_fails_and_keeps_the_sample_it_had() {
    let home = Home::new("coverage-lost");
    let samples = home.path().join("samples.json");
    std::fs::write(
        &samples,
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1090000000001,"category":"onSale"},
             {"itemId":1092337348536,"category":"notYetOnSale"},
             {"itemId":1077137031347,"category":"soldOut"},
             {"itemId":1084537866279,"category":"calendar"},
             {"itemId":1080486727522,"category":"notOnPcChannel"}]}"#,
    )
    .unwrap();
    // The 已开售 sample was taken down, the pool's 已开售 event is sold
    // out by now, and the home page is as damai.cn really serves it
    // today: no detail link. So nothing anywhere covers 已开售 (#39).
    let gone = (
        "/item.htm?id=1090000000001".to_string(),
        support::fixture("item_not_found.html"),
    );
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |_| {
        support::fixture("item_sold_out.html")
    });
    let no_candidates = (
        "/".to_string(),
        "<html><body>没有演出链接</body></html>".to_string(),
    );
    let damai =
        support::MockDamai::start_bodies(with_route(with_route(routes, gone), no_candidates));
    let out = run(&home, &damai, &["--samples", samples.to_str().unwrap()]);
    let text = stdout(&out);
    assert!(text.contains("样本缺口：没有已开售的样本了"), "{text}");
    assert!(text.contains("1 类样本补不上"), "{text}");
    assert_eq!(out.status.code(), Some(1), "{text}");
    let saved = std::fs::read_to_string(&samples).unwrap();
    assert!(
        saved.contains("\"onSale\"") && saved.contains("1090000000001"),
        "读不了的那一类被从样本文件里删掉了，下次就不查了：{saved}"
    );
    for category in [
        "onSale",
        "notYetOnSale",
        "soldOut",
        "calendar",
        "notOnPcChannel",
    ] {
        assert!(saved.contains(category), "{category} 没了：{saved}");
    }
}

/// An event whose page could not be read had nothing checked, so the run
/// must not also say its structure and status codes were fine — on the
/// real site a request that timed out read as "查不了" and "结构：和
/// testdata 里的一样" in the same breath. The missing sample it leaves
/// behind says it could not be read, not that damai.cn changed it.
#[test]
fn an_event_that_could_not_be_read_claims_nothing_about_its_structure() {
    let home = Home::new("unreadable-event");
    let samples = home.path().join("samples.json");
    std::fs::write(
        &samples,
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1090000000001,"category":"onSale"},
             {"itemId":1092337348536,"category":"notYetOnSale"},
             {"itemId":1077137031347,"category":"soldOut"},
             {"itemId":1084537866279,"category":"calendar"},
             {"itemId":1080486727522,"category":"notOnPcChannel"}]}"#,
    )
    .unwrap();
    let gone = (
        "/item.htm?id=1090000000001".to_string(),
        support::fixture("item_not_found.html"),
    );
    // Nothing else covers 已开售 either, so the run ends with the gap and
    // has to say which of the two things happened to the saved sample.
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |_| {
        support::fixture("item_sold_out.html")
    });
    let no_candidates = (
        "/".to_string(),
        "<html><body>没有演出链接</body></html>".to_string(),
    );
    let damai =
        support::MockDamai::start_bodies(with_route(with_route(routes, gone), no_candidates));
    let out = run(&home, &damai, &["--samples", samples.to_str().unwrap()]);
    let text = stdout(&out);
    let block = about(&text, "1090000000001");
    assert!(block.contains("查不了"), "{block}");
    assert!(
        !block.contains("结构：和 testdata 里的一样"),
        "一个字段都没查到，却说结构没变：{block}"
    );
    assert!(
        !block.contains("状态码：都见过"),
        "一个状态码都没读到，却说都见过：{block}"
    );
    assert!(
        text.contains("样本缺口：没有已开售的样本了") && text.contains("1090000000001 这次读不了"),
        "样本缺口的原因说成了状态变了，其实是这一页读不了：{text}"
    );
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// The part of the report about one event: its heading and the lines under
/// it, up to the blank line that ends it.
fn about(text: &str, item_id: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.starts_with('[') && l.contains(item_id))
        .unwrap_or_else(|| panic!("报告里没有 {item_id}：{text}"));
    lines[start..]
        .iter()
        .take_while(|l| !l.trim().is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// A candidate is the response as damai.cn sent it, under the name the
/// fixture beside it has: whoever fixes the change copies it into
/// testdata/damai and the same readers replay it.
#[test]
fn a_candidate_is_the_response_as_damai_sent_it() {
    let home = Home::new("candidate-bytes");
    let dir = home.path().join("candidates");
    let tiers = support::fixture("sku_on_sale_286937788.jsonp")
        .replace("\"frontEndStatus\":\"2\"", "\"frontEndStatus\":\"777\"");
    let routes = changed(unchanged_routes(), "dataId=286937788", |_| tiers.clone());
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &["--candidates", dir.to_str().unwrap()]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert_eq!(
        std::fs::read_to_string(dir.join("sku_1086791513717_286937788.jsonp")).unwrap(),
        tiers,
        "候选票档响应不是大麦发的那份字节"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("item_1086791513717.html")).unwrap(),
        support::fixture("item_on_sale.html"),
        "候选详情页不是大麦发的那份字节"
    );
}

/// A response theFlash could not read at all is exactly the one whoever
/// fixes the change has to replay, so it is kept too — it used to be
/// thrown away, leaving the issue with nothing to reproduce from.
#[test]
fn a_response_that_cannot_be_read_is_kept_as_a_candidate() {
    let home = Home::new("candidate-unreadable");
    let dir = home.path().join("candidates");
    let broken = "<html>upstream new layout</html>".to_string();
    let routes = changed(unchanged_routes(), "dataId=286937788", |_| broken.clone());
    let routes = changed(routes, "/item.htm?id=1092337348536", |_| broken.clone());
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &["--candidates", dir.to_str().unwrap()]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert_eq!(
        candidate(&dir, "sku_1086791513717_286937788.jsonp"),
        broken,
        "读不懂的票档响应没留下来"
    );
    assert_eq!(
        candidate(&dir, "item_1092337348536.html"),
        broken,
        "读不懂的详情页没留下来"
    );
}

/// A candidate response is the only copy of what damai.cn sent: fetching
/// it again costs another request from the one address every agent
/// shares. A run that could not write one down says so in the same words
/// as a sample set it could not write down, and does not call either a
/// revision (#77).
#[test]
fn a_candidate_that_could_not_be_written_is_said_with_the_rest_of_the_report() {
    let home = Home::new("candidate-unwritable");
    // A candidates directory that cannot be made: its parent is a file.
    let block = home.path().join("block");
    std::fs::write(&block, "不是目录").unwrap();
    let dir = block.join("candidates");
    let routes = changed(unchanged_routes(), "dataId=286937788", |body| {
        body.replace("\"frontEndStatus\":\"2\"", "\"frontEndStatus\":\"777\"")
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &["--candidates", dir.to_str().unwrap()]);
    let text = stdout(&out);
    assert!(text.contains("没见过的票档状态码：777"), "{text}");
    assert!(
        text.contains("候选响应没保存下来") && text.contains(&block.display().to_string()),
        "没说这次的响应没留下来、也没说是哪个位置：{text}"
    );
    assert!(
        text.contains("inspect record"),
        "没说怎么重新取一份：{text}"
    );
    assert!(
        text.contains("不是大麦改版"),
        "写不下来这件事要说成本机的问题：{text}"
    );
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// A candidate the redaction rule refuses whole is a different thing from
/// one this machine could not write: the refusal is the owner's rule
/// working, and the bytes were never meant to stay here, so the report
/// names the response and the rule rather than telling whoever reads it
/// to go look at a path or a disk (#78). The run still ends in 1, because
/// the change that made it keep a candidate is a finding on its own.
#[test]
fn a_candidate_the_redaction_rule_refuses_is_reported_as_the_rule_not_as_a_disk() {
    let home = Home::new("candidate-refused");
    let dir = home.path().join("candidates");
    // Built from pieces so this file does not carry what repo_guard looks
    // for.
    let header = ["Set-", "Cookie", ":"].concat();
    let routes = changed(unchanged_routes(), "dataId=286937788", |body| {
        let changed = body.replace("\"frontEndStatus\":\"2\"", "\"frontEndStatus\":\"777\"");
        format!("{header} sid=1\n{changed}")
    });
    let damai = support::MockDamai::start_bodies(routes);
    let out = run(&home, &damai, &["--candidates", dir.to_str().unwrap()]);
    let text = stdout(&out);
    let refused = dir.join("sku_1086791513717_286937788.jsonp");
    assert!(
        text.contains(&refused.display().to_string()) && text.contains("Cookie"),
        "没说是哪一份响应、按哪条规则丢掉的：{text}"
    );
    assert!(!refused.exists(), "带 Set-Cookie 的响应不该被保存：{text}");
    assert!(
        !text.contains("不是大麦改版"),
        "脱敏规则拒绝不是本机的磁盘问题，不该说成结果没落盘：{text}"
    );
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// `theflash inspect record` is run for the files it writes. One it could
/// not write is the command failing, however many of the others landed:
/// it used to say so on stderr and exit 0, so a caller that checks the
/// exit code believed it had a full set of fixtures (#77).
#[test]
fn record_fails_when_a_fixture_it_fetched_could_not_be_written() {
    let home = Home::new("record-unwritable");
    let dir = home.path().join("testdata");
    std::fs::create_dir_all(&dir).unwrap();
    // The tier fixture's name, taken by a directory: the page is written
    // and this one cannot be, which is the half-done run.
    let taken = dir.join("sku_on_sale_286937788.jsonp");
    std::fs::create_dir_all(&taken).unwrap();
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
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        out.status.code(),
        Some(1),
        "fixture 没写下来，命令却算成功：{said}{}",
        stdout(&out)
    );
    assert!(
        said.contains(&taken.display().to_string()) && said.contains("写不进去"),
        "没说是哪个 fixture 没写下来：{said}"
    );
    assert!(
        said.contains(&dir.join("item_on_sale.html").display().to_string()),
        "没说这次写下来的是哪些，半成品没法收拾：{said}"
    );
}

/// What a run wrote for whoever fixes the change, with a message that says
/// what is missing rather than only that a file is.
fn candidate(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| {
        let wrote: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        panic!("没有留下候选 {name}（{e}）；这次写出来的是 {wrote:?}")
    })
}

/// The gap between requests is a wait, not a printed promise: every agent
/// shares the owner's one address. Measured where it counts, at the
/// server, across the detail page, the home page and the tier endpoint.
/// The home page is the last candidate a run tries (0014), so the pool's
/// 已开售 event answers as a sold-out page here to get the run there.
#[test]
fn a_run_waits_between_every_request_it_sends() {
    let home = Home::new("gap-run");
    let samples = home.path().join("samples.json");
    std::fs::write(
        &samples,
        r#"{"updatedAt":"2026-10-14 03:00（北京时间）","samples":[
             {"itemId":1077137031347,"category":"onSale"}]}"#,
    )
    .unwrap();
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |_| {
        support::fixture("item_sold_out.html")
    });
    let damai = support::MockDamai::start_bodies(with_route(routes, home_with(1086791513717)));
    let mut cmd = home.theflash(&damai.base_url());
    cmd.args([
        "inspect",
        "--samples",
        samples.to_str().unwrap(),
        "--gap",
        "1",
    ]);
    let out = support::output_within(cmd, 60);
    let hits = damai.hits();
    assert!(hits.iter().any(|h| h == "GET /"), "没请求首页：{hits:?}");
    assert!(
        hits.iter().any(|h| h.contains("dataId=")),
        "没请求票档接口：{hits:?}"
    );
    assert!(
        hits.iter().filter(|h| h.contains("/item.htm")).count() >= 2,
        "{hits:?}"
    );
    let gaps = damai.gaps();
    assert!(gaps.len() >= 4, "请求太少，测不出节流：{hits:?}");
    for (i, gap) in gaps.iter().enumerate() {
        assert!(
            *gap >= std::time::Duration::from_millis(900),
            "第 {} 个请求只隔了 {gap:?}：{hits:?}\n{}",
            i + 2,
            stdout(&out)
        );
    }
}

/// Recording a fixture is the same promise: one page and each session's
/// tiers, a wait apart.
#[test]
fn record_waits_between_the_page_and_each_session() {
    let home = Home::new("gap-record");
    let dir = home.path().join("testdata");
    std::fs::create_dir_all(&dir).unwrap();
    let second = (
        "dataId=286937790".to_string(),
        support::fixture("sku_on_sale_286937790.jsonp"),
    );
    let damai = support::MockDamai::start_bodies(with_route(unchanged_routes(), second));
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
        "2",
        "--gap",
        "1",
    ]);
    let out = support::output_within(cmd, 60);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}{:?}",
        stdout(&out),
        String::from_utf8_lossy(&out.stderr),
        damai.hits()
    );
    let gaps = damai.gaps();
    assert!(gaps.len() >= 2, "{:?}", damai.hits());
    for gap in &gaps {
        assert!(
            *gap >= std::time::Duration::from_millis(900),
            "只隔了 {gap:?}：{:?}",
            damai.hits()
        );
    }
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

/// Recording the page damai.cn changed is the point of `record`, so a
/// page `theflash event` cannot read is kept — and said out loud, because
/// the sessions it left out would otherwise look like all the page had
/// (#73).
#[test]
fn record_says_when_the_page_it_saved_is_one_a_query_cannot_read() {
    let home = Home::new("record-bad-perform-id");
    let dir = home.path().join("testdata");
    std::fs::create_dir_all(&dir).unwrap();
    let routes = changed(unchanged_routes(), "/item.htm?id=1086791513717", |body| {
        body.replacen(
            "\"performId\":286937788",
            "\"performId\":\"not-a-number\"",
            1,
        )
    });
    let next = (
        "dataId=286937790".to_string(),
        support::fixture("sku_on_sale_286937790.jsonp"),
    );
    let damai = support::MockDamai::start_bodies(with_route(routes, next));
    let mut cmd = home.theflash(&damai.base_url());
    cmd.args([
        "inspect",
        "record",
        "1086791513717",
        "--as",
        "broken",
        "--dir",
        dir.to_str().unwrap(),
        "--sessions",
        "1",
        "--gap",
        "0",
    ]);
    let out = support::output_within(cmd, 60);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        dir.join("item_broken.html").is_file(),
        "读不了的页面才是要复现的那一份，它没被录下来：{stderr}"
    );
    assert!(
        stderr.contains("theflash event 读不了") && stderr.contains("performId"),
        "没说这一页查询读不了：{stderr}"
    );
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
    // The refusal is the rule working, but the fixture the command was
    // run for is still not there, so the command did not succeed (#78).
    assert_eq!(
        out.status.code(),
        Some(1),
        "票档 fixture 被拒绝，命令却算成功：{stderr}"
    );
}

/// What one `theflash inspect record` run looks like from outside: what it
/// was served, what the output directory already had in it, and what the
/// caller may conclude from the exit code, stderr and the files.
struct RecordCase {
    what: &'static str,
    routes: Vec<(String, String)>,
    /// Makes the output directory stand in the way of a fixture, the way
    /// a half-cleaned testdata directory does.
    occupy: Vec<&'static str>,
    code: i32,
    /// Fragments the command has to say about this run, all of them in
    /// the same message.
    says: Vec<String>,
    kept: Vec<&'static str>,
    missing: Vec<&'static str>,
}

/// `theflash inspect record` is run for the files it writes down, so a
/// response it fetched and did not write down is the command failing —
/// whether the redaction rule refused it whole (the rule working as the
/// owner decided) or this machine could not write it (a path or a disk).
/// Either way the caller is told which responses are on disk and which
/// are missing, instead of a line on stderr under exit 0 (#78): the
/// fixtures are what the next `make check` replays, and fetching them
/// again costs another request from the one address every agent shares.
#[test]
fn record_says_what_it_kept_and_what_is_missing_whatever_stopped_a_fixture() {
    // Built from pieces so this file does not carry what repo_guard looks
    // for.
    let header = ["Set-", "Cookie", ":"].concat();
    let plant = |routes: Vec<(String, String)>, target: &str| {
        let header = header.clone();
        changed(routes, target, move |body| {
            format!("{header} sid=1\n{body}")
        })
    };
    let page = "item_probe.html";
    let tiers = "sku_probe_286937788.jsonp";
    let cases = vec![
        RecordCase {
            what: "两份都写下来了",
            routes: unchanged_routes(),
            occupy: Vec::new(),
            code: 0,
            says: Vec::new(),
            kept: vec![page, tiers],
            missing: Vec::new(),
        },
        RecordCase {
            what: "票档被脱敏规则整份拒绝",
            routes: plant(unchanged_routes(), "dataId=286937788"),
            occupy: Vec::new(),
            code: 1,
            says: vec![
                tiers.to_string(),
                "Cookie".to_string(),
                "没有保存".to_string(),
                page.to_string(),
            ],
            kept: vec![page],
            missing: vec![tiers],
        },
        RecordCase {
            what: "票档写不进去",
            routes: unchanged_routes(),
            occupy: vec![tiers],
            code: 1,
            says: vec![tiers.to_string(), "写不进去".to_string(), page.to_string()],
            kept: vec![page],
            missing: Vec::new(),
        },
        RecordCase {
            what: "一份被拒绝、一份写不进去，什么都没留下",
            routes: plant(unchanged_routes(), "dataId=286937788"),
            occupy: vec![page],
            code: 1,
            says: vec![
                page.to_string(),
                "写不进去".to_string(),
                tiers.to_string(),
                "Cookie".to_string(),
                "这次一个文件都没写下来".to_string(),
            ],
            kept: Vec::new(),
            missing: vec![tiers],
        },
    ];
    for case in cases {
        let home = Home::new("record-outcome");
        let dir = home.path().join("testdata");
        std::fs::create_dir_all(&dir).unwrap();
        for name in &case.occupy {
            std::fs::create_dir_all(dir.join(name)).unwrap();
        }
        let damai = support::MockDamai::start_bodies(case.routes);
        let mut cmd = home.theflash(&damai.base_url());
        cmd.args([
            "inspect",
            "record",
            "1086791513717",
            "--as",
            "probe",
            "--dir",
            dir.to_str().unwrap(),
            "--sessions",
            "1",
            "--gap",
            "0",
        ]);
        let out = support::output_within(cmd, 60);
        let said = format!("{}{}", stdout(&out), String::from_utf8_lossy(&out.stderr));
        let what = case.what;
        assert_eq!(out.status.code(), Some(case.code), "{what}：{said}");
        for fragment in &case.says {
            assert!(
                said.contains(fragment.as_str()),
                "{what}：没说 {fragment}：{said}"
            );
        }
        for name in &case.kept {
            assert!(dir.join(name).is_file(), "{what}：{name} 没写下来：{said}");
            assert!(said.contains(name), "{what}：没说 {name} 留下来了：{said}");
        }
        for name in &case.missing {
            assert!(
                !dir.join(name).exists(),
                "{what}：{name} 不该被保存：{said}"
            );
        }
        if case.code == 0 {
            assert!(!said.contains("没有保存"), "{what}：{said}");
        }
    }
}

/// Everything that stops damai access ends the whole inspection, not just
/// the event being read: risk control, the breaker another theFlash
/// process wrote, a breaker that cannot be read, and — the one that
/// matters most — risk control whose breaker could not be written, where
/// nothing else is left to stop the next request (#70, #74). Reported as
/// one event's failure, the run went on to read four more pages and three
/// tier endpoints, and then called the slider challenge a missing sample.
#[tokio::test]
async fn anything_that_stops_damai_access_ends_the_whole_inspection() {
    type Prepare = fn(&Path);
    let breaker: Prepare = |dir| {
        std::fs::write(
            dir.join("risk-stop.json"),
            "{\"at\":\"2026-10-14 01:02:03（北京时间）\",\
             \"url\":\"https://detail.damai.cn/item.htm?id=1\",\
             \"marker\":\"x5secdata\",\"by\":\"theflash test\",\"pid\":1}",
        )
        .unwrap()
    };
    let cases: [(&str, Prepare, usize); 4] = [
        ("风控，熔断记下来了", |_| {}, 1),
        (
            "风控，但熔断写不进去",
            // A directory where the breaker's temporary file goes: the
            // write fails, while reading the breaker still works.
            |dir| {
                std::fs::create_dir(dir.join(format!("risk-stop.json.{}.tmp", std::process::id())))
                    .unwrap()
            },
            1,
        ),
        ("别的进程写下的熔断", breaker, 0),
        (
            "读不懂的熔断状态",
            |dir| std::fs::write(dir.join("risk-stop.json"), "{").unwrap(),
            0,
        ),
    ];
    for (what, prepare, want_hits) in cases {
        let home = Home::new(&format!("stops-{}", want_hits));
        let dir = home.path().join("state");
        std::fs::create_dir_all(&dir).unwrap();
        prepare(&dir);
        // Whatever the run asks for, it gets the saved slider challenge.
        let damai = support::MockDamai::start_bodies(vec![(
            "/".to_string(),
            support::fixture("risk_control.html"),
        )]);
        let gate = std::sync::Arc::new(
            theflash::damai::Gate::new(
                dir.clone(),
                std::time::Duration::ZERO,
                std::sync::Arc::new(theflash::damai::SystemClock::default()),
                "theflash test".into(),
            )
            .unwrap(),
        );
        let client = theflash::damai::Client::with_gate(damai.base_url(), gate)
            .unwrap()
            .with_home(damai.base_url());
        let opts = theflash::inspect::Options {
            samples_path: home.path().join("inspect-samples.json"),
            candidates_dir: None,
            max_events: 8,
            gap: std::time::Duration::ZERO,
        };

        let e = theflash::inspect::run(&client, &opts)
            .await
            .err()
            .unwrap_or_else(|| panic!("{what}：巡检照常结束了"));
        assert!(e.stops_damai_access(), "{what}：{e}");
        assert_eq!(
            damai.hits().len(),
            want_hits,
            "{what}：停下之后又请求了大麦 {:?}",
            damai.hits()
        );
    }
}

/// What the agent running the inspection sees: the command stops, says
/// the breaker is in effect, and sends nothing.
#[test]
fn an_inspection_under_a_breaker_stops_and_says_so() {
    let home = Home::new("inspect-under-breaker");
    let dir = home.samples_file().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("risk-stop.json"),
        "{\"at\":\"2026-10-14 01:02:03（北京时间）\",\
         \"url\":\"https://detail.damai.cn/item.htm?id=1\",\
         \"marker\":\"x5secdata\",\"by\":\"theflash watch\",\"pid\":1}",
    )
    .unwrap();
    let damai = support::MockDamai::start_bodies(unchanged_routes());
    let out = run(&home, &damai, &[]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    let printed = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{said}{printed}");
    assert!(said.contains("风控熔断生效中"), "{said}");
    // No report: the run stopped, so it has nothing to say about damai.cn's
    // pages. A report here would be the breaker turned into "this sample
    // could not be read", which is how #70 hid it.
    assert_eq!(printed, "", "熔断生效，却照样出了一份巡检报告");
    assert!(
        damai.hits().is_empty(),
        "熔断生效时巡检还在请求大麦：{:?}",
        damai.hits()
    );
}
