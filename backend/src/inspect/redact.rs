//! Taking credentials and personal data out of a real response before it is
//! saved as a fixture, and refusing to save one that still has any.
//!
//! The rules are the owner's (docs/code-standards.md): no login cookie with
//! a value, no Cookie/Authorization header, no mainland mobile number, no
//! valid 18-digit ID card number. `backend/tests/repo_guard.rs` enforces
//! the same rules over the whole repository, and calls [`sensitive`] to do
//! it: what `theflash inspect record` writes and what may be committed are
//! decided by the one reading of the rules, so neither can drift away from
//! the other (#71; what the two copies cost is in 0011's "后果").
//!
//! A credential is not always written as `name=value` on one line. The name,
//! its separator and its value may sit on different lines, and a browser
//! exports a cookie as a `{"name": …, "value": …}` object, as a cookies.txt
//! line or as two adjacent arguments. So the scan reads the whole text and
//! reports the line the name is on. What [`redact`] can replace it replaces;
//! a login header is refused whole instead, because a response carrying one
//! is not a fixture to repair.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    Cookie,
    Header,
    IdCard,
    Phone,
}

impl Rule {
    /// What an inspection report calls it.
    pub fn name(self) -> &'static str {
        match self {
            Rule::Cookie => "登录 cookie 带值",
            Rule::Header => "Cookie 或 Authorization 请求头",
            Rule::IdCard => "18 位身份证号",
            Rule::Phone => "11 位手机号",
        }
    }

    /// The short name `repo_guard.rs` prints and its allowlist is written
    /// in.
    pub fn id(self) -> &'static str {
        match self {
            Rule::Cookie => "cookie",
            Rule::Header => "header",
            Rule::IdCard => "id-card",
            Rule::Phone => "phone",
        }
    }

    pub fn from_id(name: &str) -> Option<Rule> {
        [Rule::Cookie, Rule::Header, Rule::IdCard, Rule::Phone]
            .into_iter()
            .find(|r| r.id() == name)
    }
}

/// Something in a response that must not be committed. The value is never
/// carried along, only which rule, which line and what was found.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hit {
    pub line: usize,
    pub rule: Rule,
    /// What was found, without the sensitive value: `cookie unb with a
    /// value`.
    pub what: String,
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

/// What the text would carry into the repository, by line. A cookie or a
/// header split across lines is reported on the line of its name.
pub fn sensitive(text: &str) -> Vec<Hit> {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let mut hits: Vec<Hit> = scan(text)
        .into_iter()
        .map(|found| Hit {
            line: starts.partition_point(|s| *s <= found.at),
            rule: found.rule,
            what: found.what,
        })
        .collect();
    hits.sort();
    hits.dedup();
    hits
}

