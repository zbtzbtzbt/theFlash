//! `claims` 这条命令：用保存下来的真实 GitHub 响应，走它在终端里的样子（#16）。
//!
//! Every case replays `testdata/github/claims/two-claims`, the responses
//! GitHub gave for issues #16 and #31 while both were claimed, so the
//! command is tested on the bytes it will really read. `support::claims`
//! takes `gh` off PATH, so a run that forgot `--replay` fails instead of
//! asking the real GitHub, and nothing here posts a comment: the write
//! paths stop at printing what they would post.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The live snapshot: #16 claimed by Claude (modules include
/// `backend/src/claim` and `AGENTS.md`, branch `night/issue-16` pushed at
/// 2026-10-14 03:07 +08:00) and #31 claimed by DemoAgent (module
/// `docs/demo/claim-lease-demo.md`, heartbeat at 03:06).
fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/github/claims/two-claims")
}

/// A copy of the saved snapshot with one response replaced, the way the
/// merge gate's tests do it: a case that differs from the live snapshot
/// in a single file stays honest about what it changed. The copy is in a
/// temporary directory, so no test writes into `testdata/`.
fn fixtures_with(name: &str, from: &str) -> PathBuf {
    let dir = temp("replay");
    for entry in std::fs::read_dir(fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    std::fs::copy(fixtures().join(from), dir.join(name)).unwrap();
    dir
}

struct Run {
    out: String,
    code: i32,
}

/// `claims <args>` on the snapshot in `replay` at `now`, with its stdout
/// and stderr together, which is what an agent reads.
fn claims(replay: &Path, now: &str, args: &[&str]) -> Run {
    let mut cmd: Command = support::claims(&temp("state"));
    cmd.args(args)
        .arg("--replay")
        .arg(replay)
        .arg("--now")
        .arg(now);
    run(cmd)
}

fn run(cmd: Command) -> Run {
    let out = support::output_within(cmd, 30);
    Run {
        out: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        code: out.status.code().unwrap_or(-1),
    }
}

/// A fresh temporary directory, named after what it is for. Every run
/// gets its own, so the state a run writes cannot reach another test or
/// the real home directory.
fn temp(what: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "claims-test-{what}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn list_says_who_holds_what_and_when_it_was_last_alive() {
    let run = claims(&fixtures(), "2026-10-14T03:20:00+08:00", &["list"]);
    assert_eq!(run.code, 0, "{}", run.out);
    for want in [
        "#16",
        "Claude",
        "backend/src/claim",
        "推送 night/issue-16",
        "#31",
        "DemoAgent",
        "docs/demo/claim-lease-demo.md",
        "心跳评论",
        "活跃",
        "2 条认领：2 活跃，0 该心跳了，0 已过期",
    ] {
        assert!(
            run.out.contains(want),
            "list 里少了 {want:?}：\n{}",
            run.out
        );
    }
}

#[test]
fn list_shows_a_claim_as_expired_once_it_has_been_silent_for_two_hours() {
    let run = claims(&fixtures(), "2026-10-14T06:10:00+08:00", &["list"]);
    assert_eq!(run.code, 0, "{}", run.out);
    assert!(
        run.out.contains("已过期，可以收回"),
        "两小时不说话的认领要显示成过期：\n{}",
        run.out
    );
    assert!(
        run.out.contains("2 条认领：0 活跃，0 该心跳了，2 已过期"),
        "{}",
        run.out
    );
    assert!(
        run.out.contains("3 小时 3 分钟前"),
        "要写出最后更新离现在多久：\n{}",
        run.out
    );
}

#[test]
fn list_as_json_carries_the_fields_a_dispatcher_decides_on() {
    let run = claims(
        &fixtures(),
        "2026-10-14T06:10:00+08:00",
        &["list", "--json"],
    );
    assert_eq!(run.code, 0, "{}", run.out);
    let board: serde_json::Value = serde_json::from_str(run.out.trim()).expect(&run.out);
    assert_eq!(board["expires_after_minutes"], 120);
    let demo = board["leases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["issue"] == 31)
        .expect(&run.out);
    assert_eq!(demo["agent"], "DemoAgent");
    assert_eq!(demo["modules"][0], "docs/demo/claim-lease-demo.md");
    assert_eq!(demo["expired"], true);
    assert_eq!(demo["idle_minutes"], 183);
    assert_eq!(demo["last_update"], "2026-10-14T03:06:20+08:00");
    assert_eq!(demo["last_update_was"], "心跳评论");
}

#[test]
fn claiming_what_another_agent_is_changing_is_refused_until_the_split_is_written_down() {
    // #25 carries no claim, so the refusal is about the modules #16's
    // claim covers, not about #25 being taken.
    let overlapping = &[
        "claim",
        "--issue",
        "25",
        "--agent",
        "Codex",
        "--modules",
        "backend/src/claim/lease.rs",
    ];
    let run = claims(&fixtures(), "2026-10-14T03:20:00+08:00", overlapping);
    assert_eq!(run.code, 1, "{}", run.out);
    assert!(
        run.out.contains("#16") && run.out.contains("--overlap"),
        "拒绝要说清和谁重叠、怎么继续：\n{}",
        run.out
    );

    let mut with_plan = overlapping.to_vec();
    with_plan.extend(["--overlap", "#16 先合，我排在它后面"]);
    let run = claims(&fixtures(), "2026-10-14T03:20:00+08:00", &with_plan);
    assert_eq!(run.code, 0, "{}", run.out);
    assert!(
        run.out
            .contains("- 重叠: 和 #16（Claude）的 backend/src/claim/lease.rs"),
        "放行时要把分工写进认领评论：\n{}",
        run.out
    );
    assert!(
        run.out.contains("没有真的发评论"),
        "回放的数据写不回去：\n{}",
        run.out
    );
}

#[test]
fn claiming_an_issue_someone_else_holds_names_the_agent_and_when_it_expires() {
    let run = claims(
        &fixtures(),
        "2026-10-14T03:20:00+08:00",
        &[
            "claim",
            "--issue",
            "31",
            "--agent",
            "Claude",
            "--modules",
            "docs/demo/claim-lease-demo.md",
        ],
    );
    assert_eq!(run.code, 1, "{}", run.out);
    assert!(
        run.out.contains("DemoAgent") && run.out.contains("才算过期"),
        "{}",
        run.out
    );
}

#[test]
fn reclaiming_waits_for_the_lease_to_expire() {
    let too_early = claims(
        &fixtures(),
        "2026-10-14T03:20:00+08:00",
        &["reclaim", "--issue", "31", "--agent", "Claude"],
    );
    assert_eq!(too_early.code, 1, "{}", too_early.out);
    assert!(
        too_early.out.contains("还没过期，不能收回"),
        "{}",
        too_early.out
    );

    let expired = claims(
        &fixtures(),
        "2026-10-14T06:10:00+08:00",
        &["reclaim", "--issue", "31", "--agent", "Claude"],
    );
    assert_eq!(expired.code, 0, "{}", expired.out);
    for want in [
        "## 收回认领",
        "- 原认领: DemoAgent",
        "- 最后更新: 2026-10-14T03:06:20+08:00",
        "- 收回者: Claude",
        "没有真的发评论、没有去掉标签",
    ] {
        assert!(
            expired.out.contains(want),
            "收回评论里少了 {want:?}：\n{}",
            expired.out
        );
    }
}

#[test]
fn a_heartbeat_on_an_issue_you_do_not_hold_is_refused() {
    let run = claims(
        &fixtures(),
        "2026-10-14T03:20:00+08:00",
        &[
            "heartbeat",
            "--issue",
            "31",
            "--agent",
            "Claude",
            "--note",
            "顺手报个进度",
        ],
    );
    assert_eq!(run.code, 1, "{}", run.out);
    assert!(run.out.contains("是 DemoAgent 的认领"), "{}", run.out);

    let mine = claims(
        &fixtures(),
        "2026-10-14T03:20:00+08:00",
        &[
            "heartbeat",
            "--issue",
            "16",
            "--agent",
            "Claude（第 4 条线）",
            "--note",
            "租约命令做完了，在补回放测试",
        ],
    );
    assert_eq!(mine.code, 0, "{}", mine.out);
    assert!(mine.out.contains("## 心跳"), "{}", mine.out);
    assert!(mine.out.contains("租约命令做完了"), "{}", mine.out);
}

#[test]
fn an_issue_labelled_claimed_with_no_claim_comment_is_reported_not_treated_as_free() {
    let replay = fixtures_with("comments-31.json", "comments-none.json");
    let run = claims(&replay, "2026-10-14T03:20:00+08:00", &["list"]);
    assert_eq!(run.code, 0, "{}", run.out);
    assert!(
        run.out.contains("读不出来") && run.out.contains("没有认领评论"),
        "没有认领评论的 issue 要被报出来：\n{}",
        run.out
    );
    assert!(
        run.out.contains("另有 1 个 issue 的认领读不出来"),
        "{}",
        run.out
    );

    let reclaim = claims(
        &replay,
        "2026-10-14T06:10:00+08:00",
        &["reclaim", "--issue", "31", "--agent", "Claude"],
    );
    assert_eq!(reclaim.code, 1, "{}", reclaim.out);
    assert!(
        reclaim.out.contains("读不出来"),
        "读不出来的认领不能当成过期的认领收回：\n{}",
        reclaim.out
    );
}

#[test]
fn a_handed_back_claim_does_not_keep_the_issue_looking_busy() {
    let replay = fixtures_with("comments-31.json", "comments-31-after-reclaim.json");
    let run = claims(&replay, "2026-10-14T06:20:00+08:00", &["list"]);
    assert_eq!(run.code, 0, "{}", run.out);
    assert!(
        run.out.contains("有人收回了它"),
        "收回之后标签还在，要说标签该去掉：\n{}",
        run.out
    );
    assert!(
        run.out.contains("1 条认领"),
        "被收回的认领不再算进行中：\n{}",
        run.out
    );
}

#[test]
fn forgetting_replay_fails_instead_of_reading_the_real_github() {
    let mut cmd = support::claims(&temp("state"));
    cmd.arg("list");
    let run = run(cmd);
    assert_eq!(run.code, 2, "{}", run.out);
    assert!(
        run.out.contains("问 GitHub 要") && run.out.contains("失败"),
        "没有 gh 就该说取不到数据：{}",
        run.out
    );
}

#[test]
fn a_wrong_command_line_says_how_to_use_it() {
    for args in [
        vec!["list", "--nope"],
        vec!["claim", "--issue", "31"],
        vec!["heartbeat", "--issue", "31", "--agent", "Claude"],
        vec!["reclaim"],
        vec!["nonsense"],
    ] {
        let mut cmd = support::claims(&temp("state"));
        cmd.args(&args);
        let run = run(cmd);
        assert_eq!(run.code, 2, "{args:?} 应该是用法错：{}", run.out);
        assert!(
            run.out.contains("用法："),
            "{args:?} 要打印用法：{}",
            run.out
        );
    }
}
