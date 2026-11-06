use agent_net::config::{data_dir, load_config};
use agent_net::{daemon, host, install, process, update, VERSION};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::{Read, Write};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const USAGE: &str = "Usage: agent-net <command>

  start            Start the daemon in the background.
  stop             Stop the background daemon.
  run              Run the daemon in this terminal.
  ls               List agents on every machine.
  attach <id>      Watch and type into a local agent. Ctrl+] detaches.
  setup            Install prerequisites, sign in to Dev Tunnels and install agent-net.
  install          Install agent-net, add the extension to Copilot and start the daemon at login.
  uninstall        Undo install.
  update           Install the latest release now.
  version          Print the version.";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(args).await {
        agent_net::log(&format!("error: {e:#}"));
        std::process::exit(1);
    }
}

async fn run(args: Vec<String>) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("start") => start().await,
        Some("stop") => stop(),
        Some("run") | Some("daemon") => daemon::run(data_dir()).await,
        Some("host") => {
            let options = host::decode_options(args.get(1).context("host needs its options")?)?;
            let code = host::run_host(options).await?;
            std::process::exit(code);
        }
        Some("ls") => ls().await,
        Some("attach") => attach(args.get(1)).await,
        Some("setup") => agent_net::setup::setup().await,
        Some("install") => install::install(),
        Some("uninstall") => install::uninstall(),
        Some("update") => match update::update(true).await? {
            Some(v) => {
                println!("Updated to {v}. Restart the daemon with \"agent-net stop\" and \"agent-net start\" if it is not managed by install.");
                Ok(())
            }
            None => {
                println!("agent-net {VERSION} is up to date.");
                Ok(())
            }
        },
        Some("version") | Some("--version") | Some("-V") => {
            println!("{VERSION}");
            Ok(())
        }
        _ => {
            println!("{USAGE}");
            Ok(())
        }
    }
}

async fn api(path: &str) -> Result<Value> {
    let port = load_config(&data_dir())?.port;
    let response = reqwest::Client::new().get(format!("http://127.0.0.1:{port}{path}")).timeout(Duration::from_secs(60)).send().await?;
    let ok = response.status().is_success();
    let body: Value = response.json().await?;
    if !ok {
        bail!("{}", body["error"].as_str().unwrap_or("The daemon returned an error."));
    }
    Ok(body)
}

async fn daemon_up() -> bool {
    api("/version").await.is_ok()
}

async fn start() -> Result<()> {
    if daemon_up().await {
        println!("The daemon is already running.");
        return Ok(());
    }
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let log = dir.join("daemon.log");
    process::spawn_detached(&agent_net::current_exe()?, &["daemon".into()], None, Some(&log))?;
    for _ in 0..300 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if daemon_up().await {
            println!("The daemon is running.");
            return Ok(());
        }
    }
    bail!("The daemon did not start. See {}.", log.display())
}

fn stop() -> Result<()> {
    let dir = data_dir();
    let state = std::fs::read_to_string(daemon::state_path(&dir)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let Some(pid) = state.and_then(|s| s["pid"].as_u64()) else {
        println!("The daemon is not running.");
        return Ok(());
    };
    process::terminate(pid as u32);
    for _ in 0..50 {
        if !process::alive(pid as u32) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Windows cannot signal the daemon to shut down, so its tunnel host is ended here.
    if let Ok(t) = std::fs::read_to_string(dir.join("tunnel.pid")) {
        if let Ok(tp) = t.trim().parse::<u32>() {
            process::kill_tree(tp);
        }
    }
    let _ = std::fs::remove_file(dir.join("tunnel.pid"));
    let _ = std::fs::remove_file(daemon::state_path(&dir));
    println!("The daemon is stopped.");
    Ok(())
}

async fn ls() -> Result<()> {
    let sessions = api("/sessions").await?;
    let list = sessions.as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        println!("No agents.");
    }
    for s in list {
        let field = |k: &str| s[k].as_str().unwrap_or("-").to_string();
        println!("{}\t{}\t{}\t{}\t{}", field("machine"), field("id"), field("state"), field("name"), field("cwd"));
    }
    Ok(())
}

/// Leaves the alternate screen and turns off the mouse, paste and keyboard modes Copilot turns on.
const RESET_TERMINAL: &str = "\x1b[?1049l\x1b[?2004l\x1b[?1004l\x1b[?1003l\x1b[?1006l\x1b[?1000l\x1b[>4;0m\x1b[<u\x1b[?25h\x1b[0m";

async fn attach(prefix: Option<&String>) -> Result<()> {
    let prefix = prefix.context("Usage: agent-net attach <id>")?;
    let dir = data_dir();
    let machine = gethostname::gethostname().to_string_lossy().to_string();
    let sessions = api("/sessions").await?;
    let matches: Vec<String> = sessions
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["id"].as_str().is_some_and(|id| id.starts_with(prefix.as_str())) && s["state"] == "running" && s["machine"] == machine.as_str())
        .filter_map(|s| s["id"].as_str().map(String::from))
        .collect();
    if matches.len() != 1 {
        bail!(if matches.is_empty() { format!("No running agent {prefix} on this machine.") } else { format!("{prefix} matches more than one agent.") });
    }
    let id = matches[0].clone();
    let size = || crossterm::terminal::size().unwrap_or((120, 30));
    let (cols, rows) = size();
    let stream = host::attach(&dir, &id, cols, rows).await?;
    let (mut read, mut write) = tokio::io::split(stream);
    #[cfg(windows)]
    let _ = crossterm::ansi_support::supports_ansi();
    crossterm::terminal::enable_raw_mode()?;

    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut chunk = [0u8; 4096];
        while let Ok(n) = stdin.read(&mut chunk) {
            if n == 0 || input_tx.blocking_send(chunk[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let output = tokio::spawn(async move {
        let mut chunk = vec![0u8; 16 * 1024];
        let mut stdout = std::io::stdout();
        while let Ok(n) = read.read(&mut chunk).await {
            if n == 0 {
                break;
            }
            let _ = stdout.write_all(&chunk[..n]);
            let _ = stdout.flush();
        }
    });
    let mut last = (cols, rows);
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    let mut output = output;
    loop {
        tokio::select! {
            data = input_rx.recv() => {
                let Some(data) = data else { break };
                if let Some(at) = data.iter().position(|b| *b == 0x1d) {
                    let _ = write.write_all(&data[..at]).await;
                    break;
                }
                if write.write_all(&data).await.is_err() { break; }
            }
            _ = &mut output => break,
            _ = ticker.tick() => {
                let now = size();
                if now != last {
                    last = now;
                    let _ = host::send(&dir, &id, &host::HostRequest::Resize { cols: now.0, rows: now.1 }).await;
                }
            }
        }
    }
    output.abort();
    crossterm::terminal::disable_raw_mode()?;
    print!("{RESET_TERMINAL}\r\nDetached.\r\n");
    let _ = std::io::stdout().flush();
    std::process::exit(0);
}
