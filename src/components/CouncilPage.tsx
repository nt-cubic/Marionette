/**
 * Council Mode 主视图(任务书 §5):设置 / 讨论时间线 / 计划门 / 历史。
 *
 * 挂载方式对齐 MemoBox 的伪标签页模式(App.tsx 条件渲染,关闭由挂载
 * 点的铬负责,Esc 关闭)。状态全部由后端事件驱动(`council-event`),
 * UI 不持有编排真相——只是 RunDetail 的渲染(任务书 §3.5)。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ChevronLeft, CircleStop, Pause, Play, Plus, Trash2, Users } from "lucide-react";
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
  councilResumeRun,
  deriveMemberId,
  groupTimeline,
  isTerminalPhase,
  kindLabel,
  memberColor,
  memberLabel,
  memberModelLabel,
  memberStatusText,
  phaseLabel,
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
        <div className="council-empty">
          <button type="button" className="council-back" onClick={() => setView({ kind: "list" })}>
            <ChevronLeft size={14} /> {t("council.run.back")}
          </button>
          <p>{t("council.list.missing")}</p>
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
    <div className="council-list">
      <div className="council-list__head">
        <div>
          <h2>{t("council.list.title")}</h2>
          <p className="council-sub">{t("council.list.subtitle")}</p>
        </div>
        <button type="button" className="council-btn council-btn--primary" onClick={() => onNew()}>
          <Plus size={14} /> {t("council.list.new")}
        </button>
      </div>
      {runs.length === 0 ? (
        <div className="council-empty">
          <Users size={28} />
          <p>{t("council.list.empty")}</p>
        </div>
      ) : (
        <div className="council-run-rows">
          {runs.map((run) => (
            <div key={run.runId} className="council-run-row">
              <button type="button" className="council-run-row__main" onClick={() => onOpen(run.runId)}>
                <span className="council-run-row__title">{run.title || t("council.list.untitled")}</span>
                <span className="council-run-row__meta">
                  {t("council.list.runMeta", { phase: phaseLabel(run.phase), members: run.memberCount, rounds: run.rounds, messages: run.messageCount })}
                </span>
              </button>
              <span
                className={
                  isTerminalPhase(run.phase)
                    ? "council-badge council-badge--done"
                    : run.phase === "plan_ready"
                      ? "council-badge council-badge--plan"
                      : "council-badge"
                }
              >
                {phaseLabel(run.phase)}
              </span>
              <button type="button" className="council-icon-btn" title={t("council.list.delete")} onClick={() => onDelete(run.runId)}>
                <Trash2 size={14} />
              </button>
            </div>
          ))}
        </div>
      )}
      <button type="button" className="council-link-btn" onClick={onRefresh}>
        {t("council.list.refresh")}
      </button>
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
          model: "",
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
      <button type="button" className="council-back" onClick={onBack}>
        <ChevronLeft size={14} /> {t("council.setup.back")}
      </button>
      <h2>{t("council.setup.title")}</h2>

      {!engineReady && (
        <div className="council-warn">{t("council.setup.engineNotReady")}</div>
      )}

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
              title={modeHint(opt.id)}
            >
              {modeLabel(opt.id)}
            </button>
          ))}
        </div>
        <p className="council-sub">{modeHint(mode)}</p>
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

      <div className="council-field">
        <span>
          {t("council.setup.membersLabel", { count: drafts.length, models: distinctModels })}
          {distinctModels < Math.min(3, drafts.length) ? t("council.setup.membersFewModels") : ""}
        </span>
        <div className="council-members">
          {drafts.map((draft) => (
            <div key={draft.key} className="council-member">
              <div className="council-member__row">
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

      {error && <div className="council-warn council-warn--error">{error}</div>}

      <div className="council-setup__actions">
        <button type="button" className="council-btn council-btn--primary" disabled={creating || !engineReady} onClick={submit}>
          {creating ? t("council.setup.starting") : t("council.setup.start")}
        </button>
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

  return (
    <div className="council-run">
      <div className="council-run__head">
        <button type="button" className="council-back" onClick={onBack}>
          <ChevronLeft size={14} /> {t("council.run.back")}
        </button>
        <div className="council-run__title">
          <strong>{spec.title}</strong>
          <span className="council-badge">{phaseLabel(state.phase)}{state.round > 1 ? ` · ${t("council.run.roundBadge", { n: state.round })}` : ""}</span>
          {detail.live && <span className="council-badge council-badge--live">{t("council.run.live")}</span>}
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

      {state.error && <div className="council-warn council-warn--error">{state.error}</div>}

      <div className="council-run__members">
        {spec.members.map((member) => (
          <div key={member.id} className="council-member-chip" style={{ borderColor: memberColor(spec.members, member.id) }}>
            <span className="council-member-chip__dot" style={{ background: memberColor(spec.members, member.id) }} />
            <span className="council-member-chip__name">
              {member.isChair ? "★ " : ""}
              {member.displayName}
            </span>
            <span className="council-member-chip__model">{member.model}</span>
            <span className="council-member-chip__status">{memberStatusText(state.memberStatus[member.id])}</span>
          </div>
        ))}
        <div className="council-budget">
          {t("council.run.budget", { requests: state.budgetUsage.requests, tokens: state.budgetUsage.tokensEst.toLocaleString() })}
          {spec.budget.maxRequests ? t("council.run.budgetLimit", { limit: spec.budget.maxRequests }) : ""}
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

      {plan && <CouncilPlanView plan={plan} phase={state.phase} onApprove={control} />}

      <div className="council-timeline">
        {groups.map((group) => (
          <section key={group.key} className="council-tgroup">
            <h3 className="council-tgroup__title">{group.title}</h3>
            {group.messages.map((msg) => (
              <CouncilMessageCard key={msg.messageId} msg={msg} spec={spec} />
            ))}
          </section>
        ))}
        {messages.length === 0 && <div className="council-empty">{t("council.run.timelineEmpty")}</div>}
      </div>
    </div>
  );
}

function CouncilMessageCard(props: { msg: CouncilMessage; spec: RunDetail["spec"] }) {
  const { msg, spec } = props;
  const color = memberColor(spec.members, msg.from);
  const isSystem = msg.from === "system";
  const kind = msg.kind as CouncilMessageKind;
  if (isSystem || kind === "status") {
    return (
      <div className="council-msg council-msg--system">
        <span>{msg.content}</span>
      </div>
    );
  }
  return (
    <article className="council-msg" style={{ borderLeftColor: color }}>
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
}) {
  const { plan, phase, onApprove } = props;
  const t = useT();
  return (
    <section className="council-plan">
      <header className="council-plan__head">
        <h3>{t("council.plan.title")}</h3>
        {phase === "plan_ready" ? (
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
