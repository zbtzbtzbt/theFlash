//! The watch loop on virtual time: Fake answers from a script and moves its
//! clock only when the loop sleeps or waits on a request.

use super::clock::Sample;
use super::*;
use crate::damai::{Error, Event, Session, Tier};

/// 1092337348536's sale: 2026-10-14 10:18 Beijing.
const SALE: i64 = 1_791_944_280_000;
const RTT: i64 = 40;
/// A watch that should have stopped and didn't fails here instead of
/// spinning on virtual time forever.
const MAX_QUERIES: usize = 1_000;
const MAX_VIRTUAL_MS: i64 = 30 * 24 * 3_600_000;

type Script = Box<dyn FnMut(usize, i64) -> Result<Event, Error>>;

struct Fake {
    now: i64,
    /// damai.cn's time = now + damai_offset.
    damai_offset: i64,
    /// This machine's wall clock = now + wall_offset.
    wall_offset: i64,
    /// Answers query n (from 1) at damai.cn time t.
    script: Script,
    calls: usize,
    clock_works: bool,
    stop_after_calls: usize,
    stop_after_alerts: usize,
    /// The machine sleeps once, when the monotonic clock passes the first
    /// value, for the second: damai.cn's clock and the wall clock move on,
    /// the monotonic clock doesn't (macOS's stops while asleep).
    machine_sleep: Option<(i64, i64)>,
    /// (damai.cn time, what happened)
    log: Vec<(i64, String)>,
    sleeps: Vec<i64>,
}

impl Fake {
    /// damai.cn's clock reads `damai_at_start` when the watch starts, and
    /// this machine's is `wall_ahead` ms ahead of it.
    fn new(damai_at_start: i64, wall_ahead: i64, script: Script) -> Self {
        Fake {
            now: 0,
            damai_offset: damai_at_start,
            wall_offset: damai_at_start + wall_ahead,
            script,
            calls: 0,
            clock_works: true,
            stop_after_calls: usize::MAX,
            stop_after_alerts: usize::MAX,
            machine_sleep: None,
            log: Vec::new(),
            sleeps: Vec::new(),
        }
    }

    fn damai(&self) -> i64 {
        self.now + self.damai_offset
    }

    fn alerts(&self) -> Vec<(i64, String)> {
        self.log
            .iter()
            .filter(|(_, l)| l.starts_with("alert "))
            .cloned()
            .collect()
    }

    fn said(&self, part: &str) -> bool {
        self.log
            .iter()
            .any(|(_, l)| l.starts_with("say ") && l.contains(part))
    }
}

impl Env for Fake {
    fn now(&self) -> i64 {
        self.now
    }

    fn wall(&self) -> i64 {
        self.now + self.wall_offset
    }

    async fn sleep_until(&mut self, mono: i64) {
        assert!(
            mono < MAX_VIRTUAL_MS,
            "the watch was still running after 30 virtual days"
        );
        if mono > self.now {
            self.sleeps.push(mono - self.now);
            if let Some((at, slept)) = self.machine_sleep
                && self.now < at
                && mono >= at
            {
                self.damai_offset += slept;
                self.wall_offset += slept;
                self.machine_sleep = None;
            }
            self.now = mono;
        }
    }

    async fn event(&mut self) -> Result<Event, Error> {
        assert!(
            self.calls < MAX_QUERIES,
            "the watch never stopped: {} queries",
            self.calls
        );
        self.calls += 1;
        self.now += RTT;
        let t = self.damai();
        (self.script)(self.calls, t)
    }

    fn all_sessions(&self) -> Vec<String> {
        vec!["2026-10-15 周四 17:00".into()]
    }

    async fn clock_samples(&mut self) -> Result<Vec<Sample>, Error> {
        if !self.clock_works {
            return Err(Error::Parse {
                what: "响应头",
                reason: "没有 Date".into(),
            });
        }
        let mut out = Vec::new();
        for _ in 0..5 {
            let sent = self.now;
            let stamped = sent + RTT / 2 + self.damai_offset;
            self.now += RTT;
            out.push(Sample {
                sent,
                received: self.now,
                date_ms: stamped - stamped.rem_euclid(1000),
            });
            self.now += 250;
        }
        Ok(out)
    }

