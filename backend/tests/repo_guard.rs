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
//! The sample values below are built from pieces so that this file does not
//! contain what it looks for. Findings name the file, line and rule, never
//! the value.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rule {
    Cookie,
    Header,
    IdCard,
    Phone,
    Domain,
}

impl Rule {
    fn name(self) -> &'static str {
        match self {
            Rule::Cookie => "cookie",
            Rule::Header => "header",
            Rule::IdCard => "id-card",
            Rule::Phone => "phone",
            Rule::Domain => "domain",
        }
    }

    fn from_name(name: &str) -> Option<Rule> {
        [Rule::Cookie, Rule::Header, Rule::IdCard, Rule::Phone]
            .into_iter()
            .find(|r| r.name() == name)
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

/// Cookie names of Taobao and damai.cn logins. Longer names first.
const COOKIE_NAMES: [&str; 7] = [
    "_m_h5_tk_enc",
    "_m_h5_tk",
    "cookie2",
    "munb",
    "unb",
    "sgcookie",
    "_tb_token_",
];

const HEADER_NAMES: [&str; 3] = ["set-cookie", "cookie", "authorization"];

/// First two digits of an ID card number: the provinces, and Hong Kong,
/// Macao, Taiwan and abroad.
const REGIONS: [u32; 36] = [
    11, 12, 13, 14, 15, 21, 22, 23, 31, 32, 33, 34, 35, 36, 37, 41, 42, 43, 44, 45, 46, 50, 51, 52,
    53, 54, 61, 62, 63, 64, 65, 71, 81, 82, 83, 91,
];

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// A cookie value as it is written in a header or a JSON dump. Excludes
/// placeholders such as `<redacted>`, `***`, `${TOKEN}` and prose.
fn is_value_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'%' | b'-' | b'_' | b'.' | b'+' | b'/' | b'~')
}

fn is_quote(b: u8) -> bool {
    matches!(b, b'"' | b'\'')
}

