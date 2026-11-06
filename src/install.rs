use crate::config::{data_dir, home_dir};
use crate::trust::copilot_home;
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// The Copilot extension, a self-contained ES module embedded in the binary.
pub const EXTENSION: &str = include_str!("../.github/extensions/agent-net/extension.mjs");

pub fn extension_path() -> PathBuf {
    copilot_home().join("extensions").join("agent-net").join("extension.mjs")
}

/// Where `install` keeps the executable, so updates and the login item have a fixed path.
pub fn installed_exe() -> PathBuf {
    data_dir().join("bin").join(if cfg!(windows) { "agent-net.exe" } else { "agent-net" })
}

/// Rewrites the user-level extension when it is installed and differs from this binary's.
pub fn refresh_extension() {
    let path = extension_path();
    if let Ok(current) = std::fs::read_to_string(&path) {
        if current != EXTENSION {
            let _ = std::fs::write(&path, EXTENSION);
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Step {
    pub command: Vec<String>,
    /// Steps that may fail, such as unloading something not loaded.
    pub may_fail: bool,
}

#[derive(Debug)]
pub struct Autostart {
    /// Files to write before running the commands.
    pub files: Vec<(PathBuf, String)>,
    pub install: Vec<Step>,
    pub uninstall: Vec<Step>,
}

fn step(command: &[&str], may_fail: bool) -> Step {
    Step { command: command.iter().map(|s| s.to_string()).collect(), may_fail }
}

fn xml(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// How to start the daemon at login on each OS, with the current PATH so it can find
/// copilot and devtunnel. Linux uses a systemd user service, macOS a LaunchAgent and
/// Windows a logon task that runs in a headless console, so no window appears.
pub fn autostart(exe: &Path, path_env: &str, home: &Path, os: &str, uid: u32) -> Result<Autostart> {
    let exe = exe.to_string_lossy();
    match os {
        "linux" => {
            let file = home.join(".config/systemd/user/agent-net.service");
            let content = [
                "[Unit]",
                "Description=agent-net daemon",
                "",
                "[Service]",
                &format!("ExecStart=\"{exe}\" daemon"),
                &format!("Environment=\"PATH={path_env}\""),
                "Restart=always",
                "RestartSec=10",
                // Session hosts are children of the daemon; restarting it must not end them.
                "KillMode=process",
                "",
                "[Install]",
                "WantedBy=default.target",
                "",
            ]
            .join("\n");
            Ok(Autostart {
                files: vec![(file, content)],
                install: vec![step(&["systemctl", "--user", "daemon-reload"], false), step(&["systemctl", "--user", "enable", "--now", "agent-net.service"], false)],
                uninstall: vec![step(&["systemctl", "--user", "disable", "--now", "agent-net.service"], true)],
            })
        }
        "macos" => {
            let file = home.join("Library/LaunchAgents/com.agent-net.daemon.plist");
            let domain = format!("gui/{uid}");
            let content = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.agent-net.daemon</string>
  <key>ProgramArguments</key>
  <array><string>{}</string><string>daemon</string></array>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>{}</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>AbandonProcessGroup</key><true/>
</dict>
</plist>
"#,
                xml(&exe),
                xml(path_env)
            );
            let f = file.to_string_lossy().to_string();
            Ok(Autostart {
                files: vec![(file, content)],
                install: vec![step(&["launchctl", "bootout", &domain, &f], true), step(&["launchctl", "bootstrap", &domain, &f], false)],
                uninstall: vec![step(&["launchctl", "bootout", &domain, &f], true)],
            })
        }
        "windows" => {
            let run = format!("conhost.exe --headless \"{exe}\" daemon");
            Ok(Autostart {
                files: vec![],
                install: vec![step(&["schtasks", "/create", "/f", "/tn", "agent-net", "/sc", "onlogon", "/rl", "limited", "/tr", &run], false), step(&[&*exe, "stop"], true), step(&["schtasks", "/run", "/tn", "agent-net"], false)],
                uninstall: vec![step(&["schtasks", "/end", "/tn", "agent-net"], true), step(&["schtasks", "/delete", "/f", "/tn", "agent-net"], true)],
            })
        }
        other => bail!("Starting at login is not supported on {other}."),
    }
}

fn uid() -> u32 {
    #[cfg(unix)]
    unsafe {
        libc::getuid()
    }
    #[cfg(windows)]
    0
}

/// Drops PATH entries that will not outlive this shell: missing folders, temp folders
/// and per-shell version manager folders.
pub fn stable_path(path: &str, temp: &Path) -> String {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let temp = temp.to_string_lossy().to_lowercase();
    let mut kept: Vec<&str> = Vec::new();
    for entry in path.split(separator) {
        let lower = entry.to_lowercase();
        let temporary = lower.starts_with(temp.trim_end_matches(['/', '\\'])) || lower.contains("/var/folders/") || lower.contains("fnm_multishells") || lower.contains("/tmp/");
        if !entry.is_empty() && !temporary && Path::new(entry).is_dir() && !kept.contains(&entry) {
            kept.push(entry);
        }
    }
    kept.join(&separator.to_string())
}

fn plan(exe: &Path) -> Result<Autostart> {
    let path = std::env::var("PATH").or_else(|_| std::env::var("Path")).unwrap_or_default();
    let path = stable_path(&path, &std::env::temp_dir());
    autostart(exe, &path, &home_dir(), std::env::consts::OS, uid())
}

fn run_steps(steps: &[Step]) -> Result<()> {
    for s in steps {
        let mut command = std::process::Command::new(&s.command[0]);
        command.args(&s.command[1..]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let ok = crate::process::quiet(&mut command).status().map(|st| st.success()).unwrap_or(false);
        if !ok && !s.may_fail {
            bail!("{} failed.", s.command.join(" "));
        }
    }
    Ok(())
}

/// Copies this executable to a fixed place, adds the extension to Copilot and starts the
/// daemon at login.
pub fn install() -> Result<()> {
    let exe = installed_exe();
    let current = std::env::current_exe()?;
    std::fs::create_dir_all(exe.parent().unwrap())?;
    if crate::config::absolute(&current) != crate::config::absolute(&exe) {
        // A running daemon may hold the old file; replace it the same way updates do.
        if exe.exists() {
            let temp = exe.with_extension("download");
            std::fs::copy(&current, &temp)?;
            let result = replace(&exe, &temp);
            let _ = std::fs::remove_file(&temp);
            result?;
        } else {
            std::fs::copy(&current, &exe)?;
        }
    }
    let ext = extension_path();
    std::fs::create_dir_all(ext.parent().unwrap())?;
    std::fs::write(&ext, EXTENSION)?;
    let login = plan(&exe)?;
    for (file, content) in &login.files {
        std::fs::create_dir_all(file.parent().unwrap())?;
        std::fs::write(file, content)?;
    }
    run_steps(&login.install)?;
    println!("Installed {} and the Copilot extension at {}.", exe.display(), ext.display());
    println!("The daemon starts at login and is starting now.");
    if let Some(dir) = exe.parent() {
        println!("Add {} to PATH to run agent-net from anywhere.", dir.display());
    }
    Ok(())
}

fn replace(target: &Path, source: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        // Renaming over a running executable is safe on Unix.
        std::fs::rename(source, target)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        let old = target.with_extension("old");
        let _ = std::fs::remove_file(&old);
        if target.exists() {
            std::fs::rename(target, &old)?;
        }
        std::fs::rename(source, target)?;
        Ok(())
    }
}

pub fn uninstall() -> Result<()> {
    let login = plan(&installed_exe())?;
    run_steps(&login.uninstall)?;
    for (file, _) in &login.files {
        let _ = std::fs::remove_file(file);
    }
    if let Some(dir) = extension_path().parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
    println!("Removed the extension and the login item. {} is left in place.", installed_exe().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn stable_path_drops_temporary_entries() {
        let path = "/usr/bin:/tmp/shim:/nonexistent-dir:/usr/bin:/bin:/x/fnm_multishells/1/bin";
        assert_eq!(stable_path(path, Path::new("/tmp")), "/usr/bin:/bin");
    }

    #[test]
    fn linux_unit_keeps_sessions_alive() {
        let a = autostart(Path::new("/h/.agent-net/bin/agent-net"), "/usr/bin", Path::new("/h"), "linux", 1000).unwrap();
        assert!(a.files[0].1.contains("ExecStart=\"/h/.agent-net/bin/agent-net\" daemon"));
        assert!(a.files[0].1.contains("KillMode=process"));
    }

    #[test]
    fn macos_plist_escapes() {
        let a = autostart(Path::new("/a&b/agent-net"), "/usr/bin", Path::new("/h"), "macos", 501).unwrap();
        assert!(a.files[0].1.contains("/a&amp;b/agent-net"));
        assert_eq!(a.install[1].command[2], "gui/501");
    }

    #[test]
    fn windows_runs_headless() {
        let a = autostart(Path::new(r"C:\a\agent-net.exe"), "", Path::new("C:/h"), "windows", 0).unwrap();
        assert!(a.install[0].command.last().unwrap().starts_with("conhost.exe --headless"));
    }

    #[test]
    fn extension_is_embedded() {
        assert!(EXTENSION.contains("joinSession"));
    }
}
