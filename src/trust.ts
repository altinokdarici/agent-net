import { readFile, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

export function copilotConfigPath(env: NodeJS.ProcessEnv = process.env): string {
  return join(env.COPILOT_HOME ?? join(homedir(), ".copilot"), "config.json");
}

function stripComments(text: string): string {
  return text.replace(/^\s*\/\/.*$/gm, "");
}

function same(a: string, b: string): boolean {
  return process.platform === "win32" || process.platform === "darwin" ? a.toLowerCase() === b.toLowerCase() : a === b;
}

/**
 * Adds a folder to Copilot's trustedFolders so a hidden session never stops at the
 * folder trust prompt. Copilot's config has leading // comments, so this edits the
 * text in place instead of rewriting the file. Returns false if it was already trusted.
 */
export function addTrustedFolder(text: string, folder: string): { text: string; changed: boolean } {
  const path = resolve(folder);
  const config = text.trim() ? JSON.parse(stripComments(text)) : {};
  const trusted: unknown = config.trustedFolders;
  if (Array.isArray(trusted) && trusted.some(entry => typeof entry === "string" && same(resolve(entry), path))) return { text, changed: false };
  const entry = JSON.stringify(path);
  let next: string;
  if (Array.isArray(trusted)) {
    next = text.replace(/("trustedFolders"\s*:\s*\[)(\s*)(\]?)/, (_m, open: string, space: string, close: string) =>
      close ? `${open}${entry}]` : `${open}\n    ${entry},${space}`);
  } else if (text.trim()) {
    next = text.replace(/\{/, `{\n  "trustedFolders": [${entry}],`);
  } else {
    next = `${JSON.stringify({ trustedFolders: [path] }, null, 2)}\n`;
  }
  JSON.parse(stripComments(next));
  return { text: next, changed: true };
}

export async function ensureTrusted(folder: string, configPath = copilotConfigPath()): Promise<boolean> {
  let text = "";
  try {
    text = await readFile(configPath, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const result = addTrustedFolder(text, folder);
  if (result.changed) await writeFile(configPath, result.text);
  return result.changed;
}
