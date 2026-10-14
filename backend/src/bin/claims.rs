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
use theflash::claim::priority::{self, Held, Level, Priority, Queue};
use theflash::claim::{
    self, Board, Issue, Lease, OpenIssue, Snapshot, Status, Trouble, lease, minutes, short,
    snapshot, stamp,
};

const USAGE: &str = "用法：
  claims queue [--agent <名字>] [--json]
                              现在可以开工的 issue，按优先级排（P0 到 P3，
                              没定级的在最前面），以及其他 issue 为什么不在里面
  claims list [--repo <owner/name>] [--json] [--now <时间>]
                              列出所有进行中的认领：哪个 agent、改哪些模块、
                              最后更新时间、是否已过期
  claims triage --issue <号> --agent <名字> --priority <P0～P3>
                --important <为什么重要或不重要> --urgent <为什么紧急或不紧急> [--dry-run]
                              定级或改级：发一条 ## 优先级 评论，再换标签
  claims claim --issue <号> --agent <名字> --modules <路径,路径>
               [--branch <分支>] [--due <时间>] [--overlap <怎么分工>]
               [--out-of-order <为什么不按队列>] [--dry-run]
                              认领一个 issue：加 认领中 标签，发一条认领评论。
                              没定级、等裁决、依赖没关、或者队列里有更高优先级
                              的 issue 时拒绝，除非用 --out-of-order 写明理由
  claims heartbeat --issue <号> --agent <名字> [--claim <认领评论链接>] --note <进度> [--dry-run]
                              在 issue 上报一次进度，刷新租约
  claims release --issue <号> --agent <名字> [--claim <认领评论链接>] [--note <说明>] [--dry-run]
                              持有者或调度程序主动交还一条租约：发 ## 交还认领 评论并去掉标签
  claims reclaim --issue <号> --agent <名字> [--dry-run]
                              收回一条已过期的认领：说明情况并去掉标签
  任何子命令都接受 --replay <目录>（用保存的 GitHub 响应判断，测试用）
  和 --now <时间>（RFC3339，默认现在）。

