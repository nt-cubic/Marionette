# Sidebar Auto-Hide + Unified Scrollbar Implementation Plan

> **For agentic workers:** Use subagent-driven-development or executing-plans to implement task-by-task.

**Goal:** Two UI immersion improvements — (1) collapsed sidebars fully hide to 0px with hover-to-reveal (VSCode-style), (2) all scrollbars unified to thin Zed-style auto-hide variant.

**Architecture:** 
- Sidebar: keep existing CSS Grid layout, change collapsed column width from 34px to 0px, add overlay capture zones at screen edges with CSS transition animation. Use JS state for hover detection with debounced mouseenter/mouseleave.
- Scrollbar: define CSS custom properties for scrollbar tokens, create a `.custom-scrollbar` utility class, apply globally, use a lightweight JS observer for auto-hide via `.is-scrolling` class + CSS opacity transition.

**Tech Stack:** React (TSX), CSS custom properties, CSS Grid, Tauri webview (Chromium-based).

## Global Constraints

- Keep the existing minimal black-white Zed-style UI identity (memory id=7)
- No Tailwind/shadcn or any UI framework imports (memory id=7)
- All scrollable containers must use the same scrollbar style
- Sidebar expanded width must still be resizable via drag handle
- Must still work with `is-panel-resizing` body class during drag

---

### Task 1: Scrollbar CSS Variables + Global Styles

**Files:**
- Modify: `src/styles/tokens.css` — add scrollbar color tokens
- Modify: `src/styles/app.css` — add global scrollbar styles + auto-hide keyframes/transitions

- [ ] **Step 1: Add scrollbar tokens to tokens.css**

Insert after the existing `--composer-bg` line:

```css
  --scrollbar-width: 4px;
  --scrollbar-track: transparent;
  --scrollbar-thumb: color-mix(in srgb, var(--text-muted) 28%, transparent);
  --scrollbar-thumb-hover: color-mix(in srgb, var(--text-muted) 48%, transparent);
```

- [ ] **Step 2: Add global scrollbar base rules + auto-hide to app.css**

Append after the `@keyframes activity-pulse` block:

```css
/* ============================================================
 * Unified custom scrollbar (Zed-style thin)
 * Auto-hide: scrollbar thumb fades out when not scrolling.
 * Activate with a `.is-scrolling` class on the scrollable container.
 * ============================================================ */

/* Firefox — thin scrollbar with custom colors */
.custom-scrollbar {
  scrollbar-width: thin;
  scrollbar-color: var(--scrollbar-thumb) var(--scrollbar-track);
}

/* Chrome/Edge/Safari — custom thin scrollbar */
.custom-scrollbar::-webkit-scrollbar {
  width: var(--scrollbar-width, 4px);
  height: var(--scrollbar-width, 4px);
}

.custom-scrollbar::-webkit-scrollbar-track {
  background: var(--scrollbar-track, transparent);
}

.custom-scrollbar::-webkit-scrollbar-thumb {
  border-radius: 999px;
  background: var(--scrollbar-thumb);
  transition: background 0.15s ease, opacity 0.25s ease;
}

.custom-scrollbar::-webkit-scrollbar-thumb:hover {
  background: var(--scrollbar-thumb-hover);
}

/* Auto-hide: thumb is invisible by default, fades in when .is-scrolling */
.custom-scrollbar.scrollbar-autohide::-webkit-scrollbar-thumb {
  opacity: 0;
}

.custom-scrollbar.scrollbar-autohide.is-scrolling::-webkit-scrollbar-thumb,
.custom-scrollbar.scrollbar-autohide:hover::-webkit-scrollbar-thumb {
  opacity: 1;
}

/* Firefox auto-hide uses scrollbar-color with transparent fallback */
.custom-scrollbar.scrollbar-autohide {
  scrollbar-color: transparent var(--scrollbar-track);
  transition: scrollbar-color 0.25s ease;
}

.custom-scrollbar.scrollbar-autohide.is-scrolling,
.custom-scrollbar.scrollbar-autohide:hover {
  scrollbar-color: var(--scrollbar-thumb) var(--scrollbar-track);
}
```

