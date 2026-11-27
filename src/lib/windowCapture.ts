// P1-10: capture one user-owned window into a normal draft attachment.
//
// This file deliberately has no App/Composer wiring. It returns a
// prefill-only payload; the caller decides whether it lands in the current
// composer or a freshly created chat. It never calls computerShare(), so a
// capture does not grant an agent read/control access. The two Tauri commands
// below are the thin human-side counterparts of the Computer Use capture
// tools; they must be added to Rust (see the report) because the agent tools'
// ensure_read() gate is not the right gate for a user-initiated screenshot.

import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime, savePastedImage } from "./api";
import { computerListShareable, type ShareableWindow } from "./computer";
import { attachmentFromPath, type ImageAttachment } from "./imageAttachments";
import { t } from "./i18n";

export const WINDOW_CAPTURE_CAPTURE_COMMAND = "computer_capture_window";
export const WINDOW_CAPTURE_SNAPSHOT_COMMAND = "computer_snapshot_window";

export const WINDOW_CAPTURE_MAX_WIDTH = 1568;
/** save_pasted_image refuses >12 MB; leave headroom for base64/extension. */
const MAX_CAPTURE_BYTES = 11 * 1024 * 1024;
const MAX_DRAFT_TEXT_CHARS = 4200;

export type CaptureDraftTarget = "current" | "new";

export type WindowCaptureImage = {
  /** A `data:image/...;base64,...` URL from the Computer Use capture path. */
  dataUrl: string;
  width: number;
  height: number;
};

export type WindowCaptureBackend = {
  capture(window: ShareableWindow, options: { maxWidth: number }): Promise<WindowCaptureImage>;
  accessibility(window: ShareableWindow): Promise<unknown>;
};

export type SavedCaptureImage = {
  path: string;
  name?: string;
  mimeType?: string;
};

export type WindowCaptureSaveImage = (dataUrl: string, mimeType: string) => Promise<SavedCaptureImage>;

export type WindowCaptureFailureKind =
  | "not-desktop"
  | "driver-missing"
  | "list-unavailable"
  | "not-capturable"
  | "window-minimized"
  | "capture-unavailable"
  | "capture-failed"
  | "invalid-image"
  | "image-too-large"
  | "save-failed"
  | "draft-unavailable"
  | "unexpected";

export type WindowCaptureFailure = {
  ok: false;
  kind: WindowCaptureFailureKind;
  message: string;
  retryable: boolean;
  cause?: string;
};

export type WindowCaptureListResult =
  | { ok: true; windows: CapturableWindow[]; warning?: string }
  | {
      ok: false;
      kind: "not-desktop" | "driver-missing" | "list-unavailable";
      message: string;
      cause?: string;
    };

export type WindowCaptureWarning =
  | "summary-unavailable"
  | "summary-empty"
  | "summary-truncated"
  | "window-shared";

export type WindowCaptureDraftPayload = {
  /** The caller chooses current vs new; this helper never creates sessions. */
  target: CaptureDraftTarget;
  attachments: ImageAttachment[];
  /** Bounded caption/summary block for the composer; never auto-sent. */
  text: string;
  delivery: "prefill-only";
  warnings: WindowCaptureWarning[];
  meta: {
    windowId: number;
    pid: number;
    title: string;
    processImage: string | null;
    processName: string;
    capturedAt: string;
    width: number;
    height: number;
    summaryNodes: number;
    summaryChars: number;
    summaryTruncated: boolean;
    shared: ShareableWindow["shared"];
  };
};

export type WindowCaptureResult =
  | { ok: true; payload: WindowCaptureDraftPayload }
  | WindowCaptureFailure;

export type AccessibilitySummaryOptions = {
  /** Total characters of the summary body (not counting the draft header). */
  maxChars?: number;
  maxNodes?: number;
  maxDepth?: number;
  maxNameChars?: number;
  /** Values are included unless the field looks sensitive. */
  includeValues?: boolean;
};

export type AccessibilitySummary = {
  text: string;
  nodes: number;
  truncated: boolean;
};

