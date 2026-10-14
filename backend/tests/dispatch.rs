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
    assert!(!line_for(&out, "review-38-").contains("codebuddy"), "{out}");
    assert!(!line_for(&out, "review-35-").contains("codex"), "{out}");
    assert!(!line_for(&out, "review-33-").contains("claude"), "{out}");
    // PR #24 is Claude's and was sent back for changes, so Claude fixes it.
    assert!(line_for(&out, "fix-pr-24-").contains("claude"), "{out}");
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
fn work_sent_back_by_acceptance_goes_to_the_agent_that_wrote_it() {
    // #22 的真实评论加上验收者按租约格式留的认领评论。租约说的是“现在谁在
    // 干活”，改写不了“是谁实现的”：#22 是 CodeBuddy 实现的，Codex 只是验收。
    let home = Home::new("backtoauthor");
    let config = home.config(serde_json::json!({}));
    let (code, out) = run(
        &home,
        &[
            "plan",
            "--json",
            "--config",
            &config.display().to_string(),
            "--replay",
            &fixtures("fix-issue"),
            "--now",
            "2026-10-14T04:00:00+08:00",
        ],
    );
    assert_eq!(code, 0, "{out}");
    let plan: serde_json::Value = serde_json::from_str(out.trim()).expect("--json 要是 JSON");
    assert_eq!(plan["start"][0]["role"], "fix-issue", "{plan}");
    assert_eq!(
        plan["start"][0]["vendor"], "codebuddy",
        "验收未通过要回到实现它的 CodeBuddy，不是最后认领的验收者 Codex：{plan}"
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
    // The task is named after the run that failed, so the next failing run
    // is a new task instead of one that was already dispatched.
    assert!(
        out.contains("fix-ci-38456076570-a1"),
        "失败的检查要派修红，活的编号带着那个 run：{out}"
    );

    // macos-latest cancelled after never getting a runner, ubuntu green:
    // GitHub capacity, not a failing test, so nothing is dispatched.
    let cancelled = args("cancelled-main");
    let (code, out) = run(&home, &borrow(&cancelled));
    assert!(!out.contains("fix-ci"), "被取消的 run 不是红的：{out}");
    assert_eq!(code, 1, "没有要派的活时退出码是 1：{out}");
}

/// A `gh` that answers out of the saved responses in `dir`, one page per
/// request unless the test asks for more. `gh api --paginate` prints one
/// independent JSON document per page, so this is what the dispatcher has
/// to read once any issue passes 100 comments.
fn gh_serving(home: &Home, dir: &str, extra_page: &str) {
    real_tool(home.path(), "cat");
    stub(
        home.path(),
        "gh",
        &format!(
            "case \"$*\" in\n\
             *comments*) cat '{dir}/comments-22.json'; {extra_page} ;;\n\
             *issues*) cat '{dir}/issues.json' ;;\n\
             *pulls*) cat '{dir}/pulls.json' ;;\n\
             *runs*) cat '{dir}/main-test-runs.json' ;;\n\
             *) echo '[]' ;;\n\
             esac"
        ),
    );
}

#[test]
fn comments_that_arrive_in_pages_are_all_read() {
    let home = Home::new("pages 空格");
    let spaced = home.path().join("theFlash-验收 空格/accept");
    std::fs::create_dir_all(&spaced).unwrap();
    for entry in std::fs::read_dir(fixtures("accept")).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::copy(&path, spaced.join(path.file_name().unwrap())).unwrap();
        }
    }
    let config = home.config(serde_json::json!({}));
    for dir in [fixtures("accept"), spaced.display().to_string()] {
        gh_serving(&home, &dir, "echo '[]'");
        let (code, out) = run(
            &home,
            &[
                "plan",
                "--json",
                "--config",
                &config.display().to_string(),
                "--now",
                "2026-10-14T04:00:00+08:00",
            ],
        );
        assert_eq!(code, 0, "分页的评论要读完，不是整轮失败（{dir}）：{out}");
        let plan: serde_json::Value = serde_json::from_str(out.trim()).expect("--json 要是 JSON");
        assert_eq!(plan["start"][0]["role"], "accept", "{plan}");
        assert_ne!(
            plan["start"][0]["vendor"], "codebuddy",
            "第一页里的认领评论仍然决定实现者：{plan}"
        );
    }
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
fn a_session_that_finished_is_closed_on_the_next_tick() {
    use std::os::unix::process::CommandExt;

    // A child nobody waits for becomes a zombie, and `kill -0` calls a
    // zombie alive. Without collecting the children, a session that
    // finished in a minute would hold its slot until it timed out.
    let home = Home::new("reap");
    checkout(&home);
    real_tool(home.path(), "git");
    stub(home.path(), "agent-claude", "echo '做完了'");
    let config = home.config(serde_json::json!({
        "interval_seconds": 1,
        "max_sessions": 1,
        "ignore_issues": [20],
    }));

    let mut resident = dispatch(home.path())
        .args([
            "run",
            "--config",
            &config.display().to_string(),
            "--replay",
            &fixtures("accept"),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();

    let state_file = home.path().join("state/state.json");
    let mut outcome = serde_json::Value::Null;
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(&state_file)
            && let Ok(state) = serde_json::from_str::<serde_json::Value>(&text)
            && !state["sessions"][0]["outcome"].is_null()
        {
            outcome = state["sessions"][0]["outcome"].clone();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = resident.kill();
    let _ = resident.wait();

    assert_eq!(
        outcome, "done",
        "会话的进程结束了，调度程序就该把它收掉，不要占着名额"
    );
}

#[test]
fn a_failed_session_releases_its_claim_and_tells_the_issue() {
    use std::os::unix::process::CommandExt;

    let home = Home::new("failed-cleanup 空格");
    checkout(&home);
    real_tool(home.path(), "git");
    real_tool(home.path(), "cat");
    stub(home.path(), "agent-claude", "exit 1");
    stub(
        home.path(),
        "claims",
        &format!(
            "echo \"$@\" >> '{}'; echo '已经收回'",
            home.path().join("claims-calls").display()
        ),
    );
    stub(
        home.path(),
        "gh",
        &format!(
            "echo \"$@\" >> '{calls}'; cat \"$7\" >> '{body}'",
            calls = home.path().join("gh-calls").display(),
            body = home.path().join("gh-body").display(),
        ),
    );
    let config = home.config(serde_json::json!({
        "interval_seconds": 1,
        "max_sessions": 1,
        "ignore_issues": [20],
    }));
    let mut resident = dispatch(home.path())
        .args([
            "run",
            "--config",
            &config.display().to_string(),
            "--replay",
            &fixtures("accept"),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();

    let state_file = home.path().join("state/state.json");
    let mut cleaned_up = false;
    for _ in 0..150 {
        let failed = std::fs::read_to_string(&state_file)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .is_some_and(|state| state["sessions"][0]["outcome"] == "failed");
        if failed
            && home.path().join("claims-calls").exists()
            && home.path().join("gh-body").exists()
        {
            cleaned_up = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = resident.kill();
    let _ = resident.wait();

    assert!(cleaned_up, "失败会话必须走认领释放协议并通知 issue");
    let claims = std::fs::read_to_string(home.path().join("claims-calls")).unwrap();
    assert!(claims.contains("reclaim --issue 22"), "{claims}");
    let body = std::fs::read_to_string(home.path().join("gh-body")).unwrap();
    assert!(body.contains("失败"), "{body}");
    assert!(body.contains("accept-22-a1"), "{body}");
}

#[test]
fn the_resident_loop_sees_the_time_pass_so_a_session_can_time_out() {
    use std::os::unix::process::CommandExt;

    // 常驻循环每轮都要用这一轮的时间。按启动时刻算，会话跑多久都不到超时，
    // 跨日的额度和之后到点的定时任务也一样永远不发生。
    let home = Home::new("clock");
    let mut child = Command::new("sleep")
        .arg("600")
        .process_group(0)
        .spawn()
        .unwrap();
    // chrono 在这个项目里没开 `clock`，时间从系统时钟的时间戳来，和调度程序用的
    // 是同一个函数。
    let started = theflash::dispatch::now() - chrono::Duration::seconds(55);
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
                "started_at": started.to_rfc3339(),
                "pid": child.id(),
                "log": home.path().join("logs/accept-22-a1.log"),
                "worktree": home.path().join("sessions/accept-22-a1"),
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    real_tool(home.path(), "cat");
    stub(home.path(), "gh", "cat \"$7\" > /dev/null");
    stub(home.path(), "claims", "echo '已经收回'");
    // 没有活可派（演示标签没人带），这一轮只会做收尾。
    let config = home.config(serde_json::json!({
        "interval_seconds": 1,
        "session_timeout_minutes": 1,
        "require_label": "演示:调度",
    }));
    let mut resident = dispatch(home.path())
        .args([
            "run",
            "--config",
            &config.display().to_string(),
            "--replay",
            &fixtures("cancelled-main"),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();

    let mut outcome = serde_json::Value::Null;
    for _ in 0..400 {
        if let Ok(text) = std::fs::read_to_string(state_dir.join("state.json"))
            && let Ok(state) = serde_json::from_str::<serde_json::Value>(&text)
            && !state["sessions"][0]["outcome"].is_null()
        {
            outcome = state["sessions"][0]["outcome"].clone();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = resident.kill();
    let _ = resident.wait();
    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(
        outcome, "timed-out",
        "跑过超时上限的会话，下一轮就要收尾，而不是按调度程序的启动时刻算"
    );
}

#[test]
fn a_session_that_ran_too_long_is_stopped_the_claim_released_and_the_issue_told() {
    use std::os::unix::process::CommandExt;

    let home = Home::new("timeout 空格");
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
            "echo \"$@\" >> '{calls}'; cat \"$7\" >> '{body}'",
            calls = home.path().join("gh-calls").display(),
            body = home.path().join("gh-body").display(),
        ),
    );
    stub(
        home.path(),
        "claims",
        &format!(
            "echo \"$@\" >> '{calls}'; echo '拒绝：#22 的认领还没过期，还差 44 分钟'; exit 1",
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
    let home = Home::new("cap 空格");
    real_tool(home.path(), "cat");
    stub(
        home.path(),
        "gh",
        &format!(
            "echo \"$@\" >> '{calls}'; cat \"$7\" >> '{body}'",
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
    let out = dispatch(home.path())
        .arg("example-config")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("重定向的 stdout 要是合法的 JSON");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("checkout"),
        "操作提示留在 stderr"
    );
    assert_eq!(json["max_sessions"], 1, "一开始只许一个会话：{json}");
    assert_eq!(json["require_label"], "演示:调度", "{json}");
    assert_eq!(
        json["cloud_roles"],
        serde_json::json!(["review", "fix-ci", "nightly-review"]),
        "默认按 #9 的裁决把不碰大麦的活交给云端：{json}"
    );
    // 存下来就能直接用。
    let path = home.path().join("example.json");
    std::fs::write(&path, &stdout).unwrap();
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

#[test]
fn implement_follows_claims_queue_and_skips_waiting_blocked_and_untriaged() {
    let home = Home::new("priority-queue");
    let replay = home.path().join("replay");
    std::fs::create_dir_all(&replay).unwrap();
    let priority_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/github/claims/priority");
    std::fs::copy(
        priority_dir.join("open-issues.json"),
        replay.join("issues.json"),
    )
    .unwrap();
    for entry in std::fs::read_dir(&priority_dir).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("comments-") || n.starts_with("branch-"))
        {
            std::fs::copy(&path, replay.join(path.file_name().unwrap())).unwrap();
        }
    }
    std::fs::write(replay.join("pulls.json"), "[]").unwrap();
    std::fs::copy(
        PathBuf::from(fixtures("night")).join("main-test-runs.json"),
        replay.join("main-test-runs.json"),
    )
    .unwrap();

    let mut queue_cmd = support::claims(home.path());
    queue_cmd.args([
        "queue",
        "--json",
        "--replay",
        &priority_dir.display().to_string(),
        "--now",
        "2026-10-14T11:12:00+08:00",
    ]);
    let queue_out = output_within(queue_cmd, 60);
    assert!(
        queue_out.status.success(),
        "{}",
        String::from_utf8_lossy(&queue_out.stderr)
    );
    let queue_json: serde_json::Value =
        serde_json::from_slice(&queue_out.stdout).expect("claims queue --json");
    let queue_ready_fresh: Vec<u64> = queue_json["ready"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| matches!(r["priority"].as_str(), Some("P0" | "P1" | "P2" | "P3")))
        .filter(|r| {
            !r["notes"].as_array().is_some_and(|ns| {
                ns.iter()
                    .any(|n| n.as_str().is_some_and(|s| s.contains("验收没通过")))
            })
        })
        .map(|r| r["issue"].as_u64().unwrap())
        .collect();

    let config = home.config(serde_json::json!({
        "max_sessions": 50,
        "max_sessions_per_day": 50,
        "ignore_issues": [],
    }));
    let (code, out) = run(
        &home,
        &[
            "plan",
            "--json",
            "--config",
            &config.display().to_string(),
            "--replay",
            &replay.display().to_string(),
            "--now",
            "2026-10-14T11:12:00+08:00",
        ],
    );
    assert_eq!(code, 0, "{out}");
    let plan: serde_json::Value = serde_json::from_str(out.trim()).expect(&out);
    let implements: Vec<u64> = plan["start"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["role"] == "implement")
        .map(|t| t["issue"].as_u64().unwrap())
        .collect();
    assert_eq!(
        implements, queue_ready_fresh,
        "dispatch 的 implement 序列应与 claims queue 的已定级新活序列完全一致"
    );
    assert!(
        !implements.contains(&14),
        "#14 在 11:04:49 刚 force-push 续租，11:12 时不该派 implement：{implements:?}"
    );
    assert_eq!(
        implements.first().copied(),
        Some(2),
        "第一个派出的 implement 应当是队列的下一个 #2（P1）：{implements:?}"
    );
    let pos = |n: u64| {
        implements
            .iter()
            .position(|x| *x == n)
            .unwrap_or_else(|| panic!("#{n} 不在 implement 列表里：{implements:?}"))
    };
    assert!(
        pos(2) < pos(46) && pos(46) < pos(20),
        "P1（#2）应排在 P2（#46）前，P2 排在 P3（#20）前：{implements:?}"
    );
    for waiting in [25_u64, 63, 65, 67] {
        assert!(
            !plan["start"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["issue"].as_u64() == Some(waiting)),
            "等裁决的 #{waiting} 不该派任何角色：{plan:#}"
        );
    }
    assert!(
        !implements.contains(&54),
        "被 #4 挡住的 #54 不该派 implement：{implements:?}"
    );
    let skip_why = |n: u64| -> String {
        plan["skip"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["issue"].as_u64() == Some(n) && s["role"] == "implement")
            .and_then(|s| s["skipped"].as_str())
            .unwrap_or_else(|| panic!("#{n} 不在 implement 跳过列表里：{plan:#}"))
            .to_string()
    };
    assert!(
        skip_why(54).contains("#4"),
        "#54 应说明被 #4 挡着：{}",
        skip_why(54)
    );
    for untriaged in [70_u64, 71, 80, 81] {
        assert!(
            !implements.contains(&untriaged),
            "没定级的 #{untriaged} 不该派 implement：{implements:?}"
        );
        assert!(
            skip_why(untriaged).contains("还没定优先级"),
            "#{untriaged} 应说明没定优先级：{}",
            skip_why(untriaged)
        );
    }
}

#[test]
fn implement_matches_claims_queue_across_push_unreadable_and_url_bound_lease_cases() {
    let priority_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/github/claims/priority");
    let opened: Vec<serde_json::Value> = serde_json::from_str(
        &std::fs::read_to_string(priority_dir.join("open-issues.json")).unwrap(),
    )
    .unwrap();
    let claimed: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(priority_dir.join("issues.json")).unwrap())
            .unwrap();

    struct Case {
        name: &'static str,
        numbers: &'static [u64],
        vendors: &'static [&'static str],
        now: &'static str,
        alter: Option<&'static str>,
    }

    let cases = [
        Case {
            name: "missing-assignee",
            numbers: &[53, 58],
            vendors: &["codex"],
            now: "2026-10-14T11:12:00+08:00",
            alter: None,
        },
        Case {
            name: "expired",
            numbers: &[14, 58],
            vendors: &["claude", "codex"],
            now: "2026-10-14T15:00:00+08:00",
            alter: None,
        },
        Case {
            name: "live-push",
            numbers: &[14, 58],
            vendors: &["claude", "codex"],
            now: "2026-10-14T11:12:00+08:00",
            alter: None,
        },
        Case {
            name: "no-push-evidence",
            numbers: &[14, 58],
            vendors: &["claude", "codex"],
            now: "2026-10-14T15:00:00+08:00",
            alter: Some("no-push"),
        },
        Case {
            name: "empty-comments",
            numbers: &[14, 58],
            vendors: &["claude", "codex"],
            now: "2026-10-14T15:00:00+08:00",
            alter: Some("empty"),
        },
        Case {
            name: "valid-heartbeat",
            numbers: &[14, 58],
            vendors: &["claude", "codex"],
            now: "2026-10-14T15:00:00+08:00",
            alter: Some("validbeat"),
        },
        Case {
            name: "valid-reclaim",
            numbers: &[14, 58],
            vendors: &["claude", "codex"],
            now: "2026-10-14T11:12:00+08:00",
            alter: Some("validreclaim"),
        },
        Case {
            name: "malformed-claim",
            numbers: &[14, 58],
            vendors: &["claude", "codex"],
            now: "2026-10-14T15:00:00+08:00",
            alter: Some("malformed"),
        },
        Case {
            name: "wrong-heartbeat",
            numbers: &[14, 58],
            vendors: &["claude", "codex"],
            now: "2026-10-14T15:00:00+08:00",
            alter: Some("heartbeat"),
        },
        Case {
            name: "stale-reclaim",
            numbers: &[14, 58],
            vendors: &["claude", "codex"],
            now: "2026-10-14T11:12:00+08:00",
            alter: Some("reclaim"),
        },
        Case {
            name: "boundary-120m",
            numbers: &[14, 58],
            vendors: &["claude", "codex"],
            now: "2026-10-14T15:00:00+08:00",
            alter: Some("boundary-120m"),
        },
        Case {
            name: "boundary-119m59s",
            numbers: &[14, 58],
            vendors: &["claude", "codex"],
            now: "2026-10-14T15:00:00+08:00",
            alter: Some("boundary-119m59s"),
        },
        Case {
            name: "fullwidth-assigned-colon",
            numbers: &[53, 58],
            vendors: &["claude"],
            now: "2026-10-14T11:12:00+08:00",
            alter: Some("fullwidth-colon"),
        },
        Case {
            name: "hyphen-suffixed-assignee",
            numbers: &[53, 58],
            vendors: &["codex"],
            now: "2026-10-14T11:12:00+08:00",
            alter: Some("suffixed-assignee"),
        },
    ];

    for case in &cases {
        let Case {
            name,
            numbers,
            vendors,
            now,
            alter,
        } = *case;
        let home = Home::new(&format!("queue-parity-{name}"));
        let replay = home.path().join("replay");
        std::fs::create_dir_all(&replay).unwrap();
        for entry in std::fs::read_dir(&priority_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                std::fs::copy(&path, replay.join(path.file_name().unwrap())).unwrap();
            }
        }
        let mut selected: Vec<serde_json::Value> = opened
            .iter()
            .filter(|i| numbers.contains(&i["number"].as_u64().unwrap()))
            .cloned()
            .collect();
        if alter == Some("fullwidth-colon") {
            for item in &mut selected {
                if item["number"] == 53 {
                    item["labels"] = serde_json::json!([{"name": "P0"}, {"name": "指派：codex"}]);
                }
            }
        } else if alter == Some("suffixed-assignee") {
            for item in &mut selected {
                if item["number"] == 53 {
                    item["labels"] = serde_json::json!([{"name": "P0"}, {"name": "指派:codex-2"}]);
                }
            }
        }
        let selected_claimed: Vec<serde_json::Value> = claimed
            .iter()
            .filter(|i| numbers.contains(&i["number"].as_u64().unwrap()))
            .cloned()
            .collect();
        std::fs::write(
            replay.join("open-issues.json"),
            serde_json::to_string(&selected).unwrap(),
        )
        .unwrap();
        std::fs::write(
            replay.join("issues.json"),
            serde_json::to_string(&selected_claimed).unwrap(),
        )
        .unwrap();

        let mut comments: Vec<serde_json::Value> = serde_json::from_str(
            &std::fs::read_to_string(replay.join("comments-14.json")).unwrap(),
        )
        .unwrap();
        match alter {
            Some("no-push") => {
                let _ = std::fs::remove_file(replay.join("branch-night-issue-14.json"));
            }
            Some("boundary-120m" | "boundary-119m59s") => {
                let _ = std::fs::remove_file(replay.join("branch-night-issue-14.json"));
                let created_at = if alter == Some("boundary-120m") {
                    "2026-10-14T05:00:00Z"
                } else {
                    "2026-10-14T05:00:01Z"
                };
                comments[0]["created_at"] = serde_json::json!(created_at);
                std::fs::write(
                    replay.join("comments-14.json"),
                    serde_json::to_string(&comments[..1]).unwrap(),
                )
                .unwrap();
            }
            Some("empty") => {
                std::fs::write(replay.join("comments-14.json"), "[]").unwrap();
            }
            Some("heartbeat" | "validbeat") => {
                let mut body = "## 心跳\n- agent: Codex\n- 认领: https://github.com/zbtzbtzbt/theFlash/issues/14#issuecomment-9999\n- 进度: 另一条租约的迟到心跳\nAgent: Codex".to_string();
                if alter == Some("validbeat") {
                    body = body
                        .replace("issuecomment-9999", "issuecomment-6153808195")
                        .replace("- agent: Codex", "- agent: Claude")
                        .replace("Agent: Codex", "Agent: Claude");
                }
                comments.push(serde_json::json!({
                    "id": 999,
                    "created_at": "2026-10-14T06:59:00Z",
                    "html_url": "https://github.com/zbtzbtzbt/theFlash/issues/14#issuecomment-999",
                    "body": body,
                }));
                std::fs::write(
                    replay.join("comments-14.json"),
                    serde_json::to_string(&comments).unwrap(),
                )
                .unwrap();
            }
            Some("reclaim" | "validreclaim") => {
                let mut body = "## 收回认领\n- 原认领: Claude（https://github.com/zbtzbtzbt/theFlash/issues/14#issuecomment-9999）\n- 收回者: Codex\nAgent: Codex".to_string();
                if alter == Some("validreclaim") {
                    body = body.replace("issuecomment-9999", "issuecomment-6153808195");
                }
                comments.push(serde_json::json!({
                    "id": 998,
                    "created_at": "2026-10-14T03:10:00Z",
                    "html_url": "https://github.com/zbtzbtzbt/theFlash/issues/14#issuecomment-998",
                    "body": body,
                }));
                std::fs::write(
                    replay.join("comments-14.json"),
                    serde_json::to_string(&comments).unwrap(),
                )
                .unwrap();
            }
            Some("malformed") => {
                std::fs::write(
                    replay.join("comments-14.json"),
                    serde_json::to_string(&serde_json::json!([{
                        "id": 999,
                        "created_at": "2026-10-14T03:00:00Z",
                        "html_url": "https://github.com/zbtzbtzbt/theFlash/issues/14#issuecomment-999",
                        "body": "## 认领\n- agent: Claude\nAgent: Claude",
                    }]))
                    .unwrap(),
                )
                .unwrap();
            }
            _ => {}
        }

        let mut q_cmd = support::claims(home.path());
        q_cmd.args([
            "queue",
            "--json",
            "--agent",
            vendors[0],
            "--replay",
            &replay.display().to_string(),
            "--now",
            now,
        ]);
        let q_out = output_within(q_cmd, 60);
        assert!(
            q_out.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&q_out.stderr)
        );
        let q: serde_json::Value = serde_json::from_slice(&q_out.stdout).unwrap();
        let expected: Vec<u64> = q["next"].as_u64().into_iter().collect();

        std::fs::write(
            replay.join("issues.json"),
            serde_json::to_string(&selected).unwrap(),
        )
        .unwrap();
        std::fs::write(replay.join("pulls.json"), "[]").unwrap();
        std::fs::copy(
            PathBuf::from(fixtures("night")).join("main-test-runs.json"),
            replay.join("main-test-runs.json"),
        )
        .unwrap();

        let mut agents = serde_json::Map::new();
        for v in vendors {
            agents.insert(
                (*v).to_string(),
                serde_json::json!({ "command": ["true"], "prompt": "argv" }),
            );
        }
        let cfg = home.config(serde_json::json!({
            "max_sessions": 1,
            "max_sessions_per_day": 10,
            "cloud_roles": [],
            "require_label": "",
            "ignore_issues": [],
            "agents": agents,
            "vendor_order": vendors,
        }));
        let (_, p_text) = run(
            &home,
            &[
                "plan",
                "--json",
                "--config",
                &cfg.display().to_string(),
                "--replay",
                &replay.display().to_string(),
                "--now",
                now,
            ],
        );
        let p: serde_json::Value = serde_json::from_str(p_text.trim()).expect(&p_text);
        let actual: Vec<u64> = p["start"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["role"] == "implement")
            .map(|t| t["issue"].as_u64().unwrap())
            .collect();
        assert_eq!(
            actual, expected,
            "用例 {name}：dispatch implement ({actual:?}) != claims queue next ({expected:?})"
        );
    }
}

#[test]
fn branch_activity_network_failure_stops_dispatch_like_claims_queue_while_403_marks_push_unreadable()
 {
    // Issue #170 reproduction: #40 is P1 + 认领中 with an expired claim on
    // branch `night/issue-40`, and #41 is P2.
    // - When `gh api .../activity` fails with a network error, both
    //   `claims queue` and `dispatch plan` must exit 2 and refuse to
    //   dispatch #41 on partial data.
    // - When `gh api .../activity` returns 403 (no push access), both
    //   treat #40's push time as unreadable and offer #41 next.
    let home = Home::new("activity-error-parity");
    let data = home.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let issue_40 = serde_json::json!({
        "number": 40,
        "title": "expired-p1",
        "labels": [{"name": "P1"}, {"name": "认领中"}],
        "body": "",
    });
    let issue_41 = serde_json::json!({
        "number": 41,
        "title": "fresh-p2",
        "labels": [{"name": "P2"}],
        "body": "",
    });
    std::fs::write(
        data.join("open-issues.json"),
        serde_json::to_string(&vec![&issue_40, &issue_41]).unwrap(),
    )
    .unwrap();
    std::fs::write(
        data.join("claimed-issues.json"),
        serde_json::to_string(&vec![&issue_40]).unwrap(),
    )
    .unwrap();
    std::fs::write(
        data.join("comments-40.json"),
        serde_json::to_string(&serde_json::json!([{
            "id": 1001,
            "created_at": "2026-10-14T03:00:00Z",
            "html_url": "https://github.com/zbtzbtzbt/theFlash/issues/40#issuecomment-1001",
            "body": "## 认领\n- agent: Claude\n- 改动模块: backend/src/dispatch\n- 分支: night/issue-40\nAgent: Claude",
        }]))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(data.join("comments-41.json"), "[]").unwrap();
    std::fs::write(data.join("pulls.json"), "[]").unwrap();
    std::fs::copy(
        PathBuf::from(fixtures("night")).join("main-test-runs.json"),
        data.join("main-test-runs.json"),
    )
    .unwrap();

    real_tool(home.path(), "cat");
    stub(
        home.path(),
        "gh",
        &format!(
            "case \"$*\" in\n\
             *issues*labels=*) cat '{dir}/claimed-issues.json' ;;\n\
             *issues/40/comments*) cat '{dir}/comments-40.json' ;;\n\
             *issues/41/comments*) cat '{dir}/comments-41.json' ;;\n\
             *issues\\?state=open*) cat '{dir}/open-issues.json' ;;\n\
             *pulls\\?state=open*) cat '{dir}/pulls.json' ;;\n\
             *runs\\?branch=main*) cat '{dir}/main-test-runs.json' ;;\n\
             *activity*) echo \"$FAKE_GH_ACTIVITY_ERR\" >&2; exit 1 ;;\n\
             *) echo '[]' ;;\n\
             esac",
            dir = data.display()
        ),
    );

    let config = home.config(serde_json::json!({
        "max_sessions": 1,
        "max_sessions_per_day": 10,
        "cloud_roles": [],
        "require_label": "",
        "ignore_issues": [],
        "agents": {
            "claude": { "command": ["agent-claude"], "prompt": "argv" },
        },
        "vendor_order": ["claude"],
    }));
    let now = "2026-10-14T16:00:00+08:00";

    // 1. Network failure on `activity`: both exit 2 and mention the failed request.
    let mut q_net = support::claims(home.path());
    q_net
        .env("PATH", home.path().join("bin"))
        .env("FAKE_GH_ACTIVITY_ERR", "error connecting to api.github.com")
        .args(["queue", "--json", "--agent", "claude", "--now", now]);
    let q_net_out = output_within(q_net, 60);
    let q_net_text = format!(
        "{}{}",
        String::from_utf8_lossy(&q_net_out.stdout),
        String::from_utf8_lossy(&q_net_out.stderr)
    );
    assert_eq!(q_net_out.status.code(), Some(2), "{q_net_text}");
    assert!(
        q_net_text.contains("activity?ref=refs/heads/night%2Fissue-40")
            && q_net_text.contains("error connecting to api.github.com"),
        "{q_net_text}"
    );

    let mut d_net = dispatch(home.path());
    d_net
        .env("FAKE_GH_ACTIVITY_ERR", "error connecting to api.github.com")
        .args([
            "plan",
            "--config",
            &config.display().to_string(),
            "--now",
            now,
        ]);
    let d_net_out = output_within(d_net, 60);
    let d_net_text = format!(
        "{}{}",
        String::from_utf8_lossy(&d_net_out.stdout),
        String::from_utf8_lossy(&d_net_out.stderr)
    );
    assert_eq!(
        d_net_out.status.code(),
        Some(2),
        "activity 断网时 dispatch plan 必须像 claims queue 一样退出 2，不能去派 #41：\n{d_net_text}"
    );
    assert!(
        d_net_text.contains("activity?ref=refs/heads/night%2Fissue-40")
            && d_net_text.contains("error connecting to api.github.com")
            && !d_net_text.contains("implement-41"),
        "{d_net_text}"
    );

    // 2. 403 permission error on `activity`: both treat #40's push time as
    //    unreadable (lease stays held) and offer #41 as the next implement.
    let perm_err = "gh: Must have push access to repository (HTTP 403)";
    let mut q_perm = support::claims(home.path());
    q_perm
        .env("PATH", home.path().join("bin"))
        .env("FAKE_GH_ACTIVITY_ERR", perm_err)
        .args(["queue", "--json", "--agent", "claude", "--now", now]);
    let q_perm_out = output_within(q_perm, 60);
    assert!(
        q_perm_out.status.success(),
        "{}",
        String::from_utf8_lossy(&q_perm_out.stderr)
    );
    let q_json: serde_json::Value = serde_json::from_slice(&q_perm_out.stdout).unwrap();
    assert_eq!(q_json["next"], 41, "{q_json:#}");

    let mut d_perm = dispatch(home.path());
    d_perm.env("FAKE_GH_ACTIVITY_ERR", perm_err).args([
        "plan",
        "--json",
        "--config",
        &config.display().to_string(),
        "--now",
        now,
    ]);
    let d_perm_out = output_within(d_perm, 60);
    let d_perm_text = format!(
        "{}{}",
        String::from_utf8_lossy(&d_perm_out.stdout),
        String::from_utf8_lossy(&d_perm_out.stderr)
    );
    assert_eq!(d_perm_out.status.code(), Some(0), "{d_perm_text}");
    let d_json: serde_json::Value = serde_json::from_str(d_perm_text.trim()).unwrap();
    assert_eq!(d_json["start"][0]["issue"], 41, "{d_json:#}");
}
