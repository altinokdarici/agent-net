use crate::config::{absolute, Config};
use crate::copilot::Command;
use crate::host::{self, HostOptions, HostRequest};
use crate::trust;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub message: String,
}

impl HttpError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        HttpError { status, message: message.into() }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HttpError {}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub cwd: PathBuf,
    pub created_at: String,
    /// Who started the session, for example "1b2c…".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// True when folder is one of the allowed paths or inside one.
pub fn is_allowed(folder: &Path, allowed: &[PathBuf]) -> bool {
    let path = absolute(folder);
    allowed.iter().any(|root| path.starts_with(absolute(root)))
}

/// Path of the file where a session's extension publishes its local message port.
pub fn port_file(dir: &Path, id: &str) -> PathBuf {
    dir.join("ports").join(format!("{id}.json"))
}

/// The prompt a session receives for a message, telling it who sent it and how to reply.
pub fn format_message(text: &str, from: Option<&str>) -> String {
    match from {
        None => text.to_string(),
        Some(from) => format!("Message from agent {from}:\n\n{text}\n\nTo reply, call the agent tool with action \"send\" and id \"{from}\"."),
    }
}

pub fn iso_now() -> String {
    let ms = crate::tunnel::now_ms();
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // Civil from days, Howard Hinnant's algorithm.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3600, rem % 3600 / 60, rem % 60, ms % 1000)
}

async fn deliver(dir: &Path, id: &str, prompt: &str) -> bool {
    let Ok(text) = tokio::fs::read_to_string(port_file(dir, id)).await else { return false };
    let Some(port) = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v["port"].as_u64()) else { return false };
    let client = reqwest::Client::new();
    match client.post(format!("http://127.0.0.1:{port}/message")).json(&serde_json::json!({ "prompt": prompt })).timeout(Duration::from_secs(30)).send().await {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    }
}

/// Local Copilot sessions, each running hidden in a session host. A sleeping session has
/// no process; a new message wakes it by restarting copilot with the same session id.
pub struct Sessions {
    pub dir: PathBuf,
    config: Config,
    copilot: Command,
    /// The agent-net executable, which runs session hosts.
    exe: PathBuf,
    queues: Mutex<HashMap<String, VecDeque<String>>>,
    last_active: Mutex<HashMap<String, Instant>>,
}

impl Sessions {
    pub fn new(dir: PathBuf, config: Config, copilot: Command, exe: PathBuf) -> Self {
        Sessions { dir, config, copilot, exe, queues: Default::default(), last_active: Default::default() }
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.dir.join("sessions").join(format!("{id}.json"))
    }

    pub fn get(&self, id: &str) -> Option<SessionRecord> {
        if id.is_empty() || id.contains(['/', '\\', '.']) {
            return None;
        }
        serde_json::from_str(&std::fs::read_to_string(self.record_path(id)).ok()?).ok()
    }

    /// True for a session this daemon started, or any local session whose extension is listening.
    pub fn has(&self, id: &str) -> bool {
        self.get(id).is_some() || (!id.contains(['/', '\\', '.']) && port_file(&self.dir, id).is_file())
    }

    pub async fn info(&self, record: SessionRecord) -> serde_json::Value {
        let running = host::is_running(&self.dir, &record.id).await;
        let mut v = serde_json::to_value(record).unwrap();
        v["state"] = (if running { "running" } else { "sleeping" }).into();
        v
    }

    pub async fn list(&self) -> Vec<serde_json::Value> {
        let mut records: Vec<SessionRecord> = std::fs::read_dir(self.dir.join("sessions"))
            .map(|entries| entries.flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").and_then(|id| self.get(id))).collect())
            .unwrap_or_default();
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let mut out = vec![];
        for r in records {
            out.push(self.info(r).await);
        }
        out
    }

    /// Starts a new session. Remote callers may only use folders in allowedPaths.
    pub async fn spawn(&self, cwd: &str, prompt: &str, name: Option<String>, from: Option<String>, remote: bool) -> Result<serde_json::Value, HttpError> {
        let cwd = absolute(cwd);
        if remote && !is_allowed(&cwd, &self.config.allowed_paths) {
            return Err(HttpError::new(403, format!("Folder is not in allowedPaths: {}", cwd.display())));
        }
        if prompt.is_empty() {
            return Err(HttpError::new(400, "prompt is required."));
        }
        if !cwd.is_dir() {
            std::fs::create_dir_all(&cwd).map_err(|e| HttpError::new(400, format!("Cannot create {}: {e}", cwd.display())))?;
        }
        let record = SessionRecord { id: uuid::Uuid::new_v4().to_string(), name, cwd, created_at: iso_now(), from };
        let _ = std::fs::create_dir_all(self.dir.join("sessions"));
        std::fs::write(self.record_path(&record.id), serde_json::to_string_pretty(&record).unwrap()).map_err(|e| HttpError::new(500, e.to_string()))?;
        self.launch(&record, prompt).await?;
        let mut v = serde_json::to_value(record).unwrap();
        v["state"] = "running".into();
        Ok(v)
    }

    async fn launch(&self, record: &SessionRecord, prompt: &str) -> Result<(), HttpError> {
        trust::ensure_trusted(&record.cwd, &trust::copilot_config_path()).map_err(|e| HttpError::new(500, format!("{e:#}")))?;
        let mut args = self.copilot.args.clone();
        args.extend(["--session-id".into(), record.id.clone(), "--allow-all".into(), "-i".into(), prompt.into()]);
        let options = HostOptions {
            id: record.id.clone(),
            command: self.copilot.command.clone(),
            args,
            cwd: record.cwd.clone(),
            dir: self.dir.clone(),
            env: HashMap::from([("AGENT_NET_SESSION".into(), record.id.clone()), ("AGENT_NET_HOME".into(), self.dir.to_string_lossy().into())]),
            scrollback: None,
        };
        host::start_host(&self.exe, &options).await.map_err(|e| HttpError::new(500, format!("{e:#}")))?;
        self.last_active.lock().unwrap().insert(record.id.clone(), Instant::now());
        Ok(())
    }

    /// Sends a message. A sleeping session wakes with the message as its prompt. A running
    /// session gets it through its extension, or later if the extension is not ready yet.
    /// Sessions this daemon did not start can only receive messages while their extension listens.
    pub async fn send(&self, id: &str, text: &str, from: Option<&str>) -> Result<&'static str, HttpError> {
        if text.is_empty() {
            return Err(HttpError::new(400, "text is required."));
        }
        let prompt = format_message(text, from);
        let Some(record) = self.get(id) else {
            if deliver(&self.dir, id, &prompt).await {
                return Ok("delivered");
            }
            return Err(HttpError::new(404, format!("No session {id}.")));
        };
        self.last_active.lock().unwrap().insert(id.into(), Instant::now());
        if !host::is_running(&self.dir, id).await {
            self.launch(&record, &prompt).await?;
            return Ok("woke");
        }
        let empty = self.queues.lock().unwrap().get(id).is_none_or(|q| q.is_empty());
        if empty && deliver(&self.dir, id, &prompt).await {
            return Ok("delivered");
        }
        self.queues.lock().unwrap().entry(id.into()).or_default().push_back(prompt);
        Ok("queued")
    }

