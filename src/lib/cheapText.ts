import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

/**
 * Cheap text generation (P1-13) — frontend boundary.
 *
 * This module is deliberately independent from agent sessions and the engine
 * gateway. It mirrors the Rust command contract in `src-tauri/src/cheap_text.rs`:
 *
 * - `cheap_text_settings_get` / `cheap_text_settings_set` — one low-cost source +
 *   model, optional title style, max tokens and timeout.
 * - `cheap_text_catalog` — configured sources and their models, with key
 *   readiness. Read-only; never starts the engine.
 * - `cheap_text_generate` — synchronous title / branch / commit copy.
 * - `cheap_text_generate_title` — fire-and-forget automatic session title; the
 *   backend emits `marionette-cheap-title` only on success.
 *
 * Every wrapper is fail-open: missing runtime, missing source or a failed HTTP
 * call yields `null` (or defaults) so callers keep their own draft/first-message
 * title. No cheap-text usage is written into the agent usage ledger; the Rust
 * side appends to its own `cheap-text/usage.jsonl`.
 */

export type CheapTextKind = "title" | "branch" | "commit";

/** Draft shape sent to `cheap_text_settings_set`. */
export type CheapTextSettingsDraft = {
  enabled: boolean;
  /** Provider id in the engine source store, or an encrypted credential id. */
  source: string;
  /** Model id exactly as the source spells it. */
  model: string;
  /** Extra instructions for session titles only. */
  titleStyle: string;
  maxTokens: number;
  timeoutMs: number;
};

/** Redacted settings view returned by the backend. */
export type CheapTextSettings = CheapTextSettingsDraft & {
  /** Both source and model are present. */
  configured: boolean;
  /** A key/endpoint resolves right now (read-only, no network). */
  credentialReady: boolean;
};

/** One selectable source in the settings dialog. */
export type CheapTextSource = {
  id: string;
  label: string;
  /** "chat" | "anthropic" | "none" (not usable for cheap text). */
  protocol: string;
  models: string[];
  hasKey: boolean;
};

export type CheapTextSaveResult =
  | { ok: true; settings: CheapTextSettings }
  | { ok: false; error: string };

export const CHEAP_TEXT_TITLE_EVENT = "marionette-cheap-title";

export const CHEAP_TEXT_MIN_MAX_TOKENS = 16;
export const CHEAP_TEXT_MAX_TOKENS = 512;
export const CHEAP_TEXT_DEFAULT_MAX_TOKENS = 96;
export const CHEAP_TEXT_MIN_TIMEOUT_MS = 1_500;
export const CHEAP_TEXT_MAX_TIMEOUT_MS = 30_000;
export const CHEAP_TEXT_DEFAULT_TIMEOUT_MS = 8_000;
export const CHEAP_TEXT_TITLE_STYLE_LIMIT = 400;

