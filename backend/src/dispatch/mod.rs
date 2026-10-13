//! 调度程序：GitHub 上有事才启动一个全新的 agent 会话。
//!
//! The dispatcher itself calls no model. It reads GitHub ([`snapshot`]),
//! compares it with what it already started ([`state`]), and decides
//! ([`plan`]) which agent sessions to start. Everything that makes a
//! decision is a pure function of a snapshot, the state, the settings and
//! the current time, so a test replays saved GitHub responses and checks
//! the decision without starting an agent or reaching the network.
//!
//! 规则和运行路径见 docs/subsystems/dispatch.md，为什么这样见
//! docs/decisions/0010-dispatcher.md。

pub mod config;
pub mod runner;
pub mod service;
pub mod snapshot;
pub mod state;

#[cfg(test)]
mod tests;

use chrono::{DateTime, FixedOffset, Utc};

use config::Config;
use snapshot::{MainChecks, Snapshot};
use state::{Outcome, Session, State};

/// The label an agent carries while it works on an issue, the same one
/// `scripts/claims` sets.
pub const CLAIM_LABEL: &str = "认领中";
pub const REVIEW_LABEL: &str = "待审查";
pub const FIX_PULL_LABEL: &str = "要修改";
pub const ACCEPT_LABEL: &str = "待验收";
pub const FIX_ISSUE_LABEL: &str = "验收未通过";
/// Labels that mean an issue is in someone's hands already, so it is not
/// fresh work to implement.
const BUSY_LABELS: [&str; 3] = [CLAIM_LABEL, ACCEPT_LABEL, FIX_ISSUE_LABEL];

/// A session that failed or timed out is tried once more, and then left
/// alone: a dispatcher that keeps restarting the same failing session
/// burns the daily ceiling on one issue.
pub const MAX_ATTEMPTS: usize = 2;

/// 北京时间，项目里所有时钟都用它（`report::beijing_time`）：owner 的机器
/// 和大麦都在 +08:00，定时任务写的 `03:00` 是北京时间的三点。
pub fn beijing() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("+08:00 是合法的时区偏移")
}

/// 现在。项目编译 chrono 时没有开 `clock`，时间一律从时间戳来
/// （`watch::Env::wall`），所以这里也从系统时钟的时间戳来。
pub fn now() -> DateTime<Utc> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

/// A time for a log line: `2026-10-14 04:30:00`, Beijing time.
pub fn stamp(at: DateTime<Utc>) -> String {
    at.with_timezone(&beijing())
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// What a session is started to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// An open issue nobody has claimed.
    Implement,
    /// A pull request labelled `待审查`.
    Review,
    /// A pull request labelled `要修改`.
    FixPull,
    /// An issue labelled `待验收`.
    Accept,
    /// An issue labelled `验收未通过`.
    FixIssue,
    /// main's checks are red.
    FixCi,
    /// A job that runs on a clock, named by the settings.
    Scheduled(String),
}

impl Role {
    pub fn name(&self) -> &str {
        match self {
            Role::Implement => "implement",
            Role::Review => "review",
            Role::FixPull => "fix-pr",
            Role::Accept => "accept",
            Role::FixIssue => "fix-issue",
            Role::FixCi => "fix-ci",
            Role::Scheduled(name) => name,
        }
    }

    /// Whether the role must go to a vendor other than the one that wrote
    /// the change. Reviewing and accepting your own work is what the
    /// project's rules forbid, so it is the dispatcher's job to enforce it.
    fn needs_another_vendor(&self) -> bool {
        matches!(self, Role::Review | Role::Accept)
    }

    /// Whether the role must go to the same vendor that wrote the change,
    /// because it is that change's author who has to fix it.
    fn needs_same_vendor(&self) -> bool {
        matches!(self, Role::FixPull | Role::FixIssue)
    }
}

/// A session the dispatcher wants to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// Stable id: the same event always produces the same id, and a new
    /// commit or a new round produces a new one.
    pub id: String,
    pub role: Role,
    pub vendor: String,
    pub issue: Option<u64>,
    pub pull: Option<u64>,
    /// What the session is told to do, in one line for the log and the
    /// issue comment.
    pub why: String,
}

