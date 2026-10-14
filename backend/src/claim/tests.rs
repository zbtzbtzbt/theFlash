//! 租约怎么读、什么时候算过期、什么时候拒绝认领。
//!
//! The bodies here are the ones agents write (the demo on issue #31 and
//! the claim on #16), plus the sloppier spellings of the same thing:
//! parsing is forgiving on purpose, and a test says exactly how far.

use super::*;
use crate::claim::lease::{EXPIRES_AFTER_MINUTES, HEARTBEAT_EVERY_MINUTES};
use crate::claim::snapshot::Comment;

fn at(text: &str) -> DateTime<Utc> {
    lease::parse_time(text).unwrap_or_else(|| panic!("{text} should parse"))
}

/// The claim the `claims` command posted on issue #31, byte for byte.
const DEMO_CLAIM: &str = "## 认领\n\n- agent: DemoAgent\n- 开始时间: 2026-10-14T03:05:55+08:00\n\
     - 改动模块: docs/demo/claim-lease-demo.md\n- 预计完成: 2026-10-14T05:00:00+08:00\n\
     - 分支: demo/claim-lease\n\n心跳至少每 30 分钟一次。\n\nAgent: DemoAgent\n";

fn comment(body: &str, posted_at: &str) -> Comment {
    comment_id(body, posted_at, 0)
}

/// A comment with GitHub's own id, which orders two comments posted in
/// the same second.
fn comment_id(body: &str, posted_at: &str, id: u64) -> Comment {
    Comment {
        id,
        url: format!("https://github.com/zbtzbtzbt/theFlash/issues/31#issuecomment-{posted_at}"),
        posted_at: at(posted_at),
        body: body.to_string(),
    }
}

/// A comment on issue #31, as GitHub returned it. `which` is its id in
/// `testdata/github/claims/two-claims/comments-31-with-demo-record.json`,
/// so a test about what agents really write runs on the real bytes.
fn real_comment(which: u64) -> Comment {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/github/claims/two-claims/comments-31-with-demo-record.json"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let list: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    let found = list
        .iter()
        .find(|c| c["id"] == which)
        .unwrap_or_else(|| panic!("{path} 里没有 id {which} 这条评论"));
    Comment {
        id: which,
        url: found["html_url"].as_str().unwrap().to_string(),
        posted_at: at(found["created_at"].as_str().unwrap()),
        body: found["body"].as_str().unwrap().to_string(),
    }
}

fn issue(labels: &[&str], comments: Vec<Comment>) -> Issue {
    Issue {
        number: 31,
        title: "测试：认领租约演示".to_string(),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        comments,
    }
}

fn board_of(issue: Issue) -> Board {
    board(&Snapshot {
        issues: vec![issue],
        pushes: BTreeMap::new(),
    })
}

#[test]
fn a_claim_says_which_agent_changes_which_modules() {
    let claim = lease::parse_claim(DEMO_CLAIM, "u", at("2026-10-14T03:05:55+08:00")).unwrap();
    assert_eq!(claim.agent, "DemoAgent");
    assert_eq!(claim.modules, ["docs/demo/claim-lease-demo.md"]);
    assert_eq!(claim.branch.as_deref(), Some("demo/claim-lease"));
    assert_eq!(claim.due, Some(at("2026-10-14T05:00:00+08:00")));
    assert_eq!(claim.missing_field(), None);
}

#[test]
fn a_claim_reads_the_same_however_an_agent_spells_it() {
    let hand_written = "### 认领：\n\
        * **agent**：Codex\n\
        * 预计改动的模块或目录： `backend/src/damai`、backend/src/watch/\n\
        * 预计完成：2026-10-14 08:30:00+0800\n\
        * branch: codex/issue-11\n";
    let claim = lease::parse_claim(hand_written, "u", at("2026-10-14T03:00:00+08:00")).unwrap();
    assert_eq!(claim.agent, "Codex");
    assert_eq!(claim.modules, ["backend/src/damai", "backend/src/watch"]);
    assert_eq!(claim.due, Some(at("2026-10-14T08:30:00+08:00")));
    assert_eq!(claim.branch.as_deref(), Some("codex/issue-11"));
}

#[test]
fn a_claim_without_modules_is_missing_a_field_not_a_claim_without_modules() {
    let claim = lease::parse_claim(
        "## 认领\n\n- agent: Claude\n- 开始时间: 2026-10-14T03:00:00+08:00\n",
        "u",
        at("2026-10-14T03:00:00+08:00"),
    )
    .unwrap();
    assert_eq!(claim.missing_field(), Some("改动模块"));
}

