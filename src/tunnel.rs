use crate::process;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::{Child, Command};

/// Label shared by every machine's tunnel in one user's network.
pub const NETWORK_LABEL: &str = "copilot-agent";

/// Dev Tunnels labels allow only [\w-=] and at most 50 characters.
pub fn machine_label(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect();
    format!("host-{clean}").chars().take(50).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct TunnelInfo {
    pub tunnel_id: String,
    pub labels: Vec<String>,
    pub description: String,
    pub ports: Vec<(u16, Option<String>)>,
}

fn tunnel_info(value: &Value) -> TunnelInfo {
    TunnelInfo {
        tunnel_id: value["tunnelId"].as_str().unwrap_or_default().to_string(),
        labels: value["labels"].as_array().map(|l| l.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        description: value["description"].as_str().unwrap_or_default().to_string(),
        ports: value["ports"]
            .as_array()
            .map(|ports| ports.iter().map(|p| (p["portNumber"].as_u64().unwrap_or(0) as u16, p["portUri"].as_str().map(String::from))).collect())
            .unwrap_or_default(),
    }
}

/// Parses the CLI's "2026-11-06 21:00:36 UTC" timestamps into Unix milliseconds.
pub fn parse_expiration(value: &str) -> Result<u64> {
    let bad = || anyhow!("Unexpected token expiration: {value}");
    let text = value.trim().trim_end_matches(" UTC");
    let (date, time) = text.split_once([' ', 'T']).ok_or_else(bad)?;
    let d: Vec<i64> = date.split('-').map(|x| x.parse()).collect::<Result<_, _>>().map_err(|_| bad())?;
    let t: Vec<i64> = time.trim_end_matches('Z').split(':').map(|x| x.parse::<f64>().map(|f| f as i64)).collect::<Result<_, _>>().map_err(|_| bad())?;
    if d.len() != 3 || t.len() != 3 {
        return Err(bad());
    }
    // Days from civil, Howard Hinnant's algorithm.
    let (y, m, day) = (if d[1] <= 2 { d[0] - 1 } else { d[0] }, d[1], d[2]);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Ok(((days * 86400 + t[0] * 3600 + t[1] * 60 + t[2]) * 1000) as u64)
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn devtunnel() -> Command {
    let mut command = Command::new(std::env::var("AGENT_NET_DEVTUNNEL").unwrap_or_else(|_| "devtunnel".into()));
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command.kill_on_drop(true);
    command
}

/// Runs the `devtunnel` CLI with JSON output. The CLI owns the user's sign-in.
pub async fn run(args: &[&str]) -> Result<Value> {
    let out = devtunnel().args(args).arg("--json").stdin(std::process::Stdio::null()).output().await.context("Could not run devtunnel. Is it installed and on PATH?")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        let text = format!("{}\n{}", stdout, String::from_utf8_lossy(&out.stderr));
        let detail = text.trim().lines().last().unwrap_or("").to_string();
        bail!("devtunnel {} failed: {}", args[0], detail);
    }
    // Some commands print a line of text before the JSON.
    let start = stdout.find('{').unwrap_or(0);
    Ok(serde_json::from_str(&stdout[start..]).unwrap_or(Value::Null))
}

pub async fn list(label: &str) -> Result<Vec<TunnelInfo>> {
    let v = run(&["list", "--labels", label]).await?;
    Ok(v["tunnels"].as_array().map(|t| t.iter().map(tunnel_info).collect()).unwrap_or_default())
}

pub async fn show(id: &str) -> Result<TunnelInfo> {
    Ok(tunnel_info(&run(&["show", id]).await?["tunnel"]))
}

pub async fn token(id: &str, scope: &str) -> Result<(String, u64)> {
    let v = run(&["token", id, "--scopes", scope]).await?;
    let token = v["token"].as_str().context("devtunnel token returned no token")?.to_string();
    Ok((token, parse_expiration(v["expiration"].as_str().unwrap_or_default())?))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Peer {
    pub name: String,
    pub tunnel_id: String,
    pub url: String,
    pub is_self: bool,
}

const REFRESH_MARGIN: u64 = 5 * 60_000;

/// One persistent private tunnel per machine, exposing only the daemon port.
pub struct MachineTunnel {
    name: String,
    port: u16,
    dir: PathBuf,
    pub tunnel_id: Mutex<Option<String>>,
    tokens: Mutex<HashMap<String, (String, u64)>>,
    urls: Mutex<HashMap<String, String>>,
    http: reqwest::Client,
}

impl MachineTunnel {
    pub fn new(name: &str, port: u16, dir: PathBuf) -> Self {
        MachineTunnel {
            name: name.into(),
            port,
            dir,
            tunnel_id: Mutex::new(None),
            tokens: Default::default(),
            urls: Default::default(),
            http: reqwest::Client::builder().timeout(Duration::from_secs(120)).build().expect("http client"),
        }
    }

    /// Finds this machine's tunnel, or creates it, and makes sure it forwards the daemon port.
    pub async fn ensure(&self) -> Result<String> {
        let label = machine_label(&self.name);
        let existing = list(NETWORK_LABEL).await?.into_iter().find(|t| t.labels.contains(&label));
        let tunnel = match existing {
            Some(t) => t,
            None => tunnel_info(&run(&["create", "--labels", NETWORK_LABEL, "--labels", &label, "--description", &self.name]).await?["tunnel"]),
        };
        let details = show(&tunnel.tunnel_id).await?;
        if !details.ports.iter().any(|(p, _)| *p == self.port) {
            run(&["port", "create", &tunnel.tunnel_id, "--port-number", &self.port.to_string(), "--protocol", "http"]).await?;
        }
        *self.tunnel_id.lock().unwrap() = Some(tunnel.tunnel_id.clone());
        Ok(tunnel.tunnel_id)
    }

    fn pid_file(&self) -> PathBuf {
        self.dir.join("tunnel.pid")
    }

    fn spawn_host(&self, id: &str) -> Result<Child> {
        let log = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("tunnel.log"))?;
        let child = devtunnel().args(["host", id]).stdin(std::process::Stdio::null()).stdout(log.try_clone()?).stderr(log).spawn().context("Could not run devtunnel host")?;
        if let Some(pid) = child.id() {
            std::fs::write(self.pid_file(), pid.to_string())?;
        }
        Ok(child)
    }

    /// Hosts the tunnel with `devtunnel host`, restarting it whenever it exits.
    pub async fn start(self: &std::sync::Arc<Self>) -> Result<()> {
        // A daemon that was killed leaves its devtunnel host behind.
        if let Ok(pid) = std::fs::read_to_string(self.pid_file()) {
            if let Ok(pid) = pid.trim().parse::<u32>() {
                process::kill_tree(pid);
            }
        }
        let id = self.ensure().await?;
        let mut child = self.spawn_host(&id)?;
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                let _ = child.wait().await;
                tokio::time::sleep(Duration::from_secs(5)).await;
                match me.spawn_host(&id) {
                    Ok(next) => child = next,
                    Err(e) => {
                        crate::log(&format!("{e:#}"));
                        tokio::time::sleep(Duration::from_secs(30)).await;
                        if let Ok(next) = me.spawn_host(&id) {
                            child = next;
                        }
                    }
                }
            }
        });
        Ok(())
    }

    pub fn stop(&self) {
        if let Ok(pid) = std::fs::read_to_string(self.pid_file()) {
            if let Ok(pid) = pid.trim().parse::<u32>() {
                process::kill_tree(pid);
            }
        }
        let _ = std::fs::remove_file(self.pid_file());
    }

    /// Every machine in the network, including this one.
    pub async fn peers(&self) -> Result<Vec<Peer>> {
        let own = self.tunnel_id.lock().unwrap().clone();
        let mut peers = vec![];
        for tunnel in list(NETWORK_LABEL).await? {
            let Some(url) = self.url(&tunnel.tunnel_id).await else { continue };
            let label = tunnel.labels.iter().find(|l| l.starts_with("host-")).map(|l| l[5..].to_string());
            let name = if !tunnel.description.is_empty() { tunnel.description.clone() } else { label.unwrap_or_else(|| tunnel.tunnel_id.clone()) };
            peers.push(Peer { name, is_self: Some(&tunnel.tunnel_id) == own.as_ref(), tunnel_id: tunnel.tunnel_id, url });
        }
        Ok(peers)
    }

    async fn url(&self, id: &str) -> Option<String> {
        if let Some(url) = self.urls.lock().unwrap().get(id) {
            return Some(url.clone());
        }
        let url = show(id).await.ok()?.ports.into_iter().find(|(p, _)| *p == self.port)?.1?;
        self.urls.lock().unwrap().insert(id.into(), url.clone());
        Some(url)
    }

    pub async fn token(&self, id: &str, scope: &str) -> Result<String> {
        let key = format!("{scope}:{id}");
        if let Some((token, expires)) = self.tokens.lock().unwrap().get(&key) {
            if expires.saturating_sub(REFRESH_MARGIN) > now_ms() {
                return Ok(token.clone());
            }
        }
        let fresh = token(id, scope).await?;
        self.tokens.lock().unwrap().insert(key, fresh.clone());
        Ok(fresh.0)
    }

    /// Sends an HTTP request to another machine's daemon through its tunnel.
    pub async fn fetch(&self, peer: &Peer, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value)> {
        let token = self.token(&peer.tunnel_id, "connect").await?;
        let url = format!("{}{}", peer.url.trim_end_matches('/'), path);
        let mut request = self.http.request(method.parse()?, url).header("X-Tunnel-Authorization", format!("tunnel {token}")).header(crate::daemon::HOP_HEADER, "1");
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let value = response.json().await.unwrap_or(Value::Null);
        Ok((status, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_valid() {
        assert_eq!(machine_label("Altinoks-MacBook.local"), "host-Altinoks-MacBook-local");
        assert_eq!(machine_label(&"x".repeat(80)).len(), 50);
    }

    #[test]
    fn parses_expiration() {
        assert_eq!(parse_expiration("1970-01-02 00:00:01 UTC").unwrap(), 86_401_000);
        assert_eq!(parse_expiration("2026-11-06 21:00:36 UTC").unwrap(), 1_793_998_836_000);
        assert!(parse_expiration("soon").is_err());
    }

    #[test]
    fn parses_tunnel_info() {
        let v = serde_json::json!({"tunnelId": "a.usw2", "labels": ["x"], "ports": [{"portNumber": 4590, "portUri": "https://u/"}]});
        let t = tunnel_info(&v);
        assert_eq!(t.ports, vec![(4590, Some("https://u/".into()))]);
        assert_eq!(t.description, "");
    }
}
