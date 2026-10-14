//! Where an inspection's findings go: a GitHub issue, or a comment on the
//! open issue a previous run already made. Nothing else in theFlash talks
//! to GitHub, so this is deliberately one small interface: the inspection
//! takes a `Tracker`, tests pass one that replays saved `gh` output, and no
//! automated test reaches GitHub.

use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

pub trait Tracker {
    fn open_issues(&self) -> Result<Vec<Issue>, String>;
    /// The new issue's URL.
    fn create(&self, title: &str, body: &str) -> Result<String, String>;
    /// The new comment's URL.
    fn comment(&self, number: u64, body: &str) -> Result<String, String>;
}

/// What [`post`] did, so the run can say where its findings went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Posted {
    Created { number: Option<u64>, url: String },
    Commented { number: u64, url: String },
}

impl std::fmt::Display for Posted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Posted::Created { url, .. } => write!(f, "开了新 issue：{url}"),
            Posted::Commented { number, url } => {
                write!(f, "在已有的 issue #{number} 上追加了评论：{url}")
            }
        }
    }
}

/// The line that makes an issue this inspection's own. A run looks for it
/// in the open issues and comments there instead of opening a second one;
/// a human-written issue about the same change is not taken over.
pub const MARKER: &str = "<!-- theflash-inspect: damai-structure -->";

/// Puts `body` where the next agent will see it: a comment on the open
/// inspection issue, or a new issue when there is none.
pub fn post(tracker: &dyn Tracker, title: &str, body: &str) -> Result<Posted, String> {
    let body = format!("{}\n\n{MARKER}\n", body.trim_end());
    match tracker
        .open_issues()?
        .into_iter()
        .find(|i| i.body.contains(MARKER))
    {
        Some(issue) => Ok(Posted::Commented {
            number: issue.number,
            url: tracker.comment(issue.number, &body)?,
        }),
        None => {
            let url = tracker.create(title, &body)?.trim().to_string();
            Ok(Posted::Created {
                number: url
                    .rsplit('/')
                    .next()
                    .and_then(|n| n.trim().parse::<u64>().ok()),
                url,
            })
        }
    }
}

type Runner = Box<dyn Fn(&[&str]) -> Result<String, String> + Send + Sync>;

/// GitHub through the `gh` command, which already holds this machine's
/// login. `gh` is not a dependency of theFlash: when it is missing, the
/// inspection says so and still prints its report.
pub struct Gh {
    repo: Option<String>,
    run: Runner,
}

impl Gh {
    pub fn new(repo: Option<String>) -> Gh {
        Gh {
            repo,
            run: Box::new(gh),
        }
    }

    /// With `gh` replaced, which is how tests check the argv and the
    /// parsing without reaching GitHub.
    pub fn with_runner(repo: Option<String>, run: Runner) -> Gh {
        Gh { repo, run }
    }

    fn call(&self, args: &[&str]) -> Result<String, String> {
        let mut full: Vec<&str> = args.to_vec();
        if let Some(repo) = &self.repo {
            full.push("--repo");
            full.push(repo);
        }
        (self.run)(&full)
    }

    fn list_open_issues(&self, limit: usize) -> Result<Vec<Issue>, String> {
        let limit = limit.to_string();
        let out = self.call(&[
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            &limit,
            "--json",
            "number,title,body",
        ])?;
        serde_json::from_str(&out).map_err(|e| format!("看不懂 gh issue list 的输出：{e}"))
    }
}

/// What the first `issue list` asks for. GitHub's own page size, and more
/// open issues than this repository has had on any day so far, so one call
/// is normally the whole answer.
const FIRST_LIMIT: usize = 100;

/// Where asking for a longer list stops being worth it. A repository with
/// this many open issues says nothing about theFlash; the ceiling is here
/// so a `gh` that keeps answering with a full list cannot keep this loop
/// going forever.
const LIMIT_CEILING: usize = 51_200;

impl Tracker for Gh {
    /// Every open issue, which takes more than one call once the
    /// repository has more of them than a list can hold: `gh issue list`
    /// answers with at most `--limit` issues and says nothing about what
    /// it left out, so a list exactly that long may be cut off. The
    /// inspection's own issue is the oldest thing it is looking for — by
    /// the time a repository has a hundred open issues, that issue has
    /// long dropped off the first page — so a run that reads one page
    /// takes "not in this page" for "nobody has opened one" and opens a
    /// second one, every day (#76). Asking again for twice as many stops
    /// as soon as a list comes back short, which is the only answer that
    /// means "this is all of them".
    fn open_issues(&self) -> Result<Vec<Issue>, String> {
        let mut limit = FIRST_LIMIT;
        loop {
            let issues = self.list_open_issues(limit)?;
            if issues.len() < limit {
                return Ok(issues);
            }
            if limit >= LIMIT_CEILING {
                return Err(format!(
                    "这个仓库的 open issue 超过 {limit} 条，读不完就无法确认巡检自己的 issue 在不在，\
                     不开新 issue 以免重复：先关掉一些 open issue，或手工找到带 {MARKER} 的那个"
                ));
            }
            limit *= 2;
        }
    }

