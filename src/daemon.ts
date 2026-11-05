import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { hostname } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { dataDir, loadConfig } from "./config.ts";
import { findCopilot } from "./copilot.ts";
import { DevtunnelCli } from "./devtunnel.ts";
import { HttpError, Sessions } from "./sessions.ts";
import { MachineTunnel } from "./tunnel.ts";
import type { Peer } from "./tunnel.ts";

/** Marks a request forwarded from another machine. Such requests are never forwarded again. */
export const HOP_HEADER = "x-agent-net-hop";

export type Network = {
  peers(): Promise<Peer[]>;
  fetch(peer: Peer, path: string, init?: RequestInit): Promise<Response>;
};

type Reply = { status: number; body: unknown };

async function readBody(request: http.IncomingMessage): Promise<Record<string, unknown>> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) chunks.push(chunk as Buffer);
  const text = Buffer.concat(chunks).toString("utf8");
  if (!text) return {};
  const value = JSON.parse(text);
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new HttpError(400, "Body must be a JSON object.");
  return value;
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value ? value : undefined;
}

/**
 * The daemon's HTTP API. The same API serves the local extension and, through the
 * machine's tunnel, other machines. Session ids are unique across the network, so a
 * request for an unknown id is asked of every peer.
 */
export class Daemon {
  private sessions: Sessions;
  private network?: Network;
  private machine: string;

  constructor(options: { sessions: Sessions; machine: string; network?: Network }) {
    this.sessions = options.sessions;
    this.machine = options.machine;
    this.network = options.network;
  }

  handler = async (request: http.IncomingMessage, response: http.ServerResponse): Promise<void> => {
    let reply: Reply;
    try {
      reply = await this.route(request);
    } catch (error) {
      const status = error instanceof HttpError ? error.status : error instanceof SyntaxError ? 400 : 500;
      reply = { status, body: { error: (error as Error).message } };
    }
    response.writeHead(reply.status, { "content-type": "application/json" });
    response.end(JSON.stringify(reply.body));
  };

  private async route(request: http.IncomingMessage): Promise<Reply> {
    const url = new URL(request.url ?? "/", "http://localhost");
    const parts = url.pathname.split("/").filter(Boolean).map(decodeURIComponent);
    const method = request.method ?? "GET";
    const remote = request.headers[HOP_HEADER] !== undefined;
    const body = method === "POST" ? await readBody(request) : {};

    if (parts[0] === "machines" && parts.length === 1 && method === "GET") return { status: 200, body: await this.machines() };
    if (parts[0] !== "sessions") throw new HttpError(404, "Not found.");

    if (parts.length === 1 && method === "GET") return { status: 200, body: await this.listAll(remote) };
    if (parts.length === 1 && method === "POST") return this.spawn(body, remote);

    const id = parts[1];
    if (!(await this.sessions.has(id))) {
      if (remote) throw new HttpError(404, `No session ${id}.`);
      return this.forward(id, method, url.pathname, body);
    }
    if (parts.length === 2 && method === "GET") return { status: 200, body: { machine: this.machine, ...(await this.sessions.read(id)) } };
    if (parts.length === 2 && method === "DELETE") {
      await this.sessions.stop(id);
      return { status: 200, body: { stopped: id } };
    }
    if (parts.length === 3 && parts[2] === "messages" && method === "POST") {
      const result = await this.sessions.send(id, text(body.text) ?? "", text(body.from));
      return { status: 200, body: { id, result } };
    }
    throw new HttpError(404, "Not found.");
  }

  private async peers(): Promise<Peer[]> {
    return this.network ? this.network.peers() : [];
  }

  private async machines(): Promise<{ name: string; self: boolean }[]> {
    const peers = await this.peers();
    if (!peers.some(p => p.self)) peers.unshift({ name: this.machine, tunnelId: "", url: "", self: true });
    return peers.map(p => ({ name: p.name, self: p.self }));
  }

