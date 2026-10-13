//! 合并闸门：按 CI 和独立审查记录决定一个 PR 能不能合并（#15）。
//!
//! The gate never calls a model. It reads GitHub's answers, matches them
//! against the review record a reviewer posted on the pull request, and
//! either says "merge this exact commit" or refuses with the reason. The
//! decision is a pure function of a [`Snapshot`], so every refusal in
//! issue #15 is a test over replayed GitHub responses
//! (backend/tests/merge_gate.rs); nothing in here talks to GitHub.

pub mod lock;
pub mod record;
pub mod snapshot;

#[cfg(test)]
mod tests;

pub use record::{ReviewRecord, Verdict};
pub use snapshot::{CheckRun, Checks, Commit, Pull, Snapshot};

/// The jobs `.github/workflows/test.yml` runs on every pull request. A
/// green run that is missing one of them did not cover both platforms the
/// project supports, so the gate treats it as unfinished.
pub const REQUIRED_CHECKS: [&str; 2] = ["check (ubuntu-latest)", "check (macos-latest)"];

/// Labels that stop a merge no matter what the rest says.
pub const BLOCKING_LABELS: [&str; 3] = ["要修改", "不要合并", "等 owner"];

/// What the gate decided about one pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Merge this commit, citing this review record.
    Merge {
        sha: String,
        reviewer: String,
        record_url: String,
    },
    Refuse(Refusal),
}

impl Decision {
    pub fn is_merge(&self) -> bool {
        matches!(self, Decision::Merge { .. })
    }
}

/// Why the gate refused. Every variant says what to do next, because the
/// agent reading it is the one who has to fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    NotOpen {
        state: String,
    },
    Draft,
    BlockingLabel {
        label: String,
    },
    NoImplementerTrailer,
    NoReviewRecord,
    MalformedRecord {
        url: String,
        missing: String,
    },
    ChangesRequested {
        reviewer: String,
        url: String,
    },
    ReviewerIsImplementer {
        agent: String,
        url: String,
    },
    PushedAfterReview {
        reviewed: String,
        head: String,
        pushed_at: String,
    },
    ShaMismatch {
        reviewed: String,
        head: String,
    },
    ChecksForOtherCommit {
        checked: String,
        reviewed: String,
    },
    ChecksMissing {
        missing: Vec<String>,
    },
    ChecksUnfinished {
        unfinished: Vec<String>,
    },
    ChecksFailed {
        failed: Vec<String>,
    },
    MainMovedOverlap {
        reviewed_main: String,
        main: String,
        files: Vec<String>,
    },
    MainMovedUnknown {
        reviewed_main: String,
        main: String,
    },
    Conflicted,
    MergeabilityUnknown,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotOpen { state } => {
                write!(f, "PR 不是打开状态（state={state}），没有要合并的东西")
            }
            Refusal::Draft => write!(f, "PR 还是草稿，实现者先点 ready for review"),
            Refusal::BlockingLabel { label } => {
                write!(f, "PR 带着 `{label}` 标签，先把标签去掉再合并")
            }
            Refusal::NoImplementerTrailer => write!(
                f,
                "PR 正文最后一行没有 `Agent: <名字>`，认不出实现者是哪个 agent，\
                 没法确认审查者不是实现者"
            ),
            Refusal::NoReviewRecord => write!(
                f,
                "PR 上没有审查记录：合并前要有一条独立审查者写的“审查记录”评论\
                （格式见 docs/subsystems/merge-gate.md）"
            ),
            Refusal::MalformedRecord { url, missing } => write!(
                f,
                "审查记录缺少字段 `{missing}`：{url}（格式见 docs/subsystems/merge-gate.md）"
            ),
            Refusal::ChangesRequested { reviewer, url } => {
                write!(f, "最新的审查记录是“要修改”（审查者 {reviewer}）：{url}")
            }
            Refusal::ReviewerIsImplementer { agent, url } => write!(
                f,
                "审查者和实现者是同一个 agent（{agent}），要另一个 agent 审：{url}"
            ),
            Refusal::PushedAfterReview {
                reviewed,
                head,
                pushed_at,
            } => write!(
                f,
                "审查之后又有新推送：审查的是 {}，现在的 head 是 {}（推于 {pushed_at}），\
                 新 head 要重新审查",
                short(reviewed),
                short(head)
            ),
            Refusal::ShaMismatch { reviewed, head } => write!(
                f,
                "审查记录写的 SHA 和 PR 的 head 对不上：审查记录是 {}，head 是 {}",
                short(reviewed),
                short(head)
            ),
            Refusal::ChecksForOtherCommit { checked, reviewed } => write!(
                f,
                "拿到的 CI 结果属于 {}，不是被审查的 {}",
                short(checked),
                short(reviewed)
            ),
            Refusal::ChecksMissing { missing } => {
                write!(f, "CI 没跑完：被审查的提交上少了 {}", missing.join("、"))
            }
            Refusal::ChecksUnfinished { unfinished } => {
                write!(f, "CI 还在跑：{}", unfinished.join("、"))
            }
            Refusal::ChecksFailed { failed } => {
                write!(f, "CI 是红的：{}", failed.join("、"))
            }
            Refusal::MainMovedOverlap {
                reviewed_main,
                main,
                files,
            } => write!(
                f,
                "审查是在 main {} 上做的，main 已经走到 {}，两边都改了 {}，\
                 要在新 main 上重新审查",
                short(reviewed_main),
                short(main),
                files.join("、")
            ),
            Refusal::MainMovedUnknown {
                reviewed_main,
                main,
            } => write!(
                f,
                "审查是在 main {} 上做的，main 已经走到 {}，但没拿到这段时间 main 改了哪些文件，\
                 没法判断会不会互相影响",
                short(reviewed_main),
                short(main)
            ),
            Refusal::Conflicted => write!(
                f,
                "PR 和 main 冲突，GitHub 合不上：实现者先 rebase 到最新 main，再重新审查"
            ),
            Refusal::MergeabilityUnknown => write!(
                f,
                "GitHub 还没算出这个 PR 能不能合并（mergeable 是 null），过一会儿再试"
            ),
        }
    }
}

