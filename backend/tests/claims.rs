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
    */issues\?*labels=*) cat "$D/issues.json" ;;
    */issues\?*) cat "$D/open-issues.json" ;;
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
  if [ -n "$CLAIMS_FAKE_GH_FAIL_COMMENT" ]; then
    echo "fake gh: 发评论失败" >&2
    exit 1
  fi
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
  if [ "$n" = "$CLAIMS_FAKE_GH_COMMENT_BARRIER" ] && [ ! -f "$D/commented.$PPID" ]; then
    : > "$D/commented.$PPID"
    i=0
    while [ "$(ls "$D" | grep -c '^commented\.')" -lt 2 ] && [ "$i" -lt 300 ]; do
      sleep 0.1
      i=$((i + 1))
    done
  fi
  echo "$url"
  exit 0
fi
if [ "$1" = issue ] && [ "$2" = edit ]; then
  n="$3"
  lock
  echo "label $n $*" >> "$D/labels"
  echo "label-$n $PPID $*" >> "$D/log"
  partial=0
  if [ -n "$CLAIMS_FAKE_GH_PARTIAL_EDIT" ] && [ ! -f "$D/partial-done.$n" ]; then
    : > "$D/partial-done.$n"
    partial=1
  fi
  python3 - "$D" "$n" "$partial" "$@" <<'PY'
import json, os, sys
d, num, partial = sys.argv[1], int(sys.argv[2]), sys.argv[3] == "1"
args = sys.argv[4:]
add, rem = [], []
i = 0
while i < len(args):
    if args[i] == "--add-label" and i + 1 < len(args):
        add.append(args[i + 1]); i += 2
    elif args[i] == "--remove-label" and i + 1 < len(args):
        if not partial:
            rem.append(args[i + 1])
        i += 2
    else:
        i += 1
def apply_labels(issue):
    names = [l["name"] for l in issue.get("labels", []) if l["name"] not in rem]
    for a in add:
        if a not in names:
            names.append(a)
    issue["labels"] = [{"name": x} for x in names]
p = os.path.join(d, f"issue-{num}.json")
if os.path.exists(p):
    data = json.load(open(p))
    apply_labels(data)
    json.dump(data, open(p, "w"), ensure_ascii=False, indent=2)
op = os.path.join(d, "open-issues.json")
if os.path.exists(op):
    items = json.load(open(op))
    for item in items:
        if item.get("number") == num:
            apply_labels(item)
    json.dump(items, open(op, "w"), ensure_ascii=False, indent=2)
PY
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

// ---- priority (#68) ----

/// The repository as it was at 2026-10-14 11:11 +08:00, just after every
/// open issue had been triaged: real responses, recorded with the script
/// in `testdata/github/claims/README.md`. Twelve issues (#70–#81) had
/// been opened minutes earlier and were not triaged yet; #54's body says
/// it depends on #4; #63 and its two duplicates wait for the owner.
fn priority_fixtures() -> PathBuf {
    recorded("priority")
}

const PRIORITY_NOW: &str = "2026-10-14T11:12:00+08:00";

/// Where `needle` first appears in `haystack`, failing the test when it
/// does not, so an order assertion names what it could not find.
fn position(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("输出里没有 {needle:?}：\n{haystack}"))
}

