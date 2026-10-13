//! The per-session tier endpoint the detail page calls through JSONP:
//! https://detail.damai.cn/subpage?...&scenario=itemsku&dataType=2&dataId=<performId>.

use serde_json::Value;

use super::page::BuyButton;
use super::text::{integer, string};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SkuRaw {
    pub name: Option<String>,
    pub price: Option<String>,
    /// "1", "2", "4" seen so far; see docs/subsystems/damai-event.md.
    pub front_end_status: Option<String>,
    pub salable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PerformSkus {
    pub perform_id: Option<u64>,
    /// The buy button for this session, which can differ from the page's.
    pub buy_button: Option<BuyButton>,
    /// None means skuList was missing or null; Some([]) is explicitly empty.
    pub skus: Option<Vec<SkuRaw>>,
}

pub(crate) fn tier_url(base: &str, item_id: u64, perform_id: u64) -> String {
    format!(
        "{base}/subpage?itemId={item_id}&apiVersion=2.0&dmChannel=pc@damai_pc&bizCode=ali.china.damai&scenario=itemsku&dataType=2&dataId={perform_id}&privilegeActId=&callback=__jp0"
    )
}

/// The JSON a JSONP response carries, which is what the tier endpoint
/// answers inside its callback. Public so the structure inspection sees
/// damai.cn's own shape rather than what `parse_sku_jsonp` read out of it.
pub fn tier_json(body: &str) -> Result<Value, String> {
    let (Some(open), Some(close)) = (body.find('('), body.rfind(')')) else {
        return Err(format!("不是 JSONP：{}", head(body)));
    };
    if close <= open {
        return Err(format!("不是 JSONP：{}", head(body)));
    }
    serde_json::from_str(&body[open + 1..close]).map_err(|e| format!("JSON 无效：{e}"))
}

pub(crate) fn parse_sku_jsonp(body: &str) -> Result<PerformSkus, String> {
    parse_sku_json(&tier_json(body)?)
}

/// The tiers in a response damai.cn already sent, which is what the
/// structure inspection has in hand. Kept apart from `parse_sku_jsonp` so
/// "what this response means" has one definition: a run that only compared
/// field paths would call a response theFlash cannot read "unchanged".
pub(crate) fn parse_sku_json(data: &Value) -> Result<PerformSkus, String> {
    let info = &data["responseInfo"];
    if info["responseSuccess"].as_str() == Some("false") {
        return Err(format!(
            "接口返回失败（responseCode {}）",
            string(&info["responseCode"]).unwrap_or_else(|| "空".into())
        ));
    }

    let perform = &data["perform"];
    let buy_button = {
        let b = &data["itemBuyBtn"];
        let status = integer(&b["btnStatus"]);
        let text = string(&b["btnText"]);
        (status.is_some() || text.is_some()).then_some(BuyButton { status, text })
    };
    let skus = match &perform["skuList"] {
        Value::Null => None,
        Value::Array(values) => Some(
            values
                .iter()
                .map(|s| SkuRaw {
                    name: string(&s["priceName"]),
                    price: string(&s["price"]),
                    front_end_status: string(&s["frontEndStatus"]),
                    salable: match s["skuSalable"].as_str() {
                        Some("true") => Some(true),
                        Some("false") => Some(false),
                        _ => s["skuSalable"].as_bool(),
                    },
                })
                .collect(),
        ),
        _ => return Err("perform.skuList 不是数组".into()),
    };

    Ok(PerformSkus {
        perform_id: integer(&perform["performId"]).and_then(|i| u64::try_from(i).ok()),
        buy_button,
        skus,
    })
}

/// Why this response carries no tiers for the session that was asked
/// about, or None when it does. damai.cn answers a session that is gone,
/// or a request it did not like, with a body of exactly the right shape:
/// only `responseSuccess` and the session id say so. Public so the
/// structure inspection asks the same question a query does.
pub fn tier_answer_problem(data: &Value, asked: u64) -> Option<String> {
    match parse_sku_json(data) {
        Err(why) => Some(why),
        Ok(p) => session_mismatch(&p, asked),
    }
}

/// Why these tiers are not the ones that were asked for, or None when they
/// are. Both the event query and the inspection ask here, so a response
/// about another session cannot be a failure in one and fine in the other.
pub(crate) fn session_mismatch(p: &PerformSkus, asked: u64) -> Option<String> {
    (p.perform_id != Some(asked)).then(|| {
        format!(
            "接口返回的是另一个场次（{}）",
            p.perform_id
                .map_or_else(|| "没有场次 ID".to_string(), |id| id.to_string())
        )
    })
}

fn head(s: &str) -> String {
    s.chars().take(80).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/testdata/damai/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn tiers_of_the_session_asked_for() {
        let p = parse_sku_jsonp(&fixture("sku_on_sale_286937790.jsonp")).unwrap();
        assert_eq!(p.perform_id, Some(286937790));
        assert_eq!(p.buy_button.as_ref().and_then(|b| b.status), Some(204));
        let skus = p.skus.unwrap();
        assert_eq!(skus.len(), 5);
        assert_eq!(
            skus[0],
            SkuRaw {
                name: Some("D-118元".into()),
                price: Some("118.00".into()),
                front_end_status: Some("2".into()),
                salable: Some(false),
            }
        );
    }

    #[test]
    fn sold_out_session_has_its_own_button() {
        let p = parse_sku_jsonp(&fixture("sku_sold_out_284490009.jsonp")).unwrap();
        assert_eq!(p.buy_button.as_ref().and_then(|b| b.status), Some(92));
        assert!(
            p.skus
                .unwrap()
                .iter()
                .all(|s| s.front_end_status.as_deref() == Some("4"))
        );
    }

    #[test]
    fn a_failed_response_is_an_error() {
        let body = r#"__jp0({"responseInfo":{"responseSuccess":"false","responseCode":"X1"}})"#;
        let err = parse_sku_jsonp(body).unwrap_err();
        assert!(err.contains("X1"), "{err}");
    }

    #[test]
    fn a_page_that_is_not_jsonp_is_an_error() {
        assert!(parse_sku_jsonp("<html>oops</html>").is_err());
    }
}
