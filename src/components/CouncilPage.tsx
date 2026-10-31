/**
 * 圆桌会议主视图(任务书 §5):设置 / 讨论时间线 / 计划门 / 历史。
 *
 * 挂载方式对齐 MemoBox 的伪标签页模式(App.tsx 条件渲染,关闭由挂载
 * 点的铬负责,Esc 关闭)。状态全部由后端事件驱动(`council-event`),
 * UI 不持有编排真相——只是 RunDetail 的渲染(任务书 §3.5)。
 */

import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ChevronLeft, CircleStop, Pause, Play, Plus, Trash2 } from "lucide-react";
import { MarkdownBody } from "./MarkdownBody";
import {
  COUNCIL_EVENT,
  councilApprovePlan,
  councilCancelRun,
  councilCreate,
  councilDeleteRun,
  councilGetRun,
  councilListModels,
  councilListRuns,
  councilPauseRun,
  councilRail,
  councilRailStatus,
  councilResumeRun,
  deriveMemberId,
  groupTimeline,
  isTerminalPhase,
  kindLabel,
  memberColor,
  memberLabel,
  memberModelLabel,
  memberStatusText,
  phaseBadgeClass,
  phaseLabel,
  railLabel,
  type CouncilMessage,
  type CouncilMessageKind,
  type CouncilMode,
  type CouncilModelInfo,
  type MemberSpec,
  type RunDetail,
  type RunPhase,
  type RunSummary,
} from "../lib/council";
import { isTauriRuntime, listAgents } from "../lib/api";
import { t as tr, useT } from "../lib/i18n";
import { showToast } from "../lib/toast";
import { taskStatusLabel, type AgentLite } from "../lib/council";

// ---------------------------------------------------------------------------
// 成员草稿(setup 表单的编辑态)
// ---------------------------------------------------------------------------

interface MemberDraft extends MemberSpec {
  key: string;
}

/** 建议角色键序(name/duty 双语都在 fe-council 片段)。 */
const ROLE_KEYS: ReadonlyArray<string> = [
  "architect",
  "reviewer",
  "analyst",
  "tester",
  "researcher",
  "verifier",
];

function roleName(key: string): string {
  return tr(`council.role.${key}.name` as Parameters<typeof tr>[0]);
}

function roleDuty(key: string): string {
  return tr(`council.role.${key}.duty` as Parameters<typeof tr>[0]);
}

function defaultDrafts(): MemberDraft[] {
  return [
    { key: "m1", id: "architect", displayName: roleName("architect"), model: "", role: roleDuty("architect"), isChair: false },
    { key: "m2", id: "reviewer", displayName: roleName("reviewer"), model: "", role: roleDuty("reviewer"), isChair: true },
    { key: "m3", id: "analyst", displayName: roleName("analyst"), model: "", role: roleDuty("analyst"), isChair: false },
  ];
}

const MODE_OPTIONS: ReadonlyArray<{ id: CouncilMode }> = [
  { id: "council_only" },
  { id: "council_and_execution" },
  { id: "autonomous" },
];

function modeLabel(id: CouncilMode): string {
  return tr(`council.setup.mode.${id}` as Parameters<typeof tr>[0]);
}

function modeHint(id: CouncilMode): string {
  return tr(`council.setup.mode.${id}.hint` as Parameters<typeof tr>[0]);
}

function seatColor(index: number): string {
  return `var(--council-m${(index % 8) + 1})`;
}

function pickUnusedModel(models: CouncilModelInfo[], taken: Iterable<string>): string {
  const used = new Set(taken);
  return models.find((m) => !used.has(m.id))?.id ?? models[0]?.id ?? "";
}

function FlowHint() {
  const t = useT();
  const steps = t("council.setup.flow").split("→").map((s) => s.trim());
  return (
    <div className="council-flow" aria-hidden>
      {steps.map((step, i) => (
        <span key={step}>
          {i > 0 && <span className="council-flow__sep">→</span>}
          {step}
        </span>
      ))}
    </div>
  );
}