#[test]
fn the_queue_puts_untriaged_issues_first_then_p0_to_p3_and_says_why_the_rest_wait() {
    let run = claims(&priority_fixtures(), PRIORITY_NOW, &["queue"]);
    assert_eq!(run.code, 0, "{}", run.out);
    let out = &run.out;
    let untriaged = position(out, "没定优先级");
    let first_untriaged = position(out, "  #70 ");
    let p1 = position(out, "P1 重要，不紧急");
    let first_p1 = position(out, "  #2 盯住一场演出");
    let p2 = position(out, "P2 不重要，紧急");
    let first_p2 = position(out, "  #46 ");
    let p3 = position(out, "P3 不重要，不紧急");
    assert!(
        untriaged < first_untriaged
            && first_untriaged < p1
            && p1 < first_p1
            && first_p1 < p2
            && p2 < first_p2
            && first_p2 < p3,
        "没定级的在最前面，然后 P1、P2、P3：\n{out}"
    );
    for want in [
        "等裁决：#25（P3）、#63（P0）、#65（P0）、#67（P0）",
        "被依赖挡着：#54（P1，等 #4）",
        "#68（P1，Claude）",
        "待验收：#11（P1）、#37（P3）",
        "下一个：#2（P1）",
    ] {
        assert!(out.contains(want), "少了 {want:?}：\n{out}");
    }
    let ready = &out[..position(out, "不在队列里")];
    for held in ["#54 ", "#63 ", "#68 ", "#11 "] {
        assert!(
            !ready.contains(&format!("  {held}")),
            "{held}不该出现在可以开工的列表里：\n{out}"
        );
    }
    assert!(
        position(out, "  #13 ") < p2 && out.contains("验收没通过，是返工"),
        "验收没通过的回到队列里，并说明是返工：\n{out}"
    );
}

#[test]
fn the_queue_for_an_agent_leaves_out_what_is_reserved_for_another() {
    let run = claims(
        &priority_fixtures(),
        PRIORITY_NOW,
        &["queue", "--agent", "Codex", "--json"],
    );
    assert_eq!(run.code, 0, "{}", run.out);
    let q: serde_json::Value = serde_json::from_str(run.out.trim()).expect(&run.out);
    assert_eq!(q["next"], 2, "{q:#}");
    let ready: Vec<u64> = q["ready"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["issue"].as_u64().unwrap())
        .collect();
    assert!(!ready.contains(&53) && !ready.contains(&70), "{ready:?}");
    let waiting = |n: u64| {
        q["waiting"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["issue"] == n)
            .unwrap_or_else(|| panic!("#{n} 不在 waiting 里：{q:#}"))
            .clone()
    };
    assert_eq!(waiting(53)["why"], "指派给别人");
    assert_eq!(waiting(53)["assigned_to"], "claude");
    assert_eq!(waiting(54)["blocked_by"], serde_json::json!([4]));
    assert_eq!(waiting(63)["priority"], "P0");
    assert_eq!(waiting(63)["why"], "等裁决");

    let anybody = claims(&priority_fixtures(), PRIORITY_NOW, &["queue", "--json"]);
    let q: serde_json::Value = serde_json::from_str(anybody.out.trim()).expect(&anybody.out);
    let first = &q["ready"][0];
    assert_eq!(first["issue"], 70, "不说是谁时，指派的 issue 也排队：{q:#}");
    assert_eq!(first["priority"], serde_json::Value::Null);
    assert_eq!(first["level"], "没定");
    assert_eq!(first["assigned_to"], "claude");
}

#[test]
fn claiming_below_the_queue_is_refused_until_the_reason_is_written_down() {
    let p2 = &[
        "claim",
        "--issue",
        "49",
        "--agent",
        "Codex",
        "--modules",
        "frontend/src/server.test.ts,frontend/src/app.test.ts",
    ];
    let run = claims(&priority_fixtures(), PRIORITY_NOW, p2);
    assert_eq!(run.code, 1, "{}", run.out);
    assert!(
        run.out.contains("#49（P2）不是现在该做的")
            && run.out.contains("#2（P1）")
            && run.out.contains("--out-of-order"),
        "拒绝要说清排在前面的是谁、怎么继续：\n{}",
        run.out
    );

    let mut with_reason = p2.to_vec();
    with_reason.extend(["--out-of-order", "前面的都要访问真实大麦，我在云端"]);
    let run = claims(&priority_fixtures(), PRIORITY_NOW, &with_reason);
    assert_eq!(run.code, 0, "{}", run.out);
    assert!(
        run.out
            .contains("- 不按顺序: 队列里还有比 P2 高、现在就能做的 #2（P1）")
            && run.out.contains("；前面的都要访问真实大麦，我在云端"),
        "放行时要把跳过了谁、为什么写进认领评论：\n{}",
        run.out
    );
}