#[test]
fn prose_about_claiming_is_not_a_claim() {
    for body in [
        "我来做这个，认领了",
        "## 进度\n\n认领: Claude\n",
        "## 收回认领\n\n- 原认领: DemoAgent（u）\n",
        "认领：\n我来做这个 issue，先看看要改哪里，等会儿用 scripts/claims 正式认领。\n\nAgent: Codex\n",
    ] {
        assert_eq!(
            lease::parse_claim(body, "u", at("2026-10-14T03:00:00+08:00")),
            None,
            "{body:?} should not read as a claim"
        );
    }
}

#[test]
fn a_time_without_a_zone_is_missing_rather_than_eight_hours_off() {
    assert_eq!(lease::parse_time("2026-10-14 05:00"), None);
    assert_eq!(
        lease::parse_time("2026-10-14T05:00:00+08:00"),
        lease::parse_time("2026-10-13T21:00:00Z")
    );
}

#[test]
fn a_signed_progress_comment_is_not_a_session_heartbeat() {
    let beat = lease::parse_heartbeat(
        "CI 绿了，在补测试。\n\nAgent: DemoAgent\n",
        "u",
        at("2026-10-14T03:30:00+08:00"),
    )
    .unwrap();
    assert_eq!(beat.agent, "DemoAgent");
    assert!(!beat.marked);
    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![
            comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00"),
            comment(
                "CI 绿了，在补测试。\n\nAgent: DemoAgent\n",
                "2026-10-14T03:30:00+08:00",
            ),
        ],
    ));
    assert_eq!(
        board.lease(31).unwrap().last.at,
        at("2026-10-14T03:05:55+08:00"),
        "厂商签名分不出同名会话；只有绑定认领 URL 的结构化心跳能续租"
    );
    assert_eq!(
        lease::parse_heartbeat(DEMO_CLAIM, "u", at("2026-10-14T03:05:55+08:00")),
        None
    );
    assert_eq!(
        lease::parse_heartbeat("别人路过说一句", "u", at("2026-10-14T03:30:00+08:00")),
        None
    );
}

#[test]
fn an_agent_name_with_a_suffix_is_the_same_agent() {
    assert!(lease::same_agent("Claude", "claude（第 4 条线）"));
    assert!(lease::same_agent("Codex", "codex"));
    assert!(!lease::same_agent("Claude", "CodeBuddy"));
    assert!(!lease::same_agent("", ""));
}

#[test]
fn a_directory_covers_what_is_under_it_but_not_what_is_next_to_it() {
    let held = ["backend/src/claim".to_string(), "AGENTS.md".to_string()];
    assert_eq!(
        lease::overlap(&held, &["backend/src/claim/mod.rs".to_string()]),
        ["backend/src/claim/mod.rs"]
    );
    assert_eq!(
        lease::overlap(&held, &["AGENTS.md".to_string()]),
        ["AGENTS.md"]
    );
    assert!(lease::overlap(&held, &["backend/src/claims.rs".to_string()]).is_empty());
    assert!(lease::overlap(&held, &["backend/src/watch".to_string()]).is_empty());
}

#[test]
fn a_lease_expires_after_two_hours_of_silence() {
    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00")],
    ));
    let held = board.lease(31).expect("the claim reads as a lease");
    assert_eq!(held.status(at("2026-10-14T03:20:00+08:00")), Status::Active);
    assert_eq!(
        held.status(at("2026-10-14T03:40:00+08:00")),
        Status::HeartbeatOverdue
    );
    assert_eq!(
        held.status(at("2026-10-14T05:06:00+08:00")),
        Status::Expired,
        "{EXPIRES_AFTER_MINUTES} 分钟没有更新之后应该算过期"
    );
    assert_eq!(held.expires_in_minutes(at("2026-10-14T04:05:55+08:00")), 60);
}

