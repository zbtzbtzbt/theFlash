use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::DateTime;

use super::Error;
use super::event::{Event, Session, Tier, sale_state, tier_status};
use super::page::parse_item_page;
use super::sku::{parse_sku_jsonp, session_mismatch, tier_url};

pub const DEFAULT_BASE_URL: &str = "https://detail.damai.cn";

/// damai.cn's home page, the only place the structure inspection takes new
/// samples from: the search endpoint answers its first anonymous request
/// with a slider challenge (0001).
pub const DEFAULT_HOME_URL: &str = "https://www.damai.cn";

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// Between two tier requests. A calendar event has a session a day (18 on
/// 1084537866279), and damai.cn's search answered its first request with a
/// slider challenge, so a query paces itself.
const TIER_REQUEST_GAP: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy)]
pub struct ClockReading {
    pub sent: Instant,
    pub received: Instant,
    /// damai.cn's Date header, ms since the epoch, milliseconds dropped.
    pub date_ms: i64,
}

/// Reads events from damai.cn without signing in. Requests go one at a time:
/// a query has no reason to burst at damai.cn.
pub struct Client {
    http: reqwest::Client,
    base: String,
    request_gap: Duration,
    next_request: Mutex<Option<Instant>>,
    home: String,
}

impl Client {
    /// `base` is DEFAULT_BASE_URL outside tests.
    pub fn new(base: impl Into<String>) -> Result<Self, Error> {
        // Opt-in conservative pacing for unattended validation (#17).
        // The default and cross-process policy remain owned by #4.
        let request_gap = match std::env::var("THEFLASH_REQUEST_GAP_MS") {
            Ok(value) => validation_gap(&value)?,
            Err(std::env::VarError::NotPresent) => Duration::ZERO,
            Err(_) => return Err(Error::InvalidInput("THEFLASH_REQUEST_GAP_MS 无效".into())),
        };
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| setup_error(&e))?;
        Ok(Client {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            request_gap,
            next_request: Mutex::new(None),
            home: DEFAULT_HOME_URL.to_string(),
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
        for (i, perform) in kept.into_iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(TIER_REQUEST_GAP).await;
            }
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
                            Ok(p.skus.map(|skus| {
                                skus.iter()
                                    .map(|s| Tier {
                                        name: s.name.clone(),
                                        price: s.price.clone(),
                                        status: tier_status(&state, s),
                                    })
                                    .collect()
                            }))
                        }
                    },
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

    /// One reading of damai.cn's clock: a HEAD of the event's page, with the
    /// response's Date header (whole seconds) and when, on this machine's
    /// monotonic clock, the request went out and the answer came back.
    pub async fn clock_reading(&self, item_id: u64) -> Result<ClockReading, Error> {
        let url = format!("{}/item.htm?id={item_id}", self.base);
        let network = |e: reqwest::Error| Error::Network {
            url: url.clone(),
            reason: chain(&e),
        };
        self.pace().await;
        let sent = Instant::now();
        let resp = self.http.head(&url).send().await.map_err(network)?;
        let received = Instant::now();
        if is_risk_control(resp.url().as_str()) {
            return Err(Error::RiskControl(url));
        }
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
        self.pace().await;
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

    async fn pace(&self) {
        if self.request_gap.is_zero() {
            return;
        }
        let delay = reserve_request(
            &mut self.next_request.lock().unwrap_or_else(|p| p.into_inner()),
            Instant::now(),
            self.request_gap,
        );
        tokio::time::sleep(delay).await;
    }
}

fn reserve_request(next: &mut Option<Instant>, now: Instant, gap: Duration) -> Duration {
    let target = next.unwrap_or(now).max(now);
    *next = target.checked_add(gap);
    target.duration_since(now)
}

fn validation_gap(value: &str) -> Result<Duration, Error> {
    // Reject unrepresentable/accidentally enormous deadlines instead of
    // overflowing the reservation and silently losing the requested pacing.
    value
        .parse::<u64>()
        .ok()
        .filter(|ms| *ms <= 86_400_000)
        .map(Duration::from_millis)
        .ok_or_else(|| {
            Error::InvalidInput("THEFLASH_REQUEST_GAP_MS 必须是 0 到 86400000 毫秒".into())
        })
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
    fn validation_requests_reserve_two_second_slots_with_injected_time() {
        let now = Instant::now();
        let gap = Duration::from_secs(2);
        let mut next = None;
        assert_eq!(reserve_request(&mut next, now, gap), Duration::ZERO);
        assert_eq!(reserve_request(&mut next, now, gap), gap);
        assert_eq!(reserve_request(&mut next, now, gap), gap * 2);
        assert_eq!(
            reserve_request(&mut next, now + gap * 10, gap),
            Duration::ZERO
        );
    }

    #[test]
    fn validation_gap_rejects_invalid_and_overflowing_deadlines() {
        assert_eq!(validation_gap("0").unwrap(), Duration::ZERO);
        assert_eq!(validation_gap("2000").unwrap(), Duration::from_secs(2));
        assert_eq!(
            validation_gap("86400000").unwrap(),
            Duration::from_secs(86400)
        );
        for value in ["-1", "bad", "86400001", "18446744073709551615"] {
            assert!(validation_gap(value).is_err());
        }
    }

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