#[test]
fn claiming_what_waits_for_the_owner_or_for_another_issue_is_refused() {
    let claim = |issue: &str, extra: &[&str]| {
        let mut args = vec![
            "claim",
            "--issue",
            issue,
            "--agent",
            "Codex",
            "--modules",
            "docs/somewhere",
        ];
        args.extend(extra);
        claims(&priority_fixtures(), PRIORITY_NOW, &args)
    };
    let owner = claim("25", &[]);
    assert_eq!(owner.code, 1, "{}", owner.out);
    assert!(owner.out.contains("等裁决"), "{}", owner.out);
    let owner = claim("25", &["--out-of-order", "我觉得可以"]);
    assert_eq!(owner.code, 1, "等裁决的不能靠理由绕过：{}", owner.out);

    let blocked = claim("54", &[]);
    assert_eq!(blocked.code, 1, "{}", blocked.out);
    assert!(
        blocked.out.contains("它依赖的 #4 还没关"),
        "{}",
        blocked.out
    );
    let blocked = claim(
        "54",
        &["--out-of-order", "#4 已合进 main（fd68bd7），只差验收"],
    );
    assert_eq!(blocked.code, 0, "{}", blocked.out);
    assert!(
        blocked
            .out
            .contains("- 不按顺序: 它依赖的 #4 还没关；#4 已合进 main"),
        "{}",
        blocked.out
    );

    // #37 carries P3 and 待验收 in open-issues.json. Even when it is the
    // only issue in the snapshot (or when --out-of-order is passed),
    // claiming an issue that is already awaiting acceptance is refused.
    let dir = temp("awaiting-acceptance");
    for entry in std::fs::read_dir(priority_fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    std::fs::write(dir.join("issues.json"), "[]\n").unwrap();
    let open: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("open-issues.json")).unwrap())
            .unwrap();
    let thirty_seven: Vec<serde_json::Value> =
        open.into_iter().filter(|i| i["number"] == 37).collect();
    std::fs::write(
        dir.join("open-issues.json"),
        serde_json::to_vec_pretty(&thirty_seven).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("issue-37.json"),
        serde_json::to_vec_pretty(&thirty_seven[0]).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("comments-37.json"), "[]\n").unwrap();
    let acceptance = claims(
        &dir,
        PRIORITY_NOW,
        &[
            "claim",
            "--issue",
            "37",
            "--agent",
            "Codex",
            "--modules",
            "docs/somewhere",
        ],
    );
    assert_eq!(acceptance.code, 1, "{}", acceptance.out);
    assert!(acceptance.out.contains("待验收"), "{}", acceptance.out);
    let acceptance = claims(
        &dir,
        PRIORITY_NOW,
        &[
            "claim",
            "--issue",
            "37",
            "--agent",
            "Codex",
            "--modules",
            "docs/somewhere",
            "--out-of-order",
            "我想再做一遍",
        ],
    );
    assert_eq!(
        acceptance.code, 1,
        "待验收的不能靠 --out-of-order 绕过：{}",
        acceptance.out
    );
}