export type CapturableWindow = ShareableWindow & { grantable: true };

const DEFAULT_SUMMARY_OPTIONS: Required<AccessibilitySummaryOptions> = {
  maxChars: 1800,
  maxNodes: 120,
  maxDepth: 12,
  maxNameChars: 120,
  includeValues: true,
};

const ROLE_KEYS = ["role", "roleName", "controlType", "localizedControlType", "type"] as const;
const NAME_KEYS = ["name", "title", "label", "text", "description", "placeholder"] as const;
const VALUE_KEYS = ["value", "valueText", "selectedValue"] as const;
const CHILD_KEYS = [
  "root",
  "tree",
  "children",
  "childNodes",
  "descendants",
  "subtree",
  "nodes",
  "elements",
  "items",
  "accessibilityTree",
] as const;

const SENSITIVE = /\b(password|passwd|pwd|secret|token|credential|api[-_ ]?key|private[-_ ]?key|authorization|cookie)\b/i;
const NODE_TEXT_KEYS = new Set<string>([...ROLE_KEYS, ...NAME_KEYS, ...VALUE_KEYS]);
const SKIP_WALK_KEYS = new Set([
  "pid",
  "windowId",
  "window_id",
  "bounds",
  "x",
  "y",
  "width",
  "height",
  "value",
  "valueText",
  "selectedValue",
  "token",
  "elementToken",
  "element_token",
  "snapshotId",
  "snapshot_id",
  "id",
]);

function unknownErrorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function isMissingCommand(error: string): boolean {
  return /not found|not allowed|unknown command|command .*not.*found|missing command/i.test(error);
}

function toPositiveInteger(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? Math.round(value) : 0;
}

export function windowProcessName(window: ShareableWindow): string {
  const raw = window.processImage ?? "";
  const base = raw.replace(/\\/g, "/").split("/").pop() ?? "";
  return base || t("capture.unknownApp");
}

function ownProcess(window: ShareableWindow): boolean {
  const base = (window.processImage ?? "").replace(/\\/g, "/").split("/").pop()?.toLowerCase() ?? "";
  return base === "marionette.exe" || base === "marionette";
}

export function isCapturableWindow(window: ShareableWindow): window is CapturableWindow {
  return (
    window.grantable === true &&
    Number.isSafeInteger(window.pid) &&
    window.pid > 0 &&
    Number.isSafeInteger(window.windowId) &&
    window.windowId > 0 &&
    !ownProcess(window)
  );
}

/** Grantable is the Rust-side own-app/blocklist verdict; do not bypass it. */
export function capturableWindows(windows: readonly ShareableWindow[]): CapturableWindow[] {
  return windows.filter(isCapturableWindow);
}

export function windowCaptureKey(window: ShareableWindow): string {
  return `${window.pid}:${window.windowId}`;
}

export async function loadCapturableWindows(): Promise<WindowCaptureListResult> {
  if (!isTauriRuntime()) {
    return { ok: false, kind: "not-desktop", message: t("capture.error.notDesktop") };
  }
  const state = await computerListShareable();
  if (!state) {
    return {
      ok: false,
      kind: "list-unavailable",
      message: t("capture.error.listUnavailable"),
      cause: "computer_list_shareable returned no state",
    };
  }
  if (!state.driver.installed) {
    return {
      ok: false,
      kind: "driver-missing",
      message: t("capture.error.driverMissing"),
      cause: state.helperError ?? undefined,
    };
  }
  const windows = capturableWindows(state.windows);
  if (windows.length === 0 && state.helperError) {
    return {
      ok: false,
      kind: "list-unavailable",
      message: t("capture.error.listUnavailable"),
      cause: state.helperError,
    };
  }
  return { ok: true, windows, warning: state.helperError ?? undefined };
}

/**
 * The default backend calls the two thin Tauri commands. They are intentionally
 * NOT the agent tools (computer_screenshot / computer_snapshot): those call
 * ensure_read() and would either demand a share or widen agent permissions.
 */
