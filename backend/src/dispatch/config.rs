//! 调度程序的设置：派给哪些 agent、上限多少、哪些活归云端。
//!
//! The file lives outside the repository (`~/.config/theflash/dispatch.json`
//! by default) because it names the agent command lines on this machine, and
//! an agent's credentials never belong in the repository. Every field has a
//! default, so a file with only `{}` in it still runs, and a field added
//! later does not break an older file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Role;

/// The repository the dispatcher watches, when the file does not say.
pub const DEFAULT_REPO: &str = "zbtzbtzbt/theFlash";

/// Roles the owner's #9 ruling puts in the cloud: they never reach
/// damai.cn and need no browser, so the Mac does not run them again.
pub const DEFAULT_CLOUD_ROLES: [&str; 3] = ["review", "fix-ci", "nightly-review"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// `owner/name` of the repository to watch.
    pub repo: String,
    /// The git checkout the dispatcher makes each session's worktree
    /// from. It is only ever read from: sessions work in their own
    /// worktrees, never in this one.
    pub checkout: PathBuf,
    /// Where each session gets its own git worktree.
    pub worktrees: PathBuf,
    /// Where session logs are written, one file per session.
    pub logs: PathBuf,
    /// Where the dispatcher remembers what it has already started.
    pub state: PathBuf,
    /// How often `dispatch run` looks at GitHub.
    pub interval_seconds: u64,
    /// How many sessions may run at the same time.
    pub max_sessions: usize,
    /// How many sessions may start in one local day. Keeps a loop in the
    /// repository from spending the owner's money without a ceiling.
    pub max_sessions_per_day: usize,
    /// A session still running after this long is treated as dead: its
    /// claim is released and the issue is told.
    pub session_timeout_minutes: i64,
    /// When set, only issues and pull requests carrying this label are
    /// dispatched. A new install starts with a demo label so it cannot
    /// touch work other agents are doing.
    pub require_label: String,
    /// Issues the dispatcher never dispatches (coordination issues, issues
    /// waiting for the owner).
    pub ignore_issues: Vec<u64>,
    /// Roles handled in the cloud (see #9). Names are the role names
    /// printed by `dispatch plan`.
    pub cloud_roles: Vec<String>,
    /// The workflow whose runs say whether main is red. Only this one
    /// counts: a refused Release run also sits on main's commit (#5).
    pub test_workflow: String,
    /// The command that reads and releases claims (`scripts/claims`).
    pub claims_command: Vec<String>,
    /// The command used to talk to GitHub. A test points this at a stub.
    pub gh_command: Vec<String>,
    /// Which agent command line each vendor means.
    pub agents: BTreeMap<String, Agent>,
    /// The order vendors are tried in when two are equally busy.
    pub vendor_order: Vec<String>,
    /// Jobs that run on a clock rather than on a GitHub event.
    pub scheduled: Vec<Scheduled>,
}

/// One vendor's command line, and how it wants the prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// Program and arguments. The prompt is added according to `prompt`.
    pub command: Vec<String>,
    #[serde(default)]
    pub prompt: PromptStyle,
}

/// How an agent command line takes its prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PromptStyle {
    /// Appended as the last argument (`cursor-agent -p "<prompt>"`).
    #[default]
    Argv,
    /// Written to the process's stdin (`codex exec -`).
    Stdin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scheduled {
    /// Name of the job, also its role name for `cloud_roles`.
    pub name: String,
    /// Local time of day, `HH:MM`.
    pub at: String,
    /// Which vendor runs it.
    pub vendor: String,
    /// What the session is told to do.
    pub instruction: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            repo: DEFAULT_REPO.to_string(),
            checkout: home().join("theFlash"),
            worktrees: home().join("theflash-sessions"),
            logs: home().join(".local/state/theflash-dispatch/logs"),
            state: home().join(".local/state/theflash-dispatch"),
            interval_seconds: 60,
            max_sessions: 1,
            max_sessions_per_day: 4,
            session_timeout_minutes: 120,
            require_label: String::new(),
            ignore_issues: Vec::new(),
            cloud_roles: DEFAULT_CLOUD_ROLES.iter().map(|s| s.to_string()).collect(),
            test_workflow: "test.yml".to_string(),
            claims_command: vec!["scripts/claims".to_string()],
            gh_command: vec!["gh".to_string()],
            agents: BTreeMap::new(),
            vendor_order: Vec::new(),
            scheduled: Vec::new(),
        }
    }
}

