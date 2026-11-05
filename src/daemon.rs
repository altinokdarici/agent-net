use crate::config::{absolute, load_config, Config};
use crate::sessions::{is_allowed, HttpError, Sessions};
use crate::tunnel::{MachineTunnel, Peer};
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Marks a request forwarded from another machine. Such requests are never forwarded again.
pub const HOP_HEADER: &str = "x-agent-net-hop";

/// Largest file the files API moves in one request.
pub const MAX_FILE_BYTES: usize = 50 * 1024 * 1024;

type Reply = Result<(u16, Value), HttpError>;

fn text(body: &Map<String, Value>, key: &str) -> Option<String> {
    body.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from)
}

/// The daemon's HTTP API. The same API serves the local extension and, through the
/// machine's tunnel, other machines. Session ids are unique across the network, so a
/// request for an unknown id is asked of every peer.
pub struct Daemon {
    pub sessions: Sessions,
    pub tunnel: Option<Arc<MachineTunnel>>,
    pub machine: String,
    pub config: Config,
}

impl Daemon {
    pub fn router(self: Arc<Self>) -> axum::Router {
        axum::Router::new().fallback(handle).layer(axum::extract::DefaultBodyLimit::max(MAX_FILE_BYTES * 3 / 2)).with_state(self)
    }