#[test]
fn a_heartbeat_or_a_push_pushes_expiry_back() {
    let claim_msg = comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00");
    let beat = format!(
        "## 心跳\n\n- agent: DemoAgent\n- 认领: {}\n- 进度: 在补测试\n\nAgent: DemoAgent\n",
        claim_msg.url
    );
    let mut snapshot = Snapshot {
        issues: vec![issue(
            &[lease::CLAIM_LABEL],
            vec![claim_msg, comment(&beat, "2026-10-14T04:30:00+08:00")],
        )],
        pushes: BTreeMap::new(),
    };
    let now = at("2026-10-14T05:06:00+08:00");
    let held = board(&snapshot).lease(31).cloned().unwrap();
    assert_eq!(
        held.status(now),
        Status::HeartbeatOverdue,
        "心跳之后 {} 分钟：该再心跳了，但还没到过期",
        HEARTBEAT_EVERY_MINUTES
    );
    assert_eq!(held.last.at, at("2026-10-14T04:30:00+08:00"));
    assert!(matches!(
        held.last.sign,
        Sign::Heartbeat { marked: true, .. }
    ));

    snapshot.pushes.insert(
        "demo/claim-lease".to_string(),
        Push::At(at("2026-10-14T05:00:00+08:00")),
    );
    let held = board(&snapshot).lease(31).cloned().unwrap();
    assert_eq!(held.status(now), Status::Active, "刚推过分支就是活跃的");
    assert_eq!(held.last.at, at("2026-10-14T05:00:00+08:00"));
    assert_eq!(
        held.last.sign,
        Sign::Push {
            branch: "demo/claim-lease".to_string()
        }
    );
}

#[test]
fn another_agents_comment_does_not_keep_a_lease_alive() {
    let claim_msg = comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00");
    let other_marked_beat = format!(
        "## 心跳\n\n- agent: Codex\n- 认领: {}\n- 进度: 路过代报\n\nAgent: Codex\n",
        claim_msg.url
    );
    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![
            claim_msg,
            comment(
                "审完了，合并了。\n\nAgent: Codex\n",
                "2026-10-14T04:20:00+08:00",
            ),
            comment(&other_marked_beat, "2026-10-14T04:30:00+08:00"),
        ],
    ));
    let held = board.lease(31).unwrap();
    assert_eq!(
        held.last.at,
        at("2026-10-14T03:05:55+08:00"),
        "别的 agent 的评论和 ## 心跳都不算持有者的心跳"
    );
    assert_eq!(
        held.status(at("2026-10-14T05:06:00+08:00")),
        Status::Expired
    );
}

#[test]
fn a_label_without_a_claim_comment_is_reported_not_guessed() {
    let board = board_of(issue(&[lease::CLAIM_LABEL], vec![]));
    assert!(board.leases.is_empty());
    let trouble = &board.trouble[0];
    assert_eq!(trouble.issue, 31);
    assert!(
        trouble.why.contains("没有认领评论"),
        "说清楚为什么读不出来，而不是当成没人在做：{}",
        trouble.why
    );
}

#[test]
fn a_handed_back_claim_does_not_hold_even_while_the_label_is_still_on() {
    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![
            comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00"),
            comment(
                "## 收回认领\n\n- 原认领: DemoAgent（u）\n- 收回者: Claude\n",
                "2026-10-14T06:10:00+08:00",
            ),
        ],
    ));
    assert!(board.leases.is_empty());
    assert!(
        board.trouble[0].why.contains("收回"),
        "{}",
        board.trouble[0].why
    );
}

#[test]
fn a_new_claim_after_a_hand_back_holds_again() {
    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![
            comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00"),
            comment(
                "## 收回认领\n\n- 收回者: Claude\n",
                "2026-10-14T06:10:00+08:00",
            ),
            comment(
                "## 认领\n\n- agent: Claude\n- 改动模块: docs/demo\n",
                "2026-10-14T06:11:00+08:00",
            ),
        ],
    ));
    assert_eq!(board.lease(31).map(|l| l.agent()), Some("Claude"));
    assert!(board.trouble.is_empty());
}

#[test]
fn a_stale_hand_back_does_not_end_the_new_lease() {
    let old = comment_id(DEMO_CLAIM, "2026-10-14T03:05:55+08:00", 1);
    let old_url = old.url.clone();
    let first_back = comment_id(
        &format!("## 收回认领\n\n- 原认领: DemoAgent（{old_url}）\n- 收回者: Codex\n"),
        "2026-10-14T06:08:00+08:00",
        2,
    );
    let fresh = comment_id(
        "## 认领\n\n- agent: Claude\n- 改动模块: docs/demo\n",
        "2026-10-14T06:09:00+08:00",
        3,
    );
    let stale_back = comment_id(
        &format!("## 收回认领\n\n- 原认领: DemoAgent（{old_url}）\n- 收回者: CodeBuddy\n"),
        "2026-10-14T06:10:00+08:00",
        4,
    );
    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![old, first_back, fresh, stale_back],
    ));
    assert_eq!(
        board.lease(31).map(|held| held.agent()),
        Some("Claude"),
        "迟到的收回只结束它点名的旧租约，不能结束中间的新认领"
    );
}

