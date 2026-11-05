# Agent instructions

- Rust, one binary (`agent-net`) for the CLI, daemon, session host and updater. Keep dependencies minimal.
- The Copilot extension is plain JavaScript in `.github/extensions/agent-net/extension.mjs`. The binary embeds it with `include_str!` and installs it. It must never write to stdout; stdout carries the CLI's JSON-RPC.
- Code must work on Windows, macOS and Linux. Never assume a POSIX shell, `/` paths or Unix signals.
- Unit tests sit next to the code; integration tests are in `tests/`. Run `cargo test` before pushing.
- Push straight to `main`. Every push to `main` publishes a release `v0.2.<run>`, which running daemons install on their own.
