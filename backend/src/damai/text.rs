use serde_json::Value;

/// A string as a browser would show it: runs of whitespace collapsed to one
/// space and the ends trimmed. damai.cn's data has "2027-01-10  周日 15:00"
/// where its page shows one space.
pub(crate) fn displayed(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A non-empty string field, as displayed.
pub(crate) fn string(v: &Value) -> Option<String> {
    v.as_str().map(displayed).filter(|s| !s.is_empty())
}

/// An integer field. The detail page writes numbers, the tier endpoint
/// writes the same fields as strings ("performId": "286937790").
pub(crate) fn integer(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn displayed_collapses_whitespace_as_a_page_does() {
        assert_eq!(displayed("2027-01-10  周日 15:00"), "2027-01-10 周日 15:00");
        assert_eq!(displayed("  a\n\tb  "), "a b");
    }

    #[test]
    fn integer_reads_numbers_and_numeric_strings_only() {
        assert_eq!(integer(&json!(286937790)), Some(286937790));
        assert_eq!(integer(&json!("286937790")), Some(286937790));
        assert_eq!(integer(&json!("")), None);
        assert_eq!(integer(&json!("12a")), None);
        assert_eq!(integer(&json!(null)), None);
    }
}
