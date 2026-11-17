export const BROWSER_IDLE_STORAGE_KEY = "marionette-browser-agent-tab-idle-minutes";

export type BrowserIdleMinutes = 0 | 10 | 30 | 60;

const IDLE_OPTIONS: readonly BrowserIdleMinutes[] = [10, 30, 60, 0];

export function parseBrowserIdleMinutes(value: unknown): BrowserIdleMinutes {
  return value === 0 || value === 10 || value === 30 || value === 60 ? value : 10;
}

export function readBrowserIdleMinutes(): BrowserIdleMinutes {
  try {
    const raw = localStorage.getItem(BROWSER_IDLE_STORAGE_KEY);
    return raw == null ? 10 : parseBrowserIdleMinutes(Number(raw));
  } catch {
    return 10;
  }
}

export function saveBrowserIdleMinutes(value: BrowserIdleMinutes): void {
  try {
    localStorage.setItem(BROWSER_IDLE_STORAGE_KEY, String(value));
  } catch {
    // Storage can be unavailable; the next command still uses the value read above.
  }
}

export function nextBrowserIdleMinutes(
  current: BrowserIdleMinutes,
): BrowserIdleMinutes {
  const index = IDLE_OPTIONS.indexOf(current);
  return IDLE_OPTIONS[(index + 1) % IDLE_OPTIONS.length]!;
}
