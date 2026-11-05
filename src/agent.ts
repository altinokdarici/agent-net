import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

export type Call = (method: string, path: string, body?: unknown) => Promise<{ status: number; body: unknown }>;

export type AgentArgs = {
  action: "machines" | "list" | "spawn" | "send" | "status" | "stop" | "upload" | "download";
  machine?: string;
  cwd?: string;
  prompt?: string;
  name?: string;
  id?: string;
  text?: string;
  path?: string;
  to?: string;
};

export const description = [
  "Talk to other Copilot agents on your machines.",
  "machines: list machines in the network.",
  "list: list agents on every machine.",
  "spawn: start an agent on a machine (default: this one) in folder cwd with a prompt. It works on its own and messages you back when done.",
  "send: send text to the agent with this id. Replies arrive later as a new message, so do not wait or poll for them.",
  "status: show whether an agent is running or sleeping.",
  "stop: end an agent for good.",
  "upload: copy the local file path to path to on machine.",
  "download: copy the file path on machine to the local path to.",
  "Other machines only accept files inside their allowed folders.",
].join("\n");

export const parameters = {
  type: "object",
  properties: {
    action: { type: "string", enum: ["machines", "list", "spawn", "send", "status", "stop", "upload", "download"] },
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
export function spawnPrompt(prompt: string, from: string): string {
  return `${prompt}\n\nYou were started by agent ${from}. When you finish, or if you are blocked, send your result to it: call the agent tool with action "send", id "${from}" and your answer as text.`;
}

function need(value: string | undefined, name: string): string {
  if (!value) throw new Error(`${name} is required.`);
  return value;
}

/** Runs one agent tool call against the local daemon. `self` is the calling session's id. */
export async function runAgent(args: AgentArgs, self: string, call: Call): Promise<string> {
  let reply: { status: number; body: unknown };
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
      reply = await call("POST", `/sessions/${encodeURIComponent(need(args.id, "id"))}/messages`, { text: need(args.text, "text"), from: self });
      break;
    case "status":
      reply = await call("GET", `/sessions/${encodeURIComponent(need(args.id, "id"))}`);
      break;
    case "stop":
      reply = await call("DELETE", `/sessions/${encodeURIComponent(need(args.id, "id"))}`);
      break;
    case "upload": {
      const content = (await readFile(resolve(need(args.path, "path")))).toString("base64");
      reply = await call("POST", "/files/write", { machine: need(args.machine, "machine"), path: need(args.to, "to"), content });
      break;
    }
    case "download": {
      reply = await call("POST", "/files/read", { machine: need(args.machine, "machine"), path: need(args.path, "path") });
      if (reply.status >= 400) break;
      const file = reply.body as { content: string; machine: string; path: string; size: number };
      const to = resolve(need(args.to, "to"));
      await mkdir(dirname(to), { recursive: true });
      await writeFile(to, Buffer.from(file.content, "base64"));
      reply = { status: 200, body: { from: `${file.machine}:${file.path}`, to, size: file.size } };
      break;
    }
    default:
      throw new Error(`Unknown action ${(args as { action: string }).action}.`);
  }
  if (reply.status >= 400) throw new Error((reply.body as { error?: string })?.error ?? `Daemon returned ${reply.status}.`);
  return JSON.stringify(reply.body, null, 2);
}

/** Calls the daemon's HTTP API on this machine. */
export function daemonCall(port: number): Call {
  return async (method, path, body) => {
    let response: Response;
    try {
      response = await fetch(`http://127.0.0.1:${port}${path}`, {
        method,
        headers: body === undefined ? {} : { "content-type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch {
      throw new Error(`The agent-net daemon is not running on port ${port}.`);
    }
    return { status: response.status, body: await response.json() };
  };
}
