# agent-net

A network of GitHub Copilot CLI agents across your own machines. Any agent can start an agent on another machine, send it messages and get replies through one `agent()` tool. Machines connect over private Microsoft Dev Tunnels.

> Status: early development.

## How it works

- **One binary:** `agent-net` is a single executable for Windows, macOS and Linux. It updates itself from GitHub releases.
- **Daemon:** one per machine. It hosts the machine's private Dev Tunnel, finds the other machines and routes messages by session ID.
- **Session host:** each agent is a normal interactive `copilot` session running hidden in a pseudo-terminal. You can attach to it to watch or type.
- **Extension:** loaded into every session. It delivers incoming messages as turns and provides the `agent()` tool.

Idle sessions are stopped after a while and resumed with the same session ID when a new message arrives.

## The agent tool

| Action | Arguments | Does |
|---|---|---|
| `machines` | | Lists machines in the network. |
| `list` | | Lists agents on every machine. |
| `spawn` | `cwd`, `prompt`, optional `machine` and `name` | Starts an agent. It messages the caller back when done. |
| `send` | `id`, `text` | Sends a message. Replies arrive later as a new message; there is no polling. |
| `status` | `id` | Shows whether an agent is running or sleeping. |
| `stop` | `id` | Ends an agent for good. |
| `upload` | `path`, `machine`, `to` | Copies a local file to another machine. |
| `download` | `machine`, `path`, `to` | Copies a file from another machine. |

## Requirements

- GitHub Copilot CLI (`copilot`), signed in.
- Dev Tunnels CLI (`devtunnel`), signed in with GitHub. Use the same GitHub account on every machine; machines only see tunnels owned by that account.

No Node or npm install is needed. `agent-net setup` installs what is missing.

## Setup

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/altinokdarici/agent-net/main/install.ps1 | iex
```

macOS and Linux:

```sh
case "$(uname -s)-$(uname -m)" in Darwin-arm64) t=aarch64-apple-darwin;; Darwin-x86_64) t=x86_64-apple-darwin;; Linux-aarch64|Linux-arm64) t=aarch64-unknown-linux-musl;; *) t=x86_64-unknown-linux-musl;; esac; curl -fsSL -o /tmp/agent-net "https://github.com/altinokdarici/agent-net/releases/latest/download/agent-net-$t" && chmod +x /tmp/agent-net && /tmp/agent-net setup
```

`setup` can be run again at any time. It:

1. Installs Copilot CLI if it is missing (winget, Homebrew or npm).
2. Downloads `devtunnel` to `~/.agent-net/bin` if it is not on PATH.
3. Signs in to Dev Tunnels with GitHub if needed (a browser, or a device code on a headless Linux machine).
4. Runs `install`: copies the binary to `~/.agent-net/bin/agent-net`, adds the extension to Copilot and starts the daemon at login.
5. Waits for the daemon and prints the machines it can see.

### Install with an agent

Paste this into Copilot CLI (or any coding agent) on the new machine:

```text
Install agent-net on this machine. Run the official installer from https://github.com/altinokdarici/agent-net#setup: on Windows, `irm https://raw.githubusercontent.com/altinokdarici/agent-net/main/install.ps1 | iex` in PowerShell; on macOS or Linux, the one-line curl command from that README section. Run it in a terminal I can see, because it may ask me to sign in to Dev Tunnels with GitHub; when it shows a sign-in link or device code, show it to me and wait. Use the same GitHub account as my other agent-net machines. When it finishes, run `agent-net version` (from ~/.agent-net/bin if it is not on PATH) and report the version and the machine list setup printed. If anything fails, show the last lines of ~/.agent-net/daemon.log and ~/.agent-net/tunnel.log.
```

| Command | Does |
|---|---|
| `agent-net start` / `stop` | Starts or stops the daemon in the background. |
| `agent-net run` | Runs the daemon in this terminal. |
| `agent-net ls` | Lists agents on every machine. |
| `agent-net attach <id>` | Watches and types into an agent on this machine. Ctrl+] detaches. |
| `agent-net setup` | Installs prerequisites, signs in to Dev Tunnels and runs `install`. |
| `agent-net install` / `uninstall` | Adds or removes the binary, the Copilot extension and the login item: a systemd user service on Linux, a LaunchAgent on macOS, a logon task on Windows. |
| `agent-net update` | Installs the latest release now. |
| `agent-net version` | Prints the version. |

## Updates

Every push to `main` publishes a release. The daemon checks for a newer one at start and every hour, replaces its own binary, refreshes the Copilot extension and restarts. Running agents are not interrupted. Set `"autoUpdate": false` to turn this off. Local development builds never update themselves. Update results and daemon errors go to `~/.agent-net/daemon.log`.

## Configuration

`~/.agent-net/config.json` (set `AGENT_NET_HOME` to use another folder):

```json
{
  "allowedPaths": ["/home/me/repo"],
  "idleMinutes": 30,
  "port": 4590
}
```

| Setting | Default | Meaning |
|---|---|---|
| `allowedPaths` | `[]` | Folders remote agents may start sessions in and transfer files to or from. |
| `idleMinutes` | `30` | Minutes a session may sit idle before it is stopped. |
| `port` | `4590` | Local port of the daemon's HTTP API. |
| `autoUpdate` | `true` | Installs new releases automatically. |

## Development

```sh
cargo test
cargo run -- run      # the daemon in this terminal
```

Copilot sessions started in this repository load the extension from `.github/extensions/agent-net/`.
