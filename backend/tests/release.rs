//! What decides whether a release is published (scripts/release.sh) and the
//! installer users run (scripts/install.sh), issue #5. Each test works in a
//! throwaway git repository with a HOME of its own, and puts a stand-in for
//! GitHub CLI first on PATH that answers with real GitHub API responses
//! saved in testdata/github. Nothing here reaches GitHub. The rules are in
//! docs/code-standards.md (发版).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

const REPO: &str = "zbtzbtzbt/theFlash";

/// Stands in for gh. Logs its arguments, one call per line. `gh api` prints
/// the file named by FAKE_GH_API; `gh release download` copies the files
/// named by --pattern from FAKE_GH_RELEASE into --dir.
const FAKE_GH: &str = r#"#!/bin/sh
printf '%s\n' "$*" >>"$FAKE_GH_LOG"
case "$1 $2" in
  "api "*) cat "$FAKE_GH_API" ;;
  "release download")
    shift 2
    dir=.
    patterns=
    while [ $# -gt 0 ]; do
      case $1 in
        --dir) dir=$2; shift ;;
        --pattern) patterns="$patterns $2"; shift ;;
        --repo) shift ;;
      esac
      shift
    done
    for p in $patterns; do
      [ -f "$FAKE_GH_RELEASE/$p" ] || { echo "release asset not found: $p" >&2; exit 1; }
      cp "$FAKE_GH_RELEASE/$p" "$dir/"
    done
    ;;
  *) echo "fake gh: unexpected arguments: $*" >&2; exit 1 ;;
esac
"#;

/// Stands in for uname, so the installer can be run as on each platform.
const FAKE_UNAME: &str = r#"#!/bin/sh
case $1 in
  -m) echo "$FAKE_UNAME_M" ;;
  *) echo "$FAKE_UNAME_S" ;;
