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
    fixtures_with_file(name, &fixtures().join(from))
}

/// The same, with the replacement taken from anywhere under
/// `testdata/github/claims`, so a case can use a response recorded for
/// another purpose (the push activity of `night/issue-16`, say).
fn fixtures_with_file(name: &str, from: &Path) -> PathBuf {
    let dir = temp("replay");
    for entry in std::fs::read_dir(fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    std::fs::copy(from, dir.join(name)).unwrap();
    dir
}

/// A response recorded outside `two-claims/`, by path under
/// `testdata/github/claims`.
fn recorded(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/github/claims")
        .join(path)
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

    let claim = claims(
        &replay,
        "2026-10-14T06:10:00+08:00",
        &[
            "claim",
            "--issue",
            "31",
            "--agent",
            "Claude",
            "--modules",
            "docs/demo",
        ],
    );
    assert_eq!(claim.code, 1, "{}", claim.out);
    assert!(
        claim.out.contains("问清楚"),
        "读不出来的认领可能有人在做，不能直接盖掉：\n{}",
        claim.out
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
fn a_claim_whose_label_never_made_it_on_is_not_overwritten() {
    // The claim comment goes out before the label (claim-lease.md), so a
    // `gh issue edit` that fails leaves a live lease on an issue the
    // label-filtered snapshot cannot see. #25 here carries DemoAgent's
    // claim and heartbeat with no `认领中` label: the next claim has to
    // read the issue it is about, not only the snapshot.
    let replay = fixtures_with("comments-25.json", "comments-31.json");
    let taken = claims(
        &replay,
        "2026-10-14T03:20:00+08:00",
        &[
            "claim",
            "--issue",
            "25",
            "--agent",
            "Codex",
            "--modules",
            "docs/unrelated",
        ],
    );
    assert_eq!(taken.code, 1, "{}", taken.out);
    assert!(
        taken.out.contains("DemoAgent") && taken.out.contains("认领中"),
        "已有租约即使标签没加上也不能被覆盖：\n{}",
        taken.out
    );

    // The holder gets its label back by reporting progress, which is the
    // recovery path: nothing else has to be done by hand.
    let beat = claims(
        &replay,
        "2026-10-14T03:20:00+08:00",
        &[
            "heartbeat",
            "--issue",
            "25",
            "--agent",
            "DemoAgent",
            "--note",
            "还在做，标签不见了",
        ],
    );
    assert_eq!(beat.code, 0, "{}", beat.out);
    assert!(
        beat.out.contains("认领中") && beat.out.contains("补"),
        "心跳要把丢掉的标签补回去：\n{}",
        beat.out
    );

    // And once it really has gone quiet, it is taken over the usual way.
    let reclaim = claims(
        &replay,
        "2026-10-14T06:10:00+08:00",
        &["reclaim", "--issue", "25", "--agent", "Codex"],
    );
    assert_eq!(reclaim.code, 0, "{}", reclaim.out);
    assert!(
        reclaim.out.contains("## 收回认领") && reclaim.out.contains("DemoAgent"),
        "{}",
        reclaim.out
    );
}

#[test]
fn a_write_up_that_quotes_the_commands_does_not_resurrect_a_handed_back_claim() {
    // #31's real comments, including the demo write-up that quotes whole
    // claim and hand-back comments inside fenced code blocks. The claim
    // was handed back at 03:08; the write-up at 03:21 is a report, not a
    // new claim.
    let replay = fixtures_with("comments-31.json", "comments-31-with-demo-record.json");
    let run = claims(&replay, "2026-10-14T03:30:00+08:00", &["list"]);
    assert_eq!(run.code, 0, "{}", run.out);
    assert!(
        run.out.contains("有人收回了它"),
        "代码块里的 ## 认领 不是一条新认领，这个 issue 已经被收回了：\n{}",
        run.out
    );
    assert!(
        run.out.contains("1 条认领"),
        "只剩 #16 一条认领：\n{}",
        run.out
    );

    let free = claims(
        &replay,
        "2026-10-14T03:30:00+08:00",
        &[
            "claim",
            "--issue",
            "31",
            "--agent",
            "Claude",
            "--modules",
            "docs/demo",
        ],
    );
    assert_eq!(free.code, 0, "{}", free.out);
}

#[test]
fn the_push_that_keeps_a_lease_alive_is_the_push_not_the_commit() {
    // Both responses are real and for the same branch: `night/issue-16`
    // was pushed at 19:35:52Z, and the commit that push carried says
    // 19:35:13Z. A lease lives by when the push happened, so a commit
    // made offline hours earlier and pushed just now keeps it alive.
    let replay = fixtures_with_file(
        "branch-night-issue-16.json",
        &recorded("push-vs-commit/activity-night-issue-16.json"),
    );
    let run = claims(&replay, "2026-10-14T03:40:00+08:00", &["list", "--json"]);
    assert_eq!(run.code, 0, "{}", run.out);
    let board: serde_json::Value = serde_json::from_str(run.out.trim()).expect(&run.out);
    let sixteen = board["leases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["issue"] == 16)
        .expect(&run.out)
        .clone();
    assert_eq!(sixteen["last_update_was"], "推送 night/issue-16");
    assert_eq!(
        sixteen["last_update"], "2026-10-14T03:35:52+08:00",
        "租约的最后更新是推送的时间：\n{}",
        run.out
    );

    let commits =
        std::fs::read_to_string(recorded("push-vs-commit/commits-night-issue-16.json")).unwrap();
    let commits: serde_json::Value = serde_json::from_str(&commits).unwrap();
    let committed = commits[0]["commit"]["committer"]["date"].as_str().unwrap();
    assert_eq!(committed, "2026-10-13T19:35:13Z");
    assert_ne!(
        sixteen["last_update"], "2026-10-14T03:35:13+08:00",
        "commit 的提交时间不是推送时间，差了 {committed} 到 19:35:52Z"
    );
}

#[test]
fn a_lease_whose_push_time_cannot_be_read_is_not_taken_over() {
    // Reading the pushes of a branch needs push access to the repository;
    // without it GitHub answers like this. Silence cannot be proved, so
    // the lease is reported and reclaim refuses, instead of counting the
    // branch as never pushed and handing the issue to someone else.
    let replay = fixtures_with_file(
        "branch-night-issue-16.json",
        &recorded("push-vs-commit/no-push-access.json"),
    );
    let run = claims(&replay, "2026-10-14T06:10:00+08:00", &["list"]);
    assert_eq!(run.code, 0, "{}", run.out);
    assert!(
        run.out.contains("推送时间读不到"),
        "读不到推送时间要说出来：\n{}",
        run.out
    );

    let reclaim = claims(
        &replay,
        "2026-10-14T06:10:00+08:00",
        &["reclaim", "--issue", "16", "--agent", "Codex"],
    );
    assert_eq!(reclaim.code, 1, "{}", reclaim.out);
    assert!(
        reclaim.out.contains("推送时间读不到"),
        "证明不了它沉默就不能收回：\n{}",
        reclaim.out
    );
}

#[test]
fn an_unlabelled_target_lease_still_reads_its_push_heartbeat() {
    let replay = fixtures_with_file(
        "branch-night-issue-16.json",
        &recorded("push-vs-commit/activity-night-issue-16.json"),
    );
    let issues: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(replay.join("issues.json")).unwrap())
            .unwrap();
    let issues: Vec<_> = issues
        .into_iter()
        .filter(|issue| issue["number"] != 16)
        .collect();
    std::fs::write(
        replay.join("issues.json"),
        serde_json::to_vec_pretty(&issues).unwrap(),
    )
    .unwrap();
    let mut issue: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(replay.join("issue-16.json")).unwrap())
            .unwrap();
    issue["labels"] = serde_json::json!([]);
    std::fs::write(
        replay.join("issue-16.json"),
        serde_json::to_vec_pretty(&issue).unwrap(),
    )
    .unwrap();

    let reclaim = claims(
        &replay,
        "2026-10-14T05:30:00+08:00",
        &["reclaim", "--issue", "16", "--agent", "Codex"],
    );
    assert_eq!(
        reclaim.code, 1,
        "03:35 的推送还不到两小时；缺标签不能让目标命令漏掉分支心跳：\n{}",
        reclaim.out
    );
    assert!(
        reclaim.out.contains("还没过期"),
        "拒绝要来自最近的推送心跳：\n{}",
        reclaim.out
    );
}

/// A stand-in for `gh` that answers from a directory of saved responses
/// and appends what it is told to post, so the write paths of two
/// `claims` processes can run at once without GitHub.
///
/// It stands in for the one thing GitHub does that matters here: posting
/// a comment is serialised and gets an id that rises, so the comments
/// have an order both processes see. Reads of `--barrier-issue`'s
/// comments wait until both processes have asked, which is the moment two
/// agents both read "nobody is working on this".
const FAKE_GH: &str = r#"#!/bin/sh
# A fake gh for tests/claims.rs. It never reaches the network.
PATH=/usr/bin:/bin
export PATH
D="$CLAIMS_FAKE_GH_DIR"
lock() { while ! mkdir "$D/lock" 2>/dev/null; do sleep 0.02; done; }
unlock() { rmdir "$D/lock"; }
if [ "$1" = api ]; then
  target=""
  for a in "$@"; do case "$a" in repos/*) target="$a";; esac; done
  case "$target" in
    */issues\?*) cat "$D/issues.json" ;;
    */comments*)
      n=$(printf '%s' "$target" | sed -e 's#.*/issues/##' -e 's#/comments.*##')
      if [ -f "$D/comments-$n.json" ]; then body=$(cat "$D/comments-$n.json"); else body='[]'; fi
      # The answer is what the comments were when this process asked, so
      # both processes see the issue as free however the two runs
      # interleave after the barrier lets them go.
      if [ "$n" = "$CLAIMS_FAKE_GH_BARRIER" ] && [ ! -f "$D/arrived.$PPID" ]; then
        : > "$D/arrived.$PPID"
        echo "arrive-$n $PPID" >> "$D/log"
        i=0
        while [ "$(ls "$D" | grep -c '^arrived\.')" -lt 2 ] && [ "$i" -lt 300 ]; do
          sleep 0.1
          i=$((i + 1))
        done
      fi
      echo "read-$n $PPID" >> "$D/log"
      printf '%s\n' "$body" ;;
    */activity*)
      b=$(printf '%s' "$target" | sed -e 's#.*refs/heads/##' -e 's#&.*##' -e 's#%2F#/#g' | tr '/' '-')
      if [ -f "$D/branch-$b.json" ]; then cat "$D/branch-$b.json"; else echo '[]'; fi ;;
    */issues/*)
      n=$(printf '%s' "$target" | sed 's#.*/issues/##')
      cat "$D/issue-$n.json" ;;
    *) echo "fake gh: 没想到的请求 $target" >&2; exit 1 ;;
  esac
  exit 0
