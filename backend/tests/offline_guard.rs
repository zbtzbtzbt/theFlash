//! A test that starts theflash without pointing it at a local server fails,
//! and says why, instead of sending a request to the real damai.cn. The
//! owner's rule: automated tests never reach the real damai.cn (issue #7).

mod support;

use std::process::Command;

const PROBE: &str = "forgot_the_mock_server_probe";

/// What a test that forgot THEFLASH_DAMAI_BASE_URL does: starts the binary
/// the way `support::theflash` does, without a base URL. Ignored so a plain
/// `cargo test` does not run it; the test below runs it in a process of its
/// own, because the tripwire ends the process it fires in.
#[test]
#[ignore = "run by a_test_that_forgot_the_mock_server_fails_and_says_so"]
fn forgot_the_mock_server_probe() {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_theflash"));
    support::offline(&mut cmd);
    let out = cmd.args(["event", "1086791513717"]).output().unwrap();
    // Not reached when the guard works. If it is, the request went
    // somewhere other than the tripwire.
    panic!(
        "the guard let the request through: stdout {:?}, stderr {:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_test_that_forgot_the_mock_server_fails_and_says_so() {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", PROBE, "--nocapture"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(101), "stderr: {stderr}");
    assert!(
        stderr.contains("测试试图访问真实大麦"),
        "the failure does not say what happened: {stderr}"
    );
    assert!(
        !stderr.contains("the guard let the request through"),
        "{stderr}"
    );
}

#[test]
fn a_test_that_points_theflash_at_a_local_server_is_not_caught() {
    let damai = support::MockDamai::start(&[
        ("/item.htm?id=1077137031347", "item_sold_out.html"),
        (
            "dataType=2&dataId=284490009",
            "sku_sold_out_284490009.jsonp",
        ),
    ]);
    let out = support::theflash(&damai.base_url())
        .args(["event", "1077137031347"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(damai.hits().len(), 2, "{:?}", damai.hits());
}
