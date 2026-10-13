//! `theflash watch`: an alert the moment a sale opens, by damai.cn's clock,
//! and whenever a watched tier comes back. Why it works this way:
//! docs/decisions/0002-watch-timing-and-polling.md.

mod clock;
mod env;
mod notify;
mod target;

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};

use crate::damai::{self, Client, SaleState, TierStatus};
use crate::report::{sale_label, tier_label};
use clock::{ServerClock, estimate, from_last};
use env::{Env, RealEnv};
pub use target::Filter;
use target::{Watched, no_match, watched};

pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(30);
pub const MIN_INTERVAL: Duration = Duration::from_secs(10);
/// The longest wait between queries while damai.cn can't be reached.
const MAX_BACKOFF_MS: i64 = 300_000;
/// Before a sale, how often the page is read again for a moved sale time.
const RECHECK_BEFORE_SALE_MS: i64 = 600_000;
/// The clock is read again this long before the sale. Close enough that
/// the connection those reads open is still alive at the sale, so the
/// first query after it needn't wait on a new TLS handshake.
const RESYNC_BEFORE_SALE_MS: i64 = 20_000;
/// After the sale time, how long the watch tries to see tickets on sale.
const CONFIRM_FOR_MS: i64 = 60_000;
/// The wall and monotonic clocks parting by more than this means the
/// machine slept or its clock was set.
const CLOCK_JUMP_MS: i64 = 2_000;

pub struct Options {
    pub item_id: u64,
    pub filter: Filter,
    pub interval: Duration,
    pub system_notify: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error(transparent)]
    Damai(#[from] damai::Error),
    /// No session or tier matches, or the PC site shows none.
    #[error("{0}")]
    NothingToWatch(String),
}

/// Watches until a risk-control page, a vanished event, or the caller
/// dropping the future (Ctrl-C).
pub async fn watch(client: &Client, opts: &Options) -> Result<(), WatchError> {
    let mut env = RealEnv::new(
        client,
        opts.item_id,
        opts.filter.session.clone(),
        opts.system_notify,
    );
    run(&mut env, opts).await
}

type Last = HashMap<(u64, String), TierStatus>;

pub(crate) async fn run<E: Env>(env: &mut E, opts: &Options) -> Result<(), WatchError> {
    let event = match env.event().await {
        Ok(e) => e,
        Err(e) => return Err(stop(env, e)),
    };
    let name = event
        .name
        .clone()
        .unwrap_or_else(|| format!("演出 {}", event.item_id));
    if event.sale == SaleState::NotOnPcChannel {
        return Err(WatchError::NothingToWatch(
            "大麦电脑版不展示这场演出的场次和票档，没法盯".into(),
        ));
    }
    let now = watched(&event, &opts.filter);
    if now.is_empty() {
        return Err(WatchError::NothingToWatch(no_match(
            &event,
            &env.all_sessions(),
            &opts.filter,
        )));
    }

    env.say(&format!("盯住：{name}（{}）", sale_label(&event.sale)));
    if event.sale == SaleState::NotYetOnSale {
        let when = event
            .sell_start_ms
            .map(|t| beijing(t, false))
            .unwrap_or_else(|| "未知".into());
        env.say(&format!("开售时间：{when}（北京时间）"));
    }
    for w in &now {
        env.say(&format!(
            "  {} · {}（现在：{}）",
            w.session,
            w.tier,
            tier_label(&w.status)
        ));
    }
    let interval_s = opts.interval.as_secs().max(1);
    let per_round = 1 + event.sessions.len() as u64;
    env.say(&format!(
        "查询节奏：每 {interval_s} 秒查一次，每次 {per_round} 个请求（约 {} 个/小时）",
        per_round * 3600 / interval_s
    ));

    let mut last: Last = HashMap::new();
    remember(&mut last, &now);
    match (&event.sale, event.sell_start_ms) {
        (SaleState::NotYetOnSale, Some(sale_at)) => {
            env.say("开售前每 10 分钟查一次页面，开售那一刻按大麦时间提醒");
            sale_open(env, opts, &name, &now, sale_at, &mut last).await?;
        }
        (SaleState::NotYetOnSale, None) => env.say("页面没给开售时间，只能按间隔查询"),
        _ => {
            let available: Vec<&Watched> = now
                .iter()
                .filter(|w| w.status == TierStatus::Available)
                .collect();
            if !available.is_empty() {
                let at = time_line(env, None);
                env.alert(
                    "现在就可以买",
                    &with(&name, available.iter().map(|w| w.line()), at),
                );
            }
        }
    }
    restock(env, opts, &name, &mut last).await
}

