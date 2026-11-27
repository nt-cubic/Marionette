/**
 * P1-9 - F6 / Shift+F6 workspace-area focus.
 *
 * Design constraints:
 * - Only F6 is handled. Tab/Shift+Tab are never intercepted, so focus is never
 *   trapped inside an area.
 * - Hidden areas are skipped: zero-size panels, `display:none`,
 *   `visibility:hidden`, `aria-hidden="true"`, `inert`, off-screen overlays.
 * - No modal is open when cycling (configurable). F6 inside a dialog does not
 *   pull focus out to the page behind it.
 * - `useOverlayFocusRestore` remembers the control that opened a palette/menu
 *   and restores it when the overlay closes (or unmounts).
 *
 * Default areas need no App.tsx edits: the helper falls back to the three
 * existing workspace columns (`.left-rail`, `.center-workspace`,
 * `.context-panel`). Add `data-workspace-area="..."` attributes later to make
 * the set explicit, including extra panes.
 */

import { useEffect, useRef } from "react";

export const WORKSPACE_AREA_ATTRIBUTE = "data-workspace-area";
export const WORKSPACE_AREA_FOCUS_ATTRIBUTE = "data-workspace-area-focus";

/** Zero-edit fallback: the three current workspace columns. */
export const FALLBACK_WORKSPACE_AREA_SELECTOR =
  ".left-rail, .center-workspace, .context-panel";

/**
 * Default discovery selector: explicit attributes plus the three existing
 * columns. `querySelectorAll` de-duplicates elements that match both, so
 * adding `data-workspace-area` to one column does not drop the other two.
 * Pass `selector: "[data-workspace-area]"` for an explicit-only set.
 */
export const DEFAULT_WORKSPACE_AREA_SELECTOR =
  `[${WORKSPACE_AREA_ATTRIBUTE}], ${FALLBACK_WORKSPACE_AREA_SELECTOR}`;

export type WorkspaceArea = {
  id: string;
  element: HTMLElement;
};

export type AreaFocusMove = {
  moved: boolean;
  from: string | null;
  to: string | null;
  reason?:
    | "no-document"
    | "no-areas"
    | "single-area"
    | "overlay-open"
    | "focus-failed";
};

export type AreaFocusOptions = {
  /** Search root. Defaults to `document`. */
  root?: ParentNode | null;
  /**
   * Area selector. When omitted, explicit `[data-workspace-area]` elements and
   * the three existing workspace columns are combined (de-duplicated). Pass an
   * explicit selector for a custom set.
   */
  selector?: string;
  /** Optional id order. Ids not listed keep DOM order after listed ones. */
  order?: readonly string[];
  /**
   * Don't cycle while a modal/overlay is open. Defaults to
   * `[aria-modal="true"]`. Pass `null` to disable, or a broader selector such
   * as `[aria-modal="true"], .composer-menu, .dock-menu` to include menus.
   */
  overlaySelector?: string | null;
  /**
   * Optional focus target inside each area. Defaults to
   * `[data-workspace-area-focus]`; without one the area container itself gets
   * `tabindex="-1"` for the duration of the focus.
   */
  targetSelector?: string;
  /** Called after every attempt, including no-op attempts. */
  onMove?: (move: AreaFocusMove) => void;
  /** `false` disables the installed listener. Defaults to enabled. */
  enabled?: boolean;
};

function isHtmlElement(value: unknown): value is HTMLElement {
  return typeof HTMLElement !== "undefined" && value instanceof HTMLElement;
}

function hasHiddenAncestor(element: HTMLElement): boolean {
  let node: HTMLElement | null = element;
  while (node) {
    if (node.hasAttribute("hidden") || node.hasAttribute("inert")) return true;
    if (node.getAttribute("aria-hidden") === "true") return true;
    const style = window.getComputedStyle(node);
    if (style.display === "none") return true;
    if (style.visibility === "hidden" || style.visibility === "collapse") {
      return true;
    }
    if (style.getPropertyValue("content-visibility") === "hidden") return true;
    node = node.parentElement;
  }
  return false;
}

