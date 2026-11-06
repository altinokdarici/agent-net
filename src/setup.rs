use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const DEVTUNNEL_DOWNLOADS: &str = "https://tunnelsassetsprod.blob.core.windows.net/cli/";

/// Finds an executable on PATH, trying the Windows extensions.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ""] } else { &[""] };
    std::env::split_paths(&path).find_map(|dir| exts.iter().map(|e| dir.join(format!("{name}{e}"))).find(|f| f.is_file()))
}

/// Where setup puts devtunnel when it is not on PATH.
pub fn bundled_devtunnel() -> PathBuf {
    crate::config::data_dir().join("bin").join(if cfg!(windows) { "devtunnel.exe" } else { "devtunnel" })
}

/// The devtunnel release file for an OS and CPU, or None when there is no build.
pub fn devtunnel_asset(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("windows", "x86_64") | ("windows", "aarch64") => "devtunnel.exe",
        ("linux", "x86_64") => "linux-x64-devtunnel",
        ("linux", "aarch64") => "linux-arm64-devtunnel",
        ("macos", "aarch64") => "osx-arm64-devtunnel",
        ("macos", "x86_64") => "osx-x64-devtunnel",
        _ => return None,
    })
}

fn quiet(program: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).output().ok()?;
    Some(format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn interactive(program: &str, args: &[&str]) -> bool {
    println!("> {program} {}", args.join(" "));
    Command::new(program).args(args).status().map(|s| s.success()).unwrap_or(false)
}

fn ensure_copilot() {
    if crate::copilot::copilot().is_ok() {
        println!("✓ Copilot CLI found.");
        return;
    }
    println!("Copilot CLI is missing. Installing it.");
    let installed = if cfg!(windows) && which("winget").is_some() {
        interactive("winget", &["install", "-e", "--id", "GitHub.Copilot", "--accept-source-agreements", "--accept-package-agreements"])
    } else if cfg!(target_os = "macos") && which("brew").is_some() {
        interactive("brew", &["install", "copilot-cli"])
    } else if which("npm").is_some() {
        interactive(if cfg!(windows) { "npm.cmd" } else { "npm" }, &["install", "-g", "@github/copilot"])
    } else {
        false
    };
    if installed {
        println!("✓ Copilot CLI installed. Open a new terminal if `copilot` is not found yet.");
    } else {
        println!("! Could not install Copilot CLI. See https://github.com/github/copilot-cli and run setup again.");
    }
}

async fn ensure_devtunnel() -> Result<PathBuf> {
    if let Some(found) = which("devtunnel") {
        println!("✓ devtunnel found at {}.", found.display());
        return Ok(found);
    }
    let target = bundled_devtunnel();
    if target.is_file() {
        println!("✓ devtunnel found at {}.", target.display());
        return Ok(target);
    }
    let asset = devtunnel_asset(std::env::consts::OS, std::env::consts::ARCH).context("There is no devtunnel build for this platform.")?;
    println!("Downloading devtunnel to {}.", target.display());
    let bytes = reqwest::Client::new().get(format!("{DEVTUNNEL_DOWNLOADS}{asset}")).timeout(Duration::from_secs(300)).send().await?.error_for_status()?.bytes().await?;
    std::fs::create_dir_all(target.parent().unwrap())?;
    std::fs::write(&target, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
    }
    println!("✓ devtunnel installed.");
    Ok(target)
}

fn ensure_login(devtunnel: &Path) -> Result<()> {
    let shown = quiet(devtunnel, &["user", "show"]).unwrap_or_default();
    if shown.contains("Logged in as") {
        println!("✓ devtunnel: {}", shown.lines().find(|l| l.contains("Logged in as")).unwrap_or("").trim());
        return Ok(());
    }
    println!("Sign in to Dev Tunnels with GitHub. Use the same account on every machine.");
    let headless = cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none();
    let args: &[&str] = if headless { &["user", "login", "-g", "-d"] } else { &["user", "login", "-g"] };
    if !interactive(&devtunnel.to_string_lossy(), args) {
        bail!("devtunnel sign-in failed. Run setup again.");
    }
    Ok(())
}

async fn wait_for_daemon() -> Option<String> {
    let port = crate::config::load_config(&crate::config::data_dir()).ok()?.port;
    let client = reqwest::Client::new();
    let url = |path: &str| format!("http://127.0.0.1:{port}{path}");
    for _ in 0..60 {
        if client.get(url("/version")).timeout(Duration::from_secs(2)).send().await.is_ok_and(|r| r.status().is_success()) {
            // Listing machines asks every peer, so it can take a few seconds.
            let r = client.get(url("/machines")).timeout(Duration::from_secs(60)).send().await.ok()?;
            return r.text().await.ok();
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    None
}

/// Installs what agent-net needs, signs in to Dev Tunnels, installs agent-net and checks
/// that the daemon joined the network. Safe to run again.
pub async fn setup() -> Result<()> {
    ensure_copilot();
    ensure_devtunnel().await?;
    ensure_login(&crate::tunnel::devtunnel_path())?;
    crate::install::install()?;
    println!("Waiting for the daemon to join the network.");
    match wait_for_daemon().await {
        Some(machines) => {
            let names: Vec<String> = serde_json::from_str::<serde_json::Value>(&machines)
                .ok()
                .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect()))
                .unwrap_or_default();
            println!("✓ agent-net is running. Machines: {}.", names.join(", "));
            println!("Start a new Copilot session to use the agent tool.");
            Ok(())
        }
        None => bail!("The daemon did not answer. See {}.", crate::config::data_dir().join("daemon.log").display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devtunnel_assets_cover_release_targets() {
        assert_eq!(devtunnel_asset("linux", "x86_64"), Some("linux-x64-devtunnel"));
        assert_eq!(devtunnel_asset("macos", "aarch64"), Some("osx-arm64-devtunnel"));
        assert_eq!(devtunnel_asset("windows", "x86_64"), Some("devtunnel.exe"));
        assert_eq!(devtunnel_asset("freebsd", "x86_64"), None);
    }
}