/// First 7 characters of a sha, the way git logs show it.
fn short(sha: &str) -> &str {
    let n = sha.char_indices().nth(7).map_or(sha.len(), |(i, _)| i);
    &sha[..n]
}

/// Can this pull request be merged? The checks run from the cheapest and
/// most specific to the most general, so the reason an agent gets back is
/// the one thing it has to fix first.
pub fn decide(s: &Snapshot) -> Decision {
    use Refusal as R;
    if s.pull.state != "open" {
        return Decision::Refuse(R::NotOpen {
            state: s.pull.state.clone(),
        });
    }
    if s.pull.draft {
        return Decision::Refuse(R::Draft);
    }
    if let Some(label) = s
        .pull
        .labels
        .iter()
        .find(|l| BLOCKING_LABELS.contains(&l.as_str()))
    {
        return Decision::Refuse(R::BlockingLabel {
            label: label.clone(),
        });
    }
    let Some(implementer) = record::agent_trailer(&s.pull.body) else {
        return Decision::Refuse(R::NoImplementerTrailer);
    };
    // The last record posted wins: a reviewer who posts a second record
    // after a fix has the final word.
    let Some(rec) = s
        .records
        .iter()
        .max_by(|a, b| a.posted_at.cmp(&b.posted_at))
    else {
        return Decision::Refuse(R::NoReviewRecord);
    };
    if let Some(missing) = rec.missing_field() {
        return Decision::Refuse(R::MalformedRecord {
            url: rec.url.clone(),
            missing: missing.to_string(),
        });
    }
    if rec.verdict == Verdict::ChangesRequested {
        return Decision::Refuse(R::ChangesRequested {
            reviewer: rec.reviewer.clone(),
            url: rec.url.clone(),
        });
    }
    if same_agent(&rec.reviewer, &implementer) {
        return Decision::Refuse(R::ReviewerIsImplementer {
            agent: implementer,
            url: rec.url.clone(),
        });
    }
    if rec.reviewed_sha != s.pull.head_sha {
        // The reviewed commit still being one of this pull request's
        // commits means the implementer pushed on top of it after the
        // review. A reviewed sha this pull request never had is a record
        // that points somewhere else entirely.
        let commit = |sha: &str| s.commits.iter().find(|c| c.sha == sha);
        let pushed_on_top = commit(&rec.reviewed_sha).and(commit(&s.pull.head_sha));
        return Decision::Refuse(match pushed_on_top {
            Some(head) => R::PushedAfterReview {
                reviewed: rec.reviewed_sha.clone(),
                head: s.pull.head_sha.clone(),
                pushed_at: head.committed_at.to_rfc3339(),
            },
            None => R::ShaMismatch {
                reviewed: rec.reviewed_sha.clone(),
                head: s.pull.head_sha.clone(),
            },
        });
    }
    if s.checks.head_sha != rec.reviewed_sha {
        return Decision::Refuse(R::ChecksForOtherCommit {
            checked: s.checks.head_sha.clone(),
            reviewed: rec.reviewed_sha.clone(),
        });
    }
    if let Some(r) = checks_refusal(&s.checks) {
        return Decision::Refuse(r);
    }
    if s.main_head.sha != rec.reviewed_main {
        match &s.main_since_review {
            None => {
                return Decision::Refuse(R::MainMovedUnknown {
                    reviewed_main: rec.reviewed_main.clone(),
                    main: s.main_head.sha.clone(),
                });
            }
            Some(changed) => {
                let both: Vec<String> = s
                    .files
                    .iter()
                    .filter(|f| changed.contains(*f))
                    .cloned()
                    .collect();
                if !both.is_empty() {
                    return Decision::Refuse(R::MainMovedOverlap {
                        reviewed_main: rec.reviewed_main.clone(),
                        main: s.main_head.sha.clone(),
                        files: both,
                    });
                }
            }
        }
    }
    match s.pull.mergeable {
        Some(false) => return Decision::Refuse(R::Conflicted),
        None => return Decision::Refuse(R::MergeabilityUnknown),
        Some(true) => {}
    }
    Decision::Merge {
        sha: rec.reviewed_sha.clone(),
        reviewer: rec.reviewer.clone(),
        record_url: rec.url.clone(),
    }
}

