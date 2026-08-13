import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { openExternal, resolveLinkTarget, revealInFileManager, type LinkResolution } from "../lib/api";
import { linkLabel, splitLinkSegments, type LinkTarget } from "../lib/linkTargets";

/**
 * Paths and URLs inside agent text become clickable.
 *
 * Left click does the obvious thing (open); right click offers the rest. Agent
 * output is untrusted, so nothing is resolved or opened until the user acts,
 * and executables need a second, explicit confirmation.
 */

/** Project root used to resolve relative paths — provided per dialog. */
export const LinkCwdContext = createContext<string | null>(null);

type MenuState = {
  x: number;
  y: number;
  target: LinkTarget;
  resolution: LinkResolution | null;
  busy: boolean;
  note: string;
};

async function copyText(value: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(value);
  } catch {
    // Clipboard can be blocked; the value is still visible in the card.
  }
}

/**
 * Shared link-click behaviour for markdown anchors and linkified prose:
 * resolve, open, and on any failure show the menu with the reason instead of
 * silently doing nothing.
 */
export function useLinkMenu() {
  const cwd = useContext(LinkCwdContext);
  const [menu, setMenu] = useState<MenuState | null>(null);

  const openMenu = useCallback(
    async (event: { clientX: number; clientY: number }, target: LinkTarget, note = "") => {
      setMenu({ x: event.clientX, y: event.clientY, target, resolution: null, busy: true, note });
      const resolution = await resolveLinkTarget(target.raw, cwd).catch(() => null);
      setMenu((current) =>
        current && current.target.raw === target.raw
          ? { ...current, resolution, busy: false }
          : current
      );
    },
    [cwd]
  );

  const primaryAction = useCallback(
    async (event: React.MouseEvent, target: LinkTarget) => {
      if (target.kind === "url") {
        try {
          const result = await openExternal(target.raw, cwd);
          if (!result?.opened) {
            void openMenu(event, target, result?.message ?? "Could not open this link.");
          }
        } catch {
          void openMenu(event, target, "Could not open this link.");
        }
        return;
      }
      const resolution = await resolveLinkTarget(target.raw, cwd).catch(() => null);
      // Missing or executable → never guess; show the menu with the reason.
      if (!resolution || resolution.kind === "missing") {
        void openMenu(event, target, "Not found on disk from this project.");
        return;
      }
      if (resolution.risky) {
        void openMenu(event, target, "Executable file — confirm below to launch it.");
        return;
      }
      const result = await openExternal(target.raw, cwd).catch(() => null);
      if (!result?.opened) {
        void openMenu(event, target, result?.message ?? "Could not open this item.");
      }
    },
    [cwd, openMenu]
  );

  const renderMenu = () =>
    menu ? (
      <LinkMenu
        state={menu}
        cwd={cwd}
        onClose={() => setMenu(null)}
        onNote={(note) => setMenu((current) => (current ? { ...current, note } : current))}
      />
    ) : null;

  return { openMenu, primaryAction, renderMenu };
}

export function LinkedText({ text, className }: { text: string; className?: string }): ReactNode {
  const { openMenu, primaryAction, renderMenu } = useLinkMenu();

  const segments = splitLinkSegments(text);

  if (segments.length === 1 && segments[0].type === "text") {
    return <>{text}</>;
  }

  return (
    <>
      {segments.map((segment, index) => {
        if (segment.type === "text") return <span key={index}>{segment.text}</span>;
        const { target } = segment;
        return (
          <button
            key={index}
            type="button"
            className={className ? `inline-link ${className}` : "inline-link"}
            data-link-kind={target.kind}
            title={`${target.raw}\nClick to open · right-click for more`}
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              void primaryAction(event, target);
            }}
            onContextMenu={(event) => {
              event.preventDefault();
              event.stopPropagation();
              void openMenu(event, target);
            }}
          >
            {linkLabel(target)}
          </button>
        );
      })}
      {renderMenu()}
    </>
  );
}

function LinkMenu({
  state,
  cwd,
  onClose,
  onNote,
}: {
  state: MenuState;
  cwd: string | null;
  onClose: () => void;
  onNote: (note: string) => void;
}) {
  const { target, resolution } = state;
  const isUrl = target.kind === "url" || resolution?.kind === "url";
  const exists = resolution != null && resolution.kind !== "missing";

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    const onDown = () => onClose();
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("wheel", onDown, { passive: true });
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("wheel", onDown);
    };
  }, [onClose]);

  const run = async (action: () => Promise<unknown>) => {
    try {
      await action();
      onClose();
    } catch (error) {
      onNote(error instanceof Error ? error.message : String(error));
    }
  };

  // Portal: the menu lives inside markdown `<p>` / clipped tool bodies, where a
  // nested div is invalid HTML and an overflow ancestor could crop it.
  return createPortal(
    <div
      className="link-menu"
      role="menu"
      // Keep inside the window without measuring: menus are small and fixed-width.
      style={{
        left: Math.min(state.x, Math.max(0, window.innerWidth - 240)),
        top: Math.min(state.y, Math.max(0, window.innerHeight - 168)),
      }}
      onMouseDown={(event) => event.stopPropagation()}
    >
      <div className="link-menu__path" title={target.raw}>
        {target.raw}
      </div>
      {isUrl ? (
        <button type="button" role="menuitem" onClick={() => void run(() => openExternal(target.raw, cwd))}>
          Open in browser
        </button>
      ) : (
        <>
          <button
            type="button"
            role="menuitem"
            disabled={!exists}
            onClick={() =>
              void run(() => openExternal(target.raw, cwd, resolution?.risky === true))
            }
          >
            {resolution?.risky ? "Run with default app ⚠" : "Open with default app"}
          </button>
          <button
            type="button"
            role="menuitem"
            disabled={!exists}
            onClick={() => void run(() => revealInFileManager(target.raw, cwd))}
          >
            Show in File Explorer
          </button>
        </>
      )}
      <button type="button" role="menuitem" onClick={() => void run(() => copyText(target.raw))}>
        {isUrl ? "Copy link" : "Copy path"}
      </button>
      {(state.busy || state.note || (!exists && !isUrl)) && (
        <div className="link-menu__note">
          {state.busy ? "Checking…" : state.note || "Not found on disk from this project."}
        </div>
      )}
    </div>,
    document.body
  );
}

/** Linkify plain-string leaves of a rendered markdown node tree. */
export function linkifyChildren(children: ReactNode): ReactNode {
  if (typeof children === "string") return <LinkedText text={children} />;
  if (Array.isArray(children)) {
    return children.map((child, index) =>
      typeof child === "string" ? <LinkedText key={index} text={child} /> : child
    );
  }
  return children;
}