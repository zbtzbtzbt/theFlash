//! 调度程序看到的 GitHub：一次取齐，再决定派什么活。
//!
//! Everything a decision is made from is in a [`Snapshot`]: the open
//! issues and which agent claimed them, the open pull requests and who
//! wrote them, and whether main's checks are red. A snapshot comes either
//! from `gh` or from a directory of saved responses, so a test replays the
//! same bytes GitHub returned and never reaches the network.

use std::path::Path;
use std::process::Command;

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// An open issue as the dispatcher needs it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    /// The agent named by the newest claim or heartbeat comment, if any.
    /// All agents share one GitHub account, so who is working on an issue
    /// comes from what the comments say, not from `user.login` (#15).
    pub agent: Option<String>,
    /// When the newest comment on the issue was posted.
    pub last_comment_at: Option<DateTime<Utc>>,
}

/// An open pull request as the dispatcher needs it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pull {
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    /// Head commit, so a review task is tied to the commit it reviewed and
    /// a new push becomes a new task.
    pub head_sha: String,
    pub head_branch: String,
    /// The agent that wrote it, from the `Agent:` line in the body.
    pub agent: Option<String>,
    /// The issue the title refers to (`（#14）`), so a pull request and its
    /// issue count as the same piece of work.
    pub issue: Option<u64>,
}

/// What the Test workflow says about main. Only the Test workflow counts:
/// a refused Release run also sits on main's commit (the release gate
/// failing by design, #5), and treating that as a red main would send an
/// agent to fix a check that did its job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MainChecks {
    Green,
    /// A run failed or timed out. Only these count as red: a run that was
    /// cancelled or never acquired a runner is GitHub capacity, not a
    /// failing test, and dispatching a fix for it wastes a session (seen
    /// repeatedly on macos-latest in #17, #22 and #5).
    Red,
    /// The newest run is still going, so there is nothing to conclude yet.
    Running,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub issues: Vec<Issue>,
    pub pulls: Vec<Pull>,
    pub main_checks: MainChecks,
    /// The Test run `main_checks` comes from, so a second failing run is a
    /// second piece of work rather than the one already dispatched.
    pub main_run: Option<u64>,
}

#[derive(Debug)]
pub enum Error {
    Gh { what: String, why: String },
    Json { what: String, why: String },
    Read { path: String, why: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Gh { what, why } => write!(f, "问 GitHub 要 {what} 失败：{why}"),
            Error::Json { what, why } => write!(f, "{what} 不是预期的格式：{why}"),
            Error::Read { path, why } => write!(f, "读 {path} 失败：{why}"),
        }
    }
}

impl std::error::Error for Error {}

// ---- the shapes of the GitHub responses the dispatcher reads ----

