//! Issue 优先级：每个 issue 是 P0～P3 的哪一档，agent 按什么顺序挑（#68）。
//!
//! Before this, the work loop said "pick the unclaimed issue that matters
//! most to users", and every agent read that differently: the handy issue
//! got done and the foundations waited. An issue's priority is now one of
//! four labels, from the two axes of the important/urgent grid
//! (docs/subsystems/issue-priority.md), and the order in which issues are
//! picked is computed rather than chosen:
//!
//! - [`queue`] lists what can be started now, highest priority first,
//!   and says why everything else is not in it.
//! - [`decide`] refuses a claim that skips the queue, unless the agent
//!   writes down why.
//! - [`triage_comment`] is the record a priority change leaves behind.
//!
//! As with the lease, nothing here calls a model: labels, issue bodies and
//! the lease board decide, so the dispatcher and an agent agree.

use chrono::{DateTime, Utc};

use super::snapshot::OpenIssue;
use super::{Board, Problem, lease, stamp};

/// The issue waits for an owner ruling (money, a real account, legal
/// risk, a product trade-off). Nobody claims it; its priority still says
/// which ruling the owner should make first.
pub const WAITING_LABEL: &str = "等裁决";

/// Merged and waiting for an independent acceptance. The work is done;
/// starting it again would redo it.
pub const ACCEPTANCE_LABEL: &str = "待验收";

/// Acceptance found a problem. The issue is rework, and it is back in the
/// queue for its implementer or whoever takes it over.
pub const REJECTED_LABEL: &str = "验收未通过";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    P0,
    P1,
    P2,
    P3,
}

impl Priority {
    pub const ALL: [Priority; 4] = [Priority::P0, Priority::P1, Priority::P2, Priority::P3];

    pub fn label(self) -> &'static str {
        match self {
            Priority::P0 => "P0",
            Priority::P1 => "P1",
            Priority::P2 => "P2",
            Priority::P3 => "P3",
        }
    }

    /// `P1`, `p1` or ` P1 `; anything else is not a priority.
    pub fn parse(text: &str) -> Option<Priority> {
        let text = text.trim();
        Priority::ALL
            .into_iter()
            .find(|p| p.label().eq_ignore_ascii_case(text))
    }

    pub fn important(self) -> bool {
        matches!(self, Priority::P0 | Priority::P1)
    }

    pub fn urgent(self) -> bool {
        matches!(self, Priority::P0 | Priority::P2)
    }

    /// The quadrant in words, as the labels and comments write it.
    pub fn meaning(self) -> &'static str {
        match self {
            Priority::P0 => "重要，紧急",
            Priority::P1 => "重要，不紧急",
            Priority::P2 => "不重要，紧急",
            Priority::P3 => "不重要，不紧急",
        }
    }
}

/// What an issue's labels say about its priority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    Set(Priority),
    /// Nobody has triaged it yet.
    Unset,
    /// Two or more priority labels: somebody changed it without removing
    /// the old one, and neither can be trusted over the other.
    Several(Vec<Priority>),
}

impl Level {
    pub fn priority(&self) -> Option<Priority> {
        match self {
            Level::Set(p) => Some(*p),
            _ => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Level::Set(p) => p.label().to_string(),
            Level::Unset => "没定".to_string(),
            Level::Several(ps) => ps.iter().map(|p| p.label()).collect::<Vec<_>>().join("+"),
        }
    }
}

pub fn level(labels: &[String]) -> Level {
    let mut found: Vec<Priority> = labels.iter().filter_map(|l| Priority::parse(l)).collect();
    found.sort();
    found.dedup();
    match found.as_slice() {
        [] => Level::Unset,
        [one] => Level::Set(*one),
        _ => Level::Several(found),
    }
}

/// The words that start a dependency line, with or without a bullet:
/// `依赖：#4、#26`, `- 前置: #4`, `Depends on: #4`.
const DEPENDS_KEYS: [&str; 4] = ["依赖", "前置", "depends on", "blocked by"];

