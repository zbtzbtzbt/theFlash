use std::time::Duration;

use super::Error;
use super::event::{Event, Session, Tier, sale_state, tier_status};
use super::page::parse_item_page;
use super::sku::{parse_sku_jsonp, tier_url};

pub const DEFAULT_BASE_URL: &str = "https://detail.damai.cn";

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// Between two tier requests. A calendar event has a session a day (18 on
/// 1084537866279), and damai.cn's search answered its first request with a
/// slider challenge, so a query paces itself.
const TIER_REQUEST_GAP: Duration = Duration::from_millis(300);

/// Reads events from damai.cn without signing in. Requests go one at a time:
/// a query has no reason to burst at damai.cn.
pub struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    /// `base` is DEFAULT_BASE_URL outside tests.
    pub fn new(base: impl Into<String>) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| Error::Network {
                url: String::new(),
                reason: chain(&e),
            })?;
        Ok(Client {
            http,
            base: base.into().trim_end_matches('/').to_string(),
        })
    }

    /// The event and the tiers of each of its sessions. A risk-control page
    /// anywhere stops the whole query; any other failure on one session's
    /// tiers is reported on that session.
    pub async fn event(&self, item_id: u64) -> Result<Event, Error> {
        let page_url = format!("{}/item.htm?id={item_id}", self.base);
        let html = self.get(&page_url, None).await?;
        let page = parse_item_page(&html)
            .map_err(|reason| Error::Parse {
                what: "演出详情页",
                reason,
            })?
            .ok_or(Error::NotFound(item_id))?;
        let sale = sale_state(page.buy_button.as_ref());

        let mut sessions = Vec::with_capacity(page.performs.len());
        for (i, perform) in page.performs.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(TIER_REQUEST_GAP).await;
            }
            let url = tier_url(&self.base, item_id, perform.id);
            let tiers = match self.get(&url, Some(&page_url)).await {
                Ok(body) => match parse_sku_jsonp(&body) {
                    Ok(p) if p.perform_id != Some(perform.id) => {
                        Err(format!("接口返回的是另一个场次（{:?}）", p.perform_id))
                    }
                    Ok(p) => {
                        let state = match &p.buy_button {
                            Some(b) => sale_state(Some(b)),
                            None => sale.clone(),
                        };
                        Ok(p.skus
                            .iter()
                            .map(|s| Tier {
                                name: s.name.clone(),
                                price: s.price.clone(),
                                status: tier_status(&state, s),
                            })
                            .collect())
                    }
                    Err(reason) => Err(format!("看不懂票档数据：{reason}")),
                },
                Err(e @ Error::RiskControl(_)) => return Err(e),
                Err(e) => Err(e.to_string()),
            };
            sessions.push(Session {
                perform_id: perform.id,
                name: perform.name.clone(),
                tiers,
            });
        }

        Ok(Event {
            item_id,
            name: page.name,
            city: page.city,
            venue: page.venue,
            sale,
            sell_start_ms: page.sell_start_ms,
            sessions,
        })
    }

    async fn get(&self, url: &str, referer: Option<&str>) -> Result<String, Error> {
        let mut req = self.http.get(url);
        if let Some(r) = referer {
            req = req.header(reqwest::header::REFERER, r);
        }
        let network = |e: reqwest::Error| Error::Network {
            url: url.to_string(),
            reason: chain(&e),
        };
        let resp = req.send().await.map_err(network)?;
        let status = resp.status();
        let body = resp.text().await.map_err(network)?;
        if is_risk_control(&body) {
            return Err(Error::RiskControl(url.to_string()));
        }
        if !status.is_success() {
            return Err(Error::Http {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }
        Ok(body)
    }
}

/// damai.cn's slider challenge, served in place of the page asked for.
fn is_risk_control(body: &str) -> bool {
    body.contains("_____tmd_____") || body.contains("x5secdata")
}

/// The error with every cause under it, which is where reqwest says what
/// actually went wrong ("Connection refused").
fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push('：');
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}

/// The item id in a detail page link (`id=` on detail.damai.cn, `itemId=`
/// on m.damai.cn), or a bare id.
pub fn parse_item_id(input: &str) -> Result<u64, Error> {
    let s = input.trim();
    let invalid = || Error::InvalidInput(input.to_string());
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse().map_err(|_| invalid());
    }
    let query = s.split_once('?').map(|(_, q)| q).ok_or_else(invalid)?;
    let query = query.split('#').next().unwrap_or("");
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=')
            && (key == "id" || key == "itemId")
            && !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit())
        {
            return value.parse().map_err(|_| invalid());
        }
    }
    Err(invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_id_from_links_and_bare_ids() {
        for (input, want) in [
            (
                "https://detail.damai.cn/item.htm?id=1086791513717",
                1086791513717,
            ),
            (
                "https://detail.damai.cn/item.htm?spm=a2oeg.home.card_0.ditem_1.591b23e1&id=1086791513717",
                1086791513717,
            ),
            (
                "https://m.damai.cn/damai/detail/item.html?itemId=1086791513717&from=share",
                1086791513717,
            ),
            (
                "https://detail.damai.cn/item.htm?id=1086791513717#top",
                1086791513717,
            ),
            ("1086791513717", 1086791513717),
            ("  1086791513717\n", 1086791513717),
        ] {
            assert_eq!(parse_item_id(input).ok(), Some(want), "{input}");
        }
    }

    #[test]
    fn input_without_an_item_id_is_refused() {
        for input in [
            "",
            "abc",
            "https://detail.damai.cn/item.htm",
            "https://detail.damai.cn/item.htm?id=",
            "https://detail.damai.cn/item.htm?id=12ab",
            "https://detail.damai.cn/item.htm?pid=123",
            "99999999999999999999999",
        ] {
            assert!(
                matches!(parse_item_id(input), Err(Error::InvalidInput(_))),
                "{input}"
            );
        }
    }

    #[test]
    fn slider_challenge_is_recognised_and_normal_pages_are_not() {
        let dir = format!("{}/testdata/damai", env!("CARGO_MANIFEST_DIR"));
        let read = |n: &str| std::fs::read_to_string(format!("{dir}/{n}")).unwrap();
        assert!(is_risk_control(&read("risk_control.html")));
        for name in [
            "item_on_sale.html",
            "item_not_yet_on_sale.html",
            "item_sold_out.html",
            "item_pc_unsupported.html",
            "item_not_found.html",
            "sku_on_sale_286937790.jsonp",
        ] {
            assert!(!is_risk_control(&read(name)), "{name}");
        }
    }
}
