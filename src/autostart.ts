import { homedir } from "node:os";
import { join } from "node:path";

export type Autostart = {
  /** Files to write before running the commands. */
  files: { path: string; content: string }[];
  install: Step[];
  uninstall: Step[];
};

/** A command to run. Steps that may fail, such as unloading something not loaded, set mayFail. */
export type Step = { command: string[]; mayFail?: boolean };

function xml(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/**
 * How to start the daemon at login on each OS, with the current PATH so it can find
 * copilot and devtunnel. Linux uses a systemd user service, macOS a LaunchAgent and
 * Windows a logon task that runs node in a headless console, so no window appears.
 */
export function autostart(options: { node: string; daemon: string; path: string; home?: string; platform?: NodeJS.Platform; uid?: number }): Autostart {
  const home = options.home ?? homedir();
  const platform = options.platform ?? process.platform;
  if (platform === "linux") {
    const path = join(home, ".config", "systemd", "user", "agent-net.service");
    const content = [
      "[Unit]",
      "Description=agent-net daemon",
      "",
      "[Service]",
      `ExecStart="${options.node}" "${options.daemon}"`,
      `Environment="PATH=${options.path}"`,
      "Restart=always",
      "RestartSec=10",
      "",
      "[Install]",
      "WantedBy=default.target",
      "",
    ].join("\n");
    return {
      files: [{ path, content }],
      install: [{ command: ["systemctl", "--user", "daemon-reload"] }, { command: ["systemctl", "--user", "enable", "--now", "agent-net.service"] }],
      uninstall: [{ command: ["systemctl", "--user", "disable", "--now", "agent-net.service"], mayFail: true }],
    };
  }
  if (platform === "darwin") {
    const path = join(home, "Library", "LaunchAgents", "com.agent-net.daemon.plist");
    const domain = `gui/${options.uid ?? process.getuid?.() ?? 0}`;
    const content = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.agent-net.daemon</string>
  <key>ProgramArguments</key>
  <array><string>${xml(options.node)}</string><string>${xml(options.daemon)}</string></array>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>${xml(options.path)}</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
`;
    return {
      files: [{ path, content }],
      install: [{ command: ["launchctl", "bootout", domain, path], mayFail: true }, { command: ["launchctl", "bootstrap", domain, path] }],
      uninstall: [{ command: ["launchctl", "bootout", domain, path], mayFail: true }],
    };
  }
  if (platform === "win32") {
    const run = `conhost.exe --headless "${options.node}" "${options.daemon}"`;
    return {
      files: [],
      install: [
        { command: ["schtasks", "/create", "/f", "/tn", "agent-net", "/sc", "onlogon", "/rl", "limited", "/tr", run] },
        { command: ["schtasks", "/run", "/tn", "agent-net"] },
      ],
      uninstall: [{ command: ["schtasks", "/end", "/tn", "agent-net"], mayFail: true }, { command: ["schtasks", "/delete", "/f", "/tn", "agent-net"], mayFail: true }],
    };
  }
  throw new Error(`Starting at login is not supported on ${platform}.`);
}
