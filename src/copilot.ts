import { existsSync } from "node:fs";
import { delimiter, join } from "node:path";

export type Command = { command: string; args: string[] };

/**
 * Finds the copilot executable without going through a shell. On Windows a real
 * copilot.exe wins over npm's .cmd shim, which ConPTY can only run through cmd.exe.
 * AGENT_NET_COPILOT overrides the search.
 */
export function findCopilot(env: NodeJS.ProcessEnv = process.env, exists: (path: string) => boolean = existsSync, platform = process.platform): Command {
  if (env.AGENT_NET_COPILOT) return { command: env.AGENT_NET_COPILOT, args: [] };
  const dirs = (env.PATH ?? env.Path ?? "").split(platform === "win32" ? ";" : delimiter).filter(Boolean);
  const names = platform === "win32" ? ["copilot.exe", "copilot.cmd"] : ["copilot"];
  for (const name of names) {
    for (const dir of dirs) {
      const path = join(dir, name);
      if (!exists(path)) continue;
      if (name.endsWith(".cmd")) return { command: env.ComSpec ?? "cmd.exe", args: ["/d", "/s", "/c", path] };
      return { command: path, args: [] };
    }
  }
  throw new Error("Could not find copilot on PATH. Set AGENT_NET_COPILOT to its full path.");
}