/// The text with the values this module can replace taken out: a login
/// cookie's value becomes `<redacted>`, and the digits of a mobile or ID
/// card number become zeros, which keeps the response the same shape and
/// the same length. A login header has no value to replace — the whole
/// response goes — so it is left for [`sensitive`] to refuse.
pub fn redact(text: &str) -> String {
    let mut spans: Vec<(usize, usize, Replacement)> = scan(text)
        .into_iter()
        .filter_map(|found| {
            let (start, end) = found.value?;
            let with = match found.rule {
                Rule::Cookie => Replacement::Redacted,
                _ => Replacement::Zeros,
            };
            Some((start, end, with))
        })
        .collect();
    // Stable, so a cookie value keeps the span a number inside it would
    // otherwise take: `scan` puts the cookies first.
    spans.sort_by_key(|(start, _, _)| *start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, with) in spans {
        if start < at {
            continue;
        }
        out.push_str(&text[at..start]);
        match with {
            Replacement::Redacted => out.push_str("<redacted>"),
            Replacement::Zeros => out.extend(std::iter::repeat_n('0', end - start)),
        }
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

#[derive(Debug, Clone, Copy)]
enum Replacement {
    /// A cookie value, which has no length worth keeping.
    Redacted,
    /// A number the response's shape depends on, so it keeps its length.
    Zeros,
}

/// One thing the rules found: where its name is, which rule it breaks,
/// what to say about it, and the span of the value when there is one to
/// replace. A header has no span: the response goes whole.
struct Found {
    at: usize,
    rule: Rule,
    what: String,
    value: Option<(usize, usize)>,
}

fn scan(text: &str) -> Vec<Found> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let cookie = |at, name, value| Found {
        at,
        rule: Rule::Cookie,
        what: format!("cookie {name} with a value"),
        value: Some(value),
    };
    for (at, name, value) in cookies_with_values(text, &lower) {
        out.push(cookie(at, name, value));
    }
    for (at, name, value) in cookies_txt(text) {
        out.push(cookie(at, name, value));
    }
    for (at, name) in headers(&lower) {
        out.push(Found {
            at,
            rule: Rule::Header,
            what: format!("{name} header"),
            value: None,
        });
    }
    for (start, end) in words(text) {
        let word = &text[start..end];
        let rule = if is_id_card(word) {
            Rule::IdCard
        } else if is_mobile(word) {
            Rule::Phone
        } else {
            continue;
        };
        let what = match rule {
            Rule::IdCard => "an 18-digit ID card number",
            _ => "an 11-digit mobile number",
        };
        out.push(Found {
            at: start,
            rule,
            what: what.to_string(),
            value: Some((start, end)),
        });
    }
    out
}

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

/// The value, quoted or not, that starts at `i`.
fn value_span(b: &[u8], mut i: usize) -> Option<(usize, usize)> {
    if b.get(i).copied().is_some_and(is_quote) {
        i += 1;
    }
    let mut end = i;
    while b.get(end).copied().is_some_and(is_value_byte) {
        end += 1;
    }
    (end > i).then_some((i, end))
}

/// The string literal starting at `i` when it holds a value rather than
/// another name. After a comma a name is the next item of a list, as in
/// `["unb", "munb"]` or `["cookie", "set-cookie"]`; a value has a digit or
/// a sign in it, as in `"Bearer abc"`, `"a=b"` or `"2200000000000"`.
fn literal_span(b: &[u8], i: usize) -> Option<(usize, usize)> {
    let quote = *b.get(i).filter(|q| is_quote(**q))?;
    let rest = &b[i + 1..];
    let len = rest
        .iter()
        .position(|c| *c == quote || *c == b'\n')
        .unwrap_or(rest.len());
    let literal = &rest[..len];
    let is_name = |n: &&str| literal.eq_ignore_ascii_case(n.as_bytes());
    let holds_value = literal.first().copied().is_some_and(is_value_byte)
        && literal
            .iter()
            .any(|c| !(c.is_ascii_alphabetic() || matches!(c, b'_' | b'-')))
        && !COOKIE_NAMES.iter().chain(&HEADER_NAMES).any(is_name);
    holds_value.then(|| (i + 1, i + 1 + len))
}

/// The value of the `value` key, when the quoted name at `at..at + len` is
/// the value of a `name` key in the same object: `{"name": "unb", "value":
/// "…"}`, the way HAR files, Playwright's storageState, Puppeteer and
/// browser extensions save cookies and headers. `b` is the text in
/// lowercase, which has the same byte offsets as the text itself.
fn object_value_span(b: &[u8], at: usize, len: usize) -> Option<(usize, usize)> {
    let end = at + len;
    if at == 0 || !is_quote(b[at - 1]) || b.get(end) != Some(&b[at - 1]) {
        return None;
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
        return None;
    }
    k = back(k - 1);
    if k > 0 && is_quote(b[k - 1]) {
        k -= 1;
    }
    if k < 4 || &b[k - 4..k] != b"name" || (k > 4 && is_name_byte(b[k - 5])) {
        return None;
    }
    // The object runs from the `{` before the name to the `}` after it.
    let (Some(lo), Some(hi)) = (
        b[..at].iter().rposition(|c| *c == b'{'),
        b[end..].iter().position(|c| *c == b'}').map(|p| end + p),
    ) else {
        return None;
    };
    let object = &b[lo..hi];
    (1..object.len()).find_map(|i| {
        if !object[i..].starts_with(b"value") || is_name_byte(object[i - 1]) {
            return None;
        }
        let mut j = i + "value".len();
        if object.get(j).copied().is_some_and(is_quote) {
            j += 1;
        }
        j = skip_space(object, j);
        if object.get(j) != Some(&b':') {
            return None;
        }
        j = skip_space(object, j + 1);
        if !object.get(j).copied().is_some_and(is_quote) {
            return None;
        }
        let (start, end) = value_span(object, j)?;
        Some((lo + start, lo + end))
    })
}

/// Where login cookies are written with a value: their name, and the span
/// of the value. Spaces and line breaks may sit between the parts:
/// - `name=value`, as in a Cookie header or `document.cookie`;
/// - `"name": "value"`, a JSON key or a JS or Python dict entry;
/// - `"name", "value"`, adjacent arguments or a pair, as in a cookie jar's
///   `new("unb", "…")`, when the second is not just another name;
/// - `{"name": "unb", …, "value": "…"}` (see [`object_value_span`]).
fn cookies_with_values<'a>(
    text: &'a str,
    lower: &'a str,
) -> Vec<(usize, &'static str, (usize, usize))> {
    let b = text.as_bytes();
    let mut found: Vec<(usize, &'static str, (usize, usize))> = Vec::new();
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
            let span = match b.get(i) {
                Some(b'=') => value_span(b, skip_space(b, i + 1)),
                Some(b':') if quoted => value_span(b, skip_space(b, i + 1)),
                Some(b',') if quoted => literal_span(b, skip_space(b, i + 1)),
                _ => None,
            }
            .or_else(|| object_value_span(lower.as_bytes(), at, name.len()));
            if let Some(span) = span {
                found.push((at, name, span));
            }
        }
    }
    found
}

