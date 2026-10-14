//! 合并闸门看到的 GitHub 数据：一次取齐，再做决定。
//!
//! Everything the gate decides on is in a [`Snapshot`]: the pull request,
//! its commits and files, the check runs on its head, where main is now,
//! and the review records posted on it. A snapshot comes either from
//! `gh api` or from a directory of saved responses, so a test replays the
//! same bytes GitHub returned and never reaches the network.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::record::{self, ReviewRecord};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pull {
    pub number: u64,
    pub state: String,
    pub draft: bool,
    pub head_sha: String,
    pub base_ref: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    /// `None` is GitHub still computing it, not "cannot merge".
    pub mergeable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub committed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checks {
    pub head_sha: String,
    pub runs: Vec<CheckRun>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub pull: Pull,
    /// Oldest first, as GitHub lists them.
    pub commits: Vec<Commit>,
    pub checks: Checks,
    pub main_head: Commit,
    pub records: Vec<ReviewRecord>,
}

#[derive(Debug)]
pub enum Error {
    /// `gh` could not be run, or answered with an error.
    Gh {
        what: String,
        why: String,
    },
    /// A saved or live response was not the shape the gate expects.
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

// ---- the shapes of the GitHub responses the gate reads ----

#[derive(Deserialize)]
struct ApiPull {
    number: u64,
    state: String,
    #[serde(default)]
    draft: bool,
    head: ApiRef,
    base: ApiRef,
    #[serde(default)]
    title: String,
    /// GitHub sends `null` for an empty body.
    body: Option<String>,
    #[serde(default)]
    labels: Vec<ApiLabel>,
    #[serde(default)]
    mergeable: Option<bool>,
}

#[derive(Deserialize)]
struct ApiRef {
    sha: String,
    #[serde(rename = "ref")]
    ref_name: String,
}

#[derive(Deserialize)]
struct ApiLabel {
    name: String,
}

#[derive(Deserialize)]
struct ApiCommit {
    sha: String,
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

#[derive(Deserialize)]
struct ApiCheckRuns {
    #[serde(default)]
    check_runs: Vec<ApiCheckRun>,
}

#[derive(Deserialize)]
struct ApiCheckRun {
    name: String,
    status: String,
    conclusion: Option<String>,
    #[serde(default)]
    head_sha: String,
}

/// An issue comment and a pull request review are the same thing to the
/// gate: a body, a time and a link.
#[derive(Deserialize)]
struct ApiComment {
    body: Option<String>,
    html_url: Option<String>,
    created_at: Option<String>,
    submitted_at: Option<String>,
}

fn parse_time(what: &str, s: &str) -> Result<DateTime<Utc>, Error> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| Error::Json {
            what: format!("{what} 的时间 {s}"),
            why: e.to_string(),
        })
}

fn from_json<T: serde::de::DeserializeOwned>(what: &str, text: &str) -> Result<T, Error> {
    serde_json::from_str(text).map_err(|e| Error::Json {
        what: what.to_string(),
        why: e.to_string(),
    })
}

/// Every page of an answer that came back in pages.
///
/// `gh api --paginate` prints each page as its own JSON value, one after
/// another (`gh api --help`), so an answer that did not fit on one page is
/// several arrays or objects back to back and not one array. Reading it as
/// a single value fails with "trailing characters", and reading only up to
/// the first value would silently lose the rest — a review record on the
/// second page of comments would look like no review record at all.
pub(super) fn pages<T: serde::de::DeserializeOwned>(
    what: &str,
    text: &str,
) -> Result<Vec<T>, Error> {
    serde_json::Deserializer::from_str(text)
        .into_iter::<T>()
        .collect::<Result<Vec<T>, _>>()
        .map_err(|e| Error::Json {
            what: what.to_string(),
            why: e.to_string(),
        })
}

/// The items of a paginated list endpoint, pages flattened in order.
pub(super) fn list_from_json<T: serde::de::DeserializeOwned>(
    what: &str,
    text: &str,
) -> Result<Vec<T>, Error> {
    Ok(pages::<Vec<T>>(what, text)?.into_iter().flatten().collect())
}

