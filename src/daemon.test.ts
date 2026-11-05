import assert from "node:assert/strict";
import http from "node:http";
import type { AddressInfo } from "node:net";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { defaults } from "./config.ts";
import { Daemon, HOP_HEADER } from "./daemon.ts";
import type { Network } from "./daemon.ts";
import { Sessions } from "./sessions.ts";
import type { Peer } from "./tunnel.ts";

type Machine = { name: string; dir: string; url: string; server: http.Server; daemon: Daemon };

/** Two daemons on localhost, wired together the way their tunnels would connect them. */
async function network() {
  const machines: Machine[] = [];
  const running = new Set<string>();
  const peersOf = (self: string): Peer[] => machines.map(m => ({ name: m.name, tunnelId: m.name, url: m.url, self: m.name === self }));
  for (const name of ["home", "mac"]) {
    const dir = await mkdtemp(join(tmpdir(), `agent-net-daemon-${name}-`));
    const sessions = new Sessions({
      dir,
      config: { ...defaults, allowedPaths: [dir] },
      copilot: { command: "copilot", args: [] },
      deps: {
        startHost: async options => { running.add(options.id); },
        stopHost: async (_d, id) => { running.delete(id); },
        isRunning: async (_d, id) => running.has(id),
        trust: async () => {},
        deliver: async () => true,
      },
    });
    const net: Network = { peers: async () => peersOf(name), fetch: (peer, path, init) => fetch(new URL(path, peer.url), init) };
    const daemon = new Daemon({ sessions, machine: name, network: net });
    const server = http.createServer(daemon.handler);
    await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
    machines.push({ name, dir, server, daemon, url: `http://127.0.0.1:${(server.address() as AddressInfo).port}` });
  }
  return {
    machines,
    async [Symbol.asyncDispose]() {
      for (const m of machines) {
        m.server.close();
        await rm(m.dir, { recursive: true, force: true });
      }
    },
  };
}

async function call(base: string, method: string, path: string, body?: unknown, headers: Record<string, string> = {}) {
  const response = await fetch(base + path, { method, headers: { "content-type": "application/json", ...headers }, body: body === undefined ? undefined : JSON.stringify(body) });
  return { status: response.status, body: await response.json() as any };
}

test("spawns on another machine and reaches the session by id from anywhere", async () => {
  await using net = await network();
  const [home, mac] = net.machines;
  const spawned = await call(home.url, "POST", "/sessions", { machine: "mac", cwd: mac.dir, prompt: "hi", from: "home/test" });
  assert.equal(spawned.status, 201);
  assert.equal(spawned.body.machine, "mac");
  const id = spawned.body.id;

  const all = await call(home.url, "GET", "/sessions");
  assert.deepEqual(all.body.map((s: any) => [s.machine, s.id]), [["mac", id]]);

  const read = await call(home.url, "GET", `/sessions/${id}`);
  assert.deepEqual([read.body.machine, read.body.state], ["mac", "running"]);
  assert.equal((await call(home.url, "POST", `/sessions/${id}/messages`, { text: "more" })).body.result, "delivered");
  assert.equal((await call(home.url, "DELETE", `/sessions/${id}`)).status, 200);
  assert.equal((await call(home.url, "GET", `/sessions/${id}`)).status, 404);
});

test("lists machines and rejects bad requests", async () => {
  await using net = await network();
  const [home, mac] = net.machines;
  assert.deepEqual((await call(home.url, "GET", "/machines")).body, [{ name: "home", self: true }, { name: "mac", self: false }]);
  assert.equal((await call(home.url, "POST", "/sessions", { machine: "nope", cwd: home.dir, prompt: "x" })).status, 404);
  assert.equal((await call(home.url, "POST", "/sessions", { prompt: "x" })).status, 400);
  const outside = await call(home.url, "POST", "/sessions", { machine: "mac", cwd: home.dir, prompt: "x" });
  assert.equal(outside.status, 403);
  assert.equal((await call(mac.url, "GET", "/sessions/unknown", undefined, { [HOP_HEADER]: "1" })).status, 404);
});
