use std::process::{Command, Stdio};

/// A desktop notification: macOS's Notification Centre through osascript,
/// Linux's through notify-send. Title and body go in as arguments, never
/// into the script's text.
pub(crate) fn system_notification(title: &str, body: &str) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("osascript");
        c.args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv) sound name \"Glass\"",
            "-e",
            "end run",
            title,
            body,
        ]);
        c
    } else {
        let mut c = Command::new("notify-send");
        c.args(["--app-name=theFlash", title, body]);
        c
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