function PhaseRail(props: { phase: RunPhase; pausedFrom?: RunPhase | null; mode: CouncilMode }) {
  const { phase, pausedFrom, mode } = props;
  const rail = councilRail(mode);
  return (
    <ol className="council-rail" aria-label={phaseLabel(phase)}>
      {rail.map((step) => {
        const status = councilRailStatus(phase, pausedFrom, step, rail);
        return (
          <li
            key={step}
            className={`council-rail__step is-${status}`}
            aria-current={status === "current" ? "step" : undefined}
          >
            <span className="council-rail__dot" />
            <span className="council-rail__label">{railLabel(step)}</span>
            <span className="council-rail__line" />
          </li>
        );
      })}
    </ol>
  );
}

// ---------------------------------------------------------------------------
// 主组件
// ---------------------------------------------------------------------------

interface CouncilPageProps {
  onClose: () => void;
}

type View = { kind: "list" } | { kind: "setup"; prefill?: string } | { kind: "run"; runId: string };

export function CouncilPage({ onClose }: CouncilPageProps) {
  const t = useT();
  const [view, setView] = useState<View>({ kind: "list" });
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const detailRef = useRef<RunDetail | null>(null);
  detailRef.current = detail;

  const refreshRuns = useCallback(async () => {
    setRuns(await councilListRuns());
  }, []);

  const openRun = useCallback(async (runId: string) => {
    setView({ kind: "run", runId });
    setDetail(await councilGetRun(runId));
  }, []);

  // Esc 关闭(与 memo 页同约定)。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  useEffect(() => {
    void refreshRuns();
  }, [refreshRuns]);

  // 事件驱动:后端全局 council-event → 当前 run 重拉(全量,M1 量级
  // <百条消息,简单可靠);list 视图时刷新列表。
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let un: UnlistenFn | undefined;
    void listen<{ runId: string; kind: string }>(COUNCIL_EVENT, (event) => {
      const payload = event.payload;
      if (view.kind === "run" && payload.runId === view.runId) {
        void councilGetRun(view.runId).then((d) => setDetail(d));
      } else if (view.kind === "list") {
        void refreshRuns();
      }
    }).then((fn) => {
      un = fn;
    });
    return () => un?.();
  }, [view, refreshRuns]);

  return (
    <div className="council-page">
      {view.kind === "list" && (
        <CouncilListView
          runs={runs}
          onNew={(prefill) => setView({ kind: "setup", prefill })}
          onOpen={openRun}
          onRefresh={refreshRuns}
          onDelete={async (runId) => {
            try {
              await councilCancelRun(runId);
            } catch {
              /* 非运行态取消会落盘失败——继续删 */
            }
            try {
              await councilDeleteRun(runId);
              await refreshRuns();
            } catch (e) {
              showToast({ text: tr("council.error.deleteFailed", { error: String(e) }), tone: "error" });
            }
          }}
        />
      )}
      {view.kind === "setup" && (
        <CouncilSetupView
          prefill={view.prefill}
          onBack={() => setView({ kind: "list" })}
          onCreated={async (runId) => {
            await refreshRuns();
            await openRun(runId);
          }}
        />
      )}
      {view.kind === "run" && detail && (
        <CouncilRunView
          detail={detail}
          onBack={async () => {
            await refreshRuns();
            setView({ kind: "list" });
          }}
          onReload={async () => setDetail(await councilGetRun(view.runId))}
        />
      )}
      {view.kind === "run" && !detail && (
        <div className="council-frame">
          <button type="button" className="council-back" onClick={() => setView({ kind: "list" })}>
            <ChevronLeft size={14} /> {t("council.run.back")}
          </button>
          <p className="council-sub">{t("council.list.missing")}</p>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// 列表视图
// ---------------------------------------------------------------------------

function CouncilListView(props: {
  runs: RunSummary[];
  onNew: (prefill?: string) => void;
  onOpen: (runId: string) => void;
  onRefresh: () => void;
  onDelete: (runId: string) => void;
}) {
  const { runs, onNew, onOpen, onRefresh, onDelete } = props;
  const t = useT();
  return (
    <div className={`council-list${runs.length === 0 ? " is-empty" : ""}`}>
      <div className="council-frame">
        {runs.length > 0 && (
          <div className="council-list__head">
            <div>
              <h2>{t("council.list.title")}</h2>
              <p className="council-sub">{t("council.list.subtitle")}</p>
              <FlowHint />
            </div>
            <button type="button" className="council-btn council-btn--primary" onClick={() => onNew()}>
              <Plus size={14} /> {t("council.list.new")}
            </button>
          </div>
        )}
        {runs.length === 0 ? (
          <div className="council-empty">
            <strong>{t("council.list.emptyTitle")}</strong>
            <p>{t("council.list.empty")}</p>
            <FlowHint />
            <button type="button" className="council-btn council-btn--primary" onClick={() => onNew()}>
              <Plus size={14} /> {t("council.list.new")}
            </button>
          </div>
        ) : (
          <div className="council-run-rows">
            {runs.map((run) => {
              const live = !isTerminalPhase(run.phase) && run.phase !== "plan_ready" && run.phase !== "paused";
              return (
                <div key={run.runId} className={`council-run-row${live ? " is-live" : ""}`}>
                  <button type="button" className="council-run-row__main" onClick={() => onOpen(run.runId)}>
                    <span className="council-run-row__title">{run.title || t("council.list.untitled")}</span>
                    <span className="council-run-row__meta">
                      {t("council.list.runMeta", { phase: phaseLabel(run.phase), members: run.memberCount, rounds: run.rounds, messages: run.messageCount })}
                    </span>
                  </button>
                  <span className={phaseBadgeClass(run.phase)}>
                    {phaseLabel(run.phase)}
                  </span>
                  <button type="button" className="council-icon-btn" title={t("council.list.delete")} onClick={() => onDelete(run.runId)}>
                    <Trash2 size={14} />
                  </button>
                </div>
              );
            })}
          </div>
        )}
        {runs.length > 0 && (
          <button type="button" className="council-link-btn" onClick={onRefresh}>
            {t("council.list.refresh")}
          </button>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// 设置视图
// ---------------------------------------------------------------------------

function CouncilSetupView(props: { prefill?: string; onBack: () => void; onCreated: (runId: string) => void }) {
  const { prefill, onBack, onCreated } = props;
  const [title, setTitle] = useState("");
  const [task, setTask] = useState(prefill ?? "");
  const [mode, setMode] = useState<CouncilMode>("council_only");
  const [drafts, setDrafts] = useState<MemberDraft[]>(defaultDrafts());
  const [rounds, setRounds] = useState(2);
  const [maxConcurrent, setMaxConcurrent] = useState(4);
  const [maxRequests, setMaxRequests] = useState<string>("60");
  const [models, setModels] = useState<CouncilModelInfo[]>([]);
  const [engineReady, setEngineReady] = useState(true);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [agents, setAgents] = useState<AgentLite[]>([]);
  const [execAgentId, setExecAgentId] = useState("");
  const [execCwd, setExecCwd] = useState("");
  const t = useT();

  useEffect(() => {
    void councilListModels().then((result) => {
      setModels(result.models);
      setEngineReady(result.ready);
    });
    void listAgents().then((all) => {
      const usable = all
        .filter((a) => a.enabled && a.id !== "web-chatgpt" && a.id !== "web-deepseek")
        .map((a) => ({ id: a.id, label: a.label }));
      setAgents(usable);
      setExecAgentId((prev) => prev || usable[0]?.id || "");
    });
  }, []);

  useEffect(() => {
    if (models.length === 0) return;
    setDrafts((prev) => {
      const taken = new Set(prev.map((d) => d.model).filter(Boolean));
      let changed = false;
      const next = prev.map((d) => {
        if (d.model) return d;
        const pick = pickUnusedModel(models, taken);
        if (!pick) return d;
        taken.add(pick);
        changed = true;
        return { ...d, model: pick };
      });
      return changed ? next : prev;
    });
  }, [models]);

  const updateDraft = (key: string, patch: Partial<MemberDraft>) => {
    setDrafts((prev) => prev.map((d) => (d.key === key ? { ...d, ...patch } : d)));
  };

  const setChair = (key: string) => {
    setDrafts((prev) => prev.map((d) => ({ ...d, isChair: d.key === key })));
  };

  const addMember = () => {
    setDrafts((prev) => {
      const n = prev.length + 1;
      const key = ROLE_KEYS[(n - 1) % ROLE_KEYS.length] ?? "reviewer";
      const name = roleName(key);
      return [
        ...prev,
        {
          key: `m${n}-${Date.now()}`,
          id: deriveMemberId(name, prev.map((d) => d.id)),
          displayName: name,
          model: pickUnusedModel(models, prev.map((d) => d.model)),
          role: roleDuty(key),
          isChair: false,
        },
      ];
    });
  };

  const removeMember = (key: string) => {
    setDrafts((prev) => {
      const next = prev.filter((d) => d.key !== key);
      // 删掉主席时把第一个非主席升为主席,保持恰好一个。
      const first = next[0];
      if (!next.some((d) => d.isChair) && first) next[0] = { ...first, isChair: true };
      return next;
    });
  };

  const distinctModels = useMemo(() => new Set(drafts.map((d) => d.model).filter(Boolean)).size, [drafts]);

  const submit = async () => {
    setError(null);
    if (task.trim().length < 10) {
      setError(t("council.setup.taskTooShort"));
      return;
    }
    if (drafts.some((d) => !d.model)) {
      setError(t("council.setup.noModel"));
      return;
    }
    setCreating(true);
    try {
      const detail = await councilCreate({
        title: title.trim() || task.trim().slice(0, 40),
        task: task.trim(),
        mode,
        members: drafts.map(({ key, ...rest }) => {
          void key;
          return rest;
        }),
        rounds,
        maxConcurrent,
        budget: {
          maxRequests: maxRequests.trim() ? Number(maxRequests) : null,
          maxTokens: null,
          wallClockSecs: null,
        },
        projectId: null,
        cwd: null,
        executor:
          mode !== "council_only" && execAgentId && execCwd.trim()
            ? { agentId: execAgentId, cwd: execCwd.trim(), projectId: null }
            : null,
      });
      if (detail?.spec?.runId) {
        onCreated(detail.spec.runId);
      } else {
        setError(t("council.setup.createFailed"));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(false);
    }
  };

  return (
    <div className="council-setup">
      <div className="council-frame council-frame--form">
      <button type="button" className="council-back" onClick={onBack}>
        <ChevronLeft size={14} /> {t("council.setup.back")}
      </button>
      <h2>{t("council.setup.title")}</h2>
      <FlowHint />

      {!engineReady && (
        <div className="council-warn">{t("council.setup.engineNotReady")}</div>
      )}

      <section className="council-section">
        <h3 className="council-section__title">{t("council.setup.section.topic")}</h3>
        <label className="council-field">
          <span>{t("council.setup.titleLabel")}</span>
          <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder={t("council.setup.titlePlaceholder")} />
        </label>

        <label className="council-field">
          <span>{t("council.setup.taskLabel")}</span>
          <textarea
            value={task}
            onChange={(e) => setTask(e.target.value)}
            rows={6}
            placeholder={t("council.setup.taskPlaceholder")}
          />
        </label>

        <div className="council-field">
          <span>{t("council.setup.modeLabel")}</span>
          <div className="council-modes">
            {MODE_OPTIONS.map((opt) => (
              <button
                key={opt.id}
                type="button"
                className={`council-mode ${mode === opt.id ? "is-active" : ""}`}
                onClick={() => setMode(opt.id)}
              >
                <strong>{modeLabel(opt.id)}</strong>
                <span>{modeHint(opt.id)}</span>
              </button>
            ))}
          </div>
        </div>

        {mode !== "council_only" && (
          <div className="council-field">
            <span>{t("council.setup.executorLabel")}</span>
            <div className="council-exec-row">
              <select value={execAgentId} onChange={(e) => setExecAgentId(e.target.value)}>
                {agents.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.label}
                  </option>
                ))}
              </select>
              <input
                value={execCwd}
                onChange={(e) => setExecCwd(e.target.value)}
                placeholder={t("council.setup.executorCwdPlaceholder")}
              />
            </div>
            <p className="council-sub">{t("council.setup.executorHint")}</p>
          </div>
        )}
      </section>

      <section className="council-section">
        <h3 className="council-section__title">{t("council.setup.section.seats")}</h3>
        <div className="council-field">
          <span>
            {t("council.setup.membersLabel", { count: drafts.length, models: distinctModels })}
            {distinctModels < Math.min(3, drafts.length) ? t("council.setup.membersFewModels") : ""}
          </span>
          <div className="council-members">
            {drafts.map((draft, index) => (
              <div key={draft.key} className="council-member">
                <div className="council-member__row">
                  <span className="council-member__seat" style={{ background: seatColor(index) }} />
                  <input
                    value={draft.displayName}
                    onChange={(e) => updateDraft(draft.key, { displayName: e.target.value })}
                    placeholder={t("council.setup.memberNamePlaceholder")}
                  />
                  <button
                    type="button"
                    className={`council-chair-toggle ${draft.isChair ? "is-chair" : ""}`}
                    onClick={() => setChair(draft.key)}
                    title={draft.isChair ? t("council.setup.memberChairTitle") : t("council.setup.memberSetChair")}
                  >
                    {draft.isChair ? t("council.setup.memberChair") : t("council.setup.memberChairOff")}
                  </button>
                  <button
                    type="button"
                    className="council-icon-btn"
                    onClick={() => removeMember(draft.key)}
                    disabled={drafts.length <= 2}
                    title={t("council.setup.memberRemove")}
                  >
                    <Trash2 size={13} />
                  </button>
                </div>
                <select value={draft.model} onChange={(e) => updateDraft(draft.key, { model: e.target.value })}>
                  <option value="">{t("council.setup.memberModelPlaceholder")}</option>
                  {models.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.id}
                    </option>
                  ))}
                </select>
                <input
                  value={draft.role}
                  onChange={(e) => updateDraft(draft.key, { role: e.target.value })}
                  placeholder={t("council.setup.memberRolePlaceholder")}
                />
              </div>
            ))}
          </div>
          <button type="button" className="council-link-btn" onClick={addMember} disabled={drafts.length >= 8}>
            {t("council.setup.addMember")}
          </button>
        </div>
      </section>

      <section className="council-section">
        <h3 className="council-section__title">{t("council.setup.section.rules")}</h3>
        <div className="council-params">
          <label className="council-field council-field--inline">
            <span>{t("council.setup.roundsLabel")}</span>
            <input
              type="number"
              min={1}
              max={4}
              value={rounds}
              onChange={(e) => setRounds(Math.max(1, Math.min(4, Number(e.target.value) || 1)))}
            />
          </label>
          <label className="council-field council-field--inline">
            <span>{t("council.setup.concurrencyLabel")}</span>
            <input
              type="number"
              min={1}
              max={8}
              value={maxConcurrent}
              onChange={(e) => setMaxConcurrent(Math.max(1, Math.min(8, Number(e.target.value) || 1)))}
            />
          </label>
          <label className="council-field council-field--inline">
            <span>{t("council.setup.budgetLabel")}</span>
            <input value={maxRequests} onChange={(e) => setMaxRequests(e.target.value.replace(/\D/g, ""))} placeholder="60" />
          </label>
        </div>
      </section>

      {error && <div className="council-warn council-warn--error">{error}</div>}

      <div className="council-setup__actions">
        <button type="button" className="council-btn council-btn--primary" disabled={creating || !engineReady} onClick={submit}>
          {creating ? t("council.setup.starting") : t("council.setup.start")}
        </button>
      </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// 运行视图
// ---------------------------------------------------------------------------

function CouncilRunView(props: { detail: RunDetail; onBack: () => void; onReload: () => void }) {
  const { detail, onBack, onReload } = props;
  const { spec, state, messages, plan } = detail;
  const t = useT();
  const groups = useMemo(() => groupTimeline(messages), [messages]);
  const running = state.phase !== "paused" && !isTerminalPhase(state.phase) && state.phase !== "plan_ready";
  const seenRef = useRef<Set<string> | null>(null);
  const [arriving, setArriving] = useState<Set<string>>(() => new Set());
  const planRef = useRef<HTMLElement>(null);
  const sawPlanReady = useRef(false);

  useEffect(() => {
    const ids = messages.map((m) => m.messageId);
    if (!seenRef.current) {
      seenRef.current = new Set(ids);
      return;
    }
    const fresh = new Set<string>();
    for (const id of ids) {
      if (!seenRef.current.has(id)) fresh.add(id);
    }
    seenRef.current = new Set(ids);
    setArriving(fresh);
  }, [messages]);

  useEffect(() => {
    if (state.phase !== "plan_ready") {
      sawPlanReady.current = false;
      return;
    }
    if (sawPlanReady.current) return;
    sawPlanReady.current = true;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    planRef.current?.scrollIntoView({ behavior: reduce ? "auto" : "smooth", block: "start" });
  }, [state.phase]);

  const control = async (action: "pause" | "resume" | "cancel" | "accept" | "reject") => {
    try {
      if (action === "pause") await councilPauseRun(spec.runId);
      if (action === "resume") await councilResumeRun(spec.runId);
      if (action === "cancel") await councilCancelRun(spec.runId);
      if (action === "accept") await councilApprovePlan(spec.runId, true);
      if (action === "reject") await councilApprovePlan(spec.runId, false);
      onReload();
    } catch (e) {
      showToast({ text: String(e), tone: "error" });
    }
  };

  const budgetLimit = spec.budget.maxRequests ?? 0;
  const budgetPct = budgetLimit > 0 ? Math.min(100, (state.budgetUsage.requests / budgetLimit) * 100) : 0;

  return (
    <div className="council-run">
      <div className="council-run__head">
        <div className="council-frame council-frame--wide">
          <div className="council-run__head-row">
            <button type="button" className="council-back" onClick={onBack}>
              <ChevronLeft size={14} /> {t("council.run.back")}
            </button>
            <div className="council-run__title">
              <strong>{spec.title}</strong>
              <span className={phaseBadgeClass(state.phase)}>
                {phaseLabel(state.phase)}
                {state.round > 1 ? ` · ${t("council.run.roundBadge", { n: state.round })}` : ""}
              </span>
              {detail.live && (
                <span className={`council-badge council-badge--live${running ? " is-pulse" : ""}`}>
                  {t("council.run.live")}
                </span>
              )}
            </div>
            <div className="council-run__controls">
              {state.phase === "paused" && (
                <button type="button" className="council-btn" onClick={() => control("resume")}>
                  <Play size={13} /> {t("council.run.resume")}
                </button>
              )}
              {running && (
                <button type="button" className="council-btn" onClick={() => control("pause")}>
                  <Pause size={13} /> {t("council.run.pause")}
                </button>
              )}
              {!isTerminalPhase(state.phase) && (
                <button type="button" className="council-btn council-btn--danger" onClick={() => control("cancel")}>
                  <CircleStop size={13} /> {t("council.run.cancel")}
                </button>
              )}
            </div>
          </div>
          <PhaseRail phase={state.phase} pausedFrom={state.pausedFrom} mode={spec.mode} />
        </div>
      </div>

      <div className="council-frame council-frame--wide">
      {state.error && <div className="council-warn council-warn--error">{state.error}</div>}

      <div className="council-run__members">
        {spec.members.map((member) => {
          const status = state.memberStatus[member.id];
          const working = status === "working";
          return (
            <div
              key={member.id}
              className={`council-member-chip${working ? " is-working" : ""}${member.isChair ? " is-chair" : ""}`}
            >
              <span className="council-member-chip__dot" style={{ background: memberColor(spec.members, member.id) }} />
              <span className="council-member-chip__name">{member.displayName}</span>
              {member.isChair ? <span className="council-member-chip__chair">{t("council.setup.memberChair")}</span> : null}
              <span className="council-member-chip__model">{member.model}</span>
              <span className="council-member-chip__status">{memberStatusText(status)}</span>
            </div>
          );
        })}
        <div className="council-budget">
          <span>
            {t("council.run.budget", { requests: state.budgetUsage.requests, tokens: state.budgetUsage.tokensEst.toLocaleString() })}
            {spec.budget.maxRequests ? t("council.run.budgetLimit", { limit: spec.budget.maxRequests }) : ""}
          </span>
          {budgetLimit > 0 && (
            <span className="council-budget__bar" aria-hidden>
              <span style={{ width: `${budgetPct}%` }} />
            </span>
          )}
        </div>
      </div>

      {state.tasks.length > 0 && (
        <section className="council-tasks">
          <h3 className="council-tgroup__title">{t("council.run.tasksTitle", { count: state.tasks.length })}</h3>
          {state.tasks.map((task) => (
            <div key={task.stepId} className="council-task">
              <span className="council-task__id">{task.stepId}</span>
              <span className="council-task__title">{task.title}</span>
              <span className={`council-task__status council-task__status--${typeof task.status === "string" ? task.status : "failed"}`}>
                {taskStatusLabel(task.status)}
              </span>
              {task.changes.length > 0 && (
                <span className="council-task__changes" title={task.changes.join(" · ")}>
                  {t("council.run.taskFiles", { count: task.changes.length })}
                </span>
              )}
            </div>
          ))}
        </section>
      )}

      {plan && (
        <CouncilPlanView
          plan={plan}
          phase={state.phase}
          onApprove={control}
          planRef={planRef}
        />
      )}

      <div className="council-timeline">
        {groups.map((group) => (
          <section key={group.key} className="council-tgroup">
            <h3 className="council-tgroup__title">{group.title}</h3>
            {group.messages.map((msg) => (
              <CouncilMessageCard
                key={msg.messageId}
                msg={msg}
                spec={spec}
                arriving={arriving.has(msg.messageId)}
              />
            ))}
          </section>
        ))}
        {messages.length === 0 && (
          <div className="council-empty">
            <p>{t("council.run.timelineEmpty")}</p>
          </div>
        )}
      </div>
      </div>
    </div>
  );
}

function CouncilMessageCard(props: { msg: CouncilMessage; spec: RunDetail["spec"]; arriving?: boolean }) {
  const { msg, spec, arriving } = props;
  const color = memberColor(spec.members, msg.from);
  const isSystem = msg.from === "system";
  const kind = msg.kind as CouncilMessageKind;
  if (isSystem || kind === "status") {
    return (
      <div className={`council-msg council-msg--system${arriving ? " is-arriving" : ""}`}>
        <span>{msg.content}</span>
      </div>
    );
  }
  return (
    <article className={`council-msg${arriving ? " is-arriving" : ""}`} style={{ borderLeftColor: color }}>
      <header className="council-msg__head">
        <span className="council-msg__author" style={{ color }}>
          {memberLabel(spec.members, msg.from)}
        </span>
        <span className="council-msg__model">{memberModelLabel(spec.members, msg.from)}</span>
        <span className="council-msg__kind">{kindLabel(kind)}</span>
        <span className="council-msg__time">{new Date(msg.timestamp).toLocaleTimeString()}</span>
      </header>
      <div className="council-msg__body">
        <MarkdownBody text={msg.content} />
      </div>
    </article>
  );
}

// ---------------------------------------------------------------------------
// 计划视图(Plan Gate)
// ---------------------------------------------------------------------------

function CouncilPlanView(props: {
  plan: NonNullable<RunDetail["plan"]>;
  phase: RunPhase;
  onApprove: (action: "accept" | "reject") => void;
  planRef: RefObject<HTMLElement>;
}) {
  const { plan, phase, onApprove, planRef } = props;
  const t = useT();
  const gating = phase === "plan_ready";
  return (
    <section ref={planRef} className={`council-plan${gating ? " is-gate" : ""}`}>
      {gating && <p className="council-plan__banner">{t("council.run.planReady")}</p>}
      <header className="council-plan__head">
        <h3>{t("council.plan.title")}</h3>
        {gating ? (
          <div className="council-plan__actions">
            <button type="button" className="council-btn" onClick={() => onApprove("reject")}>
              {t("council.plan.reconsider")}
            </button>
            <button type="button" className="council-btn council-btn--primary" onClick={() => onApprove("accept")}>
              {t("council.plan.accept")}
            </button>
          </div>
        ) : (
          <span className="council-badge council-badge--done">{phaseLabel(phase)}</span>
        )}
      </header>
      <p className="council-plan__summary">{plan.summary}</p>
      <ol className="council-plan__steps">
        {plan.steps.map((step) => (
          <li key={step.id} className="council-plan__step">
            <div className="council-plan__step-title">
              <span className="council-plan__step-id">{step.id}</span>
              <strong>{step.title}</strong>
              {step.dependsOn.length > 0 && <span className="council-plan__dep">{t("council.plan.dependsOn", { deps: step.dependsOn.join(", ") })}</span>}
            </div>
            <div className="council-plan__step-detail">{step.detail}</div>
            <div className="council-plan__step-acceptance">{t("council.plan.acceptance", { text: step.acceptance })}</div>
          </li>
        ))}
      </ol>
      {plan.openQuestions.length > 0 && (
        <div className="council-plan__section">
          <h4>{t("council.plan.openQuestions")}</h4>
          <ul>
            {plan.openQuestions.map((q, i) => (
              <li key={i}>{q}</li>
            ))}
          </ul>
        </div>
      )}
      {plan.verification.length > 0 && (
        <div className="council-plan__section">
          <h4>{t("council.plan.verification")}</h4>
          <ul className="council-plan__verify">
            {plan.verification.map((v, i) => (
              <li key={i}>
                <code>{v}</code>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
