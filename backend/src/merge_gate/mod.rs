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

/// The label set on every issue a merged pull request names, so the
/// dispatcher and `claims queue` know the implementation has landed and
/// now waits for an independent agent to accept it (#102).
pub const ACCEPT_LABEL: &str = "待验收";

/// Removed when a fix pull request merges so `dispatch` schedules a fresh
/// acceptance round instead of another `fix-issue` session (#102).
pub const REJECTED_LABEL: &str = "验收未通过";

/// Which issues a merged pull request hands over to acceptance (#102).
///
/// A title mentions numbers for all sorts of reasons — the pull request it
/// fixes, the issues a nightly review read, another repository's issue —
/// so the title alone cannot say which issues this pull request
/// implemented. Only the pull request itself can, in its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandOff {
    /// The body declared these issues (`实现: #102`), in the order written.
    Declared(Vec<u64>),
    /// The body declared that this pull request implements no issue
    /// (`实现: 无`), the way a lessons or docs pull request does.
    Nothing,
    /// The body declared nothing. The numbers are the ones its title
    /// mentions, which the gate only prints: changing labels on a number
    /// nobody declared is what marked pull requests `待验收` and cleared
    /// `验收未通过` from issues nobody had fixed (#102 的验收结论).
    Undeclared(Vec<u64>),
}

/// The issues a pull request's body declares it implemented, read from the
/// `实现: #<号>` trailer (#102). `实现: 无` means it implements none; no
/// trailer at all leaves the decision to the agent that merges it.
pub fn issues_to_hand_off(title: &str, body: &str, pull_number: u64) -> HandOff {
    let mut declared: Vec<u64> = Vec::new();
    let mut declared_none = false;
    for line in body.lines() {
        let Some(value) = implements_trailer(line) else {
            continue;
        };
        // The line AGENTS.md shows (`实现: #<issue>`) is a placeholder, not
        // a declaration; a pull request quoting the instructions still has
        // to declare its own issue.
        if value.contains('<') {
            continue;
        }
        let refs = issue_refs(value, pull_number);
        if refs.is_empty() {
            declared_none |= says_none(value);
            continue;
        }
        for n in refs {
            if !declared.contains(&n) {
                declared.push(n);
            }
        }
    }
    if !declared.is_empty() {
        HandOff::Declared(declared)
    } else if declared_none {
        HandOff::Nothing
    } else {
        HandOff::Undeclared(issues_in_title(title, pull_number))
    }
}

/// The value of a `实现:` line, however the agent marked it up (`- 实现：`,
/// `**实现**: `, `实现的 issue:`). Prose that starts with the same two
/// characters (`实现了 #3 的回流提醒`) is not a declaration.
fn implements_trailer(line: &str) -> Option<&str> {
    let line = line
        .trim()
        .trim_start_matches(['-', '*', '>', '+', ' ', '\t'])
        .trim();
    let rest = line.strip_prefix("实现")?;
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix("的 issue")
        .or_else(|| rest.strip_prefix("的issue"))
        .unwrap_or(rest);
    let rest = rest.trim_start().trim_start_matches('*').trim_start();
    let rest = rest.strip_prefix([':', '：'])?;
    Some(rest.trim())
}

/// `实现: 无` and the other ways an agent writes "this one implements no
/// issue".
fn says_none(value: &str) -> bool {
    let v = value.trim().trim_matches('`').trim();
    v.starts_with('无')
        || v.starts_with("没有")
        || v.starts_with("不涉及")
        || v == "-"
        || v.eq_ignore_ascii_case("none")
        || v.eq_ignore_ascii_case("n/a")
}

/// Every issue number named as `#<number>` in a pull request title
/// (such as `（#15）` or `（#14、#9）`), in the order written, without
/// duplicates or the pull request's own number (#102). Only a hint now:
/// what gets handed off is what the body declares.
pub fn issues_in_title(title: &str, pull_number: u64) -> Vec<u64> {
    issue_refs(title, pull_number)
}

/// The issue numbers `text` names, in the order written and without
/// duplicates or the pull request's own number.
fn issue_refs(text: &str, pull_number: u64) -> Vec<u64> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c != '#' {
            continue;
        }
        // `other/repo#5` is another repository's issue, and `v1#3` is not
        // a reference at all: a reference starts a word. `#156/#163` does,
        // twice, so a `/` before the `#` is fine — a repository name ends
        // in a letter or a digit.
        if i > 0 && chars[i - 1].is_alphanumeric() {
            continue;
        }
        let digits: String = chars[i + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        // GitHub never pads a number, so `#012` is not a reference to #12.
        if digits.len() > 1 && digits.starts_with('0') {
            continue;
        }
        if let Ok(n) = digits.parse::<u64>()
            && n > 0
            && n != pull_number
            && !out.contains(&n)
        {
            out.push(n);
        }
    }
    out
}

