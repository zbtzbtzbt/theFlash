//! 认领租约：谁在做哪个 issue、要改哪些模块、什么时候过期（#16）。
//!
//! A claim used to be prose in a comment, so a session that died left an
//! issue looking busy forever, and two agents could only find out they
//! were editing the same files by reading each other's comments. A claim
//! is now a lease: the `认领中` label plus a claim comment that names the
//! agent, the modules it will change and when it expects to be done. The
//! lease stays alive while the agent comments or pushes, and after
//! [`lease::EXPIRES_AFTER_MINUTES`] of silence anyone may take it over.
//!
//! Nothing here calls a model. Every answer comes from GitHub's own
//! responses ([`Snapshot`]) and the lease format ([`lease`]), so the
//! dispatcher and an agent reading the same issue reach the same verdict.

pub mod lease;
pub mod snapshot;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, FixedOffset, Utc};

pub use lease::{Claim, Heartbeat};
pub use snapshot::{Issue, Snapshot};

/// What kept a lease alive most recently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sign {
    /// Nothing since the claim itself.
    Claim,
    Heartbeat {
        url: String,
        marked: bool,
    },
    Push {
        branch: String,
    },
}

impl Sign {
    pub fn what(&self) -> String {
        match self {
            Sign::Claim => "认领评论".to_string(),
            Sign::Heartbeat { marked: true, .. } => "心跳评论".to_string(),
            Sign::Heartbeat { marked: false, .. } => "进度评论".to_string(),
            Sign::Push { branch } => format!("推送 {branch}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub at: DateTime<Utc>,
    pub sign: Sign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Heard from within the heartbeat interval.
    Active,
    /// Silent longer than the heartbeat interval, not yet expired.
    HeartbeatOverdue,
    /// Silent long enough that the agent is assumed gone.
    Expired,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Active => "活跃",
            Status::HeartbeatOverdue => "该心跳了",
            Status::Expired => "已过期",
        }
    }
}

/// One issue's claim, with everything needed to decide whether it still
/// holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub issue: u64,
    pub title: String,
    pub claim: Claim,
    /// The agent the `指派:<agent>` label reserves the issue for, if any.
    pub assigned_to: Option<String>,
    pub last: Activity,
}

impl Lease {
    pub fn agent(&self) -> &str {
        &self.claim.agent
    }

    pub fn idle_minutes(&self, now: DateTime<Utc>) -> i64 {
        (now - self.last.at).num_minutes().max(0)
    }

    pub fn status(&self, now: DateTime<Utc>) -> Status {
        let idle = self.idle_minutes(now);
        if idle >= lease::EXPIRES_AFTER_MINUTES {
            Status::Expired
        } else if idle >= lease::HEARTBEAT_EVERY_MINUTES {
            Status::HeartbeatOverdue
        } else {
            Status::Active
        }
    }

    /// Minutes until this lease expires, or 0 once it has.
    pub fn expires_in_minutes(&self, now: DateTime<Utc>) -> i64 {
        (lease::EXPIRES_AFTER_MINUTES - self.idle_minutes(now)).max(0)
    }
}

/// An issue that claims to be claimed but whose lease cannot be read, so
/// the board reports it instead of quietly treating it as free or as
/// taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trouble {
    pub issue: u64,
    pub title: String,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Board {
    /// Readable leases, lowest issue number first.
    pub leases: Vec<Lease>,
    pub trouble: Vec<Trouble>,
}

impl Board {
    pub fn lease(&self, issue: u64) -> Option<&Lease> {
        self.leases.iter().find(|l| l.issue == issue)
    }

    /// The leases that still hold, so work being planned avoids them.
    pub fn holding(&self, now: DateTime<Utc>) -> impl Iterator<Item = &Lease> {
        self.leases
            .iter()
            .filter(move |l| l.status(now) != Status::Expired)
    }
}

/// Every claim in the snapshot. An issue contributes either a lease or a
/// [`Trouble`], never both and never nothing: an issue labelled `认领中`
/// that the board stayed silent about is an issue nobody would look at
/// again.
pub fn board(snapshot: &Snapshot) -> Board {
    let mut out = Board::default();
    for issue in &snapshot.issues {
        match read_lease(issue, &snapshot.pushes) {
            Ok(lease) => out.leases.push(lease),
            Err(why) => out.trouble.push(Trouble {
                issue: issue.number,
                title: issue.title.clone(),
                why,
            }),
        }
    }
    out.leases.sort_by_key(|l| l.issue);
    out.trouble.sort_by_key(|t| t.issue);
    out
}

