import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";
import { autostart } from "./autostart.ts";

const base = { node: "/n/node", daemon: "/r/src/daemon.ts", path: "/bin:/usr/bin", home: "/h" };

test("linux uses a systemd user service that restarts", () => {
  const plan = autostart({ ...base, platform: "linux" });
  assert.equal(plan.files[0].path, join("/h", ".config", "systemd", "user", "agent-net.service"));
  assert.match(plan.files[0].content, /ExecStart="\/n\/node" "\/r\/src\/daemon.ts"/);
  assert.match(plan.files[0].content, /Environment="PATH=\/bin:\/usr\/bin"/);
  assert.deepEqual(plan.install.at(-1)?.command, ["systemctl", "--user", "enable", "--now", "agent-net.service"]);
});

test("macOS uses a LaunchAgent and reloads it", () => {
  const plan = autostart({ ...base, path: "/a&b", platform: "darwin", uid: 501 });
  assert.match(plan.files[0].content, /<string>\/a&amp;b<\/string>/);
  assert.deepEqual(plan.install.map(s => [s.command.slice(0, 2), s.mayFail ?? false]), [[["launchctl", "bootout"], true], [["launchctl", "bootstrap"], false]]);
  assert.equal(plan.install[1].command[2], "gui/501");
});

test("Windows uses a hidden logon task", () => {
  const plan = autostart({ ...base, platform: "win32" });
  assert.deepEqual(plan.files, []);
  const create = plan.install[0].command;
  assert.equal(create.at(-1), 'conhost.exe --headless "/n/node" "/r/src/daemon.ts"');
  assert.ok(create.includes("onlogon"));
});

test("other platforms are refused", () => {
  assert.throws(() => autostart({ ...base, platform: "aix" }), /not supported/);
});
