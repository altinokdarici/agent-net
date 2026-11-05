import { execFile } from "node:child_process";

/** Runs the `devtunnel` CLI with JSON output and returns its stdout. */
export type Run = (args: string[]) => Promise<string>;

export const runDevtunnel: Run = args => new Promise((resolve, reject) => {
  execFile("devtunnel", [...args, "--json"], { windowsHide: true, maxBuffer: 4 * 1024 * 1024 }, (error, stdout, stderr) => {
    if (error) {
      const detail = `${stdout}\n${stderr}`.trim().split("\n").at(-1) ?? "";
      reject(new Error(`devtunnel ${args[0]} failed: ${detail || error.message}`));
    } else {
      resolve(stdout);
    }
  });
});

export type TunnelPort = { portNumber: number; portUri?: string };
export type TunnelInfo = { tunnelId: string; labels: string[]; description: string; ports: TunnelPort[] };
export type AccessToken = { token: string; expiresAt: number };
export type Scope = "host" | "connect";

function tunnelInfo(value: Record<string, unknown>): TunnelInfo {
  return {
    tunnelId: String(value.tunnelId),
    labels: Array.isArray(value.labels) ? value.labels.map(String) : [],
    description: typeof value.description === "string" ? value.description : "",
    ports: Array.isArray(value.ports) ? value.ports.map(port => ({ portNumber: Number(port.portNumber), portUri: port.portUri })) : [],
  };
}

/** Parses the CLI's "2026-11-06 21:00:36 UTC" timestamps. */
export function parseExpiration(value: string): number {
  const time = Date.parse(value.replace(" UTC", "Z").replace(" ", "T"));
  if (Number.isNaN(time)) throw new Error(`Unexpected token expiration: ${value}`);
  return time;
}

/** Thin wrapper over the `devtunnel` CLI. The CLI owns the user's sign-in. */
export class DevtunnelCli {
  private run: Run;
  constructor(run: Run = runDevtunnel) {
    this.run = run;
  }
  private async json(args: string[]): Promise<Record<string, unknown>> {
    return JSON.parse(await this.run(args));
  }
  async list(label: string): Promise<TunnelInfo[]> {
    const result = await this.json(["list", "--labels", label]);
    return Array.isArray(result.tunnels) ? result.tunnels.map(tunnelInfo) : [];
  }
  async show(tunnelId: string): Promise<TunnelInfo> {
    return tunnelInfo((await this.json(["show", tunnelId])).tunnel as Record<string, unknown>);
  }
  async create(labels: string[], description: string): Promise<TunnelInfo> {
    return tunnelInfo((await this.json(["create", "--labels", ...labels, "--description", description])).tunnel as Record<string, unknown>);
  }
  async createPort(tunnelId: string, portNumber: number): Promise<void> {
    await this.run(["port", "create", tunnelId, "--port-number", String(portNumber), "--protocol", "http"]);
  }
  async token(tunnelId: string, scope: Scope): Promise<AccessToken> {
    const result = await this.json(["token", tunnelId, "--scopes", scope]);
    return { token: String(result.token), expiresAt: parseExpiration(String(result.expiration)) };
  }
}
