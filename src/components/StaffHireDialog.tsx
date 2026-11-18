import { useMemo, useState } from "react";
import type { AgentConfig, Project } from "../lib/types";
import { useT } from "../lib/i18n";
import { DialogShell } from "./DialogShell";

export type StaffHireDraft = {
  staffId?: string;
  name: string;
  title: string;
  persona: string;
  agentId: string;
  model: string;
  telegramChatId: string;
  autonomy: "manual" | "assisted" | "autonomous";
  maxActiveTasks: number;
  maxTurnsPerTask: number;
  maxWallMinutesPerTask: number;
  maxModelCallsPerTask: number;
  maxCostPerTaskUsd: number;
  maxReportsPerHour: number;
  notificationCooldownMinutes: number;
  quietHours: string;
  allowBackgroundWork: boolean;
  allowTelegram: boolean;
  allowedProjectIds: string[];
  projectPolicies: Record<string, StaffProjectDraftPolicy>;
};

export type StaffProjectDraftPolicy = {
  remoteMode: "read_only" | "approve_writes" | "write_preauthorized";
  allowComputerUse: boolean;
  selfModification: boolean;
};

const DEFAULT_PERSONA =
  "你是用户的本地首席助理。先理解目标，再决定自己执行还是派给项目里的 worker。" +
  "汇报要短，结论必须带文件、测试或 diff 证据。只在答案会改变结果时提问。" +
  "没有用户任务、到期承诺或已批准 backlog 时保持待机，不制造工作。";

