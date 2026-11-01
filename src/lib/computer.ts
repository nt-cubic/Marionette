// Computer Use (batch 3): typed wrappers over the Rust commands. The agent
// tool surface rides the same localhost MCP as the browser tools; this file
// is only the human side — the share dialog and the driver installer.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isTauriRuntime } from "./api";

export type ShareableWindow = {
  windowId: number;
  pid: number;
  title: string | null;
  bounds: unknown | null;
  minimized: boolean;
  processImage: string | null;
  processStartedAtMs: number | null;
  grantable: boolean;
  shared: "none" | "read" | "control";
};

export type DriverStatus = {
  version: string;
  installed: boolean;
  path: string | null;
};

export type ShareableState = {
  windows: ShareableWindow[];
  shares: { key: unknown; level: string; label: string }[];
  driver: DriverStatus;
  /** Listing degraded (helper/driver hiccup) — the driver itself is fine. */
  helperError?: string | null;
};

export type DriverProgress = {
  phase: "downloading" | "installed" | "failed";
  downloaded?: number;
  total?: number;
};

export async function computerListShareable(): Promise<ShareableState | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<ShareableState>("computer_list_shareable");
  } catch {
    return null;
  }
}

export async function computerShare(
  target: { kind: string; pid?: number; startedAtMs?: number; windowId?: number },
  level: "read" | "control",
): Promise<void> {
  await invoke("computer_share", { target, level });
}

export async function computerRevokeAll(): Promise<void> {
  await invoke("computer_revoke_all");
}

export async function computerInstallDriver(): Promise<{ installed: boolean } | null> {
  if (!isTauriRuntime()) return null;
  return invoke<{ installed: boolean }>("computer_install_driver");
}

export async function computerUninstallDriver(): Promise<void> {
  await invoke("computer_uninstall_driver");
}

export function onDriverProgress(handler: (progress: DriverProgress) => void): Promise<UnlistenFn> {
  return listen<DriverProgress>("marionette-computer-driver", (event) => handler(event.payload));
}
