//! 认领租约的命令行：谁在做什么、心跳、过期收回（#16）。
//!
//! No model is involved: every answer comes from GitHub's responses and
//! the lease format in the issue's comments. `--replay <目录>` decides on
//! saved responses instead of asking GitHub, which is how the tests cover
//! each case without a network, and `--now <时间>` says what the board
//! looks like at a given moment.

use std::path::PathBuf;
use std::process::ExitCode;

use chrono::{DateTime, Utc};
use theflash::claim::{
    self, Board, Issue, Lease, Snapshot, Status, Trouble, lease, minutes, short, snapshot, stamp,
};

const USAGE: &str = "用法：
  claims list [--repo <owner/name>] [--json] [--now <时间>]
                              列出所有进行中的认领：哪个 agent、改哪些模块、
                              最后更新时间、是否已过期
  claims claim --issue <号> --agent <名字> --modules <路径,路径>
               [--branch <分支>] [--due <时间>] [--overlap <怎么分工>] [--dry-run]
                              认领一个 issue：加 认领中 标签，发一条认领评论
  claims heartbeat --issue <号> --agent <名字> --note <进度> [--dry-run]
                              在 issue 上报一次进度，刷新租约
  claims reclaim --issue <号> --agent <名字> [--dry-run]
                              收回一条已过期的认领：说明情况并去掉标签
  任何子命令都接受 --replay <目录>（用保存的 GitHub 响应判断，测试用）
  和 --now <时间>（RFC3339，默认现在）。

退出码：0 做了 / 可以做，1 拒绝，2 用法或取数据出错。
租约格式、心跳和过期规则见 docs/subsystems/claim-lease.md。";

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
        "list" => list(&options),
        "claim" => claim_one(&options),
        "heartbeat" => heartbeat(&options),
        "reclaim" => reclaim(&options),
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
    issue: Option<u64>,
    agent: Option<String>,
    modules: Vec<String>,
    branch: Option<String>,
    due: Option<String>,
    overlap: Option<String>,
    note: Option<String>,
    replay: Option<PathBuf>,
    now: Option<String>,
    json: bool,
    dry_run: bool,
}

impl Options {
    fn parse(&mut self, args: &[&str]) -> Result<(), ()> {
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = || it.next().ok_or(()).map(|v| v.to_string());
            match *arg {
                "--repo" => self.repo = Some(value()?),
                "--issue" => self.issue = Some(value()?.parse().map_err(|_| ())?),
                "--agent" => self.agent = Some(value()?),
                "--modules" => self.modules = split_modules(&value()?),
                "--branch" => self.branch = Some(value()?),
                "--due" => self.due = Some(value()?),
                "--overlap" => self.overlap = Some(value()?),
                "--note" => self.note = Some(value()?),
                "--replay" => self.replay = Some(PathBuf::from(value()?)),
                "--now" => self.now = Some(value()?),
                "--json" => self.json = true,
                "--dry-run" => self.dry_run = true,
                _ => return Err(()),
            }
        }
        Ok(())
    }

    fn repo(&self) -> &str {
        self.repo.as_deref().unwrap_or(DEFAULT_REPO)
    }

    fn agent(&self) -> &str {
        self.agent.as_deref().unwrap_or_default()
    }

    /// Replayed responses cannot be written back to, so a replayed run
    /// never posts, whether or not `--dry-run` was passed.
    fn writes(&self) -> bool {
        !self.dry_run && self.replay.is_none()
    }

    fn now(&self) -> Result<DateTime<Utc>, ExitCode> {
        match &self.now {
            None => Ok(claim::now()),
            Some(text) => lease::parse_time(text).ok_or_else(|| {
                eprintln!("--now {text} 读不出来：要带时区，例如 2026-10-14T03:20:00+08:00");
                ExitCode::from(2)
            }),
        }
    }

    fn board(&self) -> Result<Board, ExitCode> {
        let snapshot = match &self.replay {
            Some(dir) => Snapshot::replay(dir),
            None => Snapshot::fetch(self.repo()),
        };
        snapshot.map(|s| claim::board(&s)).map_err(fetch_failed)
    }

    fn issue(&self, number: u64) -> Result<Issue, ExitCode> {
        match &self.replay {
            Some(dir) => Snapshot::replay_issue(dir, number),
            None => Snapshot::fetch_issue(self.repo(), number),
        }
        .map_err(fetch_failed)
    }
}

fn fetch_failed(e: snapshot::Error) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(2)
}

/// `--modules a,b c` into paths, the same way the lease format reads them.
fn split_modules(value: &str) -> Vec<String> {
    value
        .split([',', '，', ' ', '、'])
        .map(|m| m.trim().trim_end_matches('/').trim_start_matches("./"))
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .collect()
}