/// Where `Cookie`, `Set-Cookie` and `Authorization` are written as a
/// header, and their names. Their presence is refused rather than
/// redacted, so there is no value span to take:
/// - the name and a colon, also as a JSON key;
/// - the name and a literal value as adjacent arguments, the way a
///   request builder's `.header(…, "Bearer …")` takes them;
/// - `{"name": "cookie", "value": "…"}` in a HAR file.
///
/// (The names are spelled out in `HEADER_NAMES` only; writing one here
/// next to a colon or a value would make this file trip the rule itself.)
fn headers(lower: &str) -> Vec<(usize, &'static str)> {
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
                Some(b',') => literal_span(b, skip_space(b, i + 1)).is_some(),
                _ => false,
            };
            if as_header || object_value_span(b, at, name.len()).is_some() {
                found.push((at, name));
            }
        }
    }
    found
}

/// The login cookies on the cookies.txt lines of `text`, the Netscape
/// format `curl -c` and browser extensions write: seven tab-separated
/// fields, the name in the sixth and the value in the seventh.
fn cookies_txt(text: &str) -> Vec<(usize, &'static str, (usize, usize))> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let tabs: Vec<usize> = body.match_indices('\t').map(|(i, _)| i).collect();
        if let [.., fifth, sixth] = tabs[..]
            && tabs.len() == 6
        {
            let name = &body[fifth + 1..sixth];
            let value = &body[sixth + 1..];
            if value.bytes().next().is_some_and(is_value_byte)
                && let Some(name) = COOKIE_NAMES.into_iter().find(|n| *n == name)
            {
                out.push((
                    at + fifth + 1,
                    name,
                    (at + sixth + 1, at + sixth + 1 + value.len()),
                ));
            }
        }
        at += line.len();
    }
    out
}

