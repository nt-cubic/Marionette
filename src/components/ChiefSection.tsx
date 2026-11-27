import { RefreshCw, Square, Zap } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  chiefConfigLoad,
  chiefConfigSave,
  chiefStopAll,
  chiefTasksListAll,
  isTauriRuntime,
  listAgents,
  listProjects,
  type ChiefConfig,
  type ChiefProjectPolicy,
  type ChiefTask,
} from "../lib/api";
import {
  DELEGATED_TASK_STATE_LABEL,
  projectDelegatedTask,
} from "../lib/backgroundTasks";
import { useT } from "../lib/i18n";
import type { AgentConfig, Project } from "../lib/types";

/**
 * 远程助理（Marionette Chief）配置面与任务区（docs/research/telegram-chief/）。
 *
 * M1 只给三个概念（01-product.md §5）：Chief 是谁（agent）、它能碰哪些项目
 * （允许列表 + 项目策略）、遇到危险谁说了算（写走权限卡，急停在这里）。
 * 高级参数全部折叠成每项目一行的紧凑表；不暴露凭据与工具开关。
 */

const MODES = [
  { value: "off", labelKey: "chief.modeOff", hintKey: "chief.modeOffHint" },
  { value: "relay", labelKey: "chief.modeRelay", hintKey: "chief.modeRelayHint" },
  { value: "chief", labelKey: "chief.modeChief", hintKey: "chief.modeChiefHint" },
] as const satisfies ReadonlyArray<{ value: ChiefConfig["mode"]; labelKey: string; hintKey: string }>;

const REMOTE_MODES = [
  { value: "read_only", labelKey: "chief.remoteReadOnly" },
  { value: "approve_writes", labelKey: "chief.remoteApproveWrites" },
  { value: "write_preauthorized", labelKey: "chief.remotePreauthorized" },
  { value: "blocked", labelKey: "chief.remoteBlocked" },
] as const satisfies ReadonlyArray<{ value: ChiefProjectPolicy["remoteMode"]; labelKey: string }>;

