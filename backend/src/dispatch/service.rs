//! 一条命令装上、卸掉、看状态：macOS 用 launchd，Linux 用 systemd。
//!
//! The dispatcher has to survive a closed terminal and a reboot, which is
//! the whole point of #14, so it runs as a user service. The text of the
//! service file is a pure function of three paths, so a test reads what
//! would be installed without touching this machine's services.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::config;

/// The service name. `cn.theflash.validation` (#17) registers long
/// validation runs; this one is the dispatcher, and the two never share a
/// name or a log directory.
pub const LABEL: &str = "cn.theflash.dispatch";

/// Which service manager this machine uses. The project supports macOS
/// and Linux only, so there is no third case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Launchd,
    Systemd,
}

pub fn kind() -> Kind {
    if cfg!(target_os = "macos") {
        Kind::Launchd
    } else {
        Kind::Systemd
    }
}

/// Where the service file goes for this machine's service manager.
pub fn unit_path(kind: Kind) -> PathBuf {
    match kind {
        Kind::Launchd => config::home().join(format!("Library/LaunchAgents/{LABEL}.plist")),
        Kind::Systemd => config::home().join(format!(".config/systemd/user/{LABEL}.service")),
    }
}

/// The service file that runs `dispatch run` forever: started at login,
/// restarted if it dies, with its own output in the log directory next to
/// the session logs.
///
/// It carries a PATH of its own: launchd and systemd start a service with
/// a minimal PATH, which has none of the tools the dispatcher runs (`gh`,
/// `git`, the agent command lines live in Homebrew and under the home
/// directory), and a dispatcher that cannot find `gh` says "问 GitHub 要 …
/// 失败" once a minute forever.
pub fn unit(kind: Kind, program: &Path, config_path: &Path, logs: &Path) -> String {
    let path = tools_path();
    let out = logs.join("dispatch.out.log");
    let err = logs.join("dispatch.err.log");
    match kind {
        Kind::Launchd => format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{program}</string>
    <string>run</string>
    <string>--config</string>
    <string>{config_path}</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>{path}</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <!-- Ten seconds between restarts, so a settings file launchd cannot
       read fails ten times a minute instead of hundreds. -->
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{out}</string>
  <key>StandardErrorPath</key><string>{err}</string>
</dict>
</plist>
"#,
            program = escape(program),
            config_path = escape(config_path),
            out = escape(&out),
            err = escape(&err),
            path = escape(Path::new(&path)),
        ),
        Kind::Systemd => format!(
            "[Unit]\n\
             Description=theFlash dispatcher: starts one fresh agent session per piece of work\n\
             After=network-online.target\n\n\
             [Service]\n\
             ExecStart={program} run --config {config_path}\n\
             Environment=PATH={path}\n\
             Restart=always\n\
             RestartSec=10\n\
             StandardOutput=append:{out}\n\
             StandardError=append:{err}\n\n\
             [Install]\n\
             WantedBy=default.target\n",
            program = program.display(),
            config_path = config_path.display(),
            out = out.display(),
            err = err.display(),
            path = path,
        ),
    }
}

/// The PATH the service gets: the one the person installing it has, plus
/// the usual places, so the tools that are on PATH now are still found
/// after a reboot.
fn tools_path() -> String {
    let mut parts: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect();
    for fallback in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        if !parts.iter().any(|p| p == fallback) {
            parts.push(fallback.to_string());
        }
    }
    parts.join(":")
}

/// Where the service's copy of the program lives. The service never names
/// the program inside the repository, for two reasons found the hard way:
/// `cargo build` replaces `target/debug/dispatch` under a running service,
/// and a launchd agent loading a binary from a macOS-protected folder
/// (`~/Desktop`, `~/Documents`, `~/Downloads`) hangs in the dynamic loader
/// before `main` runs — the process sits in `open()` and the log stays
/// empty, which looks exactly like a program that does nothing.
pub fn installed_program() -> PathBuf {
    config::home().join(".local/libexec/theflash-dispatch")
}

/// Registers the service and starts it. `program` is this binary; a copy
/// of it is what the service runs.
pub fn install(
    kind: Kind,
    program: &Path,
    config_path: &Path,
    logs: &Path,
) -> Result<String, String> {
    let installed = installed_program();
    if let Some(dir) = installed.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建 {} 失败：{e}", dir.display()))?;
    }
    // Remove first: copying onto a file a running service is executing
    // would break that service's process.
    let _ = std::fs::remove_file(&installed);
    std::fs::copy(program, &installed).map_err(|e| {
        format!(
            "把 {} 复制到 {} 失败：{e}",
            program.display(),
            installed.display()
        )
    })?;
    let path = unit_path(kind);
    write(&path, &unit(kind, &installed, config_path, logs))?;
    std::fs::create_dir_all(logs).map_err(|e| format!("建 {} 失败：{e}", logs.display()))?;
    match kind {
        Kind::Launchd => {
            // bootout first, so installing twice replaces the running one
            // instead of failing with "service already loaded".
            let _ = launchctl(&["bootout", &format!("gui/{}/{LABEL}", uid())]);
            launchctl(&[
                "bootstrap",
                &format!("gui/{}", uid()),
                &path.display().to_string(),
            ])?;
        }
        Kind::Systemd => {
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", "--now", &format!("{LABEL}.service")])?;
        }
    }
    Ok(format!(
        "装好了：{}（跑的是 {}，仓库里的程序换了要重新 install）",
        path.display(),
        installed.display()
    ))
}

/// Stops the service and removes it. Sessions it already started keep
/// running: they are their own processes, and stopping an agent in the
/// middle of a commit is worse than letting it finish.
pub fn uninstall(kind: Kind) -> Result<String, String> {
    let path = unit_path(kind);
    match kind {
        Kind::Launchd => {
            let _ = launchctl(&["bootout", &format!("gui/{}/{LABEL}", uid())]);
        }
        Kind::Systemd => {
            let _ = systemctl(&["disable", "--now", &format!("{LABEL}.service")]);
        }
    }
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("删 {} 失败：{e}", path.display()))?;
    }
    let _ = std::fs::remove_file(installed_program());
    Ok(format!(
        "卸掉了：{}（已经启动的会话不受影响，它们是独立的进程）",
        path.display()
    ))
}

/// What the service manager says about the service, in one line.
pub fn running(kind: Kind) -> String {
    if !unit_path(kind).exists() {
        return "没装（scripts/dispatch install 装上）".to_string();
    }
    let out = match kind {
        Kind::Launchd => Command::new("launchctl")
            .args(["print", &format!("gui/{}/{LABEL}", uid())])
            .output(),
        Kind::Systemd => Command::new("systemctl")
            .args(["--user", "is-active", &format!("{LABEL}.service")])
            .output(),
    };
    match out {
        Ok(out) if out.status.success() => "装了，在跑".to_string(),
        Ok(_) => "装了，没在跑（看 logs 里的 dispatch.err.log）".to_string(),
        Err(e) => format!("问不出来：{e}"),
    }
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建 {} 失败：{e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("写 {} 失败：{e}", path.display()))
}

fn launchctl(args: &[&str]) -> Result<String, String> {
    run("launchctl", args)
}

fn systemctl(args: &[&str]) -> Result<String, String> {
    let mut all = vec!["--user"];
    all.extend_from_slice(args);
    run("systemctl", &all)
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("跑 {program} {} 失败：{e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "{program} {} 失败：{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// This user's id, which launchd needs to address a user service.
fn uid() -> String {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "501".to_string())
}

/// A path inside XML. A home directory with `&` in it would otherwise
/// produce a plist launchd refuses to read.
fn escape(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
