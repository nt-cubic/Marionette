import { FileDiff, PanelRightClose, PanelRightOpen, RefreshCw } from "lucide-react";
import { useState } from "react";
import type {
  ChangedFile,
  HandoffResult,
  ProjectContext,
  UsageSnapshot,
  UsageWindow,
} from "../lib/types";
import { WindowControls } from "./WindowControls";

type ContextPanelProps = {
  collapsed: boolean;
  onCollapse: () => void;
  onExpand: () => void;
  usage: UsageSnapshot;
  onUsageRefresh: () => void;
  changedFiles?: ChangedFile[];
  changedFilesNote?: string | null;
  onRefreshChangedFiles?: () => void;
  onOpenDiff?: (path: string) => void;
  handoff?: HandoffResult | null;
  /** MCP servers + skills found on this machine / in this project. */
  projectContext?: ProjectContext | null;
  projectContextScanning?: boolean;
  onRescanProjectContext?: () => void;
  onToggleProjectContext?: (kind: "mcp" | "skill", id: string, enabled: boolean) => void;
  /** Agent of the active dialog — decides what is already native. */
  activeAgentId?: string;
  /** Parent-driven panel width drag. */
  resizeDragging?: boolean;
  onResizeStart?: () => void;
};

function meterTone(window: UsageWindow): "ok" | "warn" | "hot" | "none" {
  if (window.percentage == null) return "none";
  if (window.kind === "cost") return "none";
  if (window.percentage >= 90) return "hot";
  if (window.percentage >= 75) return "warn";
  return "ok";
}

function formatPrimary(window: UsageWindow): string {
  if (window.kind === "cost" && window.detail) return window.detail;
  if (window.percentage === null) {
    // Prefer a short honest label; full sentence stays in detail line.
    if (window.detail) {
      const d = window.detail.toLowerCase();
      if (d.includes("ceiling") || d.includes("plan")) return "Plan only";
      if (d.includes("no public") || d.includes("unavailable") || d.includes("not public")) {
        return "N/A";
      }
      if (d.includes("waiting")) return "…";
    }
    return "N/A";
  }
  return `${window.percentage}%`;
}

function changeBadge(changeType: ChangedFile["changeType"]): string {
  switch (changeType) {
    case "added":
      return "A";
    case "deleted":
      return "D";
    case "untracked":
      return "U";
    default:
      return "M";
  }
}