/// The first byte from `i` on that is not a space, a tab or a line break:
/// a name, its separator and its value may sit on different lines (#21).
fn skip_space(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// Whether a value, quoted or not, starts at `i`.
fn value_at(b: &[u8], mut i: usize) -> bool {
    if b.get(i).copied().is_some_and(is_quote) {
        i += 1;
    }
    b.get(i).copied().is_some_and(is_value_byte)
}

/// Whether a string literal starting at `i` holds a value rather than
/// another name. After a comma a name is the next item of a list, as in
/// `["unb", "munb"]` or `["cookie", "set-cookie"]`; a value has a digit or
/// a sign in it, as in `"Bearer abc"`, `"a=b"` or `"2200000000000"`.
fn literal_value_at(b: &[u8], i: usize) -> bool {
    let Some(&quote) = b.get(i).filter(|q| is_quote(**q)) else {
        return false;
    };
    let rest = &b[i + 1..];
    let len = rest
        .iter()
        .position(|c| *c == quote || *c == b'\n')
        .unwrap_or(rest.len());
    let literal = &rest[..len];
    let is_name = |n: &&str| literal.eq_ignore_ascii_case(n.as_bytes());
    literal.first().copied().is_some_and(is_value_byte)
        && literal
            .iter()
            .any(|c| !(c.is_ascii_alphabetic() || matches!(c, b'_' | b'-')))
        && !COOKIE_NAMES.iter().chain(&HEADER_NAMES).any(is_name)
}

/// Whether the quoted name at `at..at + len` is the value of a `name` key
/// in an object whose `value` key holds a quoted value:
/// `{"name": "unb", "value": "…"}`, the way HAR files, Playwright's
/// storageState, Puppeteer and browser extensions save cookies and
/// headers. `b` is the text in lowercase.
fn named_in_object(b: &[u8], at: usize, len: usize) -> bool {
    let end = at + len;
    if at == 0 || !is_quote(b[at - 1]) || b.get(end) != Some(&b[at - 1]) {
        return false;
    }
    // Back from the opening quote: a `:`, then the key `name`, maybe quoted.
    let back = |mut k: usize| {
        while k > 0 && b[k - 1].is_ascii_whitespace() {
            k -= 1;
        }
        k
    };
    let mut k = back(at - 1);
    if k == 0 || b[k - 1] != b':' {
        return false;
    }
    k = back(k - 1);
    if k > 0 && is_quote(b[k - 1]) {
        k -= 1;
    }
    if k < 4 || &b[k - 4..k] != b"name" || (k > 4 && is_name_byte(b[k - 5])) {
        return false;
    }
    // The object runs from the `{` before the name to the `}` after it.
    let (Some(lo), Some(hi)) = (
        b[..at].iter().rposition(|c| *c == b'{'),
        b[end..].iter().position(|c| *c == b'}').map(|p| end + p),
    ) else {
        return false;
    };
    let object = &b[lo..hi];
    (1..object.len()).any(|i| {
        if !object[i..].starts_with(b"value") || is_name_byte(object[i - 1]) {
            return false;
        }
        let mut j = i + "value".len();
        if object.get(j).copied().is_some_and(is_quote) {
            j += 1;
        }
        j = skip_space(object, j);
        if object.get(j) != Some(&b':') {
            return false;
        }
        j = skip_space(object, j + 1);
        object.get(j).copied().is_some_and(is_quote) && value_at(object, j)
    })
}

/// Where login cookies are written with a value, and their names. Spaces
/// and line breaks may sit between the parts:
/// - `name=value`, as in a Cookie header or `document.cookie`;
/// - `"name": "value"`, a JSON key or a JS or Python dict entry;
/// - `"name", "value"`, adjacent arguments or a pair, as in
///   `Cookie::new("unb", "…")`, when the second is not just another name;
/// - `{"name": "unb", …, "value": "…"}` (see `named_in_object`).
fn cookies_with_values(text: &str) -> Vec<(usize, &'static str)> {
    let b = text.as_bytes();
    let lower = text.to_ascii_lowercase();
    let mut found = Vec::new();
    for name in COOKIE_NAMES {
        for (at, _) in text.match_indices(name) {
            if at > 0 && is_name_byte(b[at - 1]) {
                continue;
            }
            let mut i = at + name.len();
            let quoted = b.get(i).copied().is_some_and(is_quote);
            if quoted {
                i += 1;
            }
            i = skip_space(b, i);
            let with_value = match b.get(i) {
                Some(b'=') => value_at(b, skip_space(b, i + 1)),
                Some(b':') if quoted => value_at(b, skip_space(b, i + 1)),
                Some(b',') if quoted => literal_value_at(b, skip_space(b, i + 1)),
                _ => false,
            };
            if with_value || named_in_object(lower.as_bytes(), at, name.len()) {
                found.push((at, name));
            }
        }
    }
    found
}

/// Where `Cookie`, `Set-Cookie` and `Authorization` are written as a
/// header, and their names:
/// - `Cookie:`, also as a JSON key;
/// - `"Authorization", "Bearer …"`, the name and a literal value as
///   adjacent arguments, as in `.header("Authorization", "Bearer …")`;
/// - `{"name": "cookie", "value": "…"}` in a HAR file.
fn headers(text: &str) -> Vec<(usize, &'static str)> {
    let lower = text.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut found = Vec::new();
    for name in HEADER_NAMES {
        for (at, _) in lower.match_indices(name) {
            if at > 0 && (b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'-') {
                continue;
            }
            let mut i = at + name.len();
            if b.get(i).copied().is_some_and(is_quote) {
                i += 1;
            }
            i = skip_space(b, i);
            let as_header = match b.get(i) {
                Some(b':') => true,
                Some(b',') => literal_value_at(b, skip_space(b, i + 1)),
                _ => false,
            };
            if as_header || named_in_object(b, at, name.len()) {
                found.push((at, name));
            }
        }
    }
    found
}

/// The login cookie on a cookies.txt line, the Netscape format `curl -c`
/// and browser extensions write: seven tab-separated fields, the name in
/// the sixth and the value in the seventh.
fn cookies_txt_name(line: &str) -> Option<&'static str> {
    let fields: Vec<&str> = line.split('\t').collect();
    let [_, _, _, _, _, name, value] = fields[..] else {
        return None;
    };
    if !value.bytes().next().is_some_and(is_value_byte) {
        return None;
    }
    COOKIE_NAMES.into_iter().find(|n| *n == name)
}

fn is_mobile(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() == 11
        && b.iter().all(u8::is_ascii_digit)
        && b[0] == b'1'
        && (b'3'..=b'9').contains(&b[1])
}

/// 18 characters with a real region, a real birth date and the right check
/// character, so an 18-digit order or item number rarely qualifies.
fn is_id_card(t: &str) -> bool {
    let b = t.as_bytes();
    if b.len() != 18
        || !b[..17].iter().all(u8::is_ascii_digit)
        || !(b[17].is_ascii_digit() || b[17] == b'X' || b[17] == b'x')
    {
        return false;
    }
    let num = |r: std::ops::Range<usize>| t[r].parse::<u32>().unwrap();
    let (year, month, day) = (num(6..10), num(10..12), num(12..14));
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if !REGIONS.contains(&num(0..2)) || !(1900..=2100).contains(&year) || !(1..=days).contains(&day)
    {
        return false;
    }
    const WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    let sum: u32 = b[..17]
        .iter()
        .zip(WEIGHTS)
        .map(|(d, w)| u32::from(d - b'0') * w)
        .sum();
    b"10X98765432"[(sum % 11) as usize] == b[17].to_ascii_uppercase()
}

