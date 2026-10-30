/**
 * Locale store (zh-CN / en): module-level truth + `useSyncExternalStore`
 * subscription, mirroring dockMode.ts. `t()` reads the module state so
 * non-component lib code (errors.ts, suggestions.ts, …) called at event time
 * sees live values, while `useT()` re-renders any component that renders
 * strings — memo boundaries and body-level portals (dock footer) included.
 *
 * The pref follows the theme pattern (`marionette-theme`): localStorage
 * `marionette-language`, try/catch guarded, auto-detected from
 * navigator.language on first run. Node-side verify scripts import lib
 * modules that reach this file — every DOM access stays guarded.
 *
 * Dictionaries live in src/locales/: zh-CN.ts merges the `parts/*.{zh,en}.ts`
 * fragments (one pair per batch/domain; parallel writers never share a file),
 * `en` is checked against zh's key union so a missing translation fails tsc.
 */

import { useSyncExternalStore } from "react";
import { zh } from "../locales/zh-CN";
import { en } from "../locales/en";

export const LOCALES = ["zh-CN", "en"] as const;

export type Locale = (typeof LOCALES)[number];

export const LOCALE_LABELS: Record<Locale, string> = {
  "zh-CN": "简体中文",
  en: "English",
};

export const DEFAULT_LOCALE: Locale = "zh-CN";

export const LANGUAGE_STORAGE_KEY = "marionette-language";

const LOCALE_SET = new Set<string>(LOCALES);

export type DictKey = keyof typeof zh;

type Params = Record<string, string | number | null | undefined>;

function detectLocale(): Locale {
  try {
    const stored = localStorage.getItem(LANGUAGE_STORAGE_KEY);
    if (stored && LOCALE_SET.has(stored)) return stored as Locale;
  } catch {
    /* node or storage denied */
  }
  try {
    if (typeof navigator !== "undefined" && !navigator.language.toLowerCase().startsWith("zh")) {
      return "en";
    }
  } catch {
    /* no navigator */
  }
  return DEFAULT_LOCALE;
}

function applyDocumentLang(locale: Locale): void {
  try {
    document.documentElement.lang = locale;
  } catch {
    /* no document */
  }
}

let currentLocale: Locale = detectLocale();
const listeners = new Set<() => void>();

applyDocumentLang(currentLocale);

export function getLocale(): Locale {
  return currentLocale;
}

export function subscribeLocale(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Switch the UI language: persist, update <html lang>, notify subscribers,
 * and mirror to the Rust side (native dialogs, Telegram bot texts) via the
 * `set_ui_locale` command. No-op outside Tauri (node verify scripts). */
export async function applyLocale(locale: Locale): Promise<void> {
  if (locale === currentLocale) return;
  currentLocale = locale;
  try {
    localStorage.setItem(LANGUAGE_STORAGE_KEY, locale);
  } catch {
    /* storage denied */
  }
  applyDocumentLang(locale);
  for (const notify of listeners) notify();
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("set_ui_locale", { lang: locale });
  } catch {
    /* outside Tauri */
  }
}

export function nextLocale(current: Locale): Locale {
  const index = LOCALES.indexOf(current);
  return LOCALES[(index + 1) % LOCALES.length]!;
}

const DICTS: Record<Locale, Readonly<Record<string, string>>> = {
  "zh-CN": zh,
  en,
};

/**
 * Translate `key` with `{name}` interpolation. With a numeric `count` param,
 * English prefers the ICU-lite `key_one` / `key_other` variants when present
 * (zh has no plurals and always uses the base key). A missing key falls back
 * to zh-CN, then to the key itself — typos stay visible instead of blank.
 */
export function t(key: DictKey, params?: Params): string {
  const dict = DICTS[currentLocale];
  const count = params?.count;
  let text: string | undefined;
  if (count !== undefined && currentLocale !== "zh-CN") {
    text = dict[`${key}_${count === 1 ? "one" : "other"}`];
  }
  text ??= dict[key] ?? zh[key] ?? key;
  if (!params) return text;
  return text.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in params ? String(params[name]!) : match,
  );
}

/** Re-render on language switch (the store owns the truth). */
export function useLocale(): Locale {
  return useSyncExternalStore(
    subscribeLocale,
    getLocale,
    () => DEFAULT_LOCALE,
  );
}

/** `t` bound to re-renders — components that render strings call this once. */
export function useT(): (key: DictKey, params?: Params) => string {
  useLocale();
  return t;
}

/** Tooltip for the sidebar language button, mirroring themeToggleTitle. */
export function localeToggleTitle(locale: Locale): string {
  return t("common.languageToggle", {
    current: LOCALE_LABELS[locale],
    next: LOCALE_LABELS[nextLocale(locale)],
  });
}
