use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::time::Duration;

pub const REPO: &str = "altinokdarici/agent-net";

/// Parses "v0.2.15" or "0.2.15" into comparable numbers.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// Name of the release asset built for this platform.
pub fn asset_name() -> String {
    format!("agent-net-{}{}", env!("TARGET"), if cfg!(windows) { ".exe" } else { "" })
}

/// A GitHub token, for when the repository is private: GITHUB_TOKEN, GH_TOKEN or `gh auth token`.
fn github_token() -> Option<String> {
    for key in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(t) = std::env::var(key) {
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    let mut command = std::process::Command::new("gh");
    crate::process::quiet(command.args(["auth", "token"]));
    let out = command.output().ok()?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !token.is_empty()).then_some(token)
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(format!("agent-net/{}", crate::VERSION)).timeout(Duration::from_secs(300)).build()?)
}

fn authorize(request: reqwest::RequestBuilder, token: &Option<String>) -> reqwest::RequestBuilder {
    match token {
        Some(t) => request.bearer_auth(t),
        None => request,
    }
}

/// Installs the latest release over this executable when it is newer. Returns the new
/// version, or None when already up to date. Development builds only update when forced.
pub async fn update(force: bool) -> Result<Option<String>> {
    if !crate::RELEASED && !force {
        return Ok(None);
    }
    let token = github_token();
    let http = client()?;
    let response = authorize(http.get(format!("https://api.github.com/repos/{REPO}/releases/latest")), &token).header("accept", "application/vnd.github+json").send().await?;
    if !response.status().is_success() {
        bail!("Could not read the latest release: GitHub returned {}. A private repository needs `gh auth login` or GITHUB_TOKEN.", response.status());
    }
    let release: Value = response.json().await?;
    let tag = release["tag_name"].as_str().context("The latest release has no tag")?.to_string();
    if crate::RELEASED && !is_newer(&tag, crate::VERSION) {
        return Ok(None);
    }
    let name = asset_name();
    let asset = release["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"] == name.as_str())).with_context(|| format!("Release {tag} has no {name}"))?;
    let url = asset["url"].as_str().context("Asset has no url")?;
    let response = authorize(http.get(url), &token).header("accept", "application/octet-stream").send().await?;
    if !response.status().is_success() {
        bail!("Could not download {name}: GitHub returned {}.", response.status());
    }
    let bytes = response.bytes().await?;
    let exe = std::env::current_exe()?;
    let temp = exe.with_extension("download");
    std::fs::write(&temp, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
    }
    let result = self_replace::self_replace(&temp);
    let _ = std::fs::remove_file(&temp);
    result.context("Could not replace the agent-net executable")?;
    Ok(Some(tag.trim_start_matches('v').to_string()))
}

/// Starts the updated daemon in place of this one. On Unix the process image is replaced,
/// keeping the pid that systemd or launchd watches. Windows starts a hidden successor.
pub fn restart_daemon() -> Result<()> {
    let exe = std::env::current_exe()?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = std::process::Command::new(&exe).arg("daemon").exec();
        Err(error.into())
    }
    #[cfg(windows)]
    {
        let log = crate::config::data_dir().join("daemon.log");
        crate::process::spawn_detached(&exe, &["daemon".into()], None, Some(&log))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.10", "0.2.9"));
        assert!(!is_newer("v0.2.9", "0.2.9"));
        assert!(!is_newer("garbage", "0.2.9"));
        assert!(is_newer("1.0.0", "0.9.99"));
    }

    #[test]
    fn asset_names_match_targets() {
        assert!(asset_name().starts_with("agent-net-"));
        assert_eq!(asset_name().ends_with(".exe"), cfg!(windows));
    }
}