/// The issues the body says must close before this one starts. Only lines
/// the body itself says count: a quoted line (`>`) or a fenced code block
/// is somebody else's words, the same rule the lease format follows.
pub fn depends_on(body: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || line.starts_with('>') {
            continue;
        }
        let line = line
            .trim_start_matches(['-', '*', '+'])
            .trim()
            .trim_start_matches("**")
            .trim();
        let lower = line.to_lowercase();
        let Some(rest) = DEPENDS_KEYS.iter().find_map(|key| {
            lower
                .strip_prefix(key)
                .map(|_| line[key.len()..].trim_start_matches("**").trim_start())
        }) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('：')) else {
            continue;
        };
        for number in issue_refs(rest) {
            if !out.contains(&number) {
                out.push(number);
            }
        }
    }
    out
}

/// Every `#123` in `text`, in order.
fn issue_refs(text: &str) -> Vec<u64> {
    text.match_indices('#')
        .filter_map(|(i, _)| {
            let digits: String = text[i + 1..]
                .chars()
                .take_while(|d| d.is_ascii_digit())
                .collect();
            digits.parse().ok()
        })
        .collect()
}

/// An issue that can be started now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub issue: u64,
    pub title: String,
    pub level: Level,
    pub assigned_to: Option<String>,
    /// What the agent has to know before claiming it: a lease to take
    /// over first, rework after a failed acceptance, a priority lower
    /// than what it blocks.
    pub notes: Vec<String>,
}

/// Why an open issue is not in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    Claimed {
        agent: String,
    },
    /// Labelled claimed but its lease cannot be read: somebody may be
    /// working, and only they can say.
    Unreadable,
    AwaitingAcceptance,
    AwaitingOwner,
    AssignedTo(String),
    /// Issues its body depends on that are still open.
    Blocked(Vec<u64>),
}