/// Agent names are compared the way agents write them: `claude` and
/// `Claude` are the same agent, `Claude (lane 4)` is still Claude.
fn same_agent(a: &str, b: &str) -> bool {
    fn key(s: &str) -> String {
        s.trim()
            .to_lowercase()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    }
    let (a, b) = (key(a), key(b));
    !a.is_empty() && a == b
}

/// The CI side: every required job present, finished, and green.
fn checks_refusal(checks: &Checks) -> Option<Refusal> {
    let missing: Vec<String> = REQUIRED_CHECKS
        .iter()
        .filter(|name| !checks.runs.iter().any(|r| r.name == **name))
        .map(|n| n.to_string())
        .collect();
    if !missing.is_empty() {
        return Some(Refusal::ChecksMissing { missing });
    }
    let unfinished: Vec<String> = checks
        .runs
        .iter()
        .filter(|r| r.status != "completed")
        .map(|r| format!("{}（{}）", r.name, r.status))
        .collect();
    if !unfinished.is_empty() {
        return Some(Refusal::ChecksUnfinished { unfinished });
    }
    let failed: Vec<String> = checks
        .runs
        .iter()
        .filter(|r| {
            !matches!(
                r.conclusion.as_deref(),
                Some("success" | "neutral" | "skipped")
            )
        })
        .map(|r| {
            format!(
                "{}（{}）",
                r.name,
                r.conclusion.as_deref().unwrap_or("没有结论")
            )
        })
        .collect();
    if !failed.is_empty() {
        return Some(Refusal::ChecksFailed { failed });
    }
    None
}
