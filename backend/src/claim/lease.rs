//! 认领租约的格式：agent 写在 issue 上的认领、心跳和收回评论（#16）。
//!
//! The format is written by agents and read by the dispatcher, so parsing
//! is forgiving about what agents vary (heading level, bullet character,
//! full-width or half-width colon, backticks around values) and strict
//! about the facts a hand-off hangs on: which agent is working, which
//! modules it will change, and when the lease was last alive.
//!
//! ```text
//! ## 认领
//!
//! - agent: Claude
//! - 开始时间: 2026-10-14T02:58:00+08:00
//! - 改动模块: backend/src/claim, scripts/claims
//! - 预计完成: 2026-10-14T06:00:00+08:00
//! - 分支: night/issue-16
//! ```
//!
//! Times in the body are for people to read: a lease's own clock comes
//! from GitHub's comment timestamps, which a typo cannot move.

use chrono::{DateTime, Utc};

/// The label that says an issue is claimed. The lease itself is the
/// claim comment; the label is what `gh issue list` can filter on.
pub const CLAIM_LABEL: &str = "认领中";

/// `指派:codex` means only that agent may claim the issue, replacing the
/// hand-written “负责：某个 agent” in issue bodies (#16).
pub const ASSIGNED_PREFIX: &str = "指派:";

/// Headings that mark a comment as a claim, a heartbeat, or a hand-back.
pub const CLAIM_MARKER: &str = "认领";
pub const HEARTBEAT_MARKER: &str = "心跳";
pub const RECLAIM_MARKER: &str = "收回认领";

/// A claim with no sign of life for this long is overdue for a heartbeat.
pub const HEARTBEAT_EVERY_MINUTES: i64 = 30;

/// A claim with no sign of life for this long is expired: the agent that
/// held it is assumed gone, and anyone may take the issue over (#16).
pub const EXPIRES_AFTER_MINUTES: i64 = 120;

/// One claim comment, as posted. Missing fields stay empty so the reader
/// can name the one that is missing instead of dropping the whole claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// Link to the comment, so a report points at the lease it read.
    pub url: String,
    /// GitHub's time for the comment. This, not the `开始时间` line, is
    /// when the lease started.
    pub posted_at: DateTime<Utc>,
    /// All agents share one GitHub account (#15), so who is working comes
    /// from the comment's own `agent` field.
    pub agent: String,
    /// Paths or directories this agent expects to change.
    pub modules: Vec<String>,
    /// When the agent expects to be done, as written. For display only:
    /// a lease expires from silence, not from running late.
    pub due: Option<DateTime<Utc>>,
    /// The branch whose pushes also count as a heartbeat.
    pub branch: Option<String>,
    /// Why claiming modules another claim already covers is fine.
    pub overlap_note: String,
}

impl Claim {
    /// The first required field this claim does not have, if any.
    pub fn missing_field(&self) -> Option<&'static str> {
        if self.agent.is_empty() {
            return Some("agent");
        }
        if self.modules.is_empty() {
            return Some("改动模块");
        }
        None
    }
}

/// A sign that the agent holding a claim is still working: a heartbeat
/// comment, or any comment it signed with its `Agent:` trailer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heartbeat {
    pub url: String,
    pub posted_at: DateTime<Utc>,
    pub agent: String,
    /// `## 心跳` with fields, as opposed to a signed progress comment.
    pub marked: bool,
}

/// Reads a claim out of a comment body, or `None` when the comment is not
/// one. A comment is a claim when a line of it is the `认领` heading on
/// its own, so `## 收回认领` and prose about claiming are not claims.
pub fn parse_claim(body: &str, url: &str, posted_at: DateTime<Utc>) -> Option<Claim> {
    if !body.lines().any(|l| is_marker_line(l, CLAIM_MARKER)) {
        return None;
    }
    let field = |keys: &[&str]| field(body, keys);
    Some(Claim {
        url: url.to_string(),
        posted_at,
        agent: field(&["agent", "认领者", "认领 agent"]),
        modules: modules(&field(&[
            "改动模块",
            "模块",
            "预计改动的模块",
            "改哪些模块",
            "预计改动的模块或目录",
        ])),
        due: parse_time(&field(&["预计完成", "预计完成时间", "预计结束"])),
        branch: Some(field(&["分支", "branch"])).filter(|b| !b.is_empty()),
        overlap_note: field(&["重叠", "重叠说明", "和谁重叠"]),
    })
}

/// Reads a heartbeat out of a comment body. A `## 心跳` comment is one;
/// so is any other comment whose `Agent:` trailer names an agent, because
/// every agent already signs what it writes (AGENTS.md), and a progress
/// comment is a progress report whether or not it carries the heading.
/// A claim comment is not a heartbeat: the claim itself starts the clock.
pub fn parse_heartbeat(body: &str, url: &str, posted_at: DateTime<Utc>) -> Option<Heartbeat> {
    if body.lines().any(|l| is_marker_line(l, CLAIM_MARKER)) {
        return None;
    }
    let marked = body.lines().any(|l| is_marker_line(l, HEARTBEAT_MARKER));
    let agent = match field(body, &["agent", "认领者", "认领 agent"]) {
        a if !a.is_empty() => a,
        _ => agent_trailer(body)?,
    };
    Some(Heartbeat {
        url: url.to_string(),
        posted_at,
        agent,
        marked,
    })
}