fn pull_from_json(text: &str) -> Result<Pull, Error> {
    let p: ApiPull = from_json("PR", text)?;
    Ok(Pull {
        number: p.number,
        state: p.state,
        draft: p.draft,
        head_sha: p.head.sha,
        base_ref: p.base.ref_name,
        title: p.title,
        body: p.body.unwrap_or_default(),
        labels: p.labels.into_iter().map(|l| l.name).collect(),
        mergeable: p.mergeable,
    })
}

fn commits_from_json(what: &str, text: &str) -> Result<Vec<Commit>, Error> {
    let list: Vec<ApiCommit> = list_from_json(what, text)?;
    list.into_iter()
        .map(|c| {
            Ok(Commit {
                committed_at: parse_time(&c.sha, &c.commit.committer.date)?,
                sha: c.sha,
            })
        })
        .collect()
}

fn commit_from_json(what: &str, text: &str) -> Result<Commit, Error> {
    let c: ApiCommit = from_json(what, text)?;
    Ok(Commit {
        committed_at: parse_time(&c.sha, &c.commit.committer.date)?,
        sha: c.sha,
    })
}

fn checks_from_json(head_sha: &str, text: &str) -> Result<Checks, Error> {
    let runs: Vec<ApiCheckRun> = pages::<ApiCheckRuns>("check-runs", text)?
        .into_iter()
        .flat_map(|page| page.check_runs)
        .collect();
    let sha = runs
        .first()
        .map(|r| r.head_sha.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| head_sha.to_string());
    Ok(Checks {
        head_sha: sha,
        runs: runs
            .into_iter()
            .map(|r| CheckRun {
                name: r.name,
                status: r.status,
                conclusion: r.conclusion,
            })
            .collect(),
    })
}

/// The review records among a list of comments or reviews.
fn records_from_json(what: &str, text: &str) -> Result<Vec<ReviewRecord>, Error> {
    let list: Vec<ApiComment> = list_from_json(what, text)?;
    let mut out = Vec::new();
    for c in list {
        let body = c.body.unwrap_or_default();
        let url = c.html_url.unwrap_or_default();
        let time = c.created_at.or(c.submitted_at).unwrap_or_default();
        if time.is_empty() {
            continue;
        }
        if let Some(r) = record::parse(&body, &url, parse_time(what, &time)?) {
            out.push(r);
        }
    }
    Ok(out)
}

// ---- replayed responses ----

impl Snapshot {
    /// A snapshot built from saved responses in `dir`:
    /// `pull.json`, `commits.json`, `check-runs.json`, `main.json`,
    /// `comments.json` and `reviews.json`. Only the last is optional;
    /// everything else must be there, because a gate that silently treats
    /// a missing file as "nothing to see" would merge on no evidence.
    /// Each file holds what `gh api` printed, so a file may hold several
    /// pages back to back the way `--paginate` prints them.
    pub fn replay(dir: &Path) -> Result<Snapshot, Error> {
        let read = |name: &str| -> Result<String, Error> {
            std::fs::read_to_string(dir.join(name)).map_err(|e| Error::Read {
                path: dir.join(name).display().to_string(),
                why: e.to_string(),
            })
        };
        let optional =
            |name: &str| -> Option<String> { std::fs::read_to_string(dir.join(name)).ok() };
        let pull = pull_from_json(&read("pull.json")?)?;
        let mut records = records_from_json("comments", &read("comments.json")?)?;
        if let Some(text) = optional("reviews.json") {
            records.extend(records_from_json("reviews", &text)?);
        }
        Ok(Snapshot {
            commits: commits_from_json("PR 的提交", &read("commits.json")?)?,
            checks: checks_from_json(&pull.head_sha, &read("check-runs.json")?)?,
            main_head: commit_from_json("main", &read("main.json")?)?,
            pull,
            records,
        })
    }
}

// ---- live responses ----

/// `gh api <path>`, which carries the login the owner already did.
fn gh_api(path: &str) -> Result<String, Error> {
    gh(&["api", path])
}

/// A list endpoint, with every page, so a pull request with more comments
/// than fit on one page does not look like it has no review record.
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

