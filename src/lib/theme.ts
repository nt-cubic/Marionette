import { t } from "./i18n";

/** Themes kept in the app: 浅夜 (dim, the default) and 纯白 (white). */
export const THEME_MODES = ["dim", "white"] as const;

export type ThemeMode = (typeof THEME_MODES)[number];

/** Localized theme name (see locales/parts/theme.*). */
export function themeLabel(theme: ThemeMode): string {
  return t(`theme.${theme}` as const);
}

export const DEFAULT_THEME: ThemeMode = "dim";

const THEME_SET = new Set<string>(THEME_MODES);

/** Retired palettes (dark / light) resolve to their closest survivor.
 * index.html 的首屏预载脚本镜像了这份映射——改动必须两边同步。 */
const RETIRED_THEMES: Record<string, ThemeMode> = {
  dark: "dim",
  light: "white",
};

export function parseStoredTheme(raw: string | null | undefined): ThemeMode {
  if (raw && THEME_SET.has(raw)) return raw as ThemeMode;
  if (raw && RETIRED_THEMES[raw]) return RETIRED_THEMES[raw]!;
  return DEFAULT_THEME;
}

export function nextTheme(current: ThemeMode): ThemeMode {
  const index = THEME_MODES.indexOf(current);
  return THEME_MODES[(index + 1) % THEME_MODES.length]!;
}

export function themeColorScheme(theme: ThemeMode): "dark" | "light" {
  return theme === "white" ? "light" : "dark";
}

export function themeToggleTitle(theme: ThemeMode): string {
  return t("theme.toggleTitle", {
    current: themeLabel(theme),
    next: themeLabel(nextTheme(theme)),
  });
}