#[test]
fn a_stale_heartbeat_does_not_refresh_a_new_same_vendor_lease() {
    let old = comment_id(DEMO_CLAIM, "2026-10-14T03:05:55+08:00", 1);
    let old_url = old.url.clone();
    let fresh_url = "https://github.com/zbtzbtzbt/theFlash/issues/31#issuecomment-fresh";
    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![
            old,
            comment_id(
                &format!("## 收回认领\n\n- 原认领: DemoAgent（{old_url}）\n- 收回者: Codex\n"),
                "2026-10-14T06:08:00+08:00",
                2,
            ),
            Comment {
                id: 3,
                url: fresh_url.to_string(),
                posted_at: at("2026-10-14T06:09:00+08:00"),
                body: "## 认领\n\n- agent: DemoAgent\n- 改动模块: docs/new\n".to_string(),
            },
            comment_id(
                &format!("## 心跳\n\n- agent: DemoAgent\n- 认领: {old_url}\n- 进度: 旧会话迟到\n"),
                "2026-10-14T06:10:00+08:00",
                4,
            ),
            comment_id(
                "## 心跳\n\n- agent: DemoAgent\n- 进度: 没写认领链接的心跳\n",
                "2026-10-14T06:15:00+08:00",
                5,
            ),
        ],
    ));
    let held = board.lease(31).expect("新租约仍在");
    assert_eq!(held.claim.url, fresh_url);
    assert_eq!(
        held.last.at,
        at("2026-10-14T06:09:00+08:00"),
        "旧租约绑定的迟到心跳和没写认领链接的心跳都不能刷新同厂商的新会话"
    );
}