impl Task {
    /// The piece of work this task is about. Two tasks with the same
    /// subject never run at the same time, so one issue never has two
    /// sessions working on it.
    pub fn subject(&self) -> Option<u64> {
        self.issue.or(self.pull)
    }
}

/// Why a task the dispatcher saw was not started now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Handled in the cloud (#9), so this machine leaves it alone.
    Cloud,
    /// Another session is working on the same issue or pull request.
    Busy(String),
    /// It was already dispatched, and retries are used up.
    Done(String),
    /// Who wrote the change cannot be told from the issue or the pull
    /// request, so the dispatcher cannot pick a different vendor than the
    /// implementer and refuses to guess.
    UnknownImplementer,
    /// No agent command line for the vendor this task must go to.
    NoAgent(String),
    /// Today's ceiling is used up.
    DailyCap(usize),
    /// Too many sessions are already running.
    Concurrency(usize),
}

impl Skip {
    pub fn why(&self) -> String {
        match self {
            Skip::Cloud => "按 #9 的裁决归云端，本机不重复派".to_string(),
            Skip::Busy(id) => format!("同一个 issue 已经有会话 {id} 在做"),
            Skip::Done(id) => format!("已经派过 {id}，重试次数用完了"),
            Skip::UnknownImplementer => "看不出是哪个 agent 实现的，不猜".to_string(),
            Skip::NoAgent(vendor) => format!("没配 {vendor} 的命令行"),
            Skip::DailyCap(cap) => format!("今天已经派了 {cap} 个会话，到上限了"),
            Skip::Concurrency(cap) => format!("已经有 {cap} 个会话在跑，排队"),
        }
    }

    /// Whether the issue should be told about it. A ceiling the owner set
    /// is worth a comment, because the work is waiting on it; the others
    /// are routine and would turn into a comment every minute.
    pub fn worth_explaining(&self) -> bool {
        matches!(self, Skip::DailyCap(_))
    }
}

/// What the dispatcher is about to do: start these sessions, leave those
/// alone, and tidy up after sessions that ended.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub start: Vec<Task>,
    pub skip: Vec<(Task, Skip)>,
    pub sweep: Vec<Sweep>,
}

/// A session that is no longer running, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sweep {
    pub id: String,
    pub issue: Option<u64>,
    pub outcome: Outcome,
    /// Minutes the session ran before it was found like this.
    pub after_minutes: i64,
    pub log: std::path::PathBuf,
    /// Whether the claim on the issue has to be released: a session that
    /// timed out is not coming back, and leaving its lease in place blocks
    /// every other agent for two hours (see the claim lease).
    pub release_claim: bool,
}

/// Everything the dispatcher should do right now.
pub fn plan(snapshot: &Snapshot, state: &State, config: &Config, now: DateTime<Utc>) -> Plan {
    let sweep = sweep(state, config, now);
    // A session found dead in this tick frees its slot right away, so the
    // next task does not wait a whole interval for nothing.
    let ended: Vec<&str> = sweep.iter().map(|s| s.id.as_str()).collect();
    let running: Vec<&Session> = state
        .running()
        .filter(|s| !ended.contains(&s.id.as_str()))
        .collect();

    let mut plan = Plan {
        sweep,
        ..Plan::default()
    };
    let mut started = state.started_today(now);
    let mut busy: Vec<(Option<u64>, String)> = running
        .iter()
        .map(|s| (s.issue.or(s.pull), s.id.clone()))
        .collect();
    let mut live = running.len();

    for candidate in candidates(snapshot, state, config, now) {
        let task = match candidate {
            Candidate::Start(task) => task,
            Candidate::Skip(task, skip) => {
                plan.skip.push((task, skip));
                continue;
            }
        };
        if let Some((_, id)) = busy
            .iter()
            .find(|(subject, _)| subject.is_some() && *subject == task.subject())
        {
            plan.skip.push((task, Skip::Busy(id.clone())));
            continue;
        }
        if started >= config.max_sessions_per_day {
            plan.skip
                .push((task, Skip::DailyCap(config.max_sessions_per_day)));
            continue;
        }
        if live >= config.max_sessions {
            plan.skip
                .push((task, Skip::Concurrency(config.max_sessions)));
            continue;
        }
        busy.push((task.subject(), task.id.clone()));
        started += 1;
        live += 1;
        plan.start.push(task);
    }
    plan
}

