//! Guards for two of the owner's rules (issue #7), so breaking one fails the
//! build instead of relying on an agent remembering it:
//!
//! - credentials and personal data stay out of the repository: no login
//!   cookie with a value, no Cookie/Authorization header, no valid 18-digit
//!   ID card number, no mainland mobile number in any text file;
//! - nothing that tests run can reach the real damai.cn: every damai host
//!   named outside Markdown is on the blocklist CI resolves to nowhere, and
//!   tests start a binary only through the helpers in `support`
//!   (`support::theflash` / `support::offline` keep theflash off the real
//!   damai.cn; `support::merge_gate` and `support::claims` keep those
//!   commands off the real GitHub).
//!
//! The repository is what git tracks and the untracked files it does not
//! ignore, so a new file is checked before it is committed. The checks read
//! text, not code: what they still miss is listed in
//! docs/decisions/0003-tests-cannot-reach-damai.md.
//!
//! What counts as a credential or personal data is not read here. It is
//! read by `theflash::inspect::redact`, which `theflash inspect record`
//! runs over every response before it writes it, so what may be recorded
//! and what may be committed cannot drift apart (#71). The forms each rule
//! covers are tested next to that code; this file tests the walk over the
//! repository, the damai hosts and the allowlist.
//!
//! The sample values below are built from pieces so that this file does not
//! contain what it looks for. Findings name the file, line and rule, never
//! the value.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use theflash::inspect::redact;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rule {
    /// One of the owner's rules about credentials and personal data. They
    /// are read in one place, `theflash::inspect::redact`, which is what
    /// `theflash inspect record` cleans a response with before it saves
    /// it; the forms each rule covers are tested there (#71).
    Data(redact::Rule),
    /// A damai host CI does not block, which is this file's own rule.
    Domain,
}

impl Rule {
    fn name(self) -> &'static str {
        match self {
            Rule::Data(rule) => rule.id(),
            Rule::Domain => "domain",
        }
    }

    fn from_name(name: &str) -> Option<Rule> {
        redact::Rule::from_id(name).map(Rule::Data)
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Finding {
    path: String,
    line: usize,
    rule: Rule,
    /// What was found, without the sensitive value.
    what: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}: [{}] {}",
            self.path,
            self.line,
            self.rule.name(),
            self.what
        )
    }
}

// ---- what counts as a credential or personal data ----

/// What in a text file is a credential or personal data, by line number.
/// The rules are read by `theflash::inspect::redact`, which is also what
/// `theflash inspect record` runs over a response before it writes it:
/// one reading of the owner's rules, so a fixture that passes there
/// cannot fail here (#71). A cookie or header split across lines is
/// reported on the line of its name.
fn sensitive_in_text(text: &str) -> Vec<(usize, Rule, String)> {
    redact::sensitive(text)
        .into_iter()
        .map(|hit| (hit.line, Rule::Data(hit.rule), hit.what))
        .collect()
}

// ---- damai hosts named in the repository ----

/// Every `*.damai.cn`-style host named in a line, as written (lowercased).
/// The escape in front of a host is not part of it: `%2F` in a URL-encoded
/// link, `\t` or `\u002F` in a string literal.
fn damai_hosts_in_line(line: &str) -> BTreeSet<String> {
    let lower = line.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut found = BTreeSet::new();
    for tld in ["cn", "com", "net", "org"] {
        let needle = format!("damai.{tld}");
        for (at, _) in lower.match_indices(&needle) {
            let end = at + needle.len();
            if b.get(end).is_some_and(u8::is_ascii_alphanumeric) {
                continue;
            }
            let mut start = at;
            while start > 0
                && (b[start - 1].is_ascii_alphanumeric() || matches!(b[start - 1], b'-' | b'.'))
            {
                start -= 1;
            }
            let host = lower[after_escape(b, start, at)..end].trim_start_matches(['.', '-']);
            found.insert(host.to_string());
        }
    }
    found
}