#[test]
fn claiming_an_issue_someone_else_holds_is_refused() {
    let held = issue(
        &[lease::CLAIM_LABEL],
        vec![comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00")],
    );
    let board = board_of(held.clone());
    let verdict = decide_claim(
        &board,
        &held,
        "Claude",
        &["docs/demo/claim-lease-demo.md".to_string()],
        None,
        at("2026-10-14T03:20:00+08:00"),
    );
    let Verdict::Refuse(why) = &verdict else {
        panic!("别人认领中的 issue 不该让认领：{verdict:?}");
    };
    assert!(why.contains("DemoAgent") && why.contains("过期"), "{why}");
}

#[test]
fn claiming_an_issue_whose_claim_expired_sends_you_to_reclaim_first() {
    let held = issue(
        &[lease::CLAIM_LABEL],
        vec![comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00")],
    );
    let board = board_of(held.clone());
    let now = at("2026-10-14T06:10:00+08:00");
    let Verdict::Refuse(why) = decide_claim(
        &board,
        &held,
        "Claude",
        &["docs/demo".to_string()],
        None,
        now,
    ) else {
        panic!("过期的认领要先收回，不能直接盖掉");
    };
    assert!(why.contains("reclaim"), "{why}");
    assert!(
        decide_reclaim(&board, 31, now).is_ok(),
        "过期之后应该可以收回"
    );
    assert!(
        !decide_reclaim(&board, 31, at("2026-10-14T04:00:00+08:00")).is_ok(),
        "还没过期不能收回"
    );
}

#[test]
fn an_assigned_label_keeps_other_agents_out() {
    let free = issue(&["指派:codex"], vec![]);
    let board = Board::default();
    let now = at("2026-10-14T03:20:00+08:00");
    let Verdict::Refuse(why) =
        decide_claim(&board, &free, "Claude", &["backend".to_string()], None, now)
    else {
        panic!("指派给 codex 的 issue 不该让 Claude 认领");
    };
    assert!(why.contains("codex"), "{why}");
    assert!(
        decide_claim(&board, &free, "Codex", &["backend".to_string()], None, now).is_ok(),
        "被指派的那个 agent 可以认领"
    );
}

#[test]
fn claiming_modules_another_claim_covers_needs_a_plan() {
    let other = Issue {
        number: 16,
        title: "认领改成带过期时间的租约".to_string(),
        labels: vec![lease::CLAIM_LABEL.to_string()],
        comments: vec![comment(
            "## 认领\n\n- agent: Claude\n- 改动模块: backend/src/claim, AGENTS.md\n",
            "2026-10-14T02:57:00+08:00",
        )],
    };
    let board = board_of(other);
    let free = Issue {
        number: 25,
        title: "另一个 issue".to_string(),
        labels: vec![],
        comments: vec![],
    };
    let now = at("2026-10-14T03:20:00+08:00");
    let modules = ["backend/src/claim/lease.rs".to_string()];
    let Verdict::Refuse(why) = decide_claim(&board, &free, "Codex", &modules, None, now) else {
        panic!("改同一批文件的两个认领不该都放行");
    };
    assert!(why.contains("#16") && why.contains("--overlap"), "{why}");

    let verdict = decide_claim(
        &board,
        &free,
        "Codex",
        &modules,
        Some("#16 先合，我排在后面"),
        now,
    );
    let Verdict::Ok { overlaps } = &verdict else {
        panic!("写明了怎么分工就应该放行：{verdict:?}");
    };
    assert_eq!(overlaps[0].issue, 16);
    assert_eq!(overlaps[0].modules, ["backend/src/claim/lease.rs"]);
}

#[test]
fn an_expired_claim_elsewhere_does_not_block_a_new_claim() {
    let stale = Issue {
        number: 16,
        title: "过期了的认领".to_string(),
        labels: vec![lease::CLAIM_LABEL.to_string()],
        comments: vec![comment(
            "## 认领\n\n- agent: Claude\n- 改动模块: backend/src/claim\n",
            "2026-10-14T00:00:00+08:00",
        )],
    };
    let board = board_of(stale);
    assert!(
        decide_claim(
            &board,
            &issue(&[], vec![]),
            "Codex",
            &["backend/src/claim".to_string()],
            None,
            at("2026-10-14T03:20:00+08:00"),
        )
        .is_ok(),
        "已经过期的认领不该再占着模块"
    );
}

#[test]
fn the_comments_the_commands_post_read_back_as_what_they_are() {
    let now = at("2026-10-14T03:05:55+08:00");
    let body = claim_comment(
        "DemoAgent",
        &["docs/demo/claim-lease-demo.md".to_string()],
        Some("demo/claim-lease"),
        Some(at("2026-10-14T05:00:00+08:00")),
        &[],
        None,
        Some("队列里还有比 P2 高、现在就能做的 #3（P1）；#3 要访问真实大麦"),
        now,
    );
    assert!(body.contains("- 不按顺序: 队列里还有"), "{body}");
    let read = lease::parse_claim(&body, "u", now).expect("a posted claim reads back as a claim");
    assert_eq!(read.agent, "DemoAgent");
    assert_eq!(read.modules, ["docs/demo/claim-lease-demo.md"]);
    assert_eq!(read.branch.as_deref(), Some("demo/claim-lease"));
    assert_eq!(read.due, Some(at("2026-10-14T05:00:00+08:00")));
    assert_eq!(read.missing_field(), None);
    assert!(
        body.contains(&HEARTBEAT_EVERY_MINUTES.to_string()),
        "认领评论要告诉持有者多久要心跳一次：{body}"
    );

    let beat = heartbeat_comment("DemoAgent", "claim-url", "在补测试", now);
    let beat = lease::parse_heartbeat(&beat, "u", now).expect("a posted heartbeat reads back");
    assert!(beat.marked && beat.agent == "DemoAgent");
    assert_eq!(beat.claim_url.as_deref(), Some("claim-url"));

    let lease = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00")],
    ))
    .lease(31)
    .cloned()
    .unwrap();
    let back = reclaim_comment(&lease, "Claude", at("2026-10-14T06:10:00+08:00"));
    assert!(lease::is_reclaim(&back));
    assert_eq!(
        lease::parse_claim(&back, "u", now),
        None,
        "收回不是一条新认领"
    );
    assert!(
        back.contains("DemoAgent") && back.contains("3 小时 4 分钟前"),
        "{back}"
    );
    assert!(
        !back.contains("已去掉"),
        "收回评论在改标签前发出，不能提前声称已去掉标签：{back}"
    );
}

