import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import type { AddressInfo } from "node:net";
import { dirname } from "node:path";
import { dataDir, loadConfig } from "./config.ts";
import { daemonCall, description, parameters, runAgent } from "./agent.ts";
import type { AgentArgs } from "./agent.ts";
import { portFile } from "./sessions.ts";

type Session = { sessionId: string; send(options: { prompt: string }): Promise<unknown> };
type JoinSession = (config: { tools: unknown[] }) => Promise<Session>;

/**
 * Runs inside every Copilot session. It gives the session the agent tool and listens on a
 * local port, published in a port file, so the daemon can hand it incoming messages.
 * It must never write to stdout, which carries the CLI's JSON-RPC.
 */
export async function start(joinSession: JoinSession, dir = dataDir()): Promise<{ session: Session; close(): void }> {
  const config = await loadConfig(dir);
  const call = daemonCall(config.port);
  let self = process.env.AGENT_NET_SESSION ?? "";
  const session = await joinSession({
    tools: [{
      name: "agent",
      description,
      parameters,
      skipPermission: true,
      handler: (args: AgentArgs) => runAgent(args, self, call),
    }],
  });
  self = session.sessionId;

  const server = http.createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on("data", chunk => chunks.push(chunk));
    request.on("end", async () => {
      try {
        const { prompt } = JSON.parse(Buffer.concat(chunks).toString("utf8"));
        if (request.method !== "POST" || request.url !== "/message" || typeof prompt !== "string") throw new Error("Bad request.");
        await session.send({ prompt });
        response.writeHead(200).end("{}");
      } catch {
        response.writeHead(400).end("{}");
      }
    });
  });
  await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
  const file = portFile(dir, self);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, JSON.stringify({ port: (server.address() as AddressInfo).port, pid: process.pid }));
  process.on("exit", () => {
    // A reloaded extension for the same session may already own the file.
    try {
      if (JSON.parse(readFileSync(file, "utf8")).pid === process.pid) rmSync(file, { force: true });
    } catch {}
  });
  for (const signal of ["SIGTERM", "SIGINT"] as const) process.on(signal, () => process.exit(0));
  return { session, close: () => server.close() };
}
