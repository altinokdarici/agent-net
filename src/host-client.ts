import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import net from "node:net";
import { fileURLToPath } from "node:url";
import type { HostOptions, HostState } from "./host.ts";
import { socketPath, statePath } from "./ipc.ts";
import type { HostRequest } from "./ipc.ts";

const hostFile = fileURLToPath(new URL("./host.ts", import.meta.url));

export function connect(dir: string, id: string): Promise<net.Socket> {
  return new Promise((resolve, reject) => {
    const socket = net.connect(socketPath(dir, id));
    socket.once("connect", () => resolve(socket));
    socket.once("error", reject);
  });
}

/** Starts a session host as a detached, hidden process that outlives its parent. */
export async function startHost(options: HostOptions, timeoutMs = 10_000): Promise<void> {
  const arg = Buffer.from(JSON.stringify(options)).toString("base64url");
  spawn(process.execPath, [hostFile, arg], { detached: true, stdio: "ignore", windowsHide: true, cwd: options.cwd }).unref();
  const end = Date.now() + timeoutMs;
  for (;;) {
    try {
      (await connect(options.dir, options.id)).destroy();
      return;
    } catch {
      if (Date.now() > end) throw new Error(`Session host ${options.id} did not start.`);
      await new Promise(resolve => setTimeout(resolve, 100));
    }
  }
}

/** Sends a one-shot request to a running host. */
export async function send(dir: string, id: string, request: Exclude<HostRequest, { type: "attach" }>): Promise<void> {
  const socket = await connect(dir, id);
  await new Promise<void>(resolve => {
    socket.on("close", () => resolve());
    socket.on("error", () => resolve());
    socket.end(`${JSON.stringify(request)}\n`);
  });
}

/** Opens a raw terminal stream: replays the scrollback, then streams live output and accepts input. */
export async function attach(dir: string, id: string, size?: { cols: number; rows: number }): Promise<net.Socket> {
  const socket = await connect(dir, id);
  socket.write(`${JSON.stringify({ type: "attach", ...size })}\n`);
  return socket;
}

export async function readState(dir: string, id: string): Promise<HostState | undefined> {
  try {
    return JSON.parse(await readFile(statePath(dir, id), "utf8"));
  } catch {
    return undefined;
  }
}

export async function isRunning(dir: string, id: string): Promise<boolean> {
  try {
    (await connect(dir, id)).destroy();
    return true;
  } catch {
    return false;
  }
}
