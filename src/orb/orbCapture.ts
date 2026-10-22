/**
 * Capture plumbing for the orb window: normalize an HTML5 drop / clipboard
 * payload, persist file blobs through Rust (`capture_save_blob`), enrich URLs
 * (`capture_fetch_url` → image download / page title), and format the little
 * clock strings the panel shows. Purely best-effort: a failing item produces
 * a toast line, never a blocked capture.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "../lib/api";
import type { OrbCapturePayload } from "../lib/orbEvents";

/** Mirrors the Rust-side download cap (capture.rs). */
export const MAX_CAPTURE_BLOB_BYTES = 20 * 1024 * 1024;

/** Raw pieces pulled out of a DataTransfer (drop or clipboard). */
export type RawCaptureItems = {
  files: File[];
  urls: string[];
  html: string | null;
  text: string | null;
};

/** Shape returned by the Rust `capture_fetch_url` command (capture.rs). */
export type FetchedUrlInfo = {
  kind: "image" | "page" | "failed" | string;
  path?: string | null;
  title?: string | null;
};

export type CaptureProgress = {
  /** One async item (file save / url fetch) settled — success or failure. */
  onItemDone: () => void;
  /** Transient, non-blocking failure line for the toast area. */
  onItemFailed: (message: string) => void;
};

function readAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("read failed"));
    reader.onload = () => {
      const result = String(reader.result ?? "");
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.readAsDataURL(file);
  });
}

/**
 * Extract capture items from a drop or clipboard DataTransfer.
 * `text/uri-list` lines starting with `#` are comments; only http(s) URLs are
 * kept (file: URIs are handled through the Files track instead).
 */
export function extractCaptureItems(dt: DataTransfer | null): RawCaptureItems {
  const empty: RawCaptureItems = { files: [], urls: [], html: null, text: null };
  if (!dt) return empty;
  let files: File[] = [];
  try {
    files = Array.from(dt.files ?? []);
  } catch {
    files = [];
  }
  let urls: string[] = [];
  try {
    const uriList = dt.getData("text/uri-list");
    if (uriList) {
      urls = uriList
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter((line) => line !== "" && !line.startsWith("#") && /^https?:\/\//i.test(line));
    }
  } catch {
    /* getData may throw for unsupported types */
  }
  let html: string | null = null;
  try {
    const value = dt.getData("text/html");
    if (value) html = value;
  } catch {
    /* ignore */
  }
  let text: string | null = null;
  try {
    const value = dt.getData("text/plain");
    if (value) text = value;
  } catch {
    /* ignore */
  }
  return { files, urls, html, text };
}

export function isCaptureEmpty(raw: RawCaptureItems): boolean {
  return (
    raw.files.length === 0 &&
    raw.urls.length === 0 &&
    !(raw.text ?? "").trim() &&
    !(raw.html ?? "").trim()
  );
}

export function isCapturePayloadEmpty(payload: OrbCapturePayload): boolean {
  return (
    !payload.text.trim() &&
    payload.urls.length === 0 &&
    payload.filePaths.length === 0 &&
    !payload.html
  );
}

/**
 * Persist files + enrich URLs, then compose the normalized payload. Individual
 * failures surface through `onItemFailed` and never reject the whole capture.
 */
export async function buildCapturePayload(
  raw: RawCaptureItems,
  progress: CaptureProgress
): Promise<OrbCapturePayload> {
  const filePaths: string[] = [];
  const titles = new Map<string, string>();
  const jobs: Array<Promise<void>> = [];

  for (const file of raw.files) {
    jobs.push(
      (async () => {
        try {
          if (file.size > MAX_CAPTURE_BLOB_BYTES) {
            progress.onItemFailed(`跳过 ${file.name || "文件"}（超过 20MB）`);
            return;
          }
          if (!isTauriRuntime()) return;
          const data = await readAsBase64(file);
          const path = await invoke<string>("capture_save_blob", {
            data,
            name: file.name || "capture.bin",
          });
          if (typeof path === "string" && path) filePaths.push(path);
        } catch {
          progress.onItemFailed(`保存 ${file.name || "文件"}失败`);
        } finally {
          progress.onItemDone();
        }
      })()
    );
  }

  for (const url of raw.urls) {
    jobs.push(
      (async () => {
        try {
          if (!isTauriRuntime()) return;
          const info = await invoke<FetchedUrlInfo | null>("capture_fetch_url", { url });
          if (info && typeof info === "object") {
            if (info.kind === "image" && typeof info.path === "string" && info.path) {
              filePaths.push(info.path);
            } else if (
              info.kind === "page" &&
              typeof info.title === "string" &&
              info.title.trim()
            ) {
              titles.set(url, info.title.trim());
            }
          }
        } catch {
          /* enrichment is best-effort — the URL stays as plain text */
        } finally {
          progress.onItemDone();
        }
      })()
    );
  }

  await Promise.all(jobs);

  // Compose text: the plain-text fragment first, then one line per URL with
  // its fetched title appended (skip URLs the text already carries verbatim).
  const base = (raw.text ?? "").trim();
  const lines: string[] = base ? [base] : [];
  for (const url of raw.urls) {
    const title = titles.get(url);
    if (!title && base.includes(url)) continue;
    lines.push(title ? `${url}（标题：${title}）` : url);
  }

  return {
    text: lines.join("\n"),
    urls: raw.urls,
    html: raw.html ?? undefined,
    filePaths,
  };
}

/** `m:ss` elapsed since `startedAt` (minutes keep counting past the hour). */
export function formatElapsed(startedAt: number, now: number): string {
  const total = Math.max(0, Math.floor((now - startedAt) / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

/** Epoch ms → `HH:mm` local clock. */
export function formatClock(at: number): string {
  const d = new Date(at);
  if (Number.isNaN(d.getTime())) return "";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** ISO timestamp → `M/D HH:mm` (same shape usage.ts uses for quota resets). */
export function formatQuotaReset(iso: string | null | undefined): string | null {
  if (!iso) return null;
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return null;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${t.getMonth() + 1}/${t.getDate()} ${pad(t.getHours())}:${pad(t.getMinutes())}`;
}
