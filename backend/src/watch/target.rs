//! Which sessions and tiers a watch follows.

use crate::damai::{Event, TierStatus};
use crate::report::{price_text, tier_label};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    /// Part of the session's name, such as "10-17"; "10.17" works too.
    pub session: Option<String>,
    /// Part of the tier's name, such as "380".
    pub tier: Option<String>,
}

pub(crate) fn session_matches(filter: Option<&str>, name: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(f) => name.is_some_and(|n| normal(n).contains(&normal(f))),
    }
}

fn tier_matches(filter: Option<&str>, name: &str) -> bool {
    filter.is_none_or(|f| name.contains(f.trim()))
}

/// Dates written 10.17 or 10/17 match the page's 10-17.
fn normal(s: &str) -> String {
    s.trim().replace(['.', '/'], "-")
}

/// One watched tier as of one query.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Watched {
    pub perform_id: u64,
    pub session: String,
    pub tier: String,
    pub price: Option<String>,
    pub status: TierStatus,
}

impl Watched {
    pub fn key(&self) -> (u64, String) {
        (self.perform_id, self.tier.clone())
    }

    /// "2026-10-15 周四 17:00 · C-178元 · ¥178.00 · 可售"
    pub fn line(&self) -> String {
        format!(
            "{} · {} · {} · {}",
            self.session,
            self.tier,
            price_text(self.price.as_deref()),
            tier_label(&self.status)
        )
    }
}

/// The event's tiers the filter takes. A session whose tiers couldn't be
/// read, or whose tier list damai.cn left out, contributes none.
pub(crate) fn watched(event: &Event, f: &Filter) -> Vec<Watched> {
    let mut out = Vec::new();
    for s in &event.sessions {
        if !session_matches(f.session.as_deref(), s.name.as_deref()) {
            continue;
        }
        let Ok(Some(tiers)) = &s.tiers else { continue };
        for (i, t) in tiers.iter().enumerate() {
            let tier = t
                .name
                .clone()
                .unwrap_or_else(|| format!("第 {} 个票档", i + 1));
            if tier_matches(f.tier.as_deref(), &tier) {
                out.push(Watched {
                    perform_id: s.perform_id,
                    session: s.name.clone().unwrap_or_else(|| "未知场次".into()),
                    tier,
                    price: t.price.clone(),
                    status: t.status.clone(),
                });
            }
        }
    }
    out
}

/// Why nothing matched, with what there is to pick from.
pub(crate) fn no_match(event: &Event, all_sessions: &[String], f: &Filter) -> String {
    let sessions: Vec<_> = event
        .sessions
        .iter()
        .filter(|s| session_matches(f.session.as_deref(), s.name.as_deref()))
        .collect();
    if sessions.is_empty() {
        let wanted = f.session.as_deref().unwrap_or("");
        return format!(
            "没有匹配“{wanted}”的场次。这场演出的场次：\n{}",
            list(all_sessions.iter().map(String::as_str))
        );
    }
    let tiers: Vec<String> = sessions
        .iter()
        .filter_map(|s| s.tiers.as_ref().ok().and_then(Option::as_ref))
        .flatten()
        .filter_map(|t| t.name.clone())
        .collect();
    if tiers.is_empty() {
        return "这些场次的票档都没查到，没法盯".into();
    }
    let mut unique: Vec<String> = Vec::new();
    for t in tiers {
        if !unique.contains(&t) {
            unique.push(t);
        }
    }
    format!(
        "没有匹配“{}”的票档。这些场次的票档：\n{}",
        f.tier.as_deref().unwrap_or(""),
        list(unique.iter().map(String::as_str))
    )
}

fn list<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items
        .map(|s| format!("  {s}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_filter_takes_dots_slashes_and_dashes_alike() {
        let name = Some("2026-10-17 周六 17:00");
        for f in ["10-17", "10.17", "10/17", " 10-17 ", "17:00"] {
            assert!(session_matches(Some(f), name), "{f}");
        }
        assert!(!session_matches(Some("10-18"), name));
        assert!(!session_matches(Some("10-17"), None));
        assert!(session_matches(None, None));
    }

    #[test]
    fn tier_filter_is_part_of_the_name() {
        assert!(tier_matches(Some("380"), "A-380元"));
        assert!(tier_matches(None, "A-380元"));
        assert!(!tier_matches(Some("480"), "A-380元"));
    }
}
