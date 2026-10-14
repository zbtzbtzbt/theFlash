//! Layer 2 of decision 0003: scripts/check.sh, which `make check` and CI
//! run, first downloads what the later steps need, and runs every step after
//! that with its proxy pointing at a port nothing listens on. Whatever
//! language a test group is written in, a request it sends for anything but
//! this machine then fails instead of reaching the real damai.cn (#34). A
//! group placed above that line, or a script that keeps the proxy settings
//! of whoever runs it, lets its requests out, and no other test turns red:
//! the tests that start theflash have a tripwire of their own (layer 1),
//! and the others send no request today.
//!
//! The test runs a copy of the script in a temporary directory. Every tool
//! the script runs is a stand-in that records how it was started and
//! succeeds, and PATH holds nothing else but dirname, so no real tool runs,
//! and a step that runs a tool without a stand-in fails here until it gets
//! one.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The proxy of every step after the downloads: port 9 (discard) on this
/// machine, where nothing listens.
const DEAD: &str = "http://127.0.0.1:9";
const PROXIES: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];
const NO_PROXIES: [&str; 2] = ["NO_PROXY", "no_proxy"];
/// What may skip the proxy: this machine, nothing else.
const LOOPBACK: [&str; 3] = ["127.0.0.1", "localhost", "::1"];
/// Node's fetch and http use the proxy variables only with this set to 1.
const NODE: &str = "NODE_USE_ENV_PROXY";

/// The tools check.sh runs. A step that runs another one makes the script
/// fail here with "command not found"; add the tool's name.
const TOOLS: [&str; 3] = ["cargo", "npm", "python3"];

/// The steps that download what the others need: the only ones that may
/// use the network.
fn is_download(argv: &str) -> bool {
    let mut words = argv.split(' ');
    matches!(
        (words.next(), words.next()),
        (Some("cargo"), Some("fetch")) | (Some("npm"), Some("ci"))
    )
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// The first `tool` on this process's PATH.
fn on_path(tool: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join(tool))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("no {tool} on PATH"))
}

fn every_var() -> impl Iterator<Item = &'static str> {
    PROXIES.into_iter().chain(NO_PROXIES).chain([NODE])
}

/// One run of a stand-in: its command line, and each of `every_var()` as
/// it saw it (None for unset).
struct Step {
    argv: String,
    vars: Vec<(&'static str, Option<String>)>,
}

impl Step {
    /// What in this step's environment lets a request leave this machine,
    /// or None if nothing does.
    fn leak(&self) -> Option<String> {
        let wrong: Vec<String> = self
            .vars
            .iter()
            .filter(|(name, value)| {
                let value = value.as_deref();
                if PROXIES.contains(name) {
                    value != Some(DEAD)
                } else if NO_PROXIES.contains(name) {
                    value
                        .unwrap_or("")
                        .split(',')
                        .map(str::trim)
                        .any(|host| !host.is_empty() && !LOOPBACK.contains(&host))
                } else {
                    value != Some("1")
                }
            })
            .map(|(name, value)| match value {
                Some(v) => format!("{name}={v}"),
                None => format!("{name} unset"),
            })
            .collect();
        (!wrong.is_empty()).then(|| format!("  {}: {}", self.argv, wrong.join(", ")))
    }
}

/// A copy of the repository's layout with check.sh in it, and a bin
/// directory of stand-ins. Removed when dropped.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new() -> Scratch {
        let dir = std::env::temp_dir().join(format!("theflash-dead-proxy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["bin", "scripts", "backend", "frontend"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::copy(
            repo_root().join("scripts/check.sh"),
            dir.join("scripts/check.sh"),
        )
        .unwrap();
        let vars: String = every_var()
            .map(|v| format!(" \"${{{v}-<unset>}}\""))
            .collect();
        let stand_in = format!(
            "#!/bin/sh\n{{\n  printf '%s' \"${{0##*/}}\"\n  for a in \"$@\"; do printf ' %s' \"$a\"; done\n  \
             printf '\\t%s'{vars}\n  printf '\\n'\n}} >>'{}'\n",
            dir.join("steps").display()
        );
        for tool in TOOLS {
            let path = dir.join("bin").join(tool);
            std::fs::write(&path, &stand_in).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // The real one: the script finds the repository with it, and it
        // reads nothing but its argument.
        std::os::unix::fs::symlink(on_path("dirname"), dir.join("bin/dirname")).unwrap();
        Scratch { dir }
    }

    /// Runs the copy of check.sh under /bin/bash (bash 3.2 on macOS) with
    /// nothing of this machine's environment, and the proxy settings of a
    /// developer who uses a proxy for everything: the script has to replace
    /// them, not keep them.
    fn check(&self) -> std::process::Output {
        let mut c = Command::new("/bin/bash");
        c.arg(self.dir.join("scripts/check.sh"))
            .env_clear()
            .env("PATH", self.dir.join("bin"))
            .env("HOME", &self.dir)
            .env("TMPDIR", &self.dir)
            .stdin(Stdio::null());
        for v in PROXIES {
            c.env(v, "http://proxy.invalid:3128");
        }
        for v in NO_PROXIES {
            c.env(v, "*");
        }
        support::output_within(c, 30)
    }

    /// The stand-ins' runs, in order.
    fn steps(&self) -> Vec<Step> {
        std::fs::read_to_string(self.dir.join("steps"))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let mut fields = line.split('\t');
                let argv = fields.next().unwrap().to_string();
                let vars = every_var()
                    .map(|name| {
                        let value = fields.next().unwrap();
                        (name, (value != "<unset>").then(|| value.to_string()))
                    })
                    .collect();
                Step { argv, vars }
            })
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn every_step_but_the_downloads_runs_behind_the_dead_proxy() {
    let s = Scratch::new();
    let out = s.check();
    assert_eq!(
        out.status.code(),
        Some(0),
        "check.sh failed with only stand-ins for {TOOLS:?} on PATH. If it ran \
         another tool (\"command not found\"), add it to TOOLS.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let steps = s.steps();
    let ran: Vec<&str> = steps.iter().map(|st| st.argv.as_str()).collect();
    // The groups #34 is about, one per language, so that the check below
    // cannot pass because they stopped running.
    for group in ["python3 -m unittest", "cargo test", "npm run check"] {
        assert!(
            ran.iter().any(|argv| argv.starts_with(group)),
            "check.sh no longer runs `{group}`: {ran:#?}"
        );
    }
    let leaks: Vec<String> = steps
        .iter()
        .filter(|st| !is_download(&st.argv))
        .filter_map(Step::leak)
        .collect();
    assert!(
        leaks.is_empty(),
        "these steps of scripts/check.sh run where a request can leave this machine, so a \
         test in them could reach the real damai.cn (decision 0003, layer 2). Put them after \
         the downloads, where the dead proxy is exported; a step that has to download \
         something goes with the downloads, and into is_download here:\n{}",
        leaks.join("\n")
    );
}
