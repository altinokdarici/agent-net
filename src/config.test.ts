import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";
import { dataDir, defaults, loadConfig, parseConfig } from "./config.ts";

test("parseConfig fills defaults", () => {
  assert.deepEqual(parseConfig({}), defaults);
});

test("parseConfig resolves allowed paths", () => {
  assert.deepEqual(parseConfig({ allowedPaths: ["a/b"] }).allowedPaths, [resolve("a/b")]);
});

test("parseConfig rejects bad values", () => {
  assert.throws(() => parseConfig([]), /JSON object/);
  assert.throws(() => parseConfig({ allowedPaths: "x" }), /allowedPaths/);
  assert.throws(() => parseConfig({ allowedPaths: [""] }), /allowedPaths/);
  assert.throws(() => parseConfig({ idleMinutes: 0 }), /idleMinutes/);
  assert.throws(() => parseConfig({ port: 70000 }), /port/);
});

test("dataDir honours AGENT_NET_HOME", () => {
  assert.equal(dataDir({ AGENT_NET_HOME: "/x" }), "/x");
});

test("loadConfig returns defaults when the file is missing and parses it when present", async () => {
  const dir = await mkdtemp(join(tmpdir(), "agent-net-"));
  try {
    assert.deepEqual(await loadConfig(dir), defaults);
    await writeFile(join(dir, "config.json"), JSON.stringify({ idleMinutes: 5 }));
    assert.equal((await loadConfig(dir)).idleMinutes, 5);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
