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

/// Opens `url` in the user's default browser (`open` on macOS, `xdg-open`
/// on Linux), passing `url` as a direct argument without a shell (#54).
///
/// `THEFLASH_BROWSER` overrides the launcher for offline tests: an empty
/// value disables launching (set by `support::offline` so automated tests
/// never pop open a real browser), and a path runs that stub with `url`.
pub(crate) fn open_browser(url: &str) -> Result<(), String> {
    let mut cmd = match std::env::var_os("THEFLASH_BROWSER") {
        Some(prog) if prog.is_empty() => return Ok(()),
        Some(prog) => Command::new(prog),
        None if cfg!(target_os = "macos") => Command::new("open"),
        None => Command::new("xdg-open"),
    };
    cmd.arg(url);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("启动器退出码 {}", status.code().unwrap_or(-1)))
    }
}
