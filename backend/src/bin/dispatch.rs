//! `scripts/dispatch`：有事才启动一个全新的 agent 会话。
//!
//! The dispatcher calls no model itself. It reads GitHub, compares it with
//! what it already started, and starts one fresh agent session per piece
//! of work. Rules and paths: docs/subsystems/dispatch.md; why it looks
//! like this: docs/decisions/0010-dispatcher.md.
//!
//! 退出码：0 做了 / 可以做，1 拒绝，2 用法错或取数据失败。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use theflash::dispatch::config::Config;
use theflash::dispatch::state::{Lock, State};
use theflash::dispatch::{self, Plan, runner, service, snapshot};

const USAGE: &str = "\
用法：
  dispatch plan [--json]        现在该派什么活、为什么不派，什么都不启动
  dispatch tick                 派一轮：启动该启动的会话，收尾已经结束的
  dispatch run                  常驻循环（launchd / systemd 跑的就是它）
  dispatch status               服务在跑没跑、正在跑哪些会话、今天派了几个
  dispatch install [--print]    登记成用户服务（macOS launchd / Linux systemd）
  dispatch uninstall            停掉并删掉服务
  dispatch example-config       打印一份可以直接用的设置

每个子命令都接受：
  --config <文件>   设置（默认 ~/.config/theflash/dispatch.json）
  --replay <目录>   用保存下来的 GitHub 响应判断，不访问 GitHub（测试和演示用）
  --now <时间>      当成这个时刻（带时区，例如 2026-10-14T04:00:00+08:00）；
                    常驻的 run 是把时钟挪过去，不是停在那里
  --dry-run         只打印要做什么，不启动会话、不写状态、不评论
";

fn main() {
    match command() {
        Ok(code) => std::process::exit(code),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    }
}

struct Args {
    command: String,
    config: PathBuf,
    replay: Option<PathBuf>,
    now: DateTime<Utc>,
    /// How far `--now` moved the clock. The resident loop decides from its
    /// own time every tick, so `--now` shifts that clock rather than
    /// freezing it: a frozen clock means a session never reaches its
    /// timeout, a daily ceiling never turns over and a job due later
    /// today never becomes due.
    offset: chrono::TimeDelta,
    json: bool,
    dry_run: bool,
    print: bool,
}

impl Args {
    /// This tick's time.
    fn now(&self) -> DateTime<Utc> {
        dispatch::now() + self.offset
    }
}

fn parse() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(|| USAGE.to_string())?;
    let mut out = Args {
        command,
        config: dispatch::config::default_path(),
        replay: None,
        now: dispatch::now(),
        offset: chrono::TimeDelta::zero(),
        json: false,
        dry_run: false,
        print: false,
    };
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{arg} 后面要跟一个值\n\n{USAGE}"))
        };
        match arg.as_str() {
            "--config" => out.config = PathBuf::from(value()?),
            "--replay" => out.replay = Some(PathBuf::from(value()?)),
            "--now" => {
                let text = value()?;
                out.now = DateTime::parse_from_rfc3339(&text)
                    .map_err(|e| format!("--now {text} 不是带时区的时间：{e}"))?
                    .with_timezone(&Utc);
                out.offset = out.now - dispatch::now();
            }
            "--json" => out.json = true,
            "--dry-run" => out.dry_run = true,
            "--print" => out.print = true,
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("不认识的参数 {other}\n\n{USAGE}")),
        }
    }
    Ok(out)
}

fn command() -> Result<i32, String> {
    let args = parse()?;
    match args.command.as_str() {
        "example-config" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&Config::example()).map_err(|e| e.to_string())?
            );
            eprintln!(
                "\n# 存到 {}，把 checkout 改成这台机器上的 theFlash 路径，\n\
                 # 再 scripts/dispatch install。agent 的凭据由各自的命令行自己管，不进这个文件，也不进仓库。",
                dispatch::config::default_path().display()
            );
            Ok(0)
        }
        "plan" => plan(&args),
        "tick" => tick(&args),
        "run" => run(&args),
        "status" => status(&args),
        "install" => install(&args),
        "uninstall" => {
            println!("{}", service::uninstall(service::kind())?);
            Ok(0)
        }
        other => Err(format!("不认识的命令 {other}\n\n{USAGE}")),
    }
}

