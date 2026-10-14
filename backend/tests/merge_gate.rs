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
use theflash::merge_gate::lock;

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

    /// Rewrites a saved list answer the way `gh api --paginate` prints a
    /// list that did not fit on one page: one independent JSON array per
    /// page, back to back, not one array (`gh api --help`).
    fn in_pages(&self, name: &str) -> &Case {
        let path = self.dir.join(name);
        let list: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let (first, rest) = list.split_at(1.min(list.len()));
        std::fs::write(&path, two_pages(json!(first), json!(rest))).unwrap();
        self
    }

    /// The same for an answer whose pages are objects with the list
    /// inside, which is the shape of `commits/<sha>/check-runs`.
    fn in_pages_under(&self, name: &str, key: &str) -> &Case {
        let path = self.dir.join(name);
        let mut value: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let list = value[key].as_array().unwrap().clone();
        let (first, rest) = list.split_at(1.min(list.len()));
        value[key] = json!(rest);
        let mut page = value.clone();
        page[key] = json!(first);
        std::fs::write(&path, two_pages(page, value)).unwrap();
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

fn two_pages(first: Value, second: Value) -> String {
    format!(
        "{}\n{}\n",
        serde_json::to_string_pretty(&first).unwrap(),
        serde_json::to_string_pretty(&second).unwrap()
    )
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
    refused(&out, "CI 不是绿的");
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
fn a_required_check_that_was_skipped_is_refused() {
    // A required job GitHub reports as `skipped` did not run, so the
    // platform it stands for was never covered. `completed` with a
    // conclusion that is not `success` is not a green run.
    let case = Case::ready("ci-skipped");
    case.edit("check-runs.json", |v| {
        v["check_runs"][0]["conclusion"] = json!("skipped");
    });
    let out = case.check();
    refused(&out, "CI 不是绿的");
    assert!(
        said(&out).contains("check (macos-latest)"),
        "没说是哪个 job：{}",
        said(&out)
    );
}

#[test]
fn a_required_check_with_a_neutral_conclusion_is_refused() {
    let case = Case::ready("ci-neutral");
    case.edit("check-runs.json", |v| {
        v["check_runs"][1]["conclusion"] = json!("neutral");
    });
    let out = case.check();
    refused(&out, "CI 不是绿的");
    assert!(
        said(&out).contains("check (ubuntu-latest)"),
        "没说是哪个 job：{}",
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
fn a_verdict_that_says_it_did_not_pass_is_refused() {
    // 未通过 contains 通过, so a verdict read by looking for 通过 inside
    // the text reads a refusal as a pass.
    let case = Case::ready("verdict-unpassed");
    case.edit_record(|l| {
        l.starts_with("- 结论")
            .then(|| "- 结论: 未通过".to_string())
    });
    let out = case.check();
    refused(&out, "不是“通过”");
    assert!(said(&out).contains("未通过"), "{}", said(&out));
}

#[test]
fn a_verdict_that_only_promises_to_pass_after_changes_is_refused() {
    let case = Case::ready("verdict-conditional");
    case.edit_record(|l| {
        l.starts_with("- 结论")
            .then(|| "- 结论: 要修改，通过后再合并".to_string())
    });
    refused(&case.check(), "不是“通过”");
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
fn main_moving_after_the_review_needs_a_new_review() {
    // main went from the commit the review ran on to the merge of #19's
    // sibling. #15 asks for the change to be verified on the latest main,
    // and a review that ran on an older main did not do that — whichever
    // files the two sides touched. The gate used to compare the file
    // names and let a non-overlapping move through, which is how a
    // commit nobody checked against the current main could land (审查记录
    // b1b42f0 的第 4 条).
    let case = Case::ready("main-moved");
    case.overlay("main-moved", "main.json");
    let out = case.check();
    refused(&out, "没有在最新的 main 上验证过");
    assert!(said(&out).contains("57979b4"), "{}", said(&out));
    assert!(said(&out).contains("重新跑一遍检查"), "{}", said(&out));
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
            .replace("- 结论: 通过", "- 结论: 通过（复查）");
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
fn a_change_request_from_another_reviewer_on_the_same_head_stops_the_merge() {
    // PR #127 (#160): GPT and Codex reviewed the same head in parallel; one
    // wrote `结论: 通过` and the other wrote `结论: 要修改`. Neither arrival
    // order nor an implementer reply on the same head may let the merge
    // through; only a new `通过` from the reviewer who asked for changes or
    // a new reviewed head clears it.
    let gpt_url = "https://github.com/zbtzbtzbt/theFlash/pull/127#issuecomment-6162457684";
    let codex_url = "https://github.com/zbtzbtzbt/theFlash/pull/127#issuecomment-6162458297";

    // Implementer is Claude (so both GPT and Codex are independent reviewers).
    let setup_pr127 = |name: &str| {
        let case = Case::ready(name);
        case.edit("pull.json", |v| {
            let body = v["body"]
                .as_str()
                .unwrap()
                .replace("Agent: Codex", "Agent: Claude");
            v["body"] = json!(body);
        });
        case
    };

    // Case 1 (exact PR #127 order): GPT posts `通过` at 06:36:07Z, Codex posts `要修改` at 06:36:11Z.
    let case1 = setup_pr127("parallel-pass-then-changes");
    case1.edit("comments.json", |v| {
        let template = v[1].clone();
        let mut gpt = template.clone();
        gpt["created_at"] = json!("2026-10-14T06:36:07Z");
        gpt["html_url"] = json!(gpt_url);
        gpt["body"] = json!(
            template["body"]
                .as_str()
                .unwrap()
                .replace("- 审查者: Claude", "- 审查者: GPT")
        );
        let mut codex = template;
        codex["created_at"] = json!("2026-10-14T06:36:11Z");
        codex["html_url"] = json!(codex_url);
        codex["body"] = json!(
            codex["body"]
                .as_str()
                .unwrap()
                .replace("- 审查者: Claude", "- 审查者: Codex")
                .replace("- 结论: 通过", "- 结论: 要修改")
        );
        let arr = v.as_array_mut().unwrap();
        arr.truncate(1);
        arr.push(gpt);
        arr.push(codex);
    });
    let out1 = case1.check();
    refused(&out1, "要修改");
    assert!(
        said(&out1).contains("Codex"),
        "应点名审查者 Codex：{}",
        said(&out1)
    );
    assert!(
        said(&out1).contains(codex_url),
        "应给出要修改记录链接：{}",
        said(&out1)
    );

    // Case 2 (reverse order): Codex posts `要修改` at 06:36:07Z, then GPT posts `通过` at 06:36:11Z.
    let case2 = setup_pr127("parallel-changes-then-pass");
    case2.edit("comments.json", |v| {
        let template = v[1].clone();
        let mut codex = template.clone();
        codex["created_at"] = json!("2026-10-14T06:36:07Z");
        codex["html_url"] = json!(codex_url);
        codex["body"] = json!(
            template["body"]
                .as_str()
                .unwrap()
                .replace("- 审查者: Claude", "- 审查者: Codex")
                .replace("- 结论: 通过", "- 结论: 要修改")
        );
        let mut gpt = template;
        gpt["created_at"] = json!("2026-10-14T06:36:11Z");
        gpt["html_url"] = json!(gpt_url);
        gpt["body"] = json!(
            gpt["body"]
                .as_str()
                .unwrap()
                .replace("- 审查者: Claude", "- 审查者: GPT")
        );
        let arr = v.as_array_mut().unwrap();
        arr.truncate(1);
        arr.push(codex);
        arr.push(gpt);
    });
    let out2 = case2.check();
    refused(&out2, "要修改");
    assert!(
        said(&out2).contains("Codex"),
        "应点名审查者 Codex：{}",
        said(&out2)
    );
    assert!(
        said(&out2).contains(codex_url),
        "应给出要修改记录链接：{}",
        said(&out2)
    );

    // Case 3 (implementer comment after Codex's `要修改`): Claude replies on the PR (both prose
    // and a self-review block) after Codex's `要修改` while GPT has `通过` -> still refused.
    case1.edit("comments.json", |v| {
        let mut reply = v[1].clone();
        reply["created_at"] = json!("2026-10-14T06:40:00Z");
        reply["html_url"] =
            json!("https://github.com/zbtzbtzbt/theFlash/pull/127#issuecomment-6162600000");
        reply["body"] = json!("已回复 Codex 的审查意见。\n\nAgent: Claude\n");
        let mut self_rec = v[1].clone();
        self_rec["created_at"] = json!("2026-10-14T06:41:00Z");
        self_rec["html_url"] =
            json!("https://github.com/zbtzbtzbt/theFlash/pull/127#issuecomment-6162600001");
        self_rec["body"] = json!(
            self_rec["body"]
                .as_str()
                .unwrap()
                .replace("- 审查者: GPT", "- 审查者: Claude")
        );
        let arr = v.as_array_mut().unwrap();
        arr.push(reply);
        arr.push(self_rec);
    });
    let out3 = case1.check();
    refused(&out3, "要修改");
    assert!(said(&out3).contains("Codex"), "{}", said(&out3));
    assert!(said(&out3).contains(codex_url), "{}", said(&out3));

    // Case 4 (cleared by same reviewer): Codex posts a newer `通过` on the same head -> ready.
    case2.edit("comments.json", |v| {
        let mut codex_pass = v[1].clone();
        codex_pass["created_at"] = json!("2026-10-14T06:50:00Z");
        codex_pass["html_url"] =
            json!("https://github.com/zbtzbtzbt/theFlash/pull/127#issuecomment-6162900000");
        codex_pass["body"] = json!(
            codex_pass["body"]
                .as_str()
                .unwrap()
                .replace("- 结论: 要修改", "- 结论: 通过")
        );
        v.as_array_mut().unwrap().push(codex_pass);
    });
    ready(&case2.check());

    // Case 5 (cleared by new head): Codex asked for changes on H1, implementer pushes H2,
    // and GPT passes H2 -> ready.
    let case5 = setup_pr127("parallel-cleared-by-new-head");
    let newer = "2222222222222222222222222222222222222222";
    case5.edit("commits.json", |v| {
        let mut extra = v[0].clone();
        extra["sha"] = json!(newer);
        extra["commit"]["committer"]["date"] = json!("2026-10-14T06:45:00Z");
        extra["commit"]["message"] = json!("inspect: 按 Codex 审查意见排除未读样本（#79）");
        v.as_array_mut().unwrap().push(extra);
    });
    case5.edit("pull.json", |v| v["head"]["sha"] = json!(newer));
    case5.edit("check-runs.json", |v| {
        for run in v["check_runs"].as_array_mut().unwrap() {
            run["head_sha"] = json!(newer);
        }
    });
    case5.edit("comments.json", |v| {
        let template = v[1].clone();
        let mut codex_old = template.clone();
        codex_old["created_at"] = json!("2026-10-14T06:36:11Z");
        codex_old["html_url"] = json!(codex_url);
        codex_old["body"] = json!(
            template["body"]
                .as_str()
                .unwrap()
                .replace("- 审查者: Claude", "- 审查者: Codex")
                .replace("- 结论: 通过", "- 结论: 要修改")
        );
        let mut gpt_new = template;
        gpt_new["created_at"] = json!("2026-10-14T06:55:00Z");
        gpt_new["html_url"] =
            json!("https://github.com/zbtzbtzbt/theFlash/pull/127#issuecomment-6162950000");
        gpt_new["body"] = json!(
            gpt_new["body"]
                .as_str()
                .unwrap()
                .replace(HEAD, newer)
                .replace("- 审查者: Claude", "- 审查者: GPT")
        );
        let arr = v.as_array_mut().unwrap();
        arr.truncate(1);
        arr.push(codex_old);
        arr.push(gpt_new);
    });
    ready(&case5.check());
}

#[test]
fn the_record_format_tonights_dispatched_reviewers_write_is_read() {
    // The review task #20 dispatches writes `被审查的提交：<sha>` with a
    // full-width colon. It says the same thing as `被审查的 SHA`, and a
    // gate that refuses it would reject every record written tonight.
    let case = Case::ready("dispatcher-format");
    case.edit_record(|l| {
        l.starts_with("- 被审查的 SHA")
            .then(|| format!("- 被审查的提交：{HEAD}"))
    });
    ready(&case.check());
}

#[test]
fn answers_that_came_back_in_several_pages_are_all_read() {
    // `gh api --paginate` prints one independent JSON value per page, so
    // a pull request with more comments, commits or check runs than fit
    // on one page arrives as several arrays back to back. Reading only
    // the first page would lose the review record; reading the whole
    // output as one array fails outright.
    let case = Case::ready("paged");
    case.in_pages("commits.json")
        .in_pages("comments.json")
        .in_pages_under("check-runs.json", "check_runs");
    ready(&case.check());
}

#[test]
fn a_merge_waits_for_the_one_already_running() {
    // A queue run in another session holds the lock, so this session does
    // not merge at the same time. The holder here is a real lock taken
    // through the same code, which is also what makes it a live holder:
    // this test process is the one holding it.
    let case = Case::ready("lock-held");
    let held = lock::Lock::acquire_in(&case.state(), "queue").expect("先拿到锁");
    let out = case.dry_merge();
    let said = said(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(said.contains("另一个合并正在跑"), "{said}");
    assert!(said.contains("queue"), "没说谁在跑：{said}");
    assert!(said.contains("进程还活着"), "{said}");
    drop(held);
}

#[test]
fn a_merge_releases_the_lock_when_it_is_done() {
    let case = Case::ready("lock-released");
    let out = case.dry_merge();
    let said = said(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("没有合并"), "{said}");
    // The stable kernel-lock inode stays in place; release means the
    // holder metadata is gone and the next session can acquire it.
    assert!(!case.state().join("merge.lock/holder").exists());
    ready(&case.check());
    assert_eq!(case.dry_merge().status.code(), Some(0));
}

#[test]
fn a_refused_merge_releases_the_lock_too() {
    let case = Case::ready("lock-after-refusal");
    case.edit("check-runs.json", |v| {
        v["check_runs"][0]["conclusion"] = json!("failure");
    });
    refused(&case.dry_merge(), "CI 不是绿的");
    assert!(!case.state().join("merge.lock/holder").exists());
    refused(&case.dry_merge(), "CI 不是绿的");
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

#[test]
fn merging_marks_linked_issues_for_acceptance_and_posts_the_review_record() {
    let case = Case::ready("handoff-issues");
    case.edit("pull.json", |v| {
        v["title"] = json!("damai: 访问层的 5 处缺陷一起修（#74，#70） (#19)");
    });
    let log = case.state().join("gh.log");
    let merged_sha = "ffffffffffffffffffffffffffffffffffffffff";
    support::stub(
        &case.state(),
        "gh",
        &format!(
            "printf '%s\\n' \"$*\" >> '{}'\n\
             if [ \"$1\" = \"api\" ]; then\n\
               printf '{{\"merge_commit_sha\":\"{merged_sha}\"}}\\n'\n\
             fi\n",
            log.display()
        ),
    );
    let out = support::merge_gate(&case.state())
        .env("PATH", case.state().join("bin"))
        .args(["merge", "--pr", "19", "--replay"])
        .arg(&case.dir)
        .output()
        .unwrap();
    let said = said(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.contains(
            "issue edit 74 --repo zbtzbtzbt/theFlash --add-label 待验收 --remove-label 验收未通过"
        ),
        "合并后没有给 #74 加 待验收 并移除 验收未通过：{calls}\n{said}"
    );
    assert!(
        calls.contains(
            "issue edit 70 --repo zbtzbtzbt/theFlash --add-label 待验收 --remove-label 验收未通过"
        ),
        "合并后没有给 #70 加 待验收 并移除 验收未通过：{calls}\n{said}"
    );
    assert!(
        !calls.contains("issue edit 19 "),
        "不该把 PR 自己的编号当成关联 issue：{calls}"
    );
    assert!(
        calls.contains("issue comment 74 --repo zbtzbtzbt/theFlash --body")
            && calls.contains(merged_sha)
            && calls.contains(HEAD)
            && calls.contains("issuecomment-6152100000"),
        "合并后没有在 #74 上评论合入提交、被审查提交和审查记录链接：{calls}"
    );
    assert!(
        calls.contains("issue comment 70 --repo zbtzbtzbt/theFlash --body"),
        "合并后没有在 #70 上评论：{calls}"
    );
    // Applying `--add-label 待验收 --remove-label 验收未通过` transitions a
    // failed-acceptance issue into a new `accept` round in `dispatch::plan`.
    // Leaving both labels would keep dispatching `fix-issue` (Codex #134).
    let mut agents = std::collections::BTreeMap::new();
    for v in ["claude", "codex"] {
        agents.insert(
            v.to_string(),
            theflash::dispatch::config::Agent {
                command: vec!["true".to_string()],
                prompt: theflash::dispatch::config::PromptStyle::Argv,
            },
        );
    }
    let cfg = theflash::dispatch::config::Config {
        agents,
        vendor_order: vec!["claude".to_string(), "codex".to_string()],
        max_sessions: 4,
        max_sessions_per_day: 10,
        cloud_roles: Vec::new(),
        ..Default::default()
    };
    let both = theflash::dispatch::snapshot::Snapshot {
        issues: vec![theflash::dispatch::snapshot::Issue {
            number: 74,
            title: "issue 74".to_string(),
            labels: vec![
                "bug".to_string(),
                "P1".to_string(),
                theflash::merge_gate::REJECTED_LABEL.to_string(),
                theflash::merge_gate::ACCEPT_LABEL.to_string(),
            ],
            agent: Some("claude".to_string()),
            last_comment_at: None,
            ..Default::default()
        }],
        pulls: Vec::new(),
        main_checks: theflash::dispatch::snapshot::MainChecks::Green,
        main_run: None,
    };
    let stuck = theflash::dispatch::plan(
        &both,
        &theflash::dispatch::state::State::default(),
        &cfg,
        theflash::dispatch::now(),
    );
    assert_eq!(stuck.start[0].role.name(), "fix-issue", "{stuck:?}");
    let mut labels = both.issues[0].labels.clone();
    labels.retain(|l| l != theflash::merge_gate::REJECTED_LABEL);
    let handed = theflash::dispatch::snapshot::Snapshot {
        issues: vec![theflash::dispatch::snapshot::Issue {
            labels,
            ..both.issues[0].clone()
        }],
        ..both
    };
    let planned = theflash::dispatch::plan(
        &handed,
        &theflash::dispatch::state::State::default(),
        &cfg,
        theflash::dispatch::now(),
    );
    assert_eq!(planned.start.len(), 1, "{planned:?}");
    assert_eq!(planned.start[0].role.name(), "accept", "{planned:?}");
    assert_eq!(planned.start[0].vendor, "codex", "{planned:?}");
}

#[test]
fn label_failure_after_merge_keeps_exit_zero_and_prints_the_manual_command() {
    let case = Case::ready("handoff-label-failure");
    let log = case.state().join("gh.log");
    support::stub(
        &case.state(),
        "gh",
        &format!(
            "printf '%s\\n' \"$*\" >> '{}'\n\
             if [ \"$1\" = \"issue\" ] && [ \"$2\" = \"edit\" ]; then\n\
               echo 'HTTP 502: Bad Gateway' >&2\n\
               exit 1\n\
             fi\n",
            log.display()
        ),
    );
    let out = support::merge_gate(&case.state())
        .env("PATH", case.state().join("bin"))
        .args(["merge", "--pr", "19", "--replay"])
        .arg(&case.dir)
        .output()
        .unwrap();
    let said = said(&out);
    assert_eq!(
        out.status.code(),
        Some(0),
        "加标签失败不应改变合并成功的退出码：{said}"
    );
    assert!(said.contains("已合并 PR #19"), "应报告 PR 已合并：{said}");
    assert!(
        said.contains(
            "gh issue edit 3 --repo zbtzbtzbt/theFlash --add-label 待验收 --remove-label 验收未通过"
        ),
        "加标签失败应打印补标签的命令：{said}"
    );
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("已加上 `待验收`") && calls.contains("自动加 `待验收` 标签失败"),
        "加标签失败时 issue 评论不应谎称已加上标签：{calls}"
    );
    assert!(
        said.contains("取 PR #19 的合入提交失败")
            && !calls.contains("合入/被审查的提交")
            && calls.contains("合入 main 的提交未取到"),
        "取不到 merge_commit_sha 时必须报错且不把被审查 head 宣称为合入提交：{said}\n{calls}"
    );

    // Also when GitHub returns `{"merge_commit_sha": null}`, stderr and the
    // issue comment must report that the merged commit was not obtained.
    let case_null = Case::ready("handoff-null-sha");
    let log_null = case_null.state().join("gh.log");
    support::stub(
        &case_null.state(),
        "gh",
        &format!(
            "printf '%s\\n' \"$*\" >> '{}'\n\
             if [ \"$1\" = \"api\" ]; then\n\
               printf '{{\"merge_commit_sha\":null}}\\n'\n\
             fi\n",
            log_null.display()
        ),
    );
    let out_null = support::merge_gate(&case_null.state())
        .env("PATH", case_null.state().join("bin"))
        .args(["merge", "--pr", "19", "--replay"])
        .arg(&case_null.dir)
        .output()
        .unwrap();
    let said_null = self::said(&out_null);
    let calls_null = std::fs::read_to_string(&log_null).unwrap_or_default();
    assert_eq!(out_null.status.code(), Some(0), "{said_null}");
    assert!(
        said_null.contains("取 PR #19 的合入提交失败")
            && !calls_null.contains("合入/被审查的提交")
            && calls_null.contains("合入 main 的提交未取到"),
        "merge_commit_sha 为 null 时不能把被审查 head 宣称为合入提交：{said_null}\n{calls_null}"
    );
}
