//! 审查记录怎么读、合并条件怎么判，单独测到。
//!
//! The replayed-GitHub side of the gate is tests/merge_gate.rs; these are
//! the pieces that file would only cover indirectly: the shapes agents
//! actually write a record in, and the order the refusals come out in.

use chrono::{DateTime, Utc};

use super::record::{self, Verdict};
use super::*;

fn at(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

const SHA: &str = "0dbf6fcc0f0b9df91ec77ddf385112b3a50c0a0c";
const MAIN: &str = "293d2372fa018243ca53962a7e6fdfa9154469bd";

fn record(body: &str) -> Option<ReviewRecord> {
    record::parse(
        body,
        "https://example.invalid/c/1",
        at("2026-10-13T18:08:12Z"),
    )
}

const CANONICAL: &str = "## 审查记录

- 被审查的 SHA: 0dbf6fcc0f0b9df91ec77ddf385112b3a50c0a0c
- 合到的 main: 293d2372fa018243ca53962a7e6fdfa9154469bd
- 审查者: Claude
- 结论: 通过
- 跑了什么: make check 全绿
";

#[test]
fn a_record_in_the_documented_format_reads_field_by_field() {
    let r = record(CANONICAL).expect("这是一条审查记录");
    assert_eq!(r.reviewed_sha, SHA);
    assert_eq!(r.reviewed_main, MAIN);
    assert_eq!(r.reviewer, "Claude");
    assert_eq!(r.verdict, Verdict::Passed);
    assert_eq!(r.ran, "make check 全绿");
    assert_eq!(r.missing_field(), None);
}

#[test]
fn a_comment_that_is_not_a_record_is_not_read_as_one() {
    // The comment a reviewer left on pull request #19 before the format
    // existed: it says the same things in prose, and the gate must not
    // take prose as a record.
    let prose = "合并前检查（Codex）：head 0dbf6fcc0f0b9df91ec77ddf385112b3a50c0a0c，\
                 基于293d237；本地临时HOME make check全部通过。";
    assert!(record(prose).is_none());
    assert!(record("审查记录格式见 docs/subsystems/merge-gate.md").is_none());
}

#[test]
fn a_record_is_read_however_the_agent_wrote_the_markup() {
    for body in [
        CANONICAL.to_string(),
        CANONICAL.replace("## ", "### "),
        CANONICAL.replace("## 审查记录", "**审查记录**"),
        CANONICAL.replace("## 审查记录", "审查记录："),
        CANONICAL.replace(": ", "：").replace("- ", "* "),
        CANONICAL.replace("- 被审查的 SHA", "- **被审查的 SHA**"),
        CANONICAL.replace(SHA, &format!("`{SHA}`")),
        CANONICAL.replace(SHA, &SHA.to_uppercase()),
        format!("审查完了，结论在下面。\n\n{CANONICAL}\n\nAgent: Claude\n"),
    ] {
        let r = record(&body).unwrap_or_else(|| panic!("读不出来：{body}"));
        assert_eq!(r.reviewed_sha, SHA, "{body}");
        assert_eq!(r.reviewer, "Claude", "{body}");
        assert_eq!(r.verdict, Verdict::Passed, "{body}");
    }
}

#[test]
fn a_field_is_found_under_the_other_names_agents_write_it_under() {
    // The review task #20 dispatches writes `被审查的提交：<sha>`, and
    // agents vary the space and the case of `SHA`. All of them name the
    // commit the review was about.
    for key in [
        "- 被审查的 SHA:",
        "- 被审查的SHA：",
        "- 被审查的 sha:",
        "- 被审查的提交：",
        "- 审查的提交:",
        "* **被审查的提交**：",
    ] {
        let body = CANONICAL.replace("- 被审查的 SHA:", key);
        let r = record(&body).unwrap_or_else(|| panic!("读不出来：{body}"));
        assert_eq!(r.reviewed_sha, SHA, "{key}");
        assert_eq!(r.missing_field(), None, "{key}");
    }
}

#[test]
fn a_record_missing_a_field_says_which_one() {
    let without = |line: &str| {
        let body: String = CANONICAL
            .lines()
            .filter(|l| !l.starts_with(line))
            .collect::<Vec<_>>()
            .join("\n");
        record(&body).expect("还是一条审查记录").missing_field()
    };
    assert_eq!(without("- 被审查的 SHA"), Some("被审查的 SHA"));
    assert_eq!(without("- 合到的 main"), Some("合到的 main"));
    assert_eq!(without("- 审查者"), Some("审查者"));
    assert_eq!(without("- 跑了什么"), Some("跑了什么"));
}

#[test]
fn a_short_sha_is_not_enough_to_merge_on() {
    // `--match-head-commit` takes the full sha, and a 7-character prefix
    // does not pin a commit.
    let body = CANONICAL.replace(SHA, &SHA[..7]);
    assert_eq!(record(&body).unwrap().missing_field(), Some("被审查的 SHA"));
}

#[test]
fn a_sha_a_reviewer_annotated_is_still_read() {
    // The second review of pull request #24 wrote the main it merged onto
    // with what happened to that merge after it, and the gate read the
    // whole thing as the sha, so it refused with `合到的 main` missing
    // instead of the verdict the reviewer actually wrote. The lines below
    // are that record's own bytes.
    let body = CANONICAL
        .replace(
            "- 被审查的 SHA: 0dbf6fcc0f0b9df91ec77ddf385112b3a50c0a0c",
            "- 被审查的提交：0dbf6fcc0f0b9df91ec77ddf385112b3a50c0a0c",
        )
        .replace(
            "- 合到的 main: 293d2372fa018243ca53962a7e6fdfa9154469bd",
            "- 合到的 main：293d2372fa018243ca53962a7e6fdfa9154469bd（尝试合并，\
             发生冲突，未产生合并结果）",
        );
    let r = record(&body).expect("这是一条审查记录");
    assert_eq!(r.reviewed_main, MAIN);
    assert_eq!(r.missing_field(), None);
    // A note cannot turn a prefix into a sha, and a field that names no
    // sha at all is still missing.
    for value in [
        format!("{}（尝试合并）", &MAIN[..7]),
        "还没合，等 CI".to_string(),
    ] {
        let body = CANONICAL.replace(MAIN, &value);
        assert_eq!(
            record(&body).unwrap().missing_field(),
            Some("合到的 main"),
            "{value}"
        );
    }
}

#[test]
fn only_an_explicit_pass_counts_as_passing() {
    let verdict = |v: &str| {
        record(&CANONICAL.replace("- 结论: 通过", &format!("- 结论: {v}")))
            .unwrap()
            .verdict
    };
    assert_eq!(verdict("通过"), Verdict::Passed);
    assert_eq!(verdict("通过，可以合并"), Verdict::Passed);
    assert_eq!(verdict("通过。"), Verdict::Passed);
    assert_eq!(verdict("要修改"), Verdict::NotPassed);
    assert_eq!(verdict("不通过"), Verdict::NotPassed);
    assert_eq!(verdict(""), Verdict::NotPassed);
    assert_eq!(verdict("待定"), Verdict::NotPassed);
    // A verdict that says it did not pass contains 通过 too, and one that
    // passes only after something else is not a pass yet. Anything the
    // gate is not sure of is a refusal: the reviewer writes `通过` again
    // and the merge happens, but a merge on a misread verdict cannot be
    // taken back.
    assert_eq!(verdict("未通过"), Verdict::NotPassed);
    assert_eq!(verdict("没通过"), Verdict::NotPassed);
    assert_eq!(verdict("要修改，通过后再合并"), Verdict::NotPassed);
    assert_eq!(verdict("通过后再合并"), Verdict::NotPassed);
    assert_eq!(verdict("有条件通过"), Verdict::NotPassed);
    assert_eq!(verdict("基本通过，细节见下"), Verdict::NotPassed);
    assert_eq!(verdict("通过，但要修改文案"), Verdict::NotPassed);
    assert_eq!(verdict("不通过，要修改"), Verdict::NotPassed);
}

#[test]
fn the_agent_trailer_says_who_wrote_a_pull_request() {
    assert_eq!(
        record::agent_trailer("改了什么\n\nAgent: Claude\n").as_deref(),
        Some("Claude")
    );
    assert_eq!(
        record::agent_trailer("Agent：Codex").as_deref(),
        Some("Codex")
    );
    assert_eq!(
        record::agent_trailer("Agent: `CodeBuddy`").as_deref(),
        Some("CodeBuddy")
    );
    // The last trailer is this body's own: a body that quotes the
    // convention first still reports itself.
    assert_eq!(
        record::agent_trailer("正文最后一行写 `Agent: <名字>`\n\nAgent: Claude").as_deref(),
        Some("Claude")
    );
    assert_eq!(record::agent_trailer("没有 trailer"), None);
    assert_eq!(record::agent_trailer("Agent:"), None);
    assert_eq!(record::agent_trailer("Agent: <名字>"), None);
}

#[test]
fn an_agent_is_the_same_agent_however_it_signs_itself() {
    assert!(same_agent("Claude", "claude"));
    assert!(same_agent(" Claude ", "Claude（第 4 条线）"));
    assert!(same_agent("CodeBuddy", "codebuddy"));
    assert!(!same_agent("Claude", "Codex"));
    assert!(!same_agent("", ""));
}

#[test]
fn a_run_is_green_only_when_both_platforms_finished_green() {
    let run = |name: &str, status: &str, conclusion: Option<&str>| CheckRun {
        name: name.to_string(),
        status: status.to_string(),
        conclusion: conclusion.map(str::to_string),
    };
    let green = |runs: Vec<CheckRun>| {
        checks_refusal(&Checks {
            head_sha: SHA.to_string(),
            runs,
        })
    };
    let both = vec![
        run("check (ubuntu-latest)", "completed", Some("success")),
        run("check (macos-latest)", "completed", Some("success")),
    ];
    assert_eq!(green(both.clone()), None);
    assert!(matches!(
        green(vec![both[0].clone()]),
        Some(Refusal::ChecksMissing { .. })
    ));
    assert!(matches!(green(vec![]), Some(Refusal::ChecksMissing { .. })));
    let mut running = both.clone();
    running[1] = run("check (macos-latest)", "in_progress", None);
    assert!(matches!(
        green(running),
        Some(Refusal::ChecksUnfinished { .. })
    ));
    let mut cancelled = both.clone();
    cancelled[1] = run("check (macos-latest)", "completed", Some("cancelled"));
    assert!(matches!(
        green(cancelled),
        Some(Refusal::ChecksNotGreen { .. })
    ));
    let mut failed = both.clone();
    failed[0] = run("check (ubuntu-latest)", "completed", Some("failure"));
    assert!(matches!(
        green(failed),
        Some(Refusal::ChecksNotGreen { .. })
    ));
    // A required job GitHub finished without running is not a pass: the
    // platform it stands for was never covered.
    for conclusion in [
        "skipped",
        "neutral",
        "stale",
        "action_required",
        "timed_out",
    ] {
        let mut not_run = both.clone();
        not_run[1] = run("check (macos-latest)", "completed", Some(conclusion));
        assert!(
            matches!(green(not_run), Some(Refusal::ChecksNotGreen { .. })),
            "{conclusion} 被当成通过了"
        );
    }
    // A job the project does not require may skip; `check.sh` is not it.
    let mut extra = both;
    extra.push(run("codeql", "completed", Some("skipped")));
    assert_eq!(green(extra), None);
}

#[test]
fn a_refusal_says_what_to_do_about_it() {
    // Whatever the gate refuses with, an agent reads it on the pull
    // request, so it is never empty and never only a sha.
    for refusal in [
        Refusal::NoReviewRecord,
        Refusal::Draft,
        Refusal::NoImplementerTrailer,
        Refusal::Conflicted,
        Refusal::MergeabilityUnknown,
        Refusal::NotOpen {
            state: "closed".into(),
        },
        Refusal::BlockingLabel {
            label: "要修改".into(),
        },
        Refusal::MalformedRecord {
            url: "u".into(),
            missing: "审查者".into(),
        },
        Refusal::VerdictNotPassed {
            reviewer: "Claude".into(),
            url: "u".into(),
            said: "要修改".into(),
        },
        Refusal::ReviewerIsImplementer {
            agent: "Claude".into(),
            url: "u".into(),
        },
        Refusal::PushedAfterReview {
            reviewed: SHA.into(),
            head: MAIN.into(),
            pushed_at: "2026-10-13T18:20:00Z".into(),
        },
        Refusal::ShaMismatch {
            reviewed: SHA.into(),
            head: MAIN.into(),
        },
        Refusal::ChecksForOtherCommit {
            checked: SHA.into(),
            reviewed: MAIN.into(),
        },
        Refusal::ChecksMissing {
            missing: vec!["check (macos-latest)".into()],
        },
        Refusal::ChecksUnfinished {
            unfinished: vec!["check (macos-latest)（in_progress）".into()],
        },
        Refusal::ChecksNotGreen {
            not_green: vec!["check (ubuntu-latest)（failure）".into()],
        },
        Refusal::MainMoved {
            reviewed_main: MAIN.into(),
            main: SHA.into(),
        },
    ] {
        let said = refusal.to_string();
        assert!(said.chars().count() > 10, "{refusal:?} 说得太少：{said}");
        assert!(!said.contains(SHA), "{refusal:?} 写了整个 sha：{said}");
    }
}

#[test]
fn a_sha_is_shortened_the_way_git_log_shows_it() {
    assert_eq!(short(SHA), "0dbf6fc");
    assert_eq!(short("abc"), "abc");
    assert_eq!(short(""), "");
}

#[test]
fn an_unaddressed_change_request_on_the_same_head_blocks_another_reviewers_pass() {
    // PR #127 (#160): two independent reviewers (`GPT` and `Codex`) reviewed
    // the same head `aa6a8cf2f72de427f7e9a631211705101232bdf3` on the same
    // main `a9f06c92bfa4a4e7be6705b6faa89f9b1921690d`. One passed and one
    // asked for changes (`要修改`). Whichever record arrived first, and even
    // if the implementer (`Claude`) replies on the PR afterwards, the gate
    // must refuse until either a new head is pushed or the reviewer who
    // asked for changes (`Codex`) posts a newer `通过` on this head.
    const PR127_HEAD: &str = "aa6a8cf2f72de427f7e9a631211705101232bdf3";
    const PR127_MAIN: &str = "a9f06c92bfa4a4e7be6705b6faa89f9b1921690d";
    const GPT_URL: &str = "https://github.com/zbtzbtzbt/theFlash/pull/127#issuecomment-6162457432";
    const CODEX_URL: &str =
        "https://github.com/zbtzbtzbt/theFlash/pull/127#issuecomment-6162458297";

    let make_record = |reviewer: &str, verdict: &str, sha: &str, url: &str, time: &str| {
        record::parse(
            &format!(
                "## 审查记录\n\n\
                 - 被审查的 SHA: {sha}\n\
                 - 合到的 main: {PR127_MAIN}\n\
                 - 审查者: {reviewer}\n\
                 - 结论: {verdict}\n\
                 - 跑了什么: 临时 HOME 下完整 make check\n"
            ),
            url,
            at(time),
        )
        .expect("审查记录应能解析")
    };

    let snapshot = |head: &str, commits: Vec<Commit>, records: Vec<ReviewRecord>| Snapshot {
        pull: Pull {
            number: 127,
            state: "open".into(),
            draft: false,
            head_sha: head.into(),
            base_ref: "main".into(),
            title: "inspect: 只有样本文件真的换了人，报告才说“换掉了这一类原来的样本”（#79）"
                .into(),
            body: "修复 #79\n\nAgent: Claude\n".into(),
            labels: Vec::new(),
            mergeable: Some(true),
        },
        commits,
        checks: Checks {
            head_sha: head.into(),
            runs: vec![
                CheckRun {
                    name: "check (ubuntu-latest)".into(),
                    status: "completed".into(),
                    conclusion: Some("success".into()),
                },
                CheckRun {
                    name: "check (macos-latest)".into(),
                    status: "completed".into(),
                    conclusion: Some("success".into()),
                },
            ],
        },
        main_head: Commit {
            sha: PR127_MAIN.into(),
            committed_at: at("2026-10-14T06:20:00Z"),
        },
        records,
    };

    let base_commits = vec![Commit {
        sha: PR127_HEAD.into(),
        committed_at: at("2026-10-14T06:25:00Z"),
    }];

    // Case 1: GPT posts `通过` at 06:36:07Z, Codex posts `要修改` at 06:36:11Z.
    let gpt_pass = make_record("GPT", "通过", PR127_HEAD, GPT_URL, "2026-10-14T06:36:07Z");
    let codex_changes = make_record(
        "Codex",
        "要修改",
        PR127_HEAD,
        CODEX_URL,
        "2026-10-14T06:36:11Z",
    );
    let d1 = decide(&snapshot(
        PR127_HEAD,
        base_commits.clone(),
        vec![gpt_pass.clone(), codex_changes.clone()],
    ));
    assert_eq!(
        d1,
        Decision::Refuse(Refusal::VerdictNotPassed {
            reviewer: "Codex".into(),
            url: CODEX_URL.into(),
            said: "要修改".into(),
        })
    );

    // Case 2 (reverse order): Codex posts `要修改` first, then GPT posts `通过` on the same head.
    let codex_first = make_record(
        "Codex",
        "要修改",
        PR127_HEAD,
        CODEX_URL,
        "2026-10-14T06:36:07Z",
    );
    let gpt_second = make_record("GPT", "通过", PR127_HEAD, GPT_URL, "2026-10-14T06:36:11Z");
    let d2 = decide(&snapshot(
        PR127_HEAD,
        base_commits.clone(),
        vec![codex_first.clone(), gpt_second.clone()],
    ));
    assert_eq!(
        d2,
        Decision::Refuse(Refusal::VerdictNotPassed {
            reviewer: "Codex".into(),
            url: CODEX_URL.into(),
            said: "要修改".into(),
        }),
        "先到的 要修改 不能被另一个审查者后到的 通过 覆盖：{d2:?}"
    );

    // Case 3: Implementer (`Claude`) posts after Codex's `要修改` (even if formatted as a record)
    // while GPT has `通过` -> still refused with Codex's `要修改`.
    let impl_record = make_record(
        "Claude",
        "通过",
        PR127_HEAD,
        "https://github.com/zbtzbtzbt/theFlash/pull/127#issuecomment-6162813725",
        "2026-10-14T07:01:19Z",
    );
    let d3 = decide(&snapshot(
        PR127_HEAD,
        base_commits.clone(),
        vec![gpt_pass.clone(), codex_changes.clone(), impl_record],
    ));
    assert_eq!(
        d3,
        Decision::Refuse(Refusal::VerdictNotPassed {
            reviewer: "Codex".into(),
            url: CODEX_URL.into(),
            said: "要修改".into(),
        }),
        "实现者自己的评论或记录不能解锁另一个审查者的 要修改：{d3:?}"
    );

    // Case 4: Cleared by the same reviewer (`Codex`, even with lane suffix) posting a newer `通过` on the same head.
    let codex_repass_url = "https://github.com/zbtzbtzbt/theFlash/pull/127#issuecomment-6162900000";
    let codex_repass = make_record(
        "codex（复审）",
        "通过",
        PR127_HEAD,
        codex_repass_url,
        "2026-10-14T07:10:00Z",
    );
    let d4 = decide(&snapshot(
        PR127_HEAD,
        base_commits.clone(),
        vec![gpt_pass, codex_changes.clone(), codex_repass],
    ));
    assert_eq!(
        d4,
        Decision::Merge {
            sha: PR127_HEAD.into(),
            reviewer: "codex（复审）".into(),
            record_url: codex_repass_url.into(),
        }
    );

    // Case 5: Cleared by pushing a new head `H2` and getting an independent `通过` on `H2`.
    let new_head = "a3e3f8d000000000000000000000000000000000";
    let gpt_on_new_url = "https://github.com/zbtzbtzbt/theFlash/pull/127#issuecomment-6162950000";
    let gpt_on_new = make_record(
        "GPT",
        "通过",
        new_head,
        gpt_on_new_url,
        "2026-10-14T07:15:00Z",
    );
    let mut commits_with_new = base_commits;
    commits_with_new.push(Commit {
        sha: new_head.into(),
        committed_at: at("2026-10-14T07:05:00Z"),
    });
    let d5 = decide(&snapshot(
        new_head,
        commits_with_new,
        vec![codex_changes, gpt_on_new],
    ));
    assert_eq!(
        d5,
        Decision::Merge {
            sha: new_head.into(),
            reviewer: "GPT".into(),
            record_url: gpt_on_new_url.into(),
        }
    );
}

// ---- answers that came back in pages ----

#[test]
fn an_answer_in_several_pages_reads_as_one_list() {
    // What `gh api --paginate` prints for a list that did not fit on one
    // page: one JSON array per page, back to back (`gh api --help`).
    use super::snapshot::list_from_json;
    let one_page = "[{\"number\": 1}, {\"number\": 2}]";
    let two_pages = "[{\"number\": 1}]\n[{\"number\": 2}]\n";
    let and_an_empty_one = "[{\"number\": 1}]\n[{\"number\": 2}]\n[]\n";
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Item {
        number: u64,
    }
    let numbers = |text: &str| {
        list_from_json::<Item>("open PRs", text)
            .map(|v| v.into_iter().map(|i| i.number).collect::<Vec<_>>())
    };
    assert_eq!(numbers(one_page).unwrap(), [1, 2]);
    assert_eq!(numbers(two_pages).unwrap(), [1, 2]);
    assert_eq!(numbers(and_an_empty_one).unwrap(), [1, 2]);
    assert_eq!(numbers("[]").unwrap(), Vec::<u64>::new());
    assert_eq!(numbers("").unwrap(), Vec::<u64>::new());
    // Something that is not a list of these items is still an error, and
    // the error says which answer could not be read.
    let bad = numbers("[{\"number\": 1}]\n{\"message\": \"Not Found\"}")
        .expect_err("第二页不是这个列表")
        .to_string();
    assert!(bad.contains("open PRs"), "{bad}");
}

// ---- the merge lock ----
//
// Several sessions run the gate at once (#20), so the lock is the only
// thing between them and two merges landing on main without having been
// reviewed against each other. These tests are about the two ways that
// mutual exclusion can be lost: taking over a lock whose holder is still
// running, and a holder releasing a lock that is no longer its own.

mod merge_lock {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, SystemTime};

    use super::super::lock::{self, Lock};

    /// A pid no process has: past the highest pid macOS (99999) and Linux
    /// (4194304 at most) hand out, so `kill(pid, 0)` says "no such
    /// process" without this test having to start and end one.
    const PID_OF_NOBODY: u32 = 0x4000_0000;

    /// A state directory of this test's own, emptied first so a run does
    /// not inherit the lock of the previous one.
    fn state(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("merge-gate-lock-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Makes the lock look as old as a lock taken `by` ago, the way the
    /// clock reads it on a session that has been running that long.
    fn age_lock(dir: &Path, by: Duration) {
        let path = dir.join("merge.lock");
        let times = std::fs::FileTimes::new().set_modified(SystemTime::now() - by);
        std::fs::File::open(&path)
            .unwrap()
            .set_times(times)
            .unwrap();
    }

    fn holder_file(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("merge.lock/holder")).unwrap_or_default()
    }

    #[test]
    fn one_session_at_a_time_holds_the_lock() {
        let dir = state("one-at-a-time");
        let a = Lock::acquire_in(&dir, "merge --pr 1").expect("第一个会话拿到锁");
        let busy = Lock::acquire_in(&dir, "merge --pr 2").expect_err("第二个会话不能同时合并");
        assert!(busy.to_string().contains("merge --pr 1"), "{busy}");
        drop(a);
        let c = Lock::acquire_in(&dir, "merge --pr 3").expect("放了锁之后下一个会话拿得到");
        drop(c);
    }

    #[test]
    fn a_session_that_is_still_running_keeps_its_lock_however_old_it_looks() {
        // A merge that has been running a long time is a session to go
        // look at, not a lock to take: taking it over puts two merges on
        // main at the same time, which is the one thing the lock is for.
        let dir = state("old-but-running");
        let a = Lock::acquire_in(&dir, "merge --pr 1").unwrap();
        age_lock(&dir, Duration::from_secs(40 * 60));
        let busy = Lock::acquire_in(&dir, "merge --pr 2")
            .expect_err("持有者还活着，不能因为锁看起来旧就接管");
        assert!(busy.to_string().contains("merge --pr 1"), "{busy}");
        drop(a);
    }

    #[test]
    fn a_lock_left_behind_by_a_session_that_is_gone_is_taken_over() {
        // Without this, one crashed session stops every merge for the
        // night.
        let dir = state("holder-gone");
        lock::plant(&dir, "merge --pr 1", PID_OF_NOBODY);
        age_lock(&dir, Duration::from_secs(40 * 60));
        let b = Lock::acquire_in(&dir, "merge --pr 2").expect("接管已经退出的持有者的锁");
        assert!(
            holder_file(&dir).contains("merge --pr 2"),
            "{}",
            holder_file(&dir)
        );
        drop(b);
    }

    #[test]
    fn the_session_whose_lock_was_taken_over_does_not_release_the_new_one() {
        // A dies, B takes the lock over, A's process ends and runs its
        // Drop. A must not delete B's lock: the next session would walk
        // straight into the merge B is in the middle of.
        let dir = state("released-by-the-wrong-holder");
        let a = Lock::acquire_in(&dir, "merge --pr 1").unwrap();
        // The takeover, by hand, because A is not really dead here.
        std::fs::rename(dir.join("merge.lock"), dir.join("taken-over")).unwrap();
        std::fs::remove_dir_all(dir.join("taken-over")).unwrap();
        let b = Lock::acquire_in(&dir, "merge --pr 2").unwrap();
        drop(a);
        assert!(
            dir.join("merge.lock").exists(),
            "原持有者把接管者的锁删了：{}",
            dir.display()
        );
        assert!(
            holder_file(&dir).contains("merge --pr 2"),
            "锁不再属于接管者：{}",
            holder_file(&dir)
        );
        drop(b);
    }

    #[test]
    fn only_one_of_several_sessions_takes_over_a_dead_holders_lock() {
        // Every session that reaches a stale lock at the same moment sees
        // the same stale lock; only one of them may end up holding it.
        let dir = state("racing-takeover");
        lock::plant(&dir, "merge --pr 1", PID_OF_NOBODY);
        age_lock(&dir, Duration::from_secs(40 * 60));
        let start = Arc::new(Barrier::new(8));
        let mut threads = Vec::new();
        for i in 0..8 {
            let dir = dir.clone();
            let start = start.clone();
            threads.push(std::thread::spawn(move || {
                start.wait();
                Lock::acquire_in(&dir, &format!("merge --pr {}", 100 + i)).ok()
            }));
        }
        // Nobody releases until every thread has tried, so a second
        // winner can only mean two sessions held the lock at once.
        let held: Vec<Lock> = threads
            .into_iter()
            .filter_map(|t| t.join().unwrap())
            .collect();
        assert_eq!(held.len(), 1, "{} 个会话同时拿到了合并锁", held.len());
    }
}
