import { useCallback, useEffect, useState } from "react";
import { listAgents } from "../lib/api";
import type { AgentConfig } from "../lib/types";
import {
  listScenarios,
  saveScenarios,
  type Scenario,
  type ScenarioKind,
  type ScenarioTarget,
} from "../lib/scenarios";

/**
 * Manage the orb panel's scenario buttons (main-window entry point — the orb
 * window itself only renders the buttons, never this dialog). Self-contained:
 * loads via `listScenarios()`, persists via `saveScenarios()`, and fetches the
 * agent list itself for the agent dropdown.
 *
 * Builtin scenarios (记录收藏 / 新对话) are re-merged from code on every load,
 * so they can be neither edited nor deleted — the Rust side drops stored rows
 * whose id starts with `builtin-`.
 */

export type ScenarioDialogProps = {
  onClose: () => void;
};

const KIND_LABELS: Record<ScenarioKind, string> = {
  capture: "收藏整理",
  prompt: "快捷指令",
};

const TARGET_LABELS: Record<ScenarioTarget, string> = {
  inbox: "收集箱",
  newChat: "新对话",
};

type ScenarioDraft = {
  /** Existing scenario id while editing; null while creating. */
  editingId: string | null;
  name: string;
  emoji: string;
  kind: ScenarioKind;
  agentId: string;
  modelId: string;
  modeId: string;
  promptTemplate: string;
  target: ScenarioTarget;
};

const NEW_DRAFT: ScenarioDraft = {
  editingId: null,
  name: "",
  emoji: "⭐",
  kind: "capture",
  agentId: "",
  modelId: "",
  modeId: "",
  promptTemplate: "",
  target: "inbox",
};

function draftFromScenario(s: Scenario): ScenarioDraft {
  return {
    editingId: s.id,
    name: s.name,
    emoji: s.emoji,
    kind: s.kind,
    agentId: s.agentId ?? "",
    modelId: s.modelId ?? "",
    modeId: s.modeId ?? "",
    promptTemplate: s.promptTemplate ?? "",
    target: s.target,
  };
}

/** Unique, slug-ish id for a new scenario (never collides with `builtin-`). */
function newScenarioId(name: string): string {
  const slug =
    name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9_-]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 48) || "scenario";
  return `custom-${slug}-${Date.now().toString(36).slice(-4)}`;
}

