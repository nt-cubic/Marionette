/**
 * Dedicated Memo timeline (Memos-inspired): composer on top, day-grouped
 * cards below. Data is the inbox session's user_message stream.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { MarkdownBody } from "./MarkdownBody";
import { UserImageCard } from "./UserImageCard";
import { formatMemoTime, groupMemosByDay, memosFromEvents } from "../lib/memos";
import type { SessionEvent } from "../lib/types";

export type MemosPageProps = {
  events: SessionEvent[];
  busy?: boolean;
  onSubmit: (text: string) => Promise<void> | void;
};

export function MemosPage({ events, busy = false, onSubmit }: MemosPageProps) {
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const items = useMemo(() => memosFromEvents(events), [events]);
  const groups = useMemo(() => groupMemosByDay(items), [items]);

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(160, Math.max(40, el.scrollHeight))}px`;
  }, [draft]);

  const submit = useCallback(async () => {
    const text = draft.trim();
    if (!text || sending || busy) return;
    setSending(true);
    setDraft("");
    try {
      await onSubmit(text);
    } finally {
      setSending(false);
      textareaRef.current?.focus();
    }
  }, [draft, sending, busy, onSubmit]);

  return (
    <div className="memos-page">
      <div className="memos-composer">
        <textarea
          ref={textareaRef}
          className="memos-composer__input"
          value={draft}
          placeholder="记点什么…"
          rows={1}
          disabled={sending}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void submit();
            }
          }}
        />
        <div className="memos-composer__bar">
          <span className="memos-composer__hint">Ctrl+Enter 收集</span>
          <button
            type="button"
            className="memos-composer__send"
            disabled={!draft.trim() || sending || busy}
            onClick={() => void submit()}
          >
            {sending ? "收集中" : "收集"}
          </button>
        </div>
      </div>

      <div className="memos-stream custom-scrollbar">
        {items.length === 0 ? (
          <p className="memos-empty">还没有记录。从上面写下一条，或把文件拖到悬浮球上。</p>
        ) : (
          groups.map((group) => (
            <section key={group.day} className="memos-day">
              <h3 className="memos-day__label">{group.day}</h3>
              {group.items.map((memo) => (
                <article key={memo.id} className="memo-card">
                  {memo.text ? (
                    <div className="memo-card__body">
                      <MarkdownBody text={memo.text} />
                    </div>
                  ) : null}
                  {memo.attachments && memo.attachments.length > 0 ? (
                    <UserImageCard attachments={memo.attachments} />
                  ) : null}
                  {memo.tags.length > 0 ? (
                    <div className="memo-card__tags">
                      {memo.tags.map((tag) => (
                        <span key={tag} className="memo-card__tag">
                          #{tag}
                        </span>
                      ))}
                    </div>
                  ) : null}
                  <footer className="memo-card__meta">
                    {memo.source ? <span>{memo.source}</span> : null}
                    <time dateTime={memo.createdAt}>{formatMemoTime(memo.createdAt)}</time>
                  </footer>
                </article>
              ))}
            </section>
          ))
        )}
      </div>
    </div>
  );
}