/// One session the snapshot or the clock asks for: either a task, or the
/// task and the reason it is not started.
enum Candidate {
    Start(Task),
    Skip(Task, Skip),
}

/// One session before the dispatcher knows whether it can start it.
struct Wanted {
    role: Role,
    issue: Option<u64>,
    pull: Option<u64>,
    /// The vendor already attached to the work: the implementer of the
    /// change, or the vendor a scheduled job names.
    known: Option<String>,
    why: String,
}

/// Every session the snapshot and the clock ask for, in the order the
/// project cares about: a red main first (nothing may be stacked on it),
/// then the clock, then work already in flight, and only then new work.
fn candidates(
    snapshot: &Snapshot,
    state: &State,
    config: &Config,
    now: DateTime<Utc>,
) -> Vec<Candidate> {
    let mut wanted = Vec::new();

    if snapshot.main_checks == MainChecks::Red {
        wanted.push(Wanted {
            role: Role::FixCi,
            issue: None,
            pull: None,
            known: None,
            why: "main 的检查是红的，先修红".to_string(),
        });
    }

    for job in &config.scheduled {
        if is_due(job, state, now) {
            wanted.push(Wanted {
                role: Role::Scheduled(job.name.clone()),
                issue: None,
                pull: None,
                known: Some(job.vendor.clone()),
                why: job.instruction.clone(),
            });
        }
    }

    for pull in &snapshot.pulls {
        if !allowed(&pull.labels, config) {
            continue;
        }
        let role = if pull.labels.iter().any(|l| l == FIX_PULL_LABEL) {
            Role::FixPull
        } else if pull.labels.iter().any(|l| l == REVIEW_LABEL) {
            Role::Review
        } else {
            continue;
        };
        let why = match role {
            Role::FixPull => format!(
                "PR #{} 被要求修改（head {}）",
                pull.number,
                short(&pull.head_sha)
            ),
            _ => format!(
                "PR #{} 等独立审查（head {}）",
                pull.number,
                short(&pull.head_sha)
            ),
        };
        wanted.push(Wanted {
            role,
            // The issue the pull request is for, so a review and the
            // issue's own session never run at the same time.
            issue: pull.issue.filter(|n| !config.ignore_issues.contains(n)),
            pull: Some(pull.number),
            known: vendor_of(pull.agent.as_deref(), config),
            why,
        });
    }

    for issue in &snapshot.issues {
        if config.ignore_issues.contains(&issue.number) || !allowed(&issue.labels, config) {
            continue;
        }
        let has = |label: &str| issue.labels.iter().any(|l| l == label);
        let (role, why) = if has(FIX_ISSUE_LABEL) {
            (
                Role::FixIssue,
                format!("#{} 验收未通过，等实现者修", issue.number),
            )
        } else if has(ACCEPT_LABEL) {
            (Role::Accept, format!("#{} 等另一个厂商验收", issue.number))
        } else if !BUSY_LABELS.iter().any(|l| has(l)) {
            (
                Role::Implement,
                format!("#{} 还没人认领：{}", issue.number, issue.title),
            )
        } else {
            continue;
        };
        wanted.push(Wanted {
            role,
            issue: Some(issue.number),
            pull: None,
            // An issue assigned to one agent by label only goes to that
            // agent; otherwise the agent that last spoke on it is its
            // implementer.
            known: assigned_vendor(&issue.labels, config)
                .or_else(|| vendor_of(issue.agent.as_deref(), config)),
            why,
        });
    }

    wanted
        .into_iter()
        .map(|w| build(w, state, config, now))
        .collect()
}