    async fn route(&self, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Reply {
        let parts: Vec<String> = uri.path().split('/').filter(|p| !p.is_empty()).map(|p| percent_decode(p)).collect();
        let parts: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
        let remote = headers.contains_key(HOP_HEADER);
        let body: Map<String, Value> = if method == Method::POST && !body.is_empty() {
            match serde_json::from_slice(&body) {
                Ok(Value::Object(m)) => m,
                Ok(_) => return Err(HttpError::new(400, "Body must be a JSON object.")),
                Err(e) => return Err(HttpError::new(400, e.to_string())),
            }
        } else {
            Map::new()
        };
        match (method.as_str(), parts.as_slice()) {
            ("GET", ["machines"]) => return Ok((200, self.machines().await)),
            ("GET", ["version"]) => return Ok((200, json!({ "version": crate::VERSION }))),
            ("POST", ["files", action]) => return self.files(action, &body, remote).await,
            ("GET", ["sessions"]) => return Ok((200, self.list_all(remote).await)),
            ("POST", ["sessions"]) => return self.spawn(&body, remote).await,
            (_, ["sessions", id, ..]) => {
                let id = id.to_string();
                if !self.sessions.has(&id) {
                    if remote {
                        return Err(HttpError::new(404, format!("No session {id}.")));
                    }
                    return self.forward(&id, method.as_str(), uri.path(), &body).await;
                }
                match (method.as_str(), &parts[2..]) {
                    ("GET", []) => {
                        let mut v = self.sessions.read(&id).await?;
                        v["machine"] = self.machine.clone().into();
                        Ok((200, v))
                    }
                    ("DELETE", []) => {
                        self.sessions.stop(&id).await?;
                        Ok((200, json!({ "stopped": id })))
                    }
                    ("POST", ["messages"]) => {
                        let result = self.sessions.send(&id, &text(&body, "text").unwrap_or_default(), text(&body, "from").as_deref()).await?;
                        Ok((200, json!({ "id": id, "result": result })))
                    }
                    _ => Err(HttpError::new(404, "Not found.")),
                }
            }
            _ => Err(HttpError::new(404, "Not found.")),
        }
    }

    async fn peers(&self) -> Vec<Peer> {
        match &self.tunnel {
            Some(t) => t.peers().await.unwrap_or_default(),
            None => vec![],
        }
    }

    async fn machines(&self) -> Value {
        let mut peers = self.peers().await;
        if !peers.iter().any(|p| p.is_self) {
            peers.insert(0, Peer { name: self.machine.clone(), tunnel_id: String::new(), url: String::new(), is_self: true });
        }
        Value::Array(peers.into_iter().map(|p| json!({ "name": p.name, "self": p.is_self })).collect())
    }

    async fn list_all(&self, remote: bool) -> Value {
        let mut all: Vec<Value> = self.sessions.list().await.into_iter().map(|mut s| {
            s["machine"] = self.machine.clone().into();
            s
        }).collect();
        if !remote {
            if let Some(tunnel) = &self.tunnel {
                let others: Vec<Peer> = self.peers().await.into_iter().filter(|p| !p.is_self).collect();
                let lists = futures_join(others.iter().map(|peer| tunnel.fetch(peer, "GET", "/sessions", None))).await;
                for (status, body) in lists.into_iter().flatten() {
                    if status < 400 {
                        if let Value::Array(items) = body {
                            all.extend(items);
                        }
                    }
                }
            }
        }
        Value::Array(all)
    }

    /// The peer a request names in `machine`, or None when it is for this machine.
    async fn target(&self, machine: Option<String>, remote: bool) -> Result<Option<Peer>, HttpError> {
        let Some(machine) = machine else { return Ok(None) };
        if remote {
            return Ok(None);
        }
        let wanted = machine.to_lowercase();
        let peer = self.peers().await.into_iter().find(|p| p.name.to_lowercase() == wanted || p.tunnel_id == machine);
        if peer.is_none() && wanted != self.machine.to_lowercase() {
            return Err(HttpError::new(404, format!("No machine {machine}.")));
        }
        Ok(peer.filter(|p| !p.is_self))
    }

    /// Reads or writes a file on a machine. Content travels as base64. Requests from other
    /// machines may only touch files inside allowedPaths.
    async fn files(&self, action: &str, body: &Map<String, Value>, remote: bool) -> Reply {
        if action != "read" && action != "write" {
            return Err(HttpError::new(404, "Not found."));
        }
        if let Some(peer) = self.target(text(body, "machine"), remote).await? {
            return self.request(&peer, "POST", &format!("/files/{action}"), Some(&Value::Object(body.clone()))).await;
        }
        let Some(input) = text(body, "path") else { return Err(HttpError::new(400, "path is required.")) };
        let path = absolute(&input);
        if remote && !is_allowed(&path, &self.config.allowed_paths) {
            return Err(HttpError::new(403, format!("Path is not in allowedPaths: {}", path.display())));
        }
        let shown = path.to_string_lossy().to_string();
        if action == "read" {
            let data = tokio::fs::read(&path).await.map_err(|e| {
                let status = if e.kind() == std::io::ErrorKind::NotFound { 404 } else { 400 };
                HttpError::new(status, format!("Cannot read {shown}: {e}"))
            })?;
            if data.len() > MAX_FILE_BYTES {
                return Err(HttpError::new(413, format!("{shown} is larger than {MAX_FILE_BYTES} bytes.")));
            }
            let content = base64::engine::general_purpose::STANDARD.encode(&data);
            return Ok((200, json!({ "machine": self.machine, "path": shown, "size": data.len(), "content": content })));
        }
        let Some(content) = body.get("content").and_then(|c| c.as_str()) else { return Err(HttpError::new(400, "content is required.")) };
        let data = base64::engine::general_purpose::STANDARD.decode(content).map_err(|e| HttpError::new(400, format!("content is not base64: {e}")))?;
        write_file(&path, &data).await.map_err(|e| HttpError::new(400, format!("Cannot write {shown}: {e}")))?;
        Ok((200, json!({ "machine": self.machine, "path": shown, "size": data.len() })))
    }

    async fn spawn(&self, body: &Map<String, Value>, remote: bool) -> Reply {
        if let Some(peer) = self.target(text(body, "machine"), remote).await? {
            return self.request(&peer, "POST", "/sessions", Some(&Value::Object(body.clone()))).await;
        }
        let Some(cwd) = text(body, "cwd") else { return Err(HttpError::new(400, "cwd is required.")) };
        let mut v = self.sessions.spawn(&cwd, &text(body, "prompt").unwrap_or_default(), text(body, "name"), text(body, "from"), remote).await?;
        v["machine"] = self.machine.clone().into();
        Ok((201, v))
    }

    async fn forward(&self, id: &str, method: &str, path: &str, body: &Map<String, Value>) -> Reply {
        for peer in self.peers().await.into_iter().filter(|p| !p.is_self) {
            let payload = (method == "POST").then(|| Value::Object(body.clone()));
            // An unreachable machine cannot own the session; try the next one.
            if let Ok((status, value)) = self.request(&peer, method, path, payload.as_ref()).await {
                if status != 404 {
                    return Ok((status, value));
                }
            }
        }
        Err(HttpError::new(404, format!("No session {id}.")))
    }

    async fn request(&self, peer: &Peer, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let tunnel = self.tunnel.as_ref().ok_or_else(|| HttpError::new(500, "No network."))?;
        tunnel.fetch(peer, method, path, body).await.map_err(|e| HttpError::new(502, format!("{} did not answer: {e:#}", peer.name)))
    }
}

async fn write_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, data).await
}