/// What in a text file is a credential or personal data, by line number.
/// A cookie or header split across lines is reported on the line of its
/// name.
fn sensitive_in_text(text: &str) -> BTreeSet<(usize, Rule, String)> {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let line_of = |at: usize| starts.partition_point(|s| *s <= at);
    let cookie = |name: &str| format!("cookie {name} with a value");
    let mut out = BTreeSet::new();
    for (at, name) in cookies_with_values(text) {
        out.insert((line_of(at), Rule::Cookie, cookie(name)));
    }
    for (at, name) in headers(text) {
        out.insert((line_of(at), Rule::Header, format!("{name} header")));
    }
    for (i, line) in text.lines().enumerate() {
        if let Some(name) = cookies_txt_name(line) {
            out.insert((i + 1, Rule::Cookie, cookie(name)));
        }
        let tokens = || {
            line.split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|t| !t.is_empty())
        };
        if tokens().any(is_id_card) {
            out.insert((i + 1, Rule::IdCard, "an 18-digit ID card number".into()));
        }
        if tokens().any(is_mobile) {
            out.insert((i + 1, Rule::Phone, "an 11-digit mobile number".into()));
        }
    }
    out
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
    let without: Vec<String> = tracked_files(&root)
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
         会读写真实用户的状态目录（风控熔断就在那里）：{without:?}"
    );
}

// ---- the scanner itself ----

fn found(text: &str) -> Vec<Rule> {
    sensitive_in_text(text)
        .into_iter()
        .map(|(_, r, _)| r)
        .collect()
}

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
fn login_cookies_with_values_are_found_in_every_form() {
    for name in COOKIE_NAMES {
        for line in [
            format!("{name}=abc123"),
            format!("Cookie: a=b; {name}=abc123; c=d"),
            format!("  \"{name}\": \"abc123\","),
            format!("{name} = 'abc123'"),
            format!("x_m_h5_tk=1; {name}=abc%20123"),
        ] {
            assert!(found(&line).contains(&Rule::Cookie), "{line}");
        }
    }
}

#[test]
fn cookie_names_without_values_and_other_names_are_left_alone() {
    for line in [
        "the _m_h5_tk cookie signs mtop requests",
        "let unb: string = x;",
        "if unb == other {}",
        "cookie2=值",
        "unb=<redacted>",
        "unb=***",
        "unb=${UNB}",
        "unb=",
        "munbx=1",
        "tounb=1",
        "\"unb\", \"munb\"",
    ] {
        assert!(!found(line).contains(&Rule::Cookie), "{line}");
    }
}

#[test]
fn cookie_and_authorization_headers_are_found() {
    for line in [
        "Cookie: a=b",
        "cookie : a=b",
        "Set-Cookie: sid=1; Path=/",
        "authorization: Bearer abc",
        "Authorization:Bearer abc",
        "  \"Cookie\": \"a=b\"",
        ".header(\"Authorization: x\")",
    ] {
        assert!(found(line).contains(&Rule::Header), "{line}");
    }
    for line in [
        "the Cookie header holds the login",
        "cookies: none",
        "authorized: yes",
        "Authorization failed",
        "set-cookie headers are dropped",
    ] {
        assert!(!found(line).contains(&Rule::Header), "{line}");
    }
}

#[test]
fn headers_and_cookies_passed_as_arguments_are_found() {
    // #21: a name and its literal value as adjacent arguments or as a pair,
    // with no `:` or `=` between them.
    for text in [
        ".header(\"Authorization\", \"Bearer abc.def\")",
        "headers.append('Cookie', 'a=b; c=d')",
        ".header(header::AUTHORIZATION, \"Basic dXNlcjpwYXNz\")",
        "new Headers([[\"set-cookie\", \"sid=1; Path=/\"]])",
        ".header(\n    \"Authorization\",\n    \"Bearer abc\",\n)",
    ] {
        assert!(found(text).contains(&Rule::Header), "{text}");
    }
    for name in COOKIE_NAMES {
        for text in [
            format!("Cookie::new(\"{name}\", \"abc123\")"),
            format!("[(\"{name}\", \"2200000000000\")]"),
            format!("cookies.set('{name}', 'abc_123')"),
        ] {
            assert!(found(&text).contains(&Rule::Cookie), "{text}");
        }
    }
}

#[test]
fn lists_of_names_and_values_that_are_not_literals_are_left_alone() {
    for text in [
        "[\"unb\", \"munb\"]",
        "const KEEP: [&str; 5] = [\"_m_h5_tk\", \"_m_h5_tk_enc\", \"cookie2\", \"unb\", \"t\"];",
        "strip: [\"cookie\", \"set-cookie\", \"authorization\", \"x-forwarded-for\"]",
        ".header(\"Authorization\", token)",
        ".header(\"Cookie\", \"\")",
        "cookies.set(\"unb\", \"REDACTED\")",
    ] {
        assert!(found(text).is_empty(), "{text}: {:?}", found(text));
    }
}