export function ChiefSection({ embedded = false }: { embedded?: boolean }) {
  const t = useT();
  const [config, setConfig] = useState<ChiefConfig | null>(null);
  const [tasks, setTasks] = useState<ChiefTask[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [agents, setAgents] = useState<AgentConfig[]>([]);
  const [saving, setSaving] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [loadedConfig, loadedTasks, loadedProjects, loadedAgents] = await Promise.all([
        chiefConfigLoad(),
        chiefTasksListAll(),
        isTauriRuntime() ? listProjects() : Promise.resolve([]),
        listAgents().catch(() => [] as AgentConfig[]),
      ]);
      if (loadedConfig) setConfig(loadedConfig);
      setTasks(loadedTasks);
      setProjects(loadedProjects);
      setAgents(loadedAgents.filter((agent) => agent.enabled));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => {
      if (typeof document !== "undefined" && document.hidden) return;
      void chiefTasksListAll().then((value) => setTasks(value)).catch(() => undefined);
    }, 10000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const patchPolicy = useCallback(
    (projectId: string, patch: Partial<ChiefProjectPolicy>) => {
      setConfig((current) => {
        if (!current) return current;
        const existing = current.allowedProjects.find((p) => p.projectId === projectId);
        const base: ChiefProjectPolicy = existing ?? {
          projectId,
          enabled: true,
          remoteMode: "approve_writes",
          allowedAgents: [],
          maxConcurrentWorkers: 1,
          maxTaskMinutes: 120,
          allowComputerUse: false,
          selfModification: false,
        };
        const next = {
          ...current,
          allowedProjects: current.allowedProjects.some((p) => p.projectId === projectId)
            ? current.allowedProjects.map((p) =>
                p.projectId === projectId ? { ...p, ...patch } : p,
              )
            : [...current.allowedProjects, { ...base, ...patch }],
        };
        return next;
      });
      setDirty(true);
    },
    [],
  );

  const save = useCallback(async () => {
    if (!config) return;
    setSaving(true);
    setError(null);
    try {
      const saved = await chiefConfigSave(config);
      if (saved) setConfig(saved);
      setDirty(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }, [config]);

  const panicStop = useCallback(async () => {
    setStopping(true);
    try {
      await chiefStopAll();
      setTasks(await chiefTasksListAll());
    } finally {
      setStopping(false);
    }
  }, []);

  const enabledAgentOptions = useMemo(
    () => agents.map((agent) => ({ id: agent.id, label: agent.label })),
    [agents],
  );

  if (!config) {
    return (
      <div className={embedded ? "chief-section" : "chief-section standalone"}>
        <p className="panel-note">{t("chief.loading")}</p>
      </div>
    );
  }

  const activeTasks = tasks.filter((task) => {
    const { state } = projectDelegatedTask({
      status: task.status,
      waitingReason: task.waitingReason ?? null,
      retryAt: task.retryAt ?? null,
      lastEventAt: task.lastEventAt,
    });
    return state === "starting" || state === "running" || state === "waiting_user";
  });
  const recentTasks = tasks.slice(0, 8);

  return (
    <div className={embedded ? "chief-section" : "chief-section standalone"}>
      <div className="chief-section__modes" role="radiogroup" aria-label={t("chief.modeGroup")}>
        {MODES.map((mode) => (
          <button
            key={mode.value}
            type="button"
            className={`chip${config.mode === mode.value ? " is-active" : ""}`}
            title={t(mode.hintKey)}
            onClick={() => {
              setConfig({ ...config, mode: mode.value });
              setDirty(true);
            }}
          >
            {t(mode.labelKey)}
          </button>
        ))}
      </div>

      <div className="chief-section__row">
        <label className="chief-section__label">{t("chief.chiefAgent")}</label>
        <select
          className="chief-section__select"
          value={config.chiefAgentId ?? ""}
          onChange={(event) => {
            setConfig({ ...config, chiefAgentId: event.target.value || null });
            setDirty(true);
          }}
        >
          <option value="">{t("chief.agentAuto")}</option>
          {enabledAgentOptions.map((agent) => (
            <option key={agent.id} value={agent.id}>
              {agent.label}
            </option>
          ))}
        </select>
      </div>

      <div className="chief-section__row">
        <label className="chief-section__label">{t("chief.maxWorkers")}</label>
        <select
          className="chief-section__select"
          value={config.maxConcurrentWorkers}
          onChange={(event) => {
            setConfig({ ...config, maxConcurrentWorkers: Number(event.target.value) || 1 });
            setDirty(true);
          }}
        >
          {[1, 2, 3].map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
      </div>

      <div className="chief-section__projects">
        <div className="chief-section__subhead">{t("chief.allowedProjects")}</div>
        {projects.length === 0 ? (
          <p className="panel-note">{t("chief.noProjects")}</p>
        ) : (
          projects.map((project) => {
            const policy = config.allowedProjects.find((p) => p.projectId === project.id);
            const enabled = policy != null;
            const remoteMode = policy?.remoteMode ?? "approve_writes";
            return (
              <div className="chief-project-row" key={project.id} title={project.rootPath}>
                <label className="chief-project-row__name">
                  <input
                    type="checkbox"
                    checked={enabled}
                    onChange={(event) => {
                      patchPolicy(project.id, { enabled: event.target.checked });
                    }}
                  />
                  {project.name}
                </label>
                {enabled ? (
                  <select
                    className="chief-section__select chief-section__select--tight"
                    value={remoteMode}
                    onChange={(event) => {
                      patchPolicy(project.id, {
                        remoteMode: event.target
                          .value as ChiefProjectPolicy["remoteMode"],
                      });
                    }}
                  >
                    {REMOTE_MODES.filter((mode) => mode.value !== "blocked").map((mode) => (
                      <option key={mode.value} value={mode.value}>
                        {t(mode.labelKey)}
                      </option>
                    ))}
                  </select>
                ) : null}
              </div>
            );
          })
        )}
        <p className="panel-note panel-note--hint">{t("chief.projectHint")}</p>
      </div>

      <div className="chief-section__footer">
        {error ? <span className="chief-section__error">{error}</span> : null}
        <button
          type="button"
          className="pill-action chief-section__save"
          disabled={!dirty || saving}
          onClick={() => void save()}
        >
          <RefreshCw size={13} className={saving ? "is-spinning" : ""} />
          {dirty ? t("chief.save") : t("chief.saved")}
        </button>
      </div>

      <div className="chief-section__tasks">
        <div className="chief-section__subhead">
          {t("chief.tasksHead")}
          {activeTasks.length > 0 ? (
            <span className="context-lend__count">{activeTasks.length}</span>
          ) : null}
          {activeTasks.length > 0 || recentTasks.length > 0 ? (
            <button
              type="button"
              className="pill-action chief-section__stop"
              disabled={stopping}
              onClick={() => void panicStop()}
            >
              <Square size={12} />
              {t("chief.panicStop")}
            </button>
          ) : null}
        </div>
        {recentTasks.length === 0 ? (
          <p className="panel-note">{t("chief.tasksEmpty")}</p>
        ) : (
          recentTasks.map((task) => {
            const { state } = projectDelegatedTask({
              status: task.status,
              waitingReason: task.waitingReason ?? null,
              retryAt: task.retryAt ?? null,
              lastEventAt: task.lastEventAt,
            });
            return (
              <div className={`chief-task is-${state}`} key={task.taskId}>
                <div className="chief-task__head">
                  <Zap size={12} aria-hidden />
                  <span className="chief-task__title" title={task.goal}>
                    {task.title}
                  </span>
                  <span className="chief-task__status">{t(DELEGATED_TASK_STATE_LABEL[state])}</span>
                </div>
                {task.lastSummary ? (
                  <div className="chief-task__summary">{task.lastSummary}</div>
                ) : null}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