    fn alert(&mut self, title: &str, lines: &[String]) {
        let t = self.damai();
        self.log
            .push((t, format!("alert {title}: {}", lines.join(" | "))));
    }

    fn say(&mut self, line: &str) {
        let t = self.damai();
        self.log.push((t, format!("say {line}")));
    }

    fn countdown(&mut self, line: &str, _left_ms: i64) {
        let t = self.damai();
        self.log.push((t, format!("countdown {line}")));
    }

    fn stopped(&self) -> bool {
        self.calls >= self.stop_after_calls || self.alerts().len() >= self.stop_after_alerts
    }
}

fn event(sale: SaleState, sell_start: Option<i64>, tier: TierStatus) -> Event {
    Event {
        item_id: 1092337348536,
        name: Some("测试演出".into()),
        city: None,
        venue: None,
        sale,
        sell_start_ms: sell_start,
        sessions: vec![Session {
            perform_id: 286966465,
            name: Some("2026-10-15 周四 17:00".into()),
            tiers: Ok(Some(vec![Tier {
                name: Some("180元".into()),
                price: Some("180.00".into()),
                status: tier,
            }])),
        }],
    }
}

fn before_sale() -> Event {
    event(
        SaleState::NotYetOnSale,
        Some(SALE),
        TierStatus::NotYetOnSale,
    )
}

fn on_sale(tier: TierStatus) -> Event {
    event(SaleState::OnSale, Some(SALE), tier)
}

fn opts(filter: Filter) -> Options {
    Options {
        item_id: 1092337348536,
        filter,
        interval: DEFAULT_INTERVAL,
        system_notify: false,
    }
}

fn network() -> Error {
    Error::Network {
        url: "https://detail.damai.cn/item.htm?id=1".into(),
        reason: "Connection refused".into(),
    }
}

/// Opens on damai.cn's clock at `opens`.
fn opening_at(opens: i64) -> Script {
    Box::new(move |_, t| {
        Ok(if t < opens {
            before_sale()
        } else {
            on_sale(TierStatus::Available)
        })
    })
}

#[tokio::test]
async fn the_sale_alert_fires_on_damai_time_not_this_machines() {
    // This machine runs 3 s fast; by its clock the sale is 3 s earlier.
    let mut f = Fake::new(SALE - 300_000, 3_000, opening_at(SALE));
    f.stop_after_alerts = 2;
    run(&mut f, &opts(Filter::default())).await.unwrap();
    let a = f.alerts();
    assert!(a[0].1.starts_with("alert 开售时间到"), "{a:?}");
    assert!(
        (a[0].0 - SALE).abs() <= 1000,
        "fired {} ms from the sale",
        a[0].0 - SALE
    );
    assert!(a[1].1.starts_with("alert 已经可以买了"), "{a:?}");
    assert!(a[1].1.contains("180元 · ¥180.00 · 可售"), "{a:?}");
    assert!(f.said("大麦时间比本机慢 "), "{:#?}", f.log);
}

#[tokio::test]
async fn confirming_keeps_asking_until_tickets_can_be_bought() {
    let mut f = Fake::new(SALE - 120_000, 0, opening_at(SALE + 2_500));
    f.stop_after_alerts = 2;
    run(&mut f, &opts(Filter::default())).await.unwrap();
    let a = f.alerts();
    assert!(a[1].1.starts_with("alert 已经可以买了"), "{a:?}");
    assert!(
        (SALE + 2_500..=SALE + 4_000).contains(&a[1].0),
        "confirmed {} ms after the sale",
        a[1].0 - SALE
    );
    assert!(f.calls <= 6, "{} queries", f.calls);
}

#[tokio::test]
async fn a_sale_that_does_not_open_is_reported_after_a_minute() {
    let mut f = Fake::new(SALE - 60_000, 0, Box::new(|_, _| Ok(before_sale())));
    f.stop_after_alerts = 2;
    run(&mut f, &opts(Filter::default())).await.unwrap();
    let a = f.alerts();
    assert!(
        a[1].1.starts_with("alert 开售时间已过 1 分钟，还不能买"),
        "{a:?}"
    );
    assert!(
        (SALE + 55_000..=SALE + 61_000).contains(&a[1].0),
        "reported {} ms after the sale",
        a[1].0 - SALE
    );
    assert!(f.calls <= 1 + 30, "{} queries", f.calls);
}

