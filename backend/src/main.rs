use std::process::ExitCode;

use theflash::{damai, report};

const USAGE: &str = "用法：
  theflash event <大麦演出链接或ID>   查询演出的场次、票档和售卖状态
  theflash version                    显示版本";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["event", input] => event(input).await,
        ["version" | "--version"] => {
            println!("theflash {}", theflash::version());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

async fn event(input: &str) -> ExitCode {
    let item_id = match damai::parse_item_id(input) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    // Tests point this at a local server that replays saved damai.cn pages.
    let base = std::env::var("THEFLASH_DAMAI_BASE_URL")
        .unwrap_or_else(|_| damai::DEFAULT_BASE_URL.to_string());
    let result = match damai::Client::new(base) {
        Ok(client) => client.event(item_id).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(e) => {
            print!("{}", report::render(&e));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