#[test]
fn cookies_saved_as_name_and_value_objects_are_found() {
    // How HAR files, Playwright's storageState, Puppeteer and browser
    // extensions save a cookie: the name and the value are two entries.
    for name in COOKIE_NAMES {
        for text in [
            format!("{{\"name\": \"{name}\", \"value\": \"abc123\", \"domain\": \".damai.cn\"}}"),
            format!(
                "{{\n  \"domain\": \".damai.cn\",\n  \"name\": \"{name}\",\n  \"path\": \"/\",\n  \"value\": \"abc123\"\n}}"
            ),
            format!("{{ name: '{name}', value: 'abc123' }}"),
        ] {
            assert!(found(&text).contains(&Rule::Cookie), "{text}");
        }
    }
    let har_header = "{\"name\": \"Authorization\", \"value\": \"Bearer abc\"}";
    assert!(found(har_header).contains(&Rule::Header), "{har_header}");
    for text in [
        "{\"name\": \"unb\", \"value\": \"\"}",
        "{\"name\": \"unb\", \"value\": \"<redacted>\"}",
        "{\"name\": \"unb\"}",
        "{\"name\": \"unb\"}, {\"value\": \"abc123\"}",
        "json!({\"name\": \"unb\", \"value\": token})",
        "{\"name\": \"cookie\", \"value\": \"\"}",
    ] {
        assert!(found(text).is_empty(), "{text}: {:?}", found(text));
    }
}

#[test]
fn a_cookies_txt_line_is_found() {
    // The Netscape format that `curl -c` and browser extensions write:
    // tab-separated, the name in the sixth field and the value in the last.
    for name in COOKIE_NAMES {
        for text in [
            format!(".damai.cn\tTRUE\t/\tFALSE\t1760000000\t{name}\tabc123"),
            format!("#HttpOnly_.damai.cn\tTRUE\t/\tTRUE\t0\t{name}\tabc123"),
        ] {
            assert!(found(&text).contains(&Rule::Cookie), "{text}");
        }
    }
    for text in [
        "# Netscape HTTP Cookie File",
        ".damai.cn\tTRUE\t/\tFALSE\t0\tunb\t",
        ".damai.cn\tTRUE\t/\tFALSE\t0\tcna\tabc123",
    ] {
        assert!(found(text).is_empty(), "{text}: {:?}", found(text));
    }
}

#[test]
fn a_credential_split_across_lines_fails_and_names_the_line_of_the_name() {
    // #21: the scan went line by line, so a value on the line after its
    // name was never seen.
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
        ]
    );
}

#[test]
fn id_card_numbers_are_found_only_when_well_formed() {
    let id = sample_id();
    assert!(found(&format!("id: {id}")).contains(&Rule::IdCard));
    assert!(found(&format!("\"idNo\":\"{}\"", id.to_lowercase())).contains(&Rule::IdCard));
    assert!(found(&format!("观演人身份证{id}。")).contains(&Rule::IdCard));

    let wrong_check = join(&["110105194912", "310021"]);
    let month_13 = join(&["110105194913", "310021"]);
    let feb_30 = join(&["110105194902", "300026"]);
    let no_such_region = join(&["990105194912", "310029"]);
    let eighteen_digits = join(&["123456789", "012345678"]);
    for not_id in [
        wrong_check,
        month_13,
        feb_30,
        no_such_region,
        eighteen_digits,
    ] {
        assert!(
            !found(&format!("id {not_id}")).contains(&Rule::IdCard),
            "{not_id}"
        );
    }
    // Longer or shorter runs are not an ID card, e.g. a 19-digit order number.
    assert!(!found(&format!("order {id}7")).contains(&Rule::IdCard));
    assert!(!found(&format!("order a{id}")).contains(&Rule::IdCard));
}

#[test]
fn mobile_numbers_are_found_only_as_a_whole_eleven_digit_number() {
    let phone = sample_mobile();
    assert!(found(&format!("tel {phone}")).contains(&Rule::Phone));
    assert!(found(&format!("\"mobile\":\"{phone}\"")).contains(&Rule::Phone));
    assert!(found(&format!("手机{phone}请")).contains(&Rule::Phone));
    for not_phone in [
        join(&["1380013", "800"]),
        join(&["138", "0013", "80001"]),
        join(&["2380013", "8000"]),
        join(&["1280013", "8000"]),
        join(&["x138", "0013", "8000"]),
        // Item ids and millisecond timestamps are 12 and 13 digits.
        "1086791513717".to_string(),
        "1760000000000".to_string(),
    ] {
        assert!(
            !found(&format!("n {not_phone}")).contains(&Rule::Phone),
            "{not_phone}"
        );
    }
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