/// Where a host read back to `start` really starts, once the escape that
/// runs into it is left out: `%XX`, `\t`, `\xXX` and `\uXXXX` end in letters
/// or digits. Never past `at`, where the host's `damai.cn` begins.
fn after_escape(b: &[u8], mut start: usize, at: usize) -> usize {
    let hex = |i: usize, n: usize| i + n <= at && b[i..i + n].iter().all(u8::is_ascii_hexdigit);
    match start.checked_sub(1).map(|i| b[i]) {
        Some(b'%') => {
            // `%25` is an encoded `%`, so `%252F` is `%2F` encoded twice.
            while hex(start, 2) {
                let percent = &b[start..start + 2] == b"25";
                start += 2;
                if !percent {
                    break;
                }
            }
        }
        Some(b'\\') => {
            start += match b.get(start) {
                Some(b'u') if hex(start + 1, 4) => 5,
                Some(b'x') if hex(start + 1, 2) => 3,
                _ if start < at => 1,
                _ => 0,
            }
        }
        _ => {}
    }
    start
}

/// Markdown only talks about hosts. Every other text file can make a
/// request or feed a host to one: code, config, scripts, and the fixtures
/// the mock server replays to the program.
fn is_markdown(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|e| e == "md")
}

/// The hosts of scripts/damai-hosts.txt.
fn blocked_hosts(root: &Path) -> BTreeSet<String> {
    let path = root.join("scripts/damai-hosts.txt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    parse_host_list(&text)
}

fn parse_host_list(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap().trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty())
        .collect()
}

// ---- the allowlist ----

#[derive(Debug, Clone)]
struct Allowed {
    path: String,
    rule: Rule,
}

/// Lines of `path | rule | reason`. A confirmed false positive is let
/// through for one rule in one file, and the reason says why it is safe.
fn parse_allowlist(text: &str) -> Vec<Allowed> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, '|').map(str::trim).collect();
        let [path, rule, reason] = parts[..] else {
            panic!(
                "allowlist line {}: want `path | rule | reason`: {line}",
                n + 1
            );
        };
        assert!(
            !reason.is_empty(),
            "allowlist line {}: every entry needs a reason",
            n + 1
        );
        let rule = Rule::from_name(rule).unwrap_or_else(|| {
            panic!(
                "allowlist line {}: unknown rule {rule:?} (cookie, header, id-card, phone)",
                n + 1
            )
        });
        out.push(Allowed {
            path: path.to_string(),
            rule,
        });
    }
    out
}

// ---- scanning files ----

fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0)
}

/// The text of `file`, or None when it is binary or deleted in this
/// checkout.
fn read_text(root: &Path, file: &str) -> Option<String> {
    let bytes = std::fs::read(root.join(file)).ok()?;
    is_text(&bytes).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

/// What is wrong with `files` (paths relative to `root`): findings not on
/// the allowlist, and allowlist entries that no longer match anything.
fn check_files(root: &Path, files: &[String], allow: &[Allowed]) -> Vec<String> {
    let mut findings = Vec::new();
    let mut used = vec![false; allow.len()];
    for file in files {
        let Some(text) = read_text(root, file) else {
            continue;
        };
        for (line, rule, what) in sensitive_in_text(&text) {
            if let Some(k) = allow.iter().position(|a| a.path == *file && a.rule == rule) {
                used[k] = true;
                continue;
            }
            findings.push(Finding {
                path: file.clone(),
                line,
                rule,
                what,
            });
        }
    }
    findings.sort();
    let mut problems: Vec<String> = findings.iter().map(Finding::to_string).collect();
    for (a, used) in allow.iter().zip(used) {
        if !used {
            problems.push(format!(
                "allowlist: `{} | {}` matches nothing; remove it",
                a.path,
                a.rule.name()
            ));
        }
    }
    problems
}

/// Damai hosts named in `files`, Markdown aside, that are not in `blocked`.
fn unblocked_hosts(root: &Path, files: &[String], blocked: &BTreeSet<String>) -> Vec<Finding> {
    let mut out = Vec::new();
    for file in files.iter().filter(|f| !is_markdown(f)) {
        let Some(text) = read_text(root, file) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            for host in damai_hosts_in_line(line) {
                if !blocked.contains(&host) {
                    out.push(Finding {
                        path: file.clone(),
                        line: i + 1,
                        rule: Rule::Domain,
                        what: format!("{host} is not in scripts/damai-hosts.txt"),
                    });
                }
            }
        }
    }
    out.sort();
    out
}

