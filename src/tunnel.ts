import { TunnelRelayTunnelHost } from "@microsoft/dev-tunnels-connections";
import { ManagementApiVersions, TunnelManagementHttpClient } from "@microsoft/dev-tunnels-management";
import type { DevtunnelCli, TunnelInfo } from "./devtunnel.ts";

/** Label shared by every machine's tunnel in one user's network. */
export const NETWORK_LABEL = "copilot-agent";

/** Dev Tunnels labels allow only [\w-=] and at most 50 characters. */
export function machineLabel(name: string): string {
  return `host-${name.replace(/[^\w-]/g, "-")}`.slice(0, 50);
}

export type Peer = { name: string; tunnelId: string; url: string; self: boolean };

type Host = { connect(): Promise<void>; dispose(): Promise<void> };
export type HostFactory = (tunnelId: string, hostToken: () => Promise<string>) => Promise<Host>;

const REFRESH_MARGIN = 5 * 60_000;

/** Hosts this machine's tunnel with the SDK, using tunnel tokens issued by the CLI. */
export const sdkHost: HostFactory = async (tunnelId, hostToken) => {
  const [id, clusterId] = tunnelId.split(".");
  const management = new TunnelManagementHttpClient({ name: "agent-net", version: "0.1.0" }, ManagementApiVersions.Version20230927preview, async () => null);
  const token = await hostToken();
  const tunnel = await management.getTunnel({ tunnelId: id, clusterId }, { accessToken: token, includePorts: true });
  if (!tunnel) throw new Error(`Tunnel ${tunnelId} was not found.`);
  tunnel.accessTokens = { ...tunnel.accessTokens, host: token };
  const host = new TunnelRelayTunnelHost(management);
  host.refreshingTunnelAccessToken(event => {
    event.tunnelAccessToken = hostToken();
  });
  return {
    connect: () => host.connect(tunnel, { enableReconnect: true, keepAliveIntervalInSeconds: 15 }),
    dispose: () => host.dispose(),
  };
};

/** One persistent private tunnel per machine, exposing only the daemon port. */
export class MachineTunnel {
  private cli: DevtunnelCli;
  private name: string;
  private port: number;
  private hostFactory: HostFactory;
  private tokens = new Map<string, { token: string; expiresAt: number }>();
  private urls = new Map<string, string>();
  private host?: Host;
  tunnelId?: string;

  constructor(options: { cli: DevtunnelCli; name: string; port: number; hostFactory?: HostFactory }) {
    this.cli = options.cli;
    this.name = options.name;
    this.port = options.port;
    this.hostFactory = options.hostFactory ?? sdkHost;
  }

  /** Finds this machine's tunnel, or creates it, and makes sure it forwards the daemon port. */
  async ensure(): Promise<string> {
    const label = machineLabel(this.name);
    let tunnel: TunnelInfo | undefined = (await this.cli.list(NETWORK_LABEL)).find(t => t.labels.includes(label));
    tunnel ??= await this.cli.create([NETWORK_LABEL, label], this.name);
    const details = await this.cli.show(tunnel.tunnelId);
    if (!details.ports.some(p => p.portNumber === this.port)) await this.cli.createPort(tunnel.tunnelId, this.port);
    this.tunnelId = tunnel.tunnelId;
    return tunnel.tunnelId;
  }

  async start(): Promise<void> {
    const tunnelId = this.tunnelId ?? await this.ensure();
    this.host = await this.hostFactory(tunnelId, () => this.token(tunnelId, "host"));
    await this.host.connect();
  }

  async stop(): Promise<void> {
    await this.host?.dispose();
    this.host = undefined;
  }

  /** Every machine in the network, including this one. */
  async peers(): Promise<Peer[]> {
    const tunnels = await this.cli.list(NETWORK_LABEL);
    const peers: Peer[] = [];
    for (const tunnel of tunnels) {
      const url = await this.url(tunnel.tunnelId);
      if (!url) continue;
      const label = tunnel.labels.find(l => l.startsWith("host-"));
      peers.push({ name: tunnel.description || label?.slice(5) || tunnel.tunnelId, tunnelId: tunnel.tunnelId, url, self: tunnel.tunnelId === this.tunnelId });
    }
    return peers;
  }

  private async url(tunnelId: string): Promise<string | undefined> {
    let url = this.urls.get(tunnelId);
    if (!url) {
      url = (await this.cli.show(tunnelId)).ports.find(p => p.portNumber === this.port)?.portUri;
      if (url) this.urls.set(tunnelId, url);
    }
    return url;
  }

  async token(tunnelId: string, scope: "host" | "connect"): Promise<string> {
    const key = `${scope}:${tunnelId}`;
    const cached = this.tokens.get(key);
    if (cached && cached.expiresAt - REFRESH_MARGIN > Date.now()) return cached.token;
    const fresh = await this.cli.token(tunnelId, scope);
    this.tokens.set(key, fresh);
    return fresh.token;
  }

  /** Sends an HTTP request to another machine's daemon through its tunnel. */
  async fetch(peer: Peer, path: string, init: RequestInit = {}): Promise<Response> {
    const headers = new Headers(init.headers);
    headers.set("X-Tunnel-Authorization", `tunnel ${await this.token(peer.tunnelId, "connect")}`);
    return fetch(new URL(path, peer.url), { ...init, headers });
  }
}
