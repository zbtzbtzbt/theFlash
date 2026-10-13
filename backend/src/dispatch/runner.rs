//! 启动一个全新的 agent 会话，以及收尾。
//!
//! This is the only part of the dispatcher with side effects: it makes a
//! worktree, starts the agent command line, writes the log, stops a
//! session that ran too long and says so on the issue. The decisions it
//! carries out are made in [`super::plan`], which touches nothing.

use std::collections::BTreeMap;
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use chrono::{DateTime, Utc};

use super::config::{Config, PromptStyle};
use super::state::{Outcome, Session, State};
use super::{Plan, Sweep, Task, instruction, snapshot};

/// What one tick did, in the words the command prints and the log keeps.
#[derive(Debug, Default)]
pub struct Done {
    pub started: Vec<String>,
    pub ended: Vec<String>,
    pub notes: Vec<String>,
}

/// The sessions this dispatcher started and has not collected yet. It
/// holds them so their exit code is known: a session that exited 0 is
/// done, and one that exited with anything else failed and is worth one
/// more try. Without this, every session that simply finished would look
/// like a session whose process disappeared, and the work would be
/// dispatched a second time.
#[derive(Default)]
pub struct Sessions {
    children: BTreeMap<String, Child>,
}

impl Sessions {
    pub fn new() -> Sessions {
        Sessions::default()
    }

    fn keep(&mut self, id: &str, child: Child) {
        self.children.insert(id.to_string(), child);
    }

    /// How the session ended, if this dispatcher started it and it has
    /// ended. `None` means "still running" or "not ours".
    fn outcome_of(&mut self, id: &str) -> Option<Outcome> {
        let child = self.children.get_mut(id)?;
        match child.try_wait() {
            Ok(Some(status)) if status.success() => Some(Outcome::Done),
            Ok(Some(_)) => Some(Outcome::Failed),
            Ok(None) => None,
            Err(_) => None,
        }
    }

    /// Closes every session of this dispatcher's that has exited, and
    /// collects it. This has to happen on every tick, not only when a
    /// session looks gone: a child nobody waits for stays a zombie, and
    /// `kill -0` says a zombie is alive, so a session that finished
    /// minutes ago would hold its slot until it timed out. Seen on the
    /// real run: the agent had already written its report and exited,
    /// the dispatcher kept saying "1 个候选被跳过".
    fn reap(&mut self, state: &mut State, config: &Config, now: DateTime<Utc>) -> Vec<String> {
        let ended: Vec<(String, Outcome)> = self
            .children
            .iter_mut()
            .filter_map(|(id, child)| match child.try_wait() {
                Ok(Some(status)) if status.success() => Some((id.clone(), Outcome::Done)),
                Ok(Some(_)) => Some((id.clone(), Outcome::Failed)),
                _ => None,
            })
            .collect();
        let mut notes = Vec::new();
        for (id, outcome) in ended {
            self.children.remove(&id);
            let Some(session) = state.session_mut(&id) else {
                continue;
            };
            if session.finished_at.is_some() {
                continue;
            }
            let minutes = (now - session.started_at).num_minutes().max(0);
            session.finished_at = Some(now);
            session.outcome = Some(outcome);
            notes.push(format!("{id} {}（{minutes} 分钟）", outcome.name()));
        }
        if !notes.is_empty()
            && let Err(e) = state.write(&config.state)
        {
            notes.push(format!("状态写不下去：{e}"));
        }
        notes
    }

    fn forget(&mut self, id: &str) {
        self.children.remove(id);
    }
}