/// Every claim in progress. Exit code 0 even when there are none: asking
/// what is claimed is not a thing that can fail.
fn list(options: &Options) -> ExitCode {
    let now = match options.now() {
        Ok(n) => n,
        Err(code) => return code,
    };
    let board = match options.board() {
        Ok(b) => b,
        Err(code) => return code,
    };
    if options.json {
        println!("{}", json(&board, now));
        return ExitCode::SUCCESS;
    }
    print_board(&board, now);
    ExitCode::SUCCESS
}

fn print_board(board: &Board, now: DateTime<Utc>) {
    println!("进行中的认领（{} 北京时间）", short(now));
    if board.leases.is_empty() && board.trouble.is_empty() {
        println!("\n没有认领中的 issue。");
        return;
    }
    for l in &board.leases {
        println!();
        print_lease(l, now);
    }
    for t in &board.trouble {
        println!();
        print_trouble(t);
    }
    let expired = board
        .leases
        .iter()
        .filter(|l| l.status(now) == Status::Expired)
        .count();
    let overdue = board
        .leases
        .iter()
        .filter(|l| l.status(now) == Status::HeartbeatOverdue)
        .count();
    println!(
        "\n{} 条认领：{} 活跃，{overdue} 该心跳了，{expired} 已过期{}。{}",
        board.leases.len(),
        board.leases.len() - expired - overdue,
        if expired > 0 {
            "（`claims reclaim --issue <号> --agent <你>` 收回）"
        } else {
            ""
        },
        if board.trouble.is_empty() {
            String::new()
        } else {
            format!("另有 {} 个 issue 的认领读不出来。", board.trouble.len())
        }
    );
}

fn print_lease(l: &Lease, now: DateTime<Utc>) {
    let status = l.status(now);
    println!("#{} {}", l.issue, l.title);
    println!(
        "  agent     {}{}",
        l.agent(),
        match &l.assigned_to {
            Some(a) => format!("（{}{a}）", lease::ASSIGNED_PREFIX),
            None => String::new(),
        }
    );
    println!("  模块      {}", l.claim.modules.join(", "));
    println!(
        "  最后更新  {}（{}前，{}）",
        short(l.last.at),
        minutes(l.idle_minutes(now)),
        l.last.sign.what()
    );
    println!(
        "  状态      {}{}",
        status.name(),
        match status {
            Status::Expired => "，可以收回".to_string(),
            _ => format!("，{}后过期", minutes(l.expires_in_minutes(now))),
        }
    );
    if let Some(branch) = &l.claim.branch {
        println!("  分支      {branch}");
    }
    if let Some(due) = l.claim.due {
        println!(
            "  预计完成  {}{}",
            short(due),
            if due < now { "（已超时）" } else { "" }
        );
    }
    if !l.claim.overlap_note.is_empty() {
        println!("  重叠      {}", l.claim.overlap_note);
    }
    println!("  认领评论  {}", l.claim.url);
}

fn print_trouble(t: &Trouble) {
    println!("#{} {}", t.issue, t.title);
    println!("  读不出来  {}", t.why);
}

/// The board as one JSON object, for the dispatcher and other scripts.
fn json(board: &Board, now: DateTime<Utc>) -> String {
    let leases: Vec<serde_json::Value> = board
        .leases
        .iter()
        .map(|l| {
            serde_json::json!({
                "issue": l.issue,
                "title": l.title,
                "agent": l.agent(),
                "modules": l.claim.modules,
                "branch": l.claim.branch,
                "assigned_to": l.assigned_to,
                "claim_url": l.claim.url,
                "claimed_at": stamp(l.claim.posted_at),
                "due": l.claim.due.map(stamp),
                "last_update": stamp(l.last.at),
                "last_update_was": l.last.sign.what(),
                "idle_minutes": l.idle_minutes(now),
                "status": l.status(now).name(),
                "expired": l.status(now) == Status::Expired,
                "expires_in_minutes": l.expires_in_minutes(now),
            })
        })
        .collect();
    let trouble: Vec<serde_json::Value> = board
        .trouble
        .iter()
        .map(|t| serde_json::json!({"issue": t.issue, "title": t.title, "why": t.why}))
        .collect();
    serde_json::json!({
        "now": stamp(now),
        "heartbeat_every_minutes": lease::HEARTBEAT_EVERY_MINUTES,
        "expires_after_minutes": lease::EXPIRES_AFTER_MINUTES,
        "leases": leases,
        "trouble": trouble,
    })
    .to_string()
}

