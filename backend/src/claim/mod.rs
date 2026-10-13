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
pub use snapshot::{Issue, Push, Snapshot};

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
    /// Why the pushes of the branch this claim names could not be read.
    /// A push is a heartbeat, so while this is set the lease cannot be
    /// shown to have gone silent, and nobody may take it over.
    pub push_unreadable: Option<String>,
    /// Whether the issue carries the `认领中` label. A claim comment that
    /// went out while the label did not is still a lease; the label is
    /// only what `gh issue list` filters on.
    pub labelled: bool,
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

/// Why an issue labelled `认领中` has no readable lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// No claim comment at all, or one missing a required field: somebody
    /// may well be working, and only they can say so.
    Unreadable,
    /// The claim was handed back and the label outlived it, so the issue
    /// is free and the next claim fixes the label.
    HandedBack,
}

/// An issue that claims to be claimed but whose lease cannot be read, so
/// the board reports it instead of quietly treating it as free or as
/// taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trouble {
    pub issue: u64,
    pub title: String,
    pub problem: Problem,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Board {
    /// Readable leases, lowest issue number first.
    pub leases: Vec<Lease>,
    pub trouble: Vec<Trouble>,
    /// The pushes the snapshot read, so an issue read separately from the
    /// snapshot is judged on the same evidence.
    pub pushes: BTreeMap<String, Push>,
}

impl Board {
    pub fn lease(&self, issue: u64) -> Option<&Lease> {
        self.leases.iter().find(|l| l.issue == issue)
    }