export function ContextPanel({
  collapsed,
  onCollapse,
  onExpand,
  usage,
  onUsageRefresh,
  changedFiles = [],
  changedFilesNote = null,
  onRefreshChangedFiles,
  onOpenDiff,
  handoff = null,
  projectContext = null,
  projectContextScanning = false,
  onRescanProjectContext,
  onToggleProjectContext,
  activeAgentId,
  resizeDragging = false,
  onResizeStart,
}: ContextPanelProps) {
  const [showAllSkills, setShowAllSkills] = useState(false);
  if (collapsed) {
    return (
      <aside className="context-panel is-collapsed" aria-label="Context panel">
        {/* Drag strip is a sibling — never wrap WindowControls in data-tauri-drag-region. */}
        <div className="context-panel__chrome context-panel__chrome--collapsed">
          <div className="titlebar-drag-fill" data-tauri-drag-region />
          <WindowControls />
        </div>
        <div className="context-panel__collapsed-rail">
          <button className="icon-button icon-button--small context-panel__button" type="button" title="Show information panel" aria-label="Show information panel" onClick={onExpand}>
            <PanelRightOpen size={14} />
          </button>
        </div>
      </aside>
    );
  }

  return (
    <aside className="context-panel" aria-label="Context panel">
      {onResizeStart && (
        <button
          type="button"
          className={resizeDragging ? "panel-resizer panel-resizer--right is-dragging" : "panel-resizer panel-resizer--right"}
          aria-label="Resize information panel"
          title="Drag to resize"
          onMouseDown={(event) => {
            event.preventDefault();
            onResizeStart();
          }}
        />
      )}
      <div className="context-panel__top titlebar-row">
        <span className="context-panel__title titlebar-drag-fill" data-tauri-drag-region>
          Information
        </span>
        <WindowControls />
      </div>

      <section className="context-card">
        <div className="context-card__heading">
          <span>Usage</span>
          <button className="icon-button icon-button--small context-panel__button" type="button" title="Refresh usage" aria-label="Refresh usage" onClick={onUsageRefresh}>
            <RefreshCw size={13} />
          </button>
        </div>
        <div className="usage-agent">{usage.agentLabel}</div>
        <div className="usage-list">
          {usage.windows.map((window) => {
            const tone = meterTone(window);
            return (
              <div className={`usage-row usage-row--${tone}`} key={window.id}>
                <span>{window.label}</span>
                <strong>{formatPrimary(window)}</strong>
                {window.detail && window.kind !== "cost" && (
                  <span className="usage-row__detail">{window.detail}</span>
                )}
                {window.percentage !== null && (
                  <div className="usage-meter__track">
                    <span style={{ width: `${Math.min(100, Math.max(0, window.percentage))}%` }} />
                  </div>
                )}
              </div>
            );
          })}
        </div>
        <small className="usage-updated">
          Updated {usage.refreshedAt}
          {usage.note ? `. ${usage.note}` : "."}
        </small>
      </section>

      <section className="context-card">
        <div className="context-card__heading">
          <span>Changed Files · project</span>
          {onRefreshChangedFiles && (
            <button className="icon-button icon-button--small context-panel__button" type="button" title="Refresh git status" aria-label="Refresh git status" onClick={onRefreshChangedFiles}>
              <RefreshCw size={13} />
            </button>
          )}
        </div>
        {changedFiles.length === 0 ? (
          <p className="context-card__empty">
            {changedFilesNote ?? "No local changes (or not a git repo)."}
          </p>
        ) : (
          <ul className="changed-files-list">
            {changedFiles.map((file) => (
              <li key={`${file.changeType}:${file.path}`}>
                <button
                  type="button"
                  className="changed-file-row"
                  title={file.path}
                  onClick={() => onOpenDiff?.(file.path)}
                >
                  <span className={`change-badge change-${file.changeType}`}>{changeBadge(file.changeType)}</span>
                  <span className="changed-file-row__path">{file.path}</span>
                  {onOpenDiff && <FileDiff size={12} className="changed-file-row__icon" aria-hidden />}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="context-card">
        <div className="context-card__heading">
          <span>Project context</span>
          {onRescanProjectContext && (
            <button
              className="icon-button icon-button--small context-panel__button"
              type="button"
              title="Rescan agent configs and skill folders"
              aria-label="Rescan project context"
              onClick={onRescanProjectContext}
            >
              <RefreshCw size={13} />
            </button>
          )}
        </div>
        {projectContext == null ? (
          <p className="context-card__empty">
            {projectContextScanning ? "Scanning…" : "Open a project to scan MCP servers and skills."}
          </p>
        ) : (
          <div className="context-lend">
            <p className="context-lend__hint">
              Lend these to agents that don’t have them. Checked items go into the next
              connection — MCP servers via <code>session/new</code>, skills as a pointer list.
            </p>

            <span className="context-lend__label">
              MCP servers ({projectContext.inventory.mcpServers.length})
            </span>
            {projectContext.inventory.mcpServers.length === 0 ? (
              <p className="context-card__empty">None found in agent configs.</p>
            ) : (
              <ul className="context-lend__list">
                {projectContext.inventory.mcpServers.map((server) => {
                  const enabled = projectContext.selection.mcpServers[server.id] ?? false;
                  const native = activeAgentId ? server.agents.includes(activeAgentId) : false;
                  return (
                    <li key={server.id}>
                      <label className="context-lend__row" title={server.sourcePaths.join("\n")}>
                        <input
                          type="checkbox"
                          checked={enabled}
                          onChange={(event) =>
                            onToggleProjectContext?.("mcp", server.id, event.target.checked)
                          }
                        />
                        <span className="context-lend__name">{server.name}</span>
                        <span className="context-lend__meta">
                          {server.transport}
                          {server.agents.length > 0 ? ` · has: ${server.agents.join(", ")}` : ""}
                          {native ? " · native here" : ""}
                        </span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            )}

            <span className="context-lend__label">
              Skills ({projectContext.inventory.skills.length})
            </span>
            {projectContext.inventory.skills.length === 0 ? (
              <p className="context-card__empty">No SKILL.md folders found.</p>
            ) : (
              <ul className="context-lend__list">
                {(showAllSkills
                  ? projectContext.inventory.skills
                  : projectContext.inventory.skills.slice(0, 6)
                ).map((skill) => {
                  const enabled = projectContext.selection.skills[skill.id] ?? true;
                  const native = activeAgentId ? skill.agents.includes(activeAgentId) : false;
                  return (
                    <li key={skill.id}>
                      <label
                        className="context-lend__row"
                        title={`${skill.description || skill.name}\n${skill.file}`}
                      >
                        <input
                          type="checkbox"
                          checked={enabled}
                          onChange={(event) =>
                            onToggleProjectContext?.("skill", skill.id, event.target.checked)
                          }
                        />
                        <span className="context-lend__name">{skill.name}</span>
                        <span className="context-lend__meta">
                          {skill.sources.join(", ")}
                          {native ? " · native here" : ""}
                        </span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            )}
            {projectContext.inventory.skills.length > 6 && (
              <button
                type="button"
                className="context-lend__more"
                onClick={() => setShowAllSkills((current) => !current)}
              >
                {showAllSkills
                  ? "Show fewer"
                  : `Show all ${projectContext.inventory.skills.length}`}
              </button>
            )}

            {projectContext.inventory.notes.map((note) => (
              <p className="context-card__empty" key={note}>
                {note}
              </p>
            ))}
          </div>
        )}
      </section>

      <section className="context-card">
        <div className="context-card__heading">
          <span>Handoff</span>
        </div>
        {handoff ? (
          <div className="handoff-block">
            <strong title={handoff.handoffPath}>{handoff.summary || handoff.prompt.slice(0, 80)}</strong>
            <small>Target: {handoff.targetAgentId}</small>
            <small title={handoff.handoffPath}>{handoff.handoffPath}</small>
            <p className="handoff-block__hint">
              Per-dialog file under `.agentshell/handoff/` · prefill in Composer (not auto-sent).
            </p>
          </div>
        ) : (
          <p className="context-card__empty">
            Switch agent to write `.agentshell/handoff/&lt;session&gt;.md` and prefill Composer.
          </p>
        )}
      </section>

      <div className="context-panel__footer">
        <button className="icon-button icon-button--small context-panel__button" type="button" title="Collapse information panel" aria-label="Collapse information panel" onClick={onCollapse}>
          <PanelRightClose size={14} />
        </button>
      </div>
    </aside>
  );
}
