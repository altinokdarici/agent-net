import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

export type Config = {
  /** Folders that remote agents may start sessions in and transfer files to or from. */
  allowedPaths: string[];
  /** Minutes a session may sit idle before it is put to sleep. */
  idleMinutes: number;
  /** Local port of the daemon's HTTP API. */
  port: number;
};

export const defaults: Config = { allowedPaths: [], idleMinutes: 30, port: 4590 };

export function dataDir(env: NodeJS.ProcessEnv = process.env): string {
  return env.AGENT_NET_HOME ?? join(homedir(), ".agent-net");
}

export function parseConfig(value: unknown): Config {
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new Error("config.json must be a JSON object.");
  const input = value as Record<string, unknown>;
  const config = { ...defaults };
  if (input.allowedPaths !== undefined) {
    if (!Array.isArray(input.allowedPaths) || input.allowedPaths.some(path => typeof path !== "string" || path === "")) {
      throw new Error("allowedPaths must be an array of folder paths.");
    }
    config.allowedPaths = input.allowedPaths.map(path => resolve(path as string));
  }
  if (input.idleMinutes !== undefined) {
    if (typeof input.idleMinutes !== "number" || !(input.idleMinutes > 0)) throw new Error("idleMinutes must be a positive number.");
    config.idleMinutes = input.idleMinutes;
  }
  if (input.port !== undefined) {
    if (!Number.isInteger(input.port) || (input.port as number) < 1 || (input.port as number) > 65535) throw new Error("port must be an integer from 1 to 65535.");
    config.port = input.port as number;
  }
  return config;
}

export async function loadConfig(dir = dataDir()): Promise<Config> {
  let text: string;
  try {
    text = await readFile(join(dir, "config.json"), "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return { ...defaults };
    throw error;
  }
  return parseConfig(JSON.parse(text));
}