    fn create(&self, title: &str, body: &str) -> Result<String, String> {
        let out = self.call(&["issue", "create", "--title", title, "--body", body])?;
        Ok(url_in(&out))
    }

    fn comment(&self, number: u64, body: &str) -> Result<String, String> {
        let number = number.to_string();
        let out = self.call(&["issue", "comment", &number, "--body", body])?;
        Ok(url_in(&out))
    }
}

/// `gh` prints the URL it made on its own line, sometimes after a notice.
fn url_in(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .rfind(|l| l.starts_with("https://"))
        .unwrap_or_else(|| output.trim())
        .to_string()
}

fn gh(args: &[&str]) -> Result<String, String> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("运行 gh 失败（{e}）：巡检需要已登录的 GitHub CLI"))?;
    if !out.status.success() {
        return Err(format!(
            "gh {} 失败：{}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A tracker that records what it was asked to do.
    struct Fake {
        issues: Vec<Issue>,
        calls: Mutex<Vec<String>>,
    }

    impl Tracker for Fake {
        fn open_issues(&self) -> Result<Vec<Issue>, String> {
            Ok(self.issues.clone())
        }

        fn create(&self, title: &str, body: &str) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("create {title}: {body}"));
            Ok("https://github.com/o/r/issues/77\n".into())
        }

        fn comment(&self, number: u64, body: &str) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("comment {number}: {body}"));
            Ok(format!(
                "https://github.com/o/r/issues/{number}#issuecomment-1"
            ))
        }
    }

    fn fake(issues: Vec<Issue>) -> Fake {
        Fake {
            issues,
            calls: Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn with_no_issue_of_its_own_a_run_opens_one_and_marks_it() {
        let t = fake(vec![Issue {
            number: 5,
            title: "别的事".into(),
            body: "和巡检无关".into(),
        }]);
        assert_eq!(
            post(&t, "标题", "正文").unwrap(),
            Posted::Created {
                number: Some(77),
                url: "https://github.com/o/r/issues/77".into()
            }
        );
        let calls = t.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].starts_with("create 标题: 正文"), "{:?}", calls[0]);
        assert!(calls[0].contains(MARKER), "{:?}", calls[0]);
    }

    #[test]
    fn a_second_run_comments_on_the_issue_the_first_one_opened() {
        let t = fake(vec![
            Issue {
                number: 5,
                title: "别的事".into(),
                body: "和巡检无关".into(),
            },
            Issue {
                number: 9,
                title: "大麦改版巡检发现变化".into(),
                body: format!("上一次的报告\n\n{MARKER}\n"),
            },
        ]);
        assert_eq!(
            post(&t, "标题", "正文").unwrap(),
            Posted::Commented {
                number: 9,
                url: "https://github.com/o/r/issues/9#issuecomment-1".into()
            }
        );
        let calls = t.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1, "没有开第二个 issue：{calls:?}");
        assert!(calls[0].starts_with("comment 9: 正文"), "{:?}", calls[0]);
    }

    /// Recorded `gh` output, so the argv and the parsing are checked
    /// without a test reaching GitHub. Taken from this repository on
    /// 2026-10-14 with the same command `open_issues` runs.
    fn recorded(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/testdata/github/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn gh_asks_for_the_open_issues_of_one_repository_and_reads_them_back() {
        let seen: &'static Mutex<Vec<Vec<String>>> = Box::leak(Box::new(Mutex::new(Vec::new())));
        let gh = Gh::with_runner(
            Some("zbtzbtzbt/theFlash".into()),
            Box::new(move |args| {
                seen.lock()
                    .unwrap()
                    .push(args.iter().map(|a| a.to_string()).collect());
                Ok(recorded("issue_list_open.json"))
            }),
        );
        let issues = gh.open_issues().unwrap();
        assert_eq!(
            seen.lock().unwrap()[0],
            [
                "issue",
                "list",
                "--state",
                "open",
                "--limit",
                "100",
                "--json",
                "number,title,body",
                "--repo",
                "zbtzbtzbt/theFlash",
            ]
        );
        assert_eq!(issues.len(), 3);
        assert_eq!(issues[0].number, 34);
        assert!(issues.iter().all(|i| !i.title.is_empty()));
        // None of the real open issues is the inspection's own, so a run
        // with findings opens one rather than commenting on someone's.
        assert!(
            issues.iter().all(|i| !i.body.contains(MARKER)),
            "录下来的 issue 里不该有巡检自己的标记"
        );
    }

    /// `open` open issues, newest first, cut to `limit` the way
    /// `gh issue list` cuts its answer. The oldest is the inspection's own
    /// when `marked`: that is where it ends up once a repository has more
    /// open issues than a list holds.
    fn open_issue_list(open: u64, limit: usize, marked: bool) -> String {
        let issues: Vec<String> = (1..=open)
            .rev()
            .take(limit)
            .map(|number| {
                let body = if marked && number == 1 {
                    format!("上一次的报告\\n\\n{MARKER}\\n")
                } else {
                    "和巡检无关".to_string()
                };
                format!(r#"{{"number":{number},"title":"第 {number} 个","body":"{body}"}}"#)
            })
            .collect();
        format!("[{}]", issues.join(","))
    }

    fn limit_in(args: &[&str]) -> usize {
        args.iter()
            .position(|a| *a == "--limit")
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("argv 里没有 --limit：{args:?}"))
    }

    /// Every `--limit` the runs asked `issue list` for, in order.
    fn limits_asked(seen: &Mutex<Vec<Vec<String>>>) -> Vec<usize> {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|args| args.get(1).map(String::as_str) == Some("list"))
            .map(|args| {
                let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                limit_in(&borrowed)
            })
            .collect()
    }

    fn calls(seen: &Mutex<Vec<Vec<String>>>) -> Vec<String> {
        seen.lock()
            .unwrap()
            .iter()
            .map(|args| args[..2].join(" "))
            .collect()
    }

    fn recording() -> &'static Mutex<Vec<Vec<String>>> {
        Box::leak(Box::new(Mutex::new(Vec::new())))
    }

    /// A `gh` over a repository of `open` open issues, which records what
    /// it was asked and never reaches GitHub.
    fn gh_over(open: u64, marked: bool, seen: &'static Mutex<Vec<Vec<String>>>) -> Gh {
        Gh::with_runner(
            None,
            Box::new(move |args| {
                seen.lock()
                    .unwrap()
                    .push(args.iter().map(|a| a.to_string()).collect());
                match args.get(1).copied() {
                    Some("list") => Ok(open_issue_list(open, limit_in(args), marked)),
                    Some("comment") => Ok(format!(
                        "https://github.com/o/r/issues/{}#issuecomment-1",
                        args[2]
                    )),
                    Some("create") => Ok("https://github.com/o/r/issues/999\n".into()),
                    other => Err(format!("没想到的命令 {other:?}")),
                }
            }),
        )
    }

    /// #76: with 101 open issues, the inspection's own is the one that
    /// falls off a 100-issue list, so a run that reads one list opened a
    /// second issue — and would again the next day. A list that came back
    /// exactly as long as it asked for may be cut off, so the run asks for
    /// a longer one until it is not.
    #[test]
    fn the_inspections_own_issue_is_found_past_the_end_of_the_first_list() {
        let seen = recording();
        let gh = gh_over(101, true, seen);
        assert_eq!(
            post(&gh, "标题", "正文").unwrap(),
            Posted::Commented {
                number: 1,
                url: "https://github.com/o/r/issues/1#issuecomment-1".into()
            }
        );
        assert_eq!(limits_asked(seen), [100, 200]);
        assert_eq!(calls(seen), ["issue list", "issue list", "issue comment"]);
    }

    /// Exactly one full list and nothing of the inspection's in it: the
    /// run asks once more, hears a shorter answer, and only then opens an
    /// issue. Without the second ask it could not tell this from #76.
    #[test]
    fn a_full_list_with_no_issue_of_its_own_is_asked_about_once_more() {
        let seen = recording();
        let gh = gh_over(100, false, seen);
        assert!(matches!(
            post(&gh, "标题", "正文").unwrap(),
            Posted::Created { .. }
        ));
        assert_eq!(limits_asked(seen), [100, 200]);
        assert_eq!(calls(seen), ["issue list", "issue list", "issue create"]);
    }

    /// A `gh` that answers every ask with a full list — the repository
    /// really has that many, or gh is broken — must not keep the run
    /// asking forever, and must not end in a duplicate issue either. The
    /// stand-in refuses to answer past the ceiling, so a run without one
    /// fails here instead of hanging the test binary.
    #[test]
    fn a_gh_that_never_runs_out_of_issues_stops_instead_of_asking_forever() {
        let seen = recording();
        let gh = Gh::with_runner(
            None,
            Box::new(move |args| {
                seen.lock()
                    .unwrap()
                    .push(args.iter().map(|a| a.to_string()).collect());
                let limit = limit_in(args);
                if limit > LIMIT_CEILING {
                    return Err(format!("巡检问到了 {limit} 条，比上限还多"));
                }
                Ok(open_issue_list(limit as u64, limit, false))
            }),
        );
        let e = post(&gh, "标题", "正文").unwrap_err();
        assert!(e.contains("读不完"), "{e}");
        assert!(!calls(seen).contains(&"issue create".to_string()), "{e}");
        assert_eq!(
            *limits_asked(seen).last().unwrap(),
            LIMIT_CEILING,
            "问到头的那一次不是上限：{:?}",
            limits_asked(seen)
        );
    }

    #[test]
    fn a_failing_gh_is_reported_and_not_mistaken_for_no_issues() {
        let gh = Gh::with_runner(None, Box::new(|_| Err("gh: not logged in".into())));
        assert!(gh.open_issues().is_err());
        assert!(post(&gh, "标题", "正文").is_err());
    }

    #[test]
    fn the_url_is_taken_from_what_gh_prints() {
        assert_eq!(
            url_in("Creating issue in o/r\n\nhttps://github.com/o/r/issues/77\n"),
            "https://github.com/o/r/issues/77"
        );
        assert_eq!(url_in("奇怪的输出"), "奇怪的输出");
    }
}
