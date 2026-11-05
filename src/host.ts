import { execFile } from "node:child_process";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import net from "node:net";
import { fileURLToPath } from "node:url";
import pty from "node-pty";
import { hostsDir, socketPath, statePath } from "./ipc.ts";
import type { HostRequest } from "./ipc.ts";

export type HostOptions = {
  id: string;
  command: string;
  args: string[];
  cwd: string;
  dir: string;
  env?: Record<string, string>;
  cols?: number;
  rows?: number;
  /** Bytes of terminal output kept for replay when a client attaches. */
  scrollback?: number;
};

export type HostState = { id: string; pid: number; childPid: number; cwd: string; startedAt: string };

/** Kills a process and all its descendants. */
export function killTree(pid: number): void {
  if (process.platform === "win32") {
    execFile("taskkill", ["/pid", String(pid), "/t", "/f"], { windowsHide: true }, () => {});
    return;
  }
  // node-pty makes the child a session leader, so its process group holds the whole tree.
  try { process.kill(-pid, "SIGKILL"); } catch {}
  try { process.kill(pid, "SIGKILL"); } catch {}
}

/**
 * Runs one command in a hidden pseudo-terminal and serves it on a local socket.
 * Resolves with the command's exit code once it exits.
 */
export function runHost(options: HostOptions): Promise<number> {
  const limit = options.scrollback ?? 1024 * 1024;
  const child = pty.spawn(options.command, options.args, {
    name: "xterm-256color",
    cols: options.cols ?? 120,
    rows: options.rows ?? 40,
    cwd: options.cwd,
    env: { ...process.env, ...options.env } as Record<string, string>,
  });
  let buffer = "";
  const clients = new Set<net.Socket>();
  child.onData(data => {
    buffer = (buffer + data).slice(-limit);
    for (const client of clients) client.write(data);
  });

  const socket = socketPath(options.dir, options.id);
  mkdirSync(hostsDir(options.dir), { recursive: true });
  if (process.platform !== "win32") rmSync(socket, { force: true });
  const server = net.createServer(connection => {
    let pending = "";
    const onData = (chunk: Buffer) => {
      pending += chunk.toString("utf8");
      const end = pending.indexOf("\n");
      if (end < 0) return;
      connection.off("data", onData);
      let request: HostRequest;
      try { request = JSON.parse(pending.slice(0, end)); } catch { connection.destroy(); return; }
      const rest = pending.slice(end + 1);
      if (request.type === "attach") {
        if (request.cols && request.rows) child.resize(request.cols, request.rows);
        connection.write(buffer);
        clients.add(connection);
        if (rest) child.write(rest);
        connection.on("data", data => child.write(data.toString("utf8")));
        connection.on("close", () => clients.delete(connection));
        connection.on("error", () => clients.delete(connection));
      } else if (request.type === "resize") {
        child.resize(request.cols, request.rows);
        connection.end();
      } else if (request.type === "write") {
        child.write(request.data);
        connection.end();
      } else if (request.type === "stop") {
        connection.end();
        killTree(child.pid);
      }
    };
    connection.on("data", onData);
    connection.on("error", () => {});
  });
  server.listen(socket);

  const state: HostState = { id: options.id, pid: process.pid, childPid: child.pid, cwd: options.cwd, startedAt: new Date().toISOString() };
  writeFileSync(statePath(options.dir, options.id), JSON.stringify(state));

  return new Promise(resolve => {
    child.onExit(({ exitCode }) => {
      for (const client of clients) client.end();
      server.close();
      rmSync(statePath(options.dir, options.id), { force: true });
      if (process.platform !== "win32") rmSync(socket, { force: true });
      resolve(exitCode);
    });
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const options: HostOptions = JSON.parse(Buffer.from(process.argv[2], "base64url").toString("utf8"));
  runHost(options).then(code => process.exit(code));
}