async fn futures_join<F: std::future::Future<Output = anyhow::Result<(u16, Value)>> + Send>(futures: impl Iterator<Item = F>) -> Vec<Option<(u16, Value)>> {
    let mut out = vec![];
    let handles: Vec<_> = futures.map(|f| tokio::time::timeout(Duration::from_secs(20), f)).collect();
    for h in handles {
        out.push(h.await.ok().and_then(|r| r.ok()));
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn handle(axum::extract::State(daemon): axum::extract::State<Arc<Daemon>>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let (status, value) = match daemon.route(method, uri, headers, body).await {
        Ok(reply) => reply,
        Err(e) => (e.status, json!({ "error": e.message })),
    };
    (StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), axum::Json(value)).into_response()
}

pub fn state_path(dir: &Path) -> PathBuf {
    dir.join("daemon.json")
}

/// Starts the daemon on this machine: tunnel, session manager, HTTP API and updater.
/// It records its pid in daemon.json and shuts down cleanly on Ctrl+C or SIGTERM.
pub async fn run(dir: PathBuf) -> anyhow::Result<()> {
    let config = load_config(&dir)?;
    let machine = gethostname::gethostname().to_string_lossy().to_string();
    let exe = crate::current_exe()?;
    std::fs::create_dir_all(&dir)?;
    let listener = bind(config.port).await?;
    crate::install::refresh_extension();
    let sessions = Sessions::new(dir.clone(), config.clone(), crate::copilot::copilot()?, exe);
    let tunnel = Arc::new(MachineTunnel::new(&machine, config.port, dir.clone()));
    tunnel.start().await?;
    let daemon = Arc::new(Daemon { sessions, tunnel: Some(tunnel.clone()), machine, config: config.clone() });
    std::fs::write(state_path(&dir), json!({ "pid": std::process::id(), "port": config.port, "version": crate::VERSION }).to_string())?;
    crate::log(&format!("agent-net {} started on port {}", crate::VERSION, config.port));

    let ticker = daemon.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            ticker.sessions.tick().await;
        }
    });
    let (updated_tx, mut updated_rx) = tokio::sync::mpsc::channel::<()>(1);
    if config.auto_update {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            loop {
                match crate::update::update(false).await {
                    Ok(Some(version)) => {
                        crate::log(&format!("Updated to {version}. Restarting."));
                        let _ = updated_tx.send(()).await;
                        return;
                    }
                    Ok(None) => {}
                    Err(e) => crate::log(&format!("Update check failed: {e:#}")),
                }
                tokio::time::sleep(Duration::from_secs(60 * 60)).await;
            }
        });
    }

    let server = axum::serve(listener, daemon.router());
    let restart = tokio::select! {
        r = server => { r?; false }
        _ = shutdown_signal() => false,
        _ = updated_rx.recv() => true,
    };
    tunnel.stop();
    if let Ok(text) = std::fs::read_to_string(state_path(&dir)) {
        if serde_json::from_str::<Value>(&text).ok().and_then(|v| v["pid"].as_u64()) == Some(std::process::id() as u64) {
            let _ = std::fs::remove_file(state_path(&dir));
        }
    }
    if restart {
        crate::update::restart_daemon()?;
    }
    Ok(())
}

