//! The text `theflash event` prints. A value damai.cn didn't give reads 未知.

use std::fmt::Write;

use chrono::{DateTime, FixedOffset};

use crate::damai::{Event, SaleState, TierStatus};

const UNKNOWN: &str = "未知";

pub fn render(e: &Event) -> String {
    let mut out = String::new();
    let or_unknown = |v: &Option<String>| v.clone().unwrap_or_else(|| UNKNOWN.into());
    let _ = writeln!(out, "演出：{}", or_unknown(&e.name));
    let _ = writeln!(out, "城市：{}", or_unknown(&e.city));
    let _ = writeln!(out, "场馆：{}", or_unknown(&e.venue));
    let _ = writeln!(out, "售卖状态：{}", sale_label(&e.sale));
    if e.sale == SaleState::NotYetOnSale {
        let when = e
            .sell_start_ms
            .and_then(beijing_time)
            .unwrap_or_else(|| UNKNOWN.into());
        let _ = writeln!(out, "开售时间：{when}");
    }

    if e.sessions.is_empty() {
        out.push('\n');
        if e.sale == SaleState::NotOnPcChannel {
            out.push_str(
                "场次和票档：大麦电脑版不展示这场演出的场次和票档（页面提示“该渠道不支持购票”），请在大麦 App 查看\n",
            );
        } else {
            out.push_str("场次：页面上没有场次信息\n");
        }
        return out;
    }

    for s in &e.sessions {
        let _ = writeln!(out, "\n场次：{}", or_unknown(&s.name));
        match &s.tiers {
            Err(why) => {
                let _ = writeln!(out, "  票档：获取失败（{why}）");
            }
            Ok(None) => out.push_str("  票档：未知（大麦没有提供票档列表）\n"),
            Ok(Some(tiers)) if tiers.is_empty() => out.push_str("  票档：没有票档\n"),
            Ok(Some(tiers)) => {
                for t in tiers {
                    let price = price_text(t.price.as_deref());
                    let _ = writeln!(
                        out,
                        "  {} · {price} · {}",
                        or_unknown(&t.name),
                        tier_label(&t.status)
                    );
                }
            }
        }
    }
    out
}

pub(crate) fn price_text(price: Option<&str>) -> String {
    price
        .map(|p| format!("¥{p}"))
        .unwrap_or_else(|| UNKNOWN.into())
}

pub(crate) fn sale_label(s: &SaleState) -> String {
    match s {
        SaleState::OnSale => "在售".into(),
        SaleState::NotYetOnSale => "未开售".into(),
        SaleState::SoldOut => "已售罄".into(),
        SaleState::NotOnPcChannel => "电脑版不支持购票".into(),
        SaleState::Unknown(why) => format!("{UNKNOWN}（{why}）"),
    }
}

pub(crate) fn tier_label(s: &TierStatus) -> String {
    match s {
        TierStatus::Available => "可售".into(),
        TierStatus::OutOfStock => "缺货".into(),
        TierStatus::NotYetOnSale => "未开售".into(),
        TierStatus::SoldOut => "已售罄".into(),
        TierStatus::Unknown(why) => format!("{UNKNOWN}（{why}）"),
    }
}

/// damai.cn's sale times are instants; its pages show them in Beijing time.
pub(crate) fn beijing_time(ms: i64) -> Option<String> {
    let beijing = FixedOffset::east_opt(8 * 3600)?;
    let t = DateTime::from_timestamp_millis(ms)?.with_timezone(&beijing);
    Some(format!("{}（北京时间）", t.format("%Y-%m-%d %H:%M")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damai::{Session, Tier};

    fn event() -> Event {
        Event {
            item_id: 1,
            name: None,
            city: None,
            venue: None,
            sale: SaleState::NotYetOnSale,
            sell_start_ms: None,
            sessions: vec![Session {
                perform_id: 2,
                name: None,
                tiers: Ok(Some(vec![Tier {
                    name: None,
                    price: None,
                    status: TierStatus::Unknown("frontEndStatus 9".into()),
                    handoff: None,
                }])),
            }],
        }
    }

    #[test]
    fn sale_time_is_shown_in_beijing_time() {
        // damai.cn's sellStartTime for "10月14日 10:18".
        assert_eq!(
            beijing_time(1_791_944_280_000).as_deref(),
            Some("2026-10-14 10:18（北京时间）")
        );
    }

    #[test]
    fn values_damai_did_not_give_read_unknown() {
        let got = render(&event());
        assert_eq!(
            got,
            "演出：未知\n城市：未知\n场馆：未知\n售卖状态：未开售\n开售时间：未知\n\n场次：未知\n  未知 · 未知 · 未知（frontEndStatus 9）\n"
        );
    }

    #[test]
    fn a_session_whose_tiers_failed_says_why() {
        let mut e = event();
        e.sessions[0].tiers = Err("大麦返回了 HTTP 502".into());
        assert!(render(&e).contains("  票档：获取失败（大麦返回了 HTTP 502）\n"));
    }
}
