// agent-net Copilot extension. It gives every session the agent tool and listens on a
// local port, published in a port file, so the daemon can hand it incoming messages.
// The agent-net binary embeds this file and installs it. It must never write to stdout,
// which carries the CLI's JSON-RPC.
import { joinSession } from "@github/copilot-sdk/extension";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import http from "node:http";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";

const dir = process.env.AGENT_NET_HOME ?? join(homedir(), ".agent-net");

function daemonPort() {
  try {
    return JSON.parse(readFileSync(join(dir, "config.json"), "utf8")).port ?? 4590;
  } catch {
    return 4590;
  }
}

const description = [
  "Talk to other Copilot agents on your machines.",
  "machines: list machines in the network.",
  "list: list agents on every machine.",
  "spawn: start an agent on a machine (default: this one) in folder cwd with a prompt. It works on its own and messages you back when done.",
  "send: send text to the agent with this id. Replies arrive later as a new message, so do not wait or poll for them.",
  "status: show whether an agent is running or sleeping.",
  "stop: end an agent for good.",
  "upload: copy the local file path to path to on machine.",
  "download: copy the file path on machine to the local path to.",
  "Other machines only accept folders and files inside their allowed paths.",
].join("\n");

const actions = ["machines", "list", "spawn", "send", "status", "stop", "upload", "download"];

const parameters = {
  type: "object",
  properties: {
    action: { type: "string", enum: actions },
    machine: { type: "string", description: "spawn, upload, download: machine name from machines." },
    cwd: { type: "string", description: "spawn: folder the agent works in, on its machine." },
    prompt: { type: "string", description: "spawn: the task." },
    name: { type: "string", description: "spawn: optional short label." },
    id: { type: "string", description: "send, status, stop: the agent's session id." },
    text: { type: "string", description: "send: the message." },
    path: { type: "string", description: "upload: local file. download: file on machine." },
    to: { type: "string", description: "upload: destination on machine. download: local destination." },
  },
  required: ["action"],
};

/** The prompt a spawned agent starts with: the task, plus who to report back to. */
function spawnPrompt(prompt, from) {
  return `${prompt}\n\nYou were started by agent ${from}. When you finish, or if you are blocked, send your result to it: call the agent tool with action "send", id "${from}" and your answer as text.`;
}

function need(value, name) {
  if (!value) throw new Error(`${name} is required.`);
  return value;
}

async function call(method, path, body) {
  const port = daemonPort();
  let response;
  try {
    response = await fetch(`http://127.0.0.1:${port}${path}`, {
      method,
      headers: body === undefined ? {} : { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new Error(`The agent-net daemon is not running on port ${port}. Start it with "agent-net start".`);
  }
  return { status: response.status, body: await response.json() };
}

async function runAgent(args, self) {
  const session = id => `/sessions/${encodeURIComponent(need(id, "id"))}`;
  let reply;
  switch (args.action) {
    case "machines":
      reply = await call("GET", "/machines");
      break;
    case "list":
      reply = await call("GET", "/sessions");
      break;
    case "spawn":
      reply = await call("POST", "/sessions", {
        machine: args.machine,
        cwd: need(args.cwd, "cwd"),
        prompt: spawnPrompt(need(args.prompt, "prompt"), self),
        name: args.name,
        from: self,
      });
      break;
    case "send":
      reply = await call("POST", `${session(args.id)}/messages`, { text: need(args.text, "text"), from: self });
      break;
    case "status":
      reply = await call("GET", session(args.id));
      break;
    case "stop":
      reply = await call("DELETE", session(args.id));
      break;
    case "upload": {
      const content = (await readFile(resolve(need(args.path, "path")))).toString("base64");
      reply = await call("POST", "/files/write", { machine: need(args.machine, "machine"), path: need(args.to, "to"), content });
      break;
    }
    case "download": {
      reply = await call("POST", "/files/read", { machine: need(args.machine, "machine"), path: need(args.path, "path") });
      if (reply.status >= 400) break;
      const file = reply.body;
      const to = resolve(need(args.to, "to"));
      await mkdir(dirname(to), { recursive: true });
      await writeFile(to, Buffer.from(file.content, "base64"));
      reply = { status: 200, body: { from: `${file.machine}:${file.path}`, to, size: file.size } };
      break;
    }
    default:
      throw new Error(`Unknown action ${args.action}. Use one of: ${actions.join(", ")}.`);
  }
  if (reply.status >= 400) throw new Error(reply.body?.error ?? `Daemon returned ${reply.status}.`);
  return JSON.stringify(reply.body, null, 2);
}

let self = process.env.AGENT_NET_SESSION ?? "";
// Under some hosts (e.g. the Agents app's SDK server) joinSession never returns, and session
// creation waits on this extension. Give up quickly so the session still starts, just without the tool.
const session = await Promise.race([
  joinSession({
    tools: [{ name: "agent", description, parameters, skipPermission: true, handler: args => runAgent(args, self) }],
  }),
  new Promise(done => setTimeout(done, 5000).unref()),
]);
if (!session) process.exit(0);
self = session.sessionId;

const server = http.createServer((request, response) => {
  const chunks = [];
  request.on("data", chunk => chunks.push(chunk));
  request.on("end", async () => {
    try {
      const { prompt } = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      if (request.method !== "POST" || request.url !== "/message" || typeof prompt !== "string") throw new Error("Bad request.");
      await session.send({ prompt });
      response.writeHead(200).end("{}");
    } catch {
      response.writeHead(400).end("{}");
    }
  });
});
await new Promise(done => server.listen(0, "127.0.0.1", done));
const file = join(dir, "ports", `${self}.json`);
mkdirSync(dirname(file), { recursive: true });
writeFileSync(file, JSON.stringify({ port: server.address().port, pid: process.pid }));
process.on("exit", () => {
  // A reloaded extension for the same session may already own the file.
  try {
    if (JSON.parse(readFileSync(file, "utf8")).pid === process.pid) rmSync(file, { force: true });
  } catch {}
});
for (const signal of ["SIGTERM", "SIGINT"]) process.on(signal, () => process.exit(0));