/// Carries out a plan: tidies up the sessions that ended, then starts the
/// ones the plan asks for. The state is written after every change, so a
/// dispatcher killed between two sessions still knows what it started.
pub fn run(
    plan: &Plan,
    state: &mut State,
    config: &Config,
    now: DateTime<Utc>,
    sessions: &mut Sessions,
) -> Done {
    let mut done = Done::default();
    // Collect what has already exited before acting on the plan: a
    // session closed here frees its slot on the next tick.
    done.ended.extend(sessions.reap(state, config, now));
    for sweep in &plan.sweep {
        // The exit code, when this dispatcher is the one that started it,
        // says more than "the process is gone".
        let sweep = &match sessions.outcome_of(&sweep.id) {
            Some(outcome) => Sweep {
                outcome,
                release_claim: sweep.release_claim && outcome != Outcome::Done,
                ..sweep.clone()
            },
            None => sweep.clone(),
        };
        sessions.forget(&sweep.id);
        match finish(sweep, state, config, now) {
            Ok(note) => done.ended.push(note),
            Err(why) => done.notes.push(why),
        }
        if let Err(why) = state.write(&config.state) {
            done.notes.push(why);
        }
    }
    for task in &plan.start {
        match start(task, config, now) {
            Ok((session, child)) => {
                sessions.keep(&session.id, child);
                done.started.push(format!(
                    "{} → {}（{}）",
                    session.id, session.vendor, task.why
                ));
                state.sessions.push(session);
                if let super::Role::Scheduled(name) = &task.role {
                    state
                        .scheduled_ran
                        .insert(name.clone(), super::state::local_day(now));
                }
            }
            Err(why) => done.notes.push(format!("{} 没能启动：{why}", task.id)),
        }
        if let Err(why) = state.write(&config.state) {
            done.notes.push(why);
        }
    }
    for (task, skip) in &plan.skip {
        if !skip.worth_explaining() || state.explained.contains(&task.id) {
            continue;
        }
        let Some(number) = task.subject() else {
            continue;
        };
        let body = format!(
            "## 调度程序\n\n- 要派的活: {}（{}）\n- 现在不派: {}\n- 怎么办: 等正在跑的会话结束，或者在设置里提高 `max_sessions_per_day`。调度程序每 {} 秒再看一次，不会重复发这条。\n\nAgent: dispatcher\n",
            task.role.name(),
            task.why,
            skip.why(),
            config.interval_seconds,
        );
        match snapshot::comment(&config.gh_command, &config.repo, number, &body) {
            Ok(()) => {
                state.explained.push(task.id.clone());
                done.notes.push(format!("#{number} 上说明了为什么排队"));
            }
            Err(e) => done.notes.push(format!("#{number} 上说明排队失败：{e}")),
        }
        if let Err(why) = state.write(&config.state) {
            done.notes.push(why);
        }
    }
    done
}

/// Starts one session: its own worktree, its own log file, its own
/// process group. Nothing is shared with another session, because two
/// sessions in one checkout commit each other's staged files (AGENTS.md).
pub fn start(task: &Task, config: &Config, now: DateTime<Utc>) -> Result<(Session, Child), String> {
    let agent = config
        .agents
        .get(&task.vendor)
        .ok_or_else(|| format!("没配 {} 的命令行", task.vendor))?;
    let worktree = worktree_for(task, config)?;
    std::fs::create_dir_all(&config.logs)
        .map_err(|e| format!("建 {} 失败：{e}", config.logs.display()))?;
    let log = config.logs.join(format!("{}.log", task.id));
    let out = File::create(&log).map_err(|e| format!("建 {} 失败：{e}", log.display()))?;
    let errors = out
        .try_clone()
        .map_err(|e| format!("建 {} 失败：{e}", log.display()))?;

    let prompt = instruction(task, config, &worktree);
    let (program, args) = agent
        .command
        .split_first()
        .expect("设置检查过命令行不是空的");
    let mut command = Command::new(program);
    command.args(args);
    if agent.prompt == PromptStyle::Argv {
        command.arg(&prompt);
    }
    command
        .current_dir(&worktree)
        .env("THEFLASH_DISPATCH_TASK", &task.id)
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(errors))
        // Its own process group, so stopping a session stops the tools it
        // started as well, and a Ctrl-C in the dispatcher's terminal does
        // not reach a session that should keep running.
        .process_group(0);
    match agent.prompt {
        PromptStyle::Argv => command.stdin(Stdio::null()),
        PromptStyle::Stdin => command.stdin(Stdio::piped()),
    };
    let mut child = command
        .spawn()
        .map_err(|e| format!("启动 {program} 失败：{e}"))?;
    if agent.prompt == PromptStyle::Stdin
        && let Some(mut stdin) = child.stdin.take()
    {
        use std::io::Write;
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| format!("把任务写给 {program} 失败：{e}"))?;
    }
    let session = Session {
        id: task.id.clone(),
        role: task.role.name().to_string(),
        vendor: task.vendor.clone(),
        issue: task.issue,
        pull: task.pull,
        started_at: now,
        finished_at: None,
        outcome: None,
        pid: Some(child.id()),
        log,
        worktree,
    };
    Ok((session, child))
}

