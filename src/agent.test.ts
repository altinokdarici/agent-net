import assert from "node:assert/strict";
import { test } from "node:test";
import { runAgent, spawnPrompt } from "./agent.ts";
import type { Call } from "./agent.ts";

function recorder(status = 200, body: unknown = { ok: true }) {
  const calls: unknown[][] = [];
  const call: Call = async (...args) => { calls.push(args); return { status, body }; };
  return { calls, call };
}

test("spawn tells the new agent who to report back to", async () => {
  const { calls, call } = recorder(201);
  await runAgent({ action: "spawn", machine: "mac", cwd: "/w", prompt: "build it" }, "me", call);
  assert.deepEqual(calls[0], ["POST", "/sessions", { machine: "mac", cwd: "/w", prompt: spawnPrompt("build it", "me"), name: undefined, from: "me" }]);
  assert.match(spawnPrompt("x", "me"), /id "me"/);
});

test("maps each action to the daemon API", async () => {
  const { calls, call } = recorder();
  await runAgent({ action: "machines" }, "me", call);
  await runAgent({ action: "list" }, "me", call);
  await runAgent({ action: "send", id: "a/b", text: "hi" }, "me", call);
  await runAgent({ action: "status", id: "x" }, "me", call);
  await runAgent({ action: "stop", id: "x" }, "me", call);
  assert.deepEqual(calls, [
    ["GET", "/machines"],
    ["GET", "/sessions"],
    ["POST", "/sessions/a%2Fb/messages", { text: "hi", from: "me" }],
    ["GET", "/sessions/x"],
    ["DELETE", "/sessions/x"],
  ]);
});

test("reports missing arguments and daemon errors", async () => {
  await assert.rejects(runAgent({ action: "send", id: "x" }, "me", recorder().call), /text is required/);
  await assert.rejects(runAgent({ action: "status", id: "x" }, "me", recorder(404, { error: "No session x." }).call), /No session x/);
});
