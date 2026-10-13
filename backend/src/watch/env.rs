//! Everything a watch does to the world goes through Env: the clock,
//! sleeping, damai.cn and the terminal. Tests drive the loop with a fake on
//! virtual time.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::clock::Sample;
use super::notify::system_notification;
use super::push::PhonePush;
use super::target::session_matches;
use crate::damai::{Client, Error, Event};

const CLOCK_READINGS: usize = 5;
const CLOCK_READING_GAP: Duration = Duration::from_millis(250);

pub(crate) trait Env {
    /// ms on a monotonic clock: what every schedule is computed on.
    fn now(&self) -> i64;
    /// This machine's wall clock, ms since the epoch; for display only.
    fn wall(&self) -> i64;
    async fn sleep_until(&mut self, mono: i64);
    /// The event with only the watched sessions' tiers.
    async fn event(&mut self) -> Result<Event, Error>;
    /// Every session's name as of the last `event`, watched or not.
    fn all_sessions(&self) -> Vec<String>;
    async fn clock_samples(&mut self) -> Result<Vec<Sample>, Error>;
    fn alert(&mut self, title: &str, lines: &[String]);
    fn say(&mut self, line: &str);
    /// A countdown line, `left_ms` before the sale.
    fn countdown(&mut self, line: &str, left_ms: i64);
    /// Tests end a watch this way; a real one ends with Ctrl-C.
    fn stopped(&self) -> bool {
        false
    }
}

pub(crate) struct RealEnv<'a> {
    client: &'a Client,
    item_id: u64,
    session: Option<String>,
    start: Instant,
    all_sessions: Vec<String>,
    system_notify: bool,
    phone_push: Option<PhonePush>,
    tty: bool,
    /// A countdown line is on screen without its newline (on a terminal).
    countdown_open: bool,
    last_countdown_minute: Option<i64>,
}

impl<'a> RealEnv<'a> {
    pub fn new(
        client: &'a Client,
        item_id: u64,
        session: Option<String>,
        system_notify: bool,
    ) -> Self {
        RealEnv {
            client,
            item_id,
            session,
            start: Instant::now(),
            all_sessions: Vec::new(),
            system_notify,
            phone_push: PhonePush::from_config(),
            tty: std::io::stdout().is_terminal(),
            countdown_open: false,
            last_countdown_minute: None,
        }
    }

    fn mono(&self, t: Instant) -> i64 {
        t.saturating_duration_since(self.start).as_millis() as i64
    }

    pub async fn finish_push(&mut self) {
        if let Some(push) = &mut self.phone_push {
            push.finish().await;
        }
    }

    fn end_countdown_line(&mut self) {
        if self.countdown_open {
            println!();
            self.countdown_open = false;
        }
    }
}

impl Env for RealEnv<'_> {
    fn now(&self) -> i64 {
        self.mono(Instant::now())
    }

    fn wall(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    async fn sleep_until(&mut self, mono: i64) {
        let left = mono - self.now();
        if left > 0 {
            tokio::time::sleep(Duration::from_millis(left as u64)).await;
        }
    }

    async fn event(&mut self) -> Result<Event, Error> {
        let names = std::cell::RefCell::new(Vec::new());
        let filter = self.session.clone();
        let event = self
            .client
            .event_where(self.item_id, |name| {
                names
                    .borrow_mut()
                    .push(name.unwrap_or("未知场次").to_string());
                session_matches(filter.as_deref(), name)
            })
            .await?;
        self.all_sessions = names.into_inner();
        Ok(event)
    }

    fn all_sessions(&self) -> Vec<String> {
        self.all_sessions.clone()
    }

    async fn clock_samples(&mut self) -> Result<Vec<Sample>, Error> {
        let mut samples = Vec::with_capacity(CLOCK_READINGS);
        for i in 0..CLOCK_READINGS {
            if i > 0 {
                tokio::time::sleep(CLOCK_READING_GAP).await;
            }
            let r = self.client.clock_reading(self.item_id).await?;
            samples.push(Sample {
                sent: self.mono(r.sent),
                received: self.mono(r.received),
                date_ms: r.date_ms,
            });
        }
        Ok(samples)
    }

    fn alert(&mut self, title: &str, lines: &[String]) {
        self.end_countdown_line();
        println!("\x07【提醒】{title}");
        for l in lines {
            println!("  {l}");
        }
        if let Some(push) = &self.phone_push {
            push.alert(title, lines);
        }
        if self.system_notify
            && let Err(e) = system_notification(title, &lines.join("\n"))
        {
            println!("系统通知发不出去（{e}），之后只在终端提醒");
            self.system_notify = false;
        }
    }

    fn say(&mut self, line: &str) {
        self.end_countdown_line();
        println!("{line}");
    }

    /// On a terminal the line is redrawn in place every second; otherwise
    /// one line a minute.
    fn countdown(&mut self, line: &str, left_ms: i64) {
        if self.tty {
            print!("\r\x1b[2K{line}");
            let _ = std::io::stdout().flush();
            self.countdown_open = true;
        } else {
            let minute = left_ms / 60_000;
            if self.last_countdown_minute != Some(minute) {
                self.last_countdown_minute = Some(minute);
                println!("{line}");
            }
        }
    }
}