async fn bind(port: u16) -> anyhow::Result<tokio::net::TcpListener> {
    // A daemon that is restarting itself may still hold the port for a moment.
    for _ in 0..50 {
        if let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            return Ok(l);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(tokio::net::TcpListener::bind(("127.0.0.1", port)).await?)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(windows)]
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::Command;

    fn daemon(dir: &Path, allowed: Vec<PathBuf>) -> Daemon {
        let config = Config { allowed_paths: allowed, ..Config::default() };
        let sessions = Sessions::new(dir.to_path_buf(), config.clone(), Command { command: "copilot".into(), args: vec![] }, "agent-net".into());
        Daemon { sessions, tunnel: None, machine: "home".into(), config }
    }

    async fn call(d: &Daemon, method: Method, path: &str, remote: bool, body: Value) -> (u16, Value) {
        let mut headers = HeaderMap::new();
        if remote {
            headers.insert(HOP_HEADER, "1".parse().unwrap());
        }
        let bytes = if body.is_null() { Bytes::new() } else { Bytes::from(body.to_string()) };
        match d.route(method, path.parse().unwrap(), headers, bytes).await {
            Ok(r) => r,
            Err(e) => (e.status, json!({ "error": e.message })),
        }
    }

    #[tokio::test]
    async fn machines_lists_self() {
        let dir = tempfile::tempdir().unwrap();
        let (status, body) = call(&daemon(dir.path(), vec![]), Method::GET, "/machines", false, Value::Null).await;
        assert_eq!(status, 200);
        assert_eq!(body, json!([{ "name": "home", "self": true }]));
    }

    #[tokio::test]
    async fn files_round_trip_and_allowed_paths() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        let d = daemon(dir.path(), vec![shared.clone()]);
        let file = shared.join("a/b.txt").to_string_lossy().to_string();
        let content = base64::engine::general_purpose::STANDARD.encode("hello");
        let (status, _) = call(&d, Method::POST, "/files/write", true, json!({ "path": file, "content": content })).await;
        assert_eq!(status, 200);
        let (status, body) = call(&d, Method::POST, "/files/read", true, json!({ "path": file })).await;
        assert_eq!(status, 200);
        assert_eq!(body["content"], content);
        let outside = dir.path().join("secret.txt").to_string_lossy().to_string();
        let (status, _) = call(&d, Method::POST, "/files/read", true, json!({ "path": outside })).await;
        assert_eq!(status, 403);
        let (status, _) = call(&d, Method::POST, "/files/read", false, json!({ "path": outside })).await;
        assert_eq!(status, 404);
        let (status, _) = call(&d, Method::POST, "/files/read", false, json!({ "machine": "nowhere", "path": file })).await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn sessions_errors() {
        let dir = tempfile::tempdir().unwrap();
        let d = daemon(dir.path(), vec![]);
        assert_eq!(call(&d, Method::GET, "/sessions", false, Value::Null).await, (200, json!([])));
        assert_eq!(call(&d, Method::GET, "/sessions/nope", true, Value::Null).await.0, 404);
        assert_eq!(call(&d, Method::GET, "/sessions/nope", false, Value::Null).await.0, 404);
        assert_eq!(call(&d, Method::POST, "/sessions", true, json!({ "cwd": "/tmp/x", "prompt": "hi" })).await.0, 403);
        assert_eq!(call(&d, Method::POST, "/sessions", false, json!({ "prompt": "hi" })).await.0, 400);
        assert_eq!(call(&d, Method::POST, "/sessions", false, json!([1])).await.0, 400);
    }

    #[test]
    fn decodes_percent() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("abc%"), "abc%");
    }
}