#[test]
fn a_write_up_that_quotes_the_commands_is_a_report_not_a_claim() {
    // The demo record posted on #31 after the hand-back, byte for byte.
    // It quotes a claim, a heartbeat and a hand-back inside fenced code
    // blocks; reading those quotes as the things they quote would make a
    // report claim the issue, or revive a lease that was handed back.
    let record = real_comment(6153344810);
    assert_eq!(
        lease::parse_claim(&record.body, &record.url, record.posted_at),
        None,
        "代码块里的 ## 认领 是引用，不是一条新认领"
    );
    assert!(
        !lease::is_reclaim(&record.body),
        "代码块里的 ## 收回认领 也是引用"
    );
    let beat = lease::parse_heartbeat(&record.body, &record.url, record.posted_at)
        .expect("写进度的评论算一次心跳");
    assert_eq!(
        beat.agent, "Claude",
        "心跳属于写这条评论的 agent，不是它引用的那条认领里的 agent"
    );

    // The same, quoted with `>` and fenced with `~~~`.
    for quoted in [
        "转一下它发的：\n\n> ## 认领\n> \n> - agent: DemoAgent\n> - 改动模块: docs/demo\n",
        "~~~\n## 认领\n\n- agent: DemoAgent\n- 改动模块: docs/demo\n~~~\n",
        "贴一下别人发的认领给大家看：\n\n    ## 认领\n\n    - agent: DemoAgent\n    - 改动模块: docs/demo\n\nAgent: Codex\n",
        "贴一下收回：\n\n    ## 收回认领\n\n    - 原认领: DemoAgent（u）\n\nAgent: Codex\n",
    ] {
        assert_eq!(
            lease::parse_claim(quoted, "u", at("2026-10-14T03:30:00+08:00")),
            None,
            "{quoted:?} 是引用，不是认领"
        );
        assert!(
            !lease::is_reclaim(quoted),
            "{quoted:?} 里的缩进/围栏代码块不能算收回"
        );
    }
    assert_eq!(
        lease::agent_trailer("说明：\n\n    Agent: DemoAgent\n\nAgent: Codex\n"),
        Some("Codex".to_string()),
        "4 空格缩进代码块里的 Agent: 签名不算"
    );
    assert_eq!(
        lease::agent_trailer("说明：\n\n    Agent: DemoAgent\n"),
        None,
        "只有 4 空格缩进代码块里的 Agent: 签名不算"
    );
}

#[test]
fn the_earliest_claim_in_a_window_holds_the_issue() {
    // Two agents read "nobody is working on this" at the same moment and
    // both post a claim. GitHub has no compare-and-set, so both have to
    // reach the same answer from the comments they can both see: the
    // earliest claim holds it, and GitHub's comment id settles the same
    // second.
    let codex_claim = "## 认领\n\n- agent: Codex\n- 改动模块: docs/demo/claim-lease-demo.md\n";
    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![
            comment_id(DEMO_CLAIM, "2026-10-14T03:05:55+08:00", 2),
            comment_id(codex_claim, "2026-10-14T03:05:55+08:00", 1),
        ],
    ));
    let held = board.lease(31).expect("两条认领里有一条算数");
    assert_eq!(
        held.agent(),
        "Codex",
        "同一秒里发的两条认领，评论 id 小的那条更早"
    );

    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![
            comment_id(DEMO_CLAIM, "2026-10-14T03:05:55+08:00", 1),
            comment_id(codex_claim, "2026-10-14T03:05:58+08:00", 2),
        ],
    ));
    let held = board.lease(31).expect("两条认领里有一条算数");
    assert_eq!(held.agent(), "DemoAgent", "晚三秒的那条认领不算");
    assert_eq!(
        held.last.at,
        at("2026-10-14T03:05:55+08:00"),
        "别人晚到的认领也不算持有者的心跳"
    );

    // The agent that lost reads the same thing and says so.
    let issue = issue(
        &[lease::CLAIM_LABEL],
        vec![
            comment_id(DEMO_CLAIM, "2026-10-14T03:05:55+08:00", 1),
            comment_id(codex_claim, "2026-10-14T03:05:58+08:00", 2),
        ],
    );
    let (mine, theirs) = (issue.comments[1].url.clone(), issue.comments[0].url.clone());
    assert_eq!(
        confirm_claim(&issue, &mine, &BTreeMap::new()),
        Confirmed::Lost {
            agent: "DemoAgent".to_string(),
            claim: theirs.clone(),
        }
    );
    assert_eq!(
        confirm_claim(&issue, &theirs, &BTreeMap::new()),
        Confirmed::Held
    );
}