    pub async fn read(&self, id: &str) -> Result<serde_json::Value, HttpError> {
        match self.get(id) {
            Some(r) => Ok(self.info(r).await),
            None => Err(HttpError::new(404, format!("No session {id}."))),
        }
    }

    /// Ends a session for good: stops its process and forgets it.
    pub async fn stop(&self, id: &str) -> Result<(), HttpError> {
        if self.get(id).is_none() {
            return Err(HttpError::new(404, format!("No session {id}.")));
        }
        if host::is_running(&self.dir, id).await {
            let _ = host::send(&self.dir, id, &HostRequest::Stop).await;
        }
        self.queues.lock().unwrap().remove(id);
        self.last_active.lock().unwrap().remove(id);
        let _ = std::fs::remove_file(self.record_path(id));
        let _ = std::fs::remove_file(port_file(&self.dir, id));
        Ok(())
    }

    /// Delivers queued messages and puts sessions to sleep after idleMinutes without a message.
    pub async fn tick(&self) {
        let ids: Vec<String> = self.queues.lock().unwrap().keys().cloned().collect();
        for id in ids {
            loop {
                let next = self.queues.lock().unwrap().get(&id).and_then(|q| q.front().cloned());
                let Some(prompt) = next else { break };
                if !deliver(&self.dir, &id, &prompt).await {
                    break;
                }
                if let Some(q) = self.queues.lock().unwrap().get_mut(&id) {
                    q.pop_front();
                }
            }
            let mut queues = self.queues.lock().unwrap();
            if queues.get(&id).is_some_and(|q| q.is_empty()) {
                queues.remove(&id);
            }
        }
        let limit = Duration::from_secs_f64(self.config.idle_minutes * 60.0);
        for session in self.list().await {
            let id = session["id"].as_str().unwrap_or_default().to_string();
            if session["state"] != "running" || self.queues.lock().unwrap().contains_key(&id) {
                continue;
            }
            let last = {
                let mut active = self.last_active.lock().unwrap();
                // After a daemon restart, a running session gets a fresh idle window.
                *active.entry(id.clone()).or_insert_with(Instant::now)
            };
            if last.elapsed() < limit {
                continue;
            }
            let _ = host::send(&self.dir, &id, &HostRequest::Stop).await;
            let _ = std::fs::remove_file(port_file(&self.dir, &id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_paths() {
        let roots = vec![absolute("/work")];
        assert!(is_allowed(Path::new("/work"), &roots));
        assert!(is_allowed(Path::new("/work/a/b"), &roots));
        assert!(!is_allowed(Path::new("/workshop"), &roots));
        assert!(!is_allowed(Path::new("/work/../etc"), &roots));
        assert!(!is_allowed(Path::new("/etc"), &[]));
    }

    #[test]
    fn formats_messages() {
        assert_eq!(format_message("hi", None), "hi");
        assert!(format_message("hi", Some("abc")).contains("id \"abc\""));
    }

    #[test]
    fn iso_now_looks_right() {
        let now = iso_now();
        assert_eq!(now.len(), 24);
        assert!(now.starts_with("20") && now.ends_with('Z'));
    }

    #[test]
    fn rejects_path_like_ids() {
        let s = Sessions::new(std::env::temp_dir(), Config::default(), Command { command: "x".into(), args: vec![] }, "x".into());
        assert!(!s.has("../config"));
        assert!(s.get("..").is_none());
    }
}
