//! `theflash --version` says the version in backend/Cargo.toml. A release is
//! refused unless its tag is that version (scripts/release.sh gate), and the
//! release workflow compares each built binary's `--version` with the tag, so
//! together they make a downloaded binary say the version it was released as
//! (issue #5).

mod support;

use support::MockDamai;

/// `version` under `[package]`, read from the file rather than taken from
/// what cargo passes to the build, so that a version written anywhere else
/// in the code is caught.
fn cargo_toml_version() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let text = std::fs::read_to_string(path).unwrap();
    let mut in_package = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if let Some(value) = line.strip_prefix("version").filter(|_| in_package) {
            let value = value.trim_start().strip_prefix('=').unwrap().trim();
            return value.trim_matches('"').to_string();
        }
    }
    panic!("no version under [package] in {path}");
}

#[test]
fn version_is_the_one_in_cargo_toml() {
    let want = format!("theflash {}\n", cargo_toml_version());
    let damai = MockDamai::start(&[]);
    for arg in ["--version", "version"] {
        let out = support::theflash(&damai.base_url())
            .arg(arg)
            .output()
            .unwrap();
        assert!(out.status.success(), "{arg}: {out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "{arg}");
    }
    assert!(damai.hits().is_empty(), "{:?}", damai.hits());
}