/// The comment left on a linked issue after its pull request merges,
/// citing the squash commit on `main`, the reviewed PR head, and the
/// review record that approved it (#102). When `labelled` is false, the
/// comment says the label still needs to be added manually instead of
/// claiming `已加上 待验收`.
#[allow(clippy::too_many_arguments)]
pub fn issue_merged_comment(
    repo: &str,
    issue: u64,
    pr: u64,
    merged_sha: Option<&str>,
    reviewed_sha: &str,
    reviewer: &str,
    record_url: &str,
    labelled: bool,
) -> String {
    let commits = match merged_sha.filter(|m| !m.is_empty()) {
        Some(merged) => format!("合入的提交 `{merged}`，被审查的提交 `{reviewed_sha}`"),
        None => format!(
            "被审查的提交 `{reviewed_sha}`（合入 main 的提交未取到，请用 `gh pr view {pr} --repo {repo} --json mergeCommit` 补查）"
        ),
    };
    if labelled {
        format!(
            "PR #{pr} 已由合并闸门合入 main（{commits}，审查者 {reviewer}，审查记录：{record_url}），\
             已加上 `{ACCEPT_LABEL}`，请实现者之外的 agent 按 prompts/acceptance.md 独立验收。"
        )
    } else {
        format!(
            "PR #{pr} 已由合并闸门合入 main（{commits}，审查者 {reviewer}，审查记录：{record_url}）。\
             自动加 `{ACCEPT_LABEL}` 标签失败，请先手动执行 \
             `gh issue edit {issue} --repo {repo} --add-label {ACCEPT_LABEL} --remove-label {REJECTED_LABEL}`，\
             再由实现者之外的 agent 按 prompts/acceptance.md 独立验收。"
        )
    }
}

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
    VerdictNotPassed {
        reviewer: String,
        url: String,
        said: String,
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
    ChecksNotGreen {
        not_green: Vec<String>,
    },
    MainMoved {
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
            Refusal::VerdictNotPassed {
                reviewer,
                url,
                said,
            } => write!(
                f,
                "审查者 {reviewer} 在这个提交上的最新审查记录结论不是“通过”，写的是“{said}”：{url}。\
                 改好以后推新提交重新审查，或请 {reviewer} 写一条新的记录，结论那一行只写“通过”",
            ),
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
            Refusal::ChecksNotGreen { not_green } => write!(
                f,
                "CI 不是绿的：{}。必需的两个 job 都要是 success，\
                 skipped / neutral / cancelled 都不算跑过这个平台",
                not_green.join("、")
            ),
            Refusal::MainMoved {
                reviewed_main,
                main,
            } => write!(
                f,
                "审查是在 main {} 上做的，main 现在是 {}：这个改动没有在最新的 main 上验证过。\
                 审查者在新 main 上重新跑一遍检查，再写一条新的审查记录（`合到的 main` 写新的那个）",
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
    if s.records.is_empty() {
        return Decision::Refuse(R::NoReviewRecord);
    }
    let on_head = latest_records_on_head(s);
    if on_head.is_empty() {
        let rec = s
            .records
            .iter()
            .max_by(|a, b| a.posted_at.cmp(&b.posted_at))
            .unwrap();
        if let Some(missing) = rec.missing_field() {
            return Decision::Refuse(R::MalformedRecord {
                url: rec.url.clone(),
                missing: missing.to_string(),
            });
        }
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
    // Every reviewer who reviewed the current head has their own latest
    // record on it (#160). If any reviewer's latest record on this head is
    // not a pass, another reviewer's pass cannot override it: either a new
    // head is pushed or the reviewer who asked for changes posts a newer
    // passing record on this head.
    for rec in on_head.iter().rev() {
        if rec.verdict == Verdict::NotPassed
            && !rec.verdict_said.is_empty()
            && !rec.reviewer.is_empty()
            && record::is_sha(&rec.reviewed_sha)
        {
            return Decision::Refuse(R::VerdictNotPassed {
                reviewer: rec.reviewer.clone(),
                url: rec.url.clone(),
                said: rec.verdict_said.clone(),
            });
        }
        if let Some(missing) = rec.missing_field() {
            return Decision::Refuse(R::MalformedRecord {
                url: rec.url.clone(),
                missing: missing.to_string(),
            });
        }
        if rec.verdict == Verdict::NotPassed {
            return Decision::Refuse(R::VerdictNotPassed {
                reviewer: rec.reviewer.clone(),
                url: rec.url.clone(),
                said: rec.verdict_said.clone(),
            });
        }
    }
    let independent: Vec<&ReviewRecord> = on_head
        .iter()
        .copied()
        .filter(|r| !same_agent(&r.reviewer, &implementer))
        .collect();
    let Some(rec) = independent
        .iter()
        .rev()
        .find(|r| r.reviewed_main == s.main_head.sha)
        .or_else(|| independent.last())
        .copied()
    else {
        let rec = on_head.last().unwrap();
        return Decision::Refuse(R::ReviewerIsImplementer {
            agent: implementer,
            url: rec.url.clone(),
        });
    };
    if s.checks.head_sha != rec.reviewed_sha {
        return Decision::Refuse(R::ChecksForOtherCommit {
            checked: s.checks.head_sha.clone(),
            reviewed: rec.reviewed_sha.clone(),
        });
    }
    if let Some(r) = checks_refusal(&s.checks) {
        return Decision::Refuse(r);
    }
    // #15 asks for the change to be verified on the latest main, and the
    // review record says which main it was verified on. main having moved
    // since means it was not. Comparing the files the two sides touched is
    // not a substitute: a call site, a trait, a fixture or a CI step in
    // another file breaks just as well, and the only thing that answers it
    // is running the checks on the merge result (0007).
    if s.main_head.sha != rec.reviewed_main {
        return Decision::Refuse(R::MainMoved {
            reviewed_main: rec.reviewed_main.clone(),
            main: s.main_head.sha.clone(),
        });
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

/// Review records that apply to the pull request's current head, grouped by
/// reviewer (`agent_key`) so each reviewer has their own latest word on this
/// commit (#160). A reviewer who asked for changes and later posted `通过` on
/// the same head clears their own objection; another reviewer's `通过` (or a
/// comment from the implementer) never overwrites it.
fn latest_records_on_head(s: &Snapshot) -> Vec<&ReviewRecord> {
    let head = &s.pull.head_sha;
    let head_committed_at = s
        .commits
        .iter()
        .find(|c| &c.sha == head)
        .map(|c| c.committed_at);
    let mut on_head: Vec<&ReviewRecord> = s
        .records
        .iter()
        .filter(|r| {
            if &r.reviewed_sha == head {
                return true;
            }
            if record::is_sha(&r.reviewed_sha) {
                return false;
            }
            if !r.reviewed_sha.is_empty() {
                return head.starts_with(&r.reviewed_sha);
            }
            head_committed_at.is_none_or(|t| r.posted_at >= t)
        })
        .collect();
    on_head.sort_by_key(|r| r.posted_at);
    let mut latest: Vec<&ReviewRecord> = Vec::new();
    for rec in on_head {
        let key = agent_key(&rec.reviewer);
        if !key.is_empty() {
            latest.retain(|prev| agent_key(&prev.reviewer) != key);
        }
        latest.push(rec);
    }
    latest
}

/// Agent names are compared the way agents write them: `claude` and
/// `Claude` are the same agent, `Claude (lane 4)` is still Claude.
fn same_agent(a: &str, b: &str) -> bool {
    let (a, b) = (agent_key(a), agent_key(b));
    !a.is_empty() && a == b
}

fn agent_key(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect()
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
    // A required job has to be `success`. GitHub reports a job that never
    // ran as `completed` with `skipped` or `neutral`, which says the
    // platform was not covered, not that it passed — and a workflow edit
    // or a path filter can turn a required job into a skipped one without
    // anyone noticing. Jobs that are not required may be skipped or
    // neutral; failing is still failing whoever ran it.
    let not_green: Vec<String> = checks
        .runs
        .iter()
        .filter(|r| {
            let required = REQUIRED_CHECKS.contains(&r.name.as_str());
            match r.conclusion.as_deref() {
                Some("success") => false,
                Some("neutral" | "skipped") => required,
                _ => true,
            }
        })
        .map(|r| {
            format!(
                "{}（{}）",
                r.name,
                r.conclusion.as_deref().unwrap_or("没有结论")
            )
        })
        .collect();
    if !not_green.is_empty() {
        return Some(Refusal::ChecksNotGreen { not_green });
    }
    None
}
