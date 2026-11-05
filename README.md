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

- GitHub Copilot CLI (`copilot`).
- Dev Tunnels CLI (`devtunnel`), signed in with `devtunnel user login -g -d`. Use the same account on every machine; machines only see tunnels owned by that account.
- While this repository is private, updates need GitHub access: `gh auth login` with an account that can read it, or `GITHUB_TOKEN`.

## Setup

Download the binary for your platform from the [latest release](https://github.com/altinokdarici/agent-net/releases/latest), then run:

```sh
./agent-net-<platform> install
```

`install` copies the binary to `~/.agent-net/bin/agent-net`, adds the extension to Copilot and starts the daemon at login.

| Command | Does |
|---|---|
| `agent-net start` / `stop` | Starts or stops the daemon in the background. |
| `agent-net run` | Runs the daemon in this terminal. |
| `agent-net ls` | Lists agents on every machine. |
| `agent-net attach <id>` | Watches and types into an agent on this machine. Ctrl+] detaches. |
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
