//! 认领看板看到的 GitHub 数据：一次取齐，再做判断。
//!
//! Everything the board reports on is in a [`Snapshot`]: the open issues
//! labelled `认领中`, their comments, and when each claimed branch was
//! last pushed. A snapshot comes either from `gh api` or from a directory
//! of saved responses, so a test replays the same bytes GitHub returned
//! and never reaches the network.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::lease::{self, CLAIM_LABEL};

/// An issue as the board needs it: what it is called, its labels, and
/// every comment on it, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    pub comments: Vec<Comment>,
}

/// An open issue as the priority queue needs it: its labels say its
/// priority and state, and its body may say which issues must close
/// first (#68). Comments are not read, so listing every open issue
/// costs one request per hundred issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenIssue {
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// GitHub's id, which rises with every comment. Two comments posted
    /// in the same second are ordered by it, and the lease protocol rests
    /// on knowing which of two claims came first (#16).
    pub id: u64,
    pub url: String,
    pub posted_at: DateTime<Utc>,
    pub body: String,
}

/// When a branch was last pushed to, or why that cannot be read. Pushing
/// counts as a heartbeat, so a push time nobody can read is not the same
/// as a branch that was never pushed: the first means the lease cannot be
/// judged, the second means the lease lives by its comments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Push {
    At(DateTime<Utc>),
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// The open issues labelled `认领中`, lowest number first.
    pub issues: Vec<Issue>,
    /// When each branch named by a claim was last pushed to. A branch
    /// that does not exist yet is simply absent, which makes its pushes
    /// count for nothing rather than making the claim unreadable.
    pub pushes: BTreeMap<String, Push>,
}

#[derive(Debug)]
pub enum Error {
    /// `gh` could not be run, or answered with an error.
    Gh {
        what: String,
        why: String,
    },
    /// A saved or live response was not the shape the board expects.
    Json {
        what: String,
        why: String,
    },
    Read {
        path: String,
        why: String,
    },
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

// ---- the shapes of the GitHub responses the board reads ----

#[derive(Deserialize)]
struct ApiIssue {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    labels: Vec<ApiLabel>,
    /// GitHub sends `null` for an issue opened with no description.
    #[serde(default)]
    body: Option<String>,
    /// Present on pull requests only; a pull request is not an issue the
    /// board claims.
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ApiLabel {
    name: String,
}

#[derive(Deserialize)]
struct ApiComment {
    #[serde(default)]
    id: u64,
    body: Option<String>,
    html_url: Option<String>,
    created_at: Option<String>,
}

/// One entry of `repos/<仓库>/activity`: what happened to a ref and when.
/// `timestamp` is when GitHub received it, which is the push time; a
/// commit's own date is when it was made, possibly hours earlier and
/// offline.
#[derive(Deserialize)]
struct ApiActivity {
    #[serde(default)]
    activity_type: String,
    timestamp: Option<String>,
}

/// GitHub's error body, which `activity` answers with when the login has
/// no push access to the repository.
#[derive(Deserialize)]
struct ApiError {
    message: String,
}

fn from_json<T: serde::de::DeserializeOwned>(what: &str, text: &str) -> Result<T, Error> {
    serde_json::from_str(text).map_err(|e| Error::Json {
        what: what.to_string(),
        why: e.to_string(),
    })
}

fn parse_time(what: &str, s: &str) -> Result<DateTime<Utc>, Error> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| Error::Json {
            what: format!("{what} 的时间 {s}"),
            why: e.to_string(),
        })
}

fn issues_from_json(text: &str) -> Result<Vec<Issue>, Error> {
    let list: Vec<ApiIssue> = from_json("issue 列表", text)?;
    Ok(list
        .into_iter()
        .filter(|i| i.pull_request.is_none())
        .map(issue_from_api)
        .collect())
}