export function ScenarioDialog({ onClose }: ScenarioDialogProps) {
  /** Null while loading; empty array only in pathological cases (builtins merge in). */
  const [scenarios, setScenarios] = useState<Scenario[] | null>(null);
  const [agents, setAgents] = useState<AgentConfig[]>([]);
  const [draft, setDraft] = useState<ScenarioDraft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setScenarios(await listScenarios());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setScenarios([]);
    }
  }, []);

  useEffect(() => {
    void reload();
    void listAgents()
      .then((list) => setAgents(Array.isArray(list) ? list : []))
      .catch(() => setAgents([]));
  }, [reload]);

  const saveDraft = async () => {
    if (!draft) return;
    const name = draft.name.trim();
    if (!name) {
      setError("名称要填。");
      return;
    }
    const id = draft.editingId ?? newScenarioId(name);
    const next: Scenario = {
      id,
      name,
      emoji: draft.emoji.trim().slice(0, 8) || "⭐",
      kind: draft.kind,
      target: draft.target,
      ...(draft.agentId ? { agentId: draft.agentId } : {}),
      ...(draft.modelId.trim() ? { modelId: draft.modelId.trim() } : {}),
      ...(draft.modeId.trim() ? { modeId: draft.modeId.trim() } : {}),
      ...(draft.promptTemplate.trim() ? { promptTemplate: draft.promptTemplate } : {}),
    };
    setBusy(true);
    setError(null);
    try {
      const list = (scenarios ?? []).map((s) => (s.id === id ? next : s));
      if (!list.some((s) => s.id === id)) list.push(next);
      setScenarios(await saveScenarios(list));
      setDraft(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const removeScenario = async (scenario: Scenario) => {
    if (scenario.builtin || scenario.id.startsWith("builtin-")) return;
    setBusy(true);
    setError(null);
    try {
      setScenarios(
        await saveScenarios((scenarios ?? []).filter((s) => s.id !== scenario.id))
      );
      if (draft?.editingId === scenario.id) setDraft(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const setDraftField = <K extends keyof ScenarioDraft>(key: K, value: ScenarioDraft[K]) => {
    setDraft((d) => (d ? { ...d, [key]: value } : d));
  };

  return (
    <div className="project-dialog-backdrop" onClick={onClose}>
      <div
        className="project-dialog scenario-dialog"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-labelledby="scenario-dialog-title"
      >
        <h2 id="scenario-dialog-title">快捷场景</h2>
        <p className="custom-agent-dialog__lead">
          场景是悬浮球面板上的一键按钮：收藏整理把内容送进收集箱，快捷指令按模板执行。
        </p>

        {error && (
          <p className="custom-agent-dialog__error">
            {error}{" "}
            <button type="button" onClick={() => void reload()}>
              重试
            </button>
          </p>
        )}

        <div className="scenario-dialog__list custom-scrollbar">
          {scenarios == null && <p className="custom-agent-dialog__hint">正在读取场景…</p>}
          {(scenarios ?? []).map((s) => (
            <div key={s.id} className="scenario-dialog__row">
              <span className="scenario-dialog__emoji" aria-hidden="true">
                {s.emoji || "⭐"}
              </span>
              <div className="scenario-dialog__meta">
                <strong>{s.name || "(未命名)"}</strong>
                <span className="scenario-dialog__badges">
                  <i className="scenario-dialog__badge">{KIND_LABELS[s.kind]}</i>
                  <i className="scenario-dialog__badge">{TARGET_LABELS[s.target]}</i>
                  {s.agentId ? <i className="scenario-dialog__badge">{s.agentId}</i> : null}
                  {s.builtin ? (
                    <i className="scenario-dialog__badge scenario-dialog__badge--builtin">内置</i>
                  ) : null}
                </span>
              </div>
              {s.builtin ? null : (
                <div className="scenario-dialog__row-actions">
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => {
                      setDraft(draftFromScenario(s));
                      setError(null);
                    }}
                  >
                    编辑
                  </button>
                  <button
                    type="button"
                    className="scenario-dialog__delete"
                    disabled={busy}
                    onClick={() => void removeScenario(s)}
                  >
                    删除
                  </button>
                </div>
              )}
            </div>
          ))}
        </div>

        {draft ? (
          <div className="scenario-dialog__form">
            <div className="scenario-dialog__form-row">
              <label>
                emoji
                <input
                  value={draft.emoji}
                  maxLength={8}
                  onChange={(e) => setDraftField("emoji", e.target.value)}
                  placeholder="⭐"
                />
              </label>
              <label>
                名称
                <input
                  value={draft.name}
                  onChange={(e) => setDraftField("name", e.target.value)}
                  placeholder="例如：归档到 Notion"
                />
              </label>
            </div>
            <div className="scenario-dialog__form-row">
              <label>
                类型
                <select
                  value={draft.kind}
                  onChange={(e) => setDraftField("kind", e.target.value as ScenarioKind)}
                >
                  <option value="capture">收藏整理</option>
                  <option value="prompt">快捷指令</option>
                </select>
              </label>
              <label>
                目标
                <select
                  value={draft.target}
                  onChange={(e) => setDraftField("target", e.target.value as ScenarioTarget)}
                >
                  <option value="inbox">收集箱</option>
                  <option value="newChat">新对话</option>
                </select>
              </label>
            </div>
            <label>
              agent
              <select
                value={draft.agentId}
                onChange={(e) => setDraftField("agentId", e.target.value)}
              >
                <option value="">默认</option>
                {agents.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.label || a.id}
                  </option>
                ))}
              </select>
            </label>
            <div className="scenario-dialog__form-row">
              <label>
                模型（可选）
                <input
                  value={draft.modelId}
                  onChange={(e) => setDraftField("modelId", e.target.value)}
                />
              </label>
              <label>
                模式（可选）
                <input
                  value={draft.modeId}
                  onChange={(e) => setDraftField("modeId", e.target.value)}
                />
              </label>
            </div>
            <label>
              提示词模板
              <textarea
                className="scenario-dialog__textarea"
                rows={5}
                value={draft.promptTemplate}
                onChange={(e) => setDraftField("promptTemplate", e.target.value)}
                placeholder={
                  draft.kind === "capture"
                    ? "留空使用内置整理模板"
                    : "用 {content} 占位，例如：把下面的内容归档：\n{content}"
                }
              />
              <span className="scenario-dialog__hint">
                用 {"{content}"} 占位；收藏整理类型留空使用内置整理模板。
              </span>
            </label>
            <div className="scenario-dialog__form-actions">
              <button
                type="button"
                className="project-dialog__cancel"
                disabled={busy}
                onClick={() => setDraft(null)}
              >
                取消
              </button>
              <button
                type="button"
                className="project-dialog__submit"
                disabled={busy}
                onClick={() => void saveDraft()}
              >
                保存
              </button>
            </div>
          </div>
        ) : (
          <div className="scenario-dialog__form-actions scenario-dialog__form-actions--footer">
            <p className="scenario-dialog__hint">内置场景不可编辑；自定义场景会在悬浮球面板上同步出现。</p>
            <button
              type="button"
              className="project-dialog__submit"
              disabled={busy}
              onClick={() => {
                setDraft({ ...NEW_DRAFT });
                setError(null);
              }}
            >
              新建场景
            </button>
          </div>
        )}

        <div className="project-dialog__actions">
          <button type="button" className="project-dialog__cancel" onClick={onClose}>
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}
