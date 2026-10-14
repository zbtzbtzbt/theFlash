//! An event as theFlash reports it, and how damai.cn's codes map onto it.
//! Only codes seen on damai.cn's own pages are mapped; any other reads as
//! Unknown with the code, never as a guess.

use super::page::BuyButton;
use super::sku::SkuRaw;

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub item_id: u64,
    pub name: Option<String>,
    pub city: Option<String>,
    pub venue: Option<String>,
    pub sale: SaleState,
    pub sell_start_ms: Option<i64>,
    pub sessions: Vec<Session>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SaleState {
    OnSale,
    NotYetOnSale,
    SoldOut,
    /// The PC site doesn't sell it ("该渠道不支持购票"), and shows no
    /// sessions or tiers for it.
    NotOnPcChannel,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub perform_id: u64,
    pub name: Option<String>,
    /// Ok(None): list missing/null; Ok(Some([])): explicitly empty.
    /// Err holds why this session's tiers couldn't be read.
    pub tiers: Result<Option<Vec<Tier>>, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tier {
    pub name: Option<String>,
    pub price: Option<String>,
    pub status: TierStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TierStatus {
    Available,
    /// 缺货登记: none left now; damai.cn takes sign-ups for a restock.
    OutOfStock,
    NotYetOnSale,
    SoldOut,
    Unknown(String),
}

/// The buy button's status code, as the detail page and the tier endpoint
/// write it: 204 立即购票 / 立即预订, 206 选座购票, 106 预约抢票, 92 售罄,
/// 100 该渠道不支持购票.
pub(crate) fn sale_state(button: Option<&BuyButton>) -> SaleState {
    let Some(b) = button else {
        return SaleState::Unknown("没有购买按钮信息".into());
    };
    match b.status {
        Some(204 | 206) => SaleState::OnSale,
        Some(106) => SaleState::NotYetOnSale,
        Some(92) => SaleState::SoldOut,
        Some(100) => SaleState::NotOnPcChannel,
        Some(code) => SaleState::Unknown(format!(
            "购买按钮状态 {code}「{}」",
            b.text.as_deref().unwrap_or("")
        )),
        None => SaleState::Unknown(format!(
            "购买按钮没有状态码「{}」",
            b.text.as_deref().unwrap_or("")
        )),
    }
}

/// The sale state a buy-button status code maps to, or None for a code
/// theFlash has not seen on damai.cn's own pages. The structure inspection
/// (docs/subsystems/damai-inspect.md) asks this so that a code nobody has
/// mapped yet is reported instead of quietly printing as 未知.
pub fn sale_state_of_code(code: &str) -> Option<SaleState> {
    let status = code.trim().parse().ok()?;
    match sale_state(Some(&BuyButton {
        status: Some(status),
        text: None,
    })) {
        SaleState::Unknown(_) => None,
        state => Some(state),
    }
}

pub fn buy_button_code_is_mapped(code: &str) -> bool {
    sale_state_of_code(code).is_some()
}

/// Whether a `frontEndStatus` maps to a tier status. Asked with a session
/// that is on sale and a tier that is salable, which is the combination
/// every mapped code reads under.
pub fn tier_code_is_mapped(code: &str) -> bool {
    let sku = SkuRaw {
        name: None,
        price: None,
        front_end_status: Some(code.to_string()),
        salable: Some(true),
    };
    !matches!(
        tier_status(&SaleState::OnSale, &sku),
        TierStatus::Unknown(_)
    )
}

/// A tier's status within its session. frontEndStatus 1 alone doesn't mean
/// the tier can be bought: before the sale opens damai.cn sends 1 with
/// skuSalable true (the page shows 可预约), so the session's state decides.
pub(crate) fn tier_status(session: &SaleState, sku: &SkuRaw) -> TierStatus {
    match sku.front_end_status.as_deref() {
        Some("4") => TierStatus::SoldOut,
        Some("2") => TierStatus::OutOfStock,
        Some("1") => match (session, sku.salable) {
            (SaleState::NotYetOnSale, _) => TierStatus::NotYetOnSale,
            (SaleState::OnSale, Some(true)) => TierStatus::Available,
            (SaleState::OnSale, salable) => {
                TierStatus::Unknown(format!("frontEndStatus 1，skuSalable {salable:?}"))
            }
            (other, _) => TierStatus::Unknown(format!("frontEndStatus 1，场次状态 {other:?}")),
        },
        Some(code) => TierStatus::Unknown(format!("frontEndStatus {code}")),
        None => TierStatus::Unknown("没有 frontEndStatus".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(status: i64) -> BuyButton {
        BuyButton {
            status: Some(status),
            text: Some("文字".into()),
        }
    }

    fn sku(fes: &str, salable: bool) -> SkuRaw {
        SkuRaw {
            name: None,
            price: None,
            front_end_status: Some(fes.into()),
            salable: Some(salable),
        }
    }

    #[test]
    fn button_codes_seen_on_damai_map_to_sale_states() {
        assert_eq!(sale_state(Some(&button(204))), SaleState::OnSale);
        assert_eq!(sale_state(Some(&button(206))), SaleState::OnSale);
        assert_eq!(sale_state(Some(&button(106))), SaleState::NotYetOnSale);
        assert_eq!(sale_state(Some(&button(92))), SaleState::SoldOut);
        assert_eq!(sale_state(Some(&button(100))), SaleState::NotOnPcChannel);
    }

    /// The mapped codes are one list: asking "does this code map?" and
    /// mapping it cannot drift apart.
    #[test]
    fn code_mapping_answers_the_same_as_reading_the_button() {
        for code in 0..1000 {
            let text = code.to_string();
            let mapped = sale_state(Some(&button(code)));
            match (sale_state_of_code(&text), mapped) {
                (None, SaleState::Unknown(_)) => {}
                (Some(a), b) => assert_eq!(a, b, "{code}"),
                (None, b) => panic!("{code}：读成 {b:?}，却说没映射"),
            }
        }
        assert_eq!(sale_state_of_code(" 204 "), Some(SaleState::OnSale));
        assert_eq!(sale_state_of_code("两百零四"), None);
    }

    #[test]
    fn an_unseen_button_code_is_unknown_and_names_the_code() {
        let SaleState::Unknown(why) = sale_state(Some(&button(777))) else {
            panic!("777 mapped to a known state");
        };
        assert!(why.contains("777"), "{why}");
        assert!(matches!(sale_state(None), SaleState::Unknown(_)));
    }

    #[test]
    fn tier_before_the_sale_is_not_yet_on_sale_though_it_reads_salable() {
        assert_eq!(
            tier_status(&SaleState::NotYetOnSale, &sku("1", true)),
            TierStatus::NotYetOnSale
        );
    }

    #[test]
    fn tier_codes_seen_on_damai_map_to_tier_states() {
        assert_eq!(
            tier_status(&SaleState::OnSale, &sku("1", true)),
            TierStatus::Available
        );
        assert_eq!(
            tier_status(&SaleState::OnSale, &sku("2", false)),
            TierStatus::OutOfStock
        );
        assert_eq!(
            tier_status(&SaleState::SoldOut, &sku("4", false)),
            TierStatus::SoldOut
        );
    }

    #[test]
    fn an_unseen_tier_code_or_a_contradiction_is_unknown() {
        let TierStatus::Unknown(why) = tier_status(&SaleState::OnSale, &sku("9", true)) else {
            panic!("frontEndStatus 9 mapped to a known state");
        };
        assert!(why.contains('9'), "{why}");
        assert!(matches!(
            tier_status(&SaleState::OnSale, &sku("1", false)),
            TierStatus::Unknown(_)
        ));
        assert!(matches!(
            tier_status(&SaleState::SoldOut, &sku("1", true)),
            TierStatus::Unknown(_)
        ));
    }
}
