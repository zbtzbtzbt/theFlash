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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub url: String,
    pub posted_at: DateTime<Utc>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// The open issues labelled `认领中`, lowest number first.
    pub issues: Vec<Issue>,
    /// When each branch named by a claim was last pushed to. A branch
    /// that does not exist yet is simply absent, which makes its pushes
    /// count for nothing rather than making the claim unreadable.
    pub pushes: BTreeMap<String, DateTime<Utc>>,
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
    body: Option<String>,
    html_url: Option<String>,
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct ApiCommit {
    commit: ApiCommitDetail,
}

#[derive(Deserialize)]
struct ApiCommitDetail {
    committer: ApiCommitPerson,
}

#[derive(Deserialize)]
struct ApiCommitPerson {
    date: String,
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
            url: c.html_url.unwrap_or_default(),
            posted_at: parse_time(what, &created_at)?,
            body: c.body.unwrap_or_default(),
        });
    }
    out.sort_by_key(|c| c.posted_at);
    Ok(out)
}

/// The time of the newest commit in a `commits` response.
fn last_push_from_json(what: &str, text: &str) -> Result<Option<DateTime<Utc>>, Error> {
    let list: Vec<ApiCommit> = from_json(what, text)?;
    match list.first() {
        Some(c) => Ok(Some(parse_time(what, &c.commit.committer.date)?)),
        None => Ok(None),
    }
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
            if let Some(at) = last_push_from_json("分支的提交", &text)? {
                snapshot.pushes.insert(branch, at);
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

    /// Every branch a claim comment names, so their pushes can be read.
    fn claimed_branches(&self) -> Vec<String> {
        let mut out = Vec::new();
        for issue in &self.issues {
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
            if let Some(at) = fetch_last_push(repo, &branch)? {
                snapshot.pushes.insert(branch, at);
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
fn fetch_last_push(repo: &str, branch: &str) -> Result<Option<DateTime<Utc>>, Error> {
    let path = format!(
        "repos/{repo}/commits?sha={}&per_page=1",
        encode(branch.trim())
    );
    match gh_api(&path) {
        Ok(text) => last_push_from_json("分支的提交", &text),
        Err(Error::Gh { why, .. }) if why.contains("404") || why.contains("Not Found") => Ok(None),
        Err(e) => Err(e),
    }
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
