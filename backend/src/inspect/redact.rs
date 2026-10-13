//! Taking credentials and personal data out of a real response before it is
//! saved as a fixture, and refusing to save one that still has any.
//!
//! The rules are the owner's (docs/code-standards.md): no login cookie with
//! a value, no Cookie/Authorization header, no mainland mobile number, no
//! valid 18-digit ID card number. `backend/tests/repo_guard.rs` enforces
//! the same rules over the whole repository; it is a test, so it cannot be
//! called from here. The two lists are checked against each other by
//! `backend/tests/inspect.rs`, and unifying them is noted in 0011.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Cookie,
    Header,
    IdCard,
    Phone,
}

impl Rule {
    pub fn name(self) -> &'static str {
        match self {
            Rule::Cookie => "登录 cookie 带值",
            Rule::Header => "Cookie 或 Authorization 请求头",
            Rule::IdCard => "18 位身份证号",
            Rule::Phone => "11 位手机号",
        }
    }
}

/// One line of a response that holds something that must not be committed.
/// The value is never carried along, only which rule and which line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub line: usize,
    pub rule: Rule,
}

/// Cookie names of Taobao and damai.cn logins. Longer names first, so
/// `_m_h5_tk_enc` is not read as `_m_h5_tk`.
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

/// What the fixture would carry into the repository, by line.
pub fn sensitive(text: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line_no = i + 1;
        if !cookie_values(line).is_empty() {
            hits.push(Hit {
                line: line_no,
                rule: Rule::Cookie,
            });
        }
        if has_header(line) {
            hits.push(Hit {
                line: line_no,
                rule: Rule::Header,
            });
        }
        if tokens(line).any(is_id_card) {
            hits.push(Hit {
                line: line_no,
                rule: Rule::IdCard,
            });
        }
        if tokens(line).any(is_mobile) {
            hits.push(Hit {
                line: line_no,
                rule: Rule::Phone,
            });
        }
    }
    hits
}

/// The response with the values this module can replace taken out: a login
/// cookie's value becomes `<redacted>`, and the digits of a mobile or ID
/// card number become zeros, which keeps the response the same shape and
/// the same length. Anything else is left for [`sensitive`] to refuse.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        out.push_str(&redact_line(line));
    }
    out
}

fn redact_line(line: &str) -> String {
    let mut spans: Vec<(usize, usize, &str)> = Vec::new();
    for (start, end) in cookie_values(line) {
        spans.push((start, end, "<redacted>"));
    }
    for (start, end) in digit_runs(line) {
        let run = &line[start..end];
        if is_mobile(run) || is_id_card(run) {
            spans.push((start, end, ""));
        }
    }
    spans.sort_by_key(|(start, _, _)| *start);
    let mut out = String::with_capacity(line.len());
    let mut at = 0;
    for (start, end, with) in spans {
        if start < at {
            continue;
        }
        out.push_str(&line[at..start]);
        if with.is_empty() {
            out.extend(std::iter::repeat_n('0', end - start));
        } else {
            out.push_str(with);
        }
        at = end;
    }
    out.push_str(&line[at..]);
    out
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// A cookie value as it is written in a header or a JSON dump. Excludes
/// placeholders such as `<redacted>`, `***` and `${TOKEN}`.
fn is_value_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'%' | b'-' | b'_' | b'.' | b'+' | b'/' | b'~')
}

/// Where the value of each login cookie in this line is: `name=value`, or
/// `"name":"value"` in JSON.
fn cookie_values(line: &str) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let skip = |mut i: usize, set: &[u8]| {
        while b.get(i).is_some_and(|c| set.contains(c)) {
            i += 1;
        }
        i
    };
    let mut found: Vec<(usize, usize)> = Vec::new();
    for name in COOKIE_NAMES {
        for (at, _) in line.match_indices(name) {
            if at > 0 && is_name_byte(b[at - 1]) {
                continue;
            }
            // Inside a name already matched by a longer entry.
            if found.iter().any(|(s, e)| at >= *s && at < *e) {
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
            let start = i;
            let mut end = i;
            while b.get(end).is_some_and(|c| is_value_byte(*c)) {
                end += 1;
            }
            if end > start {
                found.push((start, end));
            }
        }
    }
    found
}

/// The Cookie, Set-Cookie and Authorization headers, each followed by its
/// colon, also as JSON keys. Their presence is refused rather than
/// redacted: a response that carries a login header is not a fixture to
/// repair, it is one to drop. (The names are spelled out in
/// `HEADER_NAMES`; writing them with their colons here would make this
/// file trip `repo_guard.rs` itself.)
fn has_header(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let b = lower.as_bytes();
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
                return true;
            }
        }
    }
    false
}