#[test]
fn a_dependency_inside_a_four_backtick_fenced_example_does_not_block_an_issue() {
    let dir = temp("fenced-dep");
    for entry in std::fs::read_dir(priority_fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    std::fs::write(dir.join("issues.json"), "[]\n").unwrap();
    let open: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("open-issues.json")).unwrap())
            .unwrap();
    let subset: Vec<serde_json::Value> = open
        .into_iter()
        .filter(|i| i["number"] == 4 || i["number"] == 54)
        .map(|mut i| {
            if i["number"] == 54 {
                i["body"] = serde_json::json!("````markdown\n```\n依赖：#4\n```\n````\n");
            }
            i
        })
        .collect();
    std::fs::write(
        dir.join("open-issues.json"),
        serde_json::to_vec_pretty(&subset).unwrap(),
    )
    .unwrap();
    let q = claims(&dir, PRIORITY_NOW, &["queue", "--json"]);
    assert_eq!(q.code, 0, "{}", q.out);
    let parsed: serde_json::Value = serde_json::from_str(q.out.trim()).expect(&q.out);
    let ready: Vec<u64> = parsed["ready"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["issue"].as_u64().unwrap())
        .collect();
    assert!(
        ready.contains(&54),
        "四反引号代码块里的三反引号示例不能把 #54 挡进 waiting：{parsed:#}"
    );
    let claim = claims(
        &dir,
        PRIORITY_NOW,
        &[
            "claim",
            "--issue",
            "54",
            "--agent",
            "Codex",
            "--modules",
            "docs/somewhere",
        ],
    );
    assert_eq!(claim.code, 0, "{}", claim.out);
}

#[test]
fn claiming_an_untriaged_issue_asks_for_its_priority_first() {
    let untriaged = &[
        "claim",
        "--issue",
        "70",
        "--agent",
        "Claude",
        "--modules",
        "backend/src/inspect",
    ];
    let run = claims(&priority_fixtures(), PRIORITY_NOW, untriaged);
    assert_eq!(run.code, 1, "{}", run.out);
    assert!(
        run.out.contains("#70 还没定优先级") && run.out.contains("scripts/claims triage"),
        "{}",
        run.out
    );
    let mut with_reason = untriaged.to_vec();
    with_reason.extend(["--out-of-order", "理由救不了没定级的"]);
    let run = claims(&priority_fixtures(), PRIORITY_NOW, &with_reason);
    assert_eq!(run.code, 1, "没定级不是顺序问题，理由绕不过去：{}", run.out);
}

#[test]
fn triage_writes_both_axes_and_what_it_replaced() {
    let triage = |issue: &str, priority: &str| {
        claims(
            &priority_fixtures(),
            PRIORITY_NOW,
            &[
                "triage",
                "--issue",
                issue,
                "--agent",
                "Codex",
                "--priority",
                priority,
                "--important",
                "测试守不住它名字里的保证",
                "--urgent",
                "没有人被它挡住",
            ],
        )
    };
    let raise = triage("49", "p1");
    assert_eq!(raise.code, 0, "{}", raise.out);
    for want in [
        "## 优先级",
        "- 优先级: P1（重要，不紧急）",
        "- 原来: P2",
        "- 重要: 是。测试守不住它名字里的保证",
        "- 紧急: 否。没有人被它挡住",
        "要改的标签：加上 P1，去掉 P2",
        "没有真的发评论、没有改标签",
    ] {
        assert!(raise.out.contains(want), "少了 {want:?}：\n{}", raise.out);
    }
    let first = triage("70", "P1");
    assert!(first.out.contains("- 原来: 没定"), "{}", first.out);
    assert!(first.out.contains("要改的标签：加上 P1\n"), "{}", first.out);

    let same = triage("2", "P1");
    assert_eq!(same.code, 0, "{}", same.out);
    assert!(
        same.out.contains("#2 已经是 P1") && !same.out.contains("## 优先级"),
        "已经是这一档就什么都不发：\n{}",
        same.out
    );
    let gone = triage("31", "P1");
    assert_eq!(gone.code, 1, "{}", gone.out);
    assert!(gone.out.contains("不在打开的 issue 里"), "{}", gone.out);
}

