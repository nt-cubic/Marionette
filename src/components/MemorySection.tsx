import { Pencil, RefreshCw, Sparkles, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import {
  memoryAdd,
  memoryDelete,
  memoryDistill,
  memoryGet,
  memorySetEntryEnabled,
  memoryUpdate,
  type MemoryEntry,
  type MemoryStore,
} from "../lib/api";

const SOURCE_LABELS: Record<string, string> = {
  manual: "manual",
  claude: "Claude",
  codex: "Codex",
  opencode: "OpenCode",
  gemini: "Gemini",
  session: "session",
};

function sourceLabel(source: string): string {
  return SOURCE_LABELS[source] ?? source;
}

/**
 * 跨 agent 共享记忆面板。
 * 打开即同步各家全局记忆；勾选 = 注入新会话；保留「从会话提炼」。
 */
export function MemorySection({ activeSessionId }: { activeSessionId?: string | null }) {
  const [store, setStore] = useState<MemoryStore | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [editId, setEditId] = useState<string | null>(null);
  const [editTitle, setEditTitle] = useState("");
  const [editText, setEditText] = useState("");

  const [draft, setDraft] = useState<string | null>(null);
  const [draftBusy, setDraftBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setStore(await memoryGet());
      setNote(null);
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    try {
      await action();
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const handleDistill = async () => {
    if (!activeSessionId) return;
    setDraftBusy(true);
    try {
      const { draft: text } = await memoryDistill(activeSessionId);
      setDraft(text);
      setNote(null);
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setDraftBusy(false);
    }
  };

  const handleSaveDraft = async () => {
    if (!draft || !draft.trim()) return;
    const date = new Date().toISOString().slice(0, 10);
    await run(() =>
      memoryAdd(`会话要点 ${date}`, draft, "session").then(() => undefined),
    );
    setDraft(null);
  };

  const entries = store?.entries ?? [];

  return (
    <section className="context-card">
      <div className="context-card__heading">
        <span>MEMORY</span>
        <div className="context-card__heading-actions">
          <button
            className="pill-action pill-action--icon pill-action--sm"
            type="button"
            title="Refresh memory"
            aria-label="Refresh memory"
            onClick={() => void refresh()}
          >
            <RefreshCw size={11} />
          </button>
        </div>
      </div>

      {note ? (
        <p className="panel-note" style={{ whiteSpace: "pre-wrap" }}>
          {note}
        </p>
      ) : null}

      {entries.length === 0 ? (
        <p className="context-card__empty" style={{ marginTop: 8 }}>
          No agent memory files found yet.
        </p>
      ) : (
        <ul className="context-lend__list custom-scrollbar scrollbar-autohide">
          {entries.map((entry) => (
            <MemoryRow
              key={entry.id}
              entry={entry}
              busy={busy}
              editId={editId}
              editTitle={editTitle}
              editText={editText}
              onEditTitle={setEditTitle}
              onEditText={setEditText}
              onEdit={() => {
                setEditId(entry.id);
                setEditTitle(entry.title);
                setEditText(entry.text);
              }}
              onCancelEdit={() => setEditId(null)}
              onSaveEdit={() =>
                void run(async () => {
                  await memoryUpdate(entry.id, editTitle, editText);
                  setEditId(null);
                })
              }
              onToggle={(enabled) =>
                void run(() => memorySetEntryEnabled(entry.id, enabled))
              }
              onDelete={() => void run(() => memoryDelete(entry.id))}
            />
          ))}
        </ul>
      )}

      <div className="panel-actions">
        {activeSessionId ? (
          <button
            className="pill-action"
            type="button"
            disabled={draftBusy}
            title="Distill lasting notes from this session"
            onClick={() => void handleDistill()}
          >
            <Sparkles size={11} />
            {draftBusy ? "提炼中…" : "从会话提炼"}
          </button>
        ) : null}
      </div>

      {draft ? (
        <div className="panel-form">
          <textarea
            className="panel-form__input panel-form__input--area"
            style={{ minHeight: 88 }}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          <div className="panel-form__actions">
            <button className="pill-action" type="button" onClick={() => setDraft(null)}>
              丢弃
            </button>
            <button
              className="pill-action"
              type="button"
              disabled={busy || draft.trim() === "NONE"}
              onClick={() => void handleSaveDraft()}
            >
              存为记忆
            </button>
          </div>
        </div>
      ) : null}
    </section>
  );
}

function MemoryRow({
  entry,
  busy,
  editId,
  editTitle,
  editText,
  onEditTitle,
  onEditText,
  onEdit,
  onCancelEdit,
  onSaveEdit,
  onToggle,
  onDelete,
}: {
  entry: MemoryEntry;
  busy: boolean;
  editId: string | null;
  editTitle: string;
  editText: string;
  onEditTitle: (v: string) => void;
  onEditText: (v: string) => void;
  onEdit: () => void;
  onCancelEdit: () => void;
  onSaveEdit: () => void;
  onToggle: (enabled: boolean) => void;
  onDelete: () => void;
}) {
  const editing = editId === entry.id;
  if (editing) {
    return (
      <li>
        <div className="panel-form">
          <input
            className="panel-form__input"
            value={editTitle}
            onChange={(e) => onEditTitle(e.target.value)}
          />
          <textarea
            className="panel-form__input panel-form__input--area"
            value={editText}
            onChange={(e) => onEditText(e.target.value)}
          />
          <div className="panel-form__actions">
            <button className="pill-action" type="button" onClick={onCancelEdit}>
              取消
            </button>
            <button className="pill-action" type="button" disabled={busy} onClick={onSaveEdit}>
              保存
            </button>
          </div>
        </div>
      </li>
    );
  }

  const excerpt = entry.text.trim().split(/\r?\n/).find((l) => l.trim()) ?? "";
  return (
    <li>
      <label
        className="context-lend__row"
        title={entry.text}
      >
        <input
          type="checkbox"
          checked={entry.enabled}
          disabled={busy}
          onChange={(e) => onToggle(e.target.checked)}
        />
        <span className="context-lend__name">{entry.title || "(untitled)"}</span>
        <span className="context-lend__meta">
          {sourceLabel(entry.source)}
          {excerpt ? ` · ${excerpt.slice(0, 48)}${excerpt.length > 48 ? "…" : ""}` : ""}
        </span>
      </label>
      <div className="panel-entry__foot" style={{ paddingLeft: 22, marginTop: 2 }}>
        <button
          className="pill-action pill-action--icon pill-action--sm"
          type="button"
          title="Edit"
          aria-label={`Edit memory ${entry.title}`}
          onClick={onEdit}
        >
          <Pencil size={11} />
        </button>
        <button
          className="pill-action pill-action--icon pill-action--sm"
          type="button"
          title="Delete"
          aria-label={`Delete memory ${entry.title}`}
          disabled={busy}
          onClick={onDelete}
        >
          <Trash2 size={11} />
        </button>
      </div>
    </li>
  );
}