type CheckVisibilityCapable = HTMLElement & {
  checkVisibility?: (options?: {
    checkOpacity?: boolean;
    checkVisibilityCSS?: boolean;
  }) => boolean;
};

/**
 * Rendered: connected, not hidden by an ancestor, and has a layout box. This
 * deliberately does not require viewport intersection, so a control inside a
 * scrolled transcript can still receive restored focus.
 */
function isElementRendered(element: HTMLElement | null): boolean {
  if (!element || !element.isConnected) return false;
  if (typeof window === "undefined") return false;
  if (hasHiddenAncestor(element)) return false;

  const check = (element as CheckVisibilityCapable).checkVisibility;
  if (
    typeof check === "function" &&
    !check.call(element, { checkOpacity: false, checkVisibilityCSS: true })
  ) {
    return false;
  }

  const rect = element.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0;
}

/**
 * Visible for the purposes of area focus: rendered *and* intersecting the
 * viewport. Off-screen dock panels and collapsed columns are skipped.
 */
export function isElementVisible(element: HTMLElement | null): boolean {
  if (!isElementRendered(element) || !element) return false;
  const rect = element.getBoundingClientRect();
  const width = window.innerWidth;
  const height = window.innerHeight;
  if (width <= 0 || height <= 0) return false;
  if (
    rect.right <= 0 ||
    rect.bottom <= 0 ||
    rect.left >= width ||
    rect.top >= height
  ) {
    return false;
  }
  return true;
}

function workspaceAreaId(element: HTMLElement, index: number): string {
  const explicit = element.getAttribute(WORKSPACE_AREA_ATTRIBUTE)?.trim();
  if (explicit) return explicit;
  const label = element.getAttribute("aria-label")?.trim();
  if (label) return label;
  return `${element.tagName.toLowerCase()}-${index}`;
}

function applyOrder(
  areas: WorkspaceArea[],
  order: readonly string[] | undefined,
): WorkspaceArea[] {
  if (!order || order.length === 0) return areas;
  const indexOf = (id: string) => order.indexOf(id);
  return [...areas].sort((a, b) => {
    const ai = indexOf(a.id);
    const bi = indexOf(b.id);
    if (ai === -1 && bi === -1) return 0;
    if (ai === -1) return 1;
    if (bi === -1) return -1;
    return ai - bi;
  });
}

/** Visible workspace areas in cycle order. */
export function getVisibleWorkspaceAreas(
  options: AreaFocusOptions = {},
): WorkspaceArea[] {
  if (typeof document === "undefined") return [];
  const root = options.root ?? document;
  const selector = options.selector?.trim() || DEFAULT_WORKSPACE_AREA_SELECTOR;
  const nodes = root.querySelectorAll<HTMLElement>(selector);

  const areas: WorkspaceArea[] = [];
  nodes.forEach((element, index) => {
    if (!isElementVisible(element)) return;
    areas.push({ id: workspaceAreaId(element, index), element });
  });
  return applyOrder(areas, options.order);
}

function isNaturallyFocusable(element: HTMLElement): boolean {
  if (element.hasAttribute("tabindex")) return true;
  if (element.isContentEditable) return true;
  const tag = element.tagName.toLowerCase();
  if (tag === "a" || tag === "area") return element.hasAttribute("href");
  if (tag === "summary") return true;
  if (tag === "audio" || tag === "video") {
    return element.hasAttribute("controls");
  }
  if (
    tag === "input" ||
    tag === "select" ||
    tag === "textarea" ||
    tag === "button"
  ) {
    return !element.hasAttribute("disabled");
  }
  const role = element.getAttribute("role");
  return (
    role === "button" ||
    role === "link" ||
    role === "tab" ||
    role === "menuitem" ||
    role === "option" ||
    role === "checkbox" ||
    role === "radio" ||
    role === "switch" ||
    role === "textbox" ||
    role === "searchbox" ||
    role === "combobox"
  );
}

