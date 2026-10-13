//! 审查记录的格式：审查者在 PR 上写的那条评论，合并闸门按它决定。
//!
//! The format is written by agents and read by the gate, so parsing is
//! forgiving about the things agents vary (heading level, bullet
//! character, full-width or half-width colon, spaces and case inside a
//! field name, upper or lower case sha) and strict about the four facts a
//! merge hangs on: which commit was reviewed, which main it was merged
//! onto, who reviewed it, and whether it passed. The shape is the one
//! tonight's reviewers already write (issue #20), including the
//! `被审查的提交` the dispatched review task uses, so their records parse
//! without being rewritten.
//!
//! ```text
//! ## 审查记录
//!
//! - 被审查的 SHA: 0dbf6fcc0f0b9df91ec77ddf385112b3a50c0a0c
//! - 合到的 main: 293d2372fa018243ca53962a7e6fdfa9154469bd
//! - 审查者: Codex
//! - 结论: 通过
//! - 跑了什么: 临时 HOME 下 make check 全绿；回退修复后 X 测试 FAIL "……"
//! ```

use chrono::{DateTime, Utc};

/// The heading that marks a comment as a review record.
pub const MARKER: &str = "审查记录";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Passed,
    NotPassed,
}

/// One review record, as posted. Missing fields stay empty so the gate can
/// name the one that is missing instead of ignoring the whole record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRecord {
    /// Link to the comment, so a refusal points at the record it read.
    pub url: String,
    pub posted_at: DateTime<Utc>,
    /// The agent GitHub attributes the comment to is the shared account, so
    /// who reviewed comes from the record's own `审查者` field.
    pub reviewer: String,
    pub reviewed_sha: String,
    pub reviewed_main: String,
    pub verdict: Verdict,
    /// The `结论` field as the reviewer wrote it, so a refusal can show
    /// what the gate read instead of only saying it was not a pass.
    pub verdict_said: String,
    pub ran: String,
}

impl ReviewRecord {
    /// The first required field this record does not have, if any.
    pub fn missing_field(&self) -> Option<&'static str> {
        if !is_sha(&self.reviewed_sha) {
            return Some("被审查的 SHA");
        }
        if !is_sha(&self.reviewed_main) {
            return Some("合到的 main");
        }
        if self.reviewer.is_empty() {
            return Some("审查者");
        }
        if self.ran.is_empty() {
            return Some("跑了什么");
        }
        None
    }
}

/// A full 40-character git sha, which is what `--match-head-commit` needs.
pub fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Reads a review record out of a comment body, or `None` when the comment
/// is not one. A comment is a record when a line of it is the marker
/// heading on its own; an unparsable field is left empty rather than
/// dropping the record, so the gate refuses with the missing field.
pub fn parse(body: &str, url: &str, posted_at: DateTime<Utc>) -> Option<ReviewRecord> {
    if !body.lines().any(is_marker_line) {
        return None;
    }
    // Keys are matched without their spaces and case, so `被审查的 SHA`,
    // `被审查的SHA` and `被审查的 sha` are one key, and each field lists
    // the other names agents write it under.
    let field = |keys: &[&str]| -> String {
        let keys: Vec<String> = keys.iter().map(|k| key_of(k)).collect();
        body.lines()
            .filter_map(split_field)
            .find(|(k, _)| keys.contains(k))
            .map(|(_, v)| v)
            .unwrap_or_default()
    };
    Some(ReviewRecord {
        url: url.to_string(),
        posted_at,
        reviewer: field(&["审查者", "审查 agent"]),
        // `被审查的提交` is the name the review task #20 dispatches uses.
        reviewed_sha: field(&[
            "被审查的 SHA",
            "被审查的提交",
            "审查的提交",
            "审查 head",
            "head",
        ])
        .to_lowercase(),
        reviewed_main: field(&["合到的 main", "基线 main", "合到的 main sha"]).to_lowercase(),
        verdict: verdict_of(&field(&["结论"])),
        verdict_said: field(&["结论"]),
        ran: field(&["跑了什么", "跑了"]),
    })
}

/// Only a verdict that says `通过` and nothing else about it is a pass.
///
/// A merge cannot be taken back, so everything the gate is not sure of is
/// a refusal: `未通过` and `没通过` contain `通过`, `要修改，通过后再合并`
/// and `有条件通过` promise a pass that has not happened yet, and
/// `通过，但要修改` is two answers at once. Writing `- 结论: 通过` again
/// costs the reviewer one comment; merging on a misread verdict costs a
/// commit on main that nobody passed.
fn verdict_of(value: &str) -> Verdict {
    /// Words that make a verdict something other than a plain pass,
    /// wherever in it they appear.
    const NOT_A_PASS: [&str; 10] = [
        "不通过",
        "未通过",
        "没通过",
        "要修改",
        "待修改",
        "不合并",
        "不要合并",
        "不能合并",
        "有条件",
        "暂",
    ];
    let value = value.trim();
    if NOT_A_PASS.iter().any(|w| value.contains(w)) {
        return Verdict::NotPassed;
    }
    // The first clause only: `通过，可以合并` is a pass, `通过后再合并`
    // and `基本通过` are not.
    let first = value
        .split(|c: char| c.is_whitespace() || "，,、；;。.！!？?（(【[“\"：:".contains(c))
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    if first == "通过" {
        Verdict::Passed
    } else {
        Verdict::NotPassed
    }
}

/// `审查记录` on a line of its own, with or without `#` heading markers,
/// bold markers or a trailing colon.
fn is_marker_line(line: &str) -> bool {
    let t = line
        .trim()
        .trim_start_matches('#')
        .trim_matches(|c: char| c == '*' || c == '_' || c.is_whitespace())
        .trim_end_matches([':', '：'])
        .trim();
    t == MARKER
}

/// `- key: value` into its two halves. Accepts `*` and `+` bullets, no
/// bullet at all, bold keys, and both colons; values keep their inner
/// punctuation but lose surrounding backticks, so a sha written as
/// `` `abc…` `` reads as the sha.
fn split_field(line: &str) -> Option<(String, String)> {
    let line = line.trim().trim_start_matches(['-', '*', '+']).trim();
    let at = line.find([':', '：'])?;
    let key = key_of(
        line[..at]
            .trim()
            .trim_matches(|c: char| c == '*' || c == '_' || c == '`'),
    );
    let value = line[at + line[at..].chars().next().unwrap().len_utf8()..]
        .trim()
        .trim_matches(|c: char| c == '`' || c.is_whitespace())
        .to_string();
    if key.is_empty() {
        return None;
    }
    Some((key, value))
}

/// A field name as the gate compares it: without its spaces, and with
/// `SHA` and `sha` the same name.
fn key_of(key: &str) -> String {
    key.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The `Agent: <name>` trailer that says which agent wrote a pull request
/// or a commit. All agents share one GitHub account (#15), so this trailer
/// is the only thing that tells them apart. The last one wins: a body that
/// quotes the convention above its own trailer still reads as its own.
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
