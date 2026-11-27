/**
 * Layout state of the Token usage page: which of the three Cost / Tokens / Limits
 * views is open, and whether the detailed-analysis layer is expanded. Both live
 * in localStorage so a reopen keeps the user's choice; malformed or absent values
 * fall back to the first-screen defaults (Cost, collapsed).
 */

export type UsageView = "cost" | "tokens" | "limits";

const VIEW_KEY = "marionette-usage-view";
const MORE_KEY = "marionette-usage-more-open";

function storage(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

export function readUsageView(): UsageView {
  try {
    const raw = storage()?.getItem(VIEW_KEY);
    if (raw === "cost" || raw === "tokens" || raw === "limits") return raw;
  } catch {
    /* private mode / quota errors: keep the default */
  }
  return "cost";
}

export function storeUsageView(view: UsageView): void {
  try {
    storage()?.setItem(VIEW_KEY, view);
  } catch {
    /* the tab still works for this mount */
  }
}

export function readMoreOpen(): boolean {
  try {
    return storage()?.getItem(MORE_KEY) === "1";
  } catch {
    return false;
  }
}

export function storeMoreOpen(open: boolean): void {
  try {
    storage()?.setItem(MORE_KEY, open ? "1" : "0");
  } catch {
    /* the section still works for this mount */
  }
}