- [ ] **Step 3: Create the auto-hide hook `useScrollbarAutoHide`**

Create new file `src/lib/useScrollbarAutoHide.ts`:

```tsx
import { useEffect, useRef, type RefObject } from "react";

const IDLE_TIMEOUT = 600; // ms after last scroll event before hiding

/**
 * Lightweight hook: adds `.is-scrolling` to `ref.current` while the user
 * is actively scrolling, removes it after `IDLE_TIMEOUT` ms of inactivity.
 * Works with `.custom-scrollbar.scrollbar-autohide` CSS.
 *
 * @param ref - Ref to a scrollable container element
 * @param enabled - Whether auto-hide is active (default true)
 */
export function useScrollbarAutoHide(
  ref: RefObject<HTMLElement | null>,
  enabled = true,
): void {
  const timerRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    const el = ref.current;
    if (!el || !enabled) return;

    const onScroll = () => {
      el.classList.add("is-scrolling");
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => {
        el.classList.remove("is-scrolling");
      }, IDLE_TIMEOUT);
    };

    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, [ref, enabled]);
}
```

- [ ] **Step 4: Apply custom-scrollbar class to all scrollable containers in app.css**

Replace the existing scrollbar styles on `.context-panel`, `.message-outline__list`, etc.

In app.css:
- Remove the dedicated `::-webkit-scrollbar` blocks on `.context-panel` (lines 325-340)
- Remove the dedicated `::-webkit-scrollbar` blocks on `.message-outline__list` (lines 1893-1900)
- Replace with `.custom-scrollbar` class

For `.context-panel` — it already has `scrollbar-width: thin` and `scrollbar-color` at line 321-322. Keep those inline but add `.custom-scrollbar.scrollbar-autohide`:

```
.context-panel { scrollbar-width: thin; scrollbar-color: ... }
```
→ remove the inline scrollbar rules, just keep `.custom-scrollbar.scrollbar-autohide` on the element

**Search for all `overflow: auto` / `overflow-y: auto` / `overflow-y: scroll` in app.css and add `.custom-scrollbar` class to those elements.**

- [ ] **Step 5: Verify**

Run `npm run build` or `npm run dev` to confirm no build errors.

---

### Task 2: Sidebar Auto-Hide — Capture Zones + State Logic

**Files:**
- Modify: `src/app/App.tsx` — add capture zone hover state, debounce logic, transition CSS
- Modify: `src/styles/app.css` — add collapsed-0 grid rule, capture zone positioning

**Interfaces:**
- Consumes: existing `leftCollapsed`/`rightCollapsed` state, `setLeftCollapsed`/`setRightCollapsed`
- Produces: `leftHover`/`rightHover` state booleans; transparent capture zones rendered in JSX

- [ ] **Step 1: Add collapsible-0 grid rules to app.css**

Replace the existing `.workspace-grid.is-left-collapsed` and `.is-right-collapsed` rules:

```css
/* Collapsed: fully hide sidebar (0px) — no rail */
.workspace-grid.is-left-collapsed {
  grid-template-columns: 0px minmax(0, 1fr) var(--right-panel-width, 270px);
}

.workspace-grid.is-right-collapsed {
  grid-template-columns: var(--left-panel-width, 224px) minmax(0, 1fr) 0px;
}

.workspace-grid.is-left-collapsed.is-right-collapsed {
  grid-template-columns: 0px minmax(0, 1fr) 0px;
}

/* Expanded via hover: restore stored width */
.workspace-grid.is-left-collapsed.is-left-hover-expanded {
  grid-template-columns: var(--left-panel-width, 224px) minmax(0, 1fr) var(--right-panel-width, 270px);
}

.workspace-grid.is-right-collapsed.is-right-hover-expanded {
  grid-template-columns: var(--left-panel-width, 224px) minmax(0, 1fr) var(--right-panel-width, 270px);
}

/* Smooth transition for the expand/collapse animation */
.workspace-grid.is-left-collapsed,
.workspace-grid.is-right-collapsed {
  transition: grid-template-columns 0.15s ease;
}

/* But not during resize */
body.is-panel-resizing .workspace-grid.is-left-collapsed,
body.is-panel-resizing .workspace-grid.is-right-collapsed {
  transition: none;
}
```

