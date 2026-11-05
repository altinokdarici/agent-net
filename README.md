# agent-net

A network of GitHub Copilot CLI agents across your own machines. Any agent can start an agent on another machine, send it messages and get replies through one `agent()` tool. Machines connect over private Microsoft Dev Tunnels.

> Status: early development. Pieces land one pull request at a time.

## How it works

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

## Requirements

- Node.js 24 or later.
- GitHub Copilot CLI (`copilot`).
- Dev Tunnels CLI (`devtunnel`), signed in with `devtunnel user login -g -d`. Use the same account on every machine; machines only see tunnels owned by that account.

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

## Development

```sh
npm install
npm run check
```

TypeScript runs directly on Node 24; there is no build step. Run the daemon with `node src/daemon.ts`. Copilot sessions started in this repository load the extension from `.github/extensions/agent-net/`.
