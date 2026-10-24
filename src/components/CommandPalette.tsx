import { useEffect, useMemo, useRef, useState } from "react";
import { Search } from "lucide-react";
import type { Project, Session } from "../lib/types";

export type PaletteAction = {
  id: string;
  label: string;
  /** Shortcut or short context, shown quiet on the right. */
  hint?: string;
  run: () => void;
};

type PaletteRow =
  | { kind: "session"; key: string; session: Session; chat: boolean }
  | { kind: "project"; key: string; project: Project }
  | { kind: "action"; key: string; action: PaletteAction };

type CommandPaletteProps = {
  sessions: Session[];
  chatSessions: Session[];
  projects: Project[];
  currentSessionId?: string | null;
  doneUnreadIds?: ReadonlySet<string>;
  agentLabel: (agentId: string) => string;
  actions: PaletteAction[];
  onOpenSession: (session: Session) => void;
  onOpenChat: (session: Session) => void;
  onSelectProject: (projectId: string) => void;
  onClose: () => void;
};

const RECENT_SHOWN = 8;
const SESSIONS_SHOWN = 12;
const PROJECTS_SHOWN = 5;

/** 3 = starts with the query, 2 = a word starts with it, 1 = contains it / every word does. */
function matchScore(text: string, query: string): number {
  if (!query) return 1;
  const hay = text.toLowerCase();
  const at = hay.indexOf(query);
  if (at === 0) return 3;
  if (at > 0) return /[\s/\\·_-]/.test(hay[at - 1]) ? 2 : 1;
  const words = query.split(/\s+/).filter(Boolean);
  return words.length > 1 && words.every((w) => hay.includes(w)) ? 1 : 0;
}

function recencyMs(session: Session): number {
  const ms = Date.parse(session.lastActiveAt);
  return Number.isFinite(ms) ? ms : 0;
}