/// Turns one wanted session into a task, or into the reason it cannot be
/// one: handled in the cloud, already dispatched, or no vendor the rules
/// allow.
fn build(wanted: Wanted, state: &State, config: &Config, now: DateTime<Utc>) -> Candidate {
    let Wanted {
        role,
        issue,
        pull,
        known,
        why,
    } = wanted;
    let key = match (&role, issue, pull) {
        (Role::Review | Role::FixPull, _, Some(pull)) => format!("{}-{pull}", role.name()),
        (_, Some(issue), _) => format!("{}-{issue}", role.name()),
        _ => role.name().to_string(),
    };
    let attempt = state
        .sessions
        .iter()
        .filter(|s| s.id.starts_with(&format!("{key}-a")))
        .count();
    let mut task = Task {
        id: format!("{key}-a{}", attempt + 1),
        role: role.clone(),
        vendor: known.clone().unwrap_or_default(),
        issue,
        pull,
        why,
    };

    if config.in_cloud(&role) {
        return Candidate::Skip(task, Skip::Cloud);
    }
    // A session that was started and ended is not started again, except
    // once more after a failure or a timeout: a dispatcher that keeps
    // restarting the same failing session burns the daily ceiling on one
    // issue.
    if attempt > 0 {
        let previous = format!("{key}-a{attempt}");
        let retriable = state
            .session(&previous)
            .and_then(|s| s.outcome)
            .is_some_and(|o| matches!(o, Outcome::Failed | Outcome::TimedOut | Outcome::Vanished));
        if attempt >= MAX_ATTEMPTS || !retriable {
            return Candidate::Skip(task, Skip::Done(previous));
        }
    }

    let vendor = if role.needs_same_vendor() {
        match known {
            Some(implementer) => implementer,
            None => return Candidate::Skip(task, Skip::UnknownImplementer),
        }
    } else if role.needs_another_vendor() {
        let Some(implementer) = known else {
            return Candidate::Skip(task, Skip::UnknownImplementer);
        };
        match least_busy(config, state, Some(&implementer), now) {
            Some(vendor) => vendor,
            None => {
                let why = Skip::NoAgent(format!("{implementer} 以外的厂商"));
                return Candidate::Skip(task, why);
            }
        }
    } else {
        match known.or_else(|| least_busy(config, state, None, now)) {
            Some(vendor) => vendor,
            None => return Candidate::Skip(task, Skip::NoAgent("任何厂商".to_string())),
        }
    };
    if !config.agents.contains_key(&vendor) {
        task.vendor = vendor.clone();
        return Candidate::Skip(task, Skip::NoAgent(vendor));
    }
    task.vendor = vendor;
    Candidate::Start(task)
}

/// The configured vendor with the fewest sessions today, skipping `avoid`.
/// Ties go to the order in the settings, so the choice is the same on
/// every machine and in every test.
fn least_busy(
    config: &Config,
    state: &State,
    avoid: Option<&str>,
    now: DateTime<Utc>,
) -> Option<String> {
    let today = state::local_day(now);
    config
        .vendors()
        .into_iter()
        .filter(|v| Some(v.as_str()) != avoid)
        .min_by_key(|vendor| {
            state
                .sessions
                .iter()
                .filter(|s| &s.vendor == vendor && state::local_day(s.started_at) == today)
                .count()
        })
}

/// The vendor behind an `Agent:` signature: `Claude（第 4 条线）` is the
/// vendor `claude`.
fn vendor_of(signature: Option<&str>, config: &Config) -> Option<String> {
    let signature = signature?.to_lowercase();
    config
        .vendors()
        .into_iter()
        .find(|v| signature == *v || signature.starts_with(v.as_str()))
}

/// The vendor an `指派:<agent>` label names.
fn assigned_vendor(labels: &[String], config: &Config) -> Option<String> {
    for label in labels {
        if let Some(name) = label.strip_prefix("指派:").or(label.strip_prefix("指派："))
            && let Some(vendor) = vendor_of(Some(name.trim()), config)
        {
            return Some(vendor);
        }
    }
    None
}

/// Whether the settings allow the dispatcher to touch this piece of work.
/// A fresh install requires a demo label, so it cannot pick up issues
/// other agents are already working on.
fn allowed(labels: &[String], config: &Config) -> bool {
    config.require_label.is_empty() || labels.contains(&config.require_label)
}

