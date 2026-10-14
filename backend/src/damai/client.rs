use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::DateTime;

use super::Error;
use super::access::{Gate, risk_control_marker};
use super::event::{Event, Session, Tier, sale_state, tier_status};
use super::page::parse_item_page;
use super::sku::{parse_sku_jsonp, session_mismatch, tier_url};

pub const DEFAULT_BASE_URL: &str = "https://detail.damai.cn";

/// damai.cn's home page, the only place the structure inspection takes new
/// samples from: the search endpoint answers its first anonymous request
/// with a slider challenge (0001).
pub const DEFAULT_HOME_URL: &str = "https://www.damai.cn";

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

#[derive(Debug, Clone, Copy)]
pub struct ClockReading {
    pub sent: Instant,
    pub received: Instant,
    /// damai.cn's Date header, ms since the epoch, milliseconds dropped.
    pub date_ms: i64,
}

/// Reads events from damai.cn without signing in. Requests go one at a time
/// and through `gate`, which is the only way out: a query has no reason to
/// burst at damai.cn, and risk control stops every theFlash process.
pub struct Client {
    http: reqwest::Client,
    base: String,
    home: String,
    gate: Arc<Gate>,
}

impl Client {
    /// `base` is DEFAULT_BASE_URL outside tests. Fails when this user's
    /// state directory, which holds the risk-control breaker, cannot be
    /// used: theFlash does not request damai.cn without it.
    pub fn new(base: impl Into<String>) -> Result<Self, Error> {
        Client::with_gate(base, Gate::for_user()?)
    }