  private async listAll(remote: boolean): Promise<unknown[]> {
    const local = (await this.sessions.list()).map(s => ({ machine: this.machine, ...s }));
    if (remote) return local;
    const others = (await this.peers()).filter(p => !p.self);
    const lists = await Promise.all(others.map(async peer => {
      try {
        const response = await this.network!.fetch(peer, "/sessions", { headers: { [HOP_HEADER]: "1" } });
        const body: unknown = response.ok ? await response.json() : [];
        return Array.isArray(body) ? body : [];
      } catch {
        return [];
      }
    }));
    return [...local, ...lists.flat()];
  }

  private async spawn(body: Record<string, unknown>, remote: boolean): Promise<Reply> {
    const machine = text(body.machine);
    if (machine && !remote) {
      const peer = (await this.peers()).find(p => p.name.toLowerCase() === machine.toLowerCase() || p.tunnelId === machine);
      if (!peer && machine.toLowerCase() !== this.machine.toLowerCase()) throw new HttpError(404, `No machine ${machine}.`);
      if (peer && !peer.self) return this.request(peer, "POST", "/sessions", body);
    }
    const cwd = text(body.cwd);
    if (!cwd) throw new HttpError(400, "cwd is required.");
    const session = await this.sessions.spawn({ cwd, prompt: text(body.prompt) ?? "", name: text(body.name), from: text(body.from), remote });
    return { status: 201, body: { machine: this.machine, ...session } };
  }

  private async forward(id: string, method: string, path: string, body: Record<string, unknown>): Promise<Reply> {
    for (const peer of (await this.peers()).filter(p => !p.self)) {
      try {
        const reply = await this.request(peer, method, path, method === "POST" ? body : undefined);
        if (reply.status !== 404) return reply;
      } catch {
        // An unreachable machine cannot own the session; try the next one.
      }
    }
    throw new HttpError(404, `No session ${id}.`);
  }

  private async request(peer: Peer, method: string, path: string, body?: unknown): Promise<Reply> {
    const headers: Record<string, string> = { [HOP_HEADER]: "1" };
    if (body !== undefined) headers["content-type"] = "application/json";
    const response = await this.network!.fetch(peer, path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    return { status: response.status, body: await response.json() };
  }
}

export function daemonStatePath(dir: string): string {
  return join(dir, "daemon.json");
}

/**
 * Starts the daemon on this machine: tunnel, session manager and HTTP API. It records its
 * pid in daemon.json and shuts down cleanly on SIGINT or SIGTERM.
 */
export async function runDaemon(dir = dataDir()): Promise<{ close(): Promise<void> }> {
  const config = await loadConfig(dir);
  const machine = hostname();
  const sessions = new Sessions({ dir, config, copilot: findCopilot() });
  const tunnel = new MachineTunnel({ cli: new DevtunnelCli(), name: machine, port: config.port });
  await tunnel.start();
  const daemon = new Daemon({ sessions, machine, network: tunnel });
  const server = http.createServer(daemon.handler);
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(config.port, "127.0.0.1", resolve);
  });
  let ticking = false;
  const timer = setInterval(() => {
    if (ticking) return;
    ticking = true;
    sessions.tick().catch(() => {}).finally(() => { ticking = false; });
  }, 2_000);
  mkdirSync(dir, { recursive: true });
  writeFileSync(daemonStatePath(dir), JSON.stringify({ pid: process.pid, port: config.port }));
  const close = async () => {
    clearInterval(timer);
    try {
      if (JSON.parse(readFileSync(daemonStatePath(dir), "utf8")).pid === process.pid) rmSync(daemonStatePath(dir), { force: true });
    } catch {}
    await new Promise(resolve => server.close(resolve));
    await tunnel.stop();
  };
  for (const signal of ["SIGINT", "SIGTERM"] as const) process.once(signal, () => void close().finally(() => process.exit(0)));
  return { close };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  runDaemon().catch(error => {
    console.error(error);
    process.exit(1);
  });
}
