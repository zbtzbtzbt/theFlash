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
    Comment {
        url: format!("https://github.com/zbtzbtzbt/theFlash/issues/31#issuecomment-{posted_at}"),
        posted_at: at(posted_at),
        body: body.to_string(),
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
fn a_signed_comment_counts_as_a_heartbeat_and_a_claim_does_not() {
    let beat = lease::parse_heartbeat(
        "CI 绿了，在补测试。\n\nAgent: DemoAgent\n",
        "u",
        at("2026-10-14T03:30:00+08:00"),
    )
    .unwrap();
    assert_eq!(beat.agent, "DemoAgent");
    assert!(!beat.marked);
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
    let beat = "## 心跳\n\n- agent: DemoAgent\n- 进度: 在补测试\n\nAgent: DemoAgent\n";
    let mut snapshot = Snapshot {
        issues: vec![issue(
            &[lease::CLAIM_LABEL],
            vec![
                comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00"),
                comment(beat, "2026-10-14T04:30:00+08:00"),
            ],
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
        at("2026-10-14T05:00:00+08:00"),
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
    let board = board_of(issue(
        &[lease::CLAIM_LABEL],
        vec![
            comment(DEMO_CLAIM, "2026-10-14T03:05:55+08:00"),
            comment(
                "审完了，合并了。\n\nAgent: Codex\n",
                "2026-10-14T04:30:00+08:00",
            ),
        ],
    ));
    let held = board.lease(31).unwrap();
    assert_eq!(
        held.last.at,
        at("2026-10-14T03:05:55+08:00"),
        "别的 agent 的评论不算持有者的心跳"
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
    let free = issue(&[], vec![]);
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
        now,
    );
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

    let beat = heartbeat_comment("DemoAgent", "在补测试", now);
    let beat = lease::parse_heartbeat(&beat, "u", now).expect("a posted heartbeat reads back");
    assert!(beat.marked && beat.agent == "DemoAgent");

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
