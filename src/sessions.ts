import { randomUUID } from "node:crypto";
import { mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { isAbsolute, join, relative, resolve } from "node:path";
import type { Config } from "./config.ts";
import type { Command } from "./copilot.ts";
import type { HostOptions } from "./host.ts";
import * as hostClient from "./host-client.ts";
import { ensureTrusted } from "./trust.ts";

export type SessionRecord = {
  id: string;
  name?: string;
  cwd: string;
  createdAt: string;
  /** Who started the session, for example "home/1b2c…". */
  from?: string;
};

export type SessionInfo = SessionRecord & { state: "running" | "sleeping" };

export type SessionDeps = {
  startHost: (options: HostOptions) => Promise<void>;
  stopHost: (dir: string, id: string) => Promise<void>;
  isRunning: (dir: string, id: string) => Promise<boolean>;
  trust: (folder: string) => Promise<unknown>;
  /** Hands a message to the agent-net extension inside a running session. */
  deliver: (dir: string, id: string, prompt: string) => Promise<boolean>;
  now: () => number;
};

const defaultDeps: SessionDeps = {
  startHost: options => hostClient.startHost(options),
  stopHost: (dir, id) => hostClient.send(dir, id, { type: "stop" }),
  isRunning: hostClient.isRunning,
  trust: folder => ensureTrusted(folder),
  deliver: deliverToExtension,
  now: Date.now,
};

/** True when folder is one of the allowed paths or inside one. */
export function isAllowed(folder: string, allowedPaths: string[]): boolean {
  const path = resolve(folder);
  return allowedPaths.some(root => {
    const rel = relative(resolve(root), path);
    return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
  });
}

/** Path of the file where a session's extension publishes its local message port. */
export function portFile(dir: string, id: string): string {
  return join(dir, "ports", `${id}.json`);
}

/** The prompt a session receives for a message, telling it who sent it and how to reply. */
export function formatMessage(text: string, from?: string): string {
  if (!from) return text;
  return `Message from agent ${from}:\n\n${text}\n\nTo reply, call the agent tool with action "send" and id "${from}".`;
}

async function hasPortFile(dir: string, id: string): Promise<boolean> {
  try {
    await readFile(portFile(dir, id));
    return true;
  } catch {
    return false;
  }
}

async function deliverToExtension(dir: string, id: string, prompt: string): Promise<boolean> {
  let port: number;
  try {
    port = JSON.parse(await readFile(portFile(dir, id), "utf8")).port;
  } catch {
    return false;
  }
  try {
    const response = await fetch(`http://127.0.0.1:${port}/message`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ prompt }),
    });
    return response.ok;
  } catch {
    return false;
  }
}

/**
 * Local Copilot sessions, each running hidden in a session host. A sleeping session has
 * no process; a new message wakes it by restarting copilot with the same session id.
 */
export class Sessions {
  private dir: string;
  private config: Config;
  private copilot: Command;
  private deps: SessionDeps;
  private queues = new Map<string, string[]>();
  private lastActive = new Map<string, number>();

  constructor(options: { dir: string; config: Config; copilot: Command; deps?: Partial<SessionDeps> }) {
    this.dir = options.dir;
    this.config = options.config;
    this.copilot = options.copilot;
    this.deps = { ...defaultDeps, ...options.deps };
  }

  private recordPath(id: string): string {
    return join(this.dir, "sessions", `${id}.json`);
  }

  async get(id: string): Promise<SessionRecord | undefined> {
    try {
      return JSON.parse(await readFile(this.recordPath(id), "utf8"));
    } catch {
      return undefined;
    }
  }

  /** True for a session this daemon started, or any local session whose extension is listening. */
  async has(id: string): Promise<boolean> {
    return (await this.get(id)) !== undefined || await hasPortFile(this.dir, id);
  }

  async list(): Promise<SessionInfo[]> {
    let files: string[] = [];
    try {
      files = await readdir(join(this.dir, "sessions"));
    } catch {
      return [];
    }
    const sessions: SessionInfo[] = [];
    for (const file of files.filter(f => f.endsWith(".json"))) {
      const record = await this.get(file.slice(0, -5));
      if (record) sessions.push(await this.info(record));
    }
    return sessions.sort((a, b) => a.createdAt.localeCompare(b.createdAt));
  }

