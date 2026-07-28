import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { userMessageAnchorId } from "../lib/acpTranscript";
import type { SessionEvent } from "../lib/types";

type MessageOutlineProps = {
  events: SessionEvent[];
  sessionId: string;
};

/** Visible rows in the hover panel before scroll (approx). */
const VISIBLE_ROWS = 6;

function previewText(text: string, max = 72): string {
  const one = text.replace(/\s+/g, " ").trim();
  if (!one) return "(empty)";
  return one.length > max ? `${one.slice(0, max - 1)}…` : one;
}

function jumpTo(id: string) {
  const el = document.getElementById(id);
  if (!el) return;
  el.scrollIntoView({ behavior: "smooth", block: "start" });
  el.classList.add("is-outline-flash");
  window.setTimeout(() => el.classList.remove("is-outline-flash"), 900);
}

/**
 * Conversation outline — user messages only.
 * Collapsed: thin rail. Hover: floating list (≈6 rows, scroll for more), click to jump.
 */
export function MessageOutline({ events, sessionId }: MessageOutlineProps) {
  const items = useMemo(
    () =>
      events.filter(
        (e): e is Extract<SessionEvent, { type: "user_message" }> =>
          e.sessionId === sessionId && e.type === "user_message"
      ),
    [events, sessionId]
  );

  const [open, setOpen] = useState(false);
  const leaveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const clearLeave = useCallback(() => {
    if (leaveTimer.current) {
      clearTimeout(leaveTimer.current);
      leaveTimer.current = null;
    }
  }, []);

  const scheduleClose = useCallback(() => {
    clearLeave();
    leaveTimer.current = setTimeout(() => setOpen(false), 180);
  }, [clearLeave]);

  const openPanel = useCallback(() => {
    clearLeave();
    setOpen(true);
  }, [clearLeave]);

  useEffect(() => () => clearLeave(), [clearLeave]);

  // When opening, scroll list so latest messages are in view
  useEffect(() => {
    if (!open || !listRef.current) return;
    listRef.current.scrollTop = listRef.current.scrollHeight;
  }, [open, items.length]);

  if (items.length < 2) return null;

  return (
    <div
      className={`message-outline${open ? " is-open" : ""}`}
      onMouseEnter={openPanel}
      onMouseLeave={scheduleClose}
      onFocus={openPanel}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node)) {
          scheduleClose();
        }
      }}
    >
      {/* Always-visible thin rail (hover target) */}
      <div className="message-outline__rail" aria-hidden={!open}>
        {items.slice(-Math.min(items.length, 8)).map((event, i) => (
          <span
            key={event.messageId ?? `${event.createdAt}-d-${i}`}
            className="message-outline__rail-dot"
          />
        ))}
      </div>

      {/* Expanded list — open on hover */}
      <div
        className="message-outline__panel"
        role="navigation"
        aria-label="Jump to user messages"
        aria-hidden={!open}
        hidden={!open}
      >
        <div className="message-outline__panel-head">
          <strong>You</strong>
          <span>{items.length} messages</span>
        </div>
        <div
          ref={listRef}
          className="message-outline__list custom-scrollbar scrollbar-autohide"
          // ~6 rows visible; wheel scrolls the rest
          style={{ maxHeight: `calc(${VISIBLE_ROWS} * 34px + 8px)` }}
        >
          {items.map((event, index) => {
            const id = userMessageAnchorId(event);
            const label = previewText(event.text);
            return (
              <button
                key={event.messageId ?? `${event.createdAt}-${index}`}
                type="button"
                className="message-outline__item"
                title={event.text.slice(0, 200)}
                onClick={() => {
                  jumpTo(id);
                  setOpen(false);
                }}
              >
                <span className="message-outline__idx">{index + 1}</span>
                <span className="message-outline__text">{label}</span>
              </button>
            );
          })}
        </div>
        {items.length > VISIBLE_ROWS && (
          <div className="message-outline__panel-foot">Scroll for more</div>
        )}
      </div>
    </div>
  );
}
