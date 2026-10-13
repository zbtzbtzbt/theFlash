//! `scripts/dispatch` 的每条路径，用保存下来的真实 GitHub 响应回放。
//!
//! No test here reaches GitHub, starts a real agent or touches this
//! machine's services: `support::dispatch` gives the command a PATH with
//! only the stubs the test put there, a HOME of its own, and the fixtures
//! in `testdata/github/dispatch` as its view of GitHub.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use support::{dispatch, output_within, real_tool, stub};

/// The fixtures: what GitHub answered on 2026-10-14, by scenario.
fn fixtures(name: &str) -> String {
    format!(
        "{}/testdata/github/dispatch/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// A temporary HOME for one test, removed when the test ends.
struct Home {
    dir: PathBuf,
}

impl Home {
    fn new(name: &str) -> Home {
        let dir =
            std::env::temp_dir().join(format!("theflash-dispatch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        // The checkout the settings point at: `scripts/claims` runs in it,
        // and only the tests that start a session put a git repository
        // there.
        std::fs::create_dir_all(dir.join("checkout")).unwrap();
        Home { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    /// Settings for this test: the given fields merged into settings that
    /// cannot touch anything real (no agent but a stub, paths under the
    /// temporary HOME).
    fn config(&self, extra: serde_json::Value) -> PathBuf {
        let mut config = serde_json::json!({
            "repo": "zbtzbtzbt/theFlash",
            "checkout": self.dir.join("checkout"),
            "worktrees": self.dir.join("sessions"),
            "logs": self.dir.join("logs"),
            "state": self.dir.join("state"),
            "max_sessions": 9,
            "max_sessions_per_day": 20,
            "cloud_roles": [],
            "claims_command": ["claims"],
            "gh_command": ["gh"],
            "vendor_order": ["claude", "codex", "codebuddy"],
            "agents": {
                "claude": { "command": ["agent-claude"], "prompt": "argv" },
                "codex": { "command": ["agent-codex", "-"], "prompt": "stdin" },
                "codebuddy": { "command": ["agent-codebuddy"], "prompt": "argv" },
            },
        });
        let serde_json::Value::Object(extra) = extra else {
            panic!("extra 要是一个对象");
        };
        for (key, value) in extra {
            config[key] = value;
        }
        let path = self.dir.join("dispatch.json");
        std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        path
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `dispatch <args>` with a temporary HOME, no network and no real agent.
fn run(home: &Home, args: &[&str]) -> (i32, String) {
    let mut cmd = dispatch(home.path());
    cmd.args(args);
    let out = output_within(cmd, 60);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap_or(-1), text)
}

fn line_for(output: &str, id: &str) -> String {
    output
        .lines()
        .find(|l| l.contains(id))
        .unwrap_or_else(|| panic!("输出里没有 {id}：\n{output}"))
        .to_string()
}

// ---- which event goes to which role and vendor ----

#[test]
fn tonights_github_sends_every_event_to_the_role_that_handles_it() {
    let home = Home::new("roles");
    let config = home.config(serde_json::json!({ "ignore_issues": [20] }));
    let (code, out) = run(
        &home,
        &[
            "plan",
            "--config",
            &config.display().to_string(),
            "--replay",
            &fixtures("night"),
            "--now",
            "2026-10-14T04:00:00+08:00",
        ],
    );
    assert_eq!(code, 0, "{out}");
    // PR #38 and #36 are CodeBuddy's, #35 is Codex's, #33 is Claude's:
    // each review goes to someone else.
    assert!(
        !line_for(&out, "review-38-a1").contains("codebuddy"),
        "{out}"
    );
    assert!(!line_for(&out, "review-35-a1").contains("codex"), "{out}");
    assert!(!line_for(&out, "review-33-a1").contains("claude"), "{out}");
    // PR #24 is Claude's and was sent back for changes, so Claude fixes it.
    assert!(line_for(&out, "fix-pr-24-a1").contains("claude"), "{out}");
    // #37 has no label and nobody on it: fresh work.
    assert!(line_for(&out, "implement-37-a1").contains("派"), "{out}");
    // #16 and #14 are claimed, #20 is the coordination issue.
    for untouched in ["-16-", "-14-", "-20-"] {
        assert!(
            !out.contains(untouched),
            "认领中的和被忽略的 issue 不该出现：{untouched}\n{out}"
        );
    }
}

#[test]
fn acceptance_goes_to_another_vendor_than_the_one_that_implemented_it() {
    // #22's real comments: CodeBuddy claimed it, implemented it and
    // reported; Codex then accepted it and signed last. The implementer is
    // the one who claimed it, not the one who spoke last — otherwise the
    // next round goes back to the agent that just reviewed it.
    //
    // Claude already ran a session today, so the two readings pick
    // different vendors: with CodeBuddy as the implementer the task goes
    // to Codex (the idle one of the other two), and with Codex as the
    // implementer it would go to CodeBuddy.
    let home = Home::new("accept");
    let state_dir = home.path().join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("state.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "sessions": [{
                "id": "implement-37-a1",
                "role": "implement",
                "vendor": "claude",
                "issue": 37,
                "started_at": "2026-10-14T01:00:00+08:00",
                "finished_at": "2026-10-14T01:40:00+08:00",
                "outcome": "done",
                "log": home.path().join("logs/implement-37-a1.log"),
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let config = home.config(serde_json::json!({}));
    let (code, out) = run(
        &home,
        &[
            "plan",
            "--json",
            "--config",
            &config.display().to_string(),
            "--replay",
            &fixtures("accept"),
            "--now",
            "2026-10-14T04:00:00+08:00",
        ],
    );
    assert_eq!(code, 0, "{out}");
    let plan: serde_json::Value = serde_json::from_str(out.trim()).expect("--json 要是 JSON");
    assert_eq!(plan["start"][0]["id"], "accept-22-a1", "{plan}");
    assert_eq!(
        plan["start"][0]["vendor"], "codex",
        "实现者是认领的 CodeBuddy，不是最后签名的 Codex：{plan}"
    );
}

#[test]
fn a_cancelled_run_is_not_a_red_main_but_a_failed_one_is() {
    let home = Home::new("ci");
    let config = home.config(serde_json::json!({}));
    let args = |dir: &str| {
        vec![
            "plan".to_string(),
            "--config".to_string(),
            config.display().to_string(),
            "--replay".to_string(),
            fixtures(dir),
            "--now".to_string(),
            "2026-10-14T04:00:00+08:00".to_string(),
        ]
    };
    fn borrow(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }

    let red = args("red-main");
    let (_, out) = run(&home, &borrow(&red));
    assert!(out.contains("fix-ci-a1"), "失败的检查要派修红：{out}");

    // macos-latest cancelled after never getting a runner, ubuntu green:
    // GitHub capacity, not a failing test, so nothing is dispatched.
    let cancelled = args("cancelled-main");
    let (code, out) = run(&home, &borrow(&cancelled));
    assert!(!out.contains("fix-ci"), "被取消的 run 不是红的：{out}");
    assert_eq!(code, 1, "没有要派的活时退出码是 1：{out}");
}

#[test]
fn forgetting_the_replay_directory_fails_instead_of_asking_the_real_github() {
    let home = Home::new("nogh");
    let config = home.config(serde_json::json!({}));
    let (code, out) = run(&home, &["plan", "--config", &config.display().to_string()]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("问 GitHub 要"), "{out}");
}

// ---- starting a session ----

/// A git checkout with an `origin/main` to make worktrees from, and no
/// remote: the dispatcher must not need the network to start a session.
fn checkout(home: &Home) {
    let dir = home.path().join("checkout");
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .env("HOME", home.path())
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(dir.join("AGENTS.md"), "规则\n").unwrap();
    git(&["add", "AGENTS.md"]);
    git(&["commit", "--quiet", "-m", "first"]);
    git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
}

#[test]
fn a_session_gets_its_own_worktree_its_own_log_and_the_task_in_its_prompt() {
    let home = Home::new("start");
    checkout(&home);
    real_tool(home.path(), "git");
    // The stub stands in for an agent command line: it writes down what it
    // was told to do and what it was started in, then exits.
    stub(
        home.path(),
        "agent-claude",
        "echo \"prompt: $1\"; echo \"cwd: $PWD\"; echo \"task: $THEFLASH_DISPATCH_TASK\"",
    );
    let config = home.config(serde_json::json!({ "max_sessions": 1, "ignore_issues": [20] }));
    let (code, out) = run(
        &home,
        &[
            "tick",
            "--config",
            &config.display().to_string(),
            "--replay",
            &fixtures("accept"),
            "--now",
            "2026-10-14T04:00:00+08:00",
        ],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("启动 accept-22-a1"), "{out}");

    // The session's log says what it was asked to do, in its own worktree.
    let log = home.path().join("logs/accept-22-a1.log");
    let text = wait_for_log(&log);
    assert!(text.contains("prompts/acceptance.md"), "{text}");
    assert!(text.contains("#22"), "{text}");
    assert!(
        text.contains(
            &home
                .path()
                .join("sessions/accept-22-a1")
                .display()
                .to_string()
        ),
        "会话在自己的 worktree 里：{text}"
    );
    assert!(text.contains("task: accept-22-a1"), "{text}");
    assert!(
        home.path().join("sessions/accept-22-a1/AGENTS.md").exists(),
        "worktree 要是 origin/main 的 checkout"
    );

    // And the dispatcher remembers it, so the next tick does not start it
    // again.
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.path().join("state/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["sessions"][0]["id"], "accept-22-a1");
    assert_eq!(state["sessions"][0]["role"], "accept");

    let (_, status) = run(
        &home,
        &[
            "status",
            "--config",
            &config.display().to_string(),
            "--now",
            "2026-10-14T04:05:00+08:00",
        ],
    );
    assert!(status.contains("accept-22-a1"), "{status}");

    let (code, again) = run(
        &home,
        &[
            "plan",
            "--config",
            &config.display().to_string(),
            "--replay",
            &fixtures("accept"),
            "--now",
            "2026-10-14T04:05:00+08:00",
        ],
    );
    assert_eq!(code, 1, "没有新活要派：{again}");
    assert!(again.contains("accept-22-a1"), "{again}");
}

/// The session is a process of its own, so its log appears a moment after
/// the dispatcher returns.
fn wait_for_log(path: &Path) -> String {
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(path)
            && text.contains("task:")
        {
            return text;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("{} 里没有会话的输出", path.display())
}

#[test]
fn a_session_that_ran_too_long_is_stopped_the_claim_released_and_the_issue_told() {
    use std::os::unix::process::CommandExt;

    let home = Home::new("timeout");
    // A process standing in for a session that will not end on its own,
    // in its own process group, exactly as the dispatcher starts one.
    let mut child = Command::new("sleep")
        .arg("600")
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id();
    let state_dir = home.path().join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("state.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "sessions": [{
                "id": "accept-22-a1",
                "role": "accept",
                "vendor": "codex",
                "issue": 22,
                "started_at": "2026-10-14T01:00:00+08:00",
                "pid": pid,
                "log": home.path().join("logs/accept-22-a1.log"),
                "worktree": home.path().join("sessions/accept-22-a1"),
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    // `gh` and `scripts/claims` as stubs: the test reads what the
    // dispatcher asked them to do.
    real_tool(home.path(), "cat");
    stub(
        home.path(),
        "gh",
        &format!(
            "echo \"$@\" >> {calls}; cat \"$7\" >> {body}",
            calls = home.path().join("gh-calls").display(),
            body = home.path().join("gh-body").display(),
        ),
    );
    stub(
        home.path(),
        "claims",
        &format!(
            "echo \"$@\" >> {calls}; echo '拒绝：#22 的认领还没过期，还差 44 分钟'; exit 1",
            calls = home.path().join("claims-calls").display(),
        ),
    );
    let config = home.config(serde_json::json!({ "session_timeout_minutes": 120 }));
    let (_, out) = run(
        &home,
        &[
            "tick",
            "--config",
            &config.display().to_string(),
            "--replay",
            &fixtures("red-main"),
            "--now",
            "2026-10-14T03:30:00+08:00",
        ],
    );
    assert!(out.contains("收尾 accept-22-a1 超时（150 分钟）"), "{out}");

    // The process is gone, not just forgotten.
    let mut stopped = false;
    for _ in 0..100 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            stopped = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(stopped, "超时的会话进程要被停掉");

    let claims = std::fs::read_to_string(home.path().join("claims-calls")).unwrap();
    assert!(claims.contains("reclaim --issue 22"), "{claims}");
    let body = std::fs::read_to_string(home.path().join("gh-body")).unwrap();
    assert!(body.contains("accept-22-a1"), "{body}");
    assert!(body.contains("150 分钟后超时"), "{body}");
    assert!(
        body.contains("还没过期"),
        "claims 的拒绝要原样写进 issue：{body}"
    );
    let calls = std::fs::read_to_string(home.path().join("gh-calls")).unwrap();
    assert!(calls.contains("issue comment 22"), "{calls}");

    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state_dir.join("state.json")).unwrap())
            .unwrap();
    assert_eq!(state["sessions"][0]["outcome"], "timed-out");
}

#[test]
fn the_daily_ceiling_is_explained_on_the_issue_once() {
    let home = Home::new("cap");
    real_tool(home.path(), "cat");
    stub(
        home.path(),
        "gh",
        &format!(
            "echo \"$@\" >> {calls}; cat \"$7\" >> {body}",
            calls = home.path().join("gh-calls").display(),
            body = home.path().join("gh-body").display(),
        ),
    );
    let config = home.config(serde_json::json!({
        "max_sessions_per_day": 0,
        "ignore_issues": [20],
    }));
    let args = [
        "tick",
        "--config",
        &config.display().to_string(),
        "--replay",
        &fixtures("accept"),
        "--now",
        "2026-10-14T04:00:00+08:00",
    ];
    let (code, out) = run(&home, &args);
    assert_eq!(code, 1, "到上限了什么都不派：{out}");
    let body = std::fs::read_to_string(home.path().join("gh-body")).unwrap();
    assert!(body.contains("到上限了"), "{body}");
    assert!(body.contains("max_sessions_per_day"), "{body}");

    let (_, _) = run(&home, &args);
    let body = std::fs::read_to_string(home.path().join("gh-body")).unwrap();
    assert_eq!(
        body.matches("到上限了").count(),
        1,
        "同一个活只说明一次：{body}"
    );
}

#[test]
fn two_dispatchers_do_not_run_at_once() {
    let home = Home::new("lock");
    let mut child = Command::new("sleep").arg("600").spawn().unwrap();
    let state_dir = home.path().join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("dispatch.pid"), child.id().to_string()).unwrap();
    let config = home.config(serde_json::json!({}));
    let (code, out) = run(
        &home,
        &[
            "tick",
            "--config",
            &config.display().to_string(),
            "--replay",
            &fixtures("accept"),
        ],
    );
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("另一个调度程序正在跑"), "{out}");
}

// ---- install, uninstall, status ----

#[test]
fn install_prints_the_service_file_without_touching_this_machine() {
    let home = Home::new("install");
    let config = home.config(serde_json::json!({}));
    let (code, out) = run(
        &home,
        &[
            "install",
            "--print",
            "--config",
            &config.display().to_string(),
        ],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("cn.theflash.dispatch"), "{out}");
    assert!(out.contains("run"), "{out}");
    // The service runs a copy outside the repository: `cargo build`
    // replaces the one in `target/`, and on macOS a launchd agent loading
    // a binary from under ~/Desktop hangs in the dynamic loader before
    // main runs, with an empty log.
    assert!(
        out.contains(".local/libexec/theflash-dispatch"),
        "服务要跑仓库外的那份程序：{out}"
    );
    assert!(
        !out.contains("target/debug") && !out.contains("target/release"),
        "服务不许指向仓库里的 target/：{out}"
    );
    assert!(out.contains(&config.display().to_string()), "{out}");
    assert!(
        out.contains(
            &home
                .path()
                .join("logs/dispatch.err.log")
                .display()
                .to_string()
        ),
        "{out}"
    );
    assert!(
        !home.path().join("Library/LaunchAgents").exists()
            && !home.path().join(".config/systemd").exists(),
        "--print 不许动这台机器的服务"
    );
}

#[test]
fn the_example_settings_are_settings_the_dispatcher_accepts() {
    let home = Home::new("example");
    let (code, out) = run(&home, &["example-config"]);
    assert_eq!(code, 0, "{out}");
    let json: serde_json::Value =
        serde_json::from_str(out.split("\n#").next().unwrap()).expect("要是合法的 JSON");
    assert_eq!(json["max_sessions"], 1, "一开始只许一个会话：{json}");
    assert_eq!(json["require_label"], "演示:调度", "{json}");
    assert_eq!(
        json["cloud_roles"],
        serde_json::json!(["review", "fix-ci", "nightly-review"]),
        "默认按 #9 的裁决把不碰大麦的活交给云端：{json}"
    );
    // 存下来就能直接用。
    let path = home.path().join("example.json");
    std::fs::write(&path, out.split("\n#").next().unwrap()).unwrap();
    let (code, out) = run(
        &home,
        &[
            "plan",
            "--config",
            &path.display().to_string(),
            "--replay",
            &fixtures("night"),
            "--now",
            "2026-10-14T04:00:00+08:00",
        ],
    );
    assert_eq!(code, 1, "演示标签还没人用，所以没有活：{out}");
    assert!(out.contains("没有要派的活"), "{out}");
}

#[test]
fn settings_that_would_send_work_nowhere_are_refused_at_start() {
    let home = Home::new("badconfig");
    let path = home.path().join("broken.json");
    std::fs::write(&path, "{\"agents\": {}}").unwrap();
    let (code, out) = run(&home, &["plan", "--config", &path.display().to_string()]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("agents 是空的"), "{out}");

    let missing = home.path().join("nope.json");
    let (code, out) = run(&home, &["plan", "--config", &missing.display().to_string()]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("example-config"), "说清怎么弄一份设置：{out}");
}