function focusElement(
  element: HTMLElement,
  preventScroll: boolean,
): boolean {
  if (!isElementRendered(element)) return false;
  if (
    element.hasAttribute("disabled") ||
    element.getAttribute("aria-disabled") === "true"
  ) {
    return false;
  }

  let addedTabIndex = false;
  if (!isNaturallyFocusable(element)) {
    element.setAttribute("tabindex", "-1");
    addedTabIndex = true;
  }

  try {
    element.focus({ preventScroll });
  } catch {
    if (addedTabIndex) element.removeAttribute("tabindex");
    return false;
  }

  const focused =
    typeof document !== "undefined" && document.activeElement === element;
  if (!focused) {
    if (addedTabIndex) element.removeAttribute("tabindex");
    return false;
  }

  if (addedTabIndex) {
    // Restore the DOM to its original shape once focus moves on. Keeping
    // tabindex="-1" while focused means :focus-visible can still paint.
    const cleanup = () => {
      element.removeEventListener("blur", cleanup);
      element.removeAttribute("tabindex");
    };
    element.addEventListener("blur", cleanup);
  }
  return true;
}

function resolveAreaFocusTarget(
  area: HTMLElement,
  selector: string,
): HTMLElement {
  const explicit =
    selector && area.matches(selector)
      ? area
      : selector
        ? area.querySelector<HTMLElement>(selector)
        : null;
  if (explicit && isElementRendered(explicit)) return explicit;
  return area;
}

/**
 * Move focus to the next/previous visible area. Returns what happened; only a
 * real move should call `preventDefault` (the caller owns the key event).
 */
export function focusAdjacentWorkspaceArea(
  direction: 1 | -1,
  options: AreaFocusOptions = {},
): AreaFocusMove {
  const report = (move: AreaFocusMove): AreaFocusMove => {
    options.onMove?.(move);
    return move;
  };

  if (typeof document === "undefined") {
    return report({
      moved: false,
      from: null,
      to: null,
      reason: "no-document",
    });
  }

  const overlaySelector =
    options.overlaySelector === null
      ? null
      : (options.overlaySelector ?? '[aria-modal="true"]');
  if (overlaySelector?.trim() && document.querySelector(overlaySelector)) {
    return report({
      moved: false,
      from: null,
      to: null,
      reason: "overlay-open",
    });
  }

  const areas = getVisibleWorkspaceAreas(options);
  if (areas.length === 0) {
    return report({ moved: false, from: null, to: null, reason: "no-areas" });
  }
  if (areas.length === 1) {
    return report({
      moved: false,
      from: null,
      to: areas[0]!.id,
      reason: "single-area",
    });
  }

  const active = isHtmlElement(document.activeElement)
    ? document.activeElement
    : null;
  const currentIndex = active
    ? areas.findIndex((area) => area.element.contains(active))
    : -1;
  const from = currentIndex >= 0 ? areas[currentIndex]!.id : null;
  const nextIndex =
    currentIndex < 0
      ? direction === 1
        ? 0
        : areas.length - 1
      : (currentIndex + direction + areas.length) % areas.length;
  const target = areas[nextIndex]!;
  const targetElement = resolveAreaFocusTarget(
    target.element,
    options.targetSelector?.trim() || `[${WORKSPACE_AREA_FOCUS_ATTRIBUTE}]`,
  );
  const moved = focusElement(targetElement, false);

  return report({
    moved,
    from,
    to: target.id,
    reason: moved ? undefined : "focus-failed",
  });
}

/**
 * Install the F6/Shift+F6 listener. `optionsOrGetter` may be a function so a
 * React hook can always read the latest options without re-subscribing.
 * Returns an unsubscribe function.
 */