function shortRecency(iso: string, nowMs: number): string {
  const ms = Date.parse(iso);
  if (!Number.isFinite(ms)) return "";
  const min = Math.floor((nowMs - ms) / 60_000);
  if (min < 1) return "now";
  if (min < 60) return `${min}m`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h`;
  const day = Math.floor(hr / 24);
  if (day < 7) return `${day}d`;
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/**
 * Ctrl+K: one keyboard surface for every dialog, project and shell action.
 * Empty query lists the most recent dialogs, then the actions; typing ranks
 * titles (prefix → word start → contains), newest first within a rank.
 */
export function CommandPalette({
  sessions,
  chatSessions,
  projects,
  currentSessionId,
  doneUnreadIds,
  agentLabel,
  actions,
  onOpenSession,
  onOpenChat,
  onSelectProject,
  onClose,
}: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const now = useMemo(() => Date.now(), []);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const projectName = useMemo(() => {
    const names = new Map(projects.map((p) => [p.id, p.name]));
    return (id: string) => names.get(id) ?? "";
  }, [projects]);

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const all = [
      ...sessions.map((session) => ({ session, chat: false })),
      ...chatSessions.map((session) => ({ session, chat: true })),
    ];
    const ranked = all
      .map((entry) => ({
        ...entry,
        score: Math.max(
          matchScore(entry.session.label, q),
          q ? matchScore(projectName(entry.session.projectId), q) - 1 : 0,
        ),
      }))
      .filter((entry) => entry.score > 0 && (q || entry.session.id !== currentSessionId))
      .sort((a, b) => b.score - a.score || recencyMs(b.session) - recencyMs(a.session))
      .slice(0, q ? SESSIONS_SHOWN : RECENT_SHOWN);
    const sessionRows: PaletteRow[] = ranked.map(({ session, chat }) => ({
      kind: "session",
      key: `s:${session.id}`,
      session,
      chat,
    }));
    const projectRows: PaletteRow[] = q
      ? projects
          .map((project) => ({ project, score: matchScore(project.name, q) }))
          .filter((entry) => entry.score > 0)
          .sort((a, b) => b.score - a.score)
          .slice(0, PROJECTS_SHOWN)
          .map(({ project }) => ({ kind: "project", key: `p:${project.id}`, project }))
      : [];
    const actionRows: PaletteRow[] = actions
      .filter((action) => matchScore(action.label, q) > 0)
      .map((action) => ({ kind: "action", key: `a:${action.id}`, action }));
    return [
      { title: q ? "对话" : "最近", rows: sessionRows },
      { title: "项目", rows: projectRows },
      { title: "操作", rows: actionRows },
    ].filter((group) => group.rows.length > 0);
  }, [query, sessions, chatSessions, projects, actions, currentSessionId, projectName]);

  const flat = useMemo(() => groups.flatMap((group) => group.rows), [groups]);
  const activeIndex = flat.length === 0 ? -1 : Math.min(active, flat.length - 1);

  useEffect(() => {
    setActive(0);
  }, [query]);

  useEffect(() => {
    if (activeIndex < 0) return;
    const el = listRef.current?.querySelector<HTMLElement>(`[data-index="${activeIndex}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [activeIndex]);

  const run = (row: PaletteRow) => {
    onClose();
    if (row.kind === "session") {
      if (row.chat) onOpenChat(row.session);
      else onOpenSession(row.session);
    } else if (row.kind === "project") {
      onSelectProject(row.project.id);
    } else {
      row.action.run();
    }
  };

  let index = -1;
  return (
    <div
      className="command-palette-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="command-palette" role="dialog" aria-modal="true" aria-label="命令面板">
        <label className="command-palette__search">
          <Search size={14} aria-hidden />
          <input
            ref={inputRef}
            type="text"
            value={query}
            placeholder="搜索对话、项目或操作…"
            aria-label="搜索对话、项目或操作"
            aria-activedescendant={activeIndex >= 0 ? `palette-row-${activeIndex}` : undefined}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                // Keep the app's double-Esc interrupt from seeing this one.
                event.preventDefault();
                event.stopPropagation();
                onClose();
              } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                event.preventDefault();
                if (flat.length === 0) return;
                const step = event.key === "ArrowDown" ? 1 : -1;
                setActive((activeIndex + step + flat.length) % flat.length);
              } else if (event.key === "Enter") {
                event.preventDefault();
                const row = flat[activeIndex];
                if (row) run(row);
              }
            }}
          />
          <span className="command-palette__kbd">Esc</span>
        </label>
        <div ref={listRef} className="command-palette__list custom-scrollbar scrollbar-autohide" role="listbox">
          {flat.length === 0 && <div className="command-palette__empty">没有匹配的对话、项目或操作</div>}
          {groups.map((group) => (
            <div key={group.title} className="command-palette__group">
              <div className="command-palette__label">{group.title}</div>
              {group.rows.map((row) => {
                index += 1;
                const i = index;
                const isActive = i === activeIndex;
                return (
                  <button
                    key={row.key}
                    id={`palette-row-${i}`}
                    data-index={i}
                    type="button"
                    role="option"
                    aria-selected={isActive}
                    className={`command-palette__row${isActive ? " is-active" : ""}`}
                    onMouseMove={() => {
                      if (!isActive) setActive(i);
                    }}
                    onClick={() => run(row)}
                  >
                    {row.kind === "session" ? (
                      <>
                        <span
                          className={
                            row.session.status === "running" || row.session.status === "starting"
                              ? `session-row__pulse is-${row.session.status}`
                              : doneUnreadIds?.has(row.session.id)
                                ? "session-row__dot is-done"
                                : `session-row__dot is-${row.session.status}`
                          }
                          aria-hidden
                        />
                        <span className="command-palette__title">{row.session.label}</span>
                        <span className="command-palette__meta">
                          {[row.chat ? "聊天" : projectName(row.session.projectId), agentLabel(row.session.agentId)]
                            .filter(Boolean)
                            .join(" · ")}
                        </span>
                        <span className="command-palette__hint">{shortRecency(row.session.lastActiveAt, now)}</span>
                      </>
                    ) : row.kind === "project" ? (
                      <>
                        <span className="command-palette__glyph" aria-hidden>
                          ▸
                        </span>
                        <span className="command-palette__title">{row.project.name}</span>
                        <span className="command-palette__meta">{row.project.rootPath}</span>
                      </>
                    ) : (
                      <>
                        <span className="command-palette__glyph" aria-hidden>
                          ›
                        </span>
                        <span className="command-palette__title">{row.action.label}</span>
                        {row.action.hint && <span className="command-palette__hint">{row.action.hint}</span>}
                      </>
                    )}
                  </button>
                );
              })}
            </div>
          ))}
        </div>
        <div className="command-palette__foot">
          <span>
            <b>↑↓</b> 选择
          </span>
          <span>
            <b>Enter</b> 打开
          </span>
          <span>
            <b>Ctrl K</b> 随时唤起
          </span>
        </div>
      </div>
    </div>
  );
}
