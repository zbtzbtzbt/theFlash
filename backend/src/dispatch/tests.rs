//! 判断部分的测试：事件 → 派给谁，什么时候不派，超时怎么收尾。
//!
//! Nothing here starts a process or reaches the network: `plan` is a
//! function of a snapshot, the state, the settings and the time, and that
//! is exactly what these tests give it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use super::config::{Agent, Config, PromptStyle, Scheduled};
use super::snapshot::{Issue, MainChecks, Pull, Snapshot};
use super::state::{Outcome, Session, State};
use super::*;

/// `plan` 的别名：测试里局部变量也叫 plan，别让它挡住函数。
fn decide(snapshot: &Snapshot, state: &State, config: &Config, now: DateTime<Utc>) -> Plan {
    super::plan(snapshot, state, config, now)
}

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn config() -> Config {
    let mut agents = BTreeMap::new();
    for vendor in ["claude", "codex", "codebuddy"] {
        agents.insert(
            vendor.to_string(),
            Agent {
                command: vec!["true".to_string()],
                prompt: PromptStyle::Argv,
            },
        );
    }
    Config {
        agents,
        vendor_order: ["claude", "codex", "codebuddy"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        max_sessions: 10,
        max_sessions_per_day: 20,
        // Roles the cloud takes are off by default; each test that cares
        // about #9 sets this itself.
        cloud_roles: Vec::new(),
        ..Config::default()
    }
}

fn issue(number: u64, labels: &[&str], agent: Option<&str>) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        agent: agent.map(|s| s.to_string()),
        last_comment_at: None,
    }
}

fn pull(number: u64, labels: &[&str], agent: Option<&str>, issue: Option<u64>) -> Pull {
    Pull {
        number,
        title: format!("pr {number}"),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        head_sha: "91f0f06abcdef".to_string(),
        head_branch: format!("night/issue-{number}"),
        agent: agent.map(|s| s.to_string()),
        issue,
    }
}

fn running(id: &str, issue: Option<u64>, vendor: &str, started: &str) -> Session {
    Session {
        id: id.to_string(),
        role: "accept".to_string(),
        vendor: vendor.to_string(),
        issue,
        pull: None,
        started_at: at(started),
        finished_at: None,
        outcome: None,
        // A pid nothing can be: the session counts as gone, which is what
        // a test wants unless it says otherwise.
        pid: None,
        log: PathBuf::from("/tmp/x.log"),
        worktree: PathBuf::from("/tmp/x"),
    }
}

/// A session that ran and ended, with the role it was started as. Tests
/// about "has this already been dispatched" build it from the id the
/// dispatcher itself chose, so they describe the behaviour and not the
/// shape of the id.
fn ended(
    id: &str,
    role: &str,
    issue: Option<u64>,
    pull: Option<u64>,
    started: &str,
    ended: &str,
) -> Session {
    Session {
        id: id.to_string(),
        role: role.to_string(),
        vendor: "codex".to_string(),
        issue,
        pull,
        started_at: at(started),
        finished_at: Some(at(ended)),
        outcome: Some(Outcome::Done),
        pid: None,
        log: PathBuf::from("/tmp/x.log"),
        worktree: PathBuf::from("/tmp/x"),
    }
}

fn roles(plan: &Plan) -> Vec<(String, String, Option<u64>, Option<u64>)> {
    plan.start
        .iter()
        .map(|t| (t.role.name().to_string(), t.vendor.clone(), t.issue, t.pull))
        .collect()
}

fn skipped(plan: &Plan, id: &str) -> String {
    plan.skip
        .iter()
        .find(|(t, _)| t.id == id)
        .map(|(_, s)| s.why())
        .unwrap_or_else(|| panic!("{id} 不在跳过的列表里：{:?}", plan.skip))
}

