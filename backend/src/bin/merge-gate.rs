//! 合并闸门的命令行：查一个 PR 能不能合，或者排队把能合的合进去（#15）。
//!
//! No model is involved: every answer comes from GitHub's responses and
//! the review record on the pull request. `--replay <目录>` decides on
//! saved responses instead of asking GitHub, which is how the tests cover
//! each refusal without a network.

use std::path::PathBuf;
use std::process::ExitCode;

use theflash::merge_gate::{self, Decision, Snapshot, lock, snapshot};

const USAGE: &str = "用法：
  merge-gate check --pr <号> [--repo <owner/name>]
                                  只看这个 PR 能不能合并，什么都不改
  merge-gate check --replay <目录> 用保存下来的 GitHub 响应判断（测试用）
  merge-gate merge --pr <号> [--repo <owner/name>] [--dry-run]
                                  拿到合并锁，重新核对，然后合并这一个 PR
  merge-gate queue [--repo <owner/name>] [--dry-run]
                                  把所有打开的 PR 依次核对，能合的一个一个合

退出码：0 可以合并 / 已合并，1 拒绝合并，2 用法或取数据出错。
合并条件和审查记录格式见 docs/subsystems/merge-gate.md。";

const DEFAULT_REPO: &str = "zbtzbtzbt/theFlash";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let Some((command, rest)) = args.split_first() else {
        return usage();
    };
    let mut options = Options::default();
    if options.parse(rest).is_err() {
        return usage();
    }
    match *command {
        "check" => check(&options),
        "merge" => merge_one(&options),
        "queue" => queue(&options),
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

#[derive(Default)]
struct Options {
    repo: Option<String>,
    pr: Option<u64>,
    replay: Option<PathBuf>,
    dry_run: bool,
}

impl Options {
    fn parse(&mut self, args: &[&str]) -> Result<(), ()> {
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match *arg {
                "--repo" => self.repo = Some(it.next().ok_or(())?.to_string()),
                "--pr" => self.pr = Some(it.next().ok_or(())?.parse().map_err(|_| ())?),
                "--replay" => self.replay = Some(PathBuf::from(it.next().ok_or(())?)),
                "--dry-run" => self.dry_run = true,
                _ => return Err(()),
            }
        }
        Ok(())
    }

    fn repo(&self) -> &str {
        self.repo.as_deref().unwrap_or(DEFAULT_REPO)
    }
}

/// Reads the pull request and says what the gate decided, changing
/// nothing. Exit code 0 means it would merge, 1 means it refused.
fn check(options: &Options) -> ExitCode {
    let snapshot = match load(options) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let decision = merge_gate::decide(&snapshot);
    print_decision(&snapshot, &decision);
    if decision.is_merge() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn load(options: &Options) -> Result<Snapshot, ExitCode> {
    let result = match (&options.replay, options.pr) {
        (Some(dir), _) => Snapshot::replay(dir),
        (None, Some(pr)) => Snapshot::fetch(options.repo(), pr),
        (None, None) => return Err(usage()),
    };
    result.map_err(|e| {
        eprintln!("{e}");
        ExitCode::from(2)
    })
}

fn print_decision(s: &Snapshot, decision: &Decision) {
    let head = &s.pull.head_sha;
    match decision {
        Decision::Merge {
            sha,
            reviewer,
            record_url,
        } => println!(
            "PR #{} {}：可以合并（审查者 {reviewer}，审查记录 {record_url}）",
            s.pull.number,
            &sha[..7.min(sha.len())]
        ),
        Decision::Refuse(why) => println!(
            "PR #{} {}：拒绝合并 — {why}",
            s.pull.number,
            &head[..7.min(head.len())]
        ),
    }
}

/// Holds the merge lock while it re-reads the pull request and merges it,
/// so two sessions never merge at the same time.
fn merge_one(options: &Options) -> ExitCode {
    let Some(pr) = options.pr else { return usage() };
    let lock = match lock::Lock::acquire(&format!("merge --pr {pr}")) {
        Ok(l) => l,
        Err(busy) => {
            eprintln!("{busy}");
            return ExitCode::FAILURE;
        }
    };
    let code = merge_locked(options, pr);
    drop(lock);
    code
}

/// Every open pull request in turn, under one lock, so the queue merges
/// one at a time. A refusal is reported and the queue moves on.
fn queue(options: &Options) -> ExitCode {
    let lock = match lock::Lock::acquire("queue") {
        Ok(l) => l,
        Err(busy) => {
            eprintln!("{busy}");
            return ExitCode::FAILURE;
        }
    };
    let pulls = match Snapshot::open_pulls(options.repo()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    if pulls.is_empty() {
        println!("没有打开的 PR");
    }
    let mut merged = 0;
    for pr in pulls {
        // This run is still going, so the next session leaves the lock
        // alone instead of reading the lease as a session that stopped.
        if let Err(e) = lock.renew() {
            eprintln!("刷新合并锁失败：{e}；停止队列，不在锁状态不明时继续合并");
            return ExitCode::from(2);
        }
        if merge_locked(options, pr) == ExitCode::SUCCESS {
            merged += 1;
        }
    }
    drop(lock);
    println!("这一轮合并了 {merged} 个 PR");
    ExitCode::SUCCESS
}

/// The part that runs with the lock held: read the pull request now (not
/// when it was queued), decide, and merge the reviewed commit.
fn merge_locked(options: &Options, pr: u64) -> ExitCode {
    let one = Options {
        repo: options.repo.clone(),
        pr: Some(pr),
        replay: options.replay.clone(),
        dry_run: options.dry_run,
    };
    let snapshot = match load(&one) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let decision = merge_gate::decide(&snapshot);
    print_decision(&snapshot, &decision);
    let Decision::Merge {
        sha,
        reviewer,
        record_url,
    } = &decision
    else {
        return ExitCode::FAILURE;
    };
    if options.dry_run {
        println!("--dry-run：到此为止，没有合并");
        return ExitCode::SUCCESS;
    }
    let body = message_body(&snapshot, sha, reviewer, record_url);
    match snapshot::merge(options.repo(), pr, sha, &snapshot.pull.title, &body) {
        Ok(out) => {
            print!("{out}");
            println!("已合并 PR #{pr} 的 {}", &sha[..7.min(sha.len())]);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// The squash commit's body: the pull request's own description, plus who
/// reviewed which commit, so main's history says why this landed.
fn message_body(s: &Snapshot, sha: &str, reviewer: &str, record_url: &str) -> String {
    const LIMIT: usize = 3000;
    let mut body = s.pull.body.clone();
    if body.len() > LIMIT {
        let cut = (0..=LIMIT)
            .rev()
            .find(|i| body.is_char_boundary(*i))
            .unwrap_or(0);
        body.truncate(cut);
        body.push_str("\n……（正文过长，其余见 PR）");
    }
    format!(
        "{body}\n\n合并闸门：审查者 {reviewer} 在 {sha} 上审查通过（{record_url}），\
         CI 在这个提交上两个平台都是绿的。\n"
    )
}