#[test]
fn giving_up_a_claim_that_lost_a_race_does_not_end_the_one_that_won() {
    let codex_claim = "## 认领\n\n- agent: Codex\n- 改动模块: docs/demo/claim-lease-demo.md\n";
    let mine = comment_id(codex_claim, "2026-10-14T03:05:58+08:00", 2);
    let given_up = yield_comment(
        "Codex",
        &mine.url,
        "DemoAgent",
        "https://github.com/zbtzbtzbt/theFlash/issues/31#issuecomment-1",
    );
    assert_eq!(
        lease::yielded_claim(&given_up).as_deref(),
        Some(mine.url.as_str()),
        "让出评论要说清让出的是哪一条认领：{given_up}"
    );
    assert!(
        !lease::is_reclaim(&given_up),
        "让出自己那条认领不是收回别人的认领：{given_up}"
    );
    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![
            comment_id(DEMO_CLAIM, "2026-10-14T03:05:55+08:00", 1),
            mine,
            comment_id(&given_up, "2026-10-14T03:05:59+08:00", 3),
        ],
    ));
    assert_eq!(
        board.lease(31).map(|l| l.agent()),
        Some("DemoAgent"),
        "让出之后，赢的那条认领照常有效"
    );
    assert!(board.trouble.is_empty(), "{:?}", board.trouble);
}

#[test]
fn a_later_claim_with_the_same_agent_name_does_not_replace_the_session() {
    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![
            comment_id(DEMO_CLAIM, "2026-10-14T03:05:55+08:00", 1),
            comment_id(
                "## 认领\n\n- agent: DemoAgent\n- 改动模块: docs/demo, scripts/demo\n",
                "2026-10-14T04:00:00+08:00",
                2,
            ),
        ],
    ));
    let held = board.lease(31).expect("最早的认领仍然是租约");
    assert_eq!(held.agent(), "DemoAgent");
    assert_eq!(held.claim.modules, ["docs/demo/claim-lease-demo.md"]);
    assert_eq!(
        held.last.at,
        at("2026-10-14T03:05:55+08:00"),
        "agent 名只表示厂商，不表示会话；同名的后一条认领不能替换或续租"
    );
}

