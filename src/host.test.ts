import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import type net from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { attach, isRunning, readState, send, startHost } from "./host-client.ts";

const dir = await mkdtemp(join(tmpdir(), "agent-net-"));
after(() => rm(dir, { recursive: true, force: true }));

// A tiny interactive program: prints a banner, echoes each line, exits on "bye".
const echo = `process.stdout.write("READY\\n");
process.stdin.setEncoding("utf8");
process.stdin.on("data", d => { if (d.includes("bye")) process.exit(0); process.stdout.write("echo:" + d.trim() + "\\n"); });`;

function waitFor(socket: net.Socket, text: string): Promise<string> {
  return new Promise((resolve, reject) => {
    let seen = "";
    const timer = setTimeout(() => reject(new Error(`timed out waiting for ${text}; saw ${JSON.stringify(seen)}`)), 10_000);
    socket.on("data", chunk => {
      seen += chunk.toString("utf8");
      if (seen.includes(text)) { clearTimeout(timer); resolve(seen); }
    });
  });
}

async function waitUntil(check: () => Promise<boolean>): Promise<void> {
  for (let i = 0; i < 100; i++) {
    if (await check()) return;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error("condition not met");
}

test("host runs a hidden terminal session that clients attach to, type into and stop", async () => {
  const id = randomUUID();
  await startHost({ id, command: process.execPath, args: ["-e", echo], cwd: dir, dir });
  assert.equal((await readState(dir, id))?.id, id);

  const first = await attach(dir, id);
  await waitFor(first, "READY");
  const echoed = waitFor(first, "echo:hello");
  first.write("hello\r");
  await echoed;
  first.destroy();

  // A later client gets the scrollback replayed.
  const second = await attach(dir, id);
  await waitFor(second, "echo:hello");
  second.destroy();

  await send(dir, id, { type: "write", data: "bye\r" });
  await waitUntil(async () => !(await isRunning(dir, id)));
  assert.equal(await readState(dir, id), undefined);
});

test("stop kills the session", async () => {
  const id = randomUUID();
  await startHost({ id, command: process.execPath, args: ["-e", "setInterval(() => {}, 1000)"], cwd: dir, dir });
  await send(dir, id, { type: "stop" });
  await waitUntil(async () => !(await isRunning(dir, id)));
});