fn read_config(args: &Args) -> Result<Config, String> {
    Config::read(&args.config).map_err(|e| {
        format!(
            "{e}\n（`dispatch example-config` 打印一份可以直接用的设置，存到 {}）",
            args.config.display()
        )
    })
}

/// What to do at `now`. Every command passes the time it is deciding for,
/// and the resident loop passes this tick's.
fn read_plan(args: &Args, config: &Config, now: DateTime<Utc>) -> Result<(Plan, State), String> {
    let snapshot = match &args.replay {
        Some(dir) => snapshot::Snapshot::replay(dir).map_err(|e| e.to_string())?,
        None => snapshot::fetch(&config.gh_command, &config.repo, &config.test_workflow)
            .map_err(|e| e.to_string())?,
    };
    let state = State::read(&config.state)?;
    let plan = dispatch::plan(&snapshot, &state, config, now);
    Ok((plan, state))
}

fn plan(args: &Args) -> Result<i32, String> {
    let config = read_config(args)?;
    let (plan, _) = read_plan(args, &config, args.now)?;
    if args.json {
        println!("{}", as_json(&plan));
        return Ok(0);
    }
    print_plan(&plan, &config);
    Ok(if plan.start.is_empty() { 1 } else { 0 })
}

fn print_plan(plan: &Plan, config: &Config) {
    for sweep in &plan.sweep {
        println!(
            "收尾 {}：{}，跑了 {} 分钟，日志 {}{}",
            sweep.id,
            sweep.outcome.name(),
            sweep.after_minutes,
            sweep.log.display(),
            if sweep.release_claim {
                "，要释放认领并在 issue 上说明"
            } else {
                ""
            }
        );
    }
    if plan.start.is_empty() {
        println!("现在没有要派的活。");
    }
    for task in &plan.start {
        println!("派 {} → {}：{}", task.id, task.vendor, task.why);
    }
    for (task, skip) in &plan.skip {
        println!("不派 {}（{}）：{}", task.id, task.role.name(), skip.why());
    }
    println!(
        "上限：同时 {} 个会话，今天最多 {} 个；云端负责 {}。",
        config.max_sessions,
        config.max_sessions_per_day,
        if config.cloud_roles.is_empty() {
            "（没有）".to_string()
        } else {
            config.cloud_roles.join("、")
        }
    );
}

fn as_json(plan: &Plan) -> String {
    let start: Vec<serde_json::Value> = plan
        .start
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "role": t.role.name(),
                "vendor": t.vendor,
                "issue": t.issue,
                "pull": t.pull,
                "why": t.why,
            })
        })
        .collect();
    let skip: Vec<serde_json::Value> = plan
        .skip
        .iter()
        .map(|(t, s)| {
            serde_json::json!({
                "id": t.id,
                "role": t.role.name(),
                "issue": t.issue,
                "pull": t.pull,
                "skipped": s.why(),
            })
        })
        .collect();
    let sweep: Vec<serde_json::Value> = plan
        .sweep
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "issue": s.issue,
                "outcome": s.outcome.name(),
                "after_minutes": s.after_minutes,
                "release_claim": s.release_claim,
                "log": s.log,
            })
        })
        .collect();
    serde_json::json!({ "start": start, "skip": skip, "sweep": sweep }).to_string()
}

fn tick(args: &Args) -> Result<i32, String> {
    let config = read_config(args)?;
    let _lock = Lock::take(&config.state)?;
    let now = args.now();
    let (plan, mut state) = read_plan(args, &config, now)?;
    if args.dry_run {
        print_plan(&plan, &config);
        return Ok(if plan.start.is_empty() { 1 } else { 0 });
    }
    let done = runner::run(
        &plan,
        &mut state,
        &config,
        now,
        &mut runner::Sessions::new(),
    );
    report(&done, &plan, now);
    Ok(if done.started.is_empty() { 1 } else { 0 })
}

fn report(done: &runner::Done, plan: &Plan, now: DateTime<Utc>) {
    let at = dispatch::stamp(now);
    for note in &done.ended {
        println!("[{at}] 收尾 {note}");
    }
    for note in &done.started {
        println!("[{at}] 启动 {note}");
    }
    for note in &done.notes {
        println!("[{at}] {note}");
    }
    if done.started.is_empty() && done.ended.is_empty() {
        println!("[{at}] 没有要派的活（{} 个候选被跳过）", plan.skip.len());
    }
}