/// Counts down on damai.cn's clock, alerts at the sale time, then tries
/// for a minute to see the watched tiers on sale.
async fn sale_open<E: Env>(
    env: &mut E,
    opts: &Options,
    name: &str,
    targets: &[Watched],
    mut sale_at: i64,
    last: &mut Last,
) -> Result<(), WatchError> {
    let mut clock = sync(env).await?;
    let mut resynced = false;
    let mut next_check = env.now() + RECHECK_BEFORE_SALE_MS;
    let mut seen = (env.now(), env.wall());
    loop {
        let now = env.now();
        // macOS's monotonic clock stops while the machine sleeps, so after
        // a sleep it lags damai.cn's by however long that was. The wall
        // clock keeps going; when the two part, read damai.cn's again.
        let jump = (env.wall() - seen.1) - (now - seen.0);
        if jump.abs() > CLOCK_JUMP_MS {
            env.say(&format!(
                "本机时间跳了 {} 秒（电脑睡眠过或时钟被调整），重新读大麦时间",
                jump / 1000
            ));
            clock = sync(env).await?;
            seen = (env.now(), env.wall());
            continue;
        }
        seen = (now, env.wall());
        let left = clock.mono_at(sale_at) - now;
        if left <= 0 {
            break;
        }
        if !resynced && left <= RESYNC_BEFORE_SALE_MS {
            resynced = true;
            clock = sync(env).await?;
            continue;
        }
        if now >= next_check && left > 3 * 60_000 {
            next_check = now + RECHECK_BEFORE_SALE_MS;
            match env.event().await {
                Ok(ev) => {
                    if let Some(t) = ev.sell_start_ms
                        && t != sale_at
                    {
                        env.say(&format!("开售时间改了：{}（北京时间）", beijing(t, false)));
                        sale_at = t;
                    }
                }
                Err(e) if fatal(&e) => return Err(stop(env, e)),
                Err(e) => env.say(&format!("查询失败（{e}），继续倒计时")),
            }
            continue;
        }
        let whose = if clock.error >= 0 {
            "大麦时间"
        } else {
            "本机时间"
        };
        env.countdown(
            &format!(
                "距离开售 {}（{whose} {}）",
                left_text(left),
                beijing(clock.damai_at(now), false)
            ),
            left,
        );
        let step = match left % 1000 {
            0 => left.min(1000),
            r => r,
        };
        env.sleep_until(now + step).await;
    }

    let fired = env.now();
    let mut lines = vec![name.to_string()];
    lines.extend(
        targets
            .iter()
            .map(|w| format!("{} · {}", w.session, w.tier)),
    );
    lines.push(format!(
        "{}，正在确认能不能买",
        time_line(env, Some(&clock))
    ));
    env.alert("开售时间到", &lines);

    let mut attempts = 0u32;
    loop {
        let started = env.now();
        match env.event().await {
            Ok(ev) => {
                let w = watched(&ev, &opts.filter);
                let available: Vec<&Watched> = w
                    .iter()
                    .filter(|x| x.status == TierStatus::Available)
                    .collect();
                if ev.sale == SaleState::OnSale || !available.is_empty() {
                    remember(last, &w);
                    let at = time_line(env, Some(&clock));
                    if available.is_empty() {
                        env.alert(
                            "已开售，但盯的票档现在没有票",
                            &with(name, w.iter().map(Watched::line), at),
                        );
                    } else {
                        env.alert(
                            "已经可以买了",
                            &with(name, available.iter().map(|x| x.line()), at),
                        );
                    }
                    return Ok(());
                }
            }
            Err(e) if fatal(&e) => return Err(stop(env, e)),
            Err(e) => env.say(&format!("确认失败（{e}）")),
        }
        attempts += 1;
        let gap = if attempts < 10 { 1000 } else { 3000 };
        if started + gap - fired > CONFIRM_FOR_MS {
            let at = time_line(env, Some(&clock));
            env.alert(
                "开售时间已过 1 分钟，还不能买",
                &[name.to_string(), "之后按间隔继续盯".into(), at],
            );
            return Ok(());
        }
        env.sleep_until(started + gap).await;
    }
}

