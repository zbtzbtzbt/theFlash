//! The PC detail page, https://detail.damai.cn/item.htm?id=<item>. Two hidden
//! divs hold JSON: staticDataDefault (name, city, venue) and dataDefault
//! (buy button, sale start, sessions).

use serde_json::Value;

use super::text::{integer, string};

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
    let Some(data) = embedded_json(html, "dataDefault") else {
        return Ok(None);
    };
    let data = data.map_err(|e| format!("dataDefault 不是有效的 JSON：{e}"))?;
    let stat = match embedded_json(html, "staticDataDefault") {
        Some(Ok(v)) => v,
        Some(Err(e)) => return Err(format!("staticDataDefault 不是有效的 JSON：{e}")),
        None => Value::Null,
    };

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
            let Some(id) = integer(&p["performId"]).and_then(|i| u64::try_from(i).ok()) else {
                return Err(format!("场次缺少 performId：{p}"));
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

    Ok(Some(ItemPage {
        name: string(&stat["itemBase"]["itemName"]),
        city: string(&stat["itemBase"]["cityName"])
            .or_else(|| string(&stat["venue"]["venueCityName"])),
        venue: string(&stat["venue"]["venueName"]),
        buy_button,
        sell_start_ms: integer(&data["sellStartTime"]),
        performs,
    }))
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
}
