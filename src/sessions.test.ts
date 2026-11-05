import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { defaults } from "./config.ts";
import type { HostOptions } from "./host.ts";
import { isAllowed, Sessions } from "./sessions.ts";

function fake(start = 0) {
  const state = {
    now: start,
    running: new Set<string>(),
    started: [] as HostOptions[],
    delivered: [] as string[],
    extensionReady: false,
    trusted: [] as string[],
  };
  const deps = {
    startHost: async (options: HostOptions) => { state.started.push(options); state.running.add(options.id); },
    stopHost: async (_dir: string, id: string) => { state.running.delete(id); },
    isRunning: async (_dir: string, id: string) => state.running.has(id),
    trust: async (folder: string) => { state.trusted.push(folder); },
    deliver: async (_dir: string, _id: string, text: string) => {
      if (!state.extensionReady) return false;
      state.delivered.push(text);
      return true;
    },
    now: () => state.now,
  };
  return { state, deps };
}

async function setup(allowedPaths: string[] = []) {
  const dir = await mkdtemp(join(tmpdir(), "agent-net-sessions-"));
  const { state, deps } = fake();
  const sessions = new Sessions({ dir, config: { ...defaults, allowedPaths, idleMinutes: 1 }, copilot: { command: "copilot", args: [] }, deps });
  return { dir, state, sessions, [Symbol.asyncDispose]: () => rm(dir, { recursive: true, force: true }) };
}

test("isAllowed accepts allowed folders and their children only", () => {
  const root = join(tmpdir(), "work");
  assert.equal(isAllowed(root, [root]), true);
  assert.equal(isAllowed(join(root, "app"), [root]), true);
  assert.equal(isAllowed(join(root, "..", "other"), [root]), false);
  assert.equal(isAllowed(`${root}-evil`, [root]), false);
  assert.equal(isAllowed(root, []), false);
});

test("spawn trusts the folder and starts copilot with the session id and prompt", async () => {
  await using env = await setup();
  const session = await env.sessions.spawn({ cwd: env.dir, prompt: "hello", name: "w" });
  assert.equal(session.state, "running");
  assert.deepEqual(env.state.trusted, [env.dir]);
  const options = env.state.started[0];
  assert.deepEqual(options.args, ["--session-id", session.id, "--allow-all", "-i", "hello"]);
  assert.equal(options.env?.AGENT_NET_SESSION, session.id);
  assert.deepEqual((await env.sessions.list()).map(s => s.name), ["w"]);
});

test("remote spawn is limited to allowedPaths", async () => {
  await using env = await setup();
  await assert.rejects(env.sessions.spawn({ cwd: env.dir, prompt: "x", remote: true }), { status: 403 });
});

test("send queues until the extension is ready, wakes a sleeping session", async () => {
  await using env = await setup();
  const { id } = await env.sessions.spawn({ cwd: env.dir, prompt: "start" });
  assert.equal(await env.sessions.send(id, "one"), "queued");
  assert.equal(await env.sessions.send(id, "two"), "queued");
  env.state.extensionReady = true;
  await env.sessions.tick();
  assert.deepEqual(env.state.delivered, ["one", "two"]);
  assert.equal(await env.sessions.send(id, "three"), "delivered");

  env.state.running.clear();
  assert.equal(await env.sessions.send(id, "wake up"), "woke");
  assert.deepEqual(env.state.started.at(-1)?.args.slice(-2), ["-i", "wake up"]);
});

test("tick puts sessions to sleep after idleMinutes without a message", async () => {
  await using env = await setup();
  const { id } = await env.sessions.spawn({ cwd: env.dir, prompt: "start" });
  env.state.now = 30_000;
  await env.sessions.tick();
  assert.equal(env.state.running.has(id), true);
  env.state.now = 61_000;
  await env.sessions.tick();
  assert.equal(env.state.running.has(id), false);
  assert.equal((await env.sessions.read(id)).state, "sleeping");
});

test("stop ends and forgets a session", async () => {
  await using env = await setup();
  const { id } = await env.sessions.spawn({ cwd: env.dir, prompt: "start" });
  await env.sessions.stop(id);
  assert.equal(env.state.running.size, 0);
  assert.deepEqual(await env.sessions.list(), []);
  await assert.rejects(env.sessions.send(id, "x"), { status: 404 });
});