export function installWorkspaceAreaFocus(
  optionsOrGetter: AreaFocusOptions | (() => AreaFocusOptions) = {},
): () => void {
  if (typeof window === "undefined") return () => undefined;
  const getOptions =
    typeof optionsOrGetter === "function"
      ? optionsOrGetter
      : () => optionsOrGetter;

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "F6") return;
    if (event.defaultPrevented || event.isComposing) return;
    // Leave Ctrl/Alt/Meta chords to the platform or other shortcuts. Shift is
    // the backwards direction and is intentionally allowed.
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const options = getOptions();
    if (options.enabled === false) return;
    const move = focusAdjacentWorkspaceArea(
      event.shiftKey ? -1 : 1,
      options,
    );
    if (move.moved) event.preventDefault();
  };

  window.addEventListener("keydown", onKeyDown);
  return () => window.removeEventListener("keydown", onKeyDown);
}

/** React wrapper for `installWorkspaceAreaFocus`. */
export function useWorkspaceAreaFocus(options: AreaFocusOptions = {}): void {
  const optionsRef = useRef(options);
  optionsRef.current = options;
  const enabled = options.enabled !== false;
  useEffect(() => {
    if (!enabled) return;
    return installWorkspaceAreaFocus(() => optionsRef.current);
  }, [enabled]);
}

// ---------------------------------------------------------------------------
// Focus restoration for palettes / menus / dialogs
// ---------------------------------------------------------------------------

export type FocusRestorePoint = {
  readonly element: HTMLElement | null;
  /** Re-focus the remembered control (or fallback). Returns false if gone. */
  restore: () => boolean;
  /**
   * Only restore when focus was lost (body, null, or a removed/hidden
   * control). Use this for menus that close on an outside click so focus is
   * not stolen back from the control the user just clicked.
   */
  restoreIfFocusLost: () => boolean;
  clear: () => void;
};

function focusWasLost(): boolean {
  if (typeof document === "undefined") return true;
  const active = document.activeElement;
  if (!isHtmlElement(active) || active === document.body) return true;
  return !isElementRendered(active);
}

/**
 * Snapshot the current focus before opening an overlay. Calling `restore`
 * later puts focus back; a disconnected/hidden control falls through to the
 * optional fallback. Scroll jumps are suppressed here.
 */
export function captureFocusRestore(
  fallback: HTMLElement | null = null,
): FocusRestorePoint {
  const body =
    typeof document !== "undefined" ? document.body : null;
  const active =
    typeof document !== "undefined" ? document.activeElement : null;
  let element: HTMLElement | null =
    isHtmlElement(active) && active !== body ? active : null;
  let fallbackElement = fallback && fallback !== body ? fallback : null;

  return {
    get element() {
      return element;
    },
    restore() {
      if (element && focusElement(element, true)) return true;
      if (fallbackElement && focusElement(fallbackElement, true)) return true;
      return false;
    },
    restoreIfFocusLost() {
      if (!focusWasLost()) return false;
      if (element && focusElement(element, true)) return true;
      if (fallbackElement && focusElement(fallbackElement, true)) return true;
      return false;
    },
    clear() {
      element = null;
      fallbackElement = null;
    },
  };
}

/**
 * Capture focus on the render where `active` becomes true (before React's
 * autoFocus runs) and restore it when `active` becomes false or the overlay
 * unmounts. Use one instance per overlay: a command palette, a dialog shell,
 * or a composer menu whose `open` state is passed in.
 */
export function useOverlayFocusRestore(
  active: boolean,
  fallback: HTMLElement | null = null,
): void {
  const pointRef = useRef<FocusRestorePoint | null>(null);
  if (active && pointRef.current === null) {
    pointRef.current = captureFocusRestore(fallback);
  }

  useEffect(() => {
    if (!active) {
      const point = pointRef.current;
      pointRef.current = null;
      point?.restore();
    }
  }, [active]);

  useEffect(() => {
    return () => {
      const point = pointRef.current;
      pointRef.current = null;
      point?.restore();
    };
  }, []);
}
