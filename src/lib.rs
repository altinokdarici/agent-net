pub mod config;
pub mod copilot;
pub mod daemon;
pub mod host;
pub mod install;
pub mod process;
pub mod sessions;
pub mod trust;
pub mod tunnel;
pub mod update;

/// Release builds get their version from CI; local builds use Cargo.toml's.
pub const VERSION: &str = match option_env!("AGENT_NET_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Only release builds update themselves, so a development build is never replaced.
pub const RELEASED: bool = option_env!("AGENT_NET_VERSION").is_some();

/// Prints a line to stderr and appends it to `<data dir>/daemon.log`, keeping the log under 1 MB.
pub fn log(message: &str) {
    use std::io::Write;
    eprintln!("{message}");
    let path = config::data_dir().join("daemon.log");
    if std::fs::metadata(&path).map(|m| m.len() > 1 << 20).unwrap_or(false) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{seconds} {message}");
    }
}