/// Queries every interval and alerts when a watched tier turns 可售. A
/// failure that may pass (network, HTTP, an unreadable page) waits twice as
/// long each time, up to MAX_BACKOFF_MS, and the watch carries on.
async fn restock<E: Env>(
    env: &mut E,
    opts: &Options,
    name: &str,
    last: &mut Last,
) -> Result<(), WatchError> {
    let interval = opts.interval.as_millis() as i64;
    let mut failures = 0u32;
    loop {
        if env.stopped() {
            return Ok(());
        }
        let wait = backoff(interval, failures);
        env.sleep_until(env.now() + wait).await;
        match env.event().await {
            Ok(ev) => {
                failures = 0;
                for s in &ev.sessions {
                    let session = s.name.as_deref().unwrap_or("未知场次");
                    match &s.tiers {
                        Err(why) => env.say(&format!("{session} 的票档这次没查到：{why}")),
                        Ok(None) => {
                            env.say(&format!("{session} 的票档这次未知（大麦没有提供票档列表）"))
                        }
                        Ok(Some(_)) => {}
                    }
                }
                let w = watched(&ev, &opts.filter);
                let was_available =
                    |x: &Watched| last.get(&x.key()) == Some(&TierStatus::Available);
                let back: Vec<&Watched> = w
                    .iter()
                    .filter(|x| x.status == TierStatus::Available && !was_available(x))
                    .collect();
                let gone: Vec<&Watched> = w
                    .iter()
                    .filter(|x| x.status != TierStatus::Available && was_available(x))
                    .collect();
                for x in &gone {
                    env.say(&format!("又没票了：{}", x.line()));
                }
                if !back.is_empty() {
                    let at = time_line(env, None);
                    env.alert("有票了", &with(name, back.iter().map(|x| x.line()), at));
                }
                remember(last, &w);
            }
            Err(e) if fatal(&e) => return Err(stop(env, e)),
            Err(e) => {
                failures += 1;
                env.say(&format!(
                    "查询失败（{e}），{} 秒后重试",
                    backoff(interval, failures) / 1000
                ));
            }
        }
    }
}

/// Reads damai.cn's clock. Without a usable Date header the countdown runs
/// on this machine's clock, and says so.
async fn sync<E: Env>(env: &mut E) -> Result<ServerClock, WatchError> {
    let local = ServerClock {
        offset: env.wall() - env.now(),
        error: -1,
    };
    match env.clock_samples().await {
        Ok(samples) => {
            let (c, note) = match estimate(&samples) {
                Some(c) => (c, ""),
                None => (
                    from_last(&samples).unwrap_or(local),
                    "（几次读数对不上，只用了最后一次）",
                ),
            };
            let ahead = c.offset - (env.wall() - env.now());
            env.say(&format!(
                "大麦时间比本机{} {} 毫秒，误差 ±{} 毫秒{note}",
                if ahead >= 0 { "快" } else { "慢" },
                ahead.abs(),
                c.error
            ));
            Ok(c)
        }
        Err(e) if fatal(&e) => Err(stop(env, e)),
        Err(e) => {
            env.say(&format!(
                "拿不到大麦的时间（{e}），按本机时钟倒计时，误差未知"
            ));
            Ok(local)
        }
    }
}

/// A risk-control page or a vanished event ends the watch.
fn fatal(e: &damai::Error) -> bool {
    matches!(e, damai::Error::RiskControl(_) | damai::Error::NotFound(_))
}

fn stop<E: Env>(env: &mut E, e: damai::Error) -> WatchError {
    if let damai::Error::RiskControl(_) = e {
        env.alert("触发风控，已停止盯票", &[e.to_string()]);
    }
    WatchError::Damai(e)
}

fn backoff(interval: i64, failures: u32) -> i64 {
    if failures == 0 {
        return interval;
    }
    interval
        .saturating_mul(1 << failures.min(10))
        .min(MAX_BACKOFF_MS)
        .max(interval)
}

fn remember(last: &mut Last, w: &[Watched]) {
    for x in w {
        last.insert(x.key(), x.status.clone());
    }
}

fn with(name: &str, lines: impl Iterator<Item = String>, at: String) -> Vec<String> {
    std::iter::once(name.to_string())
        .chain(lines)
        .chain(std::iter::once(at))
        .collect()
}

fn time_line<E: Env>(env: &E, clock: Option<&ServerClock>) -> String {
    match clock {
        Some(c) if c.error >= 0 => {
            format!("时间：{}（大麦时间）", beijing(c.damai_at(env.now()), true))
        }
        _ => format!("时间：{}（北京时间，本机时钟）", beijing(env.wall(), false)),
    }
}

fn beijing(ms: i64, millis: bool) -> String {
    let (Some(tz), Some(t)) = (
        FixedOffset::east_opt(8 * 3600),
        DateTime::from_timestamp_millis(ms),
    ) else {
        return ms.to_string();
    };
    let t = t.with_timezone(&tz);
    let f = if millis {
        "%Y-%m-%d %H:%M:%S%.3f"
    } else {
        "%Y-%m-%d %H:%M:%S"
    };
    t.format(f).to_string()
}

/// "1 小时 05 分 03 秒", rounded up to the second.
fn left_text(ms: i64) -> String {
    let s = (ms + 999) / 1000;
    let (h, m, s) = (s / 3600, s % 3600 / 60, s % 60);
    if h > 0 {
        format!("{h} 小时 {m:02} 分 {s:02} 秒")
    } else if m > 0 {
        format!("{m} 分 {s:02} 秒")
    } else {
        format!("{s} 秒")
    }
}

#[cfg(test)]
mod tests;