fn open_issues_from_json(text: &str) -> Result<Vec<OpenIssue>, Error> {
    let list: Vec<ApiIssue> = from_json("打开的 issue 列表", text)?;
    let mut out: Vec<OpenIssue> = list
        .into_iter()
        .filter(|i| i.pull_request.is_none())
        .map(|i| OpenIssue {
            number: i.number,
            title: i.title,
            labels: i.labels.into_iter().map(|l| l.name).collect(),
            body: i.body.unwrap_or_default(),
        })
        .collect();
    out.sort_by_key(|i| i.number);
    Ok(out)
}

fn issue_from_json(text: &str) -> Result<Issue, Error> {
    Ok(issue_from_api(from_json("issue", text)?))
}

fn issue_from_api(i: ApiIssue) -> Issue {
    Issue {
        number: i.number,
        title: i.title,
        labels: i.labels.into_iter().map(|l| l.name).collect(),
        comments: Vec::new(),
    }
}

fn comments_from_json(what: &str, text: &str) -> Result<Vec<Comment>, Error> {
    let list: Vec<ApiComment> = from_json(what, text)?;
    let mut out = Vec::new();
    for c in list {
        let Some(created_at) = c.created_at.filter(|t| !t.is_empty()) else {
            continue;
        };
        out.push(Comment {
            id: c.id,
            url: c.html_url.unwrap_or_default(),
            posted_at: parse_time(what, &created_at)?,
            body: c.body.unwrap_or_default(),
        });
    }
    out.sort_by_key(|c| (c.posted_at, c.id));
    Ok(out)
}

/// The activity types that move a branch's head, so an agent that pushed
/// is an agent that is still working. A merge of someone else's pull
/// request into the branch is not the holder reporting in.
const PUSH_TYPES: [&str; 3] = ["push", "force_push", "branch_creation"];

/// When a branch was last pushed to, from an `activity` response: the
/// newest entry that is a push. GitHub answers newest first; this does
/// not rely on that.
fn last_push_from_json(what: &str, text: &str) -> Result<Option<Push>, Error> {
    if let Ok(error) = serde_json::from_str::<ApiError>(text) {
        return Ok(Some(Push::Unreadable(error.message)));
    }
    let list: Vec<ApiActivity> = from_json(what, text)?;
    let mut last: Option<DateTime<Utc>> = None;
    for entry in list
        .iter()
        .filter(|e| PUSH_TYPES.contains(&e.activity_type.as_str()))
    {
        let Some(stamp) = entry.timestamp.as_deref().filter(|t| !t.is_empty()) else {
            continue;
        };
        let at = parse_time(what, stamp)?;
        if last.is_none_or(|known| at > known) {
            last = Some(at);
        }
    }
    Ok(last.map(Push::At))
}

// ---- replayed responses ----

impl Snapshot {
    /// A snapshot built from saved responses in `dir`: `issues.json`, a
    /// `comments-<号>.json` for each issue in it, and a
    /// `branch-<分支名，斜杠换成横线>.json` for each branch a claim names.
    /// The comments file must be there, because an issue whose comments
    /// silently read as empty would look like a claim nobody ever made.
    pub fn replay(dir: &Path) -> Result<Snapshot, Error> {
        let mut issues = issues_from_json(&read(dir, "issues.json")?)?;
        for issue in &mut issues {
            issue.comments = comments_from_json(
                "评论",
                &read(dir, &format!("comments-{}.json", issue.number))?,
            )?;
        }
        let mut snapshot = Snapshot {
            issues,
            pushes: BTreeMap::new(),
        };
        for branch in snapshot.claimed_branches() {
            let Some(text) = optional(dir, &format!("branch-{}.json", slug(&branch))) else {
                continue;
            };
            if let Some(push) = last_push_from_json("分支的推送活动", &text)? {
                snapshot.pushes.insert(branch, push);
            }
        }
        Ok(snapshot)
    }

    /// One issue and its comments from saved responses, for the paths
    /// that act on a single issue: `issue-<号>.json` and
    /// `comments-<号>.json`.
    pub fn replay_issue(dir: &Path, number: u64) -> Result<Issue, Error> {
        let mut issue = issue_from_json(&read(dir, &format!("issue-{number}.json"))?)?;
        issue.comments =
            comments_from_json("评论", &read(dir, &format!("comments-{number}.json"))?)?;
        Ok(issue)
    }

