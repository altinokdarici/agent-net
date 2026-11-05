import { createHash } from "node:crypto";
import { join } from "node:path";

/** Folder holding each session host's socket and state file. */
export function hostsDir(dir: string): string {
  return join(dir, "hosts");
}

/**
 * Local socket of a session host: a named pipe on Windows, a Unix socket elsewhere.
 * Unix sockets live in /tmp because macOS caps socket paths at 104 bytes.
 */
export function socketPath(dir: string, id: string): string {
  if (process.platform === "win32") return `\\\\.\\pipe\\agent-net-${id}`;
  const tag = createHash("sha256").update(dir).digest("hex").slice(0, 8);
  return join("/tmp", `agent-net-${tag}-${id}.sock`);
}

export function statePath(dir: string, id: string): string {
  return join(hostsDir(dir), `${id}.json`);
}

/** First line a client sends to a host. Attach turns the connection into a raw terminal stream. */
export type HostRequest =
  | { type: "attach"; cols?: number; rows?: number }
  | { type: "resize"; cols: number; rows: number }
  | { type: "write"; data: string }
  | { type: "stop" };