/// The resident loop the service runs. One tick per interval; a tick that
/// fails (GitHub unreachable, a settings file that changed under it) is
/// logged and the loop goes on, because the network comes back and
/// exiting would need launchd to restart it.
fn run(args: &Args) -> Result<i32, String> {
    let config = read_config(args)?;
    let _lock = Lock::take(&config.state)?;
    let interval = std::time::Duration::from_secs(config.interval_seconds.max(5));
    println!(
        "[{}] 调度程序启动：{}，每 {} 秒看一次，同时最多 {} 个会话，今天最多 {} 个{}",
        dispatch::stamp(args.now()),
        config.repo,
        config.interval_seconds,
        config.max_sessions,
        config.max_sessions_per_day,
        if config.require_label.is_empty() {
            String::new()
        } else {
            format!("，只认带 `{}` 标签的 issue 和 PR", config.require_label)
        }
    );
    // The sessions this dispatcher started, kept so their exit codes are
    // known across ticks.
    let mut sessions = runner::Sessions::new();
    loop {
        // This tick's time, used for the plan, for starting sessions and
        // for closing them out alike: a plan made at one time and carried
        // out at another is how a session ran past its timeout without
        // anyone noticing.
        let now = args.now();
        match read_plan(args, &config, now) {
            Ok((plan, mut state)) => {
                let done = runner::run(&plan, &mut state, &config, now, &mut sessions);
                report(&done, &plan, now);
            }
            Err(why) => println!("[{}] 这一轮没看成：{why}", dispatch::stamp(now)),
        }
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::thread::sleep(interval);
    }
}

fn status(args: &Args) -> Result<i32, String> {
    let config = read_config(args)?;
    let state = State::read(&config.state)?;
    let kind = service::kind();
    println!(
        "服务: {}（{}）",
        service::running(kind),
        service::unit_path(kind).display()
    );
    println!("仓库: {}，设置: {}", config.repo, args.config.display());
    println!(
        "上限: 同时 {}，今天 {}（已经派了 {}）；会话超时 {} 分钟",
        config.max_sessions,
        config.max_sessions_per_day,
        state.started_today(args.now),
        config.session_timeout_minutes,
    );
    if !config.require_label.is_empty() {
        println!("只认带 `{}` 标签的 issue 和 PR", config.require_label);
    }
    println!(
        "云端负责: {}",
        if config.cloud_roles.is_empty() {
            "（没有）".to_string()
        } else {
            config.cloud_roles.join("、")
        }
    );
    let running: Vec<_> = state.running().collect();
    if running.is_empty() {
        println!("现在没有会话在跑。");
    }
    for session in running {
        println!(
            "在跑 {}（{}，{}）：{} 分钟，{}，日志 {}",
            session.id,
            session.role,
            session.vendor,
            session.minutes_since_start(args.now),
            if session.process_alive() {
                "进程还在"
            } else {
                "进程已经没了，下一轮收尾"
            },
            session.log.display(),
        );
    }
    for session in state
        .sessions
        .iter()
        .rev()
        .take(5)
        .filter(|s| !s.is_running())
    {
        println!(
            "跑过 {}（{}，{}）：{}，日志 {}",
            session.id,
            session.role,
            session.vendor,
            session.outcome.map(|o| o.name()).unwrap_or("不知道"),
            session.log.display(),
        );
    }
    Ok(0)
}

fn install(args: &Args) -> Result<i32, String> {
    let config = read_config(args)?;
    let kind = service::kind();
    let program = program_path()?;
    if args.print {
        // What the service file would say, with the program it would
        // actually run: a copy outside the repository (why:
        // service::installed_program).
        print!(
            "{}",
            service::unit(
                kind,
                &service::installed_program(),
                &args.config,
                &config.logs
            )
        );
        return Ok(0);
    }
    println!(
        "{}",
        service::install(kind, &program, &args.config, &config.logs)?
    );
    println!(
        "日志在 {}；scripts/dispatch status 看它在做什么；scripts/dispatch uninstall 卸掉。",
        config.logs.display()
    );
    Ok(0)
}

/// The absolute path of this binary, which the service file has to name.
/// A service that named `cargo run` would depend on a toolchain and a
/// working directory that may not be there at boot.
fn program_path() -> Result<PathBuf, String> {
    let path = std::env::current_exe().map_err(|e| format!("问不出自己的路径：{e}"))?;
    Ok(canonical(&path))
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
