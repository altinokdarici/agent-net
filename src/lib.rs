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
