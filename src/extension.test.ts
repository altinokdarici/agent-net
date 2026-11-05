import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { start } from "./extension.ts";
import { portFile } from "./sessions.ts";

test("registers the agent tool and passes daemon messages to the session", async () => {
  const dir = await mkdtemp(join(tmpdir(), "agent-net-ext-"));
  const prompts: string[] = [];
  let tools: { name: string }[] = [];
  const extension = await start(async config => {
    tools = config.tools as { name: string }[];
    return { sessionId: "s1", send: async ({ prompt }) => { prompts.push(prompt); } };
  }, dir);
  assert.deepEqual(tools.map(t => t.name), ["agent"]);
  const { port } = JSON.parse(await readFile(portFile(dir, "s1"), "utf8"));
  const ok = await fetch(`http://127.0.0.1:${port}/message`, { method: "POST", body: JSON.stringify({ prompt: "hello" }) });
  assert.equal(ok.status, 200);
  const bad = await fetch(`http://127.0.0.1:${port}/message`, { method: "POST", body: "{}" });
  assert.equal(bad.status, 400);
  assert.deepEqual(prompts, ["hello"]);
  extension.close();
  await rm(dir, { recursive: true, force: true });
});
