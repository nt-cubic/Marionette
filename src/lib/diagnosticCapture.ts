import { appendDebugLog, isTauriRuntime } from "./api";

export type CapturedUiError = {
  source: "ui-error" | "ui-unhandled-rejection" | "ui-react-boundary";
  message: string;
  capturedAt: string;
};

const STORAGE_KEY = "marionette-last-ui-error";
let installed = false;
let lastError: CapturedUiError | null = readStoredUiError();

function formatUnknownError(value: unknown): string {
  if (value instanceof Error) {
    return value.stack || value.name + ": " + value.message;
  }
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function readStoredUiError(): CapturedUiError | null {
  try {
    const raw = window.sessionStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<CapturedUiError>;
    if (
      typeof parsed.source !== "string" ||
      typeof parsed.message !== "string" ||
      typeof parsed.capturedAt !== "string"
    ) {
      return null;
    }
    return {
      source: parsed.source as CapturedUiError["source"],
      message: parsed.message,
      capturedAt: parsed.capturedAt,
    };
  } catch {
    return null;
  }
}

function remember(next: CapturedUiError): void {
  lastError = next;
  try {
    window.sessionStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // sessionStorage can be disabled; module memory still carries the report.
  }
}

export function recordUiError(input: {
  source: CapturedUiError["source"];
  message: string;
  detail?: string;
}): void {
  remember({
    source: input.source,
    message: input.message,
    capturedAt: new Date().toISOString(),
  });
  void appendDebugLog({
    source: input.source,
    level: "error",
    summary: input.message,
    detail: input.detail,
  });
}

/**
 * Installs process-wide UI crash breadcrumbs before React mounts. The handlers
 * are best-effort and never change the browser's default error reporting.
 */
export function installDiagnosticCapture(): void {
  if (installed || !isTauriRuntime()) return;
  installed = true;

  window.addEventListener("error", (event) => {
    if (!event.message) return;
    const message = event.message;
    const location = event.filename
      ? "at " + event.filename + ":" + event.lineno + ":" + event.colno
      : "";
    const stack = event.error ? formatUnknownError(event.error) : "";
    const detail = [location, stack].filter(Boolean).join(" | ");
    recordUiError({
      source: "ui-error",
      message,
      detail,
    });
  });

  window.addEventListener("unhandledrejection", (event) => {
    const detail = formatUnknownError(event.reason);
    const firstLine = detail.split(/\r?\n/, 1)[0]?.trim() || "Unknown rejection";
    const message = "Unhandled promise rejection: " + firstLine.slice(0, 220);
    recordUiError({
      source: "ui-unhandled-rejection",
      message,
      detail,
    });
  });
}

export function lastCapturedUiError(): CapturedUiError | null {
  return lastError;
}

export function clearCapturedUiError(): void {
  lastError = null;
  try {
    window.sessionStorage.removeItem(STORAGE_KEY);
  } catch {
    // Nothing else to clear.
  }
}
