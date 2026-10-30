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
import { showToast } from "../lib/toast";
import { taskStatusLabel, type AgentLite } from "../lib/council";

// ---------------------------------------------------------------------------
// 成员草稿(setup 表单的编辑态)
// ---------------------------------------------------------------------------

interface MemberDraft extends MemberSpec {
  key: string;
}

const DEFAULT_ROLES: ReadonlyArray<string> = ["系统架构师", "批判性评审", "算法分析", "测试架构", "备选方案研究", "数学验证"];

function roleAt(index: number): string {
  return DEFAULT_ROLES[index % DEFAULT_ROLES.length] ?? "评审";
}

function defaultDrafts(): MemberDraft[] {
  return [
    { key: "m1", id: "architect", displayName: "系统架构师", model: "", role: "负责总体架构设计与技术选型", isChair: false },
    { key: "m2", id: "reviewer", displayName: "批判性评审", model: "", role: "负责找出方案的逻辑与工程缺陷", isChair: true },
    { key: "m3", id: "analyst", displayName: "算法分析", model: "", role: "负责算法正确性与复杂度分析", isChair: false },
  ];
}

const MODE_OPTIONS: Array<{ id: CouncilMode; label: string; hint: string }> = [
  { id: "council_only", label: "仅审议", hint: "模型审议并产出评审过的计划,不改动任何文件" },
  { id: "council_and_execution", label: "审议 + 执行", hint: "计划批准后由 coding agent 执行(执行集成开发中)" },
  { id: "autonomous", label: "自治研发", hint: "规划、执行、测试、评估、修订的受限循环(开发中)" },
];

// ---------------------------------------------------------------------------
// 主组件
// ---------------------------------------------------------------------------

interface CouncilPageProps {
  onClose: () => void;
}

type View = { kind: "list" } | { kind: "setup"; prefill?: string } | { kind: "run"; runId: string };