    /// Every open issue from `open-issues.json`, the saved answer to
    /// `issues?state=open`. It must be there: a queue read from a missing
    /// file would say there is nothing to do.
    pub fn replay_open_issues(dir: &Path) -> Result<Vec<OpenIssue>, Error> {
        open_issues_from_json(&read(dir, "open-issues.json")?)
    }

    /// Branch evidence named by a separately loaded target issue.
    pub fn replay_pushes_for_issue(
        dir: &Path,
        issue: &Issue,
    ) -> Result<BTreeMap<String, Push>, Error> {
        let mut pushes = BTreeMap::new();
        for branch in claimed_branches(std::slice::from_ref(issue)) {
            let Some(text) = optional(dir, &format!("branch-{}.json", slug(&branch))) else {
                continue;
            };
            if let Some(push) = last_push_from_json("分支的推送活动", &text)? {
                pushes.insert(branch, push);
            }
        }
        Ok(pushes)
    }

    /// Every branch a claim comment names, so their pushes can be read.
    fn claimed_branches(&self) -> Vec<String> {
        claimed_branches(&self.issues)
    }
}

fn claimed_branches(issues: &[Issue]) -> Vec<String> {
    let mut out = Vec::new();
    for issue in issues {
        for comment in &issue.comments {
            if let Some(branch) =
                lease::parse_claim(&comment.body, "", comment.posted_at).and_then(|c| c.branch)
                && !out.contains(&branch)
            {
                out.push(branch);
            }
        }
    }
    out
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

/// A branch name as a file name: `night/issue-16` saves as
/// `branch-night-issue-16.json`.
pub fn slug(branch: &str) -> String {
    branch.replace('/', "-")
}

// ---- live responses ----

/// `gh api <path>`, which carries the login the owner already did.
fn gh_api(path: &str) -> Result<String, Error> {
    gh(&["api", path])
}

/// A list endpoint, with every page, so an issue with more comments than
/// fit on one page does not look like it has no heartbeat.
fn gh_api_list(path: &str) -> Result<String, Error> {
    gh(&["api", "--paginate", path])
}

fn gh(args: &[&str]) -> Result<String, Error> {
    let out = Command::new("gh")
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

/// Percent-encodes a query value, so a label written in Chinese reaches
/// GitHub as the label it is.
fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Snapshot {
    /// A snapshot of the claims in `repo` (`owner/name`), read through
    /// `gh`: the open issues labelled `认领中`, their comments, and the
    /// last push to each branch a claim names.
    pub fn fetch(repo: &str) -> Result<Snapshot, Error> {
        let mut issues = issues_from_json(&gh_api_list(&format!(
            "repos/{repo}/issues?state=open&labels={}&per_page=100",
            encode(CLAIM_LABEL)
        ))?)?;
        issues.sort_by_key(|i| i.number);
        for issue in &mut issues {
            issue.comments = fetch_comments(repo, issue.number)?;
        }
        let mut snapshot = Snapshot {
            issues,
            pushes: BTreeMap::new(),
        };
        for branch in snapshot.claimed_branches() {
            if let Some(push) = fetch_last_push(repo, &branch)? {
                snapshot.pushes.insert(branch, push);
            }
        }
        Ok(snapshot)
    }

    /// One issue and its comments, for the paths that act on a single
    /// issue, including an issue that carries no claim label yet.
    pub fn fetch_issue(repo: &str, number: u64) -> Result<Issue, Error> {
        let mut issue = issue_from_json(&gh_api(&format!("repos/{repo}/issues/{number}"))?)?;
        issue.comments = fetch_comments(repo, number)?;
        Ok(issue)
    }

    /// Every open issue in `repo`, with every page, pull requests left out.
    pub fn fetch_open_issues(repo: &str) -> Result<Vec<OpenIssue>, Error> {
        open_issues_from_json(&gh_api_list(&format!(
            "repos/{repo}/issues?state=open&per_page=100"
        ))?)
    }

    /// Branch evidence named by a separately loaded target issue.
    pub fn fetch_pushes_for_issue(
        repo: &str,
        issue: &Issue,
    ) -> Result<BTreeMap<String, Push>, Error> {
        let mut pushes = BTreeMap::new();
        for branch in claimed_branches(std::slice::from_ref(issue)) {
            if let Some(push) = fetch_last_push(repo, &branch)? {
                pushes.insert(branch, push);
            }
        }
        Ok(pushes)
    }
}

fn fetch_comments(repo: &str, number: u64) -> Result<Vec<Comment>, Error> {
    comments_from_json(
        "评论",
        &gh_api_list(&format!(
            "repos/{repo}/issues/{number}/comments?per_page=100"
        ))?,
    )
}

/// When `branch` was last pushed to, or `None` when there is no such
/// branch: an agent that claimed an issue before pushing anything is
/// alive by its comments, not by a branch that does not exist.
///
/// The repository's activity is the only place GitHub says when a push
/// happened. A commit's `committer.date` is when the commit was made, so
/// a commit written offline at 03:10 and pushed at 05:10 would look like
/// two hours of silence and get the issue taken away; reading activity
/// needs push access, and without it the push time is reported as
/// unreadable rather than guessed from the commit.
fn fetch_last_push(repo: &str, branch: &str) -> Result<Option<Push>, Error> {
    let path = format!(
        "repos/{repo}/activity?ref=refs/heads/{}&per_page=100",
        encode(branch.trim())
    );
    match gh_api_list(&path) {
        Ok(text) => last_push_from_json("分支的推送活动", &text),
        Err(Error::Gh { why, .. }) if why.contains("404") || why.contains("Not Found") => Ok(None),
        Err(Error::Gh { why, .. }) if is_permission_error(&why) => Ok(Some(Push::Unreadable(why))),
        Err(e) => Err(e),
    }
}

fn is_permission_error(why: &str) -> bool {
    let lower = why.to_lowercase();
    lower.contains("403")
        || lower.contains("push access")
        || lower.contains("forbidden")
        || lower.contains("resource not accessible")
        || lower.contains("insufficient")
}

// ---- what the board writes back ----

/// Posts `body` as a comment on issue `number` and returns its link.
pub fn comment(repo: &str, number: u64, body: &str) -> Result<String, Error> {
    let dir = std::env::temp_dir().join(format!("claims-{number}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| Error::Read {
        path: dir.display().to_string(),
        why: e.to_string(),
    })?;
    let body_file = dir.join("body.md");
    std::fs::write(&body_file, body).map_err(|e| Error::Read {
        path: body_file.display().to_string(),
        why: e.to_string(),
    })?;
    let out = gh(&[
        "issue",
        "comment",
        &number.to_string(),
        "--repo",
        repo,
        "--body-file",
        &body_file.display().to_string(),
    ]);
    let _ = std::fs::remove_dir_all(&dir);
    out
}

pub fn add_label(repo: &str, number: u64, label: &str) -> Result<String, Error> {
    gh(&[
        "issue",
        "edit",
        &number.to_string(),
        "--repo",
        repo,
        "--add-label",
        label,
    ])
}

/// Adds and removes labels in one edit, so an issue is never left with
/// two priorities or none between two calls.
pub fn edit_labels(
    repo: &str,
    number: u64,
    add: &[&str],
    remove: &[&str],
) -> Result<String, Error> {
    let number = number.to_string();
    let mut args = vec!["issue", "edit", number.as_str(), "--repo", repo];
    for label in add {
        args.extend(["--add-label", label]);
    }
    for label in remove {
        args.extend(["--remove-label", label]);
    }
    gh(&args)
}

pub fn remove_label(repo: &str, number: u64, label: &str) -> Result<String, Error> {
    gh(&[
        "issue",
        "edit",
        &number.to_string(),
        "--repo",
        repo,
        "--remove-label",
        label,
    ])
}