fn tokens(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
}

/// Runs of digits, which is where a mobile or ID card number would be. An
/// ID card number can end in X, so a run followed by one is taken whole.
fn digit_runs(line: &str) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let mut end = i;
        if matches!(b.get(end), Some(b'X' | b'x'))
            && !b.get(end + 1).is_some_and(|c| is_name_byte(*c))
        {
            end += 1;
        }
        if start > 0 && is_name_byte(b[start - 1]) {
            continue;
        }
        out.push((start, end));
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Built from pieces so this file does not contain what it looks for.
    fn sample_id() -> String {
        ["110105194912", "31002X"].concat()
    }

    fn sample_mobile() -> String {
        ["138", "0013", "8000"].concat()
    }

    fn rules(text: &str) -> Vec<Rule> {
        sensitive(text).into_iter().map(|h| h.rule).collect()
    }

    #[test]
    fn a_login_cookie_keeps_its_name_and_loses_its_value() {
        for name in COOKIE_NAMES {
            let line = format!("{{\"{name}\":\"abc123\"}}");
            let out = redact(&line);
            assert!(out.contains(name), "{out}");
            assert!(!out.contains("abc123"), "{out}");
            assert!(rules(&out).is_empty(), "{out}");
        }
        let line = format!("x=1; {}=abc%20123; y=2", COOKIE_NAMES[1]);
        assert_eq!(
            redact(&line),
            format!("x=1; {}=<redacted>; y=2", COOKIE_NAMES[1]),
            "the value ends where a cookie value cannot go on"
        );
    }

    #[test]
    fn a_mobile_or_id_number_becomes_zeros_of_the_same_length() {
        let phone = sample_mobile();
        let id = sample_id();
        let line =
            format!("{{\"mobile\":\"{phone}\",\"idNo\":\"{id}\",\"itemId\":\"1086791513717\"}}");
        let out = redact(&line);
        assert_eq!(
            out,
            "{\"mobile\":\"00000000000\",\"idNo\":\"000000000000000000\",\"itemId\":\"1086791513717\"}",
            "item ids and other long numbers are left alone"
        );
        assert!(rules(&out).is_empty(), "{out}");
    }

    #[test]
    fn what_cannot_be_redacted_is_still_reported_with_its_line() {
        let text = format!("第一行\nSet-Cookie: sid=1\n第三行 {}\n", sample_mobile());
        assert_eq!(
            sensitive(&text),
            [
                Hit {
                    line: 2,
                    rule: Rule::Header
                },
                Hit {
                    line: 3,
                    rule: Rule::Phone
                },
            ]
        );
        // Redaction clears the one it can, and the header stays to refuse.
        assert_eq!(
            sensitive(&redact(&text)),
            [Hit {
                line: 2,
                rule: Rule::Header
            }]
        );
    }

    #[test]
    fn a_clean_response_is_returned_unchanged() {
        let body = "__jp0({\"perform\":{\"performId\":\"286937790\",\"skuList\":[]}})\n";
        assert_eq!(redact(body), body);
        assert!(sensitive(body).is_empty());
    }

    #[test]
    fn the_rules_find_what_the_repository_guard_finds() {
        // Built from pieces, so this file does not carry what it looks
        // for and trip `repo_guard.rs`.
        let header = |name: &str, value: &str| format!("{name}{}{value}", ":");
        for (line, rule) in [
            (header("Cookie", " a=b"), Rule::Header),
            (header("  \"Cookie\"", " \"a=b\""), Rule::Header),
            (header("authorization", " Bearer abc"), Rule::Header),
            (format!("{}=abc123", COOKIE_NAMES[4]), Rule::Cookie),
            (
                format!("  \"{}\": \"abc123\",", COOKIE_NAMES[2]),
                Rule::Cookie,
            ),
        ] {
            assert!(rules(&line).contains(&rule), "{line}");
        }
        for line in [
            "the _m_h5_tk cookie signs mtop requests".to_string(),
            format!("{}=<redacted>", COOKIE_NAMES[4]),
            format!("{}=", COOKIE_NAMES[4]),
            format!("m{}x=1", COOKIE_NAMES[4]),
            "the Cookie header holds the login".to_string(),
            "set-cookie headers are dropped".to_string(),
            "order 1086791513717".to_string(),
            "ms 1760000000000".to_string(),
        ] {
            assert!(rules(&line).is_empty(), "{line}: {:?}", rules(&line));
        }
    }

    #[test]
    fn multiple_lines_keep_their_line_endings() {
        assert_eq!(redact("a\nb\n"), "a\nb\n");
        assert_eq!(redact("a\r\nb"), "a\r\nb");
    }
}