#[tokio::test]
async fn a_restock_alerts_once_each_time_the_tier_comes_back() {
    use TierStatus::*;
    let states = [
        OutOfStock, OutOfStock, Available, Available, OutOfStock, Available,
    ];
    let rounds = states.len();
    let mut f = Fake::new(
        SALE + 3_600_000,
        0,
        Box::new(move |n, _| Ok(on_sale(states[n - 1].clone()))),
    );
    f.stop_after_calls = rounds;
    run(&mut f, &opts(Filter::default())).await.unwrap();
    let titles: Vec<String> = f.alerts().into_iter().map(|(_, l)| l).collect();
    assert_eq!(titles.len(), 2, "{titles:?}");
    assert!(
        titles.iter().all(|t| t.starts_with("alert 有票了")),
        "{titles:?}"
    );
    assert!(f.said("又没票了：2026-10-15 周四 17:00 · 180元 · ¥180.00 · 缺货"));
    assert_eq!(f.sleeps, [30_000; 5]);
}

#[tokio::test]
async fn network_failures_back_off_and_the_watch_carries_on() {
    let mut f = Fake::new(
        SALE + 3_600_000,
        0,
        Box::new(|n, _| match n {
            1 => Ok(on_sale(TierStatus::OutOfStock)),
            2 | 3 => Err(network()),
            _ => Ok(on_sale(TierStatus::Available)),
        }),
    );
    f.stop_after_calls = 4;
    run(&mut f, &opts(Filter::default())).await.unwrap();
    assert_eq!(f.sleeps, [30_000, 60_000, 120_000]);
    assert!(f.said("60 秒后重试"), "{:#?}", f.log);
    assert!(f.said("120 秒后重试"), "{:#?}", f.log);
    let a = f.alerts();
    assert_eq!(a.len(), 1, "{a:?}");
    assert!(a[0].1.starts_with("alert 有票了"), "{a:?}");
}

// damai.cn leaving the tier list out is unknown, not "no tickets" (#1's
// acceptance): the watch says so and neither alerts nor forgets the tier.
#[tokio::test]
async fn a_tier_list_damai_leaves_out_is_reported_as_unknown() {
    let mut f = Fake::new(
        SALE + 3_600_000,
        0,
        Box::new(|n, _| {
            let mut e = on_sale(TierStatus::OutOfStock);
            match n {
                2 => e.sessions[0].tiers = Ok(None),
                3 => e = on_sale(TierStatus::Available),
                _ => {}
            }
            Ok(e)
        }),
    );
    f.stop_after_calls = 3;
    run(&mut f, &opts(Filter::default())).await.unwrap();
    assert!(
        f.said("2026-10-15 周四 17:00 的票档这次未知（大麦没有提供票档列表）"),
        "{:#?}",
        f.log
    );
    let a = f.alerts();
    assert_eq!(a.len(), 1, "{a:?}");
    assert!(a[0].1.starts_with("alert 有票了"), "{a:?}");
}

#[tokio::test]
async fn a_risk_control_page_alerts_and_ends_the_watch() {
    let mut f = Fake::new(
        SALE + 3_600_000,
        0,
        Box::new(|n, _| match n {
            1 => Ok(on_sale(TierStatus::OutOfStock)),
            _ => Err(Error::RiskControl(
                "https://detail.damai.cn/item.htm?id=1".into(),
            )),
        }),
    );
    let r = run(&mut f, &opts(Filter::default())).await;
    assert!(
        matches!(r, Err(WatchError::Damai(Error::RiskControl(_)))),
        "{r:?}"
    );
    assert_eq!(f.calls, 2);
    let a = f.alerts();
    assert!(
        a.last()
            .unwrap()
            .1
            .starts_with("alert 触发风控，已停止盯票"),
        "{a:?}"
    );
}

#[tokio::test]
async fn a_tier_already_on_sale_alerts_at_the_start() {
    let mut f = Fake::new(
        SALE + 3_600_000,
        0,
        Box::new(|_, _| Ok(on_sale(TierStatus::Available))),
    );
    f.stop_after_calls = 1;
    run(&mut f, &opts(Filter::default())).await.unwrap();
    let a = f.alerts();
    assert_eq!(a.len(), 1, "{a:?}");
    assert!(a[0].1.starts_with("alert 现在就可以买"), "{a:?}");
}

