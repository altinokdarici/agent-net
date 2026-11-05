use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub command: PathBuf,
    pub args: Vec<String>,
}

/// Finds the copilot executable without going through a shell. On Windows a real
/// copilot.exe wins over npm's .cmd shim, which ConPTY can only run through cmd.exe.
/// AGENT_NET_COPILOT overrides the search.
pub fn find_copilot(env: &dyn Fn(&str) -> Option<String>, exists: &dyn Fn(&Path) -> bool, windows: bool) -> Result<Command> {
    if let Some(path) = env("AGENT_NET_COPILOT").filter(|p| !p.is_empty()) {
        return Ok(Command { command: path.into(), args: vec![] });
    }
    let path = env("PATH").or_else(|| env("Path")).unwrap_or_default();
    let dirs: Vec<&str> = path.split(if windows { ';' } else { ':' }).filter(|d| !d.is_empty()).collect();
    let names: &[&str] = if windows { &["copilot.exe", "copilot.cmd"] } else { &["copilot"] };
    for name in names {
        for dir in &dirs {
            let file = Path::new(dir).join(name);
            if !exists(&file) {
                continue;
            }
            if name.ends_with(".cmd") {
                let shell = env("ComSpec").unwrap_or_else(|| "cmd.exe".into());
                return Ok(Command { command: shell.into(), args: vec!["/d".into(), "/s".into(), "/c".into(), file.to_string_lossy().into()] });
            }
            return Ok(Command { command: file, args: vec![] });
        }
    }
    bail!("Could not find copilot on PATH. Set AGENT_NET_COPILOT to its full path.")
}

pub fn copilot() -> Result<Command> {
    find_copilot(&|k| std::env::var(k).ok(), &|p| p.is_file(), cfg!(windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins() {
        let c = find_copilot(&|k| (k == "AGENT_NET_COPILOT").then(|| "/x/copilot".into()), &|_| false, false).unwrap();
        assert_eq!(c.command, PathBuf::from("/x/copilot"));
    }

    #[test]
    fn windows_prefers_exe_over_cmd() {
        let env = |k: &str| (k == "Path").then(|| r"C:\npm;C:\bin".to_string());
        let c = find_copilot(&env, &|p| p.to_string_lossy().ends_with("copilot.exe") && p.starts_with(r"C:\bin") || p.to_string_lossy().ends_with(".cmd"), true).unwrap();
        assert!(c.command.to_string_lossy().ends_with("copilot.exe"));
        let c = find_copilot(&env, &|p| p.to_string_lossy().ends_with(".cmd"), true).unwrap();
        assert_eq!(c.command, PathBuf::from("cmd.exe"));
        assert_eq!(c.args[3], Path::new(r"C:\npm").join("copilot.cmd").to_string_lossy());
    }

    #[test]
    fn missing_is_an_error() {
        assert!(find_copilot(&|_| None, &|_| false, false).is_err());
    }
}