/// Lines of Rust files outside backend/tests/support/mod.rs that name a
/// test binary through one of cargo's `CARGO_BIN_EXE` variables. A test
/// that starts a binary itself goes around the helpers that keep it
/// offline, so only support may name one; tests/offline_guard.rs shows that
/// `support::theflash` does keep it offline.
fn binaries_started_outside_support(root: &Path, files: &[String]) -> Vec<String> {
    let needle = concat!("CARGO_BIN", "_EXE_");
    let mut out = Vec::new();
    for file in files
        .iter()
        .filter(|f| f.ends_with(".rs") && *f != "backend/tests/support/mod.rs")
    {
        let Some(text) = read_text(root, file) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if line.contains(needle) {
                out.push(format!("{file}:{}", i + 1));
            }
        }
    }
    out
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// git in the repository at `root`. A git hook that runs the tests sets
/// GIT_DIR and friends, which would point git at another repository.
fn git_in(root: &Path) -> Command {
    let mut git = Command::new("git");
    git.arg("-C").arg(root);
    for v in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
    ] {
        git.env_remove(v);
    }
    git
}

/// The files git tracks, and the ones `git add -A` would add: untracked
/// files that no ignore rule excludes. A new file is checked before it is
/// committed, so `make check` is not green just because it is not yet
/// added (#21).
fn repo_files(root: &Path) -> Vec<String> {
    let out = git_in(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .unwrap_or_else(|e| panic!("git ls-files: {e}"));
    assert!(
        out.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let files: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    assert!(
        !files.is_empty(),
        "git tracks no file under {}",
        root.display()
    );
    files
}

fn load_allowlist(root: &Path) -> Vec<Allowed> {
    let path = root.join("backend/tests/repo_guard_allowlist.txt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    parse_allowlist(&text)
}

// ---- the guards on this repository ----

#[test]
fn tracked_files_hold_no_credentials_or_personal_data() {
    let root = repo_root();
    let problems = check_files(&root, &repo_files(&root), &load_allowlist(&root));
    assert!(
        problems.is_empty(),
        "credentials or personal data in the repository. Remove them, or, for a \
         confirmed false positive, add `path | rule | reason` to \
         backend/tests/repo_guard_allowlist.txt. The values are not printed.\n{}",
        problems.join("\n")
    );
}

#[test]
fn every_damai_host_named_outside_markdown_is_blocked_in_ci() {
    let root = repo_root();
    let blocked = blocked_hosts(&root);
    let found = unblocked_hosts(&root, &repo_files(&root), &blocked);
    assert!(
        found.is_empty(),
        "these files name damai hosts that CI does not block. Add each to \
         scripts/damai-hosts.txt, so a test that reaches for it fails:\n{}",
        found
            .iter()
            .map(Finding::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn tests_start_binaries_only_through_support() {
    let root = repo_root();
    let found = binaries_started_outside_support(&root, &repo_files(&root));
    assert!(
        found.is_empty(),
        "these lines start a test binary themselves. Start it through a helper \
         in backend/tests/support/mod.rs (`support::theflash` keeps theflash \
         from reaching the real damai.cn; `support::merge_gate` and \
         `support::claims` keep those commands from reaching the real GitHub):\n{}",
        found.join("\n")
    );
}

/// The frontend tests run the same binary from Node, where there is no
/// `support::offline` to point HOME at a test directory. #4: a `beforeAll`
/// that built its fixtures with `loadEvent` ran under the real HOME and
/// created the risk-control breaker's lock file in the owner's own state
/// directory. A test that runs the binary now has to take `tempHome`.
#[test]
fn frontend_tests_run_theflash_only_under_a_temporary_home() {
    let root = repo_root();
    let without: Vec<String> = repo_files(&root)
        .into_iter()
        .filter(|f| f.starts_with("frontend/src/") && f.ends_with(".test.ts"))
        .filter(|f| {
            let text = std::fs::read_to_string(root.join(f)).unwrap_or_default();
            // loadEvent and execFile are the two ways a test starts it.
            (text.contains("loadEvent") || text.contains("execFile")) && !text.contains("tempHome(")
        })
        .collect();
    assert!(
        without.is_empty(),
        "这些前端测试会运行 theflash，但没有用 test-support 的 tempHome()，\
         可能读写真实用户的状态目录（风控熔断就在那里）。自己拼一个临时 HOME \
         也不行：状态目录在哪只写在 tempHome() 一处，否则下次改路径会漏掉这里。\
         {without:?}"
    );
}

/// The `*.md` targets of the markdown links on one line, in order.
fn markdown_links_to_md(line: &str) -> Vec<&str> {
    line.match_indices("](")
        .filter_map(|(at, _)| {
            let rest = &line[at + 2..];
            let end = rest.find(')')?;
            Some(&rest[..end]).filter(|t| t.ends_with(".md"))
        })
        .collect()
}

/// Several sessions rebase onto main at the same time, and the two index
/// tables are where their changes land together. PR #28 hit the conflict;
/// resolving it by keeping one side would have dropped the other side's row
/// without failing any check, because nothing links the files to the index.
#[test]
fn every_decision_record_and_subsystem_reference_is_in_its_index() {
    let root = repo_root();
    let files = repo_files(&root);
    let mut missing: Vec<String> = Vec::new();
    for dir in ["docs/decisions", "docs/subsystems"] {
        let index_path = format!("{dir}/README.md");
        let index = std::fs::read_to_string(root.join(&index_path))
            .unwrap_or_else(|e| panic!("{index_path}: {e}"));
        for file in &files {
            let Some(name) = file
                .strip_prefix(&format!("{dir}/"))
                .filter(|n| n.ends_with(".md") && *n != "README.md" && !n.contains('/'))
            else {
                continue;
            };
            // The row for this document, not a mention of it inside another
            // row: 0004's row links 0005 where it says 0005 supersedes part
            // of it, and that is not an entry for 0005. A row describes the
            // document it links to first; later links in it are references.
            let listed = index
                .lines()
                .filter(|l| l.trim_start().starts_with('|'))
                .any(|l| markdown_links_to_md(l).first() == Some(&name));
            if !listed {
                missing.push(format!("{file} has no row of its own in {index_path}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "这些文档没有出现在它所在目录的索引表里，后来的 agent 读索引就找不到它们。\
         在索引表里加一行（rebase 解决冲突时两边的行都要留下）：\n{}",
        missing.join("\n")
    );
}

/// A leftover marker in Rust or TypeScript stops the build, but in Markdown
/// it is silently committed. The pieces are joined here so this file is not
/// its own finding.
#[test]
fn no_file_in_the_repository_keeps_a_merge_conflict_marker() {
    let root = repo_root();
    let open = "<".repeat(7);
    let close = ">".repeat(7);
    let mut found: Vec<String> = Vec::new();
    for file in repo_files(&root) {
        let Ok(text) = std::fs::read_to_string(root.join(&file)) else {
            continue; // binary or unreadable: the other guards cover those
        };
        for (i, line) in text.lines().enumerate() {
            if line.starts_with(&open) || line.starts_with(&close) {
                found.push(format!("{file}:{}", i + 1));
            }
        }
    }
    assert!(
        found.is_empty(),
        "这些行是没解决完的合并冲突标记：\n{}",
        found.join("\n")
    );
}

// ---- the scanner itself ----

#[test]
fn a_test_that_starts_a_binary_itself_fails_even_where_it_also_uses_the_helper() {
    // #21: the guard asked only whether a file named both the binary and
    // `support::offline(`, so one start through the helper hid a direct one.
    let bin = concat!("CARGO_BIN", "_EXE_theflash");
    let mixed = [
        "mod support;",
        "fn a() { let mut c = Command::new(\"x\"); support::offline(&mut c); }",
        &format!("fn b() {{ Command::new(env!(\"{bin}\")).output(); }}"),
        "",
    ]
    .join("\n");
    let (root, files) = temp_repo(
        "binaries",
        &[
            ("backend/tests/mixed.rs", mixed),
            (
                "backend/tests/support/mod.rs",
                format!("let c = Command::new(env!(\"{bin}\"));\n"),
            ),
            (
                "backend/tests/clean.rs",
                "fn c() { support::theflash(\"x\"); }\n".to_string(),
            ),
            (
                "docs/notes.md",
                format!("tests reach {bin} through support\n"),
            ),
        ],
    );
    let got = binaries_started_outside_support(&root, &files);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(got, ["backend/tests/mixed.rs:3"]);
}

#[test]
fn files_not_yet_added_to_git_are_checked_too() {
    // #21: the guards read `git ls-files` alone, so `make check` stayed
    // green until the new file with the problem was committed.
    let root = std::env::temp_dir().join(format!("theflash-repo-guard-{}-git", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = |args: &[&str]| {
        let out = git_in(&root).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    for (path, text) in [
        ("tracked.txt", "a\n"),
        ("new.txt", "b\n"),
        (".gitignore", "ignored.txt\n"),
        ("ignored.txt", "c\n"),
    ] {
        std::fs::write(root.join(path), text).unwrap();
    }
    git(&["add", "tracked.txt"]);
    let mut files = repo_files(&root);
    files.sort();
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(files, [".gitignore", "new.txt", "tracked.txt"]);
}

fn join(parts: &[&str]) -> String {
    parts.concat()
}

/// A well-formed ID card number, split so this file does not contain it.
fn sample_id() -> String {
    join(&["110105194912", "31002X"])
}

fn sample_mobile() -> String {
    join(&["138", "0013", "8000"])
}

#[test]
fn a_credential_in_any_of_its_forms_fails_and_names_the_line_of_the_name() {
    // #21: the scan went line by line, so a value on the line after its
    // name was never seen. #71: the files below are what `theflash inspect
    // record` writes, so the walk over the repository has to read every
    // form that command cleans — the two now read the same rules.
    let (root, files) = temp_repo(
        "multiline",
        &[
            (
                "backend/testdata/damai/a7_multiline.json",
                format!("{{\n  \"{}\":\n    \"7a1f3c9d2b4e_176\"\n}}\n", "_m_h5_tk"),
            ),
            (
                "backend/testdata/damai/b.txt",
                format!("x\n{}\n  = abc123\n", "unb"),
            ),
            (
                "backend/testdata/damai/c.json",
                "{\"Cookie\"\n  : \"a=b\"}\n".to_string(),
            ),
            (
                "backend/testdata/damai/d_export.json",
                format!(
                    "[\n  {{\"domain\": \".damai.cn\",\n   \"name\": \"{}\",\n   \"value\": \"abc123\"}}\n]\n",
                    "unb"
                ),
            ),
            (
                "backend/testdata/damai/e_cookies.txt",
                format!(
                    ".damai.cn\tTRUE\t/\tFALSE\t1760000000\t{}\tabc123\n",
                    "munb"
                ),
            ),
            (
                "backend/testdata/damai/f_args.js",
                format!("jar.set('{}', 'abc_123');\n", "sgcookie"),
            ),
        ],
    );
    let problems = check_files(&root, &files, &[]);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        problems,
        [
            "backend/testdata/damai/a7_multiline.json:2: [cookie] cookie _m_h5_tk with a value",
            "backend/testdata/damai/b.txt:2: [cookie] cookie unb with a value",
            "backend/testdata/damai/c.json:1: [header] cookie header",
            "backend/testdata/damai/d_export.json:3: [cookie] cookie unb with a value",
            "backend/testdata/damai/e_cookies.txt:1: [cookie] cookie munb with a value",
            "backend/testdata/damai/f_args.js:1: [cookie] cookie sgcookie with a value",
        ]
    );
}

#[test]
fn damai_hosts_are_read_as_written() {
    let hosts = |l: &str| damai_hosts_in_line(l).into_iter().collect::<Vec<_>>();
    assert_eq!(
        hosts("https://detail.damai.cn/item.htm"),
        ["detail.damai.cn"]
    );
    assert_eq!(hosts("DETAIL.Damai.CN"), ["detail.damai.cn"]);
    assert_eq!(hosts("damai.cn's search"), ["damai.cn"]);
    assert_eq!(hosts("\"m.damai.cn\""), ["m.damai.cn"]);
    let nested = join(&["a.b-c.", "damai.cn"]);
    assert_eq!(hosts(&format!("https://{nested}:443/x")), [nested]);
    assert_eq!(
        hosts("detail.damai.cn.evil.example"),
        ["detail.damai.cn"],
        "the host that is damai's, not the whole string"
    );
    let lookalike = join(&["fake", "damai.cn"]);
    assert_eq!(hosts(&lookalike), [lookalike]);
    assert_eq!(
        hosts("damai.cnx and damai.html and crate::damai"),
        Vec::<String>::new()
    );
}

#[test]
fn damai_hosts_are_read_without_the_escape_in_front_of_them() {
    // #21: the letters and digits of an escape ran into the host: the `t`
    // of a string's `\t`, the `2F` of a URL-encoded `/`.
    let hosts = |l: &str| damai_hosts_in_line(l).into_iter().collect::<Vec<_>>();
    assert_eq!(hosts("\"0.0.0.0\\tdamai.cn\""), ["damai.cn"]);
    assert_eq!(hosts("\"a\\ndetail.damai.cn\""), ["detail.damai.cn"]);
    assert_eq!(hosts("https%3A%2F%2Ft.damai.cn%2Fyep"), ["t.damai.cn"]);
    assert_eq!(
        hosts("https%253A%252F%252Fdetail.damai.cn"),
        ["detail.damai.cn"]
    );
    assert_eq!(hosts("\"https:\\u002F\\u002Fm.damai.cn\""), ["m.damai.cn"]);
    assert_eq!(hosts("'\\x2Fsearch.damai.cn'"), ["search.damai.cn"]);
    // `%da` is not an escape of the host it starts: the host is all there.
    assert_eq!(hosts("LIKE '%damai.cn'"), ["damai.cn"]);
}

#[test]
fn damai_hosts_in_every_file_but_markdown_must_be_blocked() {
    // #21: only ten source extensions were read, so a host in a .json
    // config or in a fixture the mock server replays was never checked.
    let host = join(&["order.", "damai", ".cn"]);
    let (root, files) = temp_repo(
        "domain-any-file",
        &[
            (
                "frontend/config.json",
                format!("{{\"api\": \"https://{host}/create\"}}\n"),
            ),
            (
                "backend/testdata/damai/page.html",
                format!("<a href=\"//{host}/list\">\n"),
            ),
            ("scripts/probe.py", format!("URL = 'https://{host}'\n")),
            ("docs/notes.md", format!("prose about {host}\n")),
            ("backend/testdata/damai/shot.png", format!("\0{host}")),
        ],
    );
    let got: Vec<String> = unblocked_hosts(&root, &files, &parse_host_list("damai.cn"))
        .iter()
        .map(Finding::to_string)
        .collect();
    let _ = std::fs::remove_dir_all(&root);
    let not_listed = format!("[domain] {host} is not in scripts/damai-hosts.txt");
    assert_eq!(
        got,
        [
            format!("backend/testdata/damai/page.html:1: {not_listed}"),
            format!("frontend/config.json:1: {not_listed}"),
            format!("scripts/probe.py:1: {not_listed}"),
        ]
    );
}

fn temp_repo(name: &str, files: &[(&str, String)]) -> (PathBuf, Vec<String>) {
    let root =
        std::env::temp_dir().join(format!("theflash-repo-guard-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, content) in files {
        let at = root.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, content).unwrap();
    }
    (root, files.iter().map(|(p, _)| p.to_string()).collect())
}

#[test]
fn a_planted_credential_fails_and_names_the_file_and_line() {
    let (root, files) = temp_repo(
        "planted",
        &[
            (
                "backend/testdata/damai/a.json",
                format!("{{\n  \"ok\": 1,\n  \"{}\": \"abc123\"\n}}\n", "_m_h5_tk"),
            ),
            (
                "backend/testdata/damai/b.txt",
                format!("x\ny\n{}\n", sample_id()),
            ),
            (
                "backend/testdata/damai/c.txt",
                format!("phone {}\n", sample_mobile()),
            ),
            (
                "backend/testdata/damai/d.txt",
                "Set-Cookie: sid=1\n".to_string(),
            ),
            (
                "backend/testdata/damai/clean.txt",
                "nothing here\n".to_string(),
            ),
            (
                "backend/testdata/damai/bin.dat",
                format!("\0{}", sample_mobile()),
            ),
        ],
    );
    let problems = check_files(&root, &files, &[]);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        problems,
        [
            "backend/testdata/damai/a.json:3: [cookie] cookie _m_h5_tk with a value",
            "backend/testdata/damai/b.txt:3: [id-card] an 18-digit ID card number",
            "backend/testdata/damai/c.txt:1: [phone] an 11-digit mobile number",
            "backend/testdata/damai/d.txt:1: [header] set-cookie header",
        ]
    );
    for p in &problems {
        for secret in ["abc123", &sample_id(), &sample_mobile()] {
            assert!(!p.contains(secret), "a finding prints the value: {p}");
        }
    }
}

#[test]
fn removing_the_planted_data_passes() {
    let (root, files) = temp_repo(
        "clean",
        &[("backend/testdata/damai/a.json", "{\"ok\": 1}\n".to_string())],
    );
    let problems = check_files(&root, &files, &[]);
    let _ = std::fs::remove_dir_all(&root);
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn the_allowlist_lets_one_rule_through_in_one_file_and_must_stay_current() {
    let (root, files) = temp_repo(
        "allow",
        &[
            ("a.txt", format!("number {}\n", sample_mobile())),
            ("b.txt", format!("number {}\n", sample_mobile())),
        ],
    );
    let allow = parse_allowlist(
        "# comment\n\na.txt | phone | a made-up number from the vendor's docs\n\
         a.txt | cookie | nothing like this is in a.txt\n",
    );
    let problems = check_files(&root, &files, &allow);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        problems,
        [
            "b.txt:1: [phone] an 11-digit mobile number",
            "allowlist: `a.txt | cookie` matches nothing; remove it",
        ]
    );
}

#[test]
#[should_panic(expected = "every entry needs a reason")]
fn an_allowlist_entry_without_a_reason_is_refused() {
    parse_allowlist("a.txt | phone |  \n");
}

#[test]
#[should_panic(expected = "unknown rule")]
fn an_allowlist_entry_with_an_unknown_rule_is_refused() {
    parse_allowlist("a.txt | nonsense | why\n");
}

#[test]
fn a_damai_host_missing_from_the_blocklist_fails() {
    let new_host = join(&["new.", "damai", ".cn"]);
    let (root, files) = temp_repo(
        "domain",
        &[
            (
                "src/a.rs",
                format!("const A: &str = \"https://{new_host}/x\";\n"),
            ),
            ("src/b.rs", "// detail.damai.cn is listed\n".to_string()),
            (
                "docs/c.md",
                format!("prose about {new_host} is not source code\n"),
            ),
        ],
    );
    let blocked = parse_host_list(&["# hosts", "damai.cn", "detail.damai.cn # comment"].join("\n"));
    let got: Vec<String> = unblocked_hosts(&root, &files, &blocked)
        .iter()
        .map(Finding::to_string)
        .collect();
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        got,
        [format!(
            "src/a.rs:1: [domain] {new_host} is not in scripts/damai-hosts.txt"
        )]
    );
}

#[test]
fn the_blocklist_covers_the_hosts_the_decision_records_name() {
    let blocked = blocked_hosts(&repo_root());
    for host in [
        "damai.cn",
        "detail.damai.cn",
        "m.damai.cn",
        "search.damai.cn",
        "mtop.damai.cn",
    ] {
        assert!(blocked.contains(host), "{host}");
    }
}