- [ ] **Step 2: Add capture zone CSS to app.css**

```css
/* Capture zones — invisible hit areas at screen edges for hover detection */
.capture-zone {
  position: fixed;
  top: 0;
  bottom: 0;
  z-index: 10;
  width: 10px;
  /* Only visible when the respective sidebar is collapsed */
}

.capture-zone--left {
  left: 0;
}

.capture-zone--right {
  right: 0;
}

/* Make capture zone wider when approaching edge (easier to hit) */
.capture-zone:hover {
  width: 14px;
}

/* Keep the collapsed sidebar itself non-interactive but hide it */
.left-rail.is-collapsed,
.context-panel.is-collapsed {
  overflow: hidden;
  /* sidebar content is hidden when width=0 */
}
```

- [ ] **Step 3: Add auto-hide state + handlers in App.tsx**

In App.tsx, add these state variables after the existing sidebar states (~line 176-181):

```tsx
/** Hover auto-expand for fully-collapsed sidebars */
const [leftHoverExpanded, setLeftHoverExpanded] = useState(false);
const [rightHoverExpanded, setRightHoverExpanded] = useState(false);
const hoverLeaveTimers = useRef<{ left?: ReturnType<typeof setTimeout>; right?: ReturnType<typeof setTimeout> }>({});
```

Add handlers near the existing collapse handlers:

```tsx
const handleLeftCaptureEnter = useCallback(() => {
  if (!leftCollapsed) return;
  if (hoverLeaveTimers.current.left) clearTimeout(hoverLeaveTimers.current.left);
  setLeftHoverExpanded(true);
}, [leftCollapsed]);

const handleLeftSidebarLeave = useCallback(() => {
  if (!leftCollapsed) return;
  if (hoverLeaveTimers.current.left) clearTimeout(hoverLeaveTimers.current.left);
  // Small delay to prevent flicker when moving mouse over sidebar edge
  hoverLeaveTimers.current.left = setTimeout(() => {
    setLeftHoverExpanded(false);
  }, 300);
}, [leftCollapsed]);

const handleRightCaptureEnter = useCallback(() => {
  if (!rightCollapsed) return;
  if (hoverLeaveTimers.current.right) clearTimeout(hoverLeaveTimers.current.right);
  setRightHoverExpanded(true);
}, [rightCollapsed]);

const handleRightSidebarLeave = useCallback(() => {
  if (!rightCollapsed) return;
  if (hoverLeaveTimers.current.right) clearTimeout(hoverLeaveTimers.current.right);
  hoverLeaveTimers.current.right = setTimeout(() => {
    setRightHoverExpanded(false);
  }, 300);
}, [rightCollapsed]);

// Clean up hover timers on unmount or collapse toggle
useEffect(() => {
  if (!leftCollapsed) setLeftHoverExpanded(false);
  if (!rightCollapsed) setRightHoverExpanded(false);
  return () => {
    if (hoverLeaveTimers.current.left) clearTimeout(hoverLeaveTimers.current.left);
    if (hoverLeaveTimers.current.right) clearTimeout(hoverLeaveTimers.current.right);
  };
}, [leftCollapsed, rightCollapsed]);
```

- [ ] **Step 4: Update the grid className in App.tsx return section**