#[test]
fn each_event_goes_to_the_role_that_handles_it() {
    let snapshot = Snapshot {
        issues: vec![
            issue(50, &[], None),
            issue(51, &["待验收"], Some("claude")),
            issue(52, &["验收未通过"], Some("codex")),
            issue(53, &["认领中"], Some("claude")),
        ],
        pulls: vec![
            pull(60, &["待审查"], Some("codebuddy"), None),
            pull(61, &["要修改"], Some("codex"), None),
            pull(62, &[], Some("claude"), None),
        ],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    let got = roles(&plan);
    assert!(
        got.contains(&("review".to_string(), "claude".to_string(), None, Some(60))),
        "待审查 的 PR 要派审查：{got:?}"
    );
    assert!(
        got.contains(&("fix-pr".to_string(), "codex".to_string(), None, Some(61))),
        "要修改 的 PR 要派给实现者自己：{got:?}"
    );
    assert!(
        got.iter()
            .any(|(role, _, issue, _)| role == "accept" && *issue == Some(51)),
        "待验收 的 issue 要派验收：{got:?}"
    );
    assert!(
        got.contains(&("fix-issue".to_string(), "codex".to_string(), Some(52), None)),
        "验收未通过 要派回实现者：{got:?}"
    );
    assert!(
        got.iter()
            .any(|(role, _, issue, _)| role == "implement" && *issue == Some(50)),
        "没人认领的 issue 要派实现：{got:?}"
    );
    assert!(
        !got.iter()
            .any(|(_, _, issue, pull)| *issue == Some(53) || *pull == Some(62)),
        "认领中的 issue 和没有标签的 PR 不派：{got:?}"
    );
}

#[test]
fn reviewing_and_accepting_go_to_another_vendor_than_the_implementer() {
    let snapshot = Snapshot {
        issues: vec![issue(51, &["待验收"], Some("claude"))],
        pulls: vec![pull(60, &["待审查"], Some("claude"), None)],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    for task in &plan.start {
        assert_ne!(
            task.vendor,
            "claude",
            "{} 不能交给实现它的厂商：{:?}",
            task.role.name(),
            task
        );
    }
    assert_eq!(plan.start.len(), 2, "{:?}", plan.start);
}

#[test]
fn work_whose_implementer_is_unknown_is_left_alone() {
    let snapshot = Snapshot {
        issues: vec![issue(51, &["待验收"], None)],
        pulls: vec![pull(60, &["待审查"], None, None)],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    assert!(plan.start.is_empty(), "{:?}", plan.start);
    assert!(skipped(&plan, "accept-51-a1").contains("不猜"));
    assert!(skipped(&plan, "review-60-91f0f06-a1").contains("不猜"));
}

#[test]
fn one_issue_never_has_two_sessions() {
    let snapshot = Snapshot {
        issues: vec![issue(51, &["待验收"], Some("claude"))],
        pulls: vec![pull(60, &["待审查"], Some("claude"), Some(51))],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let state = State {
        sessions: vec![running(
            "accept-51-a1",
            Some(51),
            "codex",
            "2026-10-14T03:50:00+08:00",
        )],
        ..State::default()
    };
    // The running session's process is gone, so it is swept; the test is
    // about the session that is still alive, so keep it alive.
    let mut state = state;
    state.sessions[0].pid = Some(std::process::id());
    let plan = decide(
        &snapshot,
        &state,
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    assert!(plan.sweep.is_empty(), "{:?}", plan.sweep);
    assert!(
        plan.start.is_empty(),
        "同一个 issue 不该再派：{:?}",
        plan.start
    );
    assert!(skipped(&plan, "review-60-91f0f06-a1").contains("accept-51-a1"));
}

#[test]
fn a_task_already_dispatched_is_not_dispatched_again() {
    let snapshot = Snapshot {
        issues: vec![issue(51, &["待验收"], Some("claude"))],
        pulls: vec![],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let mut done = running(
        "accept-51-a1",
        Some(51),
        "codex",
        "2026-10-14T02:00:00+08:00",
    );
    done.finished_at = Some(at("2026-10-14T02:40:00+08:00"));
    done.outcome = Some(Outcome::Done);
    let state = State {
        sessions: vec![done],
        ..State::default()
    };
    let plan = decide(
        &snapshot,
        &state,
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    assert!(plan.start.is_empty(), "{:?}", plan.start);
    assert!(skipped(&plan, "accept-51-a2").contains("accept-51-a1"));
}

#[test]
fn a_session_that_timed_out_is_tried_once_more_and_then_left_alone() {
    let snapshot = Snapshot {
        issues: vec![issue(51, &["待验收"], Some("claude"))],
        pulls: vec![],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let mut first = running(
        "accept-51-a1",
        Some(51),
        "codex",
        "2026-10-14T00:00:00+08:00",
    );
    first.finished_at = Some(at("2026-10-14T02:00:00+08:00"));
    first.outcome = Some(Outcome::TimedOut);
    let mut state = State {
        sessions: vec![first],
        ..State::default()
    };
    let plan = decide(
        &snapshot,
        &state,
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    assert_eq!(
        plan.start.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        vec!["accept-51-a2".to_string()],
        "超时的会话要再试一次：{:?}",
        plan.start
    );

    let mut second = running(
        "accept-51-a2",
        Some(51),
        "codex",
        "2026-10-14T03:00:00+08:00",
    );
    second.finished_at = Some(at("2026-10-14T03:30:00+08:00"));
    second.outcome = Some(Outcome::TimedOut);
    state.sessions.push(second);
    let plan = decide(
        &snapshot,
        &state,
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    assert!(plan.start.is_empty(), "第三次不再派：{:?}", plan.start);
    assert!(skipped(&plan, "accept-51-a3").contains("重试次数用完"));
}

#[test]
fn a_new_head_on_a_pull_request_gets_another_review() {
    // 审查过 head A 不等于审查过 head B：实现者按审查意见改完重新推一次，
    // 就要再审一轮。
    let config = config();
    let now = at("2026-10-14T04:00:00+08:00");
    let mut first = pull(60, &["待审查"], Some("codebuddy"), None);
    first.head_sha = "91f0f06c82463f1669541bd3ef38f57ccccd6a5e".to_string();
    let snapshot = Snapshot {
        pulls: vec![first.clone()],
        main_checks: MainChecks::Green,
        ..Snapshot::default()
    };
    let plan = decide(&snapshot, &State::default(), &config, now);
    let reviewed = plan.start[0].id.clone();

    // 那一轮审查正常结束（结论是要修改），实现者推了新 head。
    let state = State {
        sessions: vec![ended(
            &reviewed,
            "review",
            None,
            Some(60),
            "2026-10-14T03:00:00+08:00",
            "2026-10-14T03:40:00+08:00",
        )],
        ..State::default()
    };
    let mut again = first;
    again.head_sha = "eca6af1b0d2e4f5a6b7c8d9e0f1a2b3c4d5e6f70".to_string();
    let snapshot = Snapshot {
        pulls: vec![again],
        ..snapshot
    };
    let plan = decide(&snapshot, &state, &config, now);
    assert_eq!(
        plan.start.len(),
        1,
        "新 head 要重新审查：派 {:?}，跳过 {:?}",
        plan.start,
        plan.skip
    );
    assert_ne!(
        plan.start[0].id, reviewed,
        "新 head 是另一件活：{:?}",
        plan.start
    );
}

#[test]
fn accepting_again_after_a_fix_is_a_new_round() {
    // 验收未通过 → 实现者修 → 重新 `待验收`，是第二轮验收，不是已经做过的那一轮。
    let config = config();
    let now = at("2026-10-14T04:00:00+08:00");
    let waiting = Snapshot {
        issues: vec![issue(51, &["待验收"], Some("codebuddy"))],
        main_checks: MainChecks::Green,
        ..Snapshot::default()
    };
    let first = decide(&waiting, &State::default(), &config, now).start[0]
        .id
        .clone();
    let mut state = State {
        sessions: vec![ended(
            &first,
            "accept",
            Some(51),
            None,
            "2026-10-14T01:00:00+08:00",
            "2026-10-14T01:50:00+08:00",
        )],
        ..State::default()
    };

    // 验收未通过，实现者修完，又挂回 `待验收`。
    let rejected = Snapshot {
        issues: vec![issue(51, &["验收未通过"], Some("codebuddy"))],
        ..waiting.clone()
    };
    let fix = decide(&rejected, &state, &config, now).start[0].id.clone();
    state.sessions.push(ended(
        &fix,
        "fix-issue",
        Some(51),
        None,
        "2026-10-14T02:00:00+08:00",
        "2026-10-14T02:50:00+08:00",
    ));

    let plan = decide(&waiting, &state, &config, now);
    assert_eq!(
        plan.start.len(),
        1,
        "修完以后要再验收一轮：派 {:?}，跳过 {:?}",
        plan.start,
        plan.skip
    );
    assert_ne!(plan.start[0].id, first, "这是新的一轮：{:?}", plan.start);
}

#[test]
fn a_daily_job_runs_again_tomorrow_even_with_yesterdays_session_recorded() {
    // 昨天那次定时任务在状态里留下一个真实的会话记录，不该把今天这次挡掉。
    let config = Config {
        scheduled: vec![Scheduled {
            name: "nightly-review".to_string(),
            at: "03:00".to_string(),
            vendor: "codex".to_string(),
            instruction: "按 prompts/nightly-review.md 复盘".to_string(),
        }],
        ..config()
    };
    let empty = Snapshot {
        main_checks: MainChecks::Green,
        ..Snapshot::default()
    };
    let yesterday = decide(
        &empty,
        &State::default(),
        &config,
        at("2026-10-14T03:01:00+08:00"),
    )
    .start[0]
        .id
        .clone();
    let state = State {
        sessions: vec![ended(
            &yesterday,
            "nightly-review",
            None,
            None,
            "2026-10-14T03:01:00+08:00",
            "2026-10-14T04:30:00+08:00",
        )],
        scheduled_ran: [("nightly-review".to_string(), "2026-10-14".to_string())]
            .into_iter()
            .collect(),
        ..State::default()
    };
    let plan = decide(&empty, &state, &config, at("2026-10-15T03:01:00+08:00"));
    assert_eq!(
        plan.start.len(),
        1,
        "第二天照常跑：派 {:?}，跳过 {:?}",
        plan.start,
        plan.skip
    );
}

#[test]
fn a_second_failing_run_on_main_is_a_second_thing_to_fix() {
    // 修过一次红不等于现在的红修过了：新的失败 run 是新的活。
    let config = config();
    let now = at("2026-10-14T04:00:00+08:00");
    let first = Snapshot {
        main_checks: MainChecks::Red,
        main_run: Some(38456076570),
        ..Snapshot::default()
    };
    let fixed = decide(&first, &State::default(), &config, now).start[0]
        .id
        .clone();
    let state = State {
        sessions: vec![ended(
            &fixed,
            "fix-ci",
            None,
            None,
            "2026-10-14T01:00:00+08:00",
            "2026-10-14T01:40:00+08:00",
        )],
        ..State::default()
    };
    let later = Snapshot {
        main_run: Some(38464764998),
        ..first
    };
    let plan = decide(&later, &state, &config, now);
    assert_eq!(
        plan.start.len(),
        1,
        "另一个失败的 run 要再派一次：派 {:?}，跳过 {:?}",
        plan.start,
        plan.skip
    );
    assert_ne!(plan.start[0].id, fixed, "{:?}", plan.start);
}

#[test]
fn the_concurrency_ceiling_queues_the_rest() {
    let snapshot = Snapshot {
        issues: vec![issue(50, &[], None), issue(52, &[], None)],
        pulls: vec![],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let config = Config {
        max_sessions: 1,
        ..config()
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config,
        at("2026-10-14T04:00:00+08:00"),
    );
    assert_eq!(plan.start.len(), 1, "{:?}", plan.start);
    let queued = plan
        .skip
        .iter()
        .find(|(_, s)| matches!(s, Skip::Concurrency(_)))
        .expect("第二个要排队");
    assert!(queued.1.why().contains("排队"), "{}", queued.1.why());
}

#[test]
fn the_daily_ceiling_stops_new_sessions_and_is_worth_saying_on_the_issue() {
    let snapshot = Snapshot {
        issues: vec![issue(50, &[], None)],
        pulls: vec![],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let config = Config {
        max_sessions_per_day: 1,
        ..config()
    };
    let mut done = running(
        "implement-49-a1",
        Some(49),
        "claude",
        "2026-10-14T01:00:00+08:00",
    );
    done.finished_at = Some(at("2026-10-14T01:30:00+08:00"));
    done.outcome = Some(Outcome::Done);
    let state = State {
        sessions: vec![done],
        ..State::default()
    };
    let plan = decide(&snapshot, &state, &config, at("2026-10-14T04:00:00+08:00"));
    assert!(plan.start.is_empty(), "{:?}", plan.start);
    let (_, skip) = plan
        .skip
        .iter()
        .find(|(t, _)| t.id == "implement-50-a1")
        .expect("#50 要排队");
    assert!(matches!(skip, Skip::DailyCap(1)), "{skip:?}");
    assert!(skip.worth_explaining(), "每日上限要在 issue 上说明");
    // 昨天跑过的会话不算今天的额度。
    let mut yesterday = state.sessions.clone();
    yesterday[0].started_at = at("2026-10-13T23:00:00+08:00");
    let plan = decide(
        &snapshot,
        &State {
            sessions: yesterday,
            ..State::default()
        },
        &config,
        at("2026-10-14T04:00:00+08:00"),
    );
    assert_eq!(
        plan.start.len(),
        1,
        "新的一天额度重新开始：{:?}",
        plan.start
    );
}

#[test]
fn a_red_main_is_the_first_thing_dispatched_but_a_cancelled_run_is_not_red() {
    let snapshot = Snapshot {
        issues: vec![issue(50, &[], None)],
        pulls: vec![],
        main_checks: MainChecks::Red,
        main_run: None,
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    assert_eq!(plan.start[0].role, Role::FixCi, "{:?}", plan.start);

    let snapshot = Snapshot {
        main_checks: MainChecks::Running,
        ..snapshot
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    assert!(
        !plan.start.iter().any(|t| t.role == Role::FixCi),
        "还在跑的检查不是红的：{:?}",
        plan.start
    );
}

#[test]
fn roles_the_cloud_handles_are_not_dispatched_here() {
    let snapshot = Snapshot {
        issues: vec![issue(51, &["待验收"], Some("claude"))],
        pulls: vec![pull(60, &["待审查"], Some("claude"), None)],
        main_checks: MainChecks::Red,
        main_run: None,
    };
    let config = Config {
        cloud_roles: ["review", "fix-ci"].iter().map(|s| s.to_string()).collect(),
        ..config()
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config,
        at("2026-10-14T04:00:00+08:00"),
    );
    assert_eq!(
        plan.start.iter().map(|t| t.role.name()).collect::<Vec<_>>(),
        vec!["accept"],
        "云端负责的角色本机不派：{:?}",
        plan.start
    );
    assert!(skipped(&plan, "review-60-91f0f06-a1").contains("#9"));
    assert!(skipped(&plan, "fix-ci-a1").contains("#9"));
}

#[test]
fn only_work_carrying_the_required_label_is_touched() {
    let snapshot = Snapshot {
        issues: vec![issue(50, &[], None), issue(51, &["演示:调度"], None)],
        pulls: vec![pull(60, &["待审查"], Some("claude"), None)],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let config = Config {
        require_label: "演示:调度".to_string(),
        ..config()
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config,
        at("2026-10-14T04:00:00+08:00"),
    );
    assert_eq!(
        plan.start.iter().map(|t| t.issue).collect::<Vec<_>>(),
        vec![Some(51)],
        "只认带演示标签的活：{:?}",
        plan.start
    );
    assert!(
        plan.skip.is_empty(),
        "别人的活连跳过都不提：{:?}",
        plan.skip
    );
}

#[test]
fn an_issue_assigned_to_one_agent_only_goes_to_that_agent() {
    let snapshot = Snapshot {
        issues: vec![issue(50, &["指派:codex"], None)],
        pulls: vec![],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    assert_eq!(plan.start[0].vendor, "codex", "{:?}", plan.start);
}

#[test]
fn ignored_issues_are_never_dispatched() {
    let snapshot = Snapshot {
        issues: vec![issue(20, &[], None)],
        pulls: vec![],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let config = Config {
        ignore_issues: vec![20],
        ..config()
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config,
        at("2026-10-14T04:00:00+08:00"),
    );
    assert!(plan.start.is_empty(), "{:?}", plan.start);
    assert!(plan.skip.is_empty(), "{:?}", plan.skip);
}

#[test]
fn an_ignored_issue_takes_the_pull_request_for_it_with_it() {
    // `ignore_issues` 是用来放过“等 owner 裁决”的活的。PR 和它的 issue 是同一件
    // 活，所以排除 issue 也排除那个 PR。
    let snapshot = Snapshot {
        issues: vec![],
        pulls: vec![pull(33, &["待审查"], Some("claude"), Some(16))],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let config = Config {
        ignore_issues: vec![16],
        ..config()
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config,
        at("2026-10-14T04:00:00+08:00"),
    );
    assert!(
        plan.start.is_empty(),
        "被排除的 issue 的 PR 也不派：{:?}",
        plan.start
    );
    assert!(plan.skip.is_empty(), "连跳过都不提：{:?}", plan.skip);
}

#[test]
fn a_pull_request_waits_while_its_own_issue_has_a_session() {
    // 排除别的 issue 不该切断 PR 和它自己 issue 的关联，否则同一件活会同时
    // 跑两个会话。
    let snapshot = Snapshot {
        issues: vec![],
        pulls: vec![pull(33, &["待审查"], Some("claude"), Some(16))],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let config = Config {
        ignore_issues: vec![20],
        ..config()
    };
    let mut live = running(
        "implement-16-a1",
        Some(16),
        "claude",
        "2026-10-14T03:50:00+08:00",
    );
    live.pid = Some(std::process::id());
    let state = State {
        sessions: vec![live],
        ..State::default()
    };
    let plan = decide(&snapshot, &state, &config, at("2026-10-14T04:00:00+08:00"));
    assert!(
        plan.start.is_empty(),
        "#16 已经有会话在做，它的 PR 要排队：{:?}",
        plan.start
    );
    assert!(
        skipped(&plan, "review-33-91f0f06-a1").contains("implement-16-a1"),
        "{:?}",
        plan.skip
    );
}

#[test]
fn a_red_main_stops_new_work_from_being_stacked_on_it() {
    // AGENTS.md：main 红着的时候不要往上叠新改动。先修红的那个会话跑起来，
    // 不等于可以同时开新实现，也不等于可以把别的 PR 合进红的 main。
    let snapshot = Snapshot {
        issues: vec![
            issue(50, &[], None),
            issue(51, &["待验收"], Some("codebuddy")),
        ],
        pulls: vec![pull(60, &["待审查"], Some("codebuddy"), None)],
        main_checks: MainChecks::Red,
        main_run: Some(38456076570),
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    assert_eq!(
        plan.start.iter().map(|t| t.role.name()).collect::<Vec<_>>(),
        vec!["fix-ci", "accept"],
        "红 main 上只修红，验收不碰 main 所以照常：{:?}",
        plan.start
    );
    assert!(
        skipped(&plan, "implement-50-a1").contains("红"),
        "{:?}",
        plan.skip
    );
    assert!(
        skipped(&plan, "review-60-91f0f06-a1").contains("红"),
        "{:?}",
        plan.skip
    );

    // 修红的活归云端时也一样：本机不派新实现，等云端把红修好。
    let config = Config {
        cloud_roles: vec!["fix-ci".to_string()],
        ..config()
    };
    let plan = decide(
        &snapshot,
        &State::default(),
        &config,
        at("2026-10-14T04:00:00+08:00"),
    );
    assert_eq!(
        plan.start.iter().map(|t| t.role.name()).collect::<Vec<_>>(),
        vec!["accept"],
        "修红归云端不等于可以在红 main 上叠新实现：{:?}",
        plan.start
    );
}

#[test]
fn a_daily_job_runs_once_a_day_after_its_time() {
    let config = Config {
        scheduled: vec![Scheduled {
            name: "nightly-review".to_string(),
            at: "03:00".to_string(),
            vendor: "codex".to_string(),
            instruction: "按 prompts/nightly-review.md 复盘".to_string(),
        }],
        ..config()
    };
    let empty = Snapshot {
        main_checks: MainChecks::Green,
        ..Snapshot::default()
    };
    let before = decide(
        &empty,
        &State::default(),
        &config,
        at("2026-10-14T02:59:00+08:00"),
    );
    assert!(before.start.is_empty(), "时间没到不派：{:?}", before.start);

    let after = decide(
        &empty,
        &State::default(),
        &config,
        at("2026-10-14T03:01:00+08:00"),
    );
    assert_eq!(
        after.start[0].role,
        Role::Scheduled("nightly-review".to_string())
    );
    assert_eq!(after.start[0].vendor, "codex");

    let state = State {
        scheduled_ran: [("nightly-review".to_string(), "2026-10-14".to_string())]
            .into_iter()
            .collect(),
        ..State::default()
    };
    let again = decide(&empty, &state, &config, at("2026-10-14T03:05:00+08:00"));
    assert!(
        again.start.is_empty(),
        "今天跑过就不再跑：{:?}",
        again.start
    );
    let tomorrow = decide(&empty, &state, &config, at("2026-10-15T03:05:00+08:00"));
    assert_eq!(
        tomorrow.start.len(),
        1,
        "第二天照常跑：{:?}",
        tomorrow.start
    );
}

#[test]
fn a_session_that_ran_too_long_is_swept_and_its_claim_released() {
    let state = State {
        sessions: vec![{
            let mut s = running(
                "accept-51-a1",
                Some(51),
                "codex",
                "2026-10-14T01:00:00+08:00",
            );
            // Alive, so only the ceiling can end it.
            s.pid = Some(std::process::id());
            s
        }],
        ..State::default()
    };
    let config = Config {
        session_timeout_minutes: 120,
        ..config()
    };
    let snapshot = Snapshot {
        main_checks: MainChecks::Green,
        ..Snapshot::default()
    };
    let not_yet = decide(&snapshot, &state, &config, at("2026-10-14T02:59:00+08:00"));
    assert!(
        not_yet.sweep.is_empty(),
        "没到上限不动它：{:?}",
        not_yet.sweep
    );

    let plan = decide(&snapshot, &state, &config, at("2026-10-14T03:30:00+08:00"));
    assert_eq!(plan.sweep.len(), 1, "{:?}", plan.sweep);
    let sweep = &plan.sweep[0];
    assert_eq!(sweep.outcome, Outcome::TimedOut);
    assert_eq!(sweep.issue, Some(51));
    assert_eq!(sweep.after_minutes, 150);
    assert!(sweep.release_claim, "超时要释放认领：{sweep:?}");
}

#[test]
fn a_session_whose_process_is_gone_is_closed_without_touching_the_claim() {
    let state = State {
        // pid None: the process cannot be found, which is what a
        // dispatcher restarted mid-session sees.
        sessions: vec![running(
            "accept-51-a1",
            Some(51),
            "codex",
            "2026-10-14T03:50:00+08:00",
        )],
        ..State::default()
    };
    let snapshot = Snapshot {
        main_checks: MainChecks::Green,
        ..Snapshot::default()
    };
    let plan = decide(
        &snapshot,
        &state,
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    assert_eq!(plan.sweep.len(), 1, "{:?}", plan.sweep);
    assert_eq!(plan.sweep[0].outcome, Outcome::Vanished);
    assert!(
        !plan.sweep[0].release_claim,
        "自己结束的会话不去动认领：{:?}",
        plan.sweep[0]
    );
}

#[test]
fn a_session_the_dispatcher_lost_track_of_is_not_dispatched_again() {
    // The dispatcher restarted while the session ran, so it never saw an
    // exit code. The session may well have done the work; doing it twice
    // is worse than waiting for the labels to say what is still missing.
    let snapshot = Snapshot {
        issues: vec![issue(51, &["待验收"], Some("claude"))],
        pulls: vec![],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let mut gone = running(
        "accept-51-a1",
        Some(51),
        "codex",
        "2026-10-14T00:00:00+08:00",
    );
    gone.finished_at = Some(at("2026-10-14T02:00:00+08:00"));
    gone.outcome = Some(Outcome::Vanished);
    let state = State {
        sessions: vec![gone],
        ..State::default()
    };

    let plan = decide(
        &snapshot,
        &state,
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );

    assert!(
        plan.start.is_empty(),
        "不知所终的会话不自动再派一次：{:?}",
        plan.start
    );
}

#[test]
fn the_instruction_says_the_task_the_rules_and_the_ceiling() {
    let task = Task {
        id: "accept-51-a1".to_string(),
        role: Role::Accept,
        vendor: "codex".to_string(),
        issue: Some(51),
        pull: None,
        why: "#51 等另一个厂商验收".to_string(),
    };
    let text = instruction(&task, &config(), std::path::Path::new("/tmp/w"));
    assert!(text.contains("prompts/acceptance.md"), "{text}");
    assert!(text.contains("#51"), "{text}");
    assert!(text.contains("accept-51-a1"), "{text}");
    assert!(text.contains("/tmp/w"), "{text}");
    assert!(text.contains("Agent: codex"), "{text}");
    assert!(text.contains("120 分钟"), "{text}");
}

#[test]
fn the_vendor_with_the_fewest_sessions_today_gets_the_next_one() {
    let snapshot = Snapshot {
        issues: vec![issue(50, &[], None)],
        pulls: vec![],
        main_checks: MainChecks::Green,
        main_run: None,
    };
    let mut claude = running(
        "implement-40-a1",
        Some(40),
        "claude",
        "2026-10-14T01:00:00+08:00",
    );
    claude.finished_at = Some(at("2026-10-14T01:30:00+08:00"));
    claude.outcome = Some(Outcome::Done);
    let mut codex = running(
        "implement-41-a1",
        Some(41),
        "codex",
        "2026-10-14T02:00:00+08:00",
    );
    codex.finished_at = Some(at("2026-10-14T02:30:00+08:00"));
    codex.outcome = Some(Outcome::Done);
    let state = State {
        sessions: vec![claude, codex],
        ..State::default()
    };
    let plan = decide(
        &snapshot,
        &state,
        &config(),
        at("2026-10-14T04:00:00+08:00"),
    );
    assert_eq!(plan.start[0].vendor, "codebuddy", "{:?}", plan.start);
}