impl Held {
    pub fn name(&self) -> &'static str {
        match self {
            Held::Claimed { .. } => "认领中",
            Held::Unreadable => "认领读不出来",
            Held::AwaitingAcceptance => "待验收",
            Held::AwaitingOwner => "等裁决",
            Held::AssignedTo(_) => "指派给别人",
            Held::Blocked(_) => "被依赖挡着",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    pub issue: u64,
    pub title: String,
    pub level: Level,
    pub held: Held,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Queue {
    /// Untriaged issues first (they have to be triaged before anyone can
    /// compare them), then P0 to P3; within a priority, the lowest issue
    /// number first, so the queue is the same whoever reads it.
    pub ready: Vec<Ready>,
    pub waiting: Vec<Waiting>,
}

impl Queue {
    /// The first triaged issue in the queue, which is what the work loop
    /// picks next.
    pub fn next(&self) -> Option<&Ready> {
        self.ready.iter().find(|r| r.level.priority().is_some())
    }
}

/// What can be started now, out of every open issue, for `agent` (or for
/// anybody, when no agent is given: then issues reserved for one agent
/// stay in the queue and say whom they are for).
pub fn queue(open: &[OpenIssue], board: &Board, agent: Option<&str>, now: DateTime<Utc>) -> Queue {
    let mut out = Queue::default();
    for issue in open {
        let own = level(&issue.labels);
        let assigned_to = lease::assigned_agent(&issue.labels);
        let mut notes = Vec::new();
        let held = match board.lease(issue.number) {
            Some(l) if !l.reclaimable(now) => Some(Held::Claimed {
                agent: l.agent().to_string(),
            }),
            Some(l) => {
                notes.push(format!(
                    "{} 的认领已经过期，先 `scripts/claims reclaim --issue {} --agent <你>` 收回",
                    l.agent(),
                    issue.number
                ));
                None
            }
            None => match board.trouble.iter().find(|t| t.issue == issue.number) {
                Some(t) if t.problem == Problem::HandedBack => None,
                Some(_) => Some(Held::Unreadable),
                // Labelled claimed, yet the board read nothing about it:
                // the two were read at different moments. Somebody may
                // have just claimed it, so it is not offered.
                None if has(issue, lease::CLAIM_LABEL) => Some(Held::Claimed {
                    agent: "?".to_string(),
                }),
                None => None,
            },
        };
        let blocking: Vec<u64> = depends_on(&issue.body)
            .into_iter()
            .filter(|n| *n != issue.number && open.iter().any(|o| o.number == *n))
            .collect();
        let held = held.or_else(|| {
            if has(issue, ACCEPTANCE_LABEL) {
                Some(Held::AwaitingAcceptance)
            } else if has(issue, WAITING_LABEL) {
                Some(Held::AwaitingOwner)
            } else if let (Some(owner), Some(me)) = (&assigned_to, agent)
                && !lease::same_agent(owner, me)
            {
                Some(Held::AssignedTo(owner.clone()))
            } else if !blocking.is_empty() {
                Some(Held::Blocked(blocking))
            } else {
                None
            }
        });
        if let Some(mine) = own.priority() {
            for dependant in open
                .iter()
                .filter(|o| o.number != issue.number && depends_on(&o.body).contains(&issue.number))
            {
                if let Some(theirs) = level(&dependant.labels).priority()
                    && theirs < mine
                {
                    notes.push(format!(
                        "挡着 #{}（{}），挡路的 issue 不低于被它挡住的：用 `scripts/claims triage` 提到 {}",
                        dependant.number,
                        theirs.label(),
                        theirs.label()
                    ));
                }
            }
        }
        if has(issue, REJECTED_LABEL) {
            notes.push("验收没通过，是返工：先读验收者写的问题".to_string());
        }
        match held {
            Some(held) => out.waiting.push(Waiting {
                issue: issue.number,
                title: issue.title.clone(),
                level: own,
                held,
            }),
            None => out.ready.push(Ready {
                issue: issue.number,
                title: issue.title.clone(),
                level: own,
                assigned_to,
                notes,
            }),
        }
    }
    out.ready.sort_by_key(|r| (rank(&r.level), r.issue));
    out.waiting.sort_by_key(|w| w.issue);
    out
}

/// Untriaged first, then P0 to P3.
fn rank(level: &Level) -> usize {
    match level {
        Level::Unset | Level::Several(_) => 0,
        Level::Set(p) => 1 + *p as usize,
    }
}

fn has(issue: &OpenIssue, label: &str) -> bool {
    issue.labels.iter().any(|l| l == label)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The claim may go ahead. `out_of_order` is what the claim comment
    /// must say about the queue it skipped, if it skipped any.
    Ok {
        out_of_order: Option<String>,
    },
    Refuse(String),
}

/// Whether claiming `target` follows the queue `agent` sees. A claim
/// that skips higher-priority work, or starts before the issues it
/// depends on, is refused unless the agent gives a reason, which then
/// goes into the claim comment where the next reader sees it.
pub fn decide(open: &[OpenIssue], queue: &Queue, target: u64, reason: Option<&str>) -> Verdict {
    let reason = reason.map(str::trim).filter(|r| !r.is_empty());
    let Some(issue) = open.iter().find(|i| i.number == target) else {
        return Verdict::Refuse(format!(
            "#{target} 不在打开的 issue 里（已经关了，或者是 PR），没有可认领的"
        ));
    };
    if has(issue, WAITING_LABEL) {
        return Verdict::Refuse(format!(
            "#{target} 带着 {WAITING_LABEL} 标签，在等 owner 裁决，agent 不认领。\
             先做不受它影响的部分；裁决之后由 owner 或转述裁决的 agent 去掉标签"
        ));
    }
    let mine = match level(&issue.labels) {
        Level::Set(p) => p,
        Level::Unset => {
            return Verdict::Refuse(format!(
                "#{target} 还没定优先级，不知道该不该现在做。先定级：\
                 `scripts/claims triage --issue {target} --agent <你> --priority <P0～P3> \
                 --important <为什么重要或不重要> --urgent <为什么紧急或不紧急>`\
                 （怎么判断见 docs/subsystems/issue-priority.md）"
            ));
        }
        Level::Several(_) => {
            return Verdict::Refuse(format!(
                "#{target} 同时有 {} 几个优先级标签，看不出哪个算数。\
                 先用 `scripts/claims triage --issue {target} …` 定成一个",
                level(&issue.labels).describe()
            ));
        }
    };
    let mut skipped = Vec::new();
    let blocking: Vec<u64> = depends_on(&issue.body)
        .into_iter()
        .filter(|n| *n != target && open.iter().any(|o| o.number == *n))
        .collect();
    if !blocking.is_empty() {
        skipped.push(format!(
            "它依赖的 {} 还没关",
            blocking
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    let higher: Vec<&Ready> = queue
        .ready
        .iter()
        .filter(|r| r.issue != target && r.level.priority().is_some_and(|p| p < mine))
        .collect();
    if !higher.is_empty() {
        const SHOWN: usize = 5;
        let mut list = higher
            .iter()
            .take(SHOWN)
            .map(|r| format!("#{}（{}）", r.issue, r.level.describe()))
            .collect::<Vec<_>>()
            .join("、");
        if higher.len() > SHOWN {
            list.push_str(&format!(" 等 {} 个", higher.len()));
        }
        skipped.push(format!(
            "队列里还有比 {} 高、现在就能做的 {list}",
            mine.label()
        ));
    }
    if skipped.is_empty() {
        return Verdict::Ok { out_of_order: None };
    }
    match reason {
        Some(reason) => Verdict::Ok {
            out_of_order: Some(format!("{}；{reason}", skipped.join("；"))),
        },
        None => Verdict::Refuse(format!(
            "#{target}（{}）不是现在该做的：{}。\
             先做排在前面的（`scripts/claims queue --agent <你>`）。\
             有正当理由（例如前面的要访问真实大麦而你在云端，或依赖的代码已经合进 main 只差验收），\
             就带上 --out-of-order <为什么> 再来一次，理由会写进认领评论",
            mine.label(),
            skipped.join("；")
        )),
    }
}

/// The comment a priority change leaves, so whoever reads the issue sees
/// both axes and who decided. The labels follow the comment, the same
/// order the lease uses: the comment is the record.
pub fn triage_comment(
    agent: &str,
    new: Priority,
    old: &Level,
    important: &str,
    urgent: &str,
    now: DateTime<Utc>,
) -> String {
    let yes = |b: bool| if b { "是" } else { "否" };
    format!(
        "## 优先级\n\n- agent: {agent}\n- 时间: {}\n- 优先级: {}（{}）\n- 原来: {}\n\
         - 重要: {}。{}\n- 紧急: {}。{}\n\n\
         定级规则见 docs/subsystems/issue-priority.md。不同意就留一条新的 \
         `## 优先级` 评论写明理由（`scripts/claims triage`），owner 说过的优先级 agent 不改。\n\n\
         Agent: {agent}\n",
        stamp(now),
        new.label(),
        new.meaning(),
        old.describe(),
        yes(new.important()),
        important.trim().trim_end_matches('。'),
        yes(new.urgent()),
        urgent.trim().trim_end_matches('。'),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claim::snapshot::Comment;
    use crate::claim::{Board, Issue, board};
    use chrono::DateTime;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn open(number: u64, labels: &[&str], body: &str) -> OpenIssue {
        OpenIssue {
            number,
            title: format!("issue {number}"),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            body: body.to_string(),
        }
    }

    fn numbers(q: &Queue) -> Vec<u64> {
        q.ready.iter().map(|r| r.issue).collect()
    }

    fn held(q: &Queue, n: u64) -> Option<&Held> {
        q.waiting.iter().find(|w| w.issue == n).map(|w| &w.held)
    }

    const NOW: &str = "2026-10-14T11:30:00+08:00";

    #[test]
    fn a_priority_label_is_read_whatever_its_case_and_two_of_them_are_not_trusted() {
        assert_eq!(
            level(&["bug".into(), "p2".into()]),
            Level::Set(Priority::P2)
        );
        assert_eq!(level(&["bug".into()]), Level::Unset);
        assert_eq!(
            level(&["P3".into(), "P1".into()]),
            Level::Several(vec![Priority::P1, Priority::P3])
        );
        assert!(Priority::P0.important() && Priority::P0.urgent());
        assert!(Priority::P1.important() && !Priority::P1.urgent());
        assert!(!Priority::P2.important() && Priority::P2.urgent());
        assert!(!Priority::P3.important() && !Priority::P3.urgent());
    }

    #[test]
    fn a_dependency_line_is_read_in_the_ways_agents_write_it_and_quotes_do_not_count() {
        assert_eq!(depends_on("依赖：#4、#26"), vec![4, 26]);
        assert_eq!(depends_on("- 前置: #4, #4 和 #9"), vec![4, 9]);
        assert_eq!(depends_on("**依赖**：#12（统一访问层）"), vec![12]);
        assert_eq!(depends_on("Depends on: #3"), vec![3]);
        assert_eq!(depends_on("这个 issue 依赖 #4 的设计"), Vec::<u64>::new());
        assert_eq!(depends_on("> 依赖：#4"), Vec::<u64>::new());
        assert_eq!(depends_on("```\n依赖：#4\n```\n"), Vec::<u64>::new());
    }

    #[test]
    fn the_queue_puts_untriaged_first_then_p0_to_p3_then_the_lowest_number() {
        let issues = vec![
            open(9, &["P2"], ""),
            open(3, &["P1"], ""),
            open(7, &["P0"], ""),
            open(5, &["P1"], ""),
            open(8, &[], ""),
            open(2, &["P3"], ""),
        ];
        let q = queue(&issues, &Board::default(), None, at(NOW));
        assert_eq!(numbers(&q), vec![8, 7, 3, 5, 9, 2]);
        assert_eq!(q.next().map(|r| r.issue), Some(7), "没定级的不能当下一个");
    }

    #[test]
    fn issues_nobody_should_start_now_are_held_with_the_reason() {
        let issues = vec![
            open(1, &["P0", WAITING_LABEL], ""),
            open(2, &["P1", ACCEPTANCE_LABEL], ""),
            open(3, &["P1", "指派:codex"], ""),
            open(4, &["P1"], ""),
            open(5, &["P1"], "依赖：#4"),
            open(6, &["P1"], "依赖：#99"),
        ];
        let q = queue(&issues, &Board::default(), Some("Claude"), at(NOW));
        assert_eq!(held(&q, 1), Some(&Held::AwaitingOwner));
        assert_eq!(held(&q, 2), Some(&Held::AwaitingAcceptance));
        assert_eq!(held(&q, 3), Some(&Held::AssignedTo("codex".to_string())));
        assert_eq!(held(&q, 5), Some(&Held::Blocked(vec![4])));
        assert_eq!(numbers(&q), vec![4, 6], "已经关掉的依赖（#99）不挡路");

        let anybody = queue(&issues, &Board::default(), None, at(NOW));
        assert!(
            numbers(&anybody).contains(&3),
            "不说是谁时，指派的 issue 留在队列里并写明给谁"
        );
        let codex = queue(&issues, &Board::default(), Some("Codex"), at(NOW));
        assert!(numbers(&codex).contains(&3), "指派给自己的照常排队");
    }

    fn claimed(number: u64, posted: &str) -> Issue {
        Issue {
            number,
            title: format!("issue {number}"),
            labels: vec![lease::CLAIM_LABEL.to_string(), "P1".to_string()],
            comments: vec![Comment {
                id: 1,
                url: format!("https://github.com/o/r/issues/{number}#issuecomment-1"),
                posted_at: at(posted),
                body: "## 认领\n\n- agent: Codex\n- 改动模块: backend/src/x\n".to_string(),
            }],
        }
    }

    #[test]
    fn a_live_lease_holds_an_issue_and_an_expired_one_offers_it_with_a_reclaim_first() {
        let live = claimed(4, "2026-10-14T11:00:00+08:00");
        let gone = claimed(5, "2026-10-14T08:00:00+08:00");
        let board = board(&crate::claim::Snapshot {
            issues: vec![live, gone],
            pushes: Default::default(),
        });
        let issues = vec![
            open(4, &["P1", lease::CLAIM_LABEL], ""),
            open(5, &["P1", lease::CLAIM_LABEL], ""),
            open(6, &["P1", lease::CLAIM_LABEL], ""),
        ];
        let q = queue(&issues, &board, None, at(NOW));
        assert_eq!(
            held(&q, 4),
            Some(&Held::Claimed {
                agent: "Codex".to_string()
            })
        );
        assert_eq!(numbers(&q), vec![5]);
        assert!(q.ready[0].notes[0].contains("reclaim"), "{:?}", q.ready[0]);
        assert!(
            matches!(held(&q, 6), Some(Held::Claimed { .. })),
            "标着认领中、看板却没读到的，不当成没人做"
        );
    }

    #[test]
    fn an_issue_that_blocks_a_higher_priority_one_says_it_should_be_raised() {
        let issues = vec![open(4, &["P2"], ""), open(5, &["P1"], "依赖：#4")];
        let q = queue(&issues, &Board::default(), None, at(NOW));
        assert_eq!(numbers(&q), vec![4]);
        assert!(
            q.ready[0].notes.iter().any(|n| n.contains("挡着 #5（P1）")),
            "{:?}",
            q.ready[0]
        );
    }

    #[test]
    fn claiming_below_the_queue_is_refused_until_the_reason_is_written_down() {
        let issues = vec![
            open(3, &["P1"], ""),
            open(4, &["P2"], ""),
            open(5, &["P2"], ""),
        ];
        let q = queue(&issues, &Board::default(), Some("Claude"), at(NOW));
        let Verdict::Refuse(why) = decide(&issues, &q, 4, None) else {
            panic!("队列里有 P1 时认领 P2 要拒绝");
        };
        assert!(
            why.contains("#3（P1）") && why.contains("--out-of-order"),
            "{why}"
        );
        assert_eq!(
            decide(&issues, &q, 3, None),
            Verdict::Ok { out_of_order: None },
            "认领队首不需要理由"
        );
        let Verdict::Ok {
            out_of_order: Some(note),
        } = decide(&issues, &q, 4, Some("#3 要访问真实大麦，我在云端"))
        else {
            panic!("带理由要放行");
        };
        assert!(
            note.contains("#3（P1）") && note.contains("我在云端"),
            "{note}"
        );
    }

    #[test]
    fn within_one_priority_any_issue_may_be_claimed() {
        // The issue number only gives readers a stable order; two P1s
        // are equally important, and modules or skills decide between them.
        let issues = vec![open(3, &["P1"], ""), open(4, &["P1"], "")];
        let q = queue(&issues, &Board::default(), None, at(NOW));
        assert_eq!(
            decide(&issues, &q, 4, None),
            Verdict::Ok { out_of_order: None }
        );
    }

    #[test]
    fn claiming_an_untriaged_waiting_or_blocked_issue_is_refused_with_the_next_step() {
        let issues = vec![
            open(1, &[], ""),
            open(2, &["P1", WAITING_LABEL], ""),
            open(3, &["P1", "P2"], ""),
            open(4, &["P1"], ""),
            open(5, &["P1"], "依赖：#4"),
        ];
        let q = queue(&issues, &Board::default(), None, at(NOW));
        for (n, want) in [
            (1, "claims triage"),
            (2, WAITING_LABEL),
            (3, "P1+P2"),
            (5, "依赖的 #4 还没关"),
            (77, "不在打开的 issue 里"),
        ] {
            let Verdict::Refuse(why) = decide(&issues, &q, n, None) else {
                panic!("#{n} 应该被拒绝");
            };
            assert!(why.contains(want), "#{n}：{why}");
        }
        assert!(
            matches!(
                decide(&issues, &q, 5, Some("#4 已合进 main，只差验收")),
                Verdict::Ok {
                    out_of_order: Some(_)
                }
            ),
            "依赖没关也能带理由开工"
        );
        assert!(
            matches!(
                decide(&issues, &q, 2, Some("我觉得可以")),
                Verdict::Refuse(_)
            ),
            "等裁决的不能靠理由绕过：那是 owner 的决定"
        );
    }

    #[test]
    fn a_triage_comment_gives_both_axes_and_what_it_replaced() {
        let body = triage_comment(
            "Claude",
            Priority::P2,
            &Level::Set(Priority::P1),
            "只影响一句提示。",
            "挡着正在干活的 agent",
            at(NOW),
        );
        for want in [
            "## 优先级",
            "- agent: Claude",
            "- 优先级: P2（不重要，紧急）",
            "- 原来: P1",
            "- 重要: 否。只影响一句提示\n",
            "- 紧急: 是。挡着正在干活的 agent\n",
            "Agent: Claude",
        ] {
            assert!(body.contains(want), "少了 {want:?}：\n{body}");
        }
        assert!(
            lease::parse_claim(&body, "", at(NOW)).is_none(),
            "定级评论不能被读成认领"
        );
    }
}