#[test]
fn triage_posts_its_comment_before_it_swaps_the_labels_in_one_edit() {
    let dir = temp("triage");
    for entry in std::fs::read_dir(priority_fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    let run_ok = run(claims_with_fake_gh(
        &dir,
        &[
            "triage",
            "--issue",
            "49",
            "--agent",
            "Codex",
            "--priority",
            "P1",
            "--important",
            "测试守不住它名字里的保证",
            "--urgent",
            "没有人被它挡住",
            "--now",
            PRIORITY_NOW,
        ],
    ));
    assert_eq!(run_ok.code, 0, "{}", run_ok.out);
    let log = std::fs::read_to_string(dir.join("log")).unwrap();
    assert!(
        position(&log, "comment-49") < position(&log, "label-49"),
        "定级评论必须在改标签之前发出：\n{log}"
    );
    let labels = std::fs::read_to_string(dir.join("labels")).unwrap();
    assert_eq!(
        labels.lines().count(),
        1,
        "加和去掉在同一次编辑里：{labels}"
    );
    assert!(
        labels.contains("--add-label P1") && labels.contains("--remove-label P2"),
        "{labels}"
    );
    let posted: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("comments-49.json")).unwrap())
            .unwrap();
    let last = posted.as_array().unwrap().last().unwrap()["body"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(last.starts_with("## 优先级"), "{last}");

    let fail_dir = temp("triage-comment-fails");
    for entry in std::fs::read_dir(priority_fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), fail_dir.join(entry.file_name())).unwrap();
    }
    let mut cmd = claims_with_fake_gh(
        &fail_dir,
        &[
            "triage",
            "--issue",
            "49",
            "--agent",
            "Codex",
            "--priority",
            "P1",
            "--important",
            "测试守不住它名字里的保证",
            "--urgent",
            "没有人被它挡住",
            "--now",
            PRIORITY_NOW,
        ],
    );
    cmd.env("CLAIMS_FAKE_GH_FAIL_COMMENT", "1");
    let failed = run(cmd);
    assert_eq!(failed.code, 2, "{}", failed.out);
    assert!(
        !fail_dir.join("labels").exists(),
        "发评论失败时不能先去改标签"
    );
}

#[test]
fn triage_removes_lowercase_or_multiple_existing_priority_labels() {
    let dir = temp("triage-lowercase");
    for entry in std::fs::read_dir(priority_fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    let set_labels = |dir: &Path, names: &[&str]| {
        let labels: Vec<serde_json::Value> = names
            .iter()
            .map(|n| serde_json::json!({"name": n}))
            .collect();
        let mut issue: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("issue-49.json")).unwrap())
                .unwrap();
        issue["labels"] = serde_json::Value::Array(labels.clone());
        std::fs::write(
            dir.join("issue-49.json"),
            serde_json::to_vec_pretty(&issue).unwrap(),
        )
        .unwrap();
        let mut open: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(dir.join("open-issues.json")).unwrap())
                .unwrap();
        for item in &mut open {
            if item["number"] == 49 {
                item["labels"] = serde_json::Value::Array(labels.clone());
            }
        }
        std::fs::write(
            dir.join("open-issues.json"),
            serde_json::to_vec_pretty(&open).unwrap(),
        )
        .unwrap();
    };
    set_labels(&dir, &["bug", "p2"]);

    let preview = claims(
        &dir,
        PRIORITY_NOW,
        &[
            "triage",
            "--issue",
            "49",
            "--agent",
            "Codex",
            "--priority",
            "P1",
            "--important",
            "测试可信度",
            "--urgent",
            "没有截止点",
        ],
    );
    assert_eq!(preview.code, 0, "{}", preview.out);
    assert!(
        preview.out.contains("要改的标签：加上 P1，去掉 p2"),
        "小写 p2 也要被列进删除列表：\n{}",
        preview.out
    );

    let write_run = run(claims_with_fake_gh(
        &dir,
        &[
            "triage",
            "--issue",
            "49",
            "--agent",
            "Codex",
            "--priority",
            "P1",
            "--important",
            "测试可信度",
            "--urgent",
            "没有截止点",
            "--now",
            PRIORITY_NOW,
        ],
    ));
    assert_eq!(write_run.code, 0, "{}", write_run.out);
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("issue-49.json")).unwrap()).unwrap();
    let names: Vec<&str> = after["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["bug", "P1"], "旧的小写 p2 必须被删掉");
}

