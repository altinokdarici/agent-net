use crate::process;
use anyhow::{bail, Context, Result};
use base64::Engine;
use portable_pty::{CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostOptions {
    pub id: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub dir: PathBuf,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Bytes of terminal output kept for replay when a client attaches.
    #[serde(default)]
    pub scrollback: Option<usize>,
}

/// First line a client sends to a host. Attach turns the connection into a raw terminal stream.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HostRequest {
    Attach { cols: Option<u16>, rows: Option<u16> },
    Resize { cols: u16, rows: u16 },
    Write { data: String },
    Stop,
}

pub fn hosts_dir(dir: &Path) -> PathBuf {
    dir.join("hosts")
}

pub fn state_path(dir: &Path, id: &str) -> PathBuf {
    hosts_dir(dir).join(format!("{id}.json"))
}

/// Local socket of a session host: a named pipe on Windows, a Unix socket elsewhere.
/// Unix sockets live in /tmp because macOS caps socket paths at 104 bytes.
pub fn socket_path(dir: &Path, id: &str) -> String {
    if cfg!(windows) {
        return format!(r"\\.\pipe\agent-net-{id}");
    }
    let hash = Sha256::digest(dir.to_string_lossy().as_bytes());
    let tag: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("/tmp/agent-net-{tag}-{id}.sock")
}

pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

pub async fn connect(dir: &Path, id: &str) -> std::io::Result<Box<dyn Stream>> {
    let path = socket_path(dir, id);
    #[cfg(unix)]
    {
        Ok(Box::new(tokio::net::UnixStream::connect(path).await?))
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        for _ in 0..50 {
            match ClientOptions::new().open(&path) {
                Ok(pipe) => return Ok(Box::new(pipe)),
                // All instances are busy for a moment while the host creates the next one.
                Err(e) if e.raw_os_error() == Some(231) => tokio::time::sleep(Duration::from_millis(20)).await,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other("Session host pipe is busy."))
    }
}

pub async fn is_running(dir: &Path, id: &str) -> bool {
    connect(dir, id).await.is_ok()
}

/// Sends a one-shot request to a running host.
pub async fn send(dir: &Path, id: &str, request: &HostRequest) -> Result<()> {
    let mut stream = connect(dir, id).await?;
    stream.write_all(format!("{}\n", serde_json::to_string(request)?).as_bytes()).await?;
    stream.flush().await?;
    let mut sink = vec![];
    let _ = tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut sink).await;
    Ok(())
}

/// Opens a raw terminal stream: replays the scrollback, then streams live output and accepts input.
pub async fn attach(dir: &Path, id: &str, cols: u16, rows: u16) -> Result<Box<dyn Stream>> {
    let mut stream = connect(dir, id).await?;
    let request = HostRequest::Attach { cols: Some(cols), rows: Some(rows) };
    stream.write_all(format!("{}\n", serde_json::to_string(&request)?).as_bytes()).await?;
    Ok(stream)
}

/// Starts a session host as a detached, hidden process that outlives its parent.
pub async fn start_host(exe: &Path, options: &HostOptions) -> Result<()> {
    let arg = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(options)?);
    process::spawn_detached(exe, &["host".into(), arg], Some(&options.cwd), None)?;
    for _ in 0..100 {
        if is_running(&options.dir, &options.id).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("Session host {} did not start.", options.id)
}

struct Shared {
    buffer: Mutex<Vec<u8>>,
    output: broadcast::Sender<Vec<u8>>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    child_pid: Option<u32>,
}

impl Shared {
    fn write(&self, data: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(data);
        let _ = w.flush();
    }
    fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.lock().unwrap().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    }
}

