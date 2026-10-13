use std::process::ExitCode;
use std::time::Duration;

use theflash::watch::{self, Filter, Options, WatchError};
use theflash::{damai, presentation, report};

const USAGE: &str = "用法：
  theflash event <大麦演出链接或ID>   查询演出的场次、票档和售卖状态
  theflash watch <大麦演出链接或ID> [选项]
                                      开售或有票时提醒，Ctrl-C 停止
      --session <场次>                只盯名称含这段文字的场次，例如 10-17
      --tier <票档>                   只盯名称含这段文字的票档，例如 380
      --interval <秒>                 开售后每隔多少秒查一次，默认 30，最少 10
      --no-system-notify              只在终端提醒，不发系统通知
  theflash version                    显示版本";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["event", input] => event(input, false).await,
        ["event-json", input] => event(input, true).await,
        ["watch", rest @ ..] => watch_command(rest).await,
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

/// Tests point this at a local server that replays saved damai.cn pages.
fn client() -> Result<damai::Client, damai::Error> {
    let base = std::env::var("THEFLASH_DAMAI_BASE_URL")
        .unwrap_or_else(|_| damai::DEFAULT_BASE_URL.to_string());
    damai::Client::new(base)
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

fn print_error(error: &damai::Error, json: bool) {
    if json {
        println!("{}", presentation::error(error));
    } else {
        eprintln!("{error}");
    }
}
