//! End-to-end checks of `theflash watch` against a local server that
//! replays real damai.cn responses. Timing logic (the sale moment, restocks,
//! backoff) is tested on virtual time in src/watch; these check the wiring:
//! arguments, filters, output, the damai.cn clock, Ctrl-C.

mod support;

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use support::MockDamai;

/// A running `theflash watch`. Killed and reaped when dropped, so no test
/// leaves one behind.
struct Watch {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Watch {
    fn start(damai: &MockDamai, args: &[&str]) -> Self {
        let mut child = support::theflash(&damai.base_url())
            .arg("watch")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Watch {
            child,
            lines,
            seen: Vec::new(),
        }
    }

    /// Reads output until a line contains one of `needles`; false after 15 s.
    fn wait_for(&mut self, needles: &[&str]) -> bool {
        let deadline = Instant::now() + Duration::from_secs(15);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    let hit = needles.iter().any(|n| line.contains(n));
                    self.seen.push(line);
                    if hit {
                        return true;
                    }
                }
                Err(_) => return false,
            }
        }
        false
    }

    /// Ctrl-C, then the exit status and everything printed.
    fn interrupt(mut self) -> (ExitStatus, String, String) {
        let pid = self.child.id().to_string();
        assert!(
            std::process::Command::new("kill")
                .args(["-INT", &pid])
                .status()
                .unwrap()
                .success()
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                break s;
            }
            assert!(Instant::now() < deadline, "still running 10 s after Ctrl-C");
            std::thread::sleep(Duration::from_millis(50));
        };
        while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(500)) {
            self.seen.push(line);
        }
        let mut err = String::new();
        if let Some(mut e) = self.child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        (status, self.seen.join("\n"), err)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn on_sale() -> MockDamai {
    MockDamai::start(&[
        ("/item.htm?id=1086791513717", "item_on_sale.html"),
        ("dataType=2&dataId=286937789", "sku_on_sale_286937789.jsonp"),
        ("dataType=2&dataId=286937790", "sku_on_sale_286937790.jsonp"),
        ("dataType=2&dataId=286937788", "sku_on_sale_286937788.jsonp"),
    ])
}

#[test]
fn a_tier_on_sale_now_alerts_at_once_and_ctrl_c_stops_the_watch() {
    let damai = on_sale();
    let mut w = Watch::start(
        &damai,
        &[
            "1086791513717",
            "--session",
            "10-15",
            "--tier",
            "C-178",
            "--no-system-notify",
        ],
    );
    assert!(w.wait_for(&["【提醒】现在就可以买"]), "{:#?}", w.seen);
    assert!(
        w.wait_for(&["2026-10-15 周四 17:00 · C-178元 · ¥178.00 · 可售"]),
        "{:#?}",
        w.seen
    );
    let (status, out, err) = w.interrupt();
    assert!(status.success(), "{status:?} {err}");
    assert!(out.contains("已停止盯票"), "{out}");
    // Only the page and the one session watched.
    let hits = damai.hits();
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert!(hits[1].contains("dataId=286937790"), "{hits:?}");
}

#[test]
fn an_out_of_stock_tier_is_watched_without_an_alert() {
    let damai = on_sale();
    let mut w = Watch::start(
        &damai,
        &[
            "1086791513717",
            "--session",
            "10-15",
            "--tier",
            "D-118",
            "--no-system-notify",
        ],
    );
    assert!(
        w.wait_for(&["2026-10-15 周四 17:00 · D-118元（现在：缺货）"]),
        "{:#?}",
        w.seen
    );
    assert!(w.wait_for(&["查询节奏：每 30 秒查一次"]), "{:#?}", w.seen);
    let (status, out, _) = w.interrupt();
    assert!(status.success());
    assert!(!out.contains("【提醒】"), "{out}");
}

#[test]
fn a_filter_matching_nothing_lists_what_there_is() {
    let damai = on_sale();
    let out = support::output_within(
        {
            let mut c = support::theflash(&damai.base_url());
            c.args(["watch", "1086791513717", "--session", "12-25"]);
            c
        },
        15,
    );
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("没有匹配“12-25”的场次"), "{err}");
    assert!(err.contains("2026-10-14 周三 17:00"), "{err}");
}

#[test]
fn risk_control_stops_the_watch_with_an_alert() {
    let damai = MockDamai::start(&[("/item.htm?id=1086791513717", "risk_control.html")]);
    let out = support::output_within(
        {
            let mut c = support::theflash(&damai.base_url());
            c.args(["watch", "1086791513717", "--no-system-notify"]);
            c
        },
        15,
    );
    assert!(!out.status.success());
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("【提醒】触发风控，已停止盯票"), "{all}");
    assert_eq!(damai.hits().len(), 1);
}

// damai.cn's pretend clock is two hours before 1092337348536's sale
// (2026-10-14 10:18 Beijing), whatever this machine's clock says: the
// countdown must come from damai.cn's Date header.
#[test]
fn the_countdown_runs_on_damai_time_not_this_machines() {
    const SALE: i64 = 1_791_944_280_000;
    let damai = MockDamai::start_with_clock(
        &[
            ("/item.htm?id=1092337348536", "item_not_yet_on_sale.html"),
            (
                "dataType=2&dataId=286966465",
                "sku_not_yet_on_sale_286966465.jsonp",
            ),
        ],
        SALE - 2 * 3600 * 1000,
    );
    let mut w = Watch::start(&damai, &["1092337348536", "--no-system-notify"]);
    assert!(
        w.wait_for(&["距离开售 2 小时 00 分", "距离开售 1 小时 59 分"]),
        "{:#?}",
        w.seen
    );
    let (status, out, _) = w.interrupt();
    assert!(status.success());
    assert!(
        out.contains("开售时间：2026-10-14 10:18:00（北京时间）"),
        "{out}"
    );
    assert!(
        damai
            .hits()
            .iter()
            .filter(|h| h.starts_with("HEAD "))
            .count()
            >= 3,
        "{:?}",
        damai.hits()
    );
}