/// The worktree a session works in: one per task id, checked out from
/// `origin/main`. An existing directory is reused, which is what a second
/// attempt at the same task wants.
fn worktree_for(task: &Task, config: &Config) -> Result<PathBuf, String> {
    let dir = config.worktrees.join(&task.id);
    if dir.exists() {
        return Ok(dir);
    }
    std::fs::create_dir_all(&config.worktrees)
        .map_err(|e| format!("建 {} 失败：{e}", config.worktrees.display()))?;
    // A fetch that fails (the network is down, there is no remote) does
    // not stop the session: the worktree is made from the origin/main the
    // checkout already has, and the session fetches again itself.
    let _ = git(&config.checkout, &["fetch", "--quiet", "origin"]);
    git(
        &config.checkout,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            &dir.display().to_string(),
            "origin/main",
        ],
    )?;
    Ok(dir)
}

fn git(checkout: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .output()
        .map_err(|e| format!("git {} 失败：{e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} 失败：{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Closes out one session that is no longer running. A session that timed
/// out is stopped, its claim released and its issue told: nobody else is
/// watching for it, and a lease left in place blocks every other agent
/// for two hours.
fn finish(
    sweep: &Sweep,
    state: &mut State,
    config: &Config,
    now: DateTime<Utc>,
) -> Result<String, String> {
    let mut note = format!(
        "{} {}（{} 分钟）",
        sweep.id,
        sweep.outcome.name(),
        sweep.after_minutes
    );
    let pid = state.session(&sweep.id).and_then(|s| s.pid);
    if sweep.outcome == Outcome::TimedOut
        && let Some(pid) = pid
    {
        stop(pid);
    }
    if let Some(session) = state.session_mut(&sweep.id) {
        session.finished_at = Some(now);
        session.outcome = Some(sweep.outcome);
    }
    if !sweep.release_claim {
        return Ok(note);
    }
    let Some(issue) = sweep.issue else {
        return Ok(note);
    };
    let claims = release_claim(config, issue, &sweep.id);
    let body = format!(
        "## 调度程序\n\n- 会话: {}（{} 分钟后超时，已经停掉）\n- 日志: {}\n- 认领: {}\n- 下一步: 这个 issue 现在没有会话在做。接手的 agent 先看日志和分支做到哪一步，再按 AGENTS.md 认领，不要重复已经完成的步骤。\n\nAgent: dispatcher\n",
        sweep.id,
        sweep.after_minutes,
        sweep.log.display(),
        claims,
    );
    match snapshot::comment(&config.gh_command, &config.repo, issue, &body) {
        Ok(()) => note.push_str(&format!("，已在 #{issue} 上说明（{claims}）")),
        Err(e) => note.push_str(&format!("，#{issue} 上说明失败：{e}")),
    }
    Ok(note)
}

/// Asks `scripts/claims` to release the lease. It refuses a lease that is
/// not expired yet, which is the right answer and goes into the comment
/// as it is: the next agent then follows the lease rules instead of the
/// dispatcher deciding for it.
fn release_claim(config: &Config, issue: u64, task: &str) -> String {
    let Some((program, args)) = config.claims_command.split_first() else {
        return "没配 claims_command，认领没有被收回".to_string();
    };
    let out = Command::new(program)
        .args(args)
        .args([
            "reclaim",
            "--issue",
            &issue.to_string(),
            "--agent",
            "dispatcher",
        ])
        .current_dir(&config.checkout)
        .env("THEFLASH_DISPATCH_TASK", task)
        .output();
    match out {
        Ok(out) if out.status.success() => "已经收回（scripts/claims reclaim）".to_string(),
        Ok(out) => format!(
            "scripts/claims reclaim 拒绝收回（退出码 {}）：{}。租约还没过期时这是对的，等它过期后任何 agent 都可以收回。",
            out.status.code().unwrap_or(-1),
            first_line(
                &String::from_utf8_lossy(&out.stdout),
                &String::from_utf8_lossy(&out.stderr)
            ),
        ),
        Err(e) => format!("跑不起 {program}：{e}，认领没有被收回"),
    }
}

fn first_line(stdout: &str, stderr: &str) -> String {
    let text = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("没有输出")
        .to_string()
}

/// Stops a session's whole process group, so the tools it started stop
/// too. The dispatcher gave every session its own group, and the group
/// is `-<pid>`. The `--` is what makes that portable: Linux's `kill`
/// (procps) reads `-1234` as options and refuses, macOS's takes it as a
/// pid either way, so without `--` a timed-out session on Linux is
/// forgotten but keeps running.
fn stop(pid: u32) {
    let _ = Command::new(super::state::kill_bin())
        .args(["-TERM", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