退出码：0 做了 / 可以做，1 拒绝，2 用法或取数据出错。
租约格式、心跳和过期规则见 docs/subsystems/claim-lease.md，
优先级怎么定、队列怎么排见 docs/subsystems/issue-priority.md。";

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
        "queue" => queue(&options),
        "list" => list(&options),
        "triage" => triage(&options),
        "claim" => claim_one(&options),
        "heartbeat" => heartbeat(&options),
        "release" => release(&options),
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
    out_of_order: Option<String>,
    claim_url: Option<String>,
    note: Option<String>,
    priority: Option<Priority>,
    important: Option<String>,
    urgent: Option<String>,
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
                "--out-of-order" => self.out_of_order = Some(value()?),
                "--claim" => self.claim_url = Some(value()?),
                "--note" => self.note = Some(value()?),
                "--priority" => self.priority = Some(Priority::parse(&value()?).ok_or(())?),
                "--important" => self.important = Some(value()?),
                "--urgent" => self.urgent = Some(value()?),
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

    /// The board plus issue `number` read on its own, which is what the
    /// commands that act on one issue decide on. The snapshot only holds
    /// issues labelled `认领中`, and the label is added after the claim
    /// comment, so without this a claim whose label never made it on
    /// would look like a free issue (#16).
    fn board_with(&self, number: u64) -> Result<(Board, Issue), ExitCode> {
        let mut board = self.board()?;
        let issue = self.issue(number)?;
        let pushes = match &self.replay {
            Some(dir) => Snapshot::replay_pushes_for_issue(dir, &issue),
            None => Snapshot::fetch_pushes_for_issue(&["gh".to_string()], self.repo(), &issue),
        }
        .map_err(fetch_failed)?;
        board.pushes.extend(pushes);
        board.include(&issue);
        Ok((board, issue))
    }

    fn issue(&self, number: u64) -> Result<Issue, ExitCode> {
        match &self.replay {
            Some(dir) => Snapshot::replay_issue(dir, number),
            None => Snapshot::fetch_issue(self.repo(), number),
        }
        .map_err(fetch_failed)
    }

    /// Every open issue with its labels and body, which is what the
    /// priority queue is computed from.
    fn open_issues(&self) -> Result<Vec<OpenIssue>, ExitCode> {
        match &self.replay {
            Some(dir) => Snapshot::replay_open_issues(dir),
            None => Snapshot::fetch_open_issues(self.repo()),
        }
        .map_err(fetch_failed)
    }

    /// `--agent`, when one was given: `queue` works without it.
    fn some_agent(&self) -> Option<&str> {
        self.agent.as_deref().filter(|a| !a.trim().is_empty())
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

/// What can be started now, by priority. Exit code 0 even when nothing
/// can: an empty queue is an answer, not a failure.
fn queue(options: &Options) -> ExitCode {
    let now = match options.now() {
        Ok(n) => n,
        Err(code) => return code,
    };
    let board = match options.board() {
        Ok(b) => b,
        Err(code) => return code,
    };
    let open = match options.open_issues() {
        Ok(o) => o,
        Err(code) => return code,
    };
    let agent = options.some_agent();
    let q = priority::queue(&open, &board, agent, now);
    if options.json {
        println!("{}", queue_json(&q, now));
    } else {
        print_queue(&q, agent, now);
    }
    ExitCode::SUCCESS
}

fn print_queue(q: &Queue, agent: Option<&str>, now: DateTime<Utc>) {
    println!(
        "可以开工的 issue（{} 北京时间{}）：P0 到 P3，同一档里号小的在前",
        short(now),
        agent
            .map(|a| format!("，按 {a} 能认领的算"))
            .unwrap_or_default()
    );
    let mut heading: Option<String> = None;
    for r in &q.ready {
        let this = match &r.level {
            Level::Set(p) => format!("{} {}", p.label(), p.meaning()),
            _ => "没定优先级：先定级再排队（`scripts/claims triage --issue <号> --agent <你> \
                  --priority <P0～P3> --important <…> --urgent <…>`）"
                .to_string(),
        };
        if heading.as_ref() != Some(&this) {
            println!("\n{this}");
            heading = Some(this);
        }
        let mut line = format!("  #{} {}", r.issue, r.title);
        if let Level::Several(_) = r.level {
            line.push_str(&format!("（同时标着 {}）", r.level.describe()));
        }
        if let Some(a) = &r.assigned_to {
            line.push_str(&format!("（{}{a}）", lease::ASSIGNED_PREFIX));
        }
        println!("{line}");
        for note in &r.notes {
            println!("      {note}");
        }
    }
    if q.ready.is_empty() {
        println!("\n没有可以开工的 issue。");
    }
    if !q.waiting.is_empty() {
        println!("\n不在队列里的 {} 个：", q.waiting.len());
        for kind in [
            "认领中",
            "认领读不出来",
            "待验收",
            "等裁决",
            "被依赖挡着",
            "指派给别人",
        ] {
            let these: Vec<String> = q
                .waiting
                .iter()
                .filter(|w| w.held.name() == kind)
                .map(|w| {
                    let detail = match &w.held {
                        Held::Claimed { agent } => agent.clone(),
                        Held::Blocked(by) => format!(
                            "等 {}",
                            by.iter()
                                .map(|n| format!("#{n}"))
                                .collect::<Vec<_>>()
                                .join("、")
                        ),
                        Held::AssignedTo(a) => a.clone(),
                        _ => String::new(),
                    };
                    let mut parts = vec![w.level.describe()];
                    if !detail.is_empty() {
                        parts.push(detail);
                    }
                    format!("#{}（{}）", w.issue, parts.join("，"))
                })
                .collect();
            if !these.is_empty() {
                println!("  {kind}：{}", these.join("、"));
            }
        }
    }
    match q.next() {
        Some(r) => println!(
            "\n下一个：#{}（{}）{}\n  scripts/claims claim --issue {} --agent {} --modules <要改的路径> --branch <分支>",
            r.issue,
            r.level.describe(),
            r.title,
            r.issue,
            agent.unwrap_or("<你>")
        ),
        None if q.ready.is_empty() => {}
        None => println!("\n队列里只有没定级的：先定级，才知道下一个是谁。"),
    }
}

/// The queue as one JSON object, for the dispatcher.
fn queue_json(q: &Queue, now: DateTime<Utc>) -> String {
    let ready: Vec<serde_json::Value> = q
        .ready
        .iter()
        .map(|r| {
            serde_json::json!({
                "issue": r.issue,
                "title": r.title,
                "priority": r.level.priority().map(Priority::label),
                "level": r.level.describe(),
                "assigned_to": r.assigned_to,
                "notes": r.notes,
            })
        })
        .collect();
    let waiting: Vec<serde_json::Value> = q
        .waiting
        .iter()
        .map(|w| {
            let mut entry = serde_json::json!({
                "issue": w.issue,
                "title": w.title,
                "priority": w.level.priority().map(Priority::label),
                "level": w.level.describe(),
                "why": w.held.name(),
            });
            match &w.held {
                Held::Claimed { agent } => entry["agent"] = agent.clone().into(),
                Held::Blocked(by) => entry["blocked_by"] = by.clone().into(),
                Held::AssignedTo(a) => entry["assigned_to"] = a.clone().into(),
                _ => {}
            }
            entry
        })
        .collect();
    serde_json::json!({
        "now": stamp(now),
        "next": q.next().map(|r| r.issue),
        "ready": ready,
        "waiting": waiting,
    })
    .to_string()
}

/// Sets an issue's priority: posts the `## 优先级` comment, then swaps
/// the label, so a label change always has its reason on the issue.
fn triage(options: &Options) -> ExitCode {
    let (Some(number), Some(new)) = (options.issue, options.priority) else {
        return usage();
    };
    let reasons = (
        options.important.as_deref().map(str::trim),
        options.urgent.as_deref().map(str::trim),
    );
    let (Some(important), Some(urgent)) = reasons else {
        return usage();
    };
    if options.agent().is_empty() || important.is_empty() || urgent.is_empty() {
        return usage();
    }
    let now = match options.now() {
        Ok(n) => n,
        Err(code) => return code,
    };
    let open = match options.open_issues() {
        Ok(o) => o,
        Err(code) => return code,
    };
    let Some(issue) = open.iter().find(|i| i.number == number) else {
        println!("拒绝 — #{number} 不在打开的 issue 里（已经关了，或者是 PR），不用定级");
        return ExitCode::FAILURE;
    };
    let old = priority::level(&issue.labels);
    let remove = priority::labels_to_remove(&issue.labels, new);
    if old == Level::Set(new) && remove.is_empty() {
        println!(
            "#{number} 已经是 {}（{}），没有改",
            new.label(),
            new.meaning()
        );
        return ExitCode::SUCCESS;
    }
    let body = priority::triage_comment(options.agent(), new, &old, important, urgent, now);
    println!("要发的定级评论：\n{body}");
    println!(
        "要改的标签：加上 {}{}",
        new.label(),
        if remove.is_empty() {
            String::new()
        } else {
            format!("，去掉 {}", remove.join("、"))
        }
    );
    if !options.writes() {
        println!("没有真的发评论、没有改标签（--dry-run 或 --replay）");
        return ExitCode::SUCCESS;
    }
    let before = match options.issue(number) {
        Ok(i) => i,
        Err(code) => return code,
    };
    let before_urls: Vec<String> = before.comments.iter().map(|c| c.url.clone()).collect();
    let posted = match comment(options, number, &body) {
        Ok(url) => url,
        Err(code) => return code,
    };
    let after_comment = match options.issue(number) {
        Ok(i) => i,
        Err(code) => return code,
    };
    match priority::confirm_triage(&before_urls, &after_comment, &posted) {
        priority::TriageConfirmed::Won => {}
        priority::TriageConfirmed::Lost {
            agent,
            priority: won,
            comment: winner_url,
        } => {
            eprintln!(
                "拒绝 — #{number} 几乎同时被 {agent} 定成了 {}（{winner_url}），\
                 那条评论更早，以它为准；我这条没有改标签",
                won.label()
            );
            return ExitCode::FAILURE;
        }
        priority::TriageConfirmed::Unclear(why) => {
            eprintln!("定级评论发出去了，但读回来确认时出错：{why}；没有改标签");
            return ExitCode::from(2);
        }
    }
    let mut remove = priority::labels_to_remove(&after_comment.labels, new);
    for label in priority::labels_to_remove(&issue.labels, new) {
        if !remove.contains(&label) {
            remove.push(label);
        }
    }
    let remove_refs: Vec<&str> = remove.iter().map(String::as_str).collect();
    if let Err(e) = snapshot::edit_labels(options.repo(), number, &[new.label()], &remove_refs) {
        eprintln!(
            "{e}\n定级评论已经发出去了，标签没改成：\
             `gh issue edit {number} --add-label {}{}` 补上",
            new.label(),
            remove
                .iter()
                .map(|l| format!(" --remove-label {l}"))
                .collect::<String>()
        );
        return ExitCode::from(2);
    }
    let mut verified = match options.issue(number) {
        Ok(i) => i,
        Err(code) => return code,
    };
    let leftover = priority::labels_to_remove(&verified.labels, new);
    let has_new = verified.labels.iter().any(|l| l == new.label());
    if !has_new || !leftover.is_empty() {
        let add_slice: &[&str] = if has_new { &[] } else { &[new.label()] };
        let leftover_refs: Vec<&str> = leftover.iter().map(String::as_str).collect();
        if let Err(e) = snapshot::edit_labels(options.repo(), number, add_slice, &leftover_refs) {
            eprintln!("{e}\n补齐 #{number} 的优先级标签失败");
            return ExitCode::from(2);
        }
        verified = match options.issue(number) {
            Ok(i) => i,
            Err(code) => return code,
        };
    }
    if priority::level(&verified.labels) != Level::Set(new)
        || !priority::labels_to_remove(&verified.labels, new).is_empty()
    {
        eprintln!(
            "#{number} 改完标签之后读回来是 {}，不是单一的 {}：请去 issue 上核对标签",
            priority::level(&verified.labels).describe(),
            new.label()
        );
        return ExitCode::from(2);
    }
    println!("#{number} 现在是 {}", new.label());
    ExitCode::SUCCESS
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
    let expired = board.leases.iter().filter(|l| l.reclaimable(now)).count();
    let unknown = board
        .leases
        .iter()
        .filter(|l| l.push_unreadable.is_some())
        .count();
    let overdue = board
        .leases
        .iter()
        .filter(|l| l.push_unreadable.is_none() && l.status(now) == Status::HeartbeatOverdue)
        .count();
    println!(
        "\n{} 条认领：{} 活跃，{overdue} 该心跳了，{expired} 已过期{}{}。{}",
        board.leases.len(),
        board.leases.len() - expired - overdue - unknown,
        if expired > 0 {
            "（`claims reclaim --issue <号> --agent <你>` 收回）"
        } else {
            ""
        },
        if unknown > 0 {
            format!("，{unknown} 条推送时间读不到、无法判断")
        } else {
            String::new()
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
    if l.push_unreadable.is_some() {
        println!("  状态      无法判断，不能收回");
    } else {
        println!(
            "  状态      {}{}",
            status.name(),
            match status {
                Status::Expired => "，可以收回".to_string(),
                _ => format!("，{}后过期", minutes(l.expires_in_minutes(now))),
            }
        );
    }
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
    if let Some(why) = &l.push_unreadable {
        println!("  推送      {why}（证明不了它沉默，不能收回）");
    }
    if !l.labelled {
        println!(
            "  标签      没有 {} 标签（发认领评论之后标签没加上），下一次心跳会补上",
            lease::CLAIM_LABEL
        );
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
                // A lease nobody can show to be silent is not expired as
                // far as a dispatcher is concerned, however long ago its
                // last readable sign of life was.
                "expired": l.reclaimable(now),
                "expires_in_minutes": l.expires_in_minutes(now),
                "push_unreadable": l.push_unreadable,
                "labelled": l.labelled,
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
    let (board, issue) = match options.board_with(number) {
        Ok(both) => both,
        Err(code) => return code,
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
    // The lease checks come first: "somebody else holds it" is the more
    // specific thing to be told than "it is not the next in the queue".
    let open = match options.open_issues() {
        Ok(o) => o,
        Err(code) => return code,
    };
    let queue = priority::queue(&open, &board, Some(options.agent()), now);
    let out_of_order =
        match priority::decide(&open, &queue, number, options.out_of_order.as_deref()) {
            priority::Verdict::Ok { out_of_order } => out_of_order,
            priority::Verdict::Refuse(why) => {
                println!("拒绝 — {why}");
                return ExitCode::FAILURE;
            }
        };
    let body = claim::claim_comment(
        options.agent(),
        &options.modules,
        options.branch.as_deref(),
        due,
        overlaps,
        options.overlap.as_deref(),
        out_of_order.as_deref(),
        now,
    );
    if let Some(note) = &out_of_order {
        println!("注意：没按优先级队列认领，已按 --out-of-order 写进认领评论（{note}）");
    }
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
    let posted = match comment(options, number, &body) {
        Ok(url) => url,
        Err(code) => return code,
    };
    // Two agents can post a claim at the same moment, so the claim is
    // confirmed rather than locked: read the issue back and see whose
    // claim came first (claim::confirm_claim).
    match confirm(options, number, &posted, &board) {
        Ok(()) => {}
        Err(code) => return code,
    }
    if let Err(code) = label(options, number, Label::Add(lease::CLAIM_LABEL)) {
        return code;
    }
    // Issue #90: two agents can also read the same board at the same
    // moment and claim two *different* issues for the same module without
    // `--overlap`. After adding the `认领中` label so this claim is visible
    // on the board, re-read the board and check whether another issue's
    // claim covering our modules landed earlier.
    confirm_board_overlap(options, number, &posted, now)
}

fn confirm_board_overlap(
    options: &Options,
    number: u64,
    posted: &str,
    now: DateTime<Utc>,
) -> ExitCode {
    if options
        .overlap
        .as_deref()
        .is_some_and(|r| !r.trim().is_empty())
    {
        return ExitCode::SUCCESS;
    }
    let fresh_board = match options.board() {
        Ok(b) => b,
        Err(code) => return code,
    };
    let my_issue = match options.issue(number) {
        Ok(i) => i,
        Err(code) => return code,
    };
    let Some(my_comment) = my_issue.comments.iter().find(|c| c.url == posted) else {
        return ExitCode::SUCCESS;
    };
    for other in fresh_board.holding(now) {
        if other.issue == number {
            continue;
        }
        let shared = lease::overlap(&other.claim.modules, &options.modules);
        if shared.is_empty() || !other.claim.overlap_note.is_empty() {
            continue;
        }
        let other_id = comment_id_from_url(&other.claim.url);
        let earlier = (other.claim.posted_at, other_id) < (my_comment.posted_at, my_comment.id)
            || ((other.claim.posted_at, other_id) == (my_comment.posted_at, my_comment.id)
                && other.issue < number);
        if earlier {
            let body = claim::yield_overlap_comment(
                options.agent(),
                posted,
                other.issue,
                other.agent(),
                &other.claim.url,
                &shared,
            );
            println!("要发的让出评论：\n{body}");
            let _ = comment(options, number, &body);
            let _ = snapshot::remove_label(options.repo(), number, lease::CLAIM_LABEL);
            eprintln!(
                "让出 — #{} 几乎同时被 {} 认领了（{}），同样要改 {}，而且那条认领更早、两边都没写分工。\
                 我这条已经作废并去掉了 {} 标签；写清分工后再带 --overlap 重新认领",
                other.issue,
                other.agent(),
                other.claim.url,
                shared.join("、"),
                lease::CLAIM_LABEL
            );
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn comment_id_from_url(url: &str) -> u64 {
    url.rsplit_once("issuecomment-")
        .and_then(|(_, digits)| digits.parse().ok())
        .unwrap_or(u64::MAX)
}

/// Checks that the claim just posted is the one that holds the issue, and
/// gives it up if an earlier claim beat it.
fn confirm(options: &Options, number: u64, posted: &str, board: &Board) -> Result<(), ExitCode> {
    let issue = options.issue(number)?;
    match claim::confirm_claim(&issue, posted, &board.pushes) {
        claim::Confirmed::Held => Ok(()),
        claim::Confirmed::Lost { agent, claim } => {
            let body = claim::yield_comment(options.agent(), posted, &agent, &claim);
            println!("要发的让出评论：\n{body}");
            let _ = comment(options, number, &body);
            eprintln!(
                "让出 — #{number} 几乎同时被 {agent} 认领了，它那条（{claim}）更早，算它的。\
                 我这条已经作废（上面那条让出评论），没有加 {} 标签。\
                 换一个 issue，或者等 {agent} 做完",
                lease::CLAIM_LABEL
            );
            Err(ExitCode::FAILURE)
        }
        claim::Confirmed::Unclear(why) => {
            eprintln!(
                "评论发出去了，但读回来的时候认领读不出来：{why}\n\
                 没有加 {} 标签：先去 issue 上看一眼是谁在做",
                lease::CLAIM_LABEL
            );
            Err(ExitCode::from(2))
        }
    }
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
    let (board, _) = match options.board_with(number) {
        Ok(both) => both,
        Err(code) => return code,
    };
    // A lease whose `认领中` label never made it on is still a lease, and
    // reporting progress is where it gets its label back: the holder does
    // not have to notice or fix the label by hand (#16).
    let (relabel, claim_url) = match board.lease(number) {
        Some(held) if !lease::same_agent(held.agent(), options.agent()) => {
            eprintln!(
                "#{number} 是 {} 的认领，不是 {} 的：心跳只刷新自己的租约",
                held.agent(),
                options.agent()
            );
            return ExitCode::FAILURE;
        }
        Some(held) => {
            if let Some(want) = options
                .claim_url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                && want != held.claim.url
            {
                eprintln!(
                    "拒绝 — #{number} 当前的认领是 {}（{}），不是你传的 {want}：\
                     你那条旧认领已经被收回或交还，不能续在另一个同厂商会话的租约上",
                    held.claim.url,
                    held.agent()
                );
                return ExitCode::FAILURE;
            }
            (!held.labelled, held.claim.url.clone())
        }
        None => {
            eprintln!(
                "#{number} 上没有读得出来的认领，心跳不会刷新任何租约。\
                 先 `claims claim --issue {number} --agent {}`",
                options.agent()
            );
            return ExitCode::FAILURE;
        }
    };
    let body = claim::heartbeat_comment(options.agent(), &claim_url, note, now);
    println!("要发的心跳评论：\n{body}");
    if relabel {
        println!(
            "#{number} 有租约但没有 {} 标签（发认领评论之后标签没加上），这次心跳把它补上",
            lease::CLAIM_LABEL
        );
    }
    if !options.writes() {
        println!("没有真的发评论（--dry-run 或 --replay）");
        return ExitCode::SUCCESS;
    }
    if let Err(code) = comment(options, number, &body) {
        return code;
    }
    if relabel && let Err(code) = label(options, number, Label::Add(lease::CLAIM_LABEL)) {
        return code;
    }
    ExitCode::SUCCESS
}

/// Voluntarily releases a lease held by `agent` (or ended by `dispatcher`)
/// without waiting 120 minutes for expiry (#87).
fn release(options: &Options) -> ExitCode {
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
    let (board, issue) = match options.board_with(number) {
        Ok(both) => both,
        Err(code) => return code,
    };
    let verdict = claim::decide_release(
        &board,
        number,
        options.agent(),
        options.claim_url.as_deref(),
    );
    if !verdict.is_ok() {
        return refused(&verdict);
    }
    let (holder, target_url, labelled) = if let Some(held) = board.lease(number) {
        (
            held.agent().to_string(),
            held.claim.url.clone(),
            held.labelled,
        )
    } else {
        let inc = board
            .trouble
            .iter()
            .find(|t| t.issue == number)
            .and_then(|t| t.incomplete.as_ref())
            .expect("decide_release checked lease or incomplete claim exists");
        let who = if inc.agent.is_empty() {
            options.agent().to_string()
        } else {
            inc.agent.clone()
        };
        (
            who,
            inc.url.clone(),
            issue.labels.iter().any(|l| l == lease::CLAIM_LABEL),
        )
    };
    let body = claim::release_comment(
        &holder,
        &target_url,
        options.agent(),
        options.note.as_deref(),
        now,
    );
    println!("要发的交还评论：\n{body}");
    if !options.writes() {
        println!("没有真的发评论、没有去掉标签（--dry-run 或 --replay）");
        return ExitCode::SUCCESS;
    }
    if let Err(code) = comment(options, number, &body) {
        return code;
    }
    match options.issue(number) {
        Ok(fresh) => match claim::read_lease(&fresh, &board.pushes) {
            Ok(now_held) => {
                println!(
                    "#{number} 上现在有一条新认领（{}，{}），{} 标签留着：\
                     上面那条交还评论只结束它点名的那条租约",
                    now_held.claim.url,
                    now_held.agent(),
                    lease::CLAIM_LABEL
                );
                return ExitCode::SUCCESS;
            }
            Err((claim::Problem::Unreadable, why)) => {
                eprintln!(
                    "交还评论发出去了，但重新读取 #{number} 时认领读不出来：{why}。\
                     为免藏起可能的新租约，{} 标签留着，请去 issue 上确认",
                    lease::CLAIM_LABEL
                );
                return ExitCode::from(2);
            }
            Err((claim::Problem::HandedBack, _)) => {}
        },
        Err(code) => return code,
    }
    if !labelled {
        println!("#{number} 本来就没有 {} 标签，不用去掉", lease::CLAIM_LABEL);
        return ExitCode::SUCCESS;
    }
    match label(options, number, Label::Remove(lease::CLAIM_LABEL)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
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
    let (board, issue) = match options.board_with(number) {
        Ok(both) => both,
        Err(code) => return code,
    };
    let verdict = claim::decide_reclaim(&board, number, now);
    if !verdict.is_ok() {
        return refused(&verdict);
    }
    let (target_url, labelled, body) = if let Some(held) = board.lease(number) {
        (
            held.claim.url.clone(),
            held.labelled,
            claim::reclaim_comment(held, options.agent(), now),
        )
    } else {
        let inc = board
            .trouble
            .iter()
            .find(|t| t.issue == number)
            .and_then(|t| t.incomplete.as_ref())
            .expect("a reclaimable lease or incomplete claim exists");
        (
            inc.url.clone(),
            issue.labels.iter().any(|l| l == lease::CLAIM_LABEL),
            claim::reclaim_incomplete_comment(inc, options.agent(), now),
        )
    };
    println!("要发的收回评论：\n{body}");
    if !options.writes() {
        println!("没有真的发评论、没有去掉标签（--dry-run 或 --replay）");
        return ExitCode::SUCCESS;
    }
    if let Err(code) = comment(options, number, &body) {
        return code;
    }
    // The snapshot this decided on may be minutes old, and the holder or
    // somebody else may have claimed or refreshed the issue in between.
    // Taking the label off then would hide a lease that is seconds old,
    // so the label comes off only if the claim being handed back was
    // actually ended and no lease holds the issue now.
    match options.issue(number) {
        Ok(fresh) => match claim::read_lease(&fresh, &board.pushes) {
            Ok(now_held) if now_held.claim.url == target_url => {
                eprintln!(
                    "拒绝 — 收回评论发出之前，{} 刚刚更新了 #{number} 的租约（{}，最后更新：{}），\
                     这条租约其实还没过期，这次收回作废；{} 标签留着",
                    now_held.agent(),
                    now_held.claim.url,
                    now_held.last.sign.what(),
                    lease::CLAIM_LABEL
                );
                return ExitCode::FAILURE;
            }
            Ok(now_held) => {
                println!(
                    "#{number} 上现在有一条认领（{}，{}），{} 标签留着：\
                         上面那条收回评论只结束它点名的旧租约",
                    now_held.claim.url,
                    now_held.agent(),
                    lease::CLAIM_LABEL
                );
                return ExitCode::SUCCESS;
            }
            Err((claim::Problem::Unreadable, why)) => {
                eprintln!(
                    "收回评论发出去了，但重新读取 #{number} 时认领读不出来：{why}。\
                         为免藏起可能的新租约，{} 标签留着，请去 issue 上确认",
                    lease::CLAIM_LABEL
                );
                return ExitCode::from(2);
            }
            Err((claim::Problem::HandedBack, _)) => {}
        },
        Err(code) => return code,
    }
    if !labelled {
        println!("#{number} 本来就没有 {} 标签，不用去掉", lease::CLAIM_LABEL);
        return ExitCode::SUCCESS;
    }
    match label(options, number, Label::Remove(lease::CLAIM_LABEL)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

/// The label change a command makes after its comment, if any. A
/// heartbeat changes no label: the issue is already claimed.
enum Label {
    Add(&'static str),
    Remove(&'static str),
}

/// Posts a comment and returns its link. The comment goes out before any
/// label changes: the comment is the lease, and the label only decides
/// what `gh issue list` can filter on.
fn comment(options: &Options, number: u64, body: &str) -> Result<String, ExitCode> {
    match snapshot::comment(options.repo(), number, body) {
        Ok(out) => {
            print!("{out}");
            Ok(out.trim().to_string())
        }
        Err(e) => {
            eprintln!("{e}");
            Err(ExitCode::from(2))
        }
    }
}

fn label(options: &Options, number: u64, which: Label) -> Result<(), ExitCode> {
    let (done, result) = match which {
        Label::Add(l) => (
            format!("#{number} 加上了 {l} 标签"),
            snapshot::add_label(options.repo(), number, l),
        ),
        Label::Remove(l) => (
            format!("#{number} 去掉了 {l} 标签"),
            snapshot::remove_label(options.repo(), number, l),
        ),
    };
    match result {
        Ok(_) => {
            println!("{done}");
            Ok(())
        }
        Err(e) => {
            eprintln!(
                "{e}\n评论已经发出去了，标签没改成。评论才是租约，所以它不会丢：\
                 别的 agent 的 `claims claim` 会读这个 issue 自己的评论，看到这条租约；\
                 `claims heartbeat --issue {number} --agent {}` 会把标签补上",
                options.agent()
            );
            Err(ExitCode::from(2))
        }
    }
}

fn refused(verdict: &claim::Verdict) -> ExitCode {
    if let claim::Verdict::Refuse(why) = verdict {
        println!("拒绝 — {why}");
    }
    ExitCode::FAILURE
}