#[test]
fn a_claim_whose_label_never_made_it_on_still_holds_the_issue() {
    // The claim comment goes out first and the label second, so a failed
    // `gh issue edit` leaves a lease the label-filtered snapshot cannot
    // see. Every command that acts on one issue reads that issue.
    let held = issue(&[], vec![comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00")]);
    let mut board = Board::default();
    board.include(&held);
    let lease = board.lease(31).expect("没有标签的认领评论也是一条租约");
    assert_eq!(lease.agent(), "DemoAgent");
    assert!(!lease.labelled, "租约知道自己的标签不在，心跳会补上");

    let Verdict::Refuse(why) = decide_claim(
        &board,
        &held,
        "Codex",
        &["docs/unrelated".to_string()],
        None,
        at("2026-10-14T03:20:00+08:00"),
    ) else {
        panic!("已有租约即使标签没加上也不能被覆盖");
    };
    assert!(why.contains("DemoAgent"), "{why}");
    assert!(
        !decide_reclaim(&board, 31, at("2026-10-14T03:20:00+08:00")).is_ok(),
        "没过期就不能收回，和有没有标签无关"
    );
    assert!(
        decide_reclaim(&board, 31, at("2026-10-14T06:10:00+08:00")).is_ok(),
        "过期之后照常可以收回"
    );

    // An issue nobody ever claimed stays free, label or no label.
    let mut board = Board::default();
    board.include(&issue(&[], vec![]));
    assert!(board.leases.is_empty() && board.trouble.is_empty());
}

#[test]
fn non_push_branch_activities_do_not_renew_a_lease() {
    let dir = std::env::temp_dir().join(format!("claims-unit-activity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("issues.json"),
        serde_json::json!([{
            "number": 31,
            "title": "测试",
            "labels": [{"name": lease::CLAIM_LABEL}]
        }])
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("comments-31.json"),
        serde_json::json!([{
            "id": 1,
            "created_at": "2026-10-13T19:05:55Z",
            "html_url": "https://github.com/o/r/issues/31#issuecomment-1",
            "body": DEMO_CLAIM
        }])
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("branch-demo-claim-lease.json"),
        serde_json::json!([
            {"id": 10, "activity_type": "pr_merge", "timestamp": "2026-10-13T21:55:00Z"},
            {"id": 11, "activity_type": "branch_deletion", "timestamp": "2026-10-13T22:00:00Z"}
        ])
        .to_string(),
    )
    .unwrap();
    let snap = Snapshot::replay(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !snap.pushes.contains_key("demo/claim-lease"),
        "pr_merge 和 branch_deletion 不是持有者推送分支：{:?}",
        snap.pushes
    );
}

#[test]
fn a_push_time_nobody_can_read_keeps_the_issue_from_being_taken_over() {
    // Pushing is a heartbeat, so a push time that cannot be read means
    // silence cannot be proved. Reading pushes needs push access to the
    // repository; without it the lease is reported, not expired.
    let mut pushes = BTreeMap::new();
    pushes.insert(
        "demo/claim-lease".to_string(),
        Push::Unreadable("Must have push access to repository".to_string()),
    );
    let board = board(&Snapshot {
        issues: vec![issue(
            &[lease::CLAIM_LABEL],
            vec![comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00")],
        )],
        pushes,
    });
    let held = board.lease(31).expect("租约还在，只是证明不了它沉默");
    let now = at("2026-10-14T06:10:00+08:00");
    assert_eq!(held.status(now), Status::Expired, "评论上看它确实沉默了");
    assert!(
        held.push_unreadable
            .as_deref()
            .is_some_and(|w| w.contains("推送时间读不到")),
        "{:?}",
        held.push_unreadable
    );
    let Verdict::Refuse(why) = decide_reclaim(&board, 31, now) else {
        panic!("读不到推送时间就不能断定它沉默，不能收回");
    };
    assert!(why.contains("推送时间读不到"), "{why}");
    let Verdict::Refuse(why) = decide_claim(
        &board,
        &issue(
            &[lease::CLAIM_LABEL],
            vec![comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00")],
        ),
        "Codex",
        &["docs/demo".to_string()],
        None,
        now,
    ) else {
        panic!("同样不能直接认领");
    };
    assert!(why.contains("推送时间读不到"), "{why}");
}

#[test]
fn a_lease_with_unreadable_pushes_still_reserves_its_modules() {
    let mut pushes = BTreeMap::new();
    pushes.insert(
        "demo/claim-lease".to_string(),
        Push::Unreadable("Must have push access to repository".to_string()),
    );
    let board = board(&Snapshot {
        issues: vec![issue(
            &[lease::CLAIM_LABEL],
            vec![comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00")],
        )],
        pushes,
    });
    let free = Issue {
        number: 25,
        title: "另一个 issue".to_string(),
        labels: vec![],
        comments: vec![],
    };
    let Verdict::Refuse(why) = decide_claim(
        &board,
        &free,
        "Codex",
        &["docs/demo/claim-lease-demo.md".to_string()],
        None,
        at("2026-10-14T06:10:00+08:00"),
    ) else {
        panic!("证明不了沉默的租约仍要预留模块，不能交给另一个 issue");
    };
    assert!(
        why.contains("#31") && why.contains("--overlap"),
        "拒绝要指出占用模块的租约：{why}"
    );
}

#[test]
fn durations_read_like_a_person_wrote_them() {
    assert_eq!(minutes(0), "0 分钟");
    assert_eq!(minutes(59), "59 分钟");
    assert_eq!(minutes(60), "1 小时");
    assert_eq!(minutes(125), "2 小时 5 分钟");
}

#[test]
fn an_unreadable_claim_is_not_a_free_issue() {
    let labelled = issue(&[lease::CLAIM_LABEL], vec![]);
    let board = board_of(labelled.clone());
    let Verdict::Refuse(why) = decide_claim(
        &board,
        &labelled,
        "Codex",
        &["backend".to_string()],
        None,
        at("2026-10-14T06:10:00+08:00"),
    ) else {
        panic!("读不出来的认领可能有人在做，不能直接盖掉");
    };
    assert!(
        why.contains("没有认领评论") && why.contains("问清楚"),
        "{why}"
    );
}

#[test]
fn a_handed_back_issue_can_be_claimed_again_right_away() {
    let handed_back = issue(
        &[lease::CLAIM_LABEL],
        vec![
            comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00"),
            comment(
                "## 收回认领\n\n- 收回者: Claude\n",
                "2026-10-14T06:10:00+08:00",
            ),
        ],
    );
    let board = board_of(handed_back.clone());
    assert_eq!(board.trouble[0].problem, Problem::HandedBack);
    assert!(
        decide_claim(
            &board,
            &handed_back,
            "Codex",
            &["docs/demo".to_string()],
            None,
            at("2026-10-14T06:11:00+08:00"),
        )
        .is_ok(),
        "认领被收回之后标签还在，下一个 agent 应该能直接认领"
    );
}
