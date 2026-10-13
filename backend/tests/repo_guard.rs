//! Guards for two of the owner's rules (issue #7), so breaking one fails the
//! build instead of relying on an agent remembering it:
//!
//! - credentials and personal data stay out of the repository: no login
//!   cookie with a value, no Cookie/Authorization header, no valid 18-digit
//!   ID card number, no mainland mobile number in any tracked text file;
//! - nothing that tests run can reach the real damai.cn: every damai host in
//!   source code is on the blocklist CI resolves to nowhere, and tests start
//!   the binary only through `support::offline`.
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

/// `name=value`, or `"name":"value"` in JSON.
fn cookie_names_with_value(line: &str) -> BTreeSet<&'static str> {
    let b = line.as_bytes();
    let skip = |mut i: usize, set: &[u8]| {
        while b.get(i).is_some_and(|c| set.contains(c)) {
            i += 1;
        }
        i
    };
    let mut found = BTreeSet::new();
    for name in COOKIE_NAMES {
        for (at, _) in line.match_indices(name) {
            if at > 0 && is_name_byte(b[at - 1]) {
                continue;
            }
            let mut i = at + name.len();
            let quoted = matches!(b.get(i), Some(b'"' | b'\''));
            if quoted {
                i += 1;
            }
            i = skip(i, b" \t");
            match b.get(i) {
                Some(b'=') => {}
                Some(b':') if quoted => {}
                _ => continue,
            }
            i = skip(i + 1, b" \t");
            if matches!(b.get(i), Some(b'"' | b'\'')) {
                i += 1;
            }
            if b.get(i).is_some_and(|c| is_value_byte(*c)) {
                found.insert(name);
            }
        }
    }
    found
}

/// `Cookie:`, `Set-Cookie:` and `Authorization:`, also as JSON keys.
fn header_names(line: &str) -> BTreeSet<&'static str> {
    let lower = line.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut found = BTreeSet::new();
    for name in HEADER_NAMES {
        for (at, _) in lower.match_indices(name) {
            if at > 0 && (b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'-') {
                continue;
            }
            let mut i = at + name.len();
            if matches!(b.get(i), Some(b'"' | b'\'')) {
                i += 1;
            }
            while matches!(b.get(i), Some(b' ' | b'\t')) {
                i += 1;
            }
            if b.get(i) == Some(&b':') {
                found.insert(name);
            }
        }
    }
    found
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

/// What in one line of a text file is a credential or personal data.
fn sensitive_in_line(line: &str) -> Vec<(Rule, String)> {
    let mut out = Vec::new();
    for name in cookie_names_with_value(line) {
        out.push((Rule::Cookie, format!("cookie {name} with a value")));
    }
    for name in header_names(line) {
        out.push((Rule::Header, format!("{name} header")));
    }
    let tokens = || {
        line.split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| !t.is_empty())
    };
    if tokens().any(is_id_card) {
        out.push((Rule::IdCard, "an 18-digit ID card number".into()));
    }
    if tokens().any(is_mobile) {
        out.push((Rule::Phone, "an 11-digit mobile number".into()));
    }
    out
}

// ---- damai hosts in source code ----

const SOURCE_EXTENSIONS: [&str; 10] = [
    "rs", "ts", "tsx", "js", "mjs", "cjs", "sh", "yml", "yaml", "toml",
];

/// Every `*.damai.cn`-style host named in a line, as written (lowercased).
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
            let host = lower[start..end].trim_start_matches(['.', '-']);
            found.insert(host.to_string());
        }
    }
    found
}

fn is_source_file(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SOURCE_EXTENSIONS.contains(&e))
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

/// What is wrong with `files` (paths relative to `root`): findings not on
/// the allowlist, and allowlist entries that no longer match anything.
fn check_files(root: &Path, files: &[String], allow: &[Allowed]) -> Vec<String> {
    let mut findings = Vec::new();
    let mut used = vec![false; allow.len()];
    for file in files {
        let Ok(bytes) = std::fs::read(root.join(file)) else {
            continue; // tracked, but deleted in this checkout
        };
        if !is_text(&bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            for (rule, what) in sensitive_in_line(line) {
                if let Some(k) = allow.iter().position(|a| a.path == *file && a.rule == rule) {
                    used[k] = true;
                    continue;
                }
                findings.push(Finding {
                    path: file.clone(),
                    line: i + 1,
                    rule,
                    what,
                });
            }
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

/// Damai hosts named in the source files of `files` that are not in `blocked`.
fn unblocked_hosts(root: &Path, files: &[String], blocked: &BTreeSet<String>) -> Vec<Finding> {
    let mut out = Vec::new();
    for file in files.iter().filter(|f| is_source_file(f)) {
        let Ok(text) = std::fs::read_to_string(root.join(file)) else {
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

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn tracked_files(root: &Path) -> Vec<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
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
    let problems = check_files(&root, &tracked_files(&root), &load_allowlist(&root));
    assert!(
        problems.is_empty(),
        "credentials or personal data in the repository. Remove them, or, for a \
         confirmed false positive, add `path | rule | reason` to \
         backend/tests/repo_guard_allowlist.txt. The values are not printed.\n{}",
        problems.join("\n")
    );
}

#[test]
fn every_damai_host_in_source_code_is_blocked_in_ci() {
    let root = repo_root();
    let blocked = blocked_hosts(&root);
    let found = unblocked_hosts(&root, &tracked_files(&root), &blocked);
    assert!(
        found.is_empty(),
        "source code names damai hosts that CI does not block. Add each to \
         scripts/damai-hosts.txt, so a test that reaches for it fails:\n{}",
        found
            .iter()
            .map(Finding::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn tests_start_theflash_only_through_the_offline_helper() {
    let root = repo_root();
    let needle = concat!("CARGO_BIN_EXE", "_theflash");
    let helper = concat!("support::", "offline(");
    let bypassing: Vec<String> = tracked_files(&root)
        .into_iter()
        .filter(|f| f.starts_with("backend/tests/") && f.ends_with(".rs"))
        .filter(|f| f != "backend/tests/support/mod.rs")
        .filter(|f| {
            let text = std::fs::read_to_string(root.join(f)).unwrap_or_default();
            text.contains(needle) && !text.contains(helper)
        })
        .collect();
    assert!(
        bypassing.is_empty(),
        "these tests start the binary without `support::offline`, so a missing \
         THEFLASH_DAMAI_BASE_URL would reach the real damai.cn: {bypassing:?}"
    );
}

// ---- the scanner itself ----

fn found(line: &str) -> Vec<Rule> {
    sensitive_in_line(line)
        .into_iter()
        .map(|(r, _)| r)
        .collect()
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
