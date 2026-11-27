// P1-10: the capture-window shortcut is a preference, not an App-level
// constant. The dialog owns the recorder; App only reads the descriptor and
// asks `shortcutMatchesEvent` (or listens to CAPTURE_SHORTCUT_EVENT).

export type ShortcutModifier = "Ctrl" | "Alt" | "Shift" | "Meta";

export type ShortcutDescriptor = {
  modifiers: ShortcutModifier[];
  /** Normalized lower-case key (`w`, `f8`, `pageup`, `space`). */
  key: string;
};

export type ShortcutActionLabelKey =
  | "capture.conflict.palette"
  | "capture.conflict.closeTab"
  | "capture.conflict.newChat"
  | "capture.conflict.nextTab"
  | "capture.conflict.prevTab"
  | "capture.conflict.reopenTab"
  | "capture.conflict.tabSwitch"
  | "capture.conflict.pasteInline"
  | "capture.conflict.computerStop"
  | "capture.conflict.find";

export type ShortcutAction = {
  id: string;
  labelKey: ShortcutActionLabelKey;
  shortcut: ShortcutDescriptor;
};

export type ShortcutConflict = {
  actionId: string;
  labelKey: ShortcutActionLabelKey;
  /** Human-readable chord of the action already using it. */
  label: string;
  shortcut: ShortcutDescriptor;
};

export type ShortcutValidation =
  | {
      ok: true;
      descriptor: ShortcutDescriptor;
      label: string;
      conflict: null;
    }
  | {
      ok: false;
      reason: "empty" | "needs-modifier" | "reserved" | "conflict";
      descriptor: ShortcutDescriptor;
      label: string;
      conflict: ShortcutConflict | null;
    };

export const CAPTURE_SHORTCUT_STORAGE_KEY = "marionette-capture-window-shortcut";
export const CAPTURE_SHORTCUT_EVENT = "marionette-capture-window-shortcut";

/** Ctrl+Alt+Shift+W: no App chord today, and hard to hit by accident. */
export const DEFAULT_CAPTURE_SHORTCUT: ShortcutDescriptor = {
  modifiers: ["Ctrl", "Alt", "Shift"],
  key: "w",
};

/**
 * Chords Marionette already owns. App may pass its own live registry; this
 * list is the honest default for the recorder and conflict preview.
 */
export const DEFAULT_APP_SHORTCUTS: readonly ShortcutAction[] = [
  { id: "palette", labelKey: "capture.conflict.palette", shortcut: { modifiers: ["Ctrl"], key: "k" } },
  { id: "close-tab", labelKey: "capture.conflict.closeTab", shortcut: { modifiers: ["Ctrl"], key: "w" } },
  { id: "new-chat", labelKey: "capture.conflict.newChat", shortcut: { modifiers: ["Ctrl"], key: "t" } },
  { id: "next-tab", labelKey: "capture.conflict.nextTab", shortcut: { modifiers: ["Ctrl"], key: "tab" } },
  {
    id: "prev-tab",
    labelKey: "capture.conflict.prevTab",
    shortcut: { modifiers: ["Ctrl", "Shift"], key: "tab" },
  },
  {
    id: "reopen-tab",
    labelKey: "capture.conflict.reopenTab",
    shortcut: { modifiers: ["Ctrl", "Shift"], key: "t" },
  },
  { id: "paste-inline", labelKey: "capture.conflict.pasteInline", shortcut: { modifiers: ["Ctrl", "Shift"], key: "v" } },
  {
    id: "computer-stop",
    labelKey: "capture.conflict.computerStop",
    shortcut: { modifiers: ["Ctrl", "Alt", "Shift"], key: "s" },
  },
  { id: "find", labelKey: "capture.conflict.find", shortcut: { modifiers: ["Ctrl"], key: "f" } },
  {
    id: "find-advanced",
    labelKey: "capture.conflict.find",
    shortcut: { modifiers: ["Ctrl", "Shift"], key: "f" },
  },
  ...Array.from({ length: 9 }, (_, index): ShortcutAction => ({
    id: `tab-${index + 1}`,
    labelKey: "capture.conflict.tabSwitch",
    shortcut: { modifiers: ["Ctrl"], key: String(index + 1) },
  })),
];

const MODIFIER_ORDER: readonly ShortcutModifier[] = ["Ctrl", "Alt", "Shift", "Meta"];

