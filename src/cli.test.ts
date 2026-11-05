import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
import { extensionLoader, extensionLoaderPath } from "./cli.ts";

test("the extension loader imports this checkout's extension by file URL", () => {
  const file = join(process.cwd(), "src", "extension.ts");
  const loader = extensionLoader(file);
  assert.match(loader, /from "@github\/copilot-sdk\/extension"/);
  assert.ok(loader.includes(JSON.stringify(pathToFileURL(file).href)));
});

test("the loader goes in Copilot's user extensions folder", () => {
  assert.ok(extensionLoaderPath().endsWith(join("extensions", "agent-net", "extension.mjs")));
});