export const DEFAULT_CHEAP_TEXT_SETTINGS: CheapTextSettingsDraft = {
  enabled: true,
  source: "",
  model: "",
  titleStyle: "",
  maxTokens: CHEAP_TEXT_DEFAULT_MAX_TOKENS,
  timeoutMs: CHEAP_TEXT_DEFAULT_TIMEOUT_MS,
};

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function finiteNumber(value: unknown, fallback: number): number {
  const parsed = typeof value === "number" ? value : Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/**
 * Mirror `cheap_text.rs::CheapTextConfig::normalized` so the dialog and the
 * backend agree on defaults, ranges and the 400-character style cap.
 */
export function normalizeCheapTextDraft(
  input: Partial<CheapTextSettingsDraft> | null | undefined,
): CheapTextSettingsDraft {
  const source = typeof input?.source === "string" ? input.source.trim() : "";
  const model = typeof input?.model === "string" ? input.model.trim() : "";
  const titleStyle =
    typeof input?.titleStyle === "string"
      ? input.titleStyle.trim().slice(0, CHEAP_TEXT_TITLE_STYLE_LIMIT)
      : "";

  const rawMax = Math.trunc(
    finiteNumber(input?.maxTokens, CHEAP_TEXT_DEFAULT_MAX_TOKENS),
  );
  const maxTokens =
    rawMax === 0
      ? CHEAP_TEXT_DEFAULT_MAX_TOKENS
      : clamp(rawMax, CHEAP_TEXT_MIN_MAX_TOKENS, CHEAP_TEXT_MAX_TOKENS);

  const rawTimeout = Math.trunc(
    finiteNumber(input?.timeoutMs, CHEAP_TEXT_DEFAULT_TIMEOUT_MS),
  );
  const timeoutMs =
    rawTimeout === 0
      ? CHEAP_TEXT_DEFAULT_TIMEOUT_MS
      : clamp(rawTimeout, CHEAP_TEXT_MIN_TIMEOUT_MS, CHEAP_TEXT_MAX_TIMEOUT_MS);

  return {
    enabled: input?.enabled !== false,
    source,
    model,
    titleStyle,
    maxTokens,
    timeoutMs,
  };
}

export function cheapTextConfigured(
  draft: Pick<CheapTextSettingsDraft, "source" | "model">,
): boolean {
  return draft.source.trim().length > 0 && draft.model.trim().length > 0;
}

export function cheapTextViewToDraft(
  view: Partial<CheapTextSettings> | null | undefined,
): CheapTextSettingsDraft {
  return normalizeCheapTextDraft(view);
}

function normalizeSettingsView(
  raw: Partial<CheapTextSettings> | null | undefined,
): CheapTextSettings {
  const draft = normalizeCheapTextDraft(raw);
  return {
    ...draft,
    configured:
      typeof raw?.configured === "boolean"
        ? raw.configured
        : cheapTextConfigured(draft),
    credentialReady: raw?.credentialReady === true,
  };
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === "string" ? error : String(error);
}

function previewSettings(): CheapTextSettings {
  return {
    ...DEFAULT_CHEAP_TEXT_SETTINGS,
    configured: false,
    credentialReady: false,
  };
}

/** Settings for the dialog. Reads local files only; never wakes the engine. */
export async function getCheapTextSettings(): Promise<CheapTextSettings> {
  if (!isTauriRuntime()) return previewSettings();
  try {
    const raw = await invoke<Partial<CheapTextSettings>>("cheap_text_settings_get");
    return normalizeSettingsView(raw);
  } catch {
    return previewSettings();
  }
}

/**
 * Persist settings. Returns a result instead of throwing so the dialog can keep
 * the draft on a transient disk error. Unknown fields are ignored by serde.
 */
export async function saveCheapTextSettings(
  draft: CheapTextSettingsDraft,
): Promise<CheapTextSaveResult> {
  const normalized = normalizeCheapTextDraft(draft);
  if (!isTauriRuntime()) {
    return {
      ok: true,
      settings: {
        ...normalized,
        configured: cheapTextConfigured(normalized),
        credentialReady: false,
      },
    };
  }
  try {
    const raw = await invoke<Partial<CheapTextSettings>>("cheap_text_settings_set", {
      settings: normalized,
    });
    return { ok: true, settings: normalizeSettingsView(raw) };
  } catch (error) {
    return { ok: false, error: errorMessage(error) };
  }
}

function toSource(value: unknown): CheapTextSource | null {
  if (!value || typeof value !== "object") return null;
  const row = value as Partial<CheapTextSource>;
  const id = typeof row.id === "string" ? row.id.trim() : "";
  if (!id) return null;
  const protocol = typeof row.protocol === "string" ? row.protocol : "none";
  const models = Array.isArray(row.models)
    ? row.models
        .filter((model): model is string => typeof model === "string")
        .map((model) => model.trim())
        .filter(Boolean)
    : [];
  return {
    id,
    label: typeof row.label === "string" && row.label.trim() ? row.label : id,
    protocol,
    models,
    hasKey: row.hasKey === true,
  };
}

/**
 * Sources usable for cheap text. Read-only catalogue peek (pinned rows plus
 * encrypted manual sources); empty on failure so the dialog stays usable.
 */
export async function listCheapTextSources(): Promise<CheapTextSource[]> {
  if (!isTauriRuntime()) return [];
  try {
    const rows = await invoke<unknown[]>("cheap_text_catalog");
    if (!Array.isArray(rows)) return [];
    return rows
      .map(toSource)
      .filter((row): row is CheapTextSource => row !== null);
  } catch {
    return [];
  }
}

/**
 * Synchronous auxiliary copy. `null` means "keep your own draft" — no key,
 * disabled config, network failure or unusable output all collapse to null.
 */
export async function generateCheapText(
  kind: CheapTextKind,
  text: string,
  style?: string | null,
): Promise<string | null> {
  if (!isTauriRuntime()) return null;
  const trimmed = text.trim();
  if (!trimmed) return null;
  try {
    const value = await invoke<string | null>("cheap_text_generate", {
      kind,
      text: trimmed,
      style: typeof style === "string" && style.trim() ? style.trim() : null,
    });
    return typeof value === "string" && value.trim() ? value.trim() : null;
  } catch {
    return null;
  }
}

/** Reusable title request (synchronous; UI keeps the draft on null). */
export function requestCheapTitle(
  text: string,
  style?: string | null,
): Promise<string | null> {
  return generateCheapText("title", text, style);
}

/** Reusable branch-name request; callers keep their own draft on null. */
export function requestCheapBranchName(text: string): Promise<string | null> {
  return generateCheapText("branch", text);
}

/** Reusable commit-message request; callers keep their own draft on null. */
export function requestCheapCommitMessage(text: string): Promise<string | null> {
  return generateCheapText("commit", text);
}

/**
 * Fire-and-forget automatic session title. The backend generates on its own
 * thread and emits `marionette-cheap-title` only on success, so sending is never
 * blocked and a missing config simply leaves the first-message title in place.
 */
export async function requestCheapSessionTitle(
  sessionId: string,
  text: string,
  style?: string | null,
): Promise<void> {
  if (!isTauriRuntime()) return;
  if (!sessionId.trim() || !text.trim()) return;
  try {
    await invoke("cheap_text_generate_title", {
      sessionId,
      text,
      style: typeof style === "string" && style.trim() ? style.trim() : null,
    });
  } catch {
    // Silent by design: the existing label stays.
  }
}

export type CheapTitlePayload = { sessionId: string; title: string };

/**
 * Subscribe to semantic title events. Returns a no-op unsubscribe outside the
 * Tauri runtime so callers do not need their own capability checks.
 */
export async function subscribeCheapTitle(
  handler: (payload: CheapTitlePayload) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) return () => undefined;
  try {
    const unlisten = await listen<{ sessionId?: string; title?: string }>(
      CHEAP_TEXT_TITLE_EVENT,
      (event) => {
        const sessionId = event.payload?.sessionId?.trim();
        const title = event.payload?.title?.trim();
        if (sessionId && title) handler({ sessionId, title });
      },
    );
    return unlisten;
  } catch {
    return () => undefined;
  }
}
