//! The per-session tier endpoint the detail page calls through JSONP:
//! https://detail.damai.cn/subpage?...&scenario=itemsku&dataType=2&dataId=<performId>.

use serde_json::Value;

use super::page::{BuyButton, perform_id};
use super::text::{integer, string};
// The owner's rules about credentials and personal data, kept in one
// reading for the whole repository (docs/code-standards.md). They live
// under `inspect` because the fixtures it records were the first thing
// that needed them; what theFlash says about a response needs the same
// rules (#72).
use crate::inspect::redact;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SkuRaw {
    pub sku_id: Option<String>,
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
    pub project_id: Option<String>,
    pub sign_key: Option<String>,
    pub has_promotion: Option<String>,
    pub rtc: Option<bool>,
    pub choose_seat: bool,
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
            head(&string(&info["responseCode"]).unwrap_or_else(|| "空".into()))
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
                    sku_id: string(&s["skuId"])
                        .or_else(|| integer(&s["skuId"]).map(|n| n.to_string())),
                    name: string(&s["priceName"]),
                    price: sku_price(s),
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

    let basic = &data["itemBasicInfo"];
    let project_id =
        string(&basic["projectId"]).or_else(|| integer(&basic["projectId"]).map(|n| n.to_string()));
    let sign_key = string(&basic["t"]);
    let has_promotion = string(&perform["hasPromotion"]);
    let rtc = match data["actionControl"]["tradeControl"]["rtc"].as_str() {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => data["actionControl"]["tradeControl"]["rtc"].as_bool(),
    };
    let seat_btn = matches!(
        buy_button.as_ref().and_then(|b| b.status),
        Some(206 | 231 | 217 | 224 | 87)
    );
    let choose_seat = seat_btn || integer(&perform["chooseSeatType"]) == Some(1);

    Ok(PerformSkus {
        perform_id: perform_id(&perform["performId"]),
        buy_button,
        skus,
        project_id,
        sign_key,
        has_promotion,
        rtc,
        choose_seat,
    })
}

/// The price of one tier as `report::price_text` formats it (with a
/// leading `¥`). For a tier without promotion, that is `price` verbatim.
/// For a tier with `hasPromotion` true, damai.cn puts the discounted
/// price in `price`, the face value in `dashPrice`, and the promotion
/// description in `mktPromotionTips` ("¥108票档，5折，到手¥54"); whether an
/// account gets the discount is only decided after signing in, so the
/// face value, the promotion details, and that caveat are all kept (#179).
fn sku_price(s: &Value) -> Option<String> {
    let price = string(&s["price"]);
    let has_promotion =
        s["hasPromotion"].as_str() == Some("true") || s["hasPromotion"].as_bool() == Some(true);
    if !has_promotion {
        return price;
    }
    let dash_price = string(&s["dashPrice"]);
    let tips = string(&s["mktPromotionTips"]);
    let base = dash_price.or_else(|| price.clone())?;
    let detail = match (tips, price) {
        (Some(tips), _) => tips,
        (None, Some(p)) if p != base => format!("到手¥{p}"),
        (None, _) => return Some(base),
    };
    Some(format!("{base}（优惠：{detail}；是否享受以登录账号为准）"))
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

/// The start of something damai.cn sent, as an error message may quote it:
/// enough to tell a slider challenge from a new layout, with the owner's
/// rules applied to it first. A query prints this on stderr and
/// `theflash inspect` puts the same string in its report and in the issue
/// it opens, so a logged-in run used to paste its own session into both
/// (#72). Redacted before it is cut down, so a value cannot survive by
/// being split in half.
fn head(s: &str) -> String {
    redact::for_report(s).chars().take(80).collect()
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
                sku_id: Some("6141329425570".into()),
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

    // Item 1089383688003 session 286929360 (#179): tiers with `hasPromotion`
    // true carry the discounted price in `price`, the face value in
    // `dashPrice`, and the promotion details in `mktPromotionTips`; tiers
    // without promotion keep `price` verbatim.
    #[test]
    fn promotion_tiers_include_face_price_and_promotion_tips_while_regular_tiers_stay_verbatim() {
        let data = serde_json::json!({
            "responseInfo": {"responseSuccess": "true", "responseCode": "0"},
            "itemBuyBtn": {"btnStatus": "204", "btnText": "立即购票"},
            "perform": {
                "performId": "286929360",
                "skuList": [
                    {
                        "skuId": "1",
                        "priceName": "早鸟票88元",
                        "price": "88.00",
                        "dashPrice": "88.00",
                        "hasPromotion": "false",
                        "mktPromotionTips": "",
                        "frontEndStatus": "1",
                        "skuSalable": "true"
                    },
                    {
                        "skuId": "2",
                        "priceName": "现场票108元",
                        "price": "54",
                        "dashPrice": "108.00",
                        "hasPromotion": "true",
                        "mktPromotionTips": "¥108票档，5折，到手¥54",
                        "frontEndStatus": "1",
                        "skuSalable": "true"
                    },
                    {
                        "skuId": "3",
                        "priceName": "VIP票188元（手链+小卡）",
                        "price": "94",
                        "dashPrice": "188.00",
                        "hasPromotion": true,
                        "mktPromotionTips": "¥188票档，5折，到手¥94",
                        "frontEndStatus": "1",
                        "skuSalable": "true"
                    }
                ]
            }
        });
        let p = parse_sku_json(&data).unwrap();
        let skus = p.skus.unwrap();
        assert_eq!(skus[0].price.as_deref(), Some("88.00"));
        assert_eq!(
            skus[1].price.as_deref(),
            Some("108.00（优惠：¥108票档，5折，到手¥54；是否享受以登录账号为准）")
        );
        assert_eq!(
            skus[2].price.as_deref(),
            Some("188.00（优惠：¥188票档，5折，到手¥94；是否享受以登录账号为准）")
        );
        assert_eq!(
            crate::report::price_text(skus[0].price.as_deref()),
            "¥88.00"
        );
        assert_eq!(
            crate::report::price_text(skus[1].price.as_deref()),
            "¥108.00（优惠：¥108票档，5折，到手¥54；是否享受以登录账号为准）"
        );
        assert_eq!(
            crate::report::price_text(skus[2].price.as_deref()),
            "¥188.00（优惠：¥188票档，5折，到手¥94；是否享受以登录账号为准）"
        );
    }
}