    /// Puts what `issue`'s own comments say onto the board, replacing
    /// whatever the snapshot said about that issue.
    ///
    /// The snapshot only holds issues labelled `认领中`, and the label is
    /// added after the claim comment, so a `gh issue edit` that failed
    /// leaves a live lease the snapshot cannot see. Every command that
    /// acts on one issue reads that issue and comes through here, so a
    /// missing label can no longer make a claim invisible (#16).
    pub fn include(&mut self, issue: &Issue) {
        self.leases.retain(|l| l.issue != issue.number);
        self.trouble.retain(|t| t.issue != issue.number);
        match read_lease(issue, &self.pushes) {
            Ok(lease) => {
                self.leases.push(lease);
                self.leases.sort_by_key(|l| l.issue);
            }
            Err((problem, why)) => {
                // Without the label, a hand-back is just a free issue
                // (the label came off, as it should), and so is an issue
                // nobody ever posted a claim on. A claim comment that is
                // missing a field is trouble either way: somebody may be
                // working and only they can say.
                let has_claim = issue
                    .comments
                    .iter()
                    .any(|c| lease::parse_claim(&c.body, &c.url, c.posted_at).is_some());
                if !issue.labels.iter().any(|l| l == lease::CLAIM_LABEL)
                    && (problem == Problem::HandedBack || !has_claim)
                {
                    return;
                }
                self.trouble.push(Trouble {
                    issue: issue.number,
                    title: issue.title.clone(),
                    problem,
                    why,
                });
                self.trouble.sort_by_key(|t| t.issue);
            }
        }
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
    let mut out = Board {
        pushes: snapshot.pushes.clone(),
        ..Board::default()
    };
    for issue in &snapshot.issues {
        match read_lease(issue, &snapshot.pushes) {
            Ok(lease) => out.leases.push(lease),
            Err((problem, why)) => out.trouble.push(Trouble {
                issue: issue.number,
                title: issue.title.clone(),
                problem,
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
/// A `收回认领` comment ends every claim before it, so the lease is read
/// from the comments after the last hand-back: the label may still be on
/// the issue for a moment, and the lease must not outlive the hand-back.
///
/// **In that window, the earliest claim comment holds the issue.** Two
/// agents that read "nobody is working on this" at the same moment both
/// post a claim, and both have to reach the same answer about which one
/// counts; GitHub's own order of the comments is the only thing they both
/// see. The holder's later claims are updates to its own lease (and
/// count as a sign of life); another agent's later claim lost the race
/// and does not count at all — the command that posted it says so with a
/// `让出认领` comment and exits non-zero.
pub fn read_lease(
    issue: &Issue,
    pushes: &BTreeMap<String, Push>,
) -> Result<Lease, (Problem, String)> {
    let handed_back_at = issue
        .comments
        .iter()
        .rposition(|c| lease::is_reclaim(&c.body));
    let window = match handed_back_at {
        Some(i) => &issue.comments[i + 1..],
        None => &issue.comments[..],
    };
    let mut claims: Vec<(DateTime<Utc>, u64, Claim)> = window
        .iter()
        .filter_map(|c| {
            lease::parse_claim(&c.body, &c.url, c.posted_at).map(|claim| (c.posted_at, c.id, claim))
        })
        .filter(|(_, _, c)| {
            // A claim the agent itself gave up after losing a race is
            // not a lease; the comment that gave it up names it.
            !window
                .iter()
                .any(|later| lease::yielded_claim(&later.body).is_some_and(|url| url == c.url))
        })
        .collect();
    // When two claims are posted in the same second, GitHub's comment id
    // says which came first.
    claims.sort_by_key(|(at, id, _)| (*at, *id));
    let claims: Vec<Claim> = claims.into_iter().map(|(_, _, c)| c).collect();
    let Some(holder) = claims.first() else {
        if let Some(i) = handed_back_at {
            let before = issue.comments[..i]
                .iter()
                .rev()
                .find_map(|c| lease::parse_claim(&c.body, &c.url, c.posted_at));
            if let Some(before) = before {
                return Err((
                    Problem::HandedBack,
                    format!(
                        "最后一条认领（{}）之后有人收回了它，但 {} 标签还在：没有人在做，\
                         下一次认领会把标签理顺",
                        before.url,
                        lease::CLAIM_LABEL
                    ),
                ));
            }
        }
        return Err((
            Problem::Unreadable,
            format!(
                "标着 {} 但没有认领评论：没法知道是哪个 agent、在改哪些模块。\
                 让持有者补一条认领评论（格式见 docs/subsystems/claim-lease.md），\
                 或者在 issue 上问清楚之后手动去掉标签",
                lease::CLAIM_LABEL
            ),
        ));
    };
    if let Some(field) = holder.missing_field() {
        return Err((
            Problem::Unreadable,
            format!(
                "认领评论缺字段 `{field}`（{}）：补上之后才知道{}",
                holder.url,
                if field == "agent" {
                    "是哪个 agent 在做"
                } else {
                    "它会改哪些模块，别人没法避开"
                }
            ),
        ));
    }
    // The holder's newest claim says what it is changing now; an earlier
    // one may name fewer modules.
    let claim = claims
        .iter()
        .rfind(|c| lease::same_agent(&c.agent, &holder.agent))
        .unwrap_or(holder)
        .clone();
    let mut last = Activity {
        at: claim.posted_at,
        sign: Sign::Claim,
    };
    for comment in window {
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
    let mut push_unreadable = None;
    if let Some(branch) = &claim.branch {
        match pushes.get(branch.as_str()) {
            Some(Push::At(at)) if *at > last.at => {
                last = Activity {
                    at: *at,
                    sign: Sign::Push {
                        branch: branch.clone(),
                    },
                };
            }
            Some(Push::Unreadable(why)) => {
                push_unreadable = Some(format!("{branch} 的推送时间读不到：{why}"));
            }
            _ => {}
        }
    }
    Ok(Lease {
        issue: issue.number,
        title: issue.title.clone(),
        assigned_to: lease::assigned_agent(&issue.labels),
        labelled: issue.labels.iter().any(|l| l == lease::CLAIM_LABEL),
        claim,
        last,
        push_unreadable,
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
    if let Some(t) = board
        .trouble
        .iter()
        .find(|t| t.issue == issue.number && t.problem == Problem::Unreadable)
    {
        return Verdict::Refuse(format!(
            "#{} 的认领读不出来，所以不知道是不是有人在做：{}。\
             先在 issue 上问清楚，别盖掉可能正在跑的工作",
            issue.number, t.why
        ));
    }
    if let Some(held) = board.lease(issue.number) {
        let status = held.status(now);
        if let Some(why) = &held.push_unreadable {
            return Verdict::Refuse(format!(
                "#{} 由 {} 认领，而且 {why}：\
                 推一次分支也算心跳，读不到推送时间就证明不了它已经沉默。\
                 先把读得到推送的权限配好（`gh auth status`），或者在 issue 上问清楚",
                issue.number,
                held.agent(),
            ));
        }
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
    if let Some(why) = &held.push_unreadable {
        return Verdict::Refuse(format!(
            "#{number} 由 {} 认领，而且 {why}：\
             推一次分支也算心跳，读不到推送时间就证明不了它已经沉默，不能收回。\
             先把读得到推送的权限配好（`gh auth status`），或者在 issue 上问清楚",
            held.agent(),
        ));
    }
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

/// What a claim comment turned out to be, read back after posting it.
///
/// Nothing stops two agents from posting a claim at the same moment:
/// GitHub has no compare-and-set, and the agents run in different
/// worktrees and sometimes on different machines. So the claim is
/// confirmed instead of locked — post, read the issue again, and the
/// earliest claim in the window is the one that holds it ([`read_lease`]).
/// Both agents read the same comments and reach the same answer, and the
/// one that lost says so and stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirmed {
    /// The claim that was posted is the lease.
    Held,
    /// An earlier claim holds the issue; this one does not count.
    Lost { agent: String, claim: String },
    /// The issue cannot be read back, so whether the claim holds is
    /// unknown and an agent should look before starting work.
    Unclear(String),
}

/// Whether the claim comment at `posted` is the lease on `issue`, read
/// from the issue as it is after posting.
pub fn confirm_claim(issue: &Issue, posted: &str, pushes: &BTreeMap<String, Push>) -> Confirmed {
    match read_lease(issue, pushes) {
        Ok(lease) if lease.claim.url == posted => Confirmed::Held,
        Ok(lease) => Confirmed::Lost {
            agent: lease.claim.agent.clone(),
            claim: lease.claim.url.clone(),
        },
        Err((_, why)) => Confirmed::Unclear(why),
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

/// The comment an agent posts when its claim lost a race: it gives up
/// the claim it names and nothing else, so the claim that won still
/// holds. [`lease::yielded_claim`] reads it back.
pub fn yield_comment(agent: &str, posted: &str, winner: &str, winner_claim: &str) -> String {
    format!(
        "## {}\n\n- agent: {agent}\n- 让出的认领: {posted}\n- 让给: {winner}（{winner_claim}）\n\
         - 为什么: 两个 agent 几乎同时认领这个 issue，两边都读到“没人在做”。\
         按 #16 的租约规则，窗口里最早的那条认领算数，所以我这条作废，{winner} 继续；\
         我不会开始做这个 issue\n\n\
         这条评论只作废上面那一条认领，不是 {}：{winner} 的租约照常有效。\n\nAgent: {agent}\n",
        lease::YIELD_MARKER,
        lease::RECLAIM_MARKER,
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
