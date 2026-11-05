use anyhow::Result;
use std::path::Path;
use std::process::{Command, Stdio};

/// Starts a hidden background process that outlives this one. Output goes to `log` if given.
pub fn spawn_detached(exe: &Path, args: &[String], cwd: Option<&Path>, log: Option<&Path>) -> Result<u32> {
    let mut command = Command::new(exe);
    command.args(args).stdin(Stdio::null());
    match log {
        Some(path) => {
            let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
            command.stdout(file.try_clone()?).stderr(file);
        }
        None => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    hide(&mut command);
    Ok(command.spawn()?.id())
}

#[cfg(unix)]
fn hide(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

#[cfg(windows)]
fn hide(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

/// Hides the console window of a short-lived helper such as taskkill.
pub fn quiet(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Kills a process and all its descendants.
pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = quiet(Command::new("taskkill").args(["/pid", &pid.to_string(), "/t", "/f"]).stdout(Stdio::null()).stderr(Stdio::null())).status();
    }
    #[cfg(unix)]
    unsafe {
        // Session hosts and pty children lead their own process groups, which hold the whole tree.
        libc::kill(-(pid as i32), libc::SIGKILL);
        libc::kill(pid as i32, libc::SIGKILL);
    }
}

/// Asks a process to exit.
pub fn terminate(pid: u32) {
    #[cfg(windows)]
    kill_tree(pid);
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

pub fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, 0) == 0
    }
    #[cfg(windows)]
    {
        let out = quiet(Command::new("tasklist").args(["/fi", &format!("PID eq {pid}"), "/nh", "/fo", "csv"])).output();
        out.map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\""))).unwrap_or(false)
    }
}
