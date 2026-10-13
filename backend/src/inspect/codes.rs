//! Status codes in a real response that theFlash does not map yet. Only
//! codes seen on damai.cn's own pages are mapped (docs/subsystems/
//! damai-event.md); everything else prints as 未知（原始值）, and until now
//! nothing told anyone that a new code had appeared.

use serde_json::Value;

use crate::damai::{buy_button_code_is_mapped, tier_code_is_mapped};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codes {
    /// `buyBtnStatus` on the page, `itemBuyBtn.btnStatus` on the tier
    /// endpoint: the whole event's or one session's sale state.
    BuyButton,
    /// `frontEndStatus` of one tier.
    Tier,
}

impl Codes {
    pub fn name(self) -> &'static str {
        match self {
            Codes::BuyButton => "购买按钮状态码",
            Codes::Tier => "票档状态码",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unseen {
    pub what: Codes,
    pub code: String,
    /// What damai.cn shows next to it: the button's own text, or the tier's
    /// tag ("缺货登记"). Whoever maps the code needs these words.
    pub text: Option<String>,
    /// Where in the response it was, and which tier it belongs to.
    pub at: String,
}

/// Codes in the detail page's JSON that theFlash does not map.
pub fn unseen_on_item_page(root: &Value) -> Vec<Unseen> {
    let data = &root["dataDefault"];
    button(
        &data["buyBtnStatus"],
        &data["buyBtnText"],
        "dataDefault.buyBtnStatus",
    )
    .into_iter()
    .collect()
}

/// Codes in one session's tier response that theFlash does not map.
pub fn unseen_on_tiers(root: &Value) -> Vec<Unseen> {
    let mut out: Vec<Unseen> = button(
        &root["itemBuyBtn"]["btnStatus"],
        &root["itemBuyBtn"]["btnText"],
        "itemBuyBtn.btnStatus",
    )
    .into_iter()
    .collect();
    for (i, sku) in root["perform"]["skuList"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(code) = code_text(&sku["frontEndStatus"]) else {
            continue;
        };
        if tier_code_is_mapped(&code) {
            continue;
        }
        out.push(Unseen {
            what: Codes::Tier,
            code,
            text: tag_text(sku),
            at: match text(&sku["priceName"]) {
                Some(name) => format!("perform.skuList[{i}].frontEndStatus（{name}）"),
                None => format!("perform.skuList[{i}].frontEndStatus"),
            },
        });
    }
    out
}

fn button(status: &Value, button_text: &Value, at: &str) -> Option<Unseen> {
    let code = code_text(status)?;
    (!buy_button_code_is_mapped(&code)).then(|| Unseen {
        what: Codes::BuyButton,
        code,
        text: text(button_text),
        at: at.to_string(),
    })
}

/// The words damai.cn puts on a tier: the tag beside it on the page.
fn tag_text(sku: &Value) -> Option<String> {
    text(&sku["otherTag"]["tagDesc"]).or_else(|| {
        sku["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|t| text(&t["tagDesc"]))
    })
}

/// A status code as damai.cn writes it: a number on the page, the same
/// value as a string on the tier endpoint.
fn code_text(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => text(v).or_else(|| (!s.is_empty()).then(|| s.clone())),
        _ => None,
    }
}

fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_page_with_a_code_theflash_maps_reports_nothing() {
        let page = json!({"dataDefault": {"buyBtnStatus": 204, "buyBtnText": "立即购票"}});
        assert_eq!(unseen_on_item_page(&page), []);
    }

    #[test]
    fn a_page_with_a_code_theflash_does_not_map_reports_it_with_the_page_words() {
        let page = json!({"dataDefault": {"buyBtnStatus": 777, "buyBtnText": "开抢提醒"}});
        assert_eq!(
            unseen_on_item_page(&page),
            [Unseen {
                what: Codes::BuyButton,
                code: "777".into(),
                text: Some("开抢提醒".into()),
                at: "dataDefault.buyBtnStatus".into(),
            }]
        );
    }

    #[test]
    fn a_tier_response_reports_unseen_session_and_tier_codes() {
        let tiers = json!({
            "itemBuyBtn": {"btnStatus": "888", "btnText": "敬请期待"},
            "perform": {"skuList": [
                {"frontEndStatus": "1", "priceName": "甲", "skuSalable": "true"},
                {"frontEndStatus": "9", "priceName": "乙", "otherTag": {"tagDesc": "等候中"}},
            ]},
        });
        let unseen = unseen_on_tiers(&tiers);
        assert_eq!(
            unseen,
            [
                Unseen {
                    what: Codes::BuyButton,
                    code: "888".into(),
                    text: Some("敬请期待".into()),
                    at: "itemBuyBtn.btnStatus".into(),
                },
                Unseen {
                    what: Codes::Tier,
                    code: "9".into(),
                    text: Some("等候中".into()),
                    at: "perform.skuList[1].frontEndStatus（乙）".into(),
                },
            ]
        );
    }

    #[test]
    fn a_tier_tag_without_other_tag_falls_back_to_the_tags_list() {
        let tiers = json!({"perform": {"skuList": [
            {"frontEndStatus": "7", "tags": [{"tagDesc": "缺货登记"}]},
        ]}});
        assert_eq!(unseen_on_tiers(&tiers)[0].text.as_deref(), Some("缺货登记"));
    }

    /// The codes damai.cn sends today, from the responses in testdata: a
    /// run with no change must stay quiet about all of them.
    #[test]
    fn the_codes_seen_on_damai_so_far_are_all_mapped() {
        for code in ["204", "206", "106", "92", "100"] {
            assert!(buy_button_code_is_mapped(code), "{code}");
        }
        for code in ["1", "2", "4"] {
            assert!(tier_code_is_mapped(code), "{code}");
        }
        assert!(!buy_button_code_is_mapped("777"));
        assert!(!tier_code_is_mapped("777"));
        assert!(!buy_button_code_is_mapped("204x"));
    }
}