  private async info(record: SessionRecord): Promise<SessionInfo> {
    return { ...record, state: await this.deps.isRunning(this.dir, record.id) ? "running" : "sleeping" };
  }

  /** Starts a new session. Remote callers may only use folders in allowedPaths. */
  async spawn(request: { cwd: string; prompt: string; name?: string; from?: string; remote?: boolean }): Promise<SessionInfo> {
    const cwd = resolve(request.cwd);
    if (request.remote && !isAllowed(cwd, this.config.allowedPaths)) throw new HttpError(403, `Folder is not in allowedPaths: ${cwd}`);
    if (!request.prompt) throw new HttpError(400, "prompt is required.");
    const record: SessionRecord = { id: randomUUID(), cwd, createdAt: new Date(this.deps.now()).toISOString() };
    if (request.name) record.name = request.name;
    if (request.from) record.from = request.from;
    await mkdir(join(this.dir, "sessions"), { recursive: true });
    await writeFile(this.recordPath(record.id), JSON.stringify(record, null, 2));
    await this.launch(record, request.prompt);
    return { ...record, state: "running" };
  }

  private async launch(record: SessionRecord, prompt: string): Promise<void> {
    await this.deps.trust(record.cwd);
    await this.deps.startHost({
      id: record.id,
      command: this.copilot.command,
      args: [...this.copilot.args, "--session-id", record.id, "--allow-all", "-i", prompt],
      cwd: record.cwd,
      dir: this.dir,
      env: { AGENT_NET_SESSION: record.id, AGENT_NET_HOME: this.dir },
    });
    this.lastActive.set(record.id, this.deps.now());
  }

  /**
   * Sends a message. A sleeping session wakes with the message as its prompt. A running
   * session gets it through its extension, or later if the extension is not ready yet.
   * Sessions this daemon did not start can only receive messages while their extension listens.
   */
  async send(id: string, text: string, from?: string): Promise<"delivered" | "queued" | "woke"> {
    if (!text) throw new HttpError(400, "text is required.");
    const prompt = formatMessage(text, from);
    const record = await this.get(id);
    if (!record) {
      if (await this.deps.deliver(this.dir, id, prompt)) return "delivered";
      throw new HttpError(404, `No session ${id}.`);
    }
    this.lastActive.set(id, this.deps.now());
    if (!(await this.deps.isRunning(this.dir, id))) {
      await this.launch(record, prompt);
      return "woke";
    }
    const queue = this.queues.get(id) ?? [];
    if (queue.length === 0 && await this.deps.deliver(this.dir, id, prompt)) return "delivered";
    queue.push(prompt);
    this.queues.set(id, queue);
    return "queued";
  }

  async read(id: string): Promise<SessionInfo> {
    const record = await this.get(id);
    if (!record) throw new HttpError(404, `No session ${id}.`);
    return this.info(record);
  }

  /** Ends a session for good: stops its process and forgets it. */
  async stop(id: string): Promise<void> {
    if (!(await this.get(id))) throw new HttpError(404, `No session ${id}.`);
    if (await this.deps.isRunning(this.dir, id)) await this.deps.stopHost(this.dir, id);
    this.queues.delete(id);
    this.lastActive.delete(id);
    await rm(this.recordPath(id), { force: true });
    await rm(portFile(this.dir, id), { force: true });
  }

  /** Delivers queued messages and puts sessions to sleep after idleMinutes without a message. */
  async tick(): Promise<void> {
    for (const [id, queue] of this.queues) {
      while (queue.length && await this.deps.deliver(this.dir, id, queue[0])) queue.shift();
      if (!queue.length) this.queues.delete(id);
    }
    const limit = this.config.idleMinutes * 60_000;
    for (const session of await this.list()) {
      if (session.state !== "running" || this.queues.has(session.id)) continue;
      const last = this.lastActive.get(session.id);
      // After a daemon restart, a running session gets a fresh idle window.
      if (last === undefined) this.lastActive.set(session.id, this.deps.now());
      if (last === undefined || this.deps.now() - last < limit) continue;
      await this.deps.stopHost(this.dir, session.id);
      await rm(portFile(this.dir, session.id), { force: true });
    }
  }
}

export class HttpError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}