    pub fn with_gate(base: impl Into<String>, gate: Arc<Gate>) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            // Redirects are followed by hand in `send`, because reqwest
            // follows them inside one `send()`: the next hop would leave
            // without a slot of its own and without the breaker being
            // checked, and a slider challenge redirected to is only
            // visible in the Location it moved to (#74).
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| setup_error(&e))?;
        Ok(Client {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            home: DEFAULT_HOME_URL.to_string(),
            gate,
        })
    }

    /// Where the home page is. Tests point it at the same local server as
    /// the detail pages, so no test reaches www.damai.cn either.
    pub fn with_home(mut self, home: impl Into<String>) -> Self {
        self.home = home.into().trim_end_matches('/').to_string();
        self
    }

    /// The detail page as damai.cn sent it, for the structure inspection
    /// and for recording a fixture. The event itself is `event`.
    pub async fn item_page(&self, item_id: u64) -> Result<String, Error> {
        self.get(&self.item_page_url(item_id), None).await
    }

    /// One session's tier response as damai.cn sent it.
    pub async fn tier_page(&self, item_id: u64, perform_id: u64) -> Result<String, Error> {
        let url = tier_url(&self.base, item_id, perform_id);
        self.get(&url, Some(&self.item_page_url(item_id))).await
    }

    /// The home page, which is where new samples come from.
    pub async fn home_page(&self) -> Result<String, Error> {
        let url = format!("{}/", self.home);
        self.get(&url, None).await
    }

    pub fn item_page_url(&self, item_id: u64) -> String {
        format!("{}/item.htm?id={item_id}", self.base)
    }

    /// The event and the tiers of each of its sessions. A risk-control page
    /// anywhere stops the whole query; any other failure on one session's
    /// tiers is reported on that session.
    pub async fn event(&self, item_id: u64) -> Result<Event, Error> {
        self.event_where(item_id, |_| true).await
    }

    /// The event with only the sessions `keep` takes, by session name; the
    /// others' tiers aren't requested. `keep` is asked about every session.
    pub async fn event_where(
        &self,
        item_id: u64,
        keep: impl Fn(Option<&str>) -> bool,
    ) -> Result<Event, Error> {
        let page_url = self.item_page_url(item_id);
        let html = self.get(&page_url, None).await?;
        let page = parse_item_page(&html)
            .map_err(|reason| Error::Parse {
                what: "演出详情页",
                reason,
            })?
            .ok_or(Error::NotFound(item_id))?;
        let sale = sale_state(page.buy_button.as_ref());

        let kept: Vec<_> = page
            .performs
            .iter()
            .filter(|p| keep(p.name.as_deref()))
            .collect();
        let mut sessions = Vec::with_capacity(kept.len());
        for perform in kept {
            let url = tier_url(&self.base, item_id, perform.id);
            let tiers = match self.get(&url, Some(&page_url)).await {
                Ok(body) => match parse_sku_jsonp(&body) {
                    Ok(p) => match session_mismatch(&p, perform.id) {
                        Some(why) => Err(why),
                        None => {
                            let state = match &p.buy_button {
                                Some(b) => sale_state(Some(b)),
                                None => sale.clone(),
                            };
                            let project_id = p.project_id.clone();
                            let sign_key = p.sign_key.clone();
                            let has_promotion = p.has_promotion.clone();
                            let rtc = p.rtc;
                            let choose_seat = p.choose_seat;
                            Ok(p.skus.map(|skus| {
                                skus.iter()
                                    .map(|s| Tier {
                                        name: s.name.clone(),
                                        price: s.price.clone(),
                                        status: tier_status(&state, s),
                                        handoff: s.sku_id.as_ref().map(|sku_id| super::Handoff {
                                            sku_id: sku_id.clone(),
                                            perform_id: perform.id,
                                            project_id: project_id.clone(),
                                            sign_key: sign_key.clone(),
                                            has_promotion: has_promotion.clone(),
                                            rtc,
                                            choose_seat,
                                        }),
                                    })
                                    .collect()
                            }))
                        }
                    },
                    Err(reason) => Err(format!("看不懂票档数据：{reason}")),
                },
                // Risk control, or not knowing whether it is in effect,
                // stops the whole query instead of being reported as this
                // one session's failure.
                Err(e) if e.stops_damai_access() => return Err(e),
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

    /// One reading of damai.cn's clock: a HEAD of the event's page, with the
    /// response's Date header (whole seconds) and when, on this machine's
    /// monotonic clock, the request went out and the answer came back.
    pub async fn clock_reading(&self, item_id: u64) -> Result<ClockReading, Error> {
        let url = format!("{}/item.htm?id={item_id}", self.base);
        let Answer {
            resp,
            sent,
            received,
            ..
        } = self.send(reqwest::Method::HEAD, &url, None).await?;
        if !resp.status().is_success() {
            return Err(Error::Http {
                url,
                status: resp.status().as_u16(),
            });
        }
        let date = resp
            .headers()
            .get(reqwest::header::DATE)
            .and_then(|v| v.to_str().ok())
            .ok_or(Error::Parse {
                what: "响应头",
                reason: "没有 Date".into(),
            })?;
        let date_ms = DateTime::parse_from_rfc2822(date)
            .map_err(|e| Error::Parse {
                what: "响应头 Date",
                reason: format!("{date}：{e}"),
            })?
            .timestamp_millis();
        Ok(ClockReading {
            sent,
            received,
            date_ms,
        })
    }

    async fn get(&self, url: &str, referer: Option<&str>) -> Result<String, Error> {
        let answer = self.send(reqwest::Method::GET, url, referer).await?;
        let status = answer.resp.status();
        let from = answer.url;
        let body = answer.resp.text().await.map_err(|e| Error::Network {
            url: from.clone(),
            reason: chain(&e),
        })?;
        if let Some(marker) = risk_control_marker(&body) {
            return Err(self.gate.trip(&from, marker));
        }
        if !status.is_success() {
            return Err(Error::Http {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }
        Ok(body)
    }

    /// One request and the response to it, following damai.cn's redirects
    /// by hand: every hop waits for its own slot and is refused while the
    /// breaker is in effect, which is what reqwest's own following would
    /// skip. A Location carrying a risk marker is the slider challenge
    /// itself, so it records the breaker instead of being followed — on a
    /// HEAD, and on a GET whose challenge page says nothing in its body,
    /// that redirect is the only place the challenge can be seen (#74).
    /// A redirect hop whose body already carries a risk marker is stopped
    /// there too, before following an ordinary-looking Location (#109).
    async fn send(
        &self,
        method: reqwest::Method,
        url: &str,
        referer: Option<&str>,
    ) -> Result<Answer, Error> {
        let mut at = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            let mut req = self.http.request(method.clone(), at.as_str());
            if let Some(r) = referer {
                req = req.header(reqwest::header::REFERER, r);
            }
            self.gate.enter().await?;
            let sent = Instant::now();
            let resp = req.send().await.map_err(|e| Error::Network {
                url: at.clone(),
                reason: chain(&e),
            })?;
            let received = Instant::now();
            let Some(location) = redirect_target(&resp) else {
                return Ok(Answer {
                    resp,
                    url: at,
                    sent,
                    received,
                });
            };
            if let Some(marker) = risk_control_marker(&location) {
                return Err(self.gate.trip(&at, marker));
            }
            let body = resp.text().await.map_err(|e| Error::Network {
                url: at.clone(),
                reason: chain(&e),
            })?;
            if let Some(marker) = risk_control_marker(&body) {
                return Err(self.gate.trip(&at, marker));
            }
            at = location;
        }
        Err(Error::Network {
            url: url.to_string(),
            reason: format!("重定向超过 {MAX_REDIRECTS} 次，没有继续跟随"),
        })
    }
}

/// How many redirects one request may follow before theFlash stops. Each
/// hop costs a slot, and damai.cn's own pages take at most one.
const MAX_REDIRECTS: usize = 5;

/// What one hop answered.
struct Answer {
    resp: reqwest::Response,
    /// The URL this response came from, which is the last hop followed.
    url: String,
    sent: Instant,
    received: Instant,
}

/// Where this response says to go next, as an absolute URL, or None when
/// it is the answer itself. Only the HTTP redirect status codes that
/// reqwest's policy follows (301, 302, 303, 307, 308) count as redirects;
/// other 3xx codes like 304 Not Modified must not follow a Location
/// header (#111). A Location that is not a URL at all is returned as it
/// came, so a risk marker in it is still seen.
fn redirect_target(resp: &reqwest::Response) -> Option<String> {
    if !matches!(resp.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)?
        .to_str()
        .ok()?;
    Some(
        resp.url()
            .join(location)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| location.to_string()),
    )
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

/// rustls-platform-verifier's words when Linux has no CA certificate (no
/// ca-certificates package, as in a minimal container image), under
/// reqwest's "builder error". tests/cli.rs runs into it for real on Linux,
/// so a dependency update that rewords it fails there.
const NO_CA_CERTIFICATES: &str = "No CA certificates were loaded from the system";

/// Building the client fails before any request goes out, so it is never
/// a network failure, and there is no URL to name.
fn setup_error(e: &dyn std::error::Error) -> Error {
    let reason = chain(e);
    if reason.contains(NO_CA_CERTIFICATES) {
        Error::NoCaCertificates
    } else {
        Error::ClientSetup(reason)
    }
}

/// The item id in a detail page link (`id=` on detail.damai.cn, `itemId=`
/// on m.damai.cn), or a bare id.
pub fn parse_item_id(input: &str) -> Result<u64, Error> {
    let s = input.trim();
    let invalid = || Error::InvalidInput(input.to_string());
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse().map_err(|_| invalid());
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) || s.contains('\\') {
        return Err(invalid());
    }
    // URL parsers repair special-scheme inputs like https:///host; refuse
    // malformed authority text before normalization hides it.
    let authority = s
        .split_once("://")
        .and_then(|(_, rest)| rest.split(['/', '?', '#']).next())
        .ok_or_else(invalid)?;
    if authority.is_empty() || authority.contains('@') {
        return Err(invalid());
    }
    let url = reqwest::Url::parse(s).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || !s
            .to_ascii_lowercase()
            .starts_with(&format!("{}://", url.scheme()))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(invalid());
    }
    let id_key = match (url.host_str(), url.path()) {
        (Some("detail.damai.cn"), "/item.htm") => "id",
        (Some("m.damai.cn"), "/damai/detail/item.html") => "itemId",
        _ => return Err(invalid()),
    };
    let mut ids = url.query_pairs().filter(|(key, _)| key == id_key);
    let (_, value) = ids.next().ok_or_else(invalid)?;
    if ids.next().is_some() || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    value.parse().map_err(|_| invalid())
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

    /// An error and the one that caused it, the way reqwest nests them.
    #[derive(Debug)]
    struct Caused(&'static str, Option<Box<Caused>>);

    impl std::fmt::Display for Caused {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for Caused {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_deref().map(|c| c as _)
        }
    }

    // What building the client returned in ubuntu:24.04 without
    // ca-certificates (#37), cause by cause. tests/cli.rs runs into the real
    // one on Linux; this keeps the mapping checked on macOS as well.
    #[test]
    fn missing_ca_certificates_are_told_apart_from_other_setup_failures() {
        let no_ca = Caused(
            "builder error",
            Some(Box::new(Caused(
                "unexpected error: No CA certificates were loaded from the system",
                None,
            ))),
        );
        let e = setup_error(&no_ca);
        assert!(matches!(e, Error::NoCaCertificates), "{e:?}");
        let said = e.to_string();
        assert!(said.contains("没有 CA 证书"), "{said}");
        assert!(
            said.contains("apt-get install -y ca-certificates"),
            "{said}"
        );

        let other = Caused(
            "builder error",
            Some(Box::new(Caused("invalid TLS versions", None))),
        );
        assert_eq!(
            setup_error(&other).to_string(),
            "没法创建发请求用的 HTTP 客户端，所以没有发出任何请求：builder error：invalid TLS versions"
        );
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
}
