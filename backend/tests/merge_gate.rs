//! 合并闸门拒绝合并的每种情况，用回放的 GitHub 响应跑一遍（#15）。
//!
//! The fixtures in testdata/github are the real responses GitHub gave for
//! pull request #19 and the commits around it (testdata/github/README.md
//! says how they were taken). Each test copies them, changes the one
//! thing its case is about, and runs the real merge-gate binary against
//! the copy. `support::merge_gate` takes `gh` off PATH, so no test can
//! reach the real GitHub: a test that forgot `--replay` fails saying so.

mod support;

use std::path::PathBuf;
use std::process::Output;

use serde_json::{Value, json};

/// The reviewed commit of pull request #19, and the main the review ran on.
const HEAD: &str = "0dbf6fcc0f0b9df91ec77ddf385112b3a50c0a0c";
const REVIEWED_MAIN: &str = "293d2372fa018243ca53962a7e6fdfa9154469bd";
/// A commit that is on main, and never was a commit of pull request #19.
const OTHER_COMMIT: &str = "e918b373ac07f561512faed1dd30f0a1929393ad";

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/github")
}

/// One pull request as the gate sees it: the saved responses, copied into
/// a temporary directory so a test can change one of them.
struct Case {
    dir: PathBuf,
}

impl Case {
    /// A reviewed pull request with green CI on both platforms, whose
    /// review ran on the main it is still based on.
    fn ready(name: &str) -> Case {
        let dir = std::env::temp_dir().join(format!("merge-gate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for file in std::fs::read_dir(testdata().join("pr-ready")).unwrap() {
            let file = file.unwrap().path();
            std::fs::copy(&file, dir.join(file.file_name().unwrap())).unwrap();
        }
        Case { dir }
    }

    /// Replaces one saved response with the result of `edit`.
    fn edit(&self, name: &str, edit: impl FnOnce(&mut Value)) -> &Case {
        let path = self.dir.join(name);
        let mut value: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        edit(&mut value);
        std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        self
    }

    /// Rewrites the lines of the review record that `replace` answers for,
    /// keeping the rest of it as the reviewer wrote it.
    fn edit_record(&self, replace: impl Fn(&str) -> Option<String>) -> &Case {
        self.edit("comments.json", |v| {
            let body = v[1]["body"].as_str().unwrap();
            let edited: Vec<String> = body
                .lines()
                .map(|l| replace(l).unwrap_or_else(|| l.to_string()))
                .collect();
            v[1]["body"] = json!(edited.join("\n"));
        })
    }

    /// Drops the lines of the review record that `drop` matches.
    fn drop_record_line(&self, drop: impl Fn(&str) -> bool) -> &Case {
        self.edit("comments.json", |v| {
            let body = v[1]["body"].as_str().unwrap();
            let kept: Vec<&str> = body.lines().filter(|l| !drop(l)).collect();
            v[1]["body"] = json!(kept.join("\n"));
        })
    }

    /// Copies a saved response from another fixture over this case's.
    fn overlay(&self, fixture: &str, name: &str) -> &Case {
        std::fs::copy(testdata().join(fixture).join(name), self.dir.join(name)).unwrap();
        self
    }

    fn state(&self) -> PathBuf {
        self.dir.join("state")
    }

    /// `merge-gate check --replay <this case>`.
    fn check(&self) -> Output {
        support::merge_gate(&self.state())
            .args(["check", "--replay"])
            .arg(&self.dir)
            .output()
            .unwrap()
    }

    /// `merge-gate merge --dry-run --replay <this case>`: everything a
    /// merge does, including the lock, except the merge itself.
    fn dry_merge(&self) -> Output {
        support::merge_gate(&self.state())
            .args(["merge", "--pr", "19", "--dry-run", "--replay"])
            .arg(&self.dir)
            .output()
            .unwrap()
    }
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The gate refused, and said `why`.
fn refused(out: &Output, why: &str) {
    let said = said(out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(said.contains("拒绝合并"), "{said}");
    assert!(said.contains(why), "没说原因 `{why}`：{said}");
}

/// The gate would merge.
fn ready(out: &Output) {
    let said = said(out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("可以合并"), "{said}");
}

#[test]
fn a_reviewed_pr_with_green_ci_is_ready_to_merge() {
    let out = Case::ready("ready").check();
    ready(&out);
    let said = said(&out);
    assert!(said.contains("审查者 Claude"), "{said}");
    assert!(said.contains(&HEAD[..7]), "{said}");
}

#[test]
fn a_pr_with_no_review_record_is_refused() {
    // Only the comment the implementer wrote about its own checks is
    // left, which is not a review record.
    let case = Case::ready("no-record");
    case.edit("comments.json", |v| {
        v.as_array_mut().unwrap().truncate(1);
    });
    refused(&case.check(), "没有审查记录");
}

#[test]
fn a_review_record_for_a_commit_this_pr_never_had_is_refused() {
    let case = Case::ready("sha-mismatch");
    case.edit_record(|l| {
        l.starts_with("- 被审查的 SHA")
            .then(|| format!("- 被审查的 SHA: {OTHER_COMMIT}"))
    });
    refused(&case.check(), "对不上");
}

#[test]
fn a_push_after_the_review_is_refused() {
    // The reviewed commit is still in the pull request, with a newer
    // commit pushed on top of it.
    let case = Case::ready("pushed-after");
    let newer = "1111111111111111111111111111111111111111";
    case.edit("commits.json", |v| {
        let mut extra = v[0].clone();
        extra["sha"] = json!(newer);
        extra["commit"]["committer"]["date"] = json!("2026-10-13T18:20:00Z");
        extra["commit"]["message"] = json!("frontend: 按审查意见改了提示文字（#3）");
        v.as_array_mut().unwrap().push(extra);
    });
    case.edit("pull.json", |v| v["head"]["sha"] = json!(newer));
    case.edit("check-runs.json", |v| {
        for run in v["check_runs"].as_array_mut().unwrap() {
            run["head_sha"] = json!(newer);
        }
    });
    refused(&case.check(), "审查之后又有新推送");
}

#[test]
fn a_pr_whose_ci_is_red_is_refused() {
    let case = Case::ready("ci-red");
    case.edit("check-runs.json", |v| {
        v["check_runs"][1]["conclusion"] = json!("failure");
    });
    let out = case.check();
    refused(&out, "CI 是红的");
    assert!(
        said(&out).contains("check (ubuntu-latest)"),
        "{}",
        said(&out)
    );
}

#[test]
fn a_pr_whose_ci_has_not_finished_is_refused() {
    let case = Case::ready("ci-running");
    case.edit("check-runs.json", |v| {
        v["check_runs"][0]["status"] = json!("in_progress");
        v["check_runs"][0]["conclusion"] = Value::Null;
        v["check_runs"][0]["completed_at"] = Value::Null;
    });
    refused(&case.check(), "CI 还在跑");
}

#[test]
fn a_pr_with_ci_on_only_one_platform_is_refused() {
    // The project supports macOS and Linux, so one green job is not a
    // green run (docs/code-standards.md#ci-必须有的).
    let case = Case::ready("ci-one-platform");
    case.edit("check-runs.json", |v| {
        v["check_runs"]
            .as_array_mut()
            .unwrap()
            .retain(|r| r["name"].as_str().unwrap_or_default() != "check (macos-latest)");
    });
    let out = case.check();
    refused(&out, "CI 没跑完");
    assert!(
        said(&out).contains("check (macos-latest)"),
        "{}",
        said(&out)
    );
}

#[test]
fn a_review_by_the_implementer_is_refused() {
    let case = Case::ready("self-review");
    case.edit_record(|l| {
        l.starts_with("- 审查者")
            .then(|| "- 审查者: Codex".to_string())
    });
    refused(&case.check(), "同一个 agent");
}

#[test]
fn an_implementer_reviewing_under_another_spelling_is_still_the_implementer() {
    let case = Case::ready("self-review-spelling");
    case.edit_record(|l| {
        l.starts_with("- 审查者")
            .then(|| "- 审查者: codex（第 1 条线）".to_string())
    });
    refused(&case.check(), "同一个 agent");
}

#[test]
fn a_record_that_asks_for_changes_is_refused() {
    let case = Case::ready("changes-requested");
    case.edit_record(|l| {
        l.starts_with("- 结论")
            .then(|| "- 结论: 要修改".to_string())
    });
    refused(&case.check(), "要修改");
}

#[test]
fn a_record_without_the_main_it_was_reviewed_on_is_refused() {
    let case = Case::ready("no-base");
    case.drop_record_line(|l| l.starts_with("- 合到的 main"));
    refused(&case.check(), "缺少字段 `合到的 main`");
}

#[test]
fn a_blocking_label_stops_the_merge() {
    let case = Case::ready("label");
    case.edit("pull.json", |v| v["labels"] = json!([{"name": "要修改"}]));
    refused(&case.check(), "`要修改` 标签");
}

#[test]
fn a_pr_without_the_agent_trailer_is_refused() {
    let case = Case::ready("no-trailer");
    case.edit("pull.json", |v| {
        let body = v["body"].as_str().unwrap().replace("Agent: Codex", "");
        v["body"] = json!(body);
    });
    refused(&case.check(), "Agent: <名字>");
}

#[test]
fn a_conflicting_pr_is_refused() {
    let case = Case::ready("conflict");
    case.edit("pull.json", |v| v["mergeable"] = json!(false));
    refused(&case.check(), "冲突");
}

#[test]
fn a_pr_github_has_not_finished_checking_is_refused() {
    let case = Case::ready("mergeable-unknown");
    case.edit("pull.json", |v| v["mergeable"] = Value::Null);
    refused(&case.check(), "mergeable 是 null");
}

#[test]
fn a_merged_pr_is_not_merged_again() {
    let case = Case::ready("already-merged");
    case.edit("pull.json", |v| {
        v["state"] = json!("closed");
        v["merged"] = json!(true);
    });
    refused(&case.check(), "不是打开状态");
}

#[test]
fn main_moving_onto_the_same_files_needs_a_new_review() {
    // main went from the commit the review ran on to the merge of #19's
    // sibling, which touched the same frontend files this pull request
    // changes.
    let case = Case::ready("main-moved-overlap");
    case.overlay("main-moved", "main.json")
        .overlay("main-moved", "compare.json");
    let out = case.check();
    refused(&out, "重新审查");
    assert!(
        said(&out).contains("frontend/public/index.html"),
        "没说是哪些文件：{}",
        said(&out)
    );
}

#[test]
fn main_moving_somewhere_else_does_not_stop_the_merge() {
    // Same moved main, but what changed on it does not touch any file of
    // this pull request.
    let case = Case::ready("main-moved-elsewhere");
    case.overlay("main-moved", "main.json")
        .overlay("main-moved", "compare.json")
        .edit("compare.json", |v| {
            v["files"] = json!([{"filename": "backend/src/watch/clock.rs", "status": "modified"}]);
        });
    ready(&case.check());
}

#[test]
fn main_moving_with_no_record_of_what_changed_is_refused() {
    let case = Case::ready("main-moved-unknown");
    case.overlay("main-moved", "main.json");
    refused(&case.check(), "没拿到这段时间 main 改了哪些文件");
}

#[test]
fn a_record_posted_as_a_pr_review_counts_as_a_review_record() {
    // Reviewers post the record either as a comment or as a pull request
    // review; both are the record.
    let case = Case::ready("record-as-review");
    let mut body = String::new();
    case.edit("comments.json", |v| {
        body = v[1]["body"].as_str().unwrap().to_string();
        v.as_array_mut().unwrap().truncate(1);
    });
    case.edit("reviews.json", |v| {
        *v = json!([{
            "id": 5507594502_i64,
            "user": {"login": "zbtzbtzbt"},
            "state": "COMMENTED",
            "commit_id": HEAD,
            "submitted_at": "2026-10-13T18:08:12Z",
            "html_url": "https://github.com/zbtzbtzbt/theFlash/pull/19#pullrequestreview-5507594502",
            "body": body,
        }]);
    });
    ready(&case.check());
}

#[test]
fn the_newest_record_has_the_last_word() {
    // A reviewer who asked for changes and then passed the fix does not
    // leave the pull request blocked by its own earlier record.
    let case = Case::ready("two-records");
    case.edit("comments.json", |v| {
        let mut later = v[1].clone();
        let body = later["body"]
            .as_str()
            .unwrap()
            .replace("- 结论: 通过", "- 结论: 通过（第二次，上一条是要修改）");
        later["body"] = json!(body);
        later["created_at"] = json!("2026-10-13T18:30:00Z");
        later["html_url"] =
            json!("https://github.com/zbtzbtzbt/theFlash/pull/19#issuecomment-6152300000");
        v[1]["body"] = json!(
            v[1]["body"]
                .as_str()
                .unwrap()
                .replace("- 结论: 通过", "- 结论: 要修改")
        );
        v.as_array_mut().unwrap().push(later);
    });
    ready(&case.check());
}

#[test]
fn a_merge_waits_for_the_one_already_running() {
    // The lock is held, so this session does not merge at the same time.
    let case = Case::ready("lock-held");
    std::fs::create_dir_all(case.state()).unwrap();
    std::fs::create_dir_all(case.state().join("merge.lock")).unwrap();
    std::fs::write(case.state().join("merge.lock/holder"), "queue pid 1234\n").unwrap();
    let out = case.dry_merge();
    let said = said(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(said.contains("另一个合并正在跑"), "{said}");
    assert!(said.contains("queue pid 1234"), "{said}");
}

#[test]
fn a_merge_releases_the_lock_when_it_is_done() {
    let case = Case::ready("lock-released");
    let out = case.dry_merge();
    let said = said(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("没有合并"), "{said}");
    assert!(
        !case.state().join("merge.lock").exists(),
        "锁没有释放：{}",
        case.state().display()
    );
    // And the next session gets the lock.
    ready(&case.check());
    assert_eq!(case.dry_merge().status.code(), Some(0));
}

#[test]
fn a_refused_merge_releases_the_lock_too() {
    let case = Case::ready("lock-after-refusal");
    case.edit("check-runs.json", |v| {
        v["check_runs"][0]["conclusion"] = json!("failure");
    });
    refused(&case.dry_merge(), "CI 是红的");
    assert!(!case.state().join("merge.lock").exists());
}

#[test]
fn without_replayed_data_the_gate_cannot_reach_github() {
    // The guard behind every test here: with `gh` gone, a run that asks
    // GitHub anything fails and says so, instead of reading the real
    // repository.
    let case = Case::ready("no-gh");
    let out = support::merge_gate(&case.state())
        .args(["check", "--pr", "19"])
        .output()
        .unwrap();
    let said = said(&out);
    assert_eq!(out.status.code(), Some(2), "{said}");
    assert!(said.contains("问 GitHub 要"), "{said}");
}

#[test]
fn an_unreadable_replay_directory_is_an_error_not_a_merge() {
    let state = std::env::temp_dir().join("merge-gate-no-state");
    let out = support::merge_gate(&state)
        .args(["check", "--replay", "/nonexistent/merge-gate-case"])
        .output()
        .unwrap();
    let said = said(&out);
    assert_eq!(out.status.code(), Some(2), "{said}");
    assert!(said.contains("pull.json"), "{said}");
}

#[test]
fn an_unknown_command_prints_the_usage() {
    let out = support::merge_gate(&std::env::temp_dir().join("merge-gate-usage"))
        .args(["merge-everything"])
        .output()
        .unwrap();
    let said = said(&out);
    assert_eq!(out.status.code(), Some(2), "{said}");
    assert!(said.contains("用法"), "{said}");
    assert!(said.contains("merge-gate.md"), "{said}");
}

#[test]
fn the_base_fixture_is_the_pull_request_it_claims_to_be() {
    // If the saved responses drift apart, the refusals above would be
    // testing something else.
    let case = Case::ready("fixture-shape");
    let pull: Value =
        serde_json::from_str(&std::fs::read_to_string(case.dir.join("pull.json")).unwrap())
            .unwrap();
    assert_eq!(pull["head"]["sha"], HEAD);
    assert_eq!(pull["base"]["sha"], REVIEWED_MAIN);
    let main: Value =
        serde_json::from_str(&std::fs::read_to_string(case.dir.join("main.json")).unwrap())
            .unwrap();
    assert_eq!(main["sha"], REVIEWED_MAIN);
    let comments: Value =
        serde_json::from_str(&std::fs::read_to_string(case.dir.join("comments.json")).unwrap())
            .unwrap();
    let record = comments[1]["body"].as_str().unwrap();
    assert!(record.contains(HEAD), "{record}");
    assert!(record.contains(REVIEWED_MAIN), "{record}");
}