export const tauriWindowCaptureBackend: WindowCaptureBackend = {
  async capture(window, options) {
    const image = await invoke<WindowCaptureImage>(WINDOW_CAPTURE_CAPTURE_COMMAND, {
      pid: window.pid,
      windowId: window.windowId,
      maxWidth: options.maxWidth,
    });
    return {
      dataUrl: typeof image?.dataUrl === "string" ? image.dataUrl : "",
      width: toPositiveInteger(image?.width),
      height: toPositiveInteger(image?.height),
    };
  },
  async accessibility(window) {
    return invoke<unknown>(WINDOW_CAPTURE_SNAPSHOT_COMMAND, {
      pid: window.pid,
      windowId: window.windowId,
    });
  },
};

async function defaultSaveImage(dataUrl: string, mimeType: string): Promise<SavedCaptureImage> {
  const saved = await savePastedImage(dataUrl, mimeType);
  return { path: saved.path, name: saved.name, mimeType: saved.mimeType };
}

function parseImageDataUrl(value: string): { mimeType: string; base64: string } | null {
  const match = /^data:(image\/(?:png|jpeg|jpg|webp|bmp));base64,([A-Za-z0-9+/=]+)$/i.exec(value.trim());
  if (!match) return null;
  const mime = match[1]?.toLowerCase().replace("image/jpg", "image/jpeg") ?? "";
  const base64 = match[2] ?? "";
  if (!mime || !base64) return null;
  return { mimeType: mime, base64 };
}

function base64ByteLength(base64: string): number {
  const padding = base64.endsWith("==") ? 2 : base64.endsWith("=") ? 1 : 0;
  return Math.max(0, Math.floor((base64.length * 3) / 4) - padding);
}