/// Runs of letters and digits, which is what a mobile or ID card number is
/// written as. A number that runs into a word is part of that word, not a
/// number of its own, so the whole run is weighed at once.
fn words(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_alphanumeric() {
            i += 1;
        }
        out.push((start, i));
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

    /// A header line, with the colon joined on for the same reason.
    fn header(name: &str, value: &str) -> String {
        format!("{name}{}{value}", ":")
    }

    /// The header names, also built from pieces: written out next to a
    /// colon or a value, they would be findings of this file's own.
    fn cookie_header() -> String {
        ["Coo", "kie"].concat()
    }

    fn auth_header() -> String {
        ["Author", "ization"].concat()
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
        let text = format!(
            "第一行\n{}\n第三行 {}\n",
            header("Set-Cookie", " sid=1"),
            sample_mobile()
        );
        assert_eq!(
            sensitive(&text)
                .into_iter()
                .map(|h| (h.line, h.rule))
                .collect::<Vec<_>>(),
            [(2, Rule::Header), (3, Rule::Phone)]
        );
        // Redaction clears the one it can, and the header stays to refuse.
        assert_eq!(
            sensitive(&redact(&text))
                .into_iter()
                .map(|h| (h.line, h.rule))
                .collect::<Vec<_>>(),
            [(2, Rule::Header)]
        );
    }

    #[test]
    fn a_clean_response_is_returned_unchanged() {
        let body = "__jp0({\"perform\":{\"performId\":\"286937790\",\"skuList\":[]}})\n";
        assert_eq!(redact(body), body);
        assert!(sensitive(body).is_empty());
    }

    #[test]
    fn multiple_lines_keep_their_line_endings() {
        assert_eq!(redact("a\nb\n"), "a\nb\n");
        assert_eq!(redact("a\r\nb"), "a\r\nb");
    }

    /// Every form the rules read, in one list, so a form added to the scan
    /// without a way to redact it is caught here rather than when a
    /// recorded fixture is committed (#71).
    fn every_form() -> Vec<(String, Rule)> {
        let mut forms = vec![
            (header(&cookie_header(), " a=b"), Rule::Header),
            (
                header(&format!("  \"{}\"", cookie_header()), " \"a=b\""),
                Rule::Header,
            ),
            (
                header(&auth_header().to_lowercase(), " Bearer abc"),
                Rule::Header,
            ),
            (header(&auth_header(), "Bearer abc"), Rule::Header),
            (
                header(&format!("Set-{}", cookie_header()), " sid=1"),
                Rule::Header,
            ),
            // The name and its value as adjacent arguments, with no `:`
            // or `=` between them.
            (
                format!(".header(\"{}\", \"Bearer abc.def\")", auth_header()),
                Rule::Header,
            ),
            (
                format!("headers.append('{}', 'a=b; c=d')", cookie_header()),
                Rule::Header,
            ),
            (
                format!(
                    ".header(\n    \"{}\",\n    \"Bearer abc\",\n)",
                    auth_header()
                ),
                Rule::Header,
            ),
            // How a HAR file saves one header.
            (
                format!(
                    "{{\"name\": \"{}\", \"value\": \"Bearer abc\"}}",
                    cookie_header()
                ),
                Rule::Header,
            ),
            (format!("id: {}", sample_id()), Rule::IdCard),
            (format!("\"idNo\":\"{}\"", sample_id()), Rule::IdCard),
            (format!("观演人身份证{}。", sample_id()), Rule::IdCard),
            (format!("tel {}", sample_mobile()), Rule::Phone),
            (format!("\"mobile\":\"{}\"", sample_mobile()), Rule::Phone),
            (format!("手机{}请", sample_mobile()), Rule::Phone),
        ];
        for name in COOKIE_NAMES {
            for text in [
                format!("{name}=abc123"),
                format!("a=b; {name}=abc123; c=d"),
                format!("  \"{name}\": \"abc123\","),
                format!("{name} = 'abc123'"),
                format!("x_m_h5_tk=1; {name}=abc%20123"),
                // The name, the separator and the value on three lines.
                format!("{{\n  \"{name}\":\n    \"7a1f3c9d2b4e_176\"\n}}"),
                format!("{name}\n  = abc123"),
                // Adjacent arguments.
                format!("{}::new(\"{name}\", \"abc123\")", "Cookie"),
                format!("[(\"{name}\", \"2200000000000\")]"),
                format!("cookies.set('{name}', 'abc_123')"),
                // How a browser exports one cookie.
                format!(
                    "{{\"name\": \"{name}\", \"value\": \"abc123\", \"domain\": \".damai.cn\"}}"
                ),
                format!(
                    "{{\n  \"domain\": \".damai.cn\",\n  \"name\": \"{name}\",\n  \"path\": \"/\",\n  \"value\": \"abc123\"\n}}"
                ),
                format!("{{ name: '{name}', value: 'abc123' }}"),
                // The Netscape cookies.txt line `curl -c` writes.
                format!(".damai.cn\tTRUE\t/\tFALSE\t1760000000\t{name}\tabc123"),
                format!("#HttpOnly_.damai.cn\tTRUE\t/\tTRUE\t0\t{name}\tabc123"),
            ] {
                forms.push((text, Rule::Cookie));
            }
        }
        forms
    }

    #[test]
    fn every_form_a_credential_is_written_in_is_found() {
        for (text, rule) in every_form() {
            assert!(rules(&text).contains(&rule), "{text}: {:?}", rules(&text));
        }
    }

    /// What `theflash inspect record` may write: everything the rules find
    /// is either taken out of the response or keeps the response from
    /// being written at all. A login header is the second kind, so it is
    /// the only thing a redacted response may still hold. Before #71 the
    /// scan and the redaction read different forms, and a recorded fixture
    /// went to disk with the value still in it.
    #[test]
    fn redacting_leaves_nothing_but_the_headers_that_refuse_a_response() {
        for (text, rule) in every_form() {
            let out = redact(&text);
            let left = rules(&out);
            assert!(
                left.iter().all(|r| *r == Rule::Header),
                "{text} 脱敏之后还命中 {left:?}：{out}"
            );
            if rule != Rule::Header {
                assert!(
                    !left.contains(&rule),
                    "{text} 脱敏之后还命中 {rule:?}：{out}"
                );
                assert!(
                    !out.contains("abc123")
                        && !out.contains(&sample_mobile())
                        && !out.contains(&sample_id()),
                    "{text} 的值还在：{out}"
                );
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
            "[\"unb\", \"munb\"]",
            "const KEEP: [&str; 3] = [\"_m_h5_tk\", \"cookie2\", \"unb\"];",
            ".damai.cn\tTRUE\t/\tFALSE\t0\tunb\t",
            ".damai.cn\tTRUE\t/\tFALSE\t0\tcna\tabc123",
            "{\"name\": \"unb\", \"value\": \"\"}",
            "{\"name\": \"unb\", \"value\": \"<redacted>\"}",
            "{\"name\": \"unb\"}",
            "{\"name\": \"unb\"}, {\"value\": \"abc123\"}",
            "json!({\"name\": \"unb\", \"value\": token})",
            "cookies.set(\"unb\", \"REDACTED\")",
        ] {
            assert!(!rules(line).contains(&Rule::Cookie), "{line}");
        }
    }

    #[test]
    fn header_names_that_are_not_headers_are_left_alone() {
        for line in [
            "the Cookie header holds the login".to_string(),
            "set-cookie headers are dropped".to_string(),
            "cookies: none".to_string(),
            "authorized: yes".to_string(),
            "Authorization failed".to_string(),
            format!("strip: [\"{}\", \"x-forwarded-for\"]", "authorization"),
            format!(".header(\"{}\", token)", "Authorization"),
            format!(".header(\"{}\", \"\")", "Cookie"),
            format!("{{\"name\": \"{}\", \"value\": \"\"}}", "cookie"),
        ] {
            assert!(!rules(&line).contains(&Rule::Header), "{line}");
        }
    }

    #[test]
    fn id_card_numbers_are_found_only_when_well_formed() {
        let join = |parts: [&str; 2]| parts.concat();
        for not_id in [
            join(["110105194912", "310021"]), // wrong check character
            join(["110105194913", "310021"]), // month 13
            join(["110105194902", "300026"]), // 30 February
            join(["990105194912", "310029"]), // no such region
            join(["123456789", "012345678"]),
        ] {
            assert!(
                !rules(&format!("id {not_id}")).contains(&Rule::IdCard),
                "{not_id}"
            );
        }
        // Longer or shorter runs are not an ID card, e.g. a 19-digit order
        // number.
        let id = sample_id();
        assert!(!rules(&format!("order {id}7")).contains(&Rule::IdCard));
        assert!(!rules(&format!("order a{id}")).contains(&Rule::IdCard));
    }

    #[test]
    fn mobile_numbers_are_found_only_as_a_whole_eleven_digit_number() {
        let join = |parts: &[&str]| parts.concat();
        for not_phone in [
            join(&["1380013", "800"]),
            join(&["138", "0013", "80001"]),
            join(&["2380013", "8000"]),
            join(&["1280013", "8000"]),
            join(&["x138", "0013", "8000"]),
            // Item ids and millisecond timestamps are 12 and 13 digits.
            "1086791513717".to_string(),
            "1760000000000".to_string(),
            "ms 1760000000000".to_string(),
            "order 1086791513717".to_string(),
        ] {
            assert!(
                !rules(&format!("n {not_phone}")).contains(&Rule::Phone),
                "{not_phone}"
            );
        }
    }

    /// A credential split across lines is reported on the line its name is
    /// on, so whoever reads the report looks where the name is.
    #[test]
    fn a_credential_split_across_lines_is_reported_on_the_line_of_its_name() {
        let text = format!("x\n{}\n  = abc123\n", COOKIE_NAMES[4]);
        assert_eq!(
            sensitive(&text),
            [Hit {
                line: 2,
                rule: Rule::Cookie,
                what: format!("cookie {} with a value", COOKIE_NAMES[4]),
            }]
        );
    }

    #[test]
    fn a_finding_never_carries_the_value() {
        let text = format!(
            "{}={}\n{}\n{}\n",
            COOKIE_NAMES[4],
            "abc123",
            sample_mobile(),
            sample_id()
        );
        for hit in sensitive(&text) {
            for secret in ["abc123", &sample_mobile(), &sample_id()] {
                assert!(!hit.what.contains(secret), "{hit:?}");
            }
        }
    }
}
