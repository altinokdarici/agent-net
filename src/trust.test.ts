import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";
import { addTrustedFolder, ensureTrusted } from "./trust.ts";

const parse = (text: string) => JSON.parse(text.replace(/^\s*\/\/.*$/gm, ""));
const header = "// managed automatically\n";
const folder = resolve("/work/repo");

test("adds to an existing list and keeps comments", () => {
  const { text, changed } = addTrustedFolder(`${header}{\n  "a": 1,\n  "trustedFolders": [\n    "/other"\n  ]\n}\n`, folder);
  assert.equal(changed, true);
  assert.ok(text.startsWith(header));
  assert.deepEqual(parse(text).trustedFolders, [folder, "/other"]);
});

test("adds to an empty list", () => {
  assert.deepEqual(parse(addTrustedFolder(`{"trustedFolders": []}`, folder).text).trustedFolders, [folder]);
});

test("adds the key when missing", () => {
  const { text } = addTrustedFolder(`${header}{\n  "a": 1\n}\n`, folder);
  assert.deepEqual(parse(text), { trustedFolders: [folder], a: 1 });
});

test("creates content for an empty file", () => {
  assert.deepEqual(parse(addTrustedFolder("", folder).text), { trustedFolders: [folder] });
});

test("leaves an already trusted folder alone", () => {
  const original = `{"trustedFolders": [${JSON.stringify(folder)}]}`;
  assert.deepEqual(addTrustedFolder(original, folder), { text: original, changed: false });
});

test("ensureTrusted writes once", async () => {
  const dir = await mkdtemp(join(tmpdir(), "agent-net-"));
  const path = join(dir, "config.json");
  try {
    await writeFile(path, `${header}{\n  "trustedFolders": []\n}\n`);
    assert.equal(await ensureTrusted(folder, path), true);
    assert.equal(await ensureTrusted(folder, path), false);
    assert.deepEqual(parse(await readFile(path, "utf8")).trustedFolders, [folder]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
