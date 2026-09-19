/** Themes kept in the app: 浅夜 (dim, the default) and 纯白 (white). */
export const THEME_MODES = ["dim", "white"] as const;

export type ThemeMode = (typeof THEME_MODES)[number];

export const THEME_LABELS: Record<ThemeMode, string> = {
  dim: "浅夜",
  white: "纯白",
};

export const DEFAULT_THEME: ThemeMode = "dim";

const THEME_SET = new Set<string>(THEME_MODES);

/** Retired palettes (dark / light) resolve to their closest survivor. */
const RETIRED_THEMES: Record<string, ThemeMode> = {
  dark: "dim",
  light: "white",
};

export function parseStoredTheme(raw: string | null | undefined): ThemeMode {
  if (raw && THEME_SET.has(raw)) return raw as ThemeMode;
  if (raw && RETIRED_THEMES[raw]) return RETIRED_THEMES[raw];
  return DEFAULT_THEME;
}

export function nextTheme(current: ThemeMode): ThemeMode {
  const index = THEME_MODES.indexOf(current);
  return THEME_MODES[(index + 1) % THEME_MODES.length];
}

export function themeColorScheme(theme: ThemeMode): "dark" | "light" {
  return theme === "white" ? "light" : "dark";
}

export function themeToggleTitle(theme: ThemeMode): string {
  return `主题：${THEME_LABELS[theme]}（点击切换到${THEME_LABELS[nextTheme(theme)]}）`;
}