export function CouncilPage({ onClose }: CouncilPageProps) {
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
              showToast({ text: `删除失败:${e}`, tone: "error" });
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
            <ChevronLeft size={14} /> 返回
          </button>
          <p>run 不存在或已删除。</p>
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
  return (
    <div className="council-list">
      <div className="council-list__head">
        <div>
          <h2>多智能体协作 · Council</h2>
          <p className="council-sub">多个模型独立提案、互相质询、共同修订,产出经评审的实施计划。</p>
        </div>
        <button type="button" className="council-btn council-btn--primary" onClick={() => onNew()}>
          <Plus size={14} /> 新建审议
        </button>
      </div>
      {runs.length === 0 ? (
        <div className="council-empty">
          <Users size={28} />
          <p>还没有审议记录。点「新建审议」,给三个模型一个复杂任务。</p>
        </div>
      ) : (
        <div className="council-run-rows">
          {runs.map((run) => (
            <div key={run.runId} className="council-run-row">
              <button type="button" className="council-run-row__main" onClick={() => onOpen(run.runId)}>
                <span className="council-run-row__title">{run.title || "(未命名任务)"}</span>
                <span className="council-run-row__meta">
                  {phaseLabel(run.phase)} · {run.memberCount} 成员 · {run.rounds} 轮 · {run.messageCount} 条消息
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
              <button type="button" className="council-icon-btn" title="删除" onClick={() => onDelete(run.runId)}>
                <Trash2 size={14} />
              </button>
            </div>
          ))}
        </div>
      )}
      <button type="button" className="council-link-btn" onClick={onRefresh}>
        刷新列表
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
      const role = roleAt(n - 1);
      return [
        ...prev,
        {
          key: `m${n}-${Date.now()}`,
          id: deriveMemberId(role, prev.map((d) => d.id)),
          displayName: role,
          model: "",
          role: "",
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
      setError("任务描述太短——至少写清目标与边界。");
      return;
    }
    if (drafts.some((d) => !d.model)) {
      setError("每个成员都要选择一个模型。");
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
        setError("创建失败:当前环境不支持(需要桌面版)。");
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
        <ChevronLeft size={14} /> 返回列表
      </button>
      <h2>新建审议</h2>

      {!engineReady && (
        <div className="council-warn">
          模型共享引擎未就绪。先在左下角「模型共享」里启动引擎(需要至少一个可用模型来源),再回到这里。
        </div>
      )}

      <label className="council-field">
        <span>标题(可选)</span>
        <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="例如:为缓存模块设计并发安全方案" />
      </label>

      <label className="council-field">
        <span>任务 *</span>
        <textarea
          value={task}
          onChange={(e) => setTask(e.target.value)}
          rows={6}
          placeholder="描述要解决的软件工程问题:目标、约束、现有代码背景、期望产出。越具体,提案质量越高。"
        />
      </label>

      <div className="council-field">
        <span>运行模式</span>
        <div className="council-modes">
          {MODE_OPTIONS.map((opt) => (
            <button
              key={opt.id}
              type="button"
              className={`council-mode ${mode === opt.id ? "is-active" : ""}`}
              onClick={() => setMode(opt.id)}
              title={opt.hint}
            >
              {opt.label}
            </button>
          ))}
        </div>
        <p className="council-sub">{MODE_OPTIONS.find((o) => o.id === mode)?.hint}</p>
      </div>

      {mode !== "council_only" && (
        <div className="council-field">
          <span>执行器(计划批准后由该 agent 落码)</span>
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
              placeholder="项目根目录(绝对路径,git 仓库)"
            />
          </div>
          <p className="council-sub">
            执行集成为早期版本:并行任务用 git worktree 隔离,冲突显式上报;不填则批准后仅交付计划。
          </p>
        </div>
      )}

      <div className="council-field">
        <span>
          成员({drafts.length} 个,{distinctModels} 个不同模型
          {distinctModels < Math.min(3, drafts.length) ? " —— 建议至少 3 个不同模型" : ""})
        </span>
        <div className="council-members">
          {drafts.map((draft) => (
            <div key={draft.key} className="council-member">
              <div className="council-member__row">
                <input
                  value={draft.displayName}
                  onChange={(e) => updateDraft(draft.key, { displayName: e.target.value })}
                  placeholder="角色名"
                />
                <button
                  type="button"
                  className={`council-chair-toggle ${draft.isChair ? "is-chair" : ""}`}
                  onClick={() => setChair(draft.key)}
                  title={draft.isChair ? "当前主席(点击其他成员转移)" : "设为主席"}
                >
                  {draft.isChair ? "★ 主席" : "☆ 主席"}
                </button>
                <button
                  type="button"
                  className="council-icon-btn"
                  onClick={() => removeMember(draft.key)}
                  disabled={drafts.length <= 2}
                  title="移除"
                >
                  <Trash2 size={13} />
                </button>
              </div>
              <select value={draft.model} onChange={(e) => updateDraft(draft.key, { model: e.target.value })}>
                <option value="">选择模型…</option>
                {models.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.id}
                  </option>
                ))}
              </select>
              <input
                value={draft.role}
                onChange={(e) => updateDraft(draft.key, { role: e.target.value })}
                placeholder="角色职责(进提示词)"
              />
            </div>
          ))}
        </div>
        <button type="button" className="council-link-btn" onClick={addMember} disabled={drafts.length >= 8}>
          + 添加成员
        </button>
      </div>

      <div className="council-params">
        <label className="council-field council-field--inline">
          <span>审议轮次</span>
          <input
            type="number"
            min={1}
            max={4}
            value={rounds}
            onChange={(e) => setRounds(Math.max(1, Math.min(4, Number(e.target.value) || 1)))}
          />
        </label>
        <label className="council-field council-field--inline">
          <span>并发上限</span>
          <input
            type="number"
            min={1}
            max={8}
            value={maxConcurrent}
            onChange={(e) => setMaxConcurrent(Math.max(1, Math.min(8, Number(e.target.value) || 1)))}
          />
        </label>
        <label className="council-field council-field--inline">
          <span>请求预算(次)</span>
          <input value={maxRequests} onChange={(e) => setMaxRequests(e.target.value.replace(/\D/g, ""))} placeholder="60" />
        </label>
      </div>

      {error && <div className="council-warn council-warn--error">{error}</div>}

      <div className="council-setup__actions">
        <button type="button" className="council-btn council-btn--primary" disabled={creating || !engineReady} onClick={submit}>
          {creating ? "启动中…" : "开始审议"}
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
          <ChevronLeft size={14} /> 返回
        </button>
        <div className="council-run__title">
          <strong>{spec.title}</strong>
          <span className="council-badge">{phaseLabel(state.phase)}{state.round > 1 ? ` · 第 ${state.round} 轮` : ""}</span>
          {detail.live && <span className="council-badge council-badge--live">运行中</span>}
        </div>
        <div className="council-run__controls">
          {state.phase === "paused" && (
            <button type="button" className="council-btn" onClick={() => control("resume")}>
              <Play size={13} /> 继续
            </button>
          )}
          {running && (
            <button type="button" className="council-btn" onClick={() => control("pause")}>
              <Pause size={13} /> 暂停
            </button>
          )}
          {!isTerminalPhase(state.phase) && (
            <button type="button" className="council-btn council-btn--danger" onClick={() => control("cancel")}>
              <CircleStop size={13} /> 取消
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
          {state.budgetUsage.requests} 次请求 · ~{state.budgetUsage.tokensEst.toLocaleString()} tokens
          {spec.budget.maxRequests ? ` / 上限 ${spec.budget.maxRequests}` : ""}
        </div>
      </div>

      {state.tasks.length > 0 && (
        <section className="council-tasks">
          <h3 className="council-tgroup__title">执行任务({state.tasks.length})</h3>
          {state.tasks.map((t) => (
            <div key={t.stepId} className="council-task">
              <span className="council-task__id">{t.stepId}</span>
              <span className="council-task__title">{t.title}</span>
              <span className={`council-task__status council-task__status--${typeof t.status === "string" ? t.status : "failed"}`}>
                {taskStatusLabel(t.status)}
              </span>
              {t.changes.length > 0 && (
                <span className="council-task__changes" title={t.changes.join(" · ")}>
                  {t.changes.length} 个文件
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
        {messages.length === 0 && <div className="council-empty">审议尚未开始……</div>}
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
  return (
    <section className="council-plan">
      <header className="council-plan__head">
        <h3>实施计划</h3>
        {phase === "plan_ready" ? (
          <div className="council-plan__actions">
            <button type="button" className="council-btn" onClick={() => onApprove("reject")}>
              再议
            </button>
            <button type="button" className="council-btn council-btn--primary" onClick={() => onApprove("accept")}>
              接受计划
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
              {step.dependsOn.length > 0 && <span className="council-plan__dep">依赖 {step.dependsOn.join(", ")}</span>}
            </div>
            <div className="council-plan__step-detail">{step.detail}</div>
            <div className="council-plan__step-acceptance">验收:{step.acceptance}</div>
          </li>
        ))}
      </ol>
      {plan.openQuestions.length > 0 && (
        <div className="council-plan__section">
          <h4>遗留不确定性(已显式记录,不接受和稀泥)</h4>
          <ul>
            {plan.openQuestions.map((q, i) => (
              <li key={i}>{q}</li>
            ))}
          </ul>
        </div>
      )}
      {plan.verification.length > 0 && (
        <div className="council-plan__section">
          <h4>验证命令</h4>
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
