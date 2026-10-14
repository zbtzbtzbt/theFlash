use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use theflash::watch::{self, Filter, Options, WatchError};
use theflash::{damai, inspect, presentation, report};

mod ui;

const USAGE: &str = "用法：
  theflash event <大麦演出链接或ID>   查询演出的场次、票档和售卖状态
  theflash serve [选项]               启动本机浏览器查询界面，Ctrl-C 停止
      --port <端口>                   绑定 127.0.0.1 的端口，默认 0（由系统分配）
      --no-open                       只打印本机地址，不自动打开浏览器
  theflash watch <大麦演出链接或ID> [选项]
                                      开售或有票时提醒，Ctrl-C 停止
      --session <场次>                只盯名称含这段文字的场次，例如 10-17
      --tier <票档>                   只盯名称含这段文字的票档，例如 380
      --count <张数>                  确认页购买张数（1～6），默认 1
      --interval <秒>                 开售后每隔多少秒查一次，默认 30，最少 10
      --no-open                       只提醒，不在浏览器里自动打开购买确认页
      --no-system-notify              只在终端提醒，不发系统通知
  theflash inspect [选项]             巡检真实大麦的页面结构，变了就报出来
      --samples <文件>                样本集，默认放在用户状态目录里
      --candidates <目录>             把脱敏后的候选 fixture 写到这里
      --max-events <场数>             最多查几场，默认 8（够查 5 类并换样本）
      --gap <秒>                      相邻请求至少隔几秒，默认 2
      --open-issue                    发现变化时开 issue 或追加评论（用 gh）
      --repo <owner/repo>             开 issue 开在哪个仓库，默认当前目录的
  theflash inspect record <链接或ID|home> [选项]
                                      把真实响应脱敏后录成 fixture
      --as <名字>                     文件名，默认 item_<ID>
      --dir <目录>                    写到哪里，默认 backend/testdata/damai
      --sessions <场次数>             录几个场次的票档，默认 2
      --gap <秒>                      相邻请求至少隔几秒，默认 2
  theflash risk                       显示大麦风控熔断状态（熔断生效时退出码 1）
  theflash risk resume --yes          解除熔断，先确认 owner 已经看过这次风控
  theflash version                    显示版本";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["event", input] => event(input, false).await,
        ["event-json", input] => event(input, true).await,
        ["serve", rest @ ..] => serve_command(rest).await,
        ["watch", rest @ ..] => watch_command(rest).await,
        ["inspect", rest @ ..] => inspect_command(rest).await,
        ["risk", rest @ ..] => risk_command(rest),
        ["version" | "--version"] => {
            println!("theflash {}", theflash::version());
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

const BASE_URL_VAR: &str = "THEFLASH_DAMAI_BASE_URL";

/// The replay server tests point theFlash at, when one is set. `client`
/// and the request pacing both read it here, so they cannot disagree about
/// whether the requests leave this machine: a value this process cannot
/// read as text is kept exactly as it reads it, which is neither the real
/// damai.cn nor a loopback address, so requests fail instead of quietly
/// going to the real site at a replay server's pace.
fn base_url() -> Option<String> {
    std::env::var_os(BASE_URL_VAR).map(|v| v.to_string_lossy().into_owned())
}

/// Tests point this at a local server that replays saved damai.cn pages.
/// The same server stands in for the home page, so no test reaches
/// www.damai.cn either.
pub(crate) fn client() -> Result<damai::Client, damai::Error> {
    match base_url() {
        Some(base) => Ok(damai::Client::new(&base)?.with_home(base)),
        None => damai::Client::new(damai::DEFAULT_BASE_URL),
    }
}

async fn serve_command(args: &[&str]) -> ExitCode {
    let mut port = 0u16;
    let mut open_browser = true;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match *arg {
            "--port" => match it.next().and_then(|s| s.parse::<u16>().ok()) {
                Some(p) => port = p,
                None => return usage(),
            },
            "--no-open" => open_browser = false,
            _ => return usage(),
        }
    }
    match ui::serve(port, open_browser).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn event(input: &str, json: bool) -> ExitCode {
    let item_id = match damai::parse_item_id(input) {
        Ok(id) => id,
        Err(e) => {
            print_error(&e, json);
            return ExitCode::from(2);
        }
    };
    let result = match client() {
        Ok(client) => client.event(item_id).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(e) => {
            if json {
                println!("{}", presentation::event(&e));
            } else {
                print!("{}", report::render(&e));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            print_error(&e, json);
            ExitCode::FAILURE
        }
    }
}

async fn watch_command(args: &[&str]) -> ExitCode {
    let mut input = None;
    let mut filter = Filter::default();
    let mut interval = watch::DEFAULT_INTERVAL;
    let mut system_notify = true;
    let mut open_browser = true;
    let mut count = 1u32;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match *arg {
            "--session" | "--tier" => {
                let Some(v) = it.next() else { return usage() };
                if *arg == "--session" {
                    filter.session = Some(v.to_string());
                } else {
                    filter.tier = Some(v.to_string());
                }
            }
            "--count" => match it.next().and_then(|s| s.parse::<u32>().ok()) {
                Some(n @ 1..=6) => count = n,
                _ => {
                    eprintln!("--count 要一个 1 到 6 之间的整数张数");
                    return ExitCode::from(2);
                }
            },
            "--interval" => match it.next().and_then(|s| s.parse::<u64>().ok()) {
                Some(s) if Duration::from_secs(s) >= watch::MIN_INTERVAL => {
                    interval = Duration::from_secs(s);
                }
                _ => {
                    eprintln!(
                        "--interval 要一个不小于 {} 的秒数：查得太勤会触发大麦的风控",
                        watch::MIN_INTERVAL.as_secs()
                    );
                    return ExitCode::from(2);
                }
            },
            "--no-open" => open_browser = false,
            "--no-system-notify" => system_notify = false,
            s if s.starts_with("--") || input.is_some() => return usage(),
            s => input = Some(s),
        }
    }
    let Some(input) = input else { return usage() };
    let item_id = match damai::parse_item_id(input) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let client = match client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let opts = Options {
        item_id,
        filter,
        interval,
        system_notify,
        open_browser,
        count,
    };
    tokio::select! {
        r = watch::watch(&client, &opts) => match r {
            Ok(()) => ExitCode::SUCCESS,
            Err(WatchError::NothingToWatch(why)) => {
                eprintln!("{why}");
                ExitCode::from(2)
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
        _ = tokio::signal::ctrl_c() => {
            println!("\n已停止盯票");
            ExitCode::SUCCESS
        }
    }
}

/// `theflash inspect` and `theflash inspect record`: the two commands that
/// an agent, not a user, points at the real damai.cn. Why they exist and
/// what they cost in requests: docs/subsystems/damai-inspect.md.
async fn inspect_command(args: &[&str]) -> ExitCode {
    match args {
        ["record", rest @ ..] => inspect_record(rest).await,
        rest => inspect_run(rest).await,
    }
}

/// Seconds between requests. Asking for less than the default is only
/// allowed when the requests go to a replay server on this machine, so
/// neither a mistyped flag nor a base URL pointing elsewhere can turn an
/// inspection into a burst at a real site. The rule itself is
/// `inspect::request_gap`, where a test holds it.
fn request_gap(asked: Option<u64>) -> Duration {
    let (gap, refused) = inspect::request_gap(asked, base_url().as_deref());
    if let Some(line) = refused {
        eprintln!("{line}");
    }
    gap
}

async fn inspect_run(args: &[&str]) -> ExitCode {
    let mut samples: Option<String> = None;
    let mut candidates: Option<String> = None;
    let mut max_events = inspect::DEFAULT_MAX_EVENTS;
    let mut gap = None;
    let mut open_issue = false;
    let mut repo = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match *arg {
            "--samples" => match it.next() {
                Some(v) => samples = Some(v.to_string()),
                None => return usage(),
            },
            "--candidates" => match it.next() {
                Some(v) => candidates = Some(v.to_string()),
                None => return usage(),
            },
            "--repo" => match it.next() {
                Some(v) => repo = Some(v.to_string()),
                None => return usage(),
            },
            "--max-events" => match it.next().and_then(|v| v.parse().ok()) {
                Some(n) if n >= 1 => max_events = n,
                _ => return usage(),
            },
            "--gap" => match it.next().and_then(|v| v.parse().ok()) {
                Some(s) => gap = Some(s),
                None => return usage(),
            },
            "--open-issue" => open_issue = true,
            _ => return usage(),
        }
    }
    let samples_path = match samples {
        Some(p) => PathBuf::from(p),
        None => match inspect::state_dir() {
            Ok(dir) => dir.join("inspect-samples.json"),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        },
    };
    let opts = inspect::Options {
        samples_path,
        candidates_dir: candidates.map(PathBuf::from),
        max_events,
        gap: request_gap(gap),
    };
    let client = match client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let report = match inspect::run(&client, &opts).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", inspect::render(&report));
    if open_issue {
        match inspect::report_findings(&report, &inspect::tracker::Gh::new(repo)) {
            Ok(Some(posted)) => println!("{posted}"),
            Ok(None) => {}
            Err(e) => {
                eprintln!("没能把变化写到 GitHub 上：{e}");
                return ExitCode::FAILURE;
            }
        }
    }
    // Something changed, or a sample could not be read: an agent has to
    // look at this run, so the exit code says so.
    if report.has_findings() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

async fn inspect_record(args: &[&str]) -> ExitCode {
    const DEFAULT_DIR: &str = "backend/testdata/damai";
    let mut target = None;
    let mut name = None;
    let mut dir = None;
    let mut sessions = 2;
    let mut gap = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match *arg {
            "--as" => match it.next() {
                Some(v) => name = Some(v.to_string()),
                None => return usage(),
            },
            "--dir" => match it.next() {
                Some(v) => dir = Some(v.to_string()),
                None => return usage(),
            },
            "--sessions" => match it.next().and_then(|v| v.parse().ok()) {
                Some(n) => sessions = n,
                None => return usage(),
            },
            "--gap" => match it.next().and_then(|v| v.parse().ok()) {
                Some(s) => gap = Some(s),
                None => return usage(),
            },
            s if s.starts_with("--") || target.is_some() => return usage(),
            s => target = Some(s),
        }
    }
    let Some(target) = target else { return usage() };
    let (target, default_name) = if target == "home" || target == "首页" {
        (inspect::Target::Home, "home".to_string())
    } else {
        match damai::parse_item_id(target) {
            Ok(id) => (inspect::Target::Item(id), id.to_string()),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(2);
            }
        }
    };
    let dir = PathBuf::from(dir.unwrap_or_else(|| DEFAULT_DIR.to_string()));
    if !dir.is_dir() {
        eprintln!(
            "{} 不是一个目录：请在仓库根目录运行，或者用 --dir 指定把 fixture 写到哪里",
            dir.display()
        );
        return ExitCode::from(2);
    }
    let client = match client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    match inspect::record(
        &client,
        target,
        &dir,
        &name.unwrap_or(default_name),
        sessions,
        request_gap(gap),
    )
    .await
    {
        Ok(written) if written.is_empty() => {
            eprintln!("没有写出任何 fixture");
            ExitCode::FAILURE
        }
        Ok(written) => {
            for path in written {
                println!("{}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// The risk-control breaker every theFlash process shares. It never clears
/// itself: the owner decides what to do after risk control, so clearing it
/// is a command the user runs on purpose. It reads and writes this
/// machine's state only, so it asks the access layer for the state
/// directory alone and not for the request pacing (#83).
fn risk_command(args: &[&str]) -> ExitCode {
    let gate = match damai::Gate::for_breaker_only() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let show = |stop: &damai::RiskStop| {
        println!("大麦风控熔断生效中：");
        println!("  {stop}");
        println!("所有 theFlash 进程都不会向大麦发请求。");
    };
    match args {
        [] => match gate.stop() {
            Ok(None) => {
                println!("没有大麦风控熔断，查询正常。");
                ExitCode::SUCCESS
            }
            Ok(Some(stop)) => {
                show(&stop);
                println!("确认 owner 已经看过这次风控后，运行 `theflash risk resume --yes` 解除。");
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
        ["resume"] => match gate.stop() {
            Ok(None) => {
                println!("没有大麦风控熔断，不用解除。");
                ExitCode::SUCCESS
            }
            Ok(Some(stop)) => {
                show(&stop);
                eprintln!(
                    "没有解除：请先确认 owner 已经看过这次风控并同意继续，再运行 `theflash risk resume --yes`。"
                );
                ExitCode::from(2)
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
        ["resume", "--yes"] => match gate.resume() {
            Ok(damai::Resumed::Nothing) => {
                println!("没有大麦风控熔断，不用解除。");
                ExitCode::SUCCESS
            }
            Ok(damai::Resumed::Cleared(stop)) => {
                println!("已解除这次熔断：");
                println!("  {stop}");
                println!("只有在 owner 看过这次风控并同意继续时才应该这么做。");
                ExitCode::SUCCESS
            }
            Ok(damai::Resumed::ClearedUnreadable(path)) => {
                println!("已删掉读不懂的熔断状态文件（{path}），查询恢复。");
                println!(
                    "文件里原来记的是哪一次风控已经没了；如果不确定是不是真的发生过风控，先问 owner。"
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
        _ => usage(),
    }
}

fn print_error(error: &damai::Error, json: bool) {
    if json {
        println!("{}", presentation::error(error));
    } else {
        eprintln!("{error}");
    }
}