function cleanText(value: string, maxChars: number): string {
  let text = value
    .replace(/[\u0000-\u001f\u007f]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  text = redactUrlSecrets(text);
  if (text.length > maxChars) text = `${text.slice(0, Math.max(0, maxChars - 3))}...`;
  return text;
}

function redactUrlSecrets(value: string): string {
  if (!/^https?:\/\//i.test(value)) return value;
  try {
    const url = new URL(value);
    url.username = "";
    url.password = "";
    for (const key of [...url.searchParams.keys()]) {
      if (SENSITIVE.test(key)) url.searchParams.set(key, "...");
    }
    url.hash = "";
    return url.toString();
  } catch {
    return value;
  }
}

function firstString(node: Record<string, unknown>, keys: readonly string[]): string {
  for (const key of keys) {
    const value = node[key];
    if (typeof value === "string" && value.trim()) return value;
  }
  return "";
}

function normalizeTree(value: unknown): unknown {
  if (typeof value !== "string") return value;
  const text = value.trim();
  if (!text.startsWith("{") && !text.startsWith("[")) return value;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return value;
  }
}

/**
 * Bounded, local-only accessibility summary. It keeps roles/names and
 * non-sensitive values, drops element tokens/bounds/internal ids entirely,
 * redacts URL credentials/secret query params, and hard-stops on node/char/
 * depth budgets. The tree is untrusted data; this never evaluates it.
 */
export function summarizeAccessibilityTree(
  tree: unknown,
  options: AccessibilitySummaryOptions = {},
): AccessibilitySummary {
  const limits = { ...DEFAULT_SUMMARY_OPTIONS, ...options };
  const root = normalizeTree(tree);
  const lines: string[] = [];
  let nodes = 0;
  let chars = 0;
  let truncated = false;
  const seen = new WeakSet<object>();

  const emit = (line: string) => {
    if (!line) return;
    if (lines[lines.length - 1] === line) return;
    if (chars + line.length + (lines.length > 0 ? 1 : 0) > limits.maxChars) {
      truncated = true;
      return false;
    }
    lines.push(line);
    chars += line.length + (lines.length > 1 ? 1 : 0);
    return true;
  };

  const walk = (value: unknown, depth: number): void => {
    if (truncated || depth > limits.maxDepth) {
      if (depth > limits.maxDepth) truncated = true;
      return;
    }
    if (nodes >= limits.maxNodes) {
      truncated = true;
      return;
    }
    if (typeof value === "string") {
      if (!value.trim()) return;
      nodes += 1;
      emit(cleanText(value, limits.maxNameChars));
      return;
    }
    if (Array.isArray(value)) {
      if (seen.has(value)) return;
      seen.add(value);
      for (const item of value) {
        if (truncated) return;
        walk(item, depth);
      }
      return;
    }
    if (!value || typeof value !== "object") return;
    const object = value as Record<string, unknown>;
    if (seen.has(object)) return;
    seen.add(object);
    nodes += 1;

    const role = cleanText(firstString(object, ROLE_KEYS), limits.maxNameChars);
    const name = cleanText(firstString(object, NAME_KEYS), limits.maxNameChars);
    const rawValue = firstString(object, VALUE_KEYS);
    const valueText = limits.includeValues && rawValue ? cleanText(rawValue, limits.maxNameChars) : "";
    const sensitive = SENSITIVE.test(role) || SENSITIVE.test(name);

    let line = "";
    if (sensitive) {
      // Names of credential fields can themselves carry the secret (e.g. an
      // error string), so emit only the role. Values are never read.
      line = `${role || "field"} (value hidden)`;
    } else {
      if (role && name) line = `${role} "${name}"`;
      else if (role) line = role;
      else if (name) line = `"${name}"`;
      if (line && valueText && valueText !== name) line += ` = "${valueText}"`;
    }
    if (line) {
      if (object.disabled === true) line += " (disabled)";
      else if (object.checked === true) line += " (checked)";
      if (emit(line) === false) return;
    }

    if (sensitive) return;
    for (const key of CHILD_KEYS) {
      if (key in object) {
        walk(object[key], depth + 1);
        if (truncated) return;
      }
    }
    // Unknown shapes: walk object values, but never the metadata-ish keys.
    const knownChild = CHILD_KEYS.some((key) => key in object);
    if (!knownChild) {
      for (const [key, child] of Object.entries(object)) {
        if (SKIP_WALK_KEYS.has(key) || NODE_TEXT_KEYS.has(key)) continue;
        if (line && (typeof child !== "object" || child === null)) continue;
        walk(child, depth + 1);
        if (truncated) return;
      }
    }
  };

  walk(root, 0);
  return { text: lines.join("\n"), nodes: lines.length, truncated };
}

function composeDraftText(
  window: ShareableWindow,
  image: WindowCaptureImage,
  summary: AccessibilitySummary | null,
): string {
  const title = window.title?.trim() || t("capture.untitledWindow");
  const size =
    image.width > 0 && image.height > 0
      ? `${image.width}x${image.height}`
      : t("capture.unknownSize");
  const parts = [
    t("capture.draftTitle", {
      title: cleanText(title, 160),
      app: cleanText(windowProcessName(window), 80),
      size,
    }),
  ];
  if (summary?.text) {
    parts.push(
      "",
      summary.truncated
        ? t("capture.draftSummaryTruncated", { count: summary.nodes })
        : t("capture.draftSummary", { count: summary.nodes }),
      summary.text,
    );
  } else {
    parts.push("", t("capture.draftNoSummary"));
  }
  return parts.join("\n").slice(0, MAX_DRAFT_TEXT_CHARS);
}

export type CaptureWindowOptions = {
  target: CaptureDraftTarget;
  backend?: WindowCaptureBackend;
  saveImage?: WindowCaptureSaveImage;
  maxWidth?: number;
  summary?: AccessibilitySummaryOptions;
};

/**
 * Capture one window into a prefill-only draft payload. Image persistence uses
 * save_pasted_image, so the file lives under Marionette's clipboard storage and
 * follows the same lifecycle as a pasted image. No agent share is created, no
 * Computer Use permission changes, and nothing is sent.
 */
export async function captureWindowIntoDraft(
  window: ShareableWindow,
  options: CaptureWindowOptions,
): Promise<WindowCaptureResult> {
  if (!isTauriRuntime()) {
    return { ok: false, kind: "not-desktop", message: t("capture.error.notDesktop"), retryable: false };
  }
  if (!isCapturableWindow(window)) {
    return {
      ok: false,
      kind: "not-capturable",
      message: t("capture.error.notCapturable"),
      retryable: false,
    };
  }
  if (window.minimized) {
    return {
      ok: false,
      kind: "window-minimized",
      message: t("capture.error.minimized"),
      retryable: true,
    };
  }

  const backend = options.backend ?? tauriWindowCaptureBackend;
  const maxWidth = Math.min(
    2048,
    Math.max(320, Math.round(options.maxWidth ?? WINDOW_CAPTURE_MAX_WIDTH)),
  );

  let shot: WindowCaptureImage;
  try {
    shot = await backend.capture(window, { maxWidth });
  } catch (error) {
    const cause = unknownErrorText(error);
    if (isMissingCommand(cause)) {
      return {
        ok: false,
        kind: "capture-unavailable",
        message: t("capture.error.captureUnavailable"),
        retryable: false,
        cause,
      };
    }
    return {
      ok: false,
      kind: "capture-failed",
      message: t("capture.error.captureFailed", { reason: cause }),
      retryable: true,
      cause,
    };
  }

  const parsed = parseImageDataUrl(shot.dataUrl);
  if (!parsed) {
    return {
      ok: false,
      kind: "invalid-image",
      message: t("capture.error.invalidImage"),
      retryable: true,
      cause: shot.dataUrl.slice(0, 80),
    };
  }
  if (base64ByteLength(parsed.base64) > MAX_CAPTURE_BYTES) {
    return {
      ok: false,
      kind: "image-too-large",
      message: t("capture.error.imageTooLarge"),
      retryable: false,
    };
  }

  const warnings: WindowCaptureWarning[] = [];
  let summary: AccessibilitySummary | null = null;
  try {
    const tree = await backend.accessibility(window);
    summary = summarizeAccessibilityTree(tree, options.summary);
    if (!summary.text) warnings.push("summary-empty");
    else if (summary.truncated) warnings.push("summary-truncated");
  } catch {
    // The screenshot is the primary artifact; a missing tree is a visible
    // degradation, not a failed capture.
    warnings.push("summary-unavailable");
  }
  if (window.shared !== "none") warnings.push("window-shared");

  const normalizedImage: WindowCaptureImage = {
    dataUrl: shot.dataUrl,
    width: toPositiveInteger(shot.width),
    height: toPositiveInteger(shot.height),
  };

  let saved: SavedCaptureImage;
  try {
    saved = await (options.saveImage ?? defaultSaveImage)(shot.dataUrl, parsed.mimeType);
  } catch (error) {
    const cause = unknownErrorText(error);
    const tooLarge = /too large|12 MB|12MB/i.test(cause);
    return {
      ok: false,
      kind: tooLarge ? "image-too-large" : "save-failed",
      message: tooLarge
        ? t("capture.error.imageTooLarge")
        : t("capture.error.saveFailed", { reason: cause }),
      retryable: !tooLarge,
      cause,
    };
  }

  const attachment = attachmentFromPath(saved.path);
  if (saved.name) attachment.name = saved.name;
  if (saved.mimeType) attachment.mimeType = saved.mimeType;

  return {
    ok: true,
    payload: {
      target: options.target,
      attachments: [attachment],
      text: composeDraftText(window, normalizedImage, summary),
      delivery: "prefill-only",
      warnings,
      meta: {
        windowId: window.windowId,
        pid: window.pid,
        title: window.title ?? "",
        processImage: window.processImage,
        processName: windowProcessName(window),
        capturedAt: new Date().toISOString(),
        width: normalizedImage.width,
        height: normalizedImage.height,
        summaryNodes: summary?.nodes ?? 0,
        summaryChars: summary?.text.length ?? 0,
        summaryTruncated: summary?.truncated ?? false,
        shared: window.shared,
      },
    },
  };
}