Find the `className={`workspace-grid...`} line (~2578) and append hover classes:

```tsx
className={`workspace-grid${leftCollapsed ? " is-left-collapsed" : ""}${rightCollapsed ? " is-right-collapsed" : ""}${leftCollapsed && leftHoverExpanded ? " is-left-hover-expanded" : ""}${rightCollapsed && rightHoverExpanded ? " is-right-hover-expanded" : ""}`}
```

- [ ] **Step 5: Add capture zones to JSX**

After the workspace-grid div's opening tag, add:

```tsx
{/* Capture zones for hover-to-reveal collapsed sidebars */}
{leftCollapsed && (
  <div
    className="capture-zone capture-zone--left"
    onMouseEnter={handleLeftCaptureEnter}
  />
)}
{rightCollapsed && (
  <div
    className="capture-zone capture-zone--right"
    onMouseEnter={handleRightCaptureEnter}
  />
)}
```

On the left sidebar `<aside>`, add `onMouseLeave={handleLeftSidebarLeave}`.
On the right sidebar `<aside>`, add `onMouseLeave={handleRightSidebarLeave}`.

- [ ] **Step 6: Hide resize handle when sidebar is fully collapsed**

In app.css, update:
```css
.left-rail.is-collapsed .panel-resizer,
.context-panel.is-collapsed .panel-resizer {
  display: none;
}
```

---

### Task 3: Sidebar Collapsed State Adaptation

**Files:**
- Modify: `src/components/ProjectShelf.tsx` — collapsed state (0px mode, no rail)
- Modify: `src/components/ContextPanel.tsx` — collapsed state (0px mode, no rail)

**Note:** Since the expanded sidebar is now triggered by edge hover (not by clicking the rail button), the collapsed rail UI is no longer needed. Both the left and right collapsed rails will be removed — they are replaced by the capture zones in the App layer.

- [ ] **Step 1: ProjectShelf collapsed — remove rail UI**

In `ProjectShelf.tsx`, the collapsed return (~line 239-248) currently renders a `collapsed-rail` div with buttons. Since the capture zone handles hover-to-expand, this can be significantly simplified. The collapsed sidebar should render nothing (or a minimal invisible proxy):

```tsx
if (collapsed) {
  // Fully collapsed: no visible rail, the capture zone in App handles hover.
  // Return an empty aside that still has .left-rail.is-collapsed for styling.
  return (
    <aside className="left-rail is-collapsed" aria-label="Projects and sessions" />
  );
}
```

- [ ] **Step 2: ContextPanel collapsed — remove rail UI**

In `ContextPanel.tsx`, the collapsed return (~line 96-109) similarly should be simplified:

```tsx
if (collapsed) {
  return (
    <aside className="context-panel is-collapsed" aria-label="Context panel" />
  );
}
```

- [ ] **Step 3: Move collapse buttons to the titlebar/tab area**

Since the collapsed rail is gone, users need another way to explicitly toggle collapse. Add collapse buttons to the sidebar's titlebar (visible when expanded):

For ProjectShelf: the existing collapse button in the sidebar footer (`sidebar-footer__button--collapse` at line 603) already exists and works — keep it.

For ContextPanel: the existing collapse button at line 333 already exists — keep it.

These remain as the way to manually collapse sidebars. To expand from fully collapsed, users use the capture zone hover.

---

### Task 4: Apply Custom Scrollbar to All Containers

**Files:**
- Modify: `src/styles/app.css` — add `.custom-scrollbar` to all scrollable container selectors
- Modify: `src/app/App.tsx` — apply hook to scrollable refs
- Modify: `src/components/SessionView.tsx` — apply scrollbar class + hook

- [ ] **Step 1: Find all scrollable containers in app.css**

Search for `overflow: auto`, `overflow-y: auto`, `overflow-y: scroll`, `overflow-x: auto` in app.css. For each, either add the `.custom-scrollbar` class to the JSX element or add the selector to the CSS rule.

Key containers:
- `.project-list` (line 654-658) — add `custom-scrollbar scrollbar-autohide` class to the JSX div
- `.session-list` — add class to JSX element
- `.context-panel` — add class to `<aside>` element in ContextPanel.tsx
- `.clean-surface__body > .event-list` (line 1787-1796) — add class
- `.message-outline__list` (line 1886-1891) — add class
- `.file-diff-card__body` (line 797) — add class
- `.terminal-surface` (line 1721-1727) — add class

- [ ] **Step 2: Apply useScrollbarAutoHide hook to key containers**

In App.tsx, add refs and hooks for main scrollable areas:

```tsx
const eventListRef = useRef<HTMLDivElement>(null);
const projectListRef = useRef<HTMLDivElement>(null);
// etc.

