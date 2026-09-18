export const THEME_MODES = ["dark", "dim", "light", "white"] as const;

export type ThemeMode = (typeof THEME_MODES)[number];

export const THEME_LABELS: Record<ThemeMode, string> = {
  dark: "深色",
  dim: "浅夜",
  light: "浅色",
  white: "纯白",
};

const THEME_SET = new Set<string>(THEME_MODES);

export function parseStoredTheme(raw: string | null | undefined): ThemeMode {
  if (raw && THEME_SET.has(raw)) return raw as ThemeMode;
  return "dark";
}

export function nextTheme(current: ThemeMode): ThemeMode {
  const index = THEME_MODES.indexOf(current);
  return THEME_MODES[(index + 1) % THEME_MODES.length];
}

export function themeColorScheme(theme: ThemeMode): "dark" | "light" {
  return theme === "light" || theme === "white" ? "light" : "dark";
}

export function themeToggleTitle(theme: ThemeMode): string {
  return `主题：${THEME_LABELS[theme]}（点击切换到${THEME_LABELS[nextTheme(theme)]}）`;
}
