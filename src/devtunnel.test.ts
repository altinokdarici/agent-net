import assert from "node:assert/strict";
import { test } from "node:test";
import { DevtunnelCli, parseExpiration } from "./devtunnel.ts";

function fake(responses: Record<string, unknown>) {
  const calls: string[][] = [];
  const cli = new DevtunnelCli(async args => {
    calls.push(args);
    const key = args.slice(0, args[0] === "port" ? 2 : 1).join(" ");
    if (!(key in responses)) throw new Error(`unexpected ${key}`);
    return JSON.stringify(responses[key]);
  });
  return { cli, calls };
}

test("parseExpiration reads CLI timestamps as UTC", () => {
  assert.equal(parseExpiration("2026-11-06 21:00:36 UTC"), Date.UTC(2026, 10, 6, 21, 0, 36));
  assert.throws(() => parseExpiration("soon"), /Unexpected/);
});

test("list and show map tunnels and ports", async () => {
  const { cli, calls } = fake({
    list: { tunnels: [{ tunnelId: "a.usw2", labels: ["copilot-agent"], description: "box" }] },
    show: { tunnel: { tunnelId: "a.usw2", labels: [], ports: [{ portNumber: 4590, portUri: "https://a-4590/" }] } },
  });
  assert.deepEqual(await cli.list("copilot-agent"), [{ tunnelId: "a.usw2", labels: ["copilot-agent"], description: "box", ports: [] }]);
  assert.deepEqual((await cli.show("a.usw2")).ports, [{ portNumber: 4590, portUri: "https://a-4590/" }]);
  assert.deepEqual(calls[0], ["list", "--labels", "copilot-agent"]);
});

test("list tolerates an empty response", async () => {
  const { cli } = fake({ list: {} });
  assert.deepEqual(await cli.list("x"), []);
});

test("create, createPort and token pass the right arguments", async () => {
  const { cli, calls } = fake({
    create: { tunnel: { tunnelId: "b.usw2", labels: ["copilot-agent", "host-x"], description: "x" } },
    "port create": { port: {} },
    token: { token: "T", expiration: "2026-11-06 21:00:36 UTC" },
  });
  assert.equal((await cli.create(["copilot-agent", "host-x"], "x")).tunnelId, "b.usw2");
  await cli.createPort("b.usw2", 4590);
  assert.deepEqual(await cli.token("b.usw2", "connect"), { token: "T", expiresAt: Date.UTC(2026, 10, 6, 21, 0, 36) });
  assert.deepEqual(calls, [
    ["create", "--labels", "copilot-agent", "host-x", "--description", "x"],
    ["port", "create", "b.usw2", "--port-number", "4590", "--protocol", "http"],
    ["token", "b.usw2", "--scopes", "connect"],
  ]);
});