/// The lease an issue's comments describe, or why it cannot be read.
///
/// The newest claim comment wins, so an issue claimed, handed back and
/// claimed again reads as the latest claim. A `收回认领` comment after the
/// claim ends it: the label may still be on the issue for a moment, and
/// the lease must not outlive the hand-back.
pub fn read_lease(
    issue: &Issue,
    pushes: &BTreeMap<String, DateTime<Utc>>,
) -> Result<Lease, String> {
    let claim = issue
        .comments
        .iter()
        .rev()
        .find_map(|c| lease::parse_claim(&c.body, &c.url, c.posted_at))
        .ok_or_else(|| {
            format!(
                "标着 {} 但没有认领评论：没法知道是哪个 agent、在改哪些模块。\
                 让持有者补一条认领评论（格式见 docs/subsystems/claim-lease.md），\
                 或者在 issue 上问清楚之后手动去掉标签",
                lease::CLAIM_LABEL
            )
        })?;
    if let Some(field) = claim.missing_field() {
        return Err(format!(
            "认领评论缺字段 `{field}`（{}）：补上之后才知道{}",
            claim.url,
            if field == "agent" {
                "是哪个 agent 在做"
            } else {
                "它会改哪些模块，别人没法避开"
            }
        ));
    }
    if issue
        .comments
        .iter()
        .any(|c| c.posted_at > claim.posted_at && lease::is_reclaim(&c.body))
    {
        return Err(format!(
            "最后一条认领（{}）之后有人收回了它，但 {} 标签还在：没有人在做，去掉标签",
            claim.url,
            lease::CLAIM_LABEL
        ));
    }
    let mut last = Activity {
        at: claim.posted_at,
        sign: Sign::Claim,
    };
    for comment in &issue.comments {
        if comment.posted_at <= claim.posted_at {
            continue;
        }
        let Some(beat) = lease::parse_heartbeat(&comment.body, &comment.url, comment.posted_at)
        else {
            continue;
        };
        if lease::same_agent(&beat.agent, &claim.agent) && beat.posted_at > last.at {
            last = Activity {
                at: beat.posted_at,
                sign: Sign::Heartbeat {
                    url: beat.url,
                    marked: beat.marked,
                },
            };
        }
    }
    if let Some(branch) = &claim.branch
        && let Some(at) = pushes.get(branch.as_str())
        && *at > last.at
    {
        last = Activity {
            at: *at,
            sign: Sign::Push {
                branch: branch.clone(),
            },
        };
    }
    Ok(Lease {
        issue: issue.number,
        title: issue.title.clone(),
        assigned_to: lease::assigned_agent(&issue.labels),
        claim,
        last,
    })
}

/// Another issue's lease covering some of the modules a new claim wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlap {
    pub issue: u64,
    pub agent: String,
    pub modules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The claim may be posted. `overlaps` is what it must say it knows
    /// about, which is empty unless the agent passed a reason.
    Ok {
        overlaps: Vec<Overlap>,
    },
    Refuse(String),
}

impl Verdict {
    pub fn is_ok(&self) -> bool {
        matches!(self, Verdict::Ok { .. })
    }
}