#[tokio::test]
async fn a_filter_matching_nothing_names_what_there_is() {
    let mut f = Fake::new(
        SALE + 3_600_000,
        0,
        Box::new(|_, _| Ok(on_sale(TierStatus::Available))),
    );
    let r = run(
        &mut f,
        &opts(Filter {
            session: Some("12-25".into()),
            tier: None,
        }),
    )
    .await;
    let Err(WatchError::NothingToWatch(why)) = r else {
        panic!("{r:?}");
    };
    assert!(why.contains("没有匹配“12-25”的场次"), "{why}");
    assert!(why.contains("2026-10-15 周四 17:00"), "{why}");

    let r = run(
        &mut f,
        &opts(Filter {
            session: None,
            tier: Some("999".into()),
        }),
    )
    .await;
    let Err(WatchError::NothingToWatch(why)) = r else {
        panic!("{r:?}");
    };
    assert!(why.contains("没有匹配“999”的票档"), "{why}");
    assert!(why.contains("180元"), "{why}");
}

#[tokio::test]
async fn without_damai_time_the_countdown_runs_on_this_machines_clock_and_says_so() {
    let mut f = Fake::new(SALE - 30_000, 0, opening_at(SALE));
    f.clock_works = false;
    f.stop_after_alerts = 2;
    run(&mut f, &opts(Filter::default())).await.unwrap();
    assert!(f.said("按本机时钟倒计时"), "{:#?}", f.log);
    let a = f.alerts();
    assert!(a[0].1.starts_with("alert 开售时间到"), "{a:?}");
    assert!((a[0].0 - SALE).abs() <= 1000);
}

// A laptop closed overnight: two hours before the sale the watch starts,
// and half an hour in the machine sleeps for an hour. On the monotonic
// clock alone the alert would come an hour late.
#[tokio::test]
async fn after_the_machine_sleeps_the_sale_alert_still_comes_on_time() {
    let mut f = Fake::new(SALE - 2 * 3_600_000, 0, opening_at(SALE));
    f.machine_sleep = Some((30 * 60_000, 3_600_000));
    f.stop_after_alerts = 2;
    run(&mut f, &opts(Filter::default())).await.unwrap();
    assert!(
        f.said("本机时间跳了"),
        "{:#?}",
        f.log
            .iter()
            .filter(|(_, l)| l.starts_with("say"))
            .collect::<Vec<_>>()
    );
    let a = f.alerts();
    assert!(a[0].1.starts_with("alert 开售时间到"), "{a:?}");
    assert!(
        (a[0].0 - SALE).abs() <= 1000,
        "fired {} ms from the sale",
        a[0].0 - SALE
    );
}

#[tokio::test]
async fn a_moved_sale_time_is_followed() {
    let moved = SALE + 60_000;
    let mut f = Fake::new(
        SALE - 15 * 60_000,
        0,
        Box::new(move |n, t| {
            Ok(if n == 1 {
                before_sale()
            } else if t < moved {
                event(
                    SaleState::NotYetOnSale,
                    Some(moved),
                    TierStatus::NotYetOnSale,
                )
            } else {
                on_sale(TierStatus::Available)
            })
        }),
    );
    f.stop_after_alerts = 2;
    run(&mut f, &opts(Filter::default())).await.unwrap();
    assert!(f.said("开售时间改了：2026-10-14 10:19:00"), "{:#?}", f.log);
    let a = f.alerts();
    assert!(
        (a[0].0 - moved).abs() <= 1000,
        "fired {} ms from the new time",
        a[0].0 - moved
    );
}

#[test]
fn backoff_doubles_to_five_minutes_and_never_drops_below_the_interval() {
    assert_eq!(backoff(30_000, 0), 30_000);
    assert_eq!(backoff(30_000, 1), 60_000);
    assert_eq!(backoff(30_000, 3), 240_000);
    assert_eq!(backoff(30_000, 9), 300_000);
    assert_eq!(backoff(600_000, 2), 600_000);
}

#[test]
fn time_left_reads_in_hours_minutes_and_seconds() {
    assert_eq!(left_text(7_200_000), "2 小时 00 分 00 秒");
    assert_eq!(left_text(7_199_001), "2 小时 00 分 00 秒");
    assert_eq!(left_text(65_000), "1 分 05 秒");
    assert_eq!(left_text(1), "1 秒");
}
