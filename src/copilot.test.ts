import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";
import { findCopilot } from "./copilot.ts";

test("AGENT_NET_COPILOT wins", () => {
  assert.deepEqual(findCopilot({ AGENT_NET_COPILOT: "/x/copilot" }, () => false), { command: "/x/copilot", args: [] });
});

test("finds copilot on PATH", () => {
  const path = join("/b", "copilot");
  assert.deepEqual(findCopilot({ PATH: ["/a", "/b"].join(process.platform === "win32" ? ";" : ":") }, p => p === path, "linux"), { command: path, args: [] });
});

test("prefers copilot.exe on Windows and runs a .cmd shim through cmd.exe", () => {
  const exe = join("B", "copilot.exe");
  const cmd = join("A", "copilot.cmd");
  assert.deepEqual(findCopilot({ PATH: "A;B" }, p => p === exe || p === cmd, "win32"), { command: exe, args: [] });
  assert.deepEqual(findCopilot({ PATH: "A;B" }, p => p === cmd, "win32"), { command: "cmd.exe", args: ["/d", "/s", "/c", cmd] });
});

test("fails clearly when copilot is missing", () => {
  assert.throws(() => findCopilot({ PATH: "/a" }, () => false, "linux"), /AGENT_NET_COPILOT/);
});
