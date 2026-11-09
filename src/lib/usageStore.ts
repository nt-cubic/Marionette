/**
 * usageStore — where the usage ledger and the per-session usage rows live.
 *
 * - Desktop (Tauri): `~/.marionette/usage/` through usage_store.rs — no quota,
 *   optimistic `rev` writes (a stale rev is a return value: reload, re-apply,
 *   retry), and an `usage-ledger-changed` event so every window follows.
 * - Browser preview: localStorage (the pre-disk behaviour; the 5 MB ceiling is
 *   acceptable for a preview). Node scripts: memory.
 *
 * The ledger used to live under the localStorage key below; the first disk
 * load migrates it (usageLedger.ts) and only then removes the key.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauriRuntime } from "./api";

export type StoredLedger = { rev: number; json: string | null };
export type SaveResult = { ok: boolean; rev: number };

export interface LedgerBackend {
  readonly kind: "disk" | "local" | "memory";
  load(): Promise<StoredLedger>;
  save(json: string, baseRev: number): Promise<SaveResult>;
  /** Some window (this one included) saved; called with the new rev. */
  subscribe(onChange: (rev: number) => void): () => void;
  appendSession(sessionId: string, lines: string[]): Promise<void>;
  readSession(sessionId: string): Promise<string[]>;
}

/** The pre-disk home of the ledger (and still the browser-preview one). */
export const LEGACY_LEDGER_KEY = "marionette-usage-ledger";
const LEDGER_CHANGED_EVENT = "usage-ledger-changed";

function diskBackend(): LedgerBackend {
  return {
    kind: "disk",
    async load() {
      const file = await invoke<{ rev: number; json: string | null }>("usage_ledger_load");
      return { rev: file.rev, json: file.json ?? null };
    },
    async save(json, baseRev) {
      return invoke<SaveResult>("usage_ledger_save", { json, baseRev });
    },
    subscribe(onChange) {
      let stop: (() => void) | null = null;
      let cancelled = false;
      void listen<number>(LEDGER_CHANGED_EVENT, (event) => onChange(Number(event.payload) || 0))
        .then((unlisten) => {
          if (cancelled) unlisten();
          else stop = unlisten;
        })
        .catch(() => {
          /* single-window semantics still hold */
        });
      return () => {
        cancelled = true;
        stop?.();
      };
    },
    async appendSession(sessionId, lines) {
      if (lines.length === 0) return;
      await invoke("usage_session_append", { sessionId, lines });
    },
    async readSession(sessionId) {
      return invoke<string[]>("usage_session_read", { sessionId });
    },
  };
}

/** In-memory session rows for the non-disk backends (preview / scripts). */
function memorySessions(): Pick<LedgerBackend, "appendSession" | "readSession"> {
  const rows = new Map<string, string[]>();
  return {
    async appendSession(sessionId, lines) {
      const list = rows.get(sessionId) ?? [];
      list.push(...lines);
      rows.set(sessionId, list.slice(-4000));
    },
    async readSession(sessionId) {
      return [...(rows.get(sessionId) ?? [])];
    },
  };
}

function localBackend(): LedgerBackend {
  let rev = 1;
  return {
    kind: "local",
    async load() {
      let json: string | null = null;
      try {
        json = window.localStorage.getItem(LEGACY_LEDGER_KEY);
      } catch {
        json = null;
      }
      return { rev, json };
    },
    async save(json, baseRev) {
      if (baseRev !== rev) return { ok: false, rev };
      // Throws on quota: the ledger logs it and retries instead of going silent.
      window.localStorage.setItem(LEGACY_LEDGER_KEY, json);
      rev += 1;
      return { ok: true, rev };
    },
    subscribe(onChange) {
      const handler = (event: StorageEvent) => {
        if (event.key !== LEGACY_LEDGER_KEY) return;
        rev += 1;
        onChange(rev);
      };
      try {
        window.addEventListener("storage", handler);
      } catch {
        return () => {};
      }
      return () => window.removeEventListener("storage", handler);
    },
    ...memorySessions(),
  };
}

export function memoryBackend(initial: string | null = null): LedgerBackend {
  let rev = 1;
  let json = initial;
  const listeners = new Set<(rev: number) => void>();
  return {
    kind: "memory",
    async load() {
      return { rev, json };
    },
    async save(next, baseRev) {
      if (baseRev !== rev) return { ok: false, rev };
      json = next;
      rev += 1;
      for (const listener of listeners) listener(rev);
      return { ok: true, rev };
    },
    subscribe(onChange) {
      listeners.add(onChange);
      return () => listeners.delete(onChange);
    },
    ...memorySessions(),
  };
}

export function pickBackend(): LedgerBackend {
  if (isTauriRuntime()) return diskBackend();
  if (typeof window !== "undefined") {
    try {
      if (window.localStorage) return localBackend();
    } catch {
      /* storage denied */
    }
  }
  return memoryBackend();
}

/** The localStorage ledger left by builds before the disk move (desktop only). */
export function readLegacyLocalLedger(): string | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage.getItem(LEGACY_LEDGER_KEY);
  } catch {
    return null;
  }
}

export function clearLegacyLocalLedger(): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(LEGACY_LEDGER_KEY);
  } catch {
    /* next start retries */
  }
}