impl Snapshot {
    /// A snapshot of pull request `number` in `repo` (`owner/name`), read
    /// through `gh`.
    pub fn fetch(repo: &str, number: u64) -> Result<Snapshot, Error> {
        let pull = pull_from_json(&gh_api(&format!("repos/{repo}/pulls/{number}"))?)?;
        let commits = commits_from_json(
            "PR 的提交",
            &gh_api_list(&format!("repos/{repo}/pulls/{number}/commits"))?,
        )?;
        let checks = checks_from_json(
            &pull.head_sha,
            &gh_api_list(&format!(
                "repos/{repo}/commits/{}/check-runs",
                pull.head_sha
            ))?,
        )?;
        let main_head = commit_from_json(
            "main",
            &gh_api(&format!("repos/{repo}/commits/{}", pull.base_ref))?,
        )?;
        let mut records = records_from_json(
            "comments",
            &gh_api_list(&format!("repos/{repo}/issues/{number}/comments"))?,
        )?;
        records.extend(records_from_json(
            "reviews",
            &gh_api_list(&format!("repos/{repo}/pulls/{number}/reviews"))?,
        )?);
        Ok(Snapshot {
            pull,
            commits,
            checks,
            main_head,
            records,
        })
    }

    /// The numbers of the open pull requests in `repo`, lowest first, so
    /// the queue merges in the order the work was proposed.
    pub fn open_pulls(repo: &str) -> Result<Vec<u64>, Error> {
        #[derive(Deserialize)]
        struct Item {
            number: u64,
        }
        let list: Vec<Item> = list_from_json(
            "open PRs",
            &gh_api_list(&format!("repos/{repo}/pulls?state=open"))?,
        )?;
        let numbers: BTreeSet<u64> = list.into_iter().map(|i| i.number).collect();
        Ok(numbers.into_iter().collect())
    }
}

/// Squash-merges `sha` of pull request `number`, refusing if the head
/// moved in the meantime (`--match-head-commit`), with `body` as the
/// commit message body.
pub fn merge(
    repo: &str,
    number: u64,
    sha: &str,
    subject: &str,
    body: &str,
) -> Result<String, Error> {
    let dir = std::env::temp_dir().join(format!("merge-gate-{number}-{sha}"));
    std::fs::create_dir_all(&dir).map_err(|e| Error::Read {
        path: dir.display().to_string(),
        why: e.to_string(),
    })?;
    let body_file = dir.join("body.txt");
    std::fs::write(&body_file, body).map_err(|e| Error::Read {
        path: body_file.display().to_string(),
        why: e.to_string(),
    })?;
    let out = gh(&[
        "pr",
        "merge",
        &number.to_string(),
        "--repo",
        repo,
        "--squash",
        "--match-head-commit",
        sha,
        "--subject",
        subject,
        "--body-file",
        &body_file.display().to_string(),
    ]);
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// Posts `body` as a comment on pull request `number`, so a refusal the
/// queue hit is on the pull request and not only in one agent's terminal.
pub fn comment(repo: &str, number: u64, body: &str) -> Result<String, Error> {
    gh(&[
        "pr",
        "comment",
        &number.to_string(),
        "--repo",
        repo,
        "--body",
        body,
    ])
}

/// Marks `issue` as ready for acceptance (`待验收`) and clears any old
/// failed-acceptance label (`验收未通过`) after its pull request merges (#102).
pub fn hand_off_issue_labels(
    repo: &str,
    issue: u64,
    add_label: &str,
    remove_label: &str,
) -> Result<String, Error> {
    gh(&[
        "issue",
        "edit",
        &issue.to_string(),
        "--repo",
        repo,
        "--add-label",
        add_label,
        "--remove-label",
        remove_label,
    ])
}

/// Reads the squash commit SHA GitHub recorded on pull request `number`
/// after `gh pr merge` completed (#102).
pub fn merged_commit_sha(repo: &str, number: u64) -> Result<String, Error> {
    #[derive(Deserialize)]
    struct MergedPull {
        merge_commit_sha: Option<String>,
    }
    let text = gh_api(&format!("repos/{repo}/pulls/{number}"))?;
    let p: MergedPull = from_json("PR 合入提交", &text)?;
    p.merge_commit_sha
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Json {
            what: format!("PR #{number} 的 merge_commit_sha"),
            why: "为空或 null".to_string(),
        })
}

/// Posts `body` as a comment on `issue` after its pull request merges (#102).
pub fn comment_issue(repo: &str, issue: u64, body: &str) -> Result<String, Error> {
    gh(&[
        "issue",
        "comment",
        &issue.to_string(),
        "--repo",
        repo,
        "--body",
        body,
    ])
}
