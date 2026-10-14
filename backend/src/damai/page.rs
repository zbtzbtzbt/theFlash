//! The PC detail page, https://detail.damai.cn/item.htm?id=<item>. Two hidden
//! divs hold JSON: staticDataDefault (name, city, venue) and dataDefault
//! (buy button, sale start, sessions).

use serde_json::Value;

use super::text::{integer, string};
// See the note in sku.rs: one reading of the owner's rules, for fixtures
// and for what theFlash says about a response alike.
use crate::inspect::redact;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BuyButton {
    pub status: Option<i64>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PerformRef {
    pub id: u64,
    pub name: Option<String>,
    /// Start of the session, ms since the epoch; sessions are listed by it.
    pub date_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ItemPage {
    pub name: Option<String>,
    pub city: Option<String>,
    pub venue: Option<String>,
    pub buy_button: Option<BuyButton>,
    pub sell_start_ms: Option<i64>,
    /// In date order, as the page lists them.
    pub performs: Vec<PerformRef>,
}

/// The page's data, or None for a page without dataDefault, which is what
/// damai.cn serves (with HTTP 200) for an item that doesn't exist.
pub(crate) fn parse_item_page(html: &str) -> Result<Option<ItemPage>, String> {
    let Some(json) = item_page_json(html)? else {
        return Ok(None);
    };
    read_item_page(&json).map(Some)
}

/// A session id as damai.cn writes it: a number on the detail page, the
/// same value as a string on the tier endpoint. Only a value theFlash can
/// put in a tier request counts, because `dataId=<performId>` is the only
/// thing a session id is for: a word, a negative number, a fraction or one
/// past the integer range is not a session. One definition, so the query,
/// the tier reader and the structure inspection cannot disagree about
/// which ids damai.cn's page offers (#73).
pub fn perform_id(v: &Value) -> Option<u64> {
    integer(v).and_then(|i| u64::try_from(i).ok())
}

/// Why theFlash cannot read this detail page, or None when it can. Kept
/// apart from `parse_item_page` so "what this response means" has one
/// definition: the fields can all be in place and of the kinds declared
/// while a `performId` is a word, which leaves a query with no session to
/// ask tiers for. The structure inspection asks here rather than reading
/// ids its own looser way (#73), the way it asks
/// [`super::tier_answer_problem`] about a tier response.
pub fn item_page_problem(json: &Value) -> Option<String> {
    read_item_page(json).err()
}

/// The page's data out of the JSON damai.cn embedded in it.
fn read_item_page(json: &Value) -> Result<ItemPage, String> {
    let data = &json["dataDefault"];
    let stat = &json["staticDataDefault"];

    let buy_button = {
        let status = integer(&data["buyBtnStatus"]);
        let text = string(&data["buyBtnText"]);
        (status.is_some() || text.is_some()).then_some(BuyButton { status, text })
    };

    // A page that lists its days in a row has them in performBases; a page
    // that picks the day on a calendar leaves that empty and has them by
    // month in calendarPerforms[].performBases.
    let bases = data["performBases"].as_array().into_iter().flatten().chain(
        data["calendarPerforms"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|month| month["performBases"].as_array().into_iter().flatten()),
    );
    let mut performs: Vec<PerformRef> = Vec::new();
    for base in bases {
        for p in base["performs"].as_array().into_iter().flatten() {
            let Some(id) = perform_id(&p["performId"]) else {
                // The session as damai.cn wrote it, so whoever reads this
                // can see which value was refused — with the owner's rules
                // applied, because a query prints it on stderr and
                // `theflash inspect` puts it in the issue it opens (#72).
                return Err(format!(
                    "场次缺少 performId：{}",
                    redact::for_report(&p.to_string())
                ));
            };
            if performs.iter().any(|known| known.id == id) {
                continue;
            }
            performs.push(PerformRef {
                id,
                name: string(&p["performName"]),
                date_ms: integer(&p["performDate"]),
            });
        }
    }
    performs.sort_by_key(|p| (p.date_ms.is_none(), p.date_ms));

    Ok(ItemPage {
        name: string(&stat["itemBase"]["itemName"]),
        city: string(&stat["itemBase"]["cityName"])
            .or_else(|| string(&stat["venue"]["venueCityName"])),
        venue: string(&stat["venue"]["venueName"]),
        buy_button,
        sell_start_ms: integer(&data["sellStartTime"]),
        performs,
    })
}

/// The page's two embedded JSON blobs side by side, under the names the
/// page gives them, so a field path reads the way
/// docs/subsystems/damai-event.md writes it
/// (`dataDefault.performBases`). None for a page without `dataDefault`,
/// which is what damai.cn serves for an item that doesn't exist. This is
/// what the structure inspection looks at, so it sees damai.cn's own
/// shape rather than what `parse_item_page` managed to read out of it.
pub fn item_page_json(html: &str) -> Result<Option<Value>, String> {
    let Some(data) = embedded_json(html, "dataDefault") else {
        return Ok(None);
    };
    let data = data.map_err(|e| format!("dataDefault 不是有效的 JSON：{e}"))?;
    let stat = match embedded_json(html, "staticDataDefault") {
        Some(Ok(v)) => v,
        Some(Err(e)) => return Err(format!("staticDataDefault 不是有效的 JSON：{e}")),
        None => Value::Null,
    };
    Ok(Some(serde_json::json!({
        "dataDefault": data,
        "staticDataDefault": stat,
    })))
}

/// The JSON value that starts right after the opening tag of the element
/// with this id. It is read as one JSON value, so a "</div>" inside one of
/// its strings doesn't cut it short.
fn embedded_json(html: &str, id: &str) -> Option<Result<Value, serde_json::Error>> {
    let at = html.find(&format!("id=\"{id}\""))?;
    let start = at + html[at..].find('>')? + 1;
    let mut values = serde_json::Deserializer::from_str(&html[start..]).into_iter::<Value>();
    values.next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(name: &str) -> ItemPage {
        let html = std::fs::read_to_string(format!(
            "{}/testdata/damai/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        parse_item_page(&html).unwrap().expect("a page with data")
    }

    #[test]
    fn on_sale_page_has_its_name_place_button_and_sessions_in_date_order() {
        let p = page("item_on_sale.html");
        assert_eq!(p.name.as_deref(), Some("【北京】2026德玛西亚杯国际邀请赛"));
        assert_eq!(p.city.as_deref(), Some("北京市"));
        assert_eq!(p.venue.as_deref(), Some("北京智慧电竞赛事中心"));
        assert_eq!(p.buy_button.as_ref().and_then(|b| b.status), Some(204));
        let ids: Vec<u64> = p.performs.iter().map(|p| p.id).collect();
        assert_eq!(ids, [286937789, 286937790, 286937788]);
        assert_eq!(p.performs[0].name.as_deref(), Some("2026-10-14 周三 17:00"));
    }

    // A page that picks the day on a calendar (1084537866279) leaves
    // performBases empty and lists its sessions by month in calendarPerforms.
    #[test]
    fn calendar_page_has_every_session_of_every_month_in_date_order() {
        let p = page("item_calendar.html");
        assert_eq!(p.performs.len(), 18);
        let first: Vec<u64> = p.performs.iter().take(3).map(|p| p.id).collect();
        assert_eq!(first, [286925075, 286938129, 286938130]);
        assert_eq!(
            p.performs.last().and_then(|p| p.name.as_deref()),
            Some("2026-11-08 星期日 14:30")
        );
    }

    #[test]
    fn not_yet_on_sale_page_has_its_sale_start() {
        let p = page("item_not_yet_on_sale.html");
        assert_eq!(p.buy_button.as_ref().and_then(|b| b.status), Some(106));
        // sellStartTimeStr beside it: "10月14日 10:18".
        assert_eq!(p.sell_start_ms, Some(1_791_944_280_000));
    }

    #[test]
    fn page_the_pc_site_does_not_sell_has_no_sessions() {
        let p = page("item_pc_unsupported.html");
        assert_eq!(p.buy_button.as_ref().and_then(|b| b.status), Some(100));
        assert!(p.performs.is_empty());
    }

    #[test]
    fn page_of_a_missing_item_has_no_data() {
        let html = std::fs::read_to_string(format!(
            "{}/testdata/damai/item_not_found.html",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        assert_eq!(parse_item_page(&html), Ok(None));
    }

    #[test]
    fn closing_tag_inside_a_string_does_not_cut_the_json() {
        let html = r#"<div id="dataDefault" style="display:none">{"buyBtnText":"a</div>b","performBases":[]}</div>"#;
        let p = parse_item_page(html).unwrap().unwrap();
        assert_eq!(p.buy_button.unwrap().text.as_deref(), Some("a</div>b"));
    }

    #[test]
    fn broken_data_is_an_error_not_an_empty_event() {
        let html = r#"<div id="dataDefault">{"performBases": [</div>"#;
        assert!(parse_item_page(html).is_err());
    }

    /// A session id is what goes into `dataId=<performId>`, so the only
    /// values that are one are the ones a tier request can carry. damai.cn
    /// writes them as numbers on the page and as strings on the tier
    /// endpoint, and both read the same.
    #[test]
    fn a_session_id_is_a_number_or_a_numeric_string_within_range() {
        use serde_json::json;
        assert_eq!(perform_id(&json!(286937790)), Some(286937790));
        assert_eq!(perform_id(&json!("286937790")), Some(286937790));
        assert_eq!(perform_id(&json!(" 286937790 ")), Some(286937790));
        assert_eq!(perform_id(&json!(0)), Some(0));
        for not_one in [
            json!("not-a-number"),
            json!(""),
            json!(-1),
            json!("-1"),
            json!(1.5),
            json!("1.5"),
            // Past the integer range theFlash reads, so a request built
            // from it would not be about this session.
            json!(u64::MAX),
            json!("18446744073709551615"),
            json!(null),
            json!([286937790]),
        ] {
            assert_eq!(perform_id(&not_one), None, "{not_one}");
        }
    }

    /// The fields can all be there, of the kinds damai.cn always sends,
    /// and the page still be one theFlash cannot read: a performId it
    /// refuses leaves the whole page without a session to ask tiers for.
    /// `item_page_problem` answers that question once, for the query and
    /// for the structure inspection alike (#73).
    #[test]
    fn a_page_with_a_session_id_the_reader_refuses_says_so() {
        let html = std::fs::read_to_string(format!(
            "{}/testdata/damai/item_on_sale.html",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let json = item_page_json(&html).unwrap().unwrap();
        assert_eq!(item_page_problem(&json), None);

        for bad in ["\"not-a-number\"", "-1", "1.5"] {
            let broken_html =
                html.replace("\"performId\":286937788", &format!("\"performId\":{bad}"));
            let broken = item_page_json(&broken_html).unwrap().unwrap();
            let why = item_page_problem(&broken).unwrap_or_else(|| panic!("{bad} 被当成了场次 ID"));
            assert!(why.contains("performId"), "{bad}：{why}");
            assert_eq!(
                parse_item_page(&broken_html).unwrap_err(),
                why,
                "{bad}：查询和巡检读到的不是同一件事"
            );
        }
    }
}