const KEY_ALIASES: Record<string, string> = {
  " ": "space",
  spacebar: "space",
  esc: "escape",
  del: "delete",
  ins: "insert",
  return: "enter",
  pgup: "pageup",
  pgdn: "pagedown",
  arrowup: "up",
  arrowdown: "down",
  arrowleft: "left",
  arrowright: "right",
};

const KEY_LABELS: Record<string, string> = {
  space: "Space",
  escape: "Esc",
  delete: "Del",
  insert: "Ins",
  pageup: "PageUp",
  pagedown: "PageDown",
  up: "Up",
  down: "Down",
  left: "Left",
  right: "Right",
};

function normalizeModifier(raw: string): ShortcutModifier | null {
  const value = raw.trim().toLowerCase();
  if (value === "ctrl" || value === "control" || value === "cmdorctrl" || value === "commandorcontrol") {
    return "Ctrl";
  }
  if (value === "alt" || value === "option") return "Alt";
  if (value === "shift") return "Shift";
  if (value === "meta" || value === "win" || value === "cmd" || value === "command" || value === "super") {
    return "Meta";
  }
  return null;
}

export function normalizeShortcutModifiers(modifiers: readonly string[]): ShortcutModifier[] {
  const set = new Set<ShortcutModifier>();
  for (const modifier of modifiers) {
    const normalized = normalizeModifier(modifier);
    if (normalized) set.add(normalized);
  }
  return MODIFIER_ORDER.filter((modifier) => set.has(modifier));
}

export function normalizeShortcutKey(key: string): string {
  const raw = key === " " ? "space" : key.trim();
  if (!raw) return "";
  const alias = KEY_ALIASES[raw.toLowerCase()];
  if (alias) return alias;
  if (raw.length === 1) return raw.toLowerCase();
  return raw.toLowerCase();
}

export function normalizeShortcutDescriptor(descriptor: ShortcutDescriptor): ShortcutDescriptor {
  return {
    modifiers: normalizeShortcutModifiers(descriptor.modifiers),
    key: normalizeShortcutKey(descriptor.key),
  };
}

function displayKey(key: string): string {
  const label = KEY_LABELS[key];
  if (label) return label;
  if (key.length === 1) return key.toUpperCase();
  if (/^f([1-9]|1[0-9]|2[0-4])$/.test(key)) return key.toUpperCase();
  return key.charAt(0).toUpperCase() + key.slice(1);
}

export function describeShortcut(descriptor: ShortcutDescriptor): string {
  const normalized = normalizeShortcutDescriptor(descriptor);
  if (!normalized.key) return "-";
  return [...normalized.modifiers, displayKey(normalized.key)].join("+");
}

export function shortcutEquals(a: ShortcutDescriptor, b: ShortcutDescriptor): boolean {
  const left = normalizeShortcutDescriptor(a);
  const right = normalizeShortcutDescriptor(b);
  return (
    left.key === right.key &&
    left.modifiers.length === right.modifiers.length &&
    left.modifiers.every((modifier, index) => modifier === right.modifiers[index])
  );
}

/** Null while the user is still holding only modifiers (or a dead key). */
export function shortcutFromKeyboardEvent(event: KeyboardEvent): ShortcutDescriptor | null {
  const key = event.key;
  if (!key || key === "Unidentified" || key === "Dead") return null;
  if (key === "Control" || key === "Alt" || key === "Shift" || key === "Meta") return null;
  return {
    modifiers: normalizeShortcutModifiers([
      ...(event.ctrlKey ? ["Ctrl"] : []),
      ...(event.altKey ? ["Alt"] : []),
      ...(event.shiftKey ? ["Shift"] : []),
      ...(event.metaKey ? ["Meta"] : []),
    ]),
    key: normalizeShortcutKey(key),
  };
}

export function shortcutMatchesEvent(descriptor: ShortcutDescriptor, event: KeyboardEvent): boolean {
  const actual = shortcutFromKeyboardEvent(event);
  return actual !== null && shortcutEquals(descriptor, actual);
}

function reservedReason(descriptor: ShortcutDescriptor): string | null {
  const { modifiers, key } = descriptor;
  if (modifiers.includes("Meta")) return "windows-key";
  if (modifiers.includes("Alt") && (key === "tab" || key === "f4" || key === "escape" || key === "space")) {
    return "system";
  }
  if (modifiers.length === 2 && modifiers.includes("Ctrl") && modifiers.includes("Alt") && key === "delete") {
    return "system";
  }
  if (modifiers.length === 1 && modifiers.includes("Ctrl") && key === "escape") return "system";
  if (
    modifiers.length === 2 &&
    modifiers.includes("Ctrl") &&
    modifiers.includes("Shift") &&
    key === "escape"
  ) {
    return "system";
  }
  return null;
}