fi
if [ "$1" = issue ] && [ "$2" = comment ]; then
  n="$3"
  body=""
  while [ $# -gt 0 ]; do
    if [ "$1" = --body-file ]; then body="$2"; fi
    shift
  done
  lock
  id=$(cat "$D/next-id" 2>/dev/null || echo 1000)
  echo $((id + 1)) > "$D/next-id"
  f="$D/comments-$n.json"
  if [ -n "$CLAIMS_FAKE_GH_BEFORE_COMMENT" ] && [ -f "$CLAIMS_FAKE_GH_BEFORE_COMMENT" ]; then
    cp "$CLAIMS_FAKE_GH_BEFORE_COMMENT" "$f"
    rm "$CLAIMS_FAKE_GH_BEFORE_COMMENT"
  fi
  [ -f "$f" ] || echo '[]' > "$f"
  escaped=$(sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' "$body" | awk '{printf "%s\\n", $0}')
  url="https://github.com/zbtzbtzbt/theFlash/issues/$n#issuecomment-$id"
  entry="{\"id\": $id, \"created_at\": \"$CLAIMS_FAKE_GH_CREATED\", \"html_url\": \"$url\", \"body\": \"$escaped\"}"
  if grep -q '"id"' "$f"; then
    sed -e '$ d' "$f" > "$f.new"
    printf ',\n%s\n]\n' "$entry" >> "$f.new"
  else
    printf '[\n%s\n]\n' "$entry" > "$f.new"
  fi
  mv "$f.new" "$f"
  echo "comment-$n $PPID $id" >> "$D/log"
  unlock
  echo "$url"
  exit 0
fi
if [ "$1" = issue ] && [ "$2" = edit ]; then
  lock
  echo "label $3 $*" >> "$D/labels"
  unlock
  exit 0
fi
echo "fake gh: 没想到的命令 $*" >&2
exit 1
"#;

/// `claims` with the fake `gh` above on PATH instead of nothing, so a
/// test can exercise the paths that post comments. The responses and
/// everything posted stay in `dir`.
fn claims_with_fake_gh(dir: &Path, args: &[&str]) -> Command {
    let mut cmd: Command = support::claims(&temp("state"));
    let bin = temp("fake-gh");
    let gh = bin.join("gh");
    std::fs::write(&gh, FAKE_GH).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    cmd.env("PATH", bin)
        .env("CLAIMS_FAKE_GH_DIR", dir)
        .env("CLAIMS_FAKE_GH_BARRIER", "25")
        // Both claims land in the same second, which is the hardest case:
        // only GitHub's comment id says which came first.
        .env("CLAIMS_FAKE_GH_CREATED", "2026-10-13T19:20:00Z")
        .args(args);
    cmd
}

#[test]
fn two_sessions_of_the_same_agent_claiming_at_the_same_moment_do_not_both_start() {
    // Nothing stops two agents from posting a claim at the same moment:
    // GitHub has no compare-and-set, and the agents run in different
    // worktrees. So the claim is confirmed after it is posted, and the
    // one that lost gives it up. The fake gh above holds both processes
    // at the read, so both of them really do see "#25 is free".
    let dir = temp("race");
    for entry in std::fs::read_dir(fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    std::fs::remove_file(dir.join("comments-25.json")).unwrap();

    let both: Vec<Run> = ["Codex", "Codex"]
        .iter()
        .map(|agent| {
            let cmd = claims_with_fake_gh(
                &dir,
                &[
                    "claim",
                    "--issue",
                    "25",
                    "--agent",
                    agent,
                    "--modules",
                    "docs/race",
                    "--now",
                    "2026-10-14T03:20:00+08:00",
                ],
            );
            std::thread::spawn(move || run(cmd))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();

    let log = std::fs::read_to_string(dir.join("log")).unwrap();
    let before_any_write = log
        .lines()
        .take_while(|l| !l.starts_with("comment-25"))
        .filter(|l| l.starts_with("arrive-25"))
        .count();
    assert_eq!(
        before_any_write, 2,
        "两个进程都要在任何一条认领发出去之前读到“没人认领”，否则这个测试没在测并发：\n{log}"
    );

    let held: Vec<&Run> = both.iter().filter(|r| r.code == 0).collect();
    let gave_up: Vec<&Run> = both.iter().filter(|r| r.code == 1).collect();
    assert_eq!(
        held.len(),
        1,
        "同时认领同一个 issue，只能有一个成功：\n{}\n----\n{}",
        both[0].out,
        both[1].out
    );
    assert!(
        held[0].out.contains("加上了 认领中 标签"),
        "赢的那个要把标签加上：\n{}",
        held[0].out
    );
    assert_eq!(gave_up.len(), 1);
    let gave_up = &gave_up[0].out;
    assert!(
        gave_up.contains("让出") && gave_up.contains("没有加 认领中 标签"),
        "输的那个要让出、不加标签、以非 0 退出：\n{gave_up}"
    );

    let labels = std::fs::read_to_string(dir.join("labels")).unwrap();
    assert_eq!(
        labels.lines().count(),
        1,
        "只有一个 agent 该加标签：\n{labels}"
    );
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("comments-25.json")).unwrap())
            .unwrap();
    assert_eq!(
        after.as_array().unwrap().len(),
        3,
        "issue 上留下两条认领和一条让出：{after:#}"
    );

    // A third agent reading the issue afterwards sees one holder: the
    // process that won, not the one whose claim comment came last.
    let third = claims(
        &dir,
        "2026-10-14T03:25:00+08:00",
        &[
            "claim",
            "--issue",
            "25",
            "--agent",
            "Claude",
            "--modules",
            "docs/race",
        ],
    );
    assert_eq!(third.code, 1, "{}", third.out);
    assert!(
        third.out.contains("Codex"),
        "#25 的持有者是先写入的那条 Codex 会话，不是同名后到者：\n{}",
        third.out
    );
}

#[test]
fn a_stale_reclaim_does_not_remove_a_new_leases_label() {
    let dir = temp("stale-reclaim");
    for entry in std::fs::read_dir(fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    let old_url = "https://github.com/zbtzbtzbt/theFlash/issues/31#issuecomment-6153109240";
    let mut before: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("comments-31.json")).unwrap())
            .unwrap();
    before.push(serde_json::json!({
        "id": 900,
        "created_at": "2026-10-13T22:08:00Z",
        "html_url": "https://github.com/zbtzbtzbt/theFlash/issues/31#issuecomment-900",
        "body": format!("## 收回认领\n\n- 原认领: DemoAgent（{old_url}）\n- 收回者: Codex\n")
    }));
    before.push(serde_json::json!({
        "id": 901,
        "created_at": "2026-10-13T22:09:00Z",
        "html_url": "https://github.com/zbtzbtzbt/theFlash/issues/31#issuecomment-901",
        "body": "## 认领\n\n- agent: Claude\n- 改动模块: docs/new-work\n"
    }));
    let injected = dir.join("comments-before-stale-reclaim.json");
    std::fs::write(&injected, serde_json::to_vec_pretty(&before).unwrap()).unwrap();

    let mut cmd = claims_with_fake_gh(
        &dir,
        &[
            "reclaim",
            "--issue",
            "31",
            "--agent",
            "CodeBuddy",
            "--now",
            "2026-10-14T06:10:00+08:00",
        ],
    );
    cmd.env("CLAIMS_FAKE_GH_BEFORE_COMMENT", &injected);
    let run = run(cmd);
    assert_eq!(run.code, 0, "{}", run.out);
    assert!(
        run.out.contains("Claude") && run.out.contains("标签留着"),
        "迟到的收回要看见并保留中间的新租约：\n{}",
        run.out
    );
    assert!(
        !dir.join("labels").exists(),
        "旧快照发出的收回不能请求删除新租约的标签"
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
