use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Folders that remote agents may start sessions in and transfer files to or from.
    pub allowed_paths: Vec<PathBuf>,
    /// Minutes a session may sit idle before it is put to sleep.
    pub idle_minutes: f64,
    /// Local port of the daemon's HTTP API.
    pub port: u16,
    /// Whether the daemon installs new releases on its own.
    pub auto_update: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { allowed_paths: vec![], idle_minutes: 30.0, port: 4590, auto_update: true }
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn data_dir() -> PathBuf {
    match std::env::var_os("AGENT_NET_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => home_dir().join(".agent-net"),
    }
}

/// Makes a path absolute without touching the file system, like Node's path.resolve.
pub fn absolute(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn parse_config(value: &Value) -> Result<Config> {
    let Some(input) = value.as_object() else { bail!("config.json must be a JSON object.") };
    let mut config = Config::default();
    if let Some(paths) = input.get("allowedPaths") {
        let Some(list) = paths.as_array() else { bail!("allowedPaths must be an array of folder paths.") };
        let mut out = vec![];
        for path in list {
            match path.as_str() {
                Some(p) if !p.is_empty() => out.push(absolute(p)),
                _ => bail!("allowedPaths must be an array of folder paths."),
            }
        }
        config.allowed_paths = out;
    }
    if let Some(idle) = input.get("idleMinutes") {
        match idle.as_f64() {
            Some(n) if n > 0.0 => config.idle_minutes = n,
            _ => bail!("idleMinutes must be a positive number."),
        }
    }
    if let Some(port) = input.get("port") {
        match port.as_u64() {
            Some(n) if (1..=65535).contains(&n) => config.port = n as u16,
            _ => bail!("port must be an integer from 1 to 65535."),
        }
    }
    if let Some(auto) = input.get("autoUpdate") {
        let Some(b) = auto.as_bool() else { bail!("autoUpdate must be true or false.") };
        config.auto_update = b;
    }
    Ok(config)
}

pub fn load_config(dir: &Path) -> Result<Config> {
    let path = dir.join("config.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_config(&serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_for_empty_object() {
        assert_eq!(parse_config(&json!({})).unwrap(), Config::default());
    }

    #[test]
    fn parses_settings() {
        let c = parse_config(&json!({"allowedPaths": ["/a/b/../c"], "idleMinutes": 5, "port": 1234, "autoUpdate": false})).unwrap();
        assert_eq!(c.allowed_paths, vec![absolute("/a/c")]);
        assert_eq!(c.idle_minutes, 5.0);
        assert_eq!(c.port, 1234);
        assert!(!c.auto_update);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(parse_config(&json!([])).is_err());
        assert!(parse_config(&json!({"allowedPaths": [""]})).is_err());
        assert!(parse_config(&json!({"idleMinutes": 0})).is_err());
        assert!(parse_config(&json!({"port": 70000})).is_err());
    }
}