/// Whether `agent` may claim `issue` for `modules`, given the claims that
/// already hold. Refusals are ordered from the most specific reason to
/// the most general, so the agent is told the one thing to do next.
///
/// `overlap_reason` is how the agent says it already knows another claim
/// covers the same modules; without it, an overlap is a refusal, because
/// two agents editing the same files is exactly what #16 exists to stop.
pub fn decide_claim(
    board: &Board,
    issue: &Issue,
    agent: &str,
    modules: &[String],
    overlap_reason: Option<&str>,
    now: DateTime<Utc>,
) -> Verdict {
    if agent.trim().is_empty() {
        return Verdict::Refuse("没说是哪个 agent：加上 --agent <名字>".to_string());
    }
    if modules.is_empty() {
        return Verdict::Refuse(
            "没说要改哪些模块：加上 --modules <路径,路径>，别人靠这个避开你".to_string(),
        );
    }
    if let Some(owner) = lease::assigned_agent(&issue.labels)
        && !lease::same_agent(&owner, agent)
    {
        return Verdict::Refuse(format!(
            "#{} 带着 {}{owner} 标签，只能由 {owner} 认领",
            issue.number,
            lease::ASSIGNED_PREFIX
        ));
    }
    if let Some(held) = board.lease(issue.number) {
        let status = held.status(now);
        if lease::same_agent(held.agent(), agent) && status != Status::Expired {
            return Verdict::Refuse(format!(
                "#{} 已经是你（{}）的认领，{}前最后更新（{}）。\
                 报进度用 `claims heartbeat --issue {} --agent {agent}`",
                issue.number,
                held.agent(),
                minutes(held.idle_minutes(now)),
                held.last.sign.what(),
                issue.number,
            ));
        }
        if status != Status::Expired {
            return Verdict::Refuse(format!(
                "#{} 由 {} 认领中（{}，{}前最后更新），改 {}。\
                 等它做完，或者在两个 issue 上写明怎么分工；它要到 {}后才算过期",
                issue.number,
                held.agent(),
                status.name(),
                minutes(held.idle_minutes(now)),
                held.claim.modules.join("、"),
                minutes(held.expires_in_minutes(now)),
            ));
        }
        return Verdict::Refuse(format!(
            "#{} 上 {} 的认领已经过期（{} 没有更新），但标签还在。\
             先 `claims reclaim --issue {} --agent {agent}` 收回，再认领",
            issue.number,
            held.agent(),
            minutes(held.idle_minutes(now)),
            issue.number,
        ));
    }
    let overlaps: Vec<Overlap> = board
        .holding(now)
        .filter(|l| l.issue != issue.number)
        .filter_map(|l| {
            let shared = lease::overlap(&l.claim.modules, modules);
            (!shared.is_empty()).then(|| Overlap {
                issue: l.issue,
                agent: l.claim.agent.clone(),
                modules: shared,
            })
        })
        .collect();
    match overlap_reason {
        Some(reason) if !reason.trim().is_empty() => Verdict::Ok { overlaps },
        _ if overlaps.is_empty() => Verdict::Ok { overlaps },
        _ => Verdict::Refuse(format!(
            "要改的模块和进行中的认领重叠：{}。\
             排在它后面，或者在两个 issue 上写明怎么分工，\
             然后用 --overlap <怎么分工> 再来一次",
            overlaps
                .iter()
                .map(|o| format!("#{}（{}）的 {}", o.issue, o.agent, o.modules.join("、")))
                .collect::<Vec<_>>()
                .join("；")
        )),
    }
}

/// Whether `agent` may take over issue `number`. Only an expired lease
/// may be taken: a lease that is merely late gets a nudge, not a
/// hand-over, or tonight's agents would reclaim each other's work.
pub fn decide_reclaim(board: &Board, number: u64, now: DateTime<Utc>) -> Verdict {
    let Some(held) = board.lease(number) else {
        if let Some(t) = board.trouble.iter().find(|t| t.issue == number) {
            return Verdict::Refuse(format!("#{number} 的认领读不出来：{}", t.why));
        }
        return Verdict::Refuse(format!(
            "#{number} 上没有进行中的认领（没有 {} 标签，或者标签不在打开的 issue 上），没有什么要收回",
            lease::CLAIM_LABEL
        ));
    };
    match held.status(now) {
        Status::Expired => Verdict::Ok { overlaps: vec![] },
        status => Verdict::Refuse(format!(
            "#{number} 由 {} 认领，{}（{}前最后更新：{}），还没过期，不能收回。\
             再等 {}，它才算过期",
            held.agent(),
            status.name(),
            minutes(held.idle_minutes(now)),
            held.last.sign.what(),
            minutes(held.expires_in_minutes(now)),
        )),
    }
}

// ---- the comments the commands post ----