/// Whether a daily job is due: its time of day has passed and it has not
/// run today.
fn is_due(job: &config::Scheduled, state: &State, now: DateTime<Utc>) -> bool {
    let Some(at) = config::parse_hhmm(&job.at) else {
        return false;
    };
    let minutes = now.with_timezone(&beijing()).format("%H:%M").to_string();
    let Some(current) = config::parse_hhmm(&minutes) else {
        return false;
    };
    let today = state::local_day(now);
    current >= at && state.scheduled_ran.get(&job.name) != Some(&today)
}

/// The sessions that are no longer running, and the ones that ran too
/// long. A session over the ceiling is stopped, its claim released and
/// the issue told, because nobody else is watching for it.
fn sweep(state: &State, config: &Config, now: DateTime<Utc>) -> Vec<Sweep> {
    let mut out = Vec::new();
    for session in state.running() {
        let minutes = session.minutes_since_start(now);
        let timed_out = minutes >= config.session_timeout_minutes;
        let alive = session.process_alive();
        if alive && !timed_out {
            continue;
        }
        out.push(Sweep {
            id: session.id.clone(),
            issue: session.issue,
            outcome: if timed_out {
                Outcome::TimedOut
            } else {
                Outcome::Vanished
            },
            after_minutes: minutes,
            log: session.log.clone(),
            release_claim: timed_out,
        });
    }
    out
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// What the session is told to do. The dispatcher calls no model, so the
/// prompt is a fixed line per role that points at the rules already in
/// the repository instead of repeating them.
pub fn instruction(task: &Task, config: &Config, worktree: &std::path::Path) -> String {
    let repo = &config.repo;
    let what = match &task.role {
        Role::Implement => format!(
            "按 AGENTS.md 的会话流程实现 issue #{}：先 scripts/claims claim 认领，复现，写一个会失败的测试，再修。",
            task.issue.unwrap_or_default()
        ),
        Role::Review => format!(
            "按 docs/code-standards.md 的“合并”一节独立审查 PR #{}：把它合到最新 main 上跑 make check，故意弄坏它守护的代码，在 PR 上写审查记录，然后决定合并还是换成 `要修改`。",
            task.pull.unwrap_or_default()
        ),
        Role::FixPull => format!(
            "PR #{} 的审查要求修改：读 PR 上最新的审查记录，逐条修，重新跑 make check，然后重新加 `待审查`。",
            task.pull.unwrap_or_default()
        ),
        Role::Accept => format!(
            "按 prompts/acceptance.md 独立验收 issue #{}：每条验收标准自己重跑一遍，通过就关闭 issue，不通过就换成 `验收未通过` 并写明问题。",
            task.issue.unwrap_or_default()
        ),
        Role::FixIssue => format!(
            "issue #{} 验收未通过：读验收结论，逐条修，重新跑 make check，然后重新加 `待验收`。",
            task.issue.unwrap_or_default()
        ),
        Role::FixCi => "main 的检查是红的。按 prompts/work-tick.md 先修红：找到失败的 run，在原因处修，不要绕过检查。".to_string(),
        Role::Scheduled(_) => task.why.clone(),
    };
    format!(
        "你是 {vendor}，由 theFlash 的调度程序启动的一个全新会话，任务编号 {id}。\n\
         仓库 {repo}，你的 worktree 是 {worktree}（已经 checkout 到 origin/main）。只在这个目录里改文件。\n\
         开工前读 AGENTS.md、LESSONS.md、docs/code-standards.md 和你要改的子系统文档。不要问人，决定自己做并写成决定记录。\n\n\
         要做的事：{what}\n\n\
         规则：遵守 AGENTS.md 的大麦安全规则（不登录、不下单、凭据不进仓库）；commit 和 PR 正文最后一行写 `Agent: {vendor}`；\
         不要合并自己的 PR，不要关闭自己的 issue；至少每 30 分钟 scripts/claims heartbeat 一次或推一次分支；\
         这个会话最多跑 {timeout} 分钟，超时会被停掉并释放认领，所以进度要及时推到分支和 issue 上。\n\
         为什么派给你：{why}",
        vendor = task.vendor,
        id = task.id,
        worktree = worktree.display(),
        timeout = config.session_timeout_minutes,
        why = task.why,
    )
}