/// Runs one command in a hidden pseudo-terminal and serves it on a local socket.
/// Returns the command's exit code once it exits.
pub async fn run_host(options: HostOptions) -> Result<i32> {
    let limit = options.scrollback.unwrap_or(1024 * 1024);
    let pty = portable_pty::native_pty_system();
    let pair = pty.openpty(PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 })?;
    let mut command = CommandBuilder::new(&options.command);
    command.args(&options.args);
    command.cwd(&options.cwd);
    command.env("TERM", "xterm-256color");
    for (k, v) in &options.env {
        command.env(k, v);
    }
    let mut child = pair.slave.spawn_command(command).with_context(|| format!("Could not start {}", options.command.display()))?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let (output, _) = broadcast::channel(1024);
    let shared = Arc::new(Shared { buffer: Mutex::new(vec![]), output, writer: Mutex::new(writer), master: Mutex::new(pair.master), child_pid: child.process_id() });

    let reading = shared.clone();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 16 * 1024];
        // ConPTY asks for the cursor position at startup and waits for an answer.
        let mut answered = !cfg!(windows);
        while let Ok(n) = reader.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if !answered && chunk[..n].windows(4).any(|w| w == b"\x1b[6n") {
                answered = true;
                reading.write(b"\x1b[1;1R");
            }
            let mut buffer = reading.buffer.lock().unwrap();
            buffer.extend_from_slice(&chunk[..n]);
            if buffer.len() > limit {
                let cut = buffer.len() - limit;
                buffer.drain(..cut);
            }
            let _ = reading.output.send(chunk[..n].to_vec());
        }
    });

    std::fs::create_dir_all(hosts_dir(&options.dir))?;
    let socket = socket_path(&options.dir, &options.id);
    serve(&socket, shared.clone())?;
    let state = serde_json::json!({ "id": options.id, "pid": std::process::id(), "childPid": shared.child_pid, "cwd": options.cwd });
    std::fs::write(state_path(&options.dir, &options.id), state.to_string())?;

    let code = tokio::task::spawn_blocking(move || child.wait().map(|s| s.exit_code() as i32).unwrap_or(1)).await?;
    let _ = std::fs::remove_file(state_path(&options.dir, &options.id));
    #[cfg(unix)]
    let _ = std::fs::remove_file(&socket);
    Ok(code)
}

#[cfg(unix)]
fn serve(path: &str, shared: Arc<Shared>) -> Result<()> {
    let _ = std::fs::remove_file(path);
    let listener = tokio::net::UnixListener::bind(path)?;
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(handle(Box::new(stream), shared.clone()));
        }
    });
    Ok(())
}

#[cfg(windows)]
fn serve(path: &str, shared: Arc<Shared>) -> Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let mut server = ServerOptions::new().first_pipe_instance(true).create(path)?;
    let path = path.to_string();
    tokio::spawn(async move {
        loop {
            if server.connect().await.is_err() {
                continue;
            }
            let connected = server;
            server = match ServerOptions::new().create(&path) {
                Ok(next) => next,
                Err(_) => return,
            };
            tokio::spawn(handle(Box::new(connected), shared.clone()));
        }
    });
    Ok(())
}

async fn handle(stream: Box<dyn Stream>, shared: Arc<Shared>) {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let mut line = String::new();
    if read.read_line(&mut line).await.is_err() {
        return;
    }
    let Ok(request) = serde_json::from_str::<HostRequest>(line.trim()) else { return };
    match request {
        HostRequest::Attach { cols, rows } => {
            if let (Some(c), Some(r)) = (cols, rows) {
                shared.resize(c, r);
            }
            let (replay, mut live) = {
                let buffer = shared.buffer.lock().unwrap();
                (buffer.clone(), shared.output.subscribe())
            };
            if write.write_all(&replay).await.is_err() {
                return;
            }
            let output = tokio::spawn(async move {
                loop {
                    match live.recv().await {
                        Ok(data) => {
                            if write.write_all(&data).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            });
            let pending = read.buffer().to_vec();
            if !pending.is_empty() {
                shared.write(&pending);
            }
            let mut chunk = vec![0u8; 8192];
            let mut inner = read.into_inner();
            loop {
                match tokio::io::AsyncReadExt::read(&mut inner, &mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = chunk[..n].to_vec();
                        let s = shared.clone();
                        let _ = tokio::task::spawn_blocking(move || s.write(&data)).await;
                    }
                }
            }
            output.abort();
        }
        HostRequest::Resize { cols, rows } => shared.resize(cols, rows),
        HostRequest::Write { data } => {
            let s = shared.clone();
            let _ = tokio::task::spawn_blocking(move || s.write(data.as_bytes())).await;
        }
        HostRequest::Stop => {
            let _ = write.shutdown().await;
            if let Some(pid) = shared.child_pid {
                process::kill_tree(pid);
            }
        }
    }
}

pub fn decode_options(arg: &str) -> Result<HostOptions> {
    Ok(serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(arg)?)?)
}
