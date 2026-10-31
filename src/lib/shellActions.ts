/**
 * App-wide "open this shell surface" pokes. The command palette and the
 * footer settings button dispatch these; ProjectShelf (where the Telegram /
 * model-share popovers live) listens. Existing footer buttons stay.
 */

export const OPEN_TELEGRAM_EVENT = "marionette:open-telegram";
export const OPEN_ENGINE_EVENT = "marionette:open-engine";

export function requestOpenTelegram(): void {
  window.dispatchEvent(new Event(OPEN_TELEGRAM_EVENT));
}

export function requestOpenEngine(): void {
  window.dispatchEvent(new Event(OPEN_ENGINE_EVENT));
}