esac
"#;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn text(out: &Output) -> String {
    format!(
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A directory of the test's own, removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "theflash-release-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A git repository standing in for a clone of theFlash, a directory of fake
/// commands first on PATH, and a HOME, so that neither git nor anything else
/// reads the real configuration.
struct Sandbox {
    tmp: TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let s = Sandbox {
            tmp: TempDir::new(),
        };
        for d in ["repo/backend", "bin", "home"] {
            fs::create_dir_all(s.path(d)).unwrap();
        }
        write_executable(&s.path("bin/gh"), FAKE_GH);
        s.git(&["init", "-q", "-b", "main"]);
        s
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.0.join(rel)
    }

    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(self.path("repo"))
            .env("HOME", self.path("home"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.path("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "agent")
            .env("GIT_AUTHOR_EMAIL", "agent@example.invalid")
            .env("GIT_COMMITTER_NAME", "agent")
            .env("GIT_COMMITTER_EMAIL", "agent@example.invalid")
            .env("GITHUB_REPOSITORY", REPO)
            .env("FAKE_GH_LOG", self.path("gh.log"))
            .env_remove("GITHUB_OUTPUT")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("THEFLASH_INSTALL_DIR");
        c
    }

    fn git(&self, args: &[&str]) -> String {
        let out = self.command("git").args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", text(&out));
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// Commits a backend/Cargo.toml saying `version`, shaped like the real
    /// one: dependencies have versions of their own.
    fn commit(&self, title: &str, version: &str) -> String {
        fs::write(
            self.path("repo/backend/Cargo.toml"),
            format!(
                "[package]\nname = \"theflash\"\nversion = \"{version}\"\nedition = \"2024\"\n\n\
                 [dependencies]\nserde = {{ version = \"1.0.229\", features = [\"derive\"] }}\n\
                 serde_json = \"1.0.154\"\n"
            ),
        )
        .unwrap();
        // Something changes in every commit, even with the same version.
        fs::write(self.path("repo/title.txt"), title).unwrap();
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", title]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// Where origin/main is, as after `git fetch`.
    fn main_at(&self, sha: &str) {
        self.git(&["update-ref", "refs/remotes/origin/main", sha]);
    }

    /// A real response of the runs API with every run's head_sha set to
    /// `sha`, then changed by `edit`.
    fn api(&self, fixture: &str, sha: &str, edit: impl FnOnce(&mut Vec<Value>)) -> PathBuf {
        let path = format!("{}/testdata/github/{fixture}", env!("CARGO_MANIFEST_DIR"));
        let mut data: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let runs = data["workflow_runs"].as_array_mut().unwrap();
        for run in runs.iter_mut() {
            run["head_sha"] = sha.into();
        }
        edit(runs);
        data["total_count"] = data["workflow_runs"].as_array().unwrap().len().into();
        let out = self.path("api.json");
        fs::write(&out, data.to_string()).unwrap();
        out
    }

    fn green(&self, sha: &str) -> PathBuf {
        self.api("test_runs_push_success.json", sha, |_| {})
    }

    fn release(&self, api: &Path, args: &[&str]) -> Output {
        self.command("bash")
            .arg(root().join("scripts/release.sh"))
            .args(args)
            .env("FAKE_GH_API", api)
            .env("GITHUB_OUTPUT", self.path("github_output"))
            .output()
            .unwrap()
    }

    fn github_output(&self) -> String {
        fs::read_to_string(self.path("github_output")).unwrap_or_default()
    }

    fn gh_calls(&self) -> String {
        fs::read_to_string(self.path("gh.log")).unwrap_or_default()
    }
}

/// main with two commits; the second, saying `version`, is the one to release.
fn on_main(version: &str) -> (Sandbox, String) {
    let s = Sandbox::new();
    s.commit("backend: first", "0.0.1");
    let sha = s.commit("backend: second", version);
    s.main_at(&sha);
    (s, sha)
}

/// Refused with a reason containing `why`, and nothing for the release
/// workflow's later jobs to act on.
fn assert_refused(s: &Sandbox, out: &Output, why: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{}", text(out));
    assert!(
        stderr.contains("拒绝发布") && stderr.contains(why),
        "want a refusal saying {why:?}\n{}",
        text(out)
    );
    assert_eq!(s.github_output(), "", "a refused gate wrote outputs");
}

// ---- the gate ----

#[test]
fn a_tag_on_main_whose_test_run_finished_green_may_be_released() {
    let (s, sha) = on_main("0.1.0-rc.1");
    s.git(&["tag", "v0.1.0-rc.1", &sha]);
    let out = s.release(&s.green(&sha), &["gate", "v0.1.0-rc.1"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        s.github_output(),
        format!("tag=v0.1.0-rc.1\ncommit={sha}\nversion=0.1.0-rc.1\nprerelease=true\n")
    );
    assert_eq!(
        s.gh_calls(),
        format!("api repos/{REPO}/actions/workflows/test.yml/runs?head_sha={sha}&per_page=100\n")
    );
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .contains("https://github.com/zbtzbtzbt/theFlash/actions/runs/38444054715"),
        "the passing run is not named: {}",
        text(&out)
    );
}

// workflow_dispatch with a commit: the tag is made when the release is.
#[test]
fn a_commit_may_be_released_under_a_tag_that_does_not_exist_yet() {
    let (s, sha) = on_main("0.2.0");
    let out = s.release(&s.green(&sha), &["gate", "v0.2.0", &sha]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        s.github_output(),
        format!("tag=v0.2.0\ncommit={sha}\nversion=0.2.0\nprerelease=false\n")
    );
}

#[test]
fn a_commit_whose_test_run_has_not_finished_is_refused() {
    for status in ["in_progress", "queued", "waiting"] {
        let (s, sha) = on_main("0.1.0");
        let api = s.api("test_runs_push_success.json", &sha, |runs| {
            runs[0]["status"] = status.into();
            runs[0]["conclusion"] = Value::Null;
        });
        let out = s.release(&api, &["gate", "v0.1.0", &sha]);
        assert_refused(&s, &out, &format!("Test 还没跑完（{status}）"));
    }
}

#[test]
fn a_commit_whose_test_run_failed_or_was_cancelled_is_refused() {
    for conclusion in ["failure", "cancelled", "timed_out", "skipped"] {
        let (s, sha) = on_main("0.1.0");
        let api = s.api("test_runs_push_success.json", &sha, |runs| {
            runs[0]["conclusion"] = conclusion.into();
        });
        let out = s.release(&api, &["gate", "v0.1.0", &sha]);
        assert_refused(&s, &out, &format!("Test 没有通过（{conclusion}）"));
    }
}

#[test]
fn one_red_run_refuses_the_commit_even_when_another_run_passed() {
    let (s, sha) = on_main("0.1.0");
    let api = s.api("test_runs_push_success.json", &sha, |runs| {
        let mut rerun = runs[0].clone();
        rerun["event"] = "workflow_dispatch".into();
        rerun["conclusion"] = "failure".into();
        runs.insert(0, rerun);
    });
    let out = s.release(&api, &["gate", "v0.1.0", &sha]);
    assert_refused(&s, &out, "Test 没有通过（failure）");
}

// A pull request's run tested the PR merged into main, not this commit.
#[test]
fn a_commit_tested_only_by_its_pull_request_is_refused() {
    let (s, sha) = on_main("0.1.0");
    let api = s.api("test_runs_pull_request_only.json", &sha, |_| {});
    let out = s.release(&api, &["gate", "v0.1.0", &sha]);
    assert_refused(&s, &out, "没有跑过 Test 工作流");
}

#[test]
fn a_commit_without_any_test_run_is_refused() {
    let (s, sha) = on_main("0.1.0");
    let api = s.api("test_runs_push_success.json", &sha, Vec::clear);
    let out = s.release(&api, &["gate", "v0.1.0", &sha]);
    assert_refused(&s, &out, "没有跑过 Test 工作流");
}

#[test]
fn a_run_of_another_commit_does_not_count() {
    let (s, sha) = on_main("0.1.0");
    let api = s.api("test_runs_push_success.json", &sha, |runs| {
        runs[0]["head_sha"] = "57979b488a5158ee4311d61506f94b14675710cb".into();
    });
    let out = s.release(&api, &["gate", "v0.1.0", &sha]);
    assert_refused(&s, &out, "没有跑过 Test 工作流");
}

#[test]
fn a_commit_that_is_not_on_main_is_refused_before_asking_github() {
    let (s, _) = on_main("0.1.0");
    s.git(&["switch", "-q", "-c", "side"]);
    let side = s.commit("backend: not merged", "0.1.0");
    let out = s.release(&s.green(&side), &["gate", "v0.1.0", &side]);
    assert_refused(&s, &out, "不在 main 上");
    assert_eq!(s.gh_calls(), "");

    // A tag pushed on that commit is refused the same way.
    s.git(&["tag", "v0.1.0", &side]);
    let out = s.release(&s.green(&side), &["gate", "v0.1.0"]);
    assert_refused(&s, &out, "不在 main 上");
}

#[test]
fn a_tag_that_differs_from_the_version_in_cargo_toml_is_refused() {
    let (s, sha) = on_main("0.1.0");
    for tag in ["v0.1.0-rc.1", "v0.1.1", "v1.0.0"] {
        let out = s.release(&s.green(&sha), &["gate", tag, &sha]);
        assert_refused(&s, &out, "backend/Cargo.toml 的版本是 0.1.0");
    }
}

#[test]
fn a_tag_that_is_not_a_release_version_is_refused() {
    let (s, sha) = on_main("0.1.0");
    for tag in [
        "0.1.0",
        "v0.1",
        "v0.1.0-beta.1",
        "v0.1.0-rc.0",
        "v0.1.0-rc1",
        "v01.1.0",
        "v0.1.0+build",
        "v0.1.0 ",
    ] {
        let out = s.release(&s.green(&sha), &["gate", tag, &sha]);
        assert_refused(&s, &out, "格式不对");
    }
    assert_eq!(s.gh_calls(), "");
}

#[test]
fn a_tag_that_already_points_at_another_commit_is_refused() {
    let s = Sandbox::new();
    let first = s.commit("backend: first", "0.1.0");
    let second = s.commit("backend: second", "0.1.0");
    s.main_at(&second);
    s.git(&["tag", "v0.1.0", &first]);
    let out = s.release(&s.green(&second), &["gate", "v0.1.0", &second]);
    assert_refused(&s, &out, &format!("已经指向 {first}"));
}

#[test]
fn a_commit_must_be_named_by_its_full_sha() {
    let (s, sha) = on_main("0.1.0");
    let upper = sha.to_uppercase();
    for commit in ["main", "HEAD", &sha[..7], &upper] {
        let out = s.release(&s.green(&sha), &["gate", "v0.1.0", commit]);
        assert_refused(&s, &out, "完整的 40 位 SHA");
    }
    let missing = "0".repeat(40);
    let out = s.release(&s.green(&sha), &["gate", "v0.1.0", &missing]);
    assert_refused(&s, &out, "仓库里没有 commit");
}

// ---- release notes ----

#[test]
fn notes_list_the_commit_titles_since_the_last_final_release() {
    let s = Sandbox::new();
    s.commit("backend: before the last release", "0.1.0");
    s.git(&["tag", "v0.1.0"]);
    s.commit("watch: an alert on the sale (#2)", "0.2.0-rc.1");
    s.git(&["tag", "v0.2.0-rc.1"]);
    s.git(&["switch", "-q", "-c", "feature"]);
    s.commit("frontend: a page in the browser (#3)", "0.2.0-rc.1");
    s.git(&["switch", "-q", "main"]);
    s.git(&[
        "merge",
        "-q",
        "--no-ff",
        "feature",
        "-m",
        "Merge pull request #19 from feature",
    ]);
    let sha = s.commit("backend: version 0.2.0", "0.2.0");
    let out = s.release(&s.green(&sha), &["notes", "v0.2.0", &sha]);
    assert!(out.status.success(), "{}", text(&out));
    let notes = String::from_utf8_lossy(&out.stdout);
    assert!(notes.starts_with("theflash 0.2.0。\n"), "{notes}");
    assert!(
        notes.contains("https://github.com/zbtzbtzbt/theFlash/blob/v0.2.0/README.md#下载和运行"),
        "{notes}"
    );
    assert!(notes.contains("## 自 v0.1.0 以来的改动\n"), "{notes}");
    // The order is git log's; the commits here share a timestamp.
    let mut titles: Vec<&str> = notes
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .map(|l| l.rsplit_once(" (").unwrap().0)
        .collect();
    titles.sort();
    assert_eq!(
        titles,
        [
            "backend: version 0.2.0",
            "frontend: a page in the browser (#3)",
            "watch: an alert on the sale (#2)",
        ],
        "{notes}"
    );
}

#[test]
fn notes_of_the_first_release_list_every_commit_and_say_pre_release() {
    let s = Sandbox::new();
    s.commit("repo: skeleton", "0.1.0");
    let sha = s.commit("backend: theflash event (#1)", "0.1.0-rc.1");
    let out = s.release(&s.green(&sha), &["notes", "v0.1.0-rc.1", &sha]);
    assert!(out.status.success(), "{}", text(&out));
    let notes = String::from_utf8_lossy(&out.stdout);
    assert!(
        notes.starts_with("theflash 0.1.0-rc.1，预发布。\n"),
        "{notes}"
    );
    assert!(notes.contains("## 改动\n"), "{notes}");
    assert_eq!(notes.matches("\n- ").count(), 2, "{notes}");
    assert!(notes.contains("\n- repo: skeleton ("), "{notes}");
}

// ---- the assets ----

const TARGETS: [&str; 3] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
];

fn sha256_hex(file: &Path) -> String {
    let out = Command::new("sh")
        .arg("-c")
        .arg("if command -v sha256sum >/dev/null 2>&1; then sha256sum \"$0\"; else shasum -a 256 \"$0\"; fi")
        .arg(file)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    String::from_utf8(out.stdout).unwrap()[..64].to_string()
}

/// `name` with `content`, and `name.sha256` as the release workflow writes it.
fn asset(dir: &Path, name: &str, content: &[u8]) {
    let file = dir.join(name);
    fs::write(&file, content).unwrap();
    fs::write(
        dir.join(format!("{name}.sha256")),
        format!("{}  {name}\n", sha256_hex(&file)),
    )
    .unwrap();
}

fn stage(s: &Sandbox, dist: &Path) -> Output {
    s.command("bash")
        .current_dir(root())
        .arg("scripts/release.sh")
        .arg("stage")
        .arg(dist)
        .output()
        .unwrap()
}

#[test]
fn staging_adds_the_installer_and_refuses_a_missing_file_or_a_wrong_checksum() {
    let s = Sandbox::new();
    let dist = s.path("dist");
    fs::create_dir_all(&dist).unwrap();
    for t in TARGETS {
        asset(&dist, &format!("theflash-{t}.tar.gz"), t.as_bytes());
    }
    let out = stage(&s, &dist);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        fs::read(dist.join("install.sh")).unwrap(),
        fs::read(root().join("scripts/install.sh")).unwrap()
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("全部 8 个产物"),
        "{}",
        text(&out)
    );

    let linux = dist.join("theflash-x86_64-unknown-linux-musl.tar.gz");
    fs::write(&linux, b"changed after its checksum was taken").unwrap();
    let out = stage(&s, &dist);
    assert_refused(
        &s,
        &out,
        "theflash-x86_64-unknown-linux-musl.tar.gz 和 theflash-x86_64-unknown-linux-musl.tar.gz.sha256 对不上",
    );

    fs::remove_file(&linux).unwrap();
    let out = stage(&s, &dist);
    assert_refused(&s, &out, "和产物清单不一致");
}

// ---- the installer ----

/// A release holding a stand-in theflash for `target` that says `version`.
fn fake_release(s: &Sandbox, target: &str, version: &str) -> PathBuf {
    let build = s.path(&format!("build-{target}-{version}"));
    fs::create_dir_all(&build).unwrap();
    write_executable(
        &build.join("theflash"),
        &format!("#!/bin/sh\necho \"theflash {version}\"\n"),
    );
    let release = s.path(&format!("release-{target}-{version}"));
    fs::create_dir_all(&release).unwrap();
    let name = format!("theflash-{target}.tar.gz");
    let out = Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .arg("-czf")
        .arg(release.join(&name))
        .arg("-C")
        .arg(&build)
        .arg("theflash")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let archive = fs::read(release.join(&name)).unwrap();
    asset(&release, &name, &archive);
    release
}

fn install(s: &Sandbox, args: &[&str], platform: (&str, &str), release: &Path) -> Output {
    write_executable(&s.path("bin/uname"), FAKE_UNAME);
    s.command("sh")
        .arg(root().join("scripts/install.sh"))
        .args(args)
        .env("FAKE_UNAME_S", platform.0)
        .env("FAKE_UNAME_M", platform.1)
        .env("FAKE_GH_RELEASE", release)
        .output()
        .unwrap()
}

fn installed_version(s: &Sandbox) -> String {
    let out = Command::new(s.path("home/.local/bin/theflash"))
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn install_puts_this_platforms_build_in_local_bin() {
    for (platform, target) in [
        (("Darwin", "arm64"), "aarch64-apple-darwin"),
        (("Darwin", "x86_64"), "x86_64-apple-darwin"),
        (("Linux", "x86_64"), "x86_64-unknown-linux-musl"),
    ] {
        let s = Sandbox::new();
        let release = fake_release(&s, target, "0.1.0-rc.1");
        let out = install(&s, &["v0.1.0-rc.1"], platform, &release);
        assert!(out.status.success(), "{target}: {}", text(&out));
        assert_eq!(installed_version(&s), "theflash 0.1.0-rc.1\n", "{target}");
        let calls = s.gh_calls();
        assert!(
            calls.starts_with(&format!(
                "release download v0.1.0-rc.1 --repo {REPO} --dir "
            )) && calls.ends_with(&format!(
                " --pattern theflash-{target}.tar.gz --pattern theflash-{target}.tar.gz.sha256\n"
            )),
            "{target}: {calls}"
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("已安装 theflash 0.1.0-rc.1"),
            "{}",
            text(&out)
        );
    }
}

#[test]
fn install_without_a_version_asks_for_the_newest_release_and_upgrades() {
    let s = Sandbox::new();
    let darwin = ("Darwin", "arm64");
    let old = fake_release(&s, "aarch64-apple-darwin", "0.1.0");
    let out = install(&s, &[], darwin, &old);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        s.gh_calls()
            .starts_with(&format!("release download --repo {REPO} --dir ")),
        "{}",
        s.gh_calls()
    );

    let new = fake_release(&s, "aarch64-apple-darwin", "0.2.0");
    let out = install(&s, &[], darwin, &new);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(installed_version(&s), "theflash 0.2.0\n");
}

#[test]
fn install_refuses_an_archive_that_does_not_match_its_checksum() {
    let s = Sandbox::new();
    let linux = ("Linux", "x86_64");
    let good = fake_release(&s, "x86_64-unknown-linux-musl", "0.1.0");
    assert!(install(&s, &[], linux, &good).status.success());

    let bad = fake_release(&s, "x86_64-unknown-linux-musl", "0.2.0");
    fs::write(
        bad.join("theflash-x86_64-unknown-linux-musl.tar.gz.sha256"),
        format!(
            "{}  theflash-x86_64-unknown-linux-musl.tar.gz\n",
            "0".repeat(64)
        ),
    )
    .unwrap();
    let out = install(&s, &[], linux, &bad);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("对不上"),
        "{}",
        text(&out)
    );
    assert_eq!(
        installed_version(&s),
        "theflash 0.1.0\n",
        "the old one was replaced"
    );
}

#[test]
fn install_says_which_platforms_have_a_build() {
    let s = Sandbox::new();
    let release = fake_release(&s, "x86_64-unknown-linux-musl", "0.1.0");
    for platform in [("Linux", "aarch64"), ("FreeBSD", "amd64")] {
        let out = install(&s, &[], platform, &release);
        assert_eq!(out.status.code(), Some(1), "{}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(&format!(
                "没有给 {} {} 构建的版本，只有 macOS arm64、macOS x86_64 和 Linux x86_64",
                platform.0, platform.1
            )),
            "{}",
            text(&out)
        );
    }
    assert_eq!(s.gh_calls(), "");
}
