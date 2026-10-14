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
        Self::start_with_gap(damai, args, None)
    }

    fn start_with_gap(damai: &MockDamai, args: &[&str], gap: Option<&str>) -> Self {
        let mut command = support::theflash(&damai.base_url());
        command.env_remove("THEFLASH_REQUEST_GAP_MS");
        if let Some(gap) = gap {
            command.env("THEFLASH_REQUEST_GAP_MS", gap);
        }
        let mut child = command
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
    // Its own HOME: the breaker this trips would stop the other tests'
    // queries. tests/access.rs checks that it does stop other processes.
    let home = support::Home::new("watch-risk");
    let damai = MockDamai::start(&[("/item.htm?id=1086791513717", "risk_control.html")]);
    let out = support::output_within(
        {
            let mut c = home.theflash(&damai.base_url());
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

#[test]
fn explicit_validation_gap_paces_heads_and_the_get_to_head_transition() {
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
    let mut watch = Watch::start_with_gap(
        &damai,
        &["1092337348536", "--no-system-notify"],
        Some("1000"),
    );
    assert!(watch.wait_for(&["距离开售"]), "{:#?}", watch.seen);
    let (status, _, err) = watch.interrupt();
    assert!(status.success(), "{status:?} {err}");
    let hits = damai.timed_hits();
    assert_eq!(hits.len(), 7, "{hits:?}");
    assert!(
        hits[..2]
            .iter()
            .all(|(request, _)| request.starts_with("GET "))
    );
    assert!(
        hits[2..]
            .iter()
            .all(|(request, _)| request.starts_with("HEAD "))
    );
    // Check the shared budget at GET -> HEAD as well as all five HEADs.
    // No upper bound; scheduling jitter gets 150 ms of lower-bound margin.
    for pair in hits[1..].windows(2) {
        let elapsed = pair[1].1.duration_since(pair[0].1);
        assert!(
            elapsed >= Duration::from_millis(850),
            "HEAD request gap missing: {} -> {} took {elapsed:?}",
            pair[0].0,
            pair[1].0
        );
    }
}

#[test]
fn watch_hands_off_the_selected_tier_to_the_browser_and_no_open_skips_it() {
    let damai = on_sale();
    let home = support::Home::new("watch-handoff");
    let opened_log = home.path().join("opened.txt");
    // Open the log file before sleeping so `opened_log.exists()` is already
    // true while the file is still 0 bytes, and sleep past the old 1.5 s / 2 s
    // wall-clock deadlines (#148): `watch` must wait for the launcher to
    // finish before reporting the handoff line on stdout.
    let browser = support::stub(
        home.path(),
        "fake-browser",
        &format!(
            "exec >> '{}'\nsleep 2.2\nprintf '%s\\n' \"$1\"",
            opened_log.display()
        ),
    );

    let mut command = home.theflash(&damai.base_url());
    command.env("THEFLASH_BROWSER", &browser);
    let mut child = command
        .args([
            "watch",
            "1086791513717",
            "--session",
            "10-15",
            "--tier",
            "C-178",
            "--count",
            "2",
            "--no-system-notify",
        ])
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
    let mut watch = Watch {
        child,
        lines,
        seen: Vec::new(),
    };
    assert!(
        watch.wait_for(&["购买确认页（2026-10-15 周四 17:00 · C-178元，2 张）："]),
        "{:#?}",
        watch.seen
    );
    let (status, stdout, stderr) = watch.interrupt();
    assert!(status.success(), "{status:?} {stderr}");
    assert!(stdout.contains("开售前检查："), "{stdout}");
    let launched = std::fs::read_to_string(&opened_log).unwrap_or_default();
    let urls: Vec<&str> = launched.lines().collect();
    assert_eq!(urls.len(), 1, "浏览器应当只被拉起一次：{launched}");
    assert!(
        urls[0].starts_with("https://m.damai.cn/app/dmfe/h5-ultron-buy/index.html?")
            && urls[0].contains("buyParam=1086791513717_2_6141329425571")
            && urls[0].contains("%22signKey%22%3A%22cEl%2BbnlbXgFiQVx%2FTklha3Fddm94XVABYUNcfEFb")
            && urls[0].contains("buyNow=true")
            && urls[0].contains("rtc=1"),
        "交接给浏览器的确认页 URL 不对：{}",
        urls[0]
    );
    // Program only performs read-only GETs; it never submits an order itself (#6).
    assert_eq!(damai.hits().len(), 2, "{:?}", damai.hits());

    // A launcher that exits non-zero (e.g. xdg-open without a browser) must
    // print a fallback warning instead of failing silently.
    let failing_browser = support::stub(home.path(), "fail-browser", "exit 1");
    let mut cmd_fail = home.theflash(&damai.base_url());
    cmd_fail.env("THEFLASH_BROWSER", &failing_browser);
    let mut child_fail = cmd_fail
        .args([
            "watch",
            "1086791513717",
            "--session",
            "10-15",
            "--tier",
            "C-178",
            "--no-system-notify",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out_fail = child_fail.stdout.take().unwrap();
    let (tx_fail, lines_fail) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out_fail).lines().map_while(Result::ok) {
            if tx_fail.send(line).is_err() {
                break;
            }
        }
    });
    let mut watch_fail = Watch {
        child: child_fail,
        lines: lines_fail,
        seen: Vec::new(),
    };
    assert!(
        watch_fail.wait_for(&["购买确认页（2026-10-15 周四 17:00 · C-178元，1 张）："]),
        "{:#?}",
        watch_fail.seen
    );
    let (status_fail, stdout_fail, _) = watch_fail.interrupt();
    assert!(status_fail.success());
    assert!(
        stdout_fail.contains("浏览器打不开（启动器退出码 1）"),
        "启动器非零退出应提示手动打开链接：{stdout_fail}"
    );

    // Seat-selection (`sku_calendar_286938129.jsonp`, `btnStatus=206`,
    // `chooseSeatType=1`): opens `select-seat-biz/kylin.html` and says to
    // pick seats on the page instead of claiming `2 张`.
    let mut cal_sku = support::fixture("sku_calendar_286938129.jsonp");
    cal_sku = cal_sku
        .replace("\"frontEndStatus\":\"2\"", "\"frontEndStatus\":\"1\"")
        .replace("\"skuSalable\":\"false\"", "\"skuSalable\":\"true\"");
    let cal_damai = MockDamai::start_bodies(vec![
        (
            "/item.htm?id=1084537866279".to_string(),
            support::fixture("item_calendar.html"),
        ),
        ("dataId=286938129".to_string(), cal_sku),
    ]);
    let _ = std::fs::remove_file(&opened_log);
    let mut cmd_seat = home.theflash(&cal_damai.base_url());
    cmd_seat.env("THEFLASH_BROWSER", &browser);
    let mut child_seat = cmd_seat
        .args([
            "watch",
            "1084537866279",
            "--session",
            "2026-10-17 星期六 14:30",
            "--tier",
            "F（380元）",
            "--count",
            "2",
            "--no-system-notify",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out_seat = child_seat.stdout.take().unwrap();
    let (tx_seat, lines_seat) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out_seat).lines().map_while(Result::ok) {
            if tx_seat.send(line).is_err() {
                break;
            }
        }
    });
    let mut watch_seat = Watch {
        child: child_seat,
        lines: lines_seat,
        seen: Vec::new(),
    };
    assert!(
        watch_seat.wait_for(&[
            "购买确认页（2026-10-17 星期六 14:30 · F（380元），选座购票请在页面内选座）："
        ]),
        "{:#?}",
        watch_seat.seen
    );
    let (status_seat, stdout_seat, _) = watch_seat.interrupt();
    assert!(status_seat.success());
    assert!(
        !stdout_seat.contains("2 张"),
        "选座购票不应宣称已预设 2 张：{stdout_seat}"
    );
    let seat_url = std::fs::read_to_string(&opened_log).unwrap_or_default();
    assert!(
        seat_url.starts_with("https://m.damai.cn/app/dmfe/select-seat-biz/kylin.html?")
            && seat_url.contains("performId=286938129")
            && seat_url.contains("projectId=240266008")
            && seat_url.contains("skuId=6139783065605")
            && seat_url.contains("signKey=")
            && seat_url.contains("rtc=1"),
        "选座页 URL 不对：{seat_url}"
    );

    // `--no-open` prints the URL but does not invoke the browser launcher.
    let _ = std::fs::remove_file(&opened_log);
    let mut cmd_no_open = home.theflash(&damai.base_url());
    cmd_no_open.env("THEFLASH_BROWSER", &browser);
    let mut child2 = cmd_no_open
        .args([
            "watch",
            "1086791513717",
            "--session",
            "10-15",
            "--tier",
            "C-178",
            "--no-open",
            "--no-system-notify",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out2 = child2.stdout.take().unwrap();
    let (tx2, lines2) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out2).lines().map_while(Result::ok) {
            if tx2.send(line).is_err() {
                break;
            }
        }
    });
    let mut watch2 = Watch {
        child: child2,
        lines: lines2,
        seen: Vec::new(),
    };
    assert!(
        watch2.wait_for(&["购买确认页（2026-10-15 周四 17:00 · C-178元，1 张）："]),
        "{:#?}",
        watch2.seen
    );
    let (status2, _, _) = watch2.interrupt();
    assert!(status2.success());
    assert!(!opened_log.exists(), "--no-open 时不应调用浏览器启动器");

    // Invalid `--count` is rejected with exit code 2 before any request.
    for bad in ["0", "7", "abc"] {
        let mut bad_cmd = home.theflash(&damai.base_url());
        bad_cmd.args(["watch", "1086791513717", "--count", bad]);
        let bad_out = support::output_within(bad_cmd, 10);
        assert_eq!(bad_out.status.code(), Some(2), "--count {bad}");
        let err = String::from_utf8_lossy(&bad_out.stderr);
        assert!(err.contains("1 到 6"), "--count {bad}: {err}");
    }
}
