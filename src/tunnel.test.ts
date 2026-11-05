import assert from "node:assert/strict";
import { test } from "node:test";
import type { AccessToken, DevtunnelCli, TunnelInfo } from "./devtunnel.ts";
import { MachineTunnel, NETWORK_LABEL, machineLabel } from "./tunnel.ts";

class FakeCli {
  tunnels: TunnelInfo[] = [];
  tokenCalls = 0;
  async list(label: string) { return this.tunnels.filter(t => t.labels.includes(label)); }
  async show(id: string) { return this.tunnels.find(t => t.tunnelId === id)!; }
  async create(labels: string[], description: string) {
    const tunnel = { tunnelId: `t${this.tunnels.length}.usw2`, labels, description, ports: [] };
    this.tunnels.push(tunnel);
    return tunnel;
  }
  async createPort(id: string, portNumber: number) {
    (await this.show(id)).ports.push({ portNumber, portUri: `https://${id}-${portNumber}/` });
  }
  async token(_id: string, scope: string): Promise<AccessToken> {
    this.tokenCalls++;
    return { token: `${scope}-${this.tokenCalls}`, expiresAt: Date.now() + 3_600_000 };
  }
}

function setup(name = "my.box") {
  const cli = new FakeCli();
  const tunnel = new MachineTunnel({ cli: cli as unknown as DevtunnelCli, name, port: 4590 });
  return { cli, tunnel };
}

test("machineLabel keeps only allowed characters and length", () => {
  assert.equal(machineLabel("Altinoks-MacBook.local"), "host-Altinoks-MacBook-local");
  assert.equal(machineLabel("x".repeat(80)).length, 50);
});

test("ensure creates the tunnel and port once, then reuses them", async () => {
  const { cli, tunnel } = setup();
  const id = await tunnel.ensure();
  assert.deepEqual(cli.tunnels[0].labels, [NETWORK_LABEL, "host-my-box"]);
  assert.equal(cli.tunnels[0].description, "my.box");
  const again = new MachineTunnel({ cli: cli as unknown as DevtunnelCli, name: "my.box", port: 4590 });
  assert.equal(await again.ensure(), id);
  assert.equal(cli.tunnels.length, 1);
  assert.equal(cli.tunnels[0].ports.length, 1);
});

test("ensure adds a missing port to an existing tunnel", async () => {
  const { cli, tunnel } = setup();
  cli.tunnels.push({ tunnelId: "old.usw2", labels: [NETWORK_LABEL, "host-my-box"], description: "my.box", ports: [] });
  assert.equal(await tunnel.ensure(), "old.usw2");
  assert.equal(cli.tunnels[0].ports[0].portNumber, 4590);
});

test("peers lists every machine with its URL and marks this one", async () => {
  const { cli, tunnel } = setup();
  await tunnel.ensure();
  cli.tunnels.push({ tunnelId: "mac.usw2", labels: [NETWORK_LABEL, "host-mac"], description: "", ports: [{ portNumber: 4590, portUri: "https://mac/" }] });
  cli.tunnels.push({ tunnelId: "noport.usw2", labels: [NETWORK_LABEL], description: "x", ports: [] });
  assert.deepEqual(await tunnel.peers(), [
    { name: "my.box", tunnelId: "t0.usw2", url: "https://t0.usw2-4590/", self: true },
    { name: "mac", tunnelId: "mac.usw2", url: "https://mac/", self: false },
  ]);
});

test("token is cached until close to expiry", async () => {
  const { cli, tunnel } = setup();
  assert.equal(await tunnel.token("a", "connect"), "connect-1");
  assert.equal(await tunnel.token("a", "connect"), "connect-1");
  assert.equal(await tunnel.token("a", "host"), "host-2");
  assert.equal(cli.tokenCalls, 2);
});

test("start hosts with a host token from the CLI", async () => {
  const cli = new FakeCli();
  let connected = "";
  const tunnel = new MachineTunnel({
    cli: cli as unknown as DevtunnelCli, name: "box", port: 4590,
    hostFactory: async (id, hostToken) => ({ connect: async () => { connected = `${id} ${await hostToken()}`; }, dispose: async () => {} }),
  });
  await tunnel.start();
  assert.equal(connected, "t0.usw2 host-1");
  await tunnel.stop();
});