impl Config {
    /// The settings in `path`. A missing file is an error rather than the
    /// defaults: a dispatcher started with the wrong `--config` would
    /// otherwise quietly watch the wrong repository with no agents.
    pub fn read(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("读 {} 失败：{e}", path.display()))?;
        let mut config: Config = serde_json::from_str(&text)
            .map_err(|e| format!("{} 不是预期的格式：{e}", path.display()))?;
        config.checkout = expand(&config.checkout);
        config.worktrees = expand(&config.worktrees);
        config.logs = expand(&config.logs);
        config.state = expand(&config.state);
        config.check()?;
        Ok(config)
    }

    /// The settings a fresh install starts with: one session at a time, a
    /// small daily ceiling, and only the demo label, so the first real run
    /// cannot touch what other agents are doing.
    pub fn example() -> Config {
        let mut agents = BTreeMap::new();
        agents.insert(
            "claude".to_string(),
            Agent {
                command: vec![
                    "cursor-agent".to_string(),
                    "-p".to_string(),
                    "--force".to_string(),
                ],
                prompt: PromptStyle::Argv,
            },
        );
        agents.insert(
            "codex".to_string(),
            Agent {
                // Homebrew's codex 0.152 rejects the configured model when
                // it is signed in with a ChatGPT account, so the path to the
                // one inside ChatGPT.app is a setting, not a constant.
                command: vec![
                    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex".to_string(),
                    "exec".to_string(),
                    "-".to_string(),
                ],
                prompt: PromptStyle::Stdin,
            },
        );
        agents.insert(
            "codebuddy".to_string(),
            Agent {
                command: vec!["codebuddy".to_string(), "-p".to_string()],
                prompt: PromptStyle::Argv,
            },
        );
        Config {
            require_label: "演示:调度".to_string(),
            max_sessions: 1,
            max_sessions_per_day: 2,
            vendor_order: vec![
                "claude".to_string(),
                "codex".to_string(),
                "codebuddy".to_string(),
            ],
            agents,
            scheduled: vec![Scheduled {
                name: "nightly-review".to_string(),
                at: "03:00".to_string(),
                vendor: "codex".to_string(),
                instruction: "按 prompts/nightly-review.md 复盘今天的 commit".to_string(),
            }],
            ..Config::default()
        }
    }

    /// Rejects a file that would send work nowhere, or to a vendor that has
    /// no command line, so the mistake is reported at start instead of the
    /// first time an event arrives.
    fn check(&self) -> Result<(), String> {
        if self.repo.split('/').count() != 2 {
            return Err(format!("repo 要写成 owner/name，现在是 {}", self.repo));
        }
        if self.checkout.as_os_str().is_empty() {
            return Err(
                "checkout 是空的：写上这台机器上 theFlash 的 git checkout 路径".to_string(),
            );
        }
        if self.agents.is_empty() {
            return Err("agents 是空的：至少配一个 agent 的命令行".to_string());
        }
        for (vendor, agent) in &self.agents {
            if agent.command.is_empty() {
                return Err(format!("agents.{vendor}.command 是空的"));
            }
        }
        for vendor in &self.vendor_order {
            if !self.agents.contains_key(vendor) {
                return Err(format!("vendor_order 里的 {vendor} 没有配命令行"));
            }
        }
        for job in &self.scheduled {
            parse_hhmm(&job.at)
                .ok_or_else(|| format!("定时任务 {} 的 at 要写成 HH:MM", job.name))?;
            if !self.agents.contains_key(&job.vendor) {
                return Err(format!(
                    "定时任务 {} 要交给 {}，但它没有配命令行",
                    job.name, job.vendor
                ));
            }
        }
        if self.max_sessions == 0 {
            return Err("max_sessions 是 0：调度程序什么都不会派".to_string());
        }
        Ok(())
    }

    /// Every vendor that can be sent work, in the order ties are broken.
    pub fn vendors(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .vendor_order
            .iter()
            .filter(|v| self.agents.contains_key(*v))
            .cloned()
            .collect();
        for vendor in self.agents.keys() {
            if !out.contains(vendor) {
                out.push(vendor.clone());
            }
        }
        out
    }

    /// Whether this role runs in the cloud, in which case the dispatcher on
    /// this machine leaves it alone (#9).
    pub fn in_cloud(&self, role: &Role) -> bool {
        self.cloud_roles.iter().any(|r| r == role.name())
    }
}

/// `HH:MM` as minutes since midnight.
pub fn parse_hhmm(s: &str) -> Option<i64> {
    let (h, m) = s.split_once(':')?;
    let h: i64 = h.trim().parse().ok()?;
    let m: i64 = m.trim().parse().ok()?;
    (0..24).contains(&h).then_some(())?;
    (0..60).contains(&m).then_some(())?;
    Some(h * 60 + m)
}

/// The home directory, or `.` when the environment has none, so a path
/// built from it is still usable.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Turns a leading `~` into the home directory: a settings file is written
/// by hand, and `~/Library/Logs/...` is what a person types.
pub fn expand(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if text == "~" => home(),
        None => path.to_path_buf(),
    }
}

/// Where the settings live when `--config` is not given.
pub fn default_path() -> PathBuf {
    home().join(".config/theflash/dispatch.json")
}
