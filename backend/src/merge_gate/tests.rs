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
fn only_an_explicit_pass_counts_as_passing() {
    let verdict = |v: &str| {
        record(&CANONICAL.replace("- 结论: 通过", &format!("- 结论: {v}")))
            .unwrap()
            .verdict
    };
    assert_eq!(verdict("通过"), Verdict::Passed);
    assert_eq!(verdict("通过，可以合并"), Verdict::Passed);
    assert_eq!(verdict("要修改"), Verdict::ChangesRequested);
    assert_eq!(verdict("不通过"), Verdict::ChangesRequested);
    assert_eq!(verdict(""), Verdict::ChangesRequested);
    assert_eq!(verdict("待定"), Verdict::ChangesRequested);
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
        Some(Refusal::ChecksFailed { .. })
    ));
    let mut failed = both;
    failed[0] = run("check (ubuntu-latest)", "completed", Some("failure"));
    assert!(matches!(green(failed), Some(Refusal::ChecksFailed { .. })));
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
        Refusal::ChangesRequested {
            reviewer: "Claude".into(),
            url: "u".into(),
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
        Refusal::ChecksFailed {
            failed: vec!["check (ubuntu-latest)（failure）".into()],
        },
        Refusal::MainMovedOverlap {
            reviewed_main: MAIN.into(),
            main: SHA.into(),
            files: vec!["backend/src/lib.rs".into()],
        },
        Refusal::MainMovedUnknown {
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
