//! The declared baseline against the real responses in
//! `backend/testdata/damai/`: every field theFlash depends on is really
//! there, with a kind it can read. Without this the declaration would be
//! what an agent remembered rather than what damai.cn sends, and a run
//! would report changes that never happened.

use serde_json::Value;

use super::*;
use crate::damai::tier_answer_problem;

const ITEM_FIXTURES: [&str; 5] = [
    "item_on_sale.html",
    "item_not_yet_on_sale.html",
    "item_sold_out.html",
    "item_calendar.html",
    "item_pc_unsupported.html",
];

const TIER_FIXTURES: [&str; 6] = [
    "sku_on_sale_286937788.jsonp",
    "sku_on_sale_286937789.jsonp",
    "sku_on_sale_286937790.jsonp",
    "sku_not_yet_on_sale_286966465.jsonp",
    "sku_calendar_286938129.jsonp",
    "sku_sold_out_284490009.jsonp",
];

fn fixture(name: &str) -> String {
    let path = format!("{}/testdata/damai/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn item_json(name: &str) -> Value {
    item_page_json(&fixture(name))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .unwrap_or_else(|| panic!("{name} 里没有 dataDefault"))
}

fn tier_json_of(name: &str) -> Value {
    tier_json(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn changes_in(root: &Value, fields: &'static [Field]) -> Vec<Change> {
    let mut changes = Vec::new();
    collect(root, fields, &mut changes);
    changes
}

/// Nothing theFlash needs is missing or of a kind it cannot read in any of
/// the responses damai.cn really sent. A declaration that said otherwise
/// would make every run report a change.
#[test]
fn the_saved_responses_all_match_the_declared_structure() {
    for name in ITEM_FIXTURES {
        let changes = changes_in(&item_json(name), shape::ITEM_PAGE_FIELDS);
        assert!(
            changes.is_empty(),
            "{name}: {:?}",
            changes.iter().map(Change::to_string).collect::<Vec<_>>()
        );
    }
    for name in TIER_FIXTURES {
        let changes = changes_in(&tier_json_of(name), shape::TIER_FIELDS);
        assert!(
            changes.is_empty(),
            "{name}: {:?}",
            changes.iter().map(Change::to_string).collect::<Vec<_>>()
        );
    }
}

/// `Need::Optional` says damai.cn leaves a field out for some events. The
/// evidence for that is a saved response without it; without such a
/// response the field should be `Always`, so that losing it is reported.
#[test]
fn every_optional_field_is_missing_from_a_saved_response() {
    let check = |fields: &'static [Field], jsons: Vec<Value>| {
        for field in fields.iter().filter(|f| f.need == shape::Need::Optional) {
            let ever_absent = jsons.iter().any(|json| {
                let o = shape::look(json, field);
                !o.absent.is_empty()
            });
            assert!(
                ever_absent,
                "{} 在每个 fixture 里都有：它应该是 Need::Always，不见了要报出来",
                field.path
            );
        }
    };
    check(
        shape::ITEM_PAGE_FIELDS,
        ITEM_FIXTURES.iter().map(|n| item_json(n)).collect(),
    );
    check(
        shape::TIER_FIELDS,
        TIER_FIXTURES.iter().map(|n| tier_json_of(n)).collect(),
    );
}

/// Every situation's saved page is classified as that situation, so a real
/// page that is not tells the run its sample went stale.
#[test]
fn the_saved_pages_are_classified_as_the_situations_they_were_recorded_for() {
    for (name, category) in [
        ("item_on_sale.html", Category::OnSale),
        ("item_not_yet_on_sale.html", Category::NotYetOnSale),
        ("item_sold_out.html", Category::SoldOut),
        ("item_calendar.html", Category::Calendar),
        ("item_pc_unsupported.html", Category::NotOnPcChannel),
    ] {
        assert_eq!(
            samples::classify(&item_json(name)),
            Some(category),
            "{name}"
        );
    }
}

/// The ids in the seed sample set are the events the fixtures came from,
/// so a first run on a new machine looks at pages theFlash was built on.
#[test]
fn the_seed_samples_are_the_events_the_fixtures_were_recorded_from() {
    for (name, category) in [
        ("item_on_sale.html", Category::OnSale),
        ("item_not_yet_on_sale.html", Category::NotYetOnSale),
        ("item_sold_out.html", Category::SoldOut),
        ("item_calendar.html", Category::Calendar),
        ("item_pc_unsupported.html", Category::NotOnPcChannel),
    ] {
        let json = item_json(name);
        let item_id = json["staticDataDefault"]["itemBase"]["itemId"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| json["staticDataDefault"]["itemBase"]["itemId"].as_u64())
            .unwrap_or_else(|| panic!("{name} 里没有 itemId"));
        let seed = samples::seed()
            .into_iter()
            .find(|s| s.category == category)
            .unwrap();
        assert_eq!(seed.item_id, item_id, "{name}");
    }
}

/// What #10 asks for: a response with `performBases` taken out reports the
/// field, and says what theFlash used it for.
#[test]
fn a_page_without_perform_bases_reports_the_field_that_went_away() {
    let mut json = item_json("item_on_sale.html");
    json["dataDefault"]
        .as_object_mut()
        .unwrap()
        .remove("performBases")
        .expect("the saved page has performBases");
    let changes = changes_in(&json, shape::ITEM_PAGE_FIELDS);
    let said: Vec<String> = changes.iter().map(Change::to_string).collect();
    assert_eq!(changes.len(), 1, "{said:?}");
    assert_eq!(changes[0].what, What::NoneOfGroup);
    assert!(
        said[0].contains("dataDefault.performBases") && said[0].contains("场次列表"),
        "{said:?}"
    );
}

/// A field theFlash always needs: gone from the one place it belongs, and
/// the report says where it was looked for.
#[test]
fn a_field_that_is_always_there_is_reported_where_it_went_missing() {
    let mut json = item_json("item_on_sale.html");
    json["staticDataDefault"]["itemBase"]
        .as_object_mut()
        .unwrap()
        .remove("itemName")
        .expect("the saved page has itemName");
    let changes = changes_in(&json, shape::ITEM_PAGE_FIELDS);
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(
        changes[0].what,
        What::Gone {
            at: "staticDataDefault.itemBase.itemName".into()
        }
    );
    assert!(
        changes[0].to_string().contains("演出名称"),
        "{}",
        changes[0]
    );
}

/// A field that is still there but of another kind: theFlash would read it
/// as 未知 instead of failing, so nothing else would notice.
#[test]
fn a_field_that_changed_kind_is_reported_with_what_is_there_now() {
    let mut json = item_json("item_on_sale.html");
    json["dataDefault"]["performBases"][0]["performs"][0]["performId"] = Value::Null;
    let changes = changes_in(&json, shape::ITEM_PAGE_FIELDS);
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(
        changes[0].what,
        What::Changed {
            at: "dataDefault.performBases[0].performs[0].performId".into(),
            got: Kind::Null,
        }
    );
    assert!(
        changes[0].to_string().contains("变成了null"),
        "{}",
        changes[0]
    );
}

/// An array element that stopped being an object takes every field under
/// it away: `perform.skuList[0]` turning into null leaves `priceName`,
/// `price` and `frontEndStatus` with nowhere to be, so without a word
/// about the element itself the whole tier list would go missing quietly.
#[test]
fn a_tier_that_is_no_longer_an_object_is_reported_with_its_index() {
    let mut json = tier_json_of("sku_on_sale_286937790.jsonp");
    json["perform"]["skuList"] = Value::from(vec![Value::Null]);
    let changes = changes_in(&json, shape::TIER_FIELDS);
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(
        changes[0].what,
        What::Changed {
            at: "perform.skuList[0]".into(),
            got: Kind::Null,
        }
    );
    assert!(
        changes[0].to_string().contains("一个票档"),
        "{}",
        changes[0]
    );
}

/// The same for the arrays the sessions are listed in, on both kinds of
/// page: an element that is not an object is where theFlash would lose
/// every session it can ask tiers for.
#[test]
fn a_session_array_element_that_is_no_longer_an_object_is_reported() {
    let mut json = item_json("item_on_sale.html");
    json["dataDefault"]["performBases"][0]["performs"] = Value::from(vec![Value::from(7)]);
    let changes = changes_in(&json, shape::ITEM_PAGE_FIELDS);
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(
        changes[0].what,
        What::Changed {
            at: "dataDefault.performBases[0].performs[0]".into(),
            got: Kind::Number,
        }
    );

    let mut calendar = item_json("item_calendar.html");
    calendar["dataDefault"]["calendarPerforms"] = Value::from(vec![Value::Null]);
    let changes = changes_in(&calendar, shape::ITEM_PAGE_FIELDS);
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(
        changes[0].what,
        What::Changed {
            at: "dataDefault.calendarPerforms[0]".into(),
            got: Kind::Null,
        }
    );
}

/// damai.cn can answer the tier endpoint with a body of exactly the right
/// shape that says the query failed, or one about another session.
/// `damai::Client::event` refuses both, so an inspection that only looked
/// at the shape would call a response no user could buy from "unchanged".
#[test]
fn a_tier_response_that_answers_no_one_is_a_finding_even_with_every_field_in_place() {
    let good = tier_json_of("sku_on_sale_286937788.jsonp");
    assert_eq!(tier_answer_problem(&good, 286937788), None);
    assert!(changes_in(&good, shape::TIER_FIELDS).is_empty());

    let mut failed = good.clone();
    failed["responseInfo"]["responseSuccess"] = Value::from("false");
    failed["responseInfo"]["responseCode"] = Value::from("X1");
    assert!(
        changes_in(&failed, shape::TIER_FIELDS).is_empty(),
        "字段都在，只有语义变了"
    );
    let why = tier_answer_problem(&failed, 286937788).expect("接口说自己失败了");
    assert!(why.contains("X1"), "{why}");

    let other = tier_answer_problem(&good, 286937790).expect("返回的是另一个场次");
    assert!(other.contains("286937788"), "{other}");
}

/// A page lists its sessions in `performBases` or, on a calendar page, in
/// `calendarPerforms`. Neither alone is required, and a page with neither
/// has no sessions theFlash can find, which is a change.
#[test]
fn a_page_needs_one_of_the_two_places_sessions_are_listed() {
    let calendar = item_json("item_calendar.html");
    assert!(
        calendar["dataDefault"].get("performBases").is_none(),
        "日历页本来就没有 performBases"
    );
    assert!(changes_in(&calendar, shape::ITEM_PAGE_FIELDS).is_empty());

    let mut neither = calendar.clone();
    neither["dataDefault"]
        .as_object_mut()
        .unwrap()
        .remove("calendarPerforms")
        .expect("日历页有 calendarPerforms");
    let changes = changes_in(&neither, shape::ITEM_PAGE_FIELDS);
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(changes[0].what, What::NoneOfGroup);
    assert!(
        changes[0].to_string().contains("performBases")
            && changes[0].to_string().contains("calendarPerforms"),
        "{}",
        changes[0]
    );
}

/// What #10 asks for: a status code nobody mapped is reported with the
/// words damai.cn shows next to it.
#[test]
fn a_page_with_status_code_777_reports_it_as_one_nobody_has_seen() {
    let mut json = item_json("item_on_sale.html");
    json["dataDefault"]["buyBtnStatus"] = Value::from(777);
    let unseen = codes::unseen_on_item_page(&json);
    assert_eq!(unseen.len(), 1, "{unseen:?}");
    assert_eq!(unseen[0].code, "777");
    assert_eq!(unseen[0].text.as_deref(), Some("立即购票"));
    assert!(changes_in(&json, shape::ITEM_PAGE_FIELDS).is_empty());
    assert_eq!(samples::classify(&json), None, "没映射的状态码也就归不了类");
}

#[test]
fn a_tier_with_status_code_777_reports_it_as_one_nobody_has_seen() {
    let mut json = tier_json_of("sku_on_sale_286937790.jsonp");
    json["perform"]["skuList"][0]["frontEndStatus"] = Value::from("777");
    let unseen = codes::unseen_on_tiers(&json);
    assert_eq!(unseen.len(), 1, "{unseen:?}");
    assert_eq!(unseen[0].code, "777");
    assert_eq!(unseen[0].text.as_deref(), Some("缺货登记"));
    assert!(unseen[0].at.contains("D-118元"), "{}", unseen[0].at);
}

/// The saved responses hold no unmapped code, so a run against unchanged
/// pages says nothing about status codes.
#[test]
fn the_saved_responses_hold_no_status_code_theflash_has_not_mapped() {
    for name in ITEM_FIXTURES {
        assert_eq!(codes::unseen_on_item_page(&item_json(name)), [], "{name}");
    }
    for name in TIER_FIXTURES {
        assert_eq!(codes::unseen_on_tiers(&tier_json_of(name)), [], "{name}");
    }
}

#[test]
fn the_first_session_of_a_page_is_the_one_whose_tiers_are_read() {
    assert_eq!(
        first_perform_id(&item_json("item_on_sale.html")),
        Some(286937788),
        "页面上第一个列出来的场次，不按日期排"
    );
    assert_eq!(
        first_perform_id(&item_json("item_calendar.html")),
        Some(286938129),
        "日历页的场次在 calendarPerforms 里"
    );
    assert_eq!(
        first_perform_id(&item_json("item_pc_unsupported.html")),
        None
    );
}

#[test]
fn a_report_with_no_finding_says_so_and_one_with_a_finding_names_it() {
    let clean = Report {
        at: "2026-10-14 04:00（北京时间）".into(),
        requests: 11,
        events: vec![EventReport {
            item_id: 1086791513717,
            name: Some("甲".into()),
            category: Some(Category::OnSale),
            replaced: None,
            changes: Vec::new(),
            unseen: Vec::new(),
            checked: true,
            session: Some(286937789),
            failed: None,
            candidates: Vec::new(),
        }],
        uncovered: Vec::new(),
        expiring: Vec::new(),
        samples_path: PathBuf::from("/tmp/inspect-samples.json"),
        unsaved: Vec::new(),
        notes: Vec::new(),
    };
    assert!(!clean.has_findings());
    let text = render(&clean);
    assert!(text.contains("结论：没有变化。"), "{text}");
    assert!(text.contains("发了 11 个请求"), "{text}");

    let mut found = clean;
    found.events[0].changes.push(Change {
        path: "dataDefault.performBases",
        accepts: &[Kind::Array],
        used_for: "场次列表",
        what: What::Gone {
            at: "dataDefault.performBases".into(),
        },
    });
    assert!(found.has_findings());
    let text = render(&found);
    assert!(
        text.contains("结构变化：dataDefault.performBases 不见了"),
        "{text}"
    );
    assert!(issue_title(&found).contains("1 处结构变化"));
    assert!(issue_body(&found).contains("dataDefault.performBases"));
}

/// A situation with no candidate left is a finding on its own: the
/// inspection has stopped checking that situation, which is what #39 was
/// opened for.
#[test]
fn a_situation_nothing_covers_is_a_finding_and_a_budget_cut_is_not() {
    let base = Report {
        at: "2026-10-14 04:00（北京时间）".into(),
        requests: 9,
        events: Vec::new(),
        uncovered: Vec::new(),
        expiring: Vec::new(),
        samples_path: PathBuf::from("/tmp/inspect-samples.json"),
        unsaved: Vec::new(),
        notes: Vec::new(),
    };

    let mut out_of_budget = Report { ..base };
    out_of_budget
        .uncovered
        .push((Category::SoldOut, Uncovered::OutOfBudget));
    assert!(!out_of_budget.has_findings());
    assert!(render(&out_of_budget).contains("这次没查已售罄"));

    let mut nothing_left = out_of_budget;
    nothing_left.uncovered.push((
        Category::Calendar,
        Uncovered::NothingLeft {
            was: "1084537866279 不再是这个状态".into(),
        },
    ));
    assert!(nothing_left.has_findings());
    assert_eq!(nothing_left.nothing_left(), 1);
    let text = render(&nothing_left);
    assert!(text.contains("样本缺口：没有日历选日期的样本了"), "{text}");
    assert!(
        text.contains("1084537866279 不再是这个状态"),
        "没说原来那一场怎么了：{text}"
    );
    assert!(
        text.contains("/tmp/inspect-samples.json"),
        "没说要改哪个文件：{text}"
    );
    assert!(issue_title(&nothing_left).contains("1 类样本补不上"));
    let body = issue_body(&nothing_left);
    assert!(body.contains("候选池"), "{body}");
    assert!(
        !body.contains("在真实大麦上发现了变化"),
        "补不上样本不是大麦改版：{body}"
    );
    assert!(
        !body.contains("用候选 fixture 复现"),
        "没有结构变化就没有候选 fixture 要复现：{body}"
    );
}

/// The notice that goes out before a sample stops covering its
/// situation, which is the only warning anyone gets: replacements are
/// found by hand (0014).
#[test]
fn a_sample_about_to_stop_covering_its_situation_is_a_finding() {
    let mut report = Report {
        at: "2026-10-14 04:00（北京时间）".into(),
        requests: 9,
        events: Vec::new(),
        uncovered: Vec::new(),
        expiring: Vec::new(),
        samples_path: PathBuf::from("/tmp/inspect-samples.json"),
        unsaved: Vec::new(),
        notes: Vec::new(),
    };
    report.expiring.push(Expiring {
        item_id: 1092337348536,
        category: Category::NotYetOnSale,
        name: Some("甲".into()),
        at: "2026-10-14 10:18（北京时间）".into(),
    });
    assert!(report.has_findings());
    let text = render(&report);
    assert!(text.contains("样本快过期：1092337348536 甲"), "{text}");
    assert!(text.contains("2026-10-14 10:18（北京时间）"), "{text}");
    assert!(text.contains("之后就不是未开售了"), "{text}");
    assert!(issue_title(&report).contains("1 个样本快过期"));
}

#[test]
fn findings_are_what_decides_whether_an_issue_is_opened() {
    struct Never;
    impl tracker::Tracker for Never {
        fn open_issues(&self) -> Result<Vec<tracker::Issue>, String> {
            panic!("没有发现变化的时候不该碰 GitHub")
        }
        fn create(&self, _: &str, _: &str) -> Result<String, String> {
            panic!("没有发现变化的时候不该碰 GitHub")
        }
        fn comment(&self, _: u64, _: &str) -> Result<String, String> {
            panic!("没有发现变化的时候不该碰 GitHub")
        }
    }
    let clean = Report {
        at: "2026-10-14 04:00（北京时间）".into(),
        requests: 1,
        events: Vec::new(),
        uncovered: Vec::new(),
        expiring: Vec::new(),
        samples_path: PathBuf::from("/tmp/inspect-samples.json"),
        unsaved: Vec::new(),
        notes: vec!["说明不算发现".into()],
    };
    assert_eq!(report_findings(&clean, &Never).unwrap(), None);

    // A situation with no sample left is a finding of its own: nobody is
    // watching it until an agent hands it a new event.
    let lost = Report {
        uncovered: vec![(
            Category::OnSale,
            Uncovered::NothingLeft {
                was: "1077137031347 不再是这个状态".into(),
            },
        )],
        ..clean
    };
    assert!(lost.has_findings());
    assert!(
        render(&lost).contains("样本缺口：没有已开售的样本了"),
        "{}",
        render(&lost)
    );
    assert!(issue_title(&lost).contains("1 类样本补不上"));
}

/// A run that could not write its result down is not a success: the next
/// run starts again from the samples this one was replacing, and whoever
/// scheduled it reads the exit code, not the report. It is said as what it
/// is — a path on this machine — because calling a disk error a damai.cn
/// revision sends the next agent to the wrong place (#77).
#[test]
fn a_result_that_did_not_reach_the_disk_is_a_finding_and_is_not_called_a_revision() {
    let clean = Report {
        at: "2026-10-14 04:00（北京时间）".into(),
        requests: 9,
        events: vec![EventReport {
            item_id: 1086791513717,
            name: Some("甲".into()),
            category: Some(Category::OnSale),
            replaced: None,
            changes: Vec::new(),
            unseen: Vec::new(),
            checked: true,
            session: Some(286937789),
            failed: None,
            candidates: Vec::new(),
        }],
        uncovered: Vec::new(),
        expiring: Vec::new(),
        samples_path: PathBuf::from("/tmp/blocked/inspect-samples.json"),
        unsaved: Vec::new(),
        notes: Vec::new(),
    };
    assert!(!clean.has_findings());

    let mut unsaved = clean;
    unsaved.unsaved.push(
        "样本没保存下来：/tmp/blocked 建不出来：File exists (os error 17)。\
         这次定下来的样本没有留下，下一次巡检还是按上一次的样本跑；\
         腾出这个路径，或者用 --samples 指一个能写的文件，再跑一次"
            .into(),
    );
    assert!(unsaved.has_findings(), "样本没保存下来，这次巡检却算成功了");
    let text = render(&unsaved);
    assert!(text.contains("/tmp/blocked 建不出来"), "{text}");
    assert!(text.contains("--samples"), "没说该怎么办：{text}");
    assert!(
        !text.contains("结论：没有变化。"),
        "结果没保存下来，结论却说这次没有变化：{text}"
    );
    assert!(
        text.contains("不是大麦改版"),
        "本机的磁盘问题要说成本机的：{text}"
    );
    assert!(issue_title(&unsaved).contains("结果没保存下来"));
    let body = issue_body(&unsaved);
    assert!(
        !body.contains("在真实大麦上发现了变化"),
        "写不下来不是大麦改版：{body}"
    );
    assert!(
        body.contains("/tmp/blocked/inspect-samples.json"),
        "issue 里没说样本集在哪：{body}"
    );
}

/// Asking for requests closer together than the real site's floor is only
/// answered for a replay server on this machine. Anything else — the real
/// damai.cn, another host, a value that is not a URL at all — waits the
/// full gap: every agent shares the owner's one address, and whether the
/// requests leave this machine is decided by where they go, not by whether
/// an override happens to be set.
#[test]
fn only_a_replay_server_on_this_machine_may_be_asked_for_a_shorter_gap() {
    for local in [
        "http://127.0.0.1:8080",
        "http://127.0.0.1:8080/",
        "http://localhost:8080",
        "http://[::1]:8080",
    ] {
        assert_eq!(
            request_gap(Some(0), Some(local)),
            (Duration::ZERO, None),
            "{local}"
        );
    }
    for elsewhere in [
        damai::DEFAULT_BASE_URL,
        damai::DEFAULT_HOME_URL,
        "http://10.0.0.7:8080",
        "http://127.0.0.1.example.com",
        "127.0.0.1:8080",
        "",
        "\u{fffd}",
    ] {
        let (gap, refused) = request_gap(Some(0), Some(elsewhere));
        assert_eq!(gap, DEFAULT_REQUEST_GAP, "{elsewhere}");
        assert!(refused.is_some(), "{elsewhere} 被放行了，没有说明原因");
    }
    assert_eq!(
        request_gap(Some(0), None).0,
        DEFAULT_REQUEST_GAP,
        "没有覆盖地址就是真实大麦"
    );
    // A longer gap is always the run's own business, and no gap asked for
    // is the floor.
    assert_eq!(
        request_gap(Some(30), Some(damai::DEFAULT_BASE_URL)),
        (Duration::from_secs(30), None)
    );
    assert_eq!(
        request_gap(None, Some("http://127.0.0.1:1")).0,
        DEFAULT_REQUEST_GAP
    );
}

/// The state directory is spelled out, so a test fails if it moves: the
/// sample set lives beside theFlash's other state.
#[test]
fn the_sample_set_lives_under_the_users_state_directory() {
    let home = Path::new("/home/someone");
    assert_eq!(
        state_dir_for(true, home, None),
        Path::new("/home/someone/Library/Application Support/theFlash")
    );
    assert_eq!(
        state_dir_for(false, home, None),
        Path::new("/home/someone/.local/state/theflash")
    );
    assert_eq!(
        state_dir_for(false, home, Some("/var/state".into())),
        Path::new("/var/state/theflash")
    );
    assert_eq!(
        state_dir_for(false, home, Some("relative".into())),
        Path::new("/home/someone/.local/state/theflash"),
        "相对路径的 XDG_STATE_HOME 按规范忽略"
    );
}