/// Whether a comment hands an issue back, so the lease before it no
/// longer counts.
pub fn is_reclaim(body: &str) -> bool {
    body.lines().any(|l| is_marker_line(l, RECLAIM_MARKER))
}

/// Two agent names for the same agent: case and a suffix such as
/// “（第 4 条线）” do not make it a different one.
pub fn same_agent(a: &str, b: &str) -> bool {
    let key = |s: &str| {
        s.trim()
            .to_lowercase()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect::<String>()
    };
    let (a, b) = (key(a), key(b));
    !a.is_empty() && a == b
}

/// The agent an issue is assigned to by its `指派:<agent>` label, if any.
pub fn assigned_agent(labels: &[String]) -> Option<String> {
    labels.iter().find_map(|l| {
        l.strip_prefix(ASSIGNED_PREFIX)
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
    })
}

/// The modules two claims both cover. A directory covers what is under
/// it, so `backend/src` and `backend/src/claim/mod.rs` overlap, while
/// `backend/src/claim` and `backend/src/claims.rs` do not.
pub fn overlap(a: &[String], b: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    for x in a {
        for y in b {
            if covers(x, y) || covers(y, x) {
                let narrower = if x.len() >= y.len() { x } else { y };
                if !found.contains(narrower) {
                    found.push(narrower.clone());
                }
            }
        }
    }
    found
}

/// Whether path `wide` is `narrow` or a directory containing it.
fn covers(wide: &str, narrow: &str) -> bool {
    narrow == wide
        || narrow
            .strip_prefix(wide)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// `改动模块: a, b c` into the paths it names. Agents separate with
/// commas, full-width commas or spaces, and sometimes wrap a path in
/// backticks; a trailing slash does not make a different directory.
fn modules(value: &str) -> Vec<String> {
    value
        .split([',', '，', ' ', '、', '\t'])
        .map(|m| {
            m.trim()
                .trim_matches('`')
                .trim_start_matches("./")
                .trim_end_matches('/')
                .trim()
        })
        .filter(|m| !m.is_empty() && *m != "和")
        .map(str::to_string)
        .collect()
}

/// A time an agent wrote by hand. Only forms that carry a zone are
/// accepted, so a time without one is reported as missing rather than
/// read as UTC and silently shifted by eight hours.
pub fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(t) = DateTime::parse_from_rfc3339(value) {
        return Some(t.with_timezone(&Utc));
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%z",
        "%Y-%m-%d %H:%M:%S%z",
        "%Y-%m-%d %H:%M%z",
        "%Y-%m-%dT%H:%M%z",
    ] {
        if let Ok(t) = DateTime::parse_from_str(value, format) {
            return Some(t.with_timezone(&Utc));
        }
    }
    None
}

/// The value of the first of `keys` the body has a field for.
fn field(body: &str, keys: &[&str]) -> String {
    body.lines()
        .filter_map(split_field)
        .find(|(k, _)| keys.iter().any(|want| k.eq_ignore_ascii_case(want)))
        .map(|(_, v)| v)
        .unwrap_or_default()
}

/// `marker` on a line of its own, with or without `#` heading markers,
/// bold markers or a trailing colon.
fn is_marker_line(line: &str, marker: &str) -> bool {
    let t = line
        .trim()
        .trim_start_matches('#')
        .trim_matches(|c: char| c == '*' || c == '_' || c.is_whitespace())
        .trim_end_matches([':', '：'])
        .trim();
    t == marker
}

/// `- key: value` into its two halves. Accepts `*` and `+` bullets, no
/// bullet at all, bold keys, and both colons; values lose surrounding
/// backticks, so a path written as `` `scripts/claims` `` reads as the
/// path.
fn split_field(line: &str) -> Option<(String, String)> {
    let line = line.trim().trim_start_matches(['-', '*', '+']).trim();
    let at = line.find([':', '：'])?;
    let key = line[..at]
        .trim()
        .trim_matches(|c: char| c == '*' || c == '_' || c == '`')
        .trim()
        .to_string();
    let value = line[at + line[at..].chars().next().unwrap().len_utf8()..]
        .trim()
        .trim_matches(|c: char| c == '`' || c.is_whitespace())
        .to_string();
    if key.is_empty() {
        return None;
    }
    Some((key, value))
}

/// The `Agent: <name>` trailer that says which agent wrote a comment. All
/// agents share one GitHub account (#15), so this trailer is the only
/// thing that tells them apart. The last one wins: a comment that quotes
/// the convention above its own trailer still reads as its own.
pub fn agent_trailer(body: &str) -> Option<String> {
    body.lines()
        .filter_map(|l| {
            let l = l.trim().trim_start_matches(['-', '*', '>']).trim();
            let rest = l.strip_prefix("Agent")?;
            let rest = rest.trim_start().strip_prefix([':', '：'])?;
            let name = rest.trim().trim_matches('`').trim();
            (!name.is_empty() && !name.contains("名字") && !name.contains('<'))
                .then(|| name.to_string())
        })
        .next_back()
}