/// The claim comment, in the format [`lease::parse_claim`] reads back.
pub fn claim_comment(
    agent: &str,
    modules: &[String],
    branch: Option<&str>,
    due: Option<DateTime<Utc>>,
    overlaps: &[Overlap],
    overlap_reason: Option<&str>,
    now: DateTime<Utc>,
) -> String {
    let mut body = format!(
        "## {}\n\n- agent: {agent}\n- 开始时间: {}\n- 改动模块: {}\n",
        lease::CLAIM_MARKER,
        stamp(now),
        modules.join(", "),
    );
    if let Some(due) = due {
        body.push_str(&format!("- 预计完成: {}\n", stamp(due)));
    }
    if let Some(branch) = branch.filter(|b| !b.is_empty()) {
        body.push_str(&format!("- 分支: {branch}\n"));
    }
    if let (Some(reason), false) = (overlap_reason, overlaps.is_empty()) {
        body.push_str(&format!(
            "- 重叠: 和 {} 重叠；{}\n",
            overlaps
                .iter()
                .map(|o| format!("#{}（{}）的 {}", o.issue, o.agent, o.modules.join("、")))
                .collect::<Vec<_>>()
                .join("；"),
            reason.trim()
        ));
    }
    body.push_str(&format!(
        "\n心跳至少每 {} 分钟一次（`scripts/claims heartbeat` 或推一次分支）；\
         超过 {} 分钟没有更新，这条认领就过期，别的 agent 可以收回（#16）。\n\nAgent: {agent}\n",
        lease::HEARTBEAT_EVERY_MINUTES,
        lease::EXPIRES_AFTER_MINUTES,
    ));
    body
}

/// The heartbeat comment, which [`lease::parse_heartbeat`] reads back as
/// a sign of life for this agent.
pub fn heartbeat_comment(agent: &str, note: &str, now: DateTime<Utc>) -> String {
    format!(
        "## {}\n\n- agent: {agent}\n- 时间: {}\n- 进度: {}\n\nAgent: {agent}\n",
        lease::HEARTBEAT_MARKER,
        stamp(now),
        note.trim(),
    )
}

/// The hand-back comment, which says what was taken over and on what
/// evidence, so the agent that comes back knows why its issue moved.
pub fn reclaim_comment(lease: &Lease, by: &str, now: DateTime<Utc>) -> String {
    format!(
        "## {}\n\n- 原认领: {}（{}）\n- 最后更新: {}（{}，{}前）\n\
         - 收回时间: {}\n- 收回者: {by}\n- 为什么: 超过 {} 分钟没有更新，按 #16 的租约规则视为过期；\
         已去掉 {} 标签，这个 issue 可以被重新认领\n\n\
         原认领的 agent 如果还在做，重新认领一次（`scripts/claims claim`）再继续。\n\nAgent: {by}\n",
        lease::RECLAIM_MARKER,
        lease.agent(),
        lease.claim.url,
        stamp(lease.last.at),
        lease.last.sign.what(),
        minutes(lease.idle_minutes(now)),
        stamp(now),
        lease::EXPIRES_AFTER_MINUTES,
        lease::CLAIM_LABEL,
    )
}

// ---- how times and durations are written ----

/// This machine's wall clock. chrono runs without its `clock` feature
/// here (backend/Cargo.toml), so the time comes from std.
pub fn now() -> DateTime<Utc> {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    DateTime::from_timestamp_millis(ms).unwrap_or(DateTime::UNIX_EPOCH)
}

/// Beijing time, which is the clock the owner and damai.cn are on.
pub fn beijing(t: DateTime<Utc>) -> DateTime<FixedOffset> {
    t.with_timezone(&FixedOffset::east_opt(8 * 3600).expect("+08:00 is a valid offset"))
}

/// A time as agents write it in a lease: Beijing time with its offset, so
/// reading it back cannot shift it.
pub fn stamp(t: DateTime<Utc>) -> String {
    beijing(t).format("%Y-%m-%dT%H:%M:%S+08:00").to_string()
}

/// A time for a person to read, without the offset noise.
pub fn short(t: DateTime<Utc>) -> String {
    beijing(t).format("%m-%d %H:%M").to_string()
}

/// A span of minutes in hours and minutes.
pub fn minutes(total: i64) -> String {
    match (total / 60, total % 60) {
        (0, m) => format!("{m} 分钟"),
        (h, 0) => format!("{h} 小时"),
        (h, m) => format!("{h} 小时 {m} 分钟"),
    }
}