export function findShortcutConflict(
  descriptor: ShortcutDescriptor,
  registry: readonly ShortcutAction[] = DEFAULT_APP_SHORTCUTS,
): ShortcutConflict | null {
  const normalized = normalizeShortcutDescriptor(descriptor);
  for (const action of registry) {
    if (shortcutEquals(normalized, action.shortcut)) {
      return {
        actionId: action.id,
        labelKey: action.labelKey,
        label: describeShortcut(action.shortcut),
        shortcut: normalizeShortcutDescriptor(action.shortcut),
      };
    }
  }
  return null;
}

export function validateCaptureShortcut(
  descriptor: ShortcutDescriptor,
  registry: readonly ShortcutAction[] = DEFAULT_APP_SHORTCUTS,
): ShortcutValidation {
  const normalized = normalizeShortcutDescriptor(descriptor);
  const label = describeShortcut(normalized);
  if (!normalized.key) {
    return { ok: false, reason: "empty", descriptor: normalized, label, conflict: null };
  }
  if (normalized.modifiers.length === 0) {
    return { ok: false, reason: "needs-modifier", descriptor: normalized, label, conflict: null };
  }
  if (reservedReason(normalized)) {
    return { ok: false, reason: "reserved", descriptor: normalized, label, conflict: null };
  }
  const conflict = findShortcutConflict(normalized, registry);
  if (conflict) {
    return { ok: false, reason: "conflict", descriptor: normalized, label, conflict };
  }
  return { ok: true, descriptor: normalized, label, conflict: null };
}

function readStored(): ShortcutDescriptor | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(CAPTURE_SHORTCUT_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as { modifiers?: unknown; key?: unknown };
    if (!Array.isArray(parsed.modifiers) || typeof parsed.key !== "string") return null;
    const descriptor = normalizeShortcutDescriptor({
      modifiers: normalizeShortcutModifiers(
        parsed.modifiers.filter((value): value is string => typeof value === "string"),
      ),
      key: parsed.key,
    });
    return descriptor.key ? descriptor : null;
  } catch {
    return null;
  }
}

export function readCaptureShortcut(): ShortcutDescriptor {
  return readStored() ?? normalizeShortcutDescriptor(DEFAULT_CAPTURE_SHORTCUT);
}

function emitShortcutChanged(descriptor: ShortcutDescriptor): void {
  if (typeof window === "undefined" || typeof CustomEvent === "undefined") return;
  window.dispatchEvent(new CustomEvent<ShortcutDescriptor>(CAPTURE_SHORTCUT_EVENT, { detail: descriptor }));
}

/**
 * Persists only a structurally valid, non-reserved, non-conflicting chord.
 * The returned validation is what the recorder shows when it refuses.
 */
export function writeCaptureShortcut(
  descriptor: ShortcutDescriptor,
  options: { registry?: readonly ShortcutAction[]; allowConflict?: boolean } = {},
): ShortcutValidation {
  const validation = validateCaptureShortcut(descriptor, options.registry ?? DEFAULT_APP_SHORTCUTS);
  if (!validation.ok && !(options.allowConflict && validation.reason === "conflict")) {
    return validation;
  }
  if (typeof window === "undefined") return validation;
  try {
    window.localStorage.setItem(CAPTURE_SHORTCUT_STORAGE_KEY, JSON.stringify(validation.descriptor));
  } catch {
    // Private mode / storage disabled: the descriptor still works this run.
  }
  emitShortcutChanged(validation.descriptor);
  return validation;
}

export function resetCaptureShortcut(options: { registry?: readonly ShortcutAction[] } = {}): ShortcutValidation {
  return writeCaptureShortcut(DEFAULT_CAPTURE_SHORTCUT, options);
}

export function subscribeCaptureShortcut(listener: (descriptor: ShortcutDescriptor) => void): () => void {
  if (typeof window === "undefined") return () => {};
  const handler = () => listener(readCaptureShortcut());
  window.addEventListener(CAPTURE_SHORTCUT_EVENT, handler);
  return () => window.removeEventListener(CAPTURE_SHORTCUT_EVENT, handler);
}

/** One-line App wiring: the stored chord matches this keydown. */
export function captureShortcutMatchesEvent(event: KeyboardEvent): boolean {
  return shortcutMatchesEvent(readCaptureShortcut(), event);
}