#[test]
fn two_agents_triaging_at_the_same_moment_leave_one_priority_label() {
    let dir = temp("triage-race");
    for entry in std::fs::read_dir(priority_fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    let labels = serde_json::json!([{"name": "bug"}, {"name": "P1"}]);
    let mut issue: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("issue-49.json")).unwrap()).unwrap();
    issue["labels"] = labels.clone();
    std::fs::write(
        dir.join("issue-49.json"),
        serde_json::to_vec_pretty(&issue).unwrap(),
    )
    .unwrap();
    let mut open: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("open-issues.json")).unwrap())
            .unwrap();
    for item in &mut open {
        if item["number"] == 49 {
            item["labels"] = labels.clone();
        }
    }
    std::fs::write(
        dir.join("open-issues.json"),
        serde_json::to_vec_pretty(&open).unwrap(),
    )
    .unwrap();

    let both: Vec<Run> = ["P0", "P2"]
        .iter()
        .map(|prio| {
            let mut cmd = claims_with_fake_gh(
                &dir,
                &[
                    "triage",
                    "--issue",
                    "49",
                    "--agent",
                    "Codex",
                    "--priority",
                    prio,
                    "--important",
                    "审查探针",
                    "--urgent",
                    "审查探针",
                    "--now",
                    PRIORITY_NOW,
                ],
            );
            cmd.env("CLAIMS_FAKE_GH_COMMENT_BARRIER", "49");
            std::thread::spawn(move || run(cmd))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();

    let winners: Vec<&Run> = both.iter().filter(|r| r.code == 0).collect();
    let losers: Vec<&Run> = both.iter().filter(|r| r.code == 1).collect();
    assert_eq!(
        winners.len(),
        1,
        "并发定级同一个 issue，只能有一个成功：\n{}\n----\n{}",
        both[0].out,
        both[1].out
    );
    assert_eq!(losers.len(), 1);
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("issue-49.json")).unwrap()).unwrap();
    let names: Vec<String> = after["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        matches!(
            theflash::claim::priority::level(&names),
            theflash::claim::priority::Level::Set(_)
        ),
        "并发定级结束后，issue 上必须只有一个优先级标签，不能留下两个：{names:?}"
    );

    // Partial label edit (gh issue edit applies --add-label but drops
    // --remove-label on the first call) is caught and cleaned up on
    // read-back before triage reports success.
    let partial_dir = temp("triage-partial");
    for entry in std::fs::read_dir(priority_fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), partial_dir.join(entry.file_name())).unwrap();
    }
    let mut cmd = claims_with_fake_gh(
        &partial_dir,
        &[
            "triage",
            "--issue",
            "49",
            "--agent",
            "Codex",
            "--priority",
            "P1",
            "--important",
            "审查探针",
            "--urgent",
            "审查探针",
            "--now",
            PRIORITY_NOW,
        ],
    );
    cmd.env("CLAIMS_FAKE_GH_PARTIAL_EDIT", "1");
    let repaired = run(cmd);
    assert_eq!(repaired.code, 0, "{}", repaired.out);
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(partial_dir.join("issue-49.json")).unwrap())
            .unwrap();
    let names: Vec<&str> = after["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["bug", "P1"],
        "第一次编辑漏删旧标签时，写后核对要把残留的 P2 清掉"
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
        vec!["triage", "--issue", "49", "--agent", "Codex"],
        vec![
            "triage",
            "--issue",
            "49",
            "--agent",
            "Codex",
            "--priority",
            "P5",
            "--important",
            "x",
            "--urgent",
            "y",
        ],
        vec![
            "triage",
            "--issue",
            "49",
            "--agent",
            "Codex",
            "--priority",
            "P1",
            "--important",
            "x",
        ],
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