export function StaffHireDialog({
  agents,
  projects,
  defaultProjectId,
  initial,
  busy = false,
  error,
  onClose,
  onSave,
}: {
  agents: AgentConfig[];
  projects: Project[];
  defaultProjectId?: string;
  initial?: Partial<StaffHireDraft>;
  busy?: boolean;
  error?: string | null;
  onClose: () => void;
  onSave: (draft: StaffHireDraft) => void | Promise<void>;
}) {
  const t = useT();
  const enabledAgents = useMemo(() => agents.filter((agent) => agent.enabled), [agents]);
  const [draft, setDraft] = useState<StaffHireDraft>(() => {
    const quickProjectId =
      defaultProjectId && projects.some((project) => project.id === defaultProjectId)
        ? defaultProjectId
        : undefined;
    return {
      staffId: initial?.staffId,
      name: initial?.name ?? "",
      title: initial?.title ?? t("staff.defaultTitle"),
      persona: initial?.persona ?? DEFAULT_PERSONA,
      agentId: initial?.agentId ?? enabledAgents[0]?.id ?? "",
      model: initial?.model ?? "",
      telegramChatId: initial?.telegramChatId ?? "",
      autonomy: initial?.autonomy ?? "assisted",
      maxActiveTasks: initial?.maxActiveTasks ?? 1,
      maxTurnsPerTask: initial?.maxTurnsPerTask ?? 30,
      maxWallMinutesPerTask: initial?.maxWallMinutesPerTask ?? 120,
      maxModelCallsPerTask: initial?.maxModelCallsPerTask ?? 0,
      maxCostPerTaskUsd: initial?.maxCostPerTaskUsd ?? 0,
      maxReportsPerHour: initial?.maxReportsPerHour ?? 4,
      notificationCooldownMinutes: initial?.notificationCooldownMinutes ?? 30,
      quietHours: initial?.quietHours ?? "23:00-08:00",
      allowBackgroundWork: initial?.allowBackgroundWork ?? true,
      allowTelegram: initial?.allowTelegram ?? false,
      allowedProjectIds:
        initial?.allowedProjectIds ?? (quickProjectId ? [quickProjectId] : []),
      projectPolicies:
        initial?.projectPolicies ??
        (quickProjectId
          ? {
              [quickProjectId]: {
                remoteMode: "approve_writes",
                allowComputerUse: false,
                selfModification: false,
              },
            }
          : {}),
    };
  });
  const [formError, setFormError] = useState("");

  const toggleProject = (projectId: string) => {
    setDraft((current) => {
      const selected = current.allowedProjectIds.includes(projectId);
      return {
        ...current,
        allowedProjectIds: selected
          ? current.allowedProjectIds.filter((id) => id !== projectId)
          : [...current.allowedProjectIds, projectId],
        projectPolicies: selected
          ? current.projectPolicies
          : {
              ...current.projectPolicies,
              [projectId]: current.projectPolicies[projectId] ?? {
                remoteMode: "approve_writes",
                allowComputerUse: false,
                selfModification: false,
              },
            },
      };
    });
  };

  const submit = async () => {
    const name = draft.name.trim();
    if (!name) {
      setFormError(t("staff.nameRequired"));
      return;
    }
    if (!draft.agentId) {
      setFormError(t("staff.agentRequired"));
      return;
    }
    setFormError("");
    await onSave({
      ...draft,
      name,
      title: draft.title.trim() || t("staff.defaultTitle"),
      persona: draft.persona.trim(),
      model: draft.model.trim(),
    });
  };

  return (
    <DialogShell
      className="project-dialog-backdrop"
      dialogClassName="project-dialog staff-dialog"
      labelledBy="staff-dialog-title"
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <div className="staff-dialog__head">
        <div>
          <h2 id="staff-dialog-title">{draft.staffId ? t("staff.editTitle") : t("staff.hireTitle")}</h2>
          <p>{t("staff.hireLead")}</p>
        </div>
        <button
          type="button"
          className="pill-action"
          disabled={busy}
          onClick={onClose}
        >
          {t("common.cancel")}
        </button>
      </div>

      <div className="staff-dialog__body custom-scrollbar scrollbar-autohide">
        <section className="staff-dialog__quick">
          <label>
            {t("staff.name")}
            <input
              value={draft.name}
              autoFocus
              placeholder={t("staff.namePlaceholder")}
              onChange={(event) => setDraft((current) => ({ ...current, name: event.target.value }))}
            />
          </label>
          <p>{t("staff.quickLead")}</p>
          <p>{t("staff.quickDefaults")}</p>
          {enabledAgents.length === 0 ? (
            <p className="staff-dialog__error">{t("staff.agentRequired")}</p>
          ) : null}
        </section>

        <details className="staff-dialog__advanced">
          <summary>
            <span>{t("staff.advanced")}</span>
            <span>{t("staff.advancedHint")}</span>
          </summary>
          <div className="staff-dialog__advanced-body">
            <section className="staff-dialog__section">
              <h3>{t("staff.identity")}</h3>
              <div className="staff-dialog__grid">
                <label>
                  {t("staff.title")}
                  <input
                    value={draft.title}
                    placeholder={t("staff.defaultTitle")}
                    onChange={(event) => setDraft((current) => ({ ...current, title: event.target.value }))}
                  />
                </label>
              </div>
              <label>
                {t("staff.persona")}
                <textarea
                  value={draft.persona}
                  rows={7}
                  onChange={(event) => setDraft((current) => ({ ...current, persona: event.target.value }))}
                />
              </label>
            </section>

        <section className="staff-dialog__section">
          <h3>{t("staff.brain")}</h3>
          <div className="staff-dialog__grid">
            <label>
              {t("staff.agent")}
              <select
                value={draft.agentId}
                onChange={(event) => setDraft((current) => ({ ...current, agentId: event.target.value }))}
              >
                {enabledAgents.map((agent) => (
                  <option key={agent.id} value={agent.id}>{agent.label}</option>
                ))}
              </select>
            </label>
            <label>
              {t("staff.model")}
              <input
                value={draft.model}
                placeholder={t("staff.modelPlaceholder")}
                onChange={(event) => setDraft((current) => ({ ...current, model: event.target.value }))}
              />
            </label>
            <label>
              {t("staff.telegramChatId")}
              <input
                value={draft.telegramChatId}
                inputMode="numeric"
                placeholder={t("staff.telegramChatIdPlaceholder")}
                onChange={(event) =>
                  setDraft((current) => ({ ...current, telegramChatId: event.target.value }))
                }
              />
            </label>
          </div>
        </section>

        <section className="staff-dialog__section">
          <h3>{t("staff.projects")}</h3>
          {projects.length === 0 ? (
            <p className="panel-note">{t("staff.noProjects")}</p>
          ) : (
            <div className="staff-dialog__projects">
              {projects.map((project) => {
                const checked = draft.allowedProjectIds.includes(project.id);
                const policy = draft.projectPolicies[project.id] ?? {
                  remoteMode: "approve_writes",
                  allowComputerUse: false,
                  selfModification: false,
                };
                const updatePolicy = (patch: Partial<StaffProjectDraftPolicy>) => {
                  setDraft((current) => ({
                    ...current,
                    projectPolicies: {
                      ...current.projectPolicies,
                      [project.id]: {
                        ...(current.projectPolicies[project.id] ?? policy),
                        ...patch,
                      },
                    },
                  }));
                };
                return (
                  <div className="staff-project-access" key={project.id}>
                    <label title={project.rootPath}>
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={() => toggleProject(project.id)}
                      />
                      <span>{project.name}</span>
                    </label>
                    {checked ? (
                      <div className="staff-project-access__policy">
                        <select
                          aria-label={project.name + " " + t("staff.projectMode")}
                          value={policy.remoteMode}
                          onChange={(event) =>
                            updatePolicy({
                              remoteMode: event.target.value as StaffProjectDraftPolicy["remoteMode"],
                            })
                          }
                        >
                          <option value="read_only">{t("staff.projectModeReadOnly")}</option>
                          <option value="approve_writes">{t("staff.projectModeApproveWrites")}</option>
                          <option value="write_preauthorized">{t("staff.projectModePreauthorized")}</option>
                        </select>
                        <label>
                          <input
                            type="checkbox"
                            checked={policy.allowComputerUse}
                            onChange={(event) => updatePolicy({ allowComputerUse: event.target.checked })}
                          />
                          <span>{t("staff.allowComputerUse")}</span>
                        </label>
                        <label>
                          <input
                            type="checkbox"
                            checked={policy.selfModification}
                            onChange={(event) => updatePolicy({ selfModification: event.target.checked })}
                          />
                          <span>{t("staff.selfModification")}</span>
                        </label>
                      </div>
                    ) : null}
                  </div>
                );
              })}
            </div>
          )}
          <p className="panel-note">{t("staff.projectsHint")}</p>
        </section>

        <section className="staff-dialog__section">
          <h3>{t("staff.workPolicy")}</h3>
          <div className="staff-dialog__grid">
            <label>
              {t("staff.autonomy")}
              <select
                value={draft.autonomy}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    autonomy: event.target.value as StaffHireDraft["autonomy"],
                  }))
                }
              >
                <option value="manual">{t("staff.autonomyManual")}</option>
                <option value="assisted">{t("staff.autonomyAssisted")}</option>
                <option value="autonomous">{t("staff.autonomyAutonomous")}</option>
              </select>
            </label>
            <label>
              {t("staff.maxActiveTasks")}
              <select
                value={draft.maxActiveTasks}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    maxActiveTasks: Math.max(1, Math.min(3, Number(event.target.value) || 1)),
                  }))
                }
              >
                {[1, 2, 3].map((count) => (
                  <option key={count} value={count}>{count}</option>
                ))}
              </select>
            </label>
            <label>
              {t("staff.maxTurns")}
              <input
                type="number"
                min={1}
                max={500}
                value={draft.maxTurnsPerTask}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    maxTurnsPerTask: Math.max(1, Number(event.target.value) || 1),
                  }))
                }
              />
            </label>
            <label>
              {t("staff.maxWallMinutes")}
              <input
                type="number"
                min={1}
                max={10080}
                value={draft.maxWallMinutesPerTask}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    maxWallMinutesPerTask: Math.max(1, Number(event.target.value) || 1),
                  }))
                }
              />
            </label>
            <label>
              {t("staff.maxModelCalls")}
              <input
                type="number"
                min={0}
                value={draft.maxModelCallsPerTask}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    maxModelCallsPerTask: Math.max(0, Number(event.target.value) || 0),
                  }))
                }
              />
            </label>
            <label>
              {t("staff.maxCost")}
              <input
                type="number"
                min={0}
                step="0.01"
                value={draft.maxCostPerTaskUsd}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    maxCostPerTaskUsd: Math.max(0, Number(event.target.value) || 0),
                  }))
                }
              />
            </label>
            <label>
              {t("staff.maxReports")}
              <input
                type="number"
                min={0}
                max={120}
                value={draft.maxReportsPerHour}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    maxReportsPerHour: Math.max(0, Number(event.target.value) || 0),
                  }))
                }
              />
            </label>
            <label>
              {t("staff.cooldownMinutes")}
              <input
                type="number"
                min={0}
                max={1440}
                value={draft.notificationCooldownMinutes}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    notificationCooldownMinutes: Math.max(0, Number(event.target.value) || 0),
                  }))
                }
              />
            </label>
            <label>
              {t("staff.quietHours")}
              <input
                value={draft.quietHours}
                placeholder="23:00-08:00"
                onChange={(event) =>
                  setDraft((current) => ({ ...current, quietHours: event.target.value }))
                }
              />
            </label>
          </div>
          <div className="staff-dialog__checks">
            <label>
              <input
                type="checkbox"
                checked={draft.allowBackgroundWork}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    allowBackgroundWork: event.target.checked,
                  }))
                }
              />
              <span>{t("staff.allowBackgroundWork")}</span>
            </label>
            <label>
              <input
                type="checkbox"
                checked={draft.allowTelegram}
                onChange={(event) =>
                  setDraft((current) => ({ ...current, allowTelegram: event.target.checked }))
                }
              />
              <span>{t("staff.allowTelegram")}</span>
            </label>
          </div>
          <p className="panel-note">{t("staff.policyHint")}</p>
        </section>
          </div>
        </details>
      </div>

      {formError || error ? (
        <p className="staff-dialog__error">{formError || error}</p>
      ) : null}
      <div className="staff-dialog__footer">
        <button type="button" className="pill-action" disabled={busy} onClick={onClose}>
          {t("common.cancel")}
        </button>
        <button type="button" className="pill-action is-primary" disabled={busy} onClick={() => void submit()}>
          {busy ? t("staff.saving") : draft.staffId ? t("staff.save") : t("staff.hireAction")}
        </button>
      </div>
    </DialogShell>
  );
}
