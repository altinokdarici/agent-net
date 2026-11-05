#!/usr/bin/env node
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, openSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { hostname } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { autostart } from "./autostart.ts";
import type { Step } from "./autostart.ts";
import { dataDir, loadConfig } from "./config.ts";
import { daemonStatePath, runDaemon } from "./daemon.ts";
import { attach, send } from "./host-client.ts";
import { copilotConfigPath } from "./trust.ts";

const daemonFile = fileURLToPath(new URL("./daemon.ts", import.meta.url));
const extensionFile = fileURLToPath(new URL("./extension.ts", import.meta.url));

const usage = `Usage: agent-net <command>

  start            Start the daemon in the background.
  stop             Stop the background daemon.
  run              Run the daemon in this terminal.
  ls               List agents on every machine.
  attach <id>      Watch and type into a local agent. Ctrl+] detaches.
  install          Add the extension to Copilot and start the daemon at login.
  uninstall        Undo install.`;

/** The user-level extension: a loader that imports this checkout's extension. */
export function extensionLoader(file: string): string {
  return `import { joinSession } from "@github/copilot-sdk/extension";

const { start } = await import(${JSON.stringify(pathToFileURL(file).href)});
await start(joinSession);
`;
}

export function extensionLoaderPath(): string {
  return join(dirname(copilotConfigPath()), "extensions", "agent-net", "extension.mjs");
}

async function api(path: string): Promise<unknown> {
  const { port } = await loadConfig();
  const response = await fetch(`http://127.0.0.1:${port}${path}`);
  const body = await response.json();
  if (!response.ok) throw new Error((body as { error?: string }).error ?? `Daemon returned ${response.status}.`);
  return body;
}

async function daemonUp(): Promise<boolean> {
  try {
    await api("/machines");
    return true;
  } catch {
    return false;
  }
}

async function start(): Promise<void> {
  if (await daemonUp()) return console.log("The daemon is already running.");
  const dir = dataDir();
  mkdirSync(dir, { recursive: true });
  const log = openSync(join(dir, "daemon.log"), "a");
  spawn(process.execPath, [daemonFile], { detached: true, stdio: ["ignore", log, log], windowsHide: true }).unref();
  for (let i = 0; i < 300; i++) {
    await new Promise(resolve => setTimeout(resolve, 100));
    if (await daemonUp()) return console.log("The daemon is running.");
  }
  throw new Error(`The daemon did not start. See ${join(dir, "daemon.log")}.`);
}

function stop(): void {
  let pid: number;
  try {
    pid = JSON.parse(readFileSync(daemonStatePath(dataDir()), "utf8")).pid;
  } catch {
    return console.log("The daemon is not running.");
  }
  try {
    process.kill(pid);
  } catch {}
  rmSync(daemonStatePath(dataDir()), { force: true });
  console.log("The daemon is stopped.");
}

type Listed = { machine: string; id: string; name?: string; state: string; cwd: string };

async function ls(): Promise<void> {
  const sessions = await api("/sessions") as Listed[];
  if (!sessions.length) return console.log("No agents.");
  for (const s of sessions) console.log([s.machine, s.id, s.state, s.name ?? "-", s.cwd].join("\t"));
}

/** Leaves the alternate screen and turns off the mouse, paste and keyboard modes Copilot turns on. */
const resetTerminal = "\x1b[?1049l\x1b[?2004l\x1b[?1004l\x1b[?1003l\x1b[?1006l\x1b[?1000l\x1b[>4;0m\x1b[<u\x1b[?25h\x1b[0m";

async function attachTo(prefix: string | undefined): Promise<void> {
  if (!prefix) throw new Error("Usage: agent-net attach <id>");
  const dir = dataDir();
  const matches = (await api("/sessions") as Listed[]).filter(s => s.id.startsWith(prefix) && s.state === "running" && s.machine === hostname());
  if (matches.length !== 1) throw new Error(matches.length ? `${prefix} matches more than one agent.` : `No running agent ${prefix} on this machine.`);
  const id = matches[0].id;
  const size = () => ({ cols: process.stdout.columns || 120, rows: process.stdout.rows || 30 });
  const socket = await attach(dir, id, size());
  const onResize = () => send(dir, id, { type: "resize", ...size() }).catch(() => {});
  process.stdout.on("resize", onResize);
  if (process.stdin.isTTY) process.stdin.setRawMode(true);
  process.stdin.resume();
  let detached = false;
  const done = () => {
    if (detached) return;
    detached = true;
    process.stdout.off("resize", onResize);
    if (process.stdin.isTTY) process.stdin.setRawMode(false);
    process.stdin.pause();
    socket.destroy();
    process.stdout.write(`${resetTerminal}\r\nDetached.\r\n`);
  };
  process.stdin.on("data", (data: Buffer) => {
    if (data.includes(0x1d)) return done();
    socket.write(data);
  });
  socket.pipe(process.stdout, { end: false });
  await new Promise<void>(resolve => socket.on("close", () => resolve()));
  done();
}

function runSteps(steps: Step[]): void {
  for (const step of steps) {
    try {
      execFileSync(step.command[0], step.command.slice(1), { stdio: "ignore", windowsHide: true });
    } catch (error) {
      if (!step.mayFail) throw new Error(`${step.command.join(" ")} failed: ${(error as Error).message}`);
    }
  }
}

function plan() {
  return autostart({ node: process.execPath, daemon: daemonFile, path: process.env.PATH ?? process.env.Path ?? "" });
}

function install(): void {
  const loader = extensionLoaderPath();
  mkdirSync(dirname(loader), { recursive: true });
  writeFileSync(loader, extensionLoader(extensionFile));
  const login = plan();
  for (const file of login.files) {
    mkdirSync(dirname(file.path), { recursive: true });
    writeFileSync(file.path, file.content);
  }
  runSteps(login.install);
  console.log(`Installed the extension at ${loader}. The daemon starts at login and is starting now.`);
}

function uninstall(): void {
  const login = plan();
  runSteps(login.uninstall);
  for (const file of login.files) rmSync(file.path, { force: true });
  rmSync(dirname(extensionLoaderPath()), { recursive: true, force: true });
  console.log("Removed the extension and the login item.");
}

export async function main(args: string[]): Promise<void> {
  const [command, ...rest] = args;
  switch (command) {
    case "start": return start();
    case "stop": return stop();
    case "run": return void await runDaemon();
    case "ls": return ls();
    case "attach": return attachTo(rest[0]);
    case "install": return install();
    case "uninstall": return uninstall();
    default: console.log(usage);
  }
}

// npm links the bin, so compare real paths.
if (process.argv[1] && fileURLToPath(import.meta.url) === realpathSync(process.argv[1])) {
  main(process.argv.slice(2)).catch(error => {
    console.error((error as Error).message);
    process.exit(1);
  });
}
