//! scripts/block-damai-hosts.sh points every damai host at 0.0.0.0 in
//! /etc/hosts, so CI's tests cannot reach the real damai.cn (#7), and
//! scripts/unblock-damai-hosts.sh takes that back (#22). A block cuts the
//! whole machine off from damai.cn, browsers and `theflash watch` included,
//! so it edits the real /etc/hosts only on a disposable GitHub-hosted runner,
//! and the undo removes exactly the lines a block wrote.
//!
//! These tests edit a hosts file in a temporary directory (HOSTS_FILE). Every
//! script runs with a fake `sudo` first on PATH that refuses and records the
//! call, so a script that reaches for the real /etc/hosts fails here instead
//! of editing it. Only CI edits a real /etc/hosts (.github/workflows/test.yml).

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// `macos` is /etc/hosts of the owner's Mac, byte for byte (macOS's default).
/// `macos-blocked-untagged` is that file after one run of the block script as
/// it was before #22 (e860486), which wrote its lines without a tag.
fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/testdata/hosts/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The hosts of scripts/damai-hosts.txt.
fn listed_hosts() -> Vec<String> {
    let path = repo_root().join("scripts/damai-hosts.txt");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .map(|l| l.split('#').next().unwrap().trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Whether the resolver, reading `hosts`, answers 0.0.0.0 for `host`: some
/// line, up to its `#` comment, is `0.0.0.0` and names `host`.
fn points_at_nowhere(hosts: &str, host: &str) -> bool {
    hosts.lines().any(|line| {
        let mut fields = line.split('#').next().unwrap().split_whitespace();
        fields.next() == Some("0.0.0.0") && fields.any(|f| f == host)
    })
}

/// A temporary directory with the hosts file under test and a fake `sudo`.
/// Removed when dropped.
struct Scratch {
    dir: PathBuf,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn ok(&self) -> &Run {
        assert_eq!(
            self.code,
            Some(0),
            "stdout: {}\nstderr: {}",
            self.stdout,
            self.stderr
        );
        self
    }
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("theflash-hosts-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let sudo = dir.join("bin/sudo");
        std::fs::write(
            &sudo,
            format!(
                "#!/bin/sh\necho \"$*\" >>'{}'\necho 'fake sudo: refused' >&2\nexit 1\n",
                dir.join("sudo-calls").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
        let s = Scratch { dir };
        // The fake is what keeps a broken script off the real /etc/hosts, so
        // make sure it is the sudo the scripts find.
        let found = s
            .command("/bin/bash")
            .args(["-c", "command -v sudo"])
            .output();
        assert_eq!(
            String::from_utf8_lossy(&found.unwrap().stdout).trim(),
            sudo.to_str().unwrap(),
            "the scripts would not run the fake sudo"
        );
        s
    }

    fn hosts(&self) -> PathBuf {
        self.dir.join("hosts")
    }

    fn write_hosts(&self, bytes: &[u8]) {
        std::fs::write(self.hosts(), bytes).unwrap();
    }

    fn read_hosts(&self) -> Vec<u8> {
        std::fs::read(self.hosts()).unwrap()
    }

    /// What the scripts tried to run through sudo.
    fn sudo_calls(&self) -> String {
        std::fs::read_to_string(self.dir.join("sudo-calls")).unwrap_or_default()
    }

    /// `program` with nothing of this machine's environment but a PATH that
    /// starts with the fake sudo, and HOME and TMPDIR in the scratch dir.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = Command::new(program);
        c.env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.dir.join("bin").display()),
            )
            .env("HOME", &self.dir)
            .env("TMPDIR", &self.dir)
            .stdin(Stdio::null());
        c
    }

    /// Runs `script` under /bin/bash (bash 3.2 on macOS) with `env` added,
    /// editing the scratch hosts file, or the real /etc/hosts if `real`.
    fn run(&self, script: &Path, real: bool, env: &[(&str, &str)]) -> Run {
        let mut c = self.command("/bin/bash");
        c.arg(script).envs(env.iter().copied());
        if !real {
            c.env("HOSTS_FILE", self.hosts());
        }
        let out = support::output_within(c, 30);
        Run {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn block(&self) -> Run {
        self.run(&script("block-damai-hosts.sh"), false, &[])
    }

    fn unblock(&self) -> Run {
        self.run(&script("unblock-damai-hosts.sh"), false, &[])
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn script(name: &str) -> PathBuf {
    repo_root().join("scripts").join(name)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn blocking_and_then_unblocking_leaves_the_hosts_file_as_it_was() {
    let s = Scratch::new("round-trip");
    let original = fixture("macos");
    s.write_hosts(&original);

    s.block().ok();
    let blocked = s.read_hosts();
    assert!(
        blocked.starts_with(&original),
        "the block changed what was in the file:\n{}",
        text(&blocked)
    );
    let added = text(&blocked[original.len()..]);
    for host in listed_hosts() {
        assert!(
            points_at_nowhere(&added, &host),
            "{host} is not blocked:\n{added}"
        );
    }
    assert_eq!(added.lines().count(), listed_hosts().len(), "{added}");
    for line in added.lines() {
        assert!(
            line.contains("scripts/unblock-damai-hosts.sh"),
            "a line of the block does not say how to undo it: {line}"
        );
    }

    s.block().ok();
    assert_eq!(
        text(&s.read_hosts()),
        text(&blocked),
        "a second block changed the file"
    );

    let undo = s.unblock();
    undo.ok();
    assert_eq!(
        text(&s.read_hosts()),
        text(&original),
        "the undo did not restore the file"
    );
    for host in listed_hosts() {
        assert!(
            undo.stdout.contains(&format!(" {host} ")),
            "the undo does not say it removed {host}: {}",
            undo.stdout
        );
    }
    assert_eq!(
        s.sudo_calls(),
        "",
        "a script ran sudo for a file it can write"
    );
}

#[test]
fn unblocking_removes_the_lines_of_the_block_script_from_before_it_tagged_them() {
    let s = Scratch::new("untagged");
    s.write_hosts(&fixture("macos-blocked-untagged"));
    s.unblock().ok();
    assert_eq!(text(&s.read_hosts()), text(&fixture("macos")));
}

#[test]
fn unblocking_keeps_every_line_a_block_did_not_write() {
    let s = Scratch::new("others");
    let mut original = fixture("macos");
    // Lines someone wrote by hand, each close to one the block writes. The
    // block sees that the first two block their hosts already, and adds its
    // own lines for the rest.
    for line in [
        // Split, or repo_guard.rs reads the `t` of `\t` as part of the host.
        concat!("0.0.0.0\t", "damai.cn"),
        "0.0.0.0 www.damai.cn # mine",
        "127.0.0.1 detail.damai.cn",
        "# 0.0.0.0 m.damai.cn",
        "0.0.0.0 search.damai.cnx",
        " 0.0.0.0 mtop.damai.cn",
    ] {
        original.extend_from_slice(line.as_bytes());
        original.push(b'\n');
    }
    s.write_hosts(&original);

    s.block().ok();
    let blocked = text(&s.read_hosts());
    for host in listed_hosts() {
        assert!(
            points_at_nowhere(&blocked, &host),
            "{host} is not blocked:\n{blocked}"
        );
    }
    s.unblock().ok();
    assert_eq!(text(&s.read_hosts()), text(&original));
}

#[test]
fn a_block_starts_on_a_line_of_its_own_when_the_file_does_not_end_with_a_newline() {
    let s = Scratch::new("no-newline");
    let whole = fixture("macos");
    s.write_hosts(&whole[..whole.len() - 1]);

    s.block().ok();
    let blocked = s.read_hosts();
    assert!(
        blocked.starts_with(&whole),
        "the block ran into the last line of the file:\n{}",
        text(&blocked)
    );
    s.unblock().ok();
    assert_eq!(text(&s.read_hosts()), text(&whole));
}

#[test]
fn unblocking_a_file_with_nothing_to_undo_does_not_write_it() {
    let s = Scratch::new("nothing");
    let original = fixture("macos");
    s.write_hosts(&original);
    // Writing a read-only file fails, so a needless rewrite shows.
    std::fs::set_permissions(s.hosts(), std::fs::Permissions::from_mode(0o444)).unwrap();

    let undo = s.unblock();
    undo.ok();
    assert!(undo.stdout.contains("nothing to undo"), "{}", undo.stdout);
    assert_eq!(text(&s.read_hosts()), text(&original));
}

#[test]
fn unblocking_removes_hosts_that_left_the_list_after_the_block() {
    // A block from a checkout whose list had one more host, undone by this one.
    let s = Scratch::new("old-list");
    let gone = ["gone.", "damai", ".cn"].concat();
    let other = s.dir.join("other-checkout/scripts");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::copy(
        script("block-damai-hosts.sh"),
        other.join("block-damai-hosts.sh"),
    )
    .unwrap();
    let list = std::fs::read_to_string(script("damai-hosts.txt")).unwrap();
    std::fs::write(other.join("damai-hosts.txt"), format!("{list}{gone}\n")).unwrap();
    let original = fixture("macos");
    s.write_hosts(&original);

    s.run(&other.join("block-damai-hosts.sh"), false, &[]).ok();
    assert!(points_at_nowhere(&text(&s.read_hosts()), &gone));
    s.unblock().ok();
    assert_eq!(text(&s.read_hosts()), text(&original));
}

#[test]
fn the_block_refuses_the_real_hosts_file_anywhere_but_a_github_hosted_runner() {
    for env in [
        &[][..],
        // Agents often set CI=true to keep tools from prompting.
        &[("CI", "true")][..],
        &[
            ("CI", "true"),
            ("GITHUB_ACTIONS", "true"),
            ("RUNNER_ENVIRONMENT", "self-hosted"),
        ][..],
    ] {
        let s = Scratch::new("refuse");
        let run = s.run(&script("block-damai-hosts.sh"), true, env);
        assert_eq!(
            s.sudo_calls(),
            "",
            "with {env:?}, the block tried to edit the real /etc/hosts through sudo"
        );
        assert_ne!(run.code, Some(0), "with {env:?}, the block did not refuse");
        assert!(
            run.stderr.contains("GitHub-hosted")
                && run.stderr.contains("scripts/unblock-damai-hosts.sh"),
            "the refusal does not say why, or how to undo an earlier block: {}",
            run.stderr
        );
    }
}