// Apply auto-hide to scrollable containers
useScrollbarAutoHide(eventListRef);
useScrollbarAutoHide(projectListRef);
```

- [ ] **Step 3: Pass refs down or refactor to forwardRef where needed**

For components like ProjectShelf and SessionView, either:
- Add a className prop for scrollbar styling and let them add the class themselves, or
- Use a CSS selector approach (all `.custom-scrollbar` elements get the auto-hide via a global intersection observer)

**Recommended simpler approach:** Instead of passing refs to every component, use a single global scroll observer in App.tsx (or as a standalone module):

```tsx
// src/lib/scrollbarAutoHide.ts
/**
 * Global scrollbar auto-hide: attaches .is-scrolling class management
 * to all elements matching a CSS selector. Run once on mount.
 */
export function initScrollbarAutoHide(selector = ".custom-scrollbar.scrollbar-autohide"): () => void {
  const timers = new Map<Element, ReturnType<typeof setTimeout>>();
  
  const onScroll = (e: Event) => {
    const el = e.currentTarget as Element;
    el.classList.add("is-scrolling");
    const existing = timers.get(el);
    if (existing) clearTimeout(existing);
    timers.set(el, setTimeout(() => {
      el.classList.remove("is-scrolling");
      timers.delete(el);
    }, 600));
  };

  const observer = new MutationObserver(() => {
    document.querySelectorAll(selector).forEach((el) => {
      if (!el.hasAttribute("data-scrollbar-wired")) {
        el.setAttribute("data-scrollbar-wired", "");
        el.addEventListener("scroll", onScroll, { passive: true });
      }
    });
  });

  // Initial scan
  document.querySelectorAll(selector).forEach((el) => {
    el.setAttribute("data-scrollbar-wired", "");
    el.addEventListener("scroll", onScroll, { passive: true });
  });

  observer.observe(document.body, { childList: true, subtree: true });

  return () => {
    observer.disconnect();
    document.querySelectorAll(selector).forEach((el) => {
      el.removeEventListener("scroll", onScroll);
    });
    for (const t of timers.values()) clearTimeout(t);
    timers.clear();
  };
}
```

Then call it in a `useEffect` in App.tsx once:
```tsx
useEffect(() => {
  const cleanup = initScrollbarAutoHide();
  return cleanup;
}, []);
```

This is much simpler than wiring refs through every component.

- [ ] **Step 4: Build & verify**

Run `npm run dev` or build to confirm no errors.

---

### Task 5: Polish — Edge Cases + QA

**Files:**
- Modify: `src/app/App.tsx` — final tuning
- Modify: `src/styles/app.css` — final tuning

- [ ] **Step 1: Handle sidebar hover-expand during drag resize**

When `resizingSide` is active, do NOT auto-expand collapsed sidebars:

```tsx
const handleLeftCaptureEnter = useCallback(() => {
  if (!leftCollapsed || resizingSide) return;
  // ...
}, [leftCollapsed, resizingSide]);
```

- [ ] **Step 2: Handle sidebar expand when clicking a sidebar element**

When the user clicks on a collapsed sidebar (which now shows nothing), the capture zone handles hover. But if they want to permanently expand, they can click the collpase button. This is fine — capture zone + manual toggle.

- [ ] **Step 3: Prevent flicker during resize**

When a sidebar is collapsed and the user resizes the window, the capture zone position is fixed (0, 0 → 10px) so it's unaffected.

- [ ] **Step 4: Global cleanup effect**

Ensure all timeouts and event listeners are cleaned up in useEffect returns.