#[derive(Deserialize)]
struct ApiIssue {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    labels: Vec<ApiLabel>,
    /// Present on pull requests only, which the issues endpoint also
    /// returns; a pull request is not an issue to implement.
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ApiLabel {
    name: String,
}

#[derive(Deserialize)]
struct ApiComment {
    body: Option<String>,
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct ApiPull {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    labels: Vec<ApiLabel>,
    head: ApiHead,
}

#[derive(Deserialize)]
struct ApiHead {
    #[serde(default)]
    sha: String,
    #[serde(default, rename = "ref")]
    branch: String,
}

#[derive(Deserialize)]
struct ApiRuns {
    #[serde(default)]
    workflow_runs: Vec<ApiRun>,
}

#[derive(Deserialize)]
struct ApiRun {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
}

/// Every element of a `gh api --paginate` answer. `--paginate` prints one
/// independent JSON document per page rather than one merged array, so
/// reading the text as a single value works only while the answer fits in
/// one page: the first issue with more than 100 comments would otherwise
/// make the whole tick fail and nothing be dispatched at all.
fn pages<T: serde::de::DeserializeOwned>(what: &str, text: &str) -> Result<Vec<T>, Error> {
    if text.trim().is_empty() {
        return Err(Error::Json {
            what: what.to_string(),
            why: "什么都没返回".to_string(),
        });
    }
    let mut out = Vec::new();
    for page in serde_json::Deserializer::from_str(text).into_iter::<Vec<T>>() {
        out.extend(page.map_err(|e| Error::Json {
            what: what.to_string(),
            why: e.to_string(),
        })?);
    }
    Ok(out)
}

/// The same, for endpoints whose page is an object wrapping the list.
fn object_pages<T: serde::de::DeserializeOwned>(what: &str, text: &str) -> Result<Vec<T>, Error> {
    if text.trim().is_empty() {
        return Err(Error::Json {
            what: what.to_string(),
            why: "什么都没返回".to_string(),
        });
    }
    let mut out = Vec::new();
    for page in serde_json::Deserializer::from_str(text).into_iter::<T>() {
        out.push(page.map_err(|e| Error::Json {
            what: what.to_string(),
            why: e.to_string(),
        })?);
    }
    Ok(out)
}

fn issues_from_json(text: &str) -> Result<Vec<Issue>, Error> {
    let list: Vec<ApiIssue> = pages("issue 列表", text)?;
    let mut out: Vec<Issue> = list
        .into_iter()
        .filter(|i| i.pull_request.is_none())
        .map(|i| Issue {
            number: i.number,
            title: i.title,
            labels: i.labels.into_iter().map(|l| l.name).collect(),
            agent: None,
            last_comment_at: None,
        })
        .collect();
    out.sort_by_key(|i| i.number);
    Ok(out)
}

fn pulls_from_json(text: &str) -> Result<Vec<Pull>, Error> {
    let list: Vec<ApiPull> = pages("pull request 列表", text)?;
    let mut out: Vec<Pull> = list
        .into_iter()
        .map(|p| {
            let body = p.body.unwrap_or_default();
            Pull {
                number: p.number,
                labels: p.labels.into_iter().map(|l| l.name).collect(),
                head_sha: p.head.sha,
                head_branch: p.head.branch,
                agent: agent_signature(&body),
                issue: issue_in_title(&p.title),
                title: p.title,
            }
        })
        .collect();
    out.sort_by_key(|p| p.number);
    Ok(out)
}

/// What the Test runs on main say, newest first, and the id of the run
/// that said it. A run still going means "wait"; a cancelled run says
/// nothing, so the next older one answers. The id is what makes one red
/// main different from the next: a fix that was already dispatched for
/// run 38456076570 says nothing about the run that failed after it.
fn checks_from_json(text: &str) -> Result<(MainChecks, Option<u64>), Error> {
    let runs: Vec<ApiRuns> = object_pages("main 上 Test 的 run", text)?;
    for run in runs.iter().flat_map(|page| &page.workflow_runs) {
        let id = Some(run.id).filter(|id| *id != 0);
        if run.status != "completed" {
            return Ok((MainChecks::Running, id));
        }
        match run.conclusion.as_deref() {
            Some("failure" | "timed_out") => return Ok((MainChecks::Red, id)),
            Some("success" | "neutral") => return Ok((MainChecks::Green, id)),
            // cancelled, skipped, stale: nothing happened, look further back.
            _ => {}
        }
    }
    Ok((MainChecks::Unknown, None))
}

/// The agent named by an `Agent: <名字>` signature or a claim field
/// (`- agent: Claude`). Lower case, because the same agent signs itself
/// `Claude`, `claude` and `Claude（第 4 条线）`.
pub fn agent_signature(body: &str) -> Option<String> {
    let mut found = None;
    for line in body.lines() {
        let line = line.trim().trim_start_matches(['-', '*', '+', ' ']);
        let lower = line.to_lowercase();
        for prefix in ["agent:", "agent：", "agent ："] {
            if let Some(rest) = lower.strip_prefix(prefix) {
                let name: String = rest
                    .trim()
                    .trim_start_matches('`')
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                if !name.is_empty() {
                    found = Some(name);
                }
            }
        }
    }
    found
}

/// The issue a pull request title points at: `（#14）` or `(#14)`.
fn issue_in_title(title: &str) -> Option<u64> {
    let mut best = None;
    let bytes: Vec<char> = title.chars().collect();
    for (i, c) in bytes.iter().enumerate() {
        if *c != '#' {
            continue;
        }
        let digits: String = bytes[i + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() && best.is_none() {
            best = digits.parse().ok();
        }
    }
    best
}

// ---- replayed responses ----

impl Snapshot {
    /// A snapshot built from saved responses in `dir`: `issues.json`,
    /// `pulls.json`, `main-test-runs.json`, and a `comments-<号>.json`
    /// for each open issue. A missing comments file means the issue has
    /// no comments, which is what a freshly opened issue looks like.
    pub fn replay(dir: &Path) -> Result<Snapshot, Error> {
        let mut issues = issues_from_json(&read(dir, "issues.json")?)?;
        for issue in &mut issues {
            let Some(text) = optional(dir, &format!("comments-{}.json", issue.number)) else {
                continue;
            };
            apply_comments(issue, &text)?;
        }
        let (main_checks, main_run) = match optional(dir, "main-test-runs.json") {
            Some(text) => checks_from_json(&text)?,
            None => (MainChecks::Unknown, None),
        };
        Ok(Snapshot {
            issues,
            pulls: pulls_from_json(&read(dir, "pulls.json")?)?,
            main_checks,
            main_run,
        })
    }
}

fn apply_comments(issue: &mut Issue, text: &str) -> Result<(), Error> {
    let list: Vec<ApiComment> = pages("评论", text)?;
    let mut comments: Vec<(DateTime<Utc>, String)> = Vec::new();
    for c in list {
        let Some(created_at) = c.created_at.filter(|t| !t.is_empty()) else {
            continue;
        };
        let at = DateTime::parse_from_rfc3339(&created_at)
            .map(|t| t.with_timezone(&Utc))
            .map_err(|e| Error::Json {
                what: format!("评论的时间 {created_at}"),
                why: e.to_string(),
            })?;
        comments.push((at, c.body.unwrap_or_default()));
    }
    comments.sort_by_key(|(at, _)| *at);
    issue.last_comment_at = comments.last().map(|(at, _)| *at);
    issue.agent = implementer(&comments);
    Ok(())
}

/// The agent that wrote the change an issue is about. Two different
/// questions get mixed up here, and only this one decides who is sent
/// back to fix something and whose vendor a reviewer may not be:
///
/// - *Who is holding the lease right now* is the newest `## 认领` /
///   `## 心跳` comment. On an issue waiting for acceptance that is the
///   acceptor, and on one sent back it is the acceptor who just rejected
///   it. Reading it as the implementer sends the fix to the agent that
///   reviewed the work (seen on #22: CodeBuddy implemented, Codex
///   accepted and claimed while accepting).
/// - *Who implemented it* is the first claim of the current lease chain:
///   the agent that took the issue and kept it until someone reclaimed
///   it. A reclaim ends the chain, so the next agent to claim is the one
///   that took the work over and is its implementer from then on.
///
/// With no claim comment at all, the earliest `Agent:` signature answers:
/// the implementer reports before any reviewer or acceptor shows up.
pub(super) fn implementer(comments: &[(DateTime<Utc>, String)]) -> Option<String> {
    let mut held: Option<String> = None;
    for (_, body) in comments {
        match lease_comment(body) {
            Some(Lease::Held(agent)) if held.is_none() => held = Some(agent),
            Some(Lease::Reclaimed) => held = None,
            _ => {}
        }
    }
    held.or_else(|| comments.iter().find_map(|(_, body)| agent_signature(body)))
}

enum Lease {
    Held(String),
    Reclaimed,
}

/// What a claim, heartbeat or reclaim comment says (see the claim lease).
/// Anything else is not a lease comment.
fn lease_comment(body: &str) -> Option<Lease> {
    let first = body.lines().map(str::trim).find(|l| !l.is_empty())?;
    if first.contains("收回认领") {
        return Some(Lease::Reclaimed);
    }
    if first.contains("认领") || first.contains("心跳") {
        return agent_signature(body).map(Lease::Held);
    }
    None
}

fn read(dir: &Path, name: &str) -> Result<String, Error> {
    std::fs::read_to_string(dir.join(name)).map_err(|e| Error::Read {
        path: dir.join(name).display().to_string(),
        why: e.to_string(),
    })
}

fn optional(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name)).ok()
}

// ---- live responses ----

/// A snapshot of `repo`, read through the configured `gh` command, which
/// carries the login the owner already did.
pub fn fetch(gh_command: &[String], repo: &str, workflow: &str) -> Result<Snapshot, Error> {
    let mut issues = issues_from_json(&gh(
        gh_command,
        &[
            "api".to_string(),
            "--paginate".to_string(),
            format!("repos/{repo}/issues?state=open&per_page=100"),
        ],
    )?)?;
    for issue in &mut issues {
        let text = gh(
            gh_command,
            &[
                "api".to_string(),
                "--paginate".to_string(),
                format!("repos/{repo}/issues/{}/comments?per_page=100", issue.number),
            ],
        )?;
        apply_comments(issue, &text)?;
    }
    let pulls = pulls_from_json(&gh(
        gh_command,
        &[
            "api".to_string(),
            "--paginate".to_string(),
            format!("repos/{repo}/pulls?state=open&per_page=100"),
        ],
    )?)?;
    let (main_checks, main_run) = checks_from_json(&gh(
        gh_command,
        &[
            "api".to_string(),
            format!("repos/{repo}/actions/workflows/{workflow}/runs?branch=main&per_page=10"),
        ],
    )?)?;
    Ok(Snapshot {
        issues,
        pulls,
        main_checks,
        main_run,
    })
}

/// Posts `body` as a comment on issue or pull request `number`.
pub fn comment(gh_command: &[String], repo: &str, number: u64, body: &str) -> Result<(), Error> {
    let dir = std::env::temp_dir().join(format!("dispatch-{number}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| Error::Read {
        path: dir.display().to_string(),
        why: e.to_string(),
    })?;
    let file = dir.join("body.md");
    std::fs::write(&file, body).map_err(|e| Error::Read {
        path: file.display().to_string(),
        why: e.to_string(),
    })?;
    let out = gh(
        gh_command,
        &[
            "issue".to_string(),
            "comment".to_string(),
            number.to_string(),
            "--repo".to_string(),
            repo.to_string(),
            "--body-file".to_string(),
            file.display().to_string(),
        ],
    );
    let _ = std::fs::remove_dir_all(&dir);
    out.map(|_| ())
}

fn gh(gh_command: &[String], args: &[String]) -> Result<String, Error> {
    let (program, fixed) = gh_command.split_first().ok_or_else(|| Error::Gh {
        what: args.join(" "),
        why: "gh_command 是空的".to_string(),
    })?;
    let out = Command::new(program)
        .args(fixed)
        .args(args)
        .output()
        .map_err(|e| Error::Gh {
            what: args.join(" "),
            why: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(Error::Gh {
            what: args.join(" "),
            why: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}
