import { Brain, Pencil, Plus, RefreshCw, Sparkles, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import {
  memoryAdd,
  memoryDelete,
  memoryDistill,
  memoryGet,
  memoryImportPreview,
  memoryImportRun,
  memorySetEnabled,
  memorySetEntryEnabled,
  memoryUpdate,
  type MemoryEntry,
  type MemoryImportSource,
  type MemoryStore,
} from "../lib/api";

const SOURCE_LABELS: Record<string, string> = {
  manual: "手动",
  claude: "Claude",
  codex: "Codex",
  opencode: "OpenCode",
  gemini: "Gemini",
  session: "会话蒸馏",
};

function sourceLabel(source: string): string {
  return SOURCE_LABELS[source] ?? source;
}

function formatDate(ms: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  return `${d.getMonth() + 1}/${d.getDate()} ${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/**
 * 跨 agent 共享记忆面板：所有 agent 的会话首条消息自动带上这份记忆
 * （注入逻辑在 Rust 侧 memory.rs / acp.rs，这里只做管理）。
 */
export function MemorySection({ activeSessionId }: { activeSessionId?: string | null }) {
  const [store, setStore] = useState<MemoryStore | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [adding, setAdding] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [newText, setNewText] = useState("");

  const [editId, setEditId] = useState<string | null>(null);
  const [editTitle, setEditTitle] = useState("");
  const [editText, setEditText] = useState("");

  const [importSources, setImportSources] = useState<MemoryImportSource[] | null>(null);
  const [importBusy, setImportBusy] = useState(false);

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

  const run = async (label: string, action: () => Promise<void>) => {
    setBusy(true);
    try {
      await action();
      await refresh();
      setNote(`${label}完成`);
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const handleImportPreview = async () => {
    setImportBusy(true);
    try {
      const { sources } = await memoryImportPreview();
      setImportSources(sources);
      const found = sources.filter((s) => s.exists && s.bytes > 0).length;
      setNote(found > 0 ? `发现 ${found} 个已有的 agent 记忆` : "没发现任何 agent 的全局记忆文件");
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setImportBusy(false);
    }
  };

  const handleImportRun = async () => {
    setImportBusy(true);
    try {
      const result = await memoryImportRun();
      const parts: string[] = [];
      if (result.imported.length > 0) parts.push(`收编 ${result.imported.length} 份`);
      if (result.updated.length > 0) parts.push(`更新 ${result.updated.length} 份`);
      if (result.unchanged.length > 0) parts.push(`${result.unchanged.length} 份无变化`);
      setNote(parts.length > 0 ? parts.join(" · ") : "没有可收编的内容");
      setImportSources(null);
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setImportBusy(false);
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
    await run("保存", () =>
      memoryAdd(`会话要点 ${date}`, draft, "session").then(() => undefined),
    );
    setDraft(null);
  };

  const entries = store?.entries ?? [];

  return (
    <section className="context-card">
      <div className="context-card__heading">
        <span>共享记忆 · Memory</span>
        <div className="context-card__heading-actions">
          <button
            className="pill-action pill-action--icon pill-action--sm"
            type="button"
            title="刷新记忆"
            aria-label="Refresh memory"
            onClick={() => void refresh()}
          >
            <RefreshCw size={11} />
          </button>
        </div>
      </div>

      <label className="panel-toggle" title="关闭后新会话不再注入记忆卡">
        <input
          type="checkbox"
          checked={store?.enabled ?? false}
          disabled={!store || busy}
          onChange={(e) => void run("切换", () => memorySetEnabled(e.target.checked))}
        />
        新会话自动携带记忆（所有 agent 共享）
      </label>

      {note ? (
        <p className="panel-note" style={{ whiteSpace: "pre-wrap" }}>
          {note}
        </p>
      ) : null}

      {entries.length === 0 ? (
        <p className="context-card__empty" style={{ marginTop: 10 }}>
          <Brain size={11} style={{ verticalAlign: -1, marginRight: 4 }} />
          还没有记忆。可以收编各 agent 已存的记忆，或手动添加。
        </p>
      ) : (
        <ul className="panel-entry-list">
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
                void run("保存", async () => {
                  await memoryUpdate(entry.id, editTitle, editText);
                  setEditId(null);
                })
              }
              onToggle={(enabled) =>
                void run("切换", () => memorySetEntryEnabled(entry.id, enabled))
              }
              onDelete={() => void run("删除", () => memoryDelete(entry.id))}
            />
          ))}
        </ul>
      )}

      <div className="panel-actions">
        <button
          className={adding ? "pill-action is-active" : "pill-action"}
          type="button"
          onClick={() => {
            setAdding((v) => !v);
            setNewTitle("");
            setNewText("");
          }}
        >
          <Plus size={11} />
          {adding ? "收起点添加" : "添加"}
        </button>
        <button
          className="pill-action"
          type="button"
          disabled={importBusy}
          onClick={() => void handleImportPreview()}
        >
          收编各家记忆
        </button>
        {activeSessionId ? (
          <button
            className="pill-action"
            type="button"
            disabled={draftBusy}
            title="把当前会话的要点提炼为长期记忆（用 DeepSeek，需 API key）"
            onClick={() => void handleDistill()}
          >
            <Sparkles size={11} />
            {draftBusy ? "提炼中…" : "从会话提炼"}
          </button>
        ) : null}
      </div>

      {adding ? (
        <div className="panel-form">
          <input
            className="panel-form__input"
            placeholder="标题（如：偏好与习惯）"
            value={newTitle}
            onChange={(e) => setNewTitle(e.target.value)}
          />
          <textarea
            className="panel-form__input panel-form__input--area"
            placeholder="记忆内容（markdown，会注入给所有 agent）"
            value={newText}
            onChange={(e) => setNewText(e.target.value)}
          />
          <div className="panel-form__actions">
            <button
              className="pill-action"
              type="button"
              onClick={() => setAdding(false)}
            >
              取消
            </button>
            <button
              className="pill-action"
              type="button"
              disabled={!newText.trim() || busy}
              onClick={() =>
                void run("添加", async () => {
                  await memoryAdd(newTitle, newText);
                  setAdding(false);
                })
              }
            >
              保存
            </button>
          </div>
        </div>
      ) : null}

      {importSources ? (
        <div className="panel-sub">
          <p className="panel-sub__title">
            收编 = 复制为共享条目，原文件不动；文件更新后可重新收编。
          </p>
          {importSources.map((src) => {
            const found = src.exists && src.bytes > 0;
            return (
              <div className="panel-sub__row" key={src.source}>
                <span>
                  {src.title}
                  {found ? ` · ${(src.bytes / 1024).toFixed(1)} KB` : " · 未找到"}
                </span>
                {found && src.excerpt ? (
                  <span className="panel-sub__hint" title={src.excerpt}>
                    {src.excerpt.slice(0, 60)}
                  </span>
                ) : null}
              </div>
            );
          })}
          <div className="panel-form__actions">
            <button
              className="pill-action"
              type="button"
              onClick={() => setImportSources(null)}
            >
              取消
            </button>
            <button
              className="pill-action"
              type="button"
              disabled={importBusy || !importSources.some((s) => s.exists && s.bytes > 0)}
              onClick={() => void handleImportRun()}
            >
              {importBusy ? "收编中…" : "确认收编"}
            </button>
          </div>
        </div>
      ) : null}

      {draft ? (
        <div className="panel-form">
          <p className="panel-sub__title">提炼草稿（可编辑，保存后所有 agent 共享）：</p>
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
  return (
    <li className={entry.enabled ? "panel-entry" : "panel-entry is-off"}>
      {editing ? (
        <>
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
        </>
      ) : (
        <>
          <div className="panel-entry__head">
            <input
              type="checkbox"
              checked={entry.enabled}
              disabled={busy}
              title={entry.enabled ? "注入新会话" : "已停用（不注入）"}
              onChange={(e) => onToggle(e.target.checked)}
            />
            <span className="panel-entry__title" title={entry.title}>
              {entry.title || "（无标题）"}
            </span>
            <span className="panel-entry__chip">{sourceLabel(entry.source)}</span>
          </div>
          <p className="panel-entry__text" title={entry.text}>
            {entry.text}
          </p>
          <div className="panel-entry__foot">
            <button
              className="pill-action pill-action--icon pill-action--sm"
              type="button"
              title="编辑这条记忆"
              aria-label={`Edit memory ${entry.title}`}
              onClick={onEdit}
            >
              <Pencil size={11} />
            </button>
            <button
              className="pill-action pill-action--icon pill-action--sm"
              type="button"
              title="删除这条记忆"
              aria-label={`Delete memory ${entry.title}`}
              disabled={busy}
              onClick={onDelete}
            >
              <Trash2 size={11} />
            </button>
            <span className="panel-entry__date">{formatDate(entry.updatedAtMs)}</span>
          </div>
        </>
      )}
    </li>
  );
}
