//! Browser contract. Displayed labels and times share the CLI renderer.
use crate::{
    damai::{Error, Event, SaleState, TierStatus},
    report,
};
use serde_json::{Value, json};

pub fn event(e: &Event) -> Value {
    let unknown = |s: &Option<String>| s.clone().unwrap_or_else(|| "未知".into());
    let sale_kind = match e.sale {
        SaleState::OnSale => "available",
        SaleState::NotYetOnSale => "not-yet-on-sale",
        SaleState::SoldOut => "sold-out",
        SaleState::NotOnPcChannel | SaleState::Unknown(_) => "unknown",
    };
    json!({
        "itemId": e.item_id.to_string(),
        "name": unknown(&e.name), "city": unknown(&e.city), "venue": unknown(&e.venue),
        "sale": {"kind": sale_kind, "label": report::sale_label(&e.sale)},
        "sellStart": if e.sale == SaleState::NotYetOnSale {
            Some(e.sell_start_ms.and_then(report::beijing_time).unwrap_or_else(|| "未知".into()))
        } else { None },
        "notice": if e.sessions.is_empty() {
            Some(if e.sale == SaleState::NotOnPcChannel {
                "大麦电脑版不展示这场演出的场次和票档（页面提示“该渠道不支持购票”），请在大麦 App 查看"
            } else { "页面上没有场次信息" })
        } else { None },
        "sessions": e.sessions.iter().map(|s| {
            let (tiers, error, notice) = match &s.tiers {
                Ok(Some(tiers)) => (tiers.iter().map(|t| {
                    let kind = match t.status {
                        TierStatus::Available => "available",
                        TierStatus::OutOfStock => "out-of-stock",
                        TierStatus::NotYetOnSale => "not-yet-on-sale",
                        TierStatus::SoldOut => "sold-out",
                        TierStatus::Unknown(_) => "unknown",
                    };
                    json!({"name": unknown(&t.name),
                        "price": report::price_text(t.price.as_deref()),
                        "status": {"kind": kind, "label": report::tier_label(&t.status)}})
                }).collect::<Vec<_>>(), None, None),
                Ok(None) => (vec![], None, Some("票档：未知（大麦没有提供票档列表）")),
                Err(why) => (vec![], Some(format!("票档：获取失败（{why}）")), None),
            };
            json!({"id": s.perform_id.to_string(), "name": unknown(&s.name), "tiers": tiers, "error": error, "notice": notice})
        }).collect::<Vec<_>>(),
        "report": report::render(e),
    })
}

pub fn error(e: &Error) -> Value {
    let code = match e {
        Error::InvalidInput(_) => "invalid-input",
        Error::NotFound(_) => "not-found",
        Error::RiskControl(_) => "risk-control",
        Error::Network { .. } | Error::Http { .. } => "network",
        Error::Parse { .. } => "parse",
    };
    json!({"error": {"code": code, "message": e.to_string()}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damai::{Session, Tier};
    #[test]
    fn unknown_values_and_partial_failure_are_preserved_without_losing_id_precision() {
        let e = Event {
            item_id: u64::MAX,
            name: None,
            city: None,
            venue: None,
            sale: SaleState::Unknown("购买按钮状态 777".into()),
            sell_start_ms: None,
            sessions: vec![
                Session {
                    perform_id: u64::MAX,
                    name: None,
                    tiers: Ok(Some(vec![Tier {
                        name: None,
                        price: None,
                        status: TierStatus::Unknown("frontEndStatus 9".into()),
                    }])),
                },
                Session {
                    perform_id: 2,
                    name: None,
                    tiers: Err("大麦返回了 HTTP 502".into()),
                },
            ],
        };
        let got = event(&e);
        assert_eq!(got["itemId"], u64::MAX.to_string());
        assert_eq!(got["sessions"][0]["id"], u64::MAX.to_string());
        assert_eq!(got["name"], "未知");
        assert_eq!(got["sale"]["kind"], "unknown");
        assert_eq!(got["sale"]["label"], "未知（购买按钮状态 777）");
        assert_eq!(got["sessions"][0]["tiers"][0]["price"], "未知");
        assert_eq!(
            got["sessions"][0]["tiers"][0]["status"]["label"],
            "未知（frontEndStatus 9）"
        );
        assert_eq!(
            got["sessions"][1]["error"],
            "票档：获取失败（大麦返回了 HTTP 502）"
        );
        assert!(got["sellStart"].is_null());
        assert_eq!(got["report"], report::render(&e));
    }
}