/// Claims an issue: refuses first, then posts the lease, then labels it,
/// so a refusal leaves nothing behind.
fn claim_one(options: &Options) -> ExitCode {
    let Some(number) = options.issue else {
        return usage();
    };
    if options.agent().is_empty() || options.modules.is_empty() {
        return usage();
    }
    let now = match options.now() {
        Ok(n) => n,
        Err(code) => return code,
    };
    let due = match &options.due {
        Some(text) => match lease::parse_time(text) {
            Some(t) => Some(t),
            None => {
                eprintln!("--due {text} 读不出来：要带时区，例如 2026-10-14T06:00:00+08:00");
                return ExitCode::from(2);
            }
        },
        None => None,
    };
    let (board, issue) = match (options.board(), options.issue(number)) {
        (Ok(b), Ok(i)) => (b, i),
        (Err(code), _) | (_, Err(code)) => return code,
    };
    let verdict = claim::decide_claim(
        &board,
        &issue,
        options.agent(),
        &options.modules,
        options.overlap.as_deref(),
        now,
    );
    let claim::Verdict::Ok { overlaps } = &verdict else {
        return refused(&verdict);
    };
    let body = claim::claim_comment(
        options.agent(),
        &options.modules,
        options.branch.as_deref(),
        due,
        overlaps,
        options.overlap.as_deref(),
        now,
    );
    if !overlaps.is_empty() {
        println!(
            "注意：和进行中的认领重叠，已按 --overlap 写进认领评论（{}）",
            overlaps
                .iter()
                .map(|o| format!("#{}：{}", o.issue, o.modules.join("、")))
                .collect::<Vec<_>>()
                .join("；")
        );
    }
    println!("要发的认领评论：\n{body}");
    if !options.writes() {
        println!("没有真的发评论、没有加标签（--dry-run 或 --replay）");
        return ExitCode::SUCCESS;
    }
    post(options, number, &body, Some(Label::Add(lease::CLAIM_LABEL)))
}

/// Reports progress, which is what keeps a lease from expiring.
fn heartbeat(options: &Options) -> ExitCode {
    let (Some(number), Some(note)) = (options.issue, options.note.as_deref()) else {
        return usage();
    };
    if options.agent().is_empty() {
        return usage();
    }
    let now = match options.now() {
        Ok(n) => n,
        Err(code) => return code,
    };
    let board = match options.board() {
        Ok(b) => b,
        Err(code) => return code,
    };
    match board.lease(number) {
        Some(held) if !lease::same_agent(held.agent(), options.agent()) => {
            eprintln!(
                "#{number} 是 {} 的认领，不是 {} 的：心跳只刷新自己的租约",
                held.agent(),
                options.agent()
            );
            return ExitCode::FAILURE;
        }
        Some(_) => {}
        None => {
            eprintln!(
                "#{number} 上没有读得出来的认领，心跳不会刷新任何租约。\
                 先 `claims claim --issue {number} --agent {}`",
                options.agent()
            );
            return ExitCode::FAILURE;
        }
    }
    let body = claim::heartbeat_comment(options.agent(), note, now);
    println!("要发的心跳评论：\n{body}");
    if !options.writes() {
        println!("没有真的发评论（--dry-run 或 --replay）");
        return ExitCode::SUCCESS;
    }
    post(options, number, &body, None)
}

/// Takes over an expired claim: says what it took over and on what
/// evidence, then removes the label so the issue can be claimed again.
fn reclaim(options: &Options) -> ExitCode {
    let Some(number) = options.issue else {
        return usage();
    };
    if options.agent().is_empty() {
        return usage();
    }
    let now = match options.now() {
        Ok(n) => n,
        Err(code) => return code,
    };
    let board = match options.board() {
        Ok(b) => b,
        Err(code) => return code,
    };
    let verdict = claim::decide_reclaim(&board, number, now);
    if !verdict.is_ok() {
        return refused(&verdict);
    }
    let held = board.lease(number).expect("a reclaimable lease exists");
    let body = claim::reclaim_comment(held, options.agent(), now);
    println!("要发的收回评论：\n{body}");
    if !options.writes() {
        println!("没有真的发评论、没有去掉标签（--dry-run 或 --replay）");
        return ExitCode::SUCCESS;
    }
    post(
        options,
        number,
        &body,
        Some(Label::Remove(lease::CLAIM_LABEL)),
    )
}

/// The label change a command makes after its comment, if any. A
/// heartbeat changes no label: the issue is already claimed.
enum Label {
    Add(&'static str),
    Remove(&'static str),
}

/// Posts the comment, then changes the label, in that order: the comment
/// is the lease, and a label without one is the state #16 calls
/// unreadable.
fn post(options: &Options, number: u64, body: &str, label: Option<Label>) -> ExitCode {
    match snapshot::comment(options.repo(), number, body) {
        Ok(out) => print!("{out}"),
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    }
    let (done, result) = match label {
        None => return ExitCode::SUCCESS,
        Some(Label::Add(l)) => (
            format!("#{number} 加上了 {l} 标签"),
            snapshot::add_label(options.repo(), number, l),
        ),
        Some(Label::Remove(l)) => (
            format!("#{number} 去掉了 {l} 标签"),
            snapshot::remove_label(options.repo(), number, l),
        ),
    };
    match result {
        Ok(_) => {
            println!("{done}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}\n评论已经发出去了，标签没改成：手动改一下标签");
            ExitCode::from(2)
        }
    }
}

fn refused(verdict: &claim::Verdict) -> ExitCode {
    if let claim::Verdict::Refuse(why) = verdict {
        println!("拒绝 — {why}");
    }
    ExitCode::FAILURE
}
