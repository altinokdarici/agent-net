# Agent instructions

- TypeScript only, run directly by Node 24 (type stripping). No build step, no shell scripts; write tooling as `.ts` files.
- Only erasable TypeScript syntax: no enums, namespaces or parameter properties. Import local files with the `.ts` extension.
- Tests use `node:test` and sit next to the code as `*.test.ts`. Run `npm run check` before committing.
- Code must work on Windows, macOS and Linux. Never assume a POSIX shell, `/` paths or Unix signals.
- Keep dependencies minimal. Each pull request stays under 15 files.
- Copilot extensions must never write to stdout; it carries JSON-RPC.
