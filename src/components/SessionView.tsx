import { Check, ChevronDown, Copy, Eye, EyeOff, FileText, Globe, MessageSquareQuote, Pencil, Plus, Square, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { extractAcpUpdateText, mergeStreamText, userMessageAnchorId } from "../lib/acpTranscript";
import {
  activityBarLabel,
  activityHealth,
  formatAgo,
  isSubagentTool,
  isToolInProgress,
  stallBannerCopy,
  type ActivityHealth,
} from "../lib/activityHealth";
import { parseUnifiedDiff } from "../lib/annotations";
import { getFileDiff } from "../lib/api";
import { detectForceWebSearchInText, stripForceWebSearchPrefix } from "../lib/forceWebSearch";
import { newQuotePinId, type QuotePin } from "../lib/quoteComment";
import { buildMessagePresentation } from "../lib/messagePresentation";
import type { AgentConfig, Session, SessionEvent, SessionStatus, SessionViewMode } from "../lib/types";
import { ClippedBody } from "./ClippedBody";
import { LinkCwdContext, LinkedText } from "./LinkedText";
import { MarkdownBody } from "./MarkdownBody";
import { MessageOutline } from "./MessageOutline";
import { MessageTimestamp } from "./MessageTimestamp";
import { UserImageCard } from "./UserImageCard";


export type UserMessageAnchor = {
  messageId?: string;
  createdAt: string;
  text: string;
};

type SessionViewProps = {
  agent: AgentConfig;
  events: SessionEvent[];
  session: Session;
  viewMode: SessionViewMode;
  openSessions: Session[];
  /** Sticky auth / account warning for the active agent (e.g. Claude not logged in). */
  authBanner?: string | null;
  /** Start native login (browser/CLI). */
  onSignIn?: () => void | Promise<void>;
  signInBusy?: boolean;
  onTabSelect: (session: Session) => void;
  onTabClose: (sessionId: string) => void;
  onNewTab: () => void;
  onSessionStatusChange?: (status: SessionStatus) => void;
  onCapabilities?: (caps: import("../lib/types").CapabilitySnapshot | null) => void;
  /** P2-UX-4: edit You → truncate following + resend. */
  onEditResend?: (anchor: UserMessageAnchor, newText: string) => void | Promise<void>;
  /** Last ACP activity ms for this session (heartbeat / stale). */
  lastActivityAt?: number | null;
  /** Pending inline quote-comments for this dialog (sent with Composer text). */
  quotePins?: QuotePin[];
  onQuotePinsChange?: (pins: QuotePin[]) => void;
  /** Interrupt the live turn (same as Esc×2 / ■). */
  onInterrupt?: () => void | Promise<void>;
  /** Project id for on-demand workspace diffs in editing-tool cards. */
  projectId?: string;
  /** All live events (for expanding @-delegate child streams). */
  allEvents?: SessionEvent[];
  onSubtaskStop?: (childSessionId: string) => void;
  onSubtaskQuote?: (summary: string) => void;
  onSubtaskRetry?: (childSessionId: string) => void;
};

/** Tabs-only for the shared workspace-titlebar (rendered by App.tsx). */
export function SessionTabs({
  openSessions,
  session,
  onTabSelect,
  onTabClose,
  onNewTab,
  onRenameSession,
}: {
  openSessions: Session[];
  session: Session;
  onTabSelect: (session: Session) => void;
  onTabClose: (sessionId: string) => void;
  onNewTab: () => void;
  onRenameSession?: (sessionId: string, label: string) => void;
}) {
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const renameInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!renamingId) return;
    renameInputRef.current?.focus();
    renameInputRef.current?.select();
  }, [renamingId]);

  const commitRename = () => {
    if (!renamingId || !onRenameSession) {
      setRenamingId(null);
      return;
    }
    onRenameSession(renamingId, renameDraft.trim() || "New session");
    setRenamingId(null);
  };

  return (
    <div className="editor-tabs__tablist" role="tablist" aria-label="Session tabs">
      {openSessions.map((openSession) => {
        const busy =
          openSession.status === "running" || openSession.status === "starting";
        const isRenaming = renamingId === openSession.id;
        return (
          <div
            className={
              openSession.id === session.id
                ? `editor-tab is-active is-${openSession.status}`
                : `editor-tab is-${openSession.status}`
            }
            key={openSession.id}
            role="tab"
            aria-selected={openSession.id === session.id}
          >
            {busy && (
              <span
                className={`editor-tab__pulse is-${openSession.status}`}
                aria-hidden
              />
            )}
            {isRenaming ? (
              <input
                ref={renameInputRef}
                className="editor-tab__rename"
                value={renameDraft}
                aria-label="Rename tab"
                onChange={(e) => setRenameDraft(e.target.value)}
                onBlur={() => commitRename()}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    commitRename();
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    setRenamingId(null);
                  }
                }}
                onClick={(e) => e.stopPropagation()}
              />
            ) : (
              <button
                className="editor-tab__select"
                type="button"
                title={`${openSession.cwd} · double-click to rename`}
                onClick={() => onTabSelect(openSession)}
                onDoubleClick={(e) => {
                  if (!onRenameSession) return;
                  e.preventDefault();
                  e.stopPropagation();
                  setRenamingId(openSession.id);
                  setRenameDraft(openSession.label);
                }}
              >
                <span>{openSession.label}</span>
              </button>
            )}
            <button className="pill-action pill-action--icon pill-action--sm editor-tab__close" type="button" title={`Close ${openSession.label}`} aria-label={`Close ${openSession.label}`} onClick={() => onTabClose(openSession.id)}>
              <X size={12} />
            </button>
          </div>
        );
      })}
      <button className="pill-action pill-action--icon pill-action--sm editor-tab__new" type="button" title="New conversation" aria-label="New conversation" onClick={onNewTab}>
        <Plus size={13} />
      </button>
    </div>
  );
}

export function SessionView({
  agent,
  events,
  session,
  viewMode,
  openSessions,
  authBanner = null,
  onSignIn,
  signInBusy = false,
  onTabSelect,
  onTabClose,
  onNewTab,
  onSessionStatusChange,
  onCapabilities,
  onEditResend,
  lastActivityAt = null,
  quotePins = [],
  onQuotePinsChange,
  onInterrupt,
  projectId,
  allEvents = [],
  onSubtaskStop,
  onSubtaskQuote,
  onSubtaskRetry,
}: SessionViewProps) {
  // Show thinking/tool rows by default (they render collapsed). Eye can hide them entirely.
  const [detailsVisible, setDetailsVisible] = useState(true);
  const [expandedChildId, setExpandedChildId] = useState<string | null>(null);
  // Only a real turn earns the top bar. Connecting is background work — the
  // composer floats a pill for it so the stage never resizes mid-reconnect.
  const isLive = session.status === "running";

  /** childSessionId → result event (latest wins). */
  const subtaskResults = useMemo(() => {
    const map = new Map<string, Extract<SessionEvent, { type: "subtask_result" }>>();
    for (const e of events) {
      if (e.type === "subtask_result" && e.childSessionId) {
        map.set(e.childSessionId, e);
      }
    }
    return map;
  }, [events]);

  return (
    <section className="session-view" aria-label="Session view">

      {isLive && (
        <SessionActivityBar status={session.status} lastActivityAt={lastActivityAt} />
      )}

      {authBanner && (
        <div className="session-auth-banner" role="status">
          <div className="session-auth-banner__copy">
            <strong>Sign in required</strong>
            <span>{authBanner}</span>
          </div>
          {onSignIn && (
            <button
              className="session-auth-banner__button"
              type="button"
              disabled={signInBusy}
              onClick={() => void onSignIn()}
            >
              {signInBusy ? "Opening login…" : "Sign in with Claude"}
            </button>
          )}
        </div>
      )}

      <div className="session-stage">
        <div className="session-stage__clean">
          <CleanPlaceholder
            agent={agent}
            session={session}
            events={events}
            detailsVisible={detailsVisible}
            onDetailsToggle={() => setDetailsVisible((visible) => !visible)}
            authBanner={authBanner}
            onSignIn={onSignIn}
            signInBusy={signInBusy}
            onEditResend={onEditResend}
            lastActivityAt={lastActivityAt}
            quotePins={quotePins}
            onQuotePinsChange={onQuotePinsChange}
            onInterrupt={onInterrupt}
            projectId={projectId}
            allEvents={allEvents}
            expandedChildId={expandedChildId}
            setExpandedChildId={setExpandedChildId}
            subtaskResults={subtaskResults}
            onSubtaskStop={onSubtaskStop}
            onSubtaskQuote={onSubtaskQuote}
            onSubtaskRetry={onSubtaskRetry}
          />
        </div>
      </div>
    </section>
  );
}

function FileChangeCard({
  path,
  changeType,
  projectId,
  createdAt,
  index,
  revision,
  onLineComment,
}: {
  path: string;
  changeType: string;
  projectId?: string;
  createdAt: string;
  index: number;
  revision?: number;
  /** Click a line → pin a line annotation for the next send. */
  onLineComment?: (args: {
    filePath: string;
    side: "old" | "new";
    lineNumber: number;
    quoted: string;
    comment: string;
  }) => void;
}) {
  const [open, setOpen] = useState(false);
  const [diff, setDiff] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [commenting, setCommenting] = useState<{
    side: "old" | "new";
    lineNumber: number;
    quoted: string;
  } | null>(null);
  const [commentDraft, setCommentDraft] = useState("");

  // A tool can edit the same file more than once in a turn. Reset the lazy
  // preview so an already-open card refreshes instead of showing stale lines.
  useEffect(() => {
    if (revision == null) return;
    setDiff(null);
    setError(null);
    setLoading(false);
    setCommenting(null);
    setCommentDraft("");
  }, [revision]);

  useEffect(() => {
    if (!open || diff !== null || loading) return;
    if (!projectId || !path) {
      setError("No project context for diff");
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    void getFileDiff(projectId, path)
      .then((text) => {
        if (!cancelled) setDiff(text || "(empty diff)");
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, projectId, path, diff, loading]);

  const fileName = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
  const parsed = useMemo(() => (diff ? parseUnifiedDiff(diff) : []), [diff]);

  return (
    <details
      className="event-card event-card--collapsible event-card--file_change"
      key={`file_change-${createdAt}-${index}`}
      open={open}
      onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}
    >
      <summary className="event-card__summary" title={`${changeType}: ${path}`}>
        <span className="event-card__type">File</span>
        <span className="event-card__preview-wrap">
          <span className="event-card__preview">
            {changeType} · {fileName}
          </span>
        </span>
      </summary>
      <div className="event-card__expand-slot">
        <div className="file-diff-card">
          <div className="file-diff-card__path" title={path}>
            {path}
          </div>
          {loading && <div className="file-diff-card__muted">Loading diff…</div>}
          {error && <div className="file-diff-card__muted">{error}</div>}
          {diff !== null && (
            <pre className="file-diff-card__body custom-scrollbar scrollbar-autohide">
              {parsed.map((line, i) => {
                const cls =
                  line.type === "add"
                    ? "file-diff-card__line is-add"
                    : line.type === "del"
                      ? "file-diff-card__line is-del"
                      : line.type === "hunk"
                        ? "file-diff-card__line is-hunk"
                        : "file-diff-card__line";
                const side: "old" | "new" | null =
                  line.type === "add"
                    ? "new"
                    : line.type === "del"
                      ? "old"
                      : line.type === "ctx"
                        ? "new"
                        : null;
                const lineNo =
                  side === "new" ? line.newLine : side === "old" ? line.oldLine : null;
                const canComment =
                  Boolean(onLineComment) &&
                  side != null &&
                  lineNo != null &&
                  (line.type === "add" || line.type === "del" || line.type === "ctx");
                return (
                  <span className={cls} key={i}>
                    <span className="file-diff-card__gutter" aria-hidden>
                      <span className="file-diff-card__ln file-diff-card__ln--old">
                        {line.oldLine ?? ""}
                      </span>
                      <span className="file-diff-card__ln file-diff-card__ln--new">
                        {line.newLine ?? ""}
                      </span>
                    </span>
                    {canComment ? (
                      <button
                        type="button"
                        className="file-diff-card__code"
                        title="评论此行"
                        onClick={(e) => {
                          e.preventDefault();
                          e.stopPropagation();
                          setCommenting({
                            side: side!,
                            lineNumber: lineNo!,
                            quoted: line.raw.replace(/^[-+ ]/, ""),
                          });
                          setCommentDraft("");
                        }}
                      >
                        {line.raw || " "}
                      </button>
                    ) : (
                      <span className="file-diff-card__code">{line.raw || " "}</span>
                    )}
                    {"\n"}
                  </span>
                );
              })}
            </pre>
          )}
          {commenting && onLineComment && (
            <div className="file-diff-card__comment">
              <div className="file-diff-card__comment-meta">
                {path}:{commenting.lineNumber}（{commenting.side === "new" ? "新" : "旧"}）
              </div>
              <input
                className="file-diff-card__comment-input"
                value={commentDraft}
                placeholder="写评论，回车添加…"
                autoFocus
                onChange={(e) => setCommentDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    setCommenting(null);
                    return;
                  }
                  if (e.key === "Enter" && commentDraft.trim()) {
                    e.preventDefault();
                    onLineComment({
                      filePath: path,
                      side: commenting.side,
                      lineNumber: commenting.lineNumber,
                      quoted: commenting.quoted,
                      comment: commentDraft.trim(),
                    });
                    setCommenting(null);
                    setCommentDraft("");
                  }
                }}
              />
              <div className="file-diff-card__comment-actions">
                <button
                  type="button"
                  className="pill-action subtask-card__btn"
                  onClick={() => setCommenting(null)}
                >
                  取消
                </button>
                <button
                  type="button"
                  className="pill-action subtask-card__btn"
                  disabled={!commentDraft.trim()}
                  onClick={() => {
                    if (!commentDraft.trim()) return;
                    onLineComment({
                      filePath: path,
                      side: commenting.side,
                      lineNumber: commenting.lineNumber,
                      quoted: commenting.quoted,
                      comment: commentDraft.trim(),
                    });
                    setCommenting(null);
                    setCommentDraft("");
                  }}
                >
                  添加
                </button>
              </div>
            </div>
          )}
          <div className="file-diff-card__note">
            当前工作区 diff · 行号与 git 对齐 · 点代码行可加评论
          </div>
        </div>
      </div>
    </details>
  );
}

/** One-click copy of a Reply's full source text (markdown, not rendered HTML). */
function CopyReplyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  const onCopy = useCallback(() => {
    const value = text;
    void (async () => {
      try {
        await navigator.clipboard.writeText(value);
      } catch {
        // Fallback for blocked clipboard APIs (rare in Tauri WebView2).
        try {
          const ta = document.createElement("textarea");
          ta.value = value;
          ta.setAttribute("readonly", "");
          ta.style.position = "fixed";
          ta.style.left = "-9999px";
          document.body.appendChild(ta);
          ta.select();
          document.execCommand("copy");
          document.body.removeChild(ta);
        } catch {
          return;
        }
      }
      setCopied(true);
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => setCopied(false), 1500);
    })();
  }, [text]);

  return (
    <button
      type="button"
      className={`pill-action pill-action--icon event-card__copy${copied ? " is-copied" : ""}`}
      title={copied ? "Copied" : "Copy reply"}
      aria-label={copied ? "Copied" : "Copy reply"}
      onMouseDown={(e) => e.preventDefault()}
      onClick={onCopy}
    >
      {copied ? <Check size={13} aria-hidden /> : <Copy size={13} aria-hidden />}
    </button>
  );
}

type ToolCallEvent = Extract<SessionEvent, { type: "tool_call" }>;

function hasUnifiedDiff(text: string): boolean {
  return /^@@\s+-\d+/m.test(text) && /^(?:\+\+\+|---|[+-])\s?/m.test(text);
}

/**
 * Older transcripts only persisted the one-line diff summary. For those
 * cards, retrieve the current workspace diff when the tool card is opened.
 * New ACP events already carry the full diff from acpTranscript.ts.
 */
function ToolCallBody({
  event,
  body,
  projectId,
  open,
}: {
  event: ToolCallEvent;
  body: string;
  projectId?: string;
  open: boolean;
}) {
  const [workspaceDiff, setWorkspaceDiff] = useState<string | null>(null);
  const [diffError, setDiffError] = useState<string | null>(null);
  const [requested, setRequested] = useState(false);
  const toolLabel = `${event.toolName ?? ""} ${event.title ?? ""}`.toLowerCase();
  const isFileEditTool = /edit|write|patch|apply|replace|file/.test(toolLabel);
  const needsWorkspaceDiff = Boolean(
    projectId &&
      event.path &&
      !hasUnifiedDiff(body) &&
      (isFileEditTool || /\[diff\]/i.test(body)),
  );

  useEffect(() => {
    setWorkspaceDiff(null);
    setDiffError(null);
    setRequested(false);
  }, [event.createdAt, event.toolCallId, event.path]);

  useEffect(() => {
    if (!open || !needsWorkspaceDiff || requested || !projectId || !event.path) return;
    let cancelled = false;
    let timeoutId: number | undefined;
    setRequested(true);
    const timeout = new Promise<never>((_, reject) => {
      timeoutId = window.setTimeout(() => reject(new Error("Diff request timed out")), 8000);
    });
    void Promise.race([getFileDiff(projectId, event.path), timeout])
      .then((text) => {
        if (!cancelled) setWorkspaceDiff(text.trim() || "(empty diff)");
      })
      .catch((error) => {
        if (!cancelled) setDiffError(error instanceof Error ? error.message : String(error));
      })
      .finally(() => {
        if (timeoutId != null) window.clearTimeout(timeoutId);
      });
    return () => {
      cancelled = true;
    };
  }, [event.path, getFileDiff, needsWorkspaceDiff, open, projectId, requested]);

  const displayBody = hasUnifiedDiff(body)
    ? body
    : workspaceDiff
      ? `${body}\n\n${workspaceDiff}`
      : diffError
        ? `${body}\n\n[full diff unavailable] ${diffError}`
        : body;
  const isDiff = hasUnifiedDiff(displayBody) || needsWorkspaceDiff;

  return (
    <ClippedBody
      className="event-card__clip"
      maxHeight={isDiff ? 560 : 220}
    >
      <pre className={`event-card__body event-card__body--tool${isDiff ? " event-card__body--diff" : ""}`}>
        <LinkedText text={displayBody} />
        {open && needsWorkspaceDiff && !workspaceDiff && !diffError && "\n\nLoading full diff…"}
      </pre>
    </ClippedBody>
  );
}

function CleanPlaceholder({
  agent,
  session,
  events,
  detailsVisible,
  onDetailsToggle,
  authBanner = null,
  onSignIn,
  signInBusy = false,
  onEditResend,
  lastActivityAt = null,
  quotePins = [],
  onQuotePinsChange,
  onInterrupt,
  projectId,
  allEvents = [],
  expandedChildId = null,
  setExpandedChildId,
  subtaskResults,
  onSubtaskStop,
  onSubtaskQuote,
  onSubtaskRetry,
}: {
  agent: AgentConfig;
  session: Session;
  events: SessionEvent[];
  detailsVisible: boolean;
  onDetailsToggle: () => void;
  authBanner?: string | null;
  onSignIn?: () => void | Promise<void>;
  signInBusy?: boolean;
  onEditResend?: (anchor: UserMessageAnchor, newText: string) => void | Promise<void>;
  lastActivityAt?: number | null;
  quotePins?: QuotePin[];
  onQuotePinsChange?: (pins: QuotePin[]) => void;
  onInterrupt?: () => void | Promise<void>;
  projectId?: string;
  allEvents?: SessionEvent[];
  expandedChildId?: string | null;
  setExpandedChildId?: (id: string | null) => void;
  subtaskResults?: Map<string, Extract<SessionEvent, { type: "subtask_result" }>>;
  onSubtaskStop?: (childSessionId: string) => void;
  onSubtaskQuote?: (summary: string) => void;
  onSubtaskRetry?: (childSessionId: string) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  /** When true, new content may stick the viewport to the bottom. */
  const stickToBottomRef = useRef(true);
  /** State mirror of stickToBottom so the jump-to-bottom chip can re-render. */
  const [atBottom, setAtBottom] = useState(true);
  const isRunning = session.status === "running";
  // Workspace file changes already live in the right-side Changed Files panel.
  // Keep them in the transcript/history data, but do not create an empty
  // collapsible "File" row after every turn in the conversation rail.
  const visibleEvents = useMemo(
    () => events.filter((event) => event.type !== "file_change"),
    [events],
  );
  const presentationItems = useMemo(
    () => buildMessagePresentation(visibleEvents, session.id),
    [session.id, visibleEvents],
  );
  const isLongTranscript = presentationItems.length >= 80;
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState("");
  const [editBusy, setEditBusy] = useState(false);
  const [expandedToolKeys, setExpandedToolKeys] = useState<Set<string>>(() => new Set());
  // Local clock for stall UI only — does not drive list scroll keys.
  const [healthNow, setHealthNow] = useState(() => Date.now());
  useEffect(() => {
    if (!isRunning) return;
    const id = window.setInterval(() => setHealthNow(Date.now()), 2000);
    return () => window.clearInterval(id);
  }, [isRunning]);

  // After send: last card is still You → show ghost "Waiting for agent…"
  const lastVisible = visibleEvents[visibleEvents.length - 1];
  const awaitingFirstChunk =
    isRunning &&
    (!lastVisible ||
      lastVisible.type === "user_message" ||
      lastVisible.type === "handoff_prepared");

  const health = activityHealth(session.status, lastActivityAt, healthNow);
  const openTool = useMemo(() => {
    for (let i = visibleEvents.length - 1; i >= 0; i -= 1) {
      const e = visibleEvents[i];
      if (e.type === "tool_call" && isToolInProgress(e.status)) {
        return e;
      }
    }
    return null;
  }, [visibleEvents]);
  const midTurn = isRunning && !awaitingFirstChunk;
  // Stall copy is about a live turn going quiet. A reconnect is not a stalled
  // turn — inserting this card while warming up only shoves the transcript.
  const showStallBanner =
    isRunning &&
    (awaitingFirstChunk || health === "quiet" || health === "stalled" || health === "stuck");

  // Content fingerprint — only real transcript changes should pin-scroll (never a clock tick).
  const scrollKey = useMemo(() => {
    const last = visibleEvents[visibleEvents.length - 1];
    const lastSig = last
      ? `${last.type}:${last.createdAt}:${"text" in last ? String(last.text).length : ""}:${"status" in last ? last.status : ""}`
      : "empty";
    return `${session.id}|${visibleEvents.length}|${lastSig}|wait:${awaitingFirstChunk}|health:${health}`;
  }, [session.id, visibleEvents, awaitingFirstChunk, health]);

  const prevAwaitingRef = useRef(false);

  // Own send → jump back to live tail (even if you were reading history).
  useEffect(() => {
    if (awaitingFirstChunk && !prevAwaitingRef.current) {
      stickToBottomRef.current = true;
      setAtBottom(true);
    }
    prevAwaitingRef.current = awaitingFirstChunk;
  }, [awaitingFirstChunk]);

  // Switching dialog → follow the new session's tail by default.
  // Also drop any leftover text selection so QuoteOverlay chrome / WebView2
  // selection ghosts cannot hide the next dialog's transcript until a click.
  useEffect(() => {
    stickToBottomRef.current = true;
    setAtBottom(true);
    window.getSelection()?.removeAllRanges();
  }, [session.id]);

  const onListScroll = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    // User scrolled up to read history → stop auto-jumping to bottom.
    const distance = el.scrollHeight - el.scrollTop - el.clientHeight;
    const nearBottom = distance < 80;
    stickToBottomRef.current = nearBottom;
    setAtBottom(nearBottom);
  }, []);

  const jumpToBottom = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    stickToBottomRef.current = true;
    setAtBottom(true);
    el.scrollTop = el.scrollHeight;
  }, []);

  // Stick to bottom only while the user is already near the end (chat apps pattern).
  // Never key this off the 1s activity clock / stale flag — that caused the 5–10s jump bug.
  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    if (!stickToBottomRef.current) return;
    el.scrollTop = el.scrollHeight;
  }, [scrollKey]);

  return (
    // Relative paths in agent text resolve against this dialog's project root.
    <LinkCwdContext.Provider value={session.cwd || null}>
    <div className="clean-surface">
      <button
        className="pill-action pill-action--icon icon-button--eye"
        type="button"
        title={
          "Clean View · live stream · thinking/tools collapsed by default"
          + " — " + (detailsVisible ? "hide" : "show") + " details"
        }
        onClick={onDetailsToggle}
      >
        {detailsVisible ? <Eye size={14} /> : <EyeOff size={14} />}
      </button>
      <div className="clean-surface__body">
      {!atBottom && visibleEvents.length > 0 && (
        <button
          type="button"
          className="scroll-to-bottom"
          title="Jump to latest message"
          aria-label="Jump to latest message"
          onClick={jumpToBottom}
        >
          <ChevronDown size={16} />
          <span>Latest</span>
        </button>
      )}
      <div
        ref={listRef}
        className={`${detailsVisible ? "event-list" : "event-list is-details-hidden"}${isLongTranscript ? " is-long" : ""} scrollbar-hidden`}
        onScroll={onListScroll}
      >
        {/* Quote UI is isolated so its setState does not re-render cards (keeps selection).
            key=session.id remounts overlay state (评论 button / draft) per dialog. */}
        {onQuotePinsChange && (
          <QuoteOverlay
            key={session.id}
            sessionId={session.id}
            listRef={listRef}
            pins={quotePins}
            onPinsChange={onQuotePinsChange}
          />
        )}
        {/* No connect card here on purpose: warming shows as a floating pill on
            the composer, so the transcript never reflows while reconnecting. */}
        {visibleEvents.length === 0 && (
          <div className="clean-empty" role="status">
            <p className="clean-empty__title">
              {authBanner ? "Sign in to continue" : `Message ${agent.label}`}
            </p>
            <p className="clean-empty__hint">
              {authBanner
                ? "Use the Sign in button above — it opens Claude’s browser login. Come back here when done."
                : "Type below to warm the agent in the background, then send when ready."}
            </p>
            {authBanner && onSignIn && (
              <button
                className="session-auth-banner__button"
                type="button"
                disabled={signInBusy}
                onClick={() => void onSignIn()}
              >
                {signInBusy ? "Opening login…" : "Sign in with Claude"}
              </button>
            )}
          </div>
        )}
        {(() => {
          const renderEvent = (
            event: SessionEvent,
            index: number,
          ) => {
          // @-delegate cards: skip started if we already have a result for the same child.
          if (event.type === "subtask_started") {
            if (subtaskResults?.has(event.childSessionId)) return null;
            const target = event.modelId
              ? `@${event.agentId}/${event.modelId}`
              : `@${event.agentId}`;
            const childStream = allEvents.filter((e) => e.sessionId === event.childSessionId);
            const expanded = expandedChildId === event.childSessionId;
            return (
              <div
                className="event-card event-card--subtask is-running"
                key={`subtask-${event.childSessionId}-run`}
              >
                <div className="subtask-card__row">
                  <span className="subtask-card__glyph" aria-hidden>⟳</span>
                  <span className="subtask-card__title">
                    {target} · {event.prompt.slice(0, 48)}
                    {event.prompt.length > 48 ? "…" : ""}
                  </span>
                  <span className="subtask-card__meta">进行中</span>
                  {onSubtaskStop && (
                    <button
                      type="button"
                      className="pill-action subtask-card__btn"
                      onClick={() => onSubtaskStop(event.childSessionId)}
                    >
                      停止
                    </button>
                  )}
                </div>
                {childStream.length > 0 && (
                  <button
                    type="button"
                    className="pill-action subtask-card__btn subtask-card__expand"
                    onClick={() =>
                      setExpandedChildId?.(expanded ? null : event.childSessionId)
                    }
                  >
                    {expanded ? "收起" : "展开"}
                  </button>
                )}
                {expanded && (
                  <div className="subtask-card__stream custom-scrollbar scrollbar-autohide">
                    {childStream.map((ce, ci) =>
                      ce.type === "assistant_message" ||
                      ce.type === "user_message" ||
                      ce.type === "thought" ||
                      ce.type === "tool_call" ? (
                        <div key={`${ce.type}-${ci}`} className="subtask-card__stream-line">
                          <strong>
                            {ce.type === "assistant_message"
                              ? "Reply"
                              : ce.type === "thought"
                                ? "Thinking"
                                : ce.type === "tool_call"
                                  ? "Tool"
                                  : "You"}
                          </strong>
                          <span>{(ce.text || "").slice(0, 400)}</span>
                        </div>
                      ) : null,
                    )}
                  </div>
                )}
              </div>
            );
          }

          if (event.type === "subtask_result") {
            const startEv = events.find(
              (e) =>
                e.type === "subtask_started" && e.childSessionId === event.childSessionId,
            );
            const prompt =
              startEv && startEv.type === "subtask_started" ? startEv.prompt : "";
            const modelId =
              startEv && startEv.type === "subtask_started" ? startEv.modelId : undefined;
            const agentLabel =
              startEv && startEv.type === "subtask_started"
                ? startEv.agentLabel || startEv.agentId
                : event.agentId;
            const target = modelId
              ? `@${event.agentId}/${modelId}`
              : `@${event.agentId}`;
            const ok = event.status === "done";
            const glyph = ok ? "✓" : "✕";
            const duration =
              event.durationMs != null
                ? event.durationMs < 60_000
                  ? `${Math.round(event.durationMs / 1000)}秒`
                  : `${Math.floor(event.durationMs / 60_000)}分${Math.round((event.durationMs % 60_000) / 1000)}秒`
                : "";
            const expanded = expandedChildId === event.childSessionId;
            const childStream = allEvents.filter((e) => e.sessionId === event.childSessionId);
            return (
              <div
                className={`event-card event-card--subtask is-${event.status}`}
                key={`subtask-${event.childSessionId}-done`}
              >
                <div className="subtask-card__row">
                  <span className="subtask-card__glyph" aria-hidden>
                    {glyph}
                  </span>
                  <span className="subtask-card__title" title={prompt}>
                    {target} · {(prompt || agentLabel).slice(0, 48)}
                    {(prompt || agentLabel).length > 48 ? "…" : ""}
                  </span>
                  <span className="subtask-card__meta">
                    {ok
                      ? duration
                      : event.error ||
                        (event.status === "timeout"
                          ? "超时"
                          : event.status === "cancelled"
                            ? "已取消"
                            : "失败")}
                  </span>
                  <button
                    type="button"
                    className="pill-action subtask-card__btn"
                    onClick={() =>
                      setExpandedChildId?.(expanded ? null : event.childSessionId)
                    }
                  >
                    {expanded ? "收起" : "展开"}
                  </button>
                  {ok && onSubtaskQuote && event.summary && (
                    <button
                      type="button"
                      className="pill-action subtask-card__btn"
                      onClick={() => onSubtaskQuote(event.summary)}
                    >
                      引用
                    </button>
                  )}
                  {!ok && onSubtaskRetry && (
                    <button
                      type="button"
                      className="pill-action subtask-card__btn"
                      onClick={() => onSubtaskRetry(event.childSessionId)}
                    >
                      重试
                    </button>
                  )}
                </div>
                {expanded && (
                  <div className="subtask-card__stream custom-scrollbar scrollbar-autohide">
                    {event.summary && (
                      <div className="subtask-card__stream-line">
                        <strong>Summary</strong>
                        <MarkdownBody text={event.summary} className="event-card__body" />
                      </div>
                    )}
                    {childStream.map((ce, ci) =>
                      ce.type === "assistant_message" ||
                      ce.type === "thought" ||
                      ce.type === "tool_call" ? (
                        <div key={`${ce.type}-${ci}`} className="subtask-card__stream-line">
                          <strong>
                            {ce.type === "assistant_message"
                              ? "Reply"
                              : ce.type === "thought"
                                ? "Thinking"
                                : "Tool"}
                          </strong>
                          <span>{(ce.text || "").slice(0, 600)}</span>
                        </div>
                      ) : null,
                    )}
                  </div>
                )}
              </div>
            );
          }

          const isCollapsible = event.type === "thought" || event.type === "tool_call";
          const toolRunning =
            event.type === "tool_call" && isToolInProgress(event.status);
          const toolStalled =
            toolRunning && (health === "stalled" || health === "stuck" || health === "quiet");

          // Thought is "live" only while the session is running and nothing
          // (tool / reply / newer thought) has followed it in this turn.
          let thoughtLive = false;
          let thoughtDurationSec: number | undefined;
          if (event.type === "thought") {
            const startMs = Date.parse(event.createdAt);
            let endMs = healthNow;
            let followed = false;
            for (let j = index + 1; j < visibleEvents.length; j++) {
              const n = visibleEvents[j];
              if (n.sessionId !== session.id) continue;
              if (
                n.type === "thought" ||
                n.type === "assistant_message" ||
                n.type === "tool_call" ||
                n.type === "user_message"
              ) {
                followed = true;
                const t = Date.parse(n.createdAt);
                if (Number.isFinite(t)) endMs = t;
                break;
              }
            }
            thoughtLive = isRunning && !followed;
            if (Number.isFinite(startMs) && startMs > 0) {
              thoughtDurationSec = Math.max(0, Math.round((endMs - startMs) / 1000));
            }
          }

          const toolStatusLabel = toolStalled
            ? health === "stuck"
              ? "no updates · appears stuck"
              : health === "stalled"
                ? "no updates · may be stuck"
                : "no updates recently"
            : event.type === "tool_call"
              ? event.status
              : undefined;

          // Collapsed summary: fixed short type + SHORT one-line teaser.
          // Full body only lives in the expanded clip box (never in the summary).
          const label =
            event.type === "thought"
              ? thoughtLive
                ? "Thinking…"
                : thoughtDurationSec != null && thoughtDurationSec > 0
                  ? `Thought · ${thoughtDurationSec}s`
                  : "Thought"
              : event.type === "tool_call"
                ? "Tool"
                : event.type === "user_message"
                  ? "You"
                  : event.type === "assistant_message"
                    ? (event.agentId ? "Reply" : "Notification")
                    : event.type === "handoff_prepared"
                      ? "Handoff"
                      : event.type.replace(/_/g, " ");

          if (event.type === "file_change") {
            return (
              <FileChangeCard
                key={`file_change-${event.createdAt}-${index}`}
                path={event.path}
                changeType={event.changeType}
                projectId={projectId}
                createdAt={event.createdAt}
                index={index}
                revision={event.revision}
                onLineComment={
                  onQuotePinsChange
                    ? ({ filePath, side, lineNumber, quoted, comment }) => {
                        const sideLabel = side === "new" ? "新" : "旧";
                        const pin: QuotePin = {
                          id: newQuotePinId(),
                          quoted: `${filePath}:${lineNumber}（${sideLabel}）\n${quoted}`,
                          comment,
                          x: 0,
                          y: 0,
                        };
                        onQuotePinsChange([...(quotePins ?? []), pin]);
                      }
                    : undefined
                }
              />
            );
          }

          // Handoff: a marker, not a wall of text. The notes themselves ride
          // along with the next message the user sends (never shown here or
          // pasted into the composer).
          const body =
            event.type === "handoff_prepared"
              ? `Notes prepared for **${event.targetAgentId}** — they will be attached to your next message.\n\nFull notes: \`${event.handoffPath}\``
              : event.text;

          const oneLine = (s: string) => s.replace(/\s+/g, " ").trim();
          const clipTeaser = (s: string, max = 42) => {
            const t = oneLine(s);
            if (!t) return "";
            return t.length > max ? `${t.slice(0, max - 1)}…` : t;
          };

          // Tools: keep the summary compact; the expanded body carries the
          // complete diff/output when the agent provided it.
          let previewFull = "";
          if (event.type === "tool_call") {
            const fileName = event.path
              ? event.path.split(/[\\/]/).filter(Boolean).pop() ?? ""
              : "";
            const bits = [
              event.title ? oneLine(String(event.title)) : "",
              fileName && fileName !== event.title ? fileName : "",
              toolStatusLabel || (event.status ? String(event.status) : ""),
            ].filter(Boolean);
            previewFull = bits.join(" · ");
          } else if (typeof body === "string") {
            previewFull = oneLine(body);
          }
          const preview = clipTeaser(previewFull, 48) || "…";

          const useMarkdown =
            event.type === "assistant_message" ||
            event.type === "user_message" ||
            event.type === "thought" ||
            event.type === "handoff_prepared";

          // Thought: open while streaming; collapsed when done (user can expand).
          // Tool: collapsed by default; auto-expand when stalled.
          // User / assistant: always fully visible.
          if (isCollapsible) {
            const forceOpen =
              thoughtLive ||
              (toolStalled && (health === "stalled" || health === "stuck"));
            const toolKey =
              event.type === "tool_call"
                ? `${event.toolCallId ?? event.createdAt}-${index}`
                : null;
            const toolOpen =
              event.type === "tool_call" &&
              (forceOpen || (toolKey != null && expandedToolKeys.has(toolKey)));
            return (
              <details
                className={`event-card event-card--collapsible event-card--${event.type}${toolRunning ? " is-tool-running" : ""}${toolStalled ? " is-tool-stalled" : ""}${toolStalled && health === "stuck" ? " is-tool-stuck" : ""}${thoughtLive ? " is-thought-live" : ""}`}
                // Remount when thought stops streaming so it starts collapsed (uncontrolled).
                key={`${event.type}-${event.createdAt}-${index}${event.type === "thought" ? (thoughtLive ? "-live" : "-done") : ""}`}
                open={event.type === "tool_call" ? toolOpen : forceOpen ? true : undefined}
                onToggle={(e) => {
                  if (event.type !== "tool_call" || toolKey == null) return;
                  const isOpen = (e.currentTarget as HTMLDetailsElement).open;
                  setExpandedToolKeys((current) => {
                    const next = new Set(current);
                    if (isOpen) next.add(toolKey);
                    else next.delete(toolKey);
                    return next;
                  });
                }}
              >
                <summary className="event-card__summary" title={previewFull || label}>
                  <span className="event-card__type">
                    {label}
                    {thoughtLive && <span className="event-card__live-dot" aria-hidden />}
                    {toolRunning && !toolStalled && <span className="event-card__live-dot" aria-hidden />}
                    {toolStalled && <span className="event-card__stall-dot" aria-hidden />}
                  </span>
                  <span className="event-card__preview-wrap">
                    <span className="event-card__preview">
                      {event.type === "thought" && thoughtLive
                        ? "…"
                        : preview}
                    </span>
                  </span>
                </summary>
                <div className="event-card__expand-slot">
                  {event.type === "tool_call" ? (
                    <ToolCallBody
                      event={event}
                      body={body}
                      projectId={projectId}
                      open={toolOpen}
                    />
                  ) : (
                    <ClippedBody className="event-card__clip" maxHeight={260}>
                      {useMarkdown && typeof body === "string" ? (
                        <MarkdownBody text={body} className="event-card__body event-card__body--clipped-md" />
                      ) : (
                        <pre className="event-card__body event-card__body--tool">
                          {typeof body === "string" ? <LinkedText text={body} /> : body}
                        </pre>
                      )}
                    </ClippedBody>
                  )}
                </div>
              </details>
            );
          }

          const isUser = event.type === "user_message";
          const userKey = isUser
            ? userMessageAnchorId(event)
            : `${event.type}-${event.createdAt}-${index}`;
          const isEditing = isUser && editingKey === userKey;

          // Mode tone for color accent (You card bar + Reply mode tag)
          const tone =
            isUser && "modeLabel" in event
              ? modeTone(event.modeLabel)
              : !isUser && "modeLabel" in event
                ? modeTone(event.modeLabel)
                : undefined;

          return (
            <article
              className={`event-card event-card--${event.type}${isEditing ? " is-editing" : ""}`}
              key={`${event.type}-${event.createdAt}-${index}`}
              id={isUser ? userKey : undefined}
              {...(tone ? { "data-mode-tone": tone } : {})}
            >
              {event.type !== "user_message" && event.type !== "assistant_message" && (
                <div className="event-card__icon">
                  <FileText size={15} />
                </div>
              )}
              <div className="event-card__main">
                <div className="event-card__type-row">
                  <span className="event-card__type">
                    {label}
                    {isUser &&
                      (("forceWebSearch" in event && event.forceWebSearch) ||
                        detectForceWebSearchInText(event.text)) && (
                        <span
                          className="event-card__web-badge"
                          title="发送时已要求模型先联网检索"
                          aria-label="本条要求联网检索"
                        >
                          <Globe size={12} aria-hidden />
                        </span>
                      )}
                  </span>
                  <span className="event-card__type-row__right">
                    <span className="event-card__time-edit">
                      <MessageTimestamp createdAt={event.createdAt} />
                      {isUser && onEditResend && !isEditing && (
                        <button
                          type="button"
                          className="pill-action pill-action--icon event-card__edit"
                          title="Edit & resend (drops later messages)"
                          aria-label="Edit and resend this message"
                          onClick={() => {
                            setEditingKey(userKey);
                            setEditDraft(stripForceWebSearchPrefix(event.text));
                          }}
                        >
                          <Pencil size={13} aria-hidden />
                        </button>
                      )}
                    </span>
                  </span>
                </div>
                {isEditing ? (
                  <div className="event-card__edit-form">
                    <textarea
                      className="event-card__edit-input"
                      value={editDraft}
                      rows={Math.min(12, Math.max(3, editDraft.split("\n").length + 1))}
                      disabled={editBusy}
                      autoFocus
                      onChange={(e) => setEditDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Escape") {
                          e.preventDefault();
                          setEditingKey(null);
                        }
                        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                          e.preventDefault();
                          const next = editDraft.trim();
                          if (!next || editBusy) return;
                          setEditBusy(true);
                          void Promise.resolve(
                            onEditResend?.(
                              {
                                messageId: event.messageId,
                                createdAt: event.createdAt,
                                text: event.text,
                              },
                              next
                            )
                          ).finally(() => {
                            setEditBusy(false);
                            setEditingKey(null);
                          });
                        }
                      }}
                    />
                    <p className="event-card__edit-hint">
                      Resend truncates everything after this message. Ctrl+Enter to confirm.
                    </p>
                    <div className="event-card__edit-actions">
                      <button
                        type="button"
                        className="pill-action event-card__edit-cancel"
                        disabled={editBusy}
                        onClick={() => setEditingKey(null)}
                      >
                        Cancel
                      </button>
                      <button
                        type="button"
                        className="pill-action event-card__edit-confirm"
                        disabled={editBusy || !editDraft.trim()}
                        onClick={() => {
                          const next = editDraft.trim();
                          if (!next) return;
                          setEditBusy(true);
                          void Promise.resolve(
                            onEditResend?.(
                              {
                                messageId: event.messageId,
                                createdAt: event.createdAt,
                                text: event.text,
                              },
                              next
                            )
                          ).finally(() => {
                            setEditBusy(false);
                            setEditingKey(null);
                          });
                        }}
                      >
                        {editBusy ? "Resending…" : "Resend from here"}
                      </button>
                    </div>
                  </div>
                ) : (
                  <>
                    {isUser &&
                      "attachments" in event &&
                      Array.isArray(event.attachments) &&
                      event.attachments.length > 0 && (
                        <UserImageCard attachments={event.attachments} />
                      )}
                    {(() => {
                      const displayBody =
                        isUser && typeof body === "string"
                          ? stripForceWebSearchPrefix(body)
                          : body;
                      if (useMarkdown && typeof displayBody === "string" && displayBody.trim()) {
                        return <MarkdownBody text={displayBody} />;
                      }
                      if (typeof displayBody === "string" && displayBody.trim()) {
                        return (
                          <pre className="event-card__plain">
                            <LinkedText text={displayBody} />
                          </pre>
                        );
                      }
                      return null;
                    })()}
                  </>
                )}
                {/* Reply footer: meta tags + one-click full-text copy */}
                {event.type === "assistant_message" && (
                  <div className="event-card__footer">
                    {event.agentId ? (
                      <div className="event-card__meta">
                        {event.modeLabel && (
                          <>
                            <span
                              className="meta-tag__symbol"
                              data-mode-tone={modeTone(event.modeLabel)}
                            >
                              ▣
                            </span>
                            <span
                              className="meta-tag meta-tag--mode"
                              data-mode-tone={modeTone(event.modeLabel)}
                            >
                              {event.modeLabel}
                            </span>
                          </>
                        )}
                        {event.agentLabel && (
                          <span className="meta-tag">{event.agentLabel}</span>
                        )}
                        {event.modelLabel && (
                          <span className="meta-tag">{event.modelLabel}</span>
                        )}
                        {event.effortLabel && (
                          <span className="meta-tag">{event.effortLabel}</span>
                        )}
                        {event.durationMs != null && (
                          <span className="meta-tag meta-tag--duration">
                            {(event.durationMs / 1000).toFixed(1)}s
                          </span>
                        )}
                      </div>
                    ) : (
                      <span className="event-card__footer-spacer" />
                    )}
                    {typeof body === "string" && body.trim() && (
                      <CopyReplyButton text={body} />
                    )}
                  </div>
                )}
              </div>
            </article>
          );
        };

          return presentationItems.flatMap((item) => {
            if (item.kind === "reply_group") {
              return item.parts.map((part) => renderEvent(part.event.event, part.event.index));
            }
            return renderEvent(item.event, item.index);
          });
        })()}
        {showStallBanner && (
          <StallBanner
            health={health}
            midTurn={midTurn}
            openToolTitle={openTool?.title ?? null}
            openToolIsSubagent={isSubagentTool(openTool?.toolName)}
            lastActivityAt={lastActivityAt}
            now={healthNow}
            onInterrupt={onInterrupt}
          />
        )}
      </div>
      <MessageOutline events={visibleEvents} sessionId={session.id} />
      </div>
    </div>
    </LinkCwdContext.Provider>
  );
}

/**
 * Floating quote / comment UI. Own state only — never re-renders event cards,
 * so browser text selection highlight stays after mouseup.
 */
function QuoteOverlay({
  sessionId,
  listRef,
  pins,
  onPinsChange,
}: {
  sessionId: string;
  listRef: React.RefObject<HTMLDivElement | null>;
  pins: QuotePin[];
  onPinsChange: (pins: QuotePin[]) => void;
}) {
  const [btn, setBtn] = useState<{ quoted: string; x: number; y: number } | null>(null);
  const [draft, setDraft] = useState<{
    quoted: string;
    x: number;
    y: number;
    comment: string;
  } | null>(null);
  const draftRootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const pinsRef = useRef(pins);
  pinsRef.current = pins;
  const committingRef = useRef(false);

  // Belt-and-suspenders with key={sessionId}: clear popover if identity changes
  // without a full remount (or if parent reuses the instance).
  useEffect(() => {
    setBtn(null);
    setDraft(null);
    window.getSelection()?.removeAllRanges();
  }, [sessionId]);

  /** Clamp a center-anchored overlay so translate(-50%) stays inside the list. */
  const clampOverlay = useCallback(
    (
      centerX: number,
      anchorY: number,
      kind: "pop" | "draft",
    ): { x: number; y: number } => {
      const list = listRef.current;
      if (!list) return { x: centerX, y: anchorY };
      const hostW = list.clientWidth;
      const hostH = list.clientHeight;
      const scrollTop = list.scrollTop;
      const pad = 10;
      const halfW =
        kind === "draft"
          ? Math.min(280, Math.max(120, hostW - 24)) / 2
          : 48;
      const boxH = kind === "draft" ? 150 : 36;
      const x = Math.min(Math.max(halfW + pad, centerX), Math.max(halfW + pad, hostW - halfW - pad));
      // Prefer above selection for pop; keep draft fully visible in the viewport.
      let y = anchorY;
      if (kind === "pop") {
        if (y - boxH < scrollTop + pad) {
          y = Math.min(scrollTop + hostH - pad, anchorY + 28);
        }
      } else {
        const maxY = scrollTop + hostH - boxH - pad;
        const minY = scrollTop + pad;
        if (y > maxY) y = Math.max(minY, maxY);
        if (y < minY) y = minY;
      }
      return { x, y };
    },
    [listRef],
  );

  const readSelection = useCallback(() => {
    const list = listRef.current;
    if (!list) return null;
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || !sel.rangeCount) return null;
    const text = sel.toString().replace(/\r\n/g, "\n").trim();
    if (text.length < 1) return null;
    const range = sel.getRangeAt(0);
    if (!list.contains(range.commonAncestorContainer)) return null;
    const rect = range.getBoundingClientRect();
    // Ignore zero-size (can happen after reflow)
    if (rect.width < 1 && rect.height < 1) return null;
    const host = list.getBoundingClientRect();
    const rawX = rect.left - host.left + rect.width / 2;
    const rawY = rect.top - host.top + list.scrollTop;
    const clamped = clampOverlay(rawX, rawY, "pop");
    return {
      quoted: text,
      x: clamped.x,
      y: clamped.y,
    };
  }, [listRef, clampOverlay]);

  // Attach to the list element so we don't rely on React bubble (and avoid parent setState).
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const onUp = (e: MouseEvent) => {
      if (draftRef.current) return;
      // Ignore mouseup that originated on our own chrome
      if (e.target instanceof Node) {
        const t = e.target as HTMLElement;
        if (t.closest(".quote-pop, .quote-draft, .quote-pin")) return;
      }
      const pos = readSelection();
      setBtn(pos);
    };
    list.addEventListener("mouseup", onUp);
    return () => list.removeEventListener("mouseup", onUp);
  }, [listRef, readSelection]);

  const commit = useCallback(
    (d: { quoted: string; x: number; y: number; comment: string }) => {
      if (committingRef.current) return;
      const comment = d.comment.trim();
      if (!comment) {
        setDraft(null);
        return;
      }
      committingRef.current = true;
      const pin: QuotePin = {
        id: newQuotePinId(),
        quoted: d.quoted,
        comment,
        x: d.x,
        y: d.y + 36,
      };
      onPinsChange([...pinsRef.current, pin]);
      setDraft(null);
      setBtn(null);
      window.setTimeout(() => {
        committingRef.current = false;
      }, 250);
    },
    [onPinsChange]
  );

  // Focus input when draft opens — do NOT use onBlur-to-cancel (that ate the empty box).
  useEffect(() => {
    if (!draft) return;
    const t = window.setTimeout(() => inputRef.current?.focus(), 0);
    return () => window.clearTimeout(t);
  }, [draft]);

  // Click outside draft → pin if has text, else close (mousedown so we don't fight focus).
  useEffect(() => {
    if (!draft) return;
    const onDocDown = (e: MouseEvent) => {
      const root = draftRootRef.current;
      if (root && e.target instanceof Node && root.contains(e.target)) return;
      const d = draftRef.current;
      if (!d) return;
      if (d.comment.trim()) commit(d);
      else setDraft(null);
    };
    // Delay so the 评论 button click that opened the draft doesn't immediately close it.
    const t = window.setTimeout(() => {
      document.addEventListener("mousedown", onDocDown);
    }, 0);
    return () => {
      window.clearTimeout(t);
      document.removeEventListener("mousedown", onDocDown);
    };
  }, [draft, commit]);

  return (
    <>
      {pins.map((pin, index) => (
        <button
          key={pin.id}
          type="button"
          className="quote-pin"
          style={{ left: pin.x, top: pin.y }}
          title={`${index + 1}. ${pin.comment}`}
          aria-label={`Comment ${index + 1}`}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            const clamped = clampOverlay(pin.x, Math.max(12, pin.y - 36), "draft");
            setDraft({
              quoted: pin.quoted,
              x: clamped.x,
              y: clamped.y,
              comment: pin.comment,
            });
            onPinsChange(pins.filter((p) => p.id !== pin.id));
            setBtn(null);
          }}
        >
          {index + 1}
        </button>
      ))}

      {btn && !draft && (
        <div className="quote-pop" style={{ left: btn.x, top: btn.y }} role="toolbar">
          <button
            type="button"
            className="quote-pop__btn"
            onMouseDown={(e) => {
              // Critical: preventDefault keeps the text selection highlight.
              e.preventDefault();
              e.stopPropagation();
            }}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              const pos = readSelection() ?? btn;
              const clamped = clampOverlay(pos.x, pos.y, "draft");
              setDraft({
                quoted: pos.quoted,
                x: clamped.x,
                y: clamped.y,
                comment: "",
              });
              setBtn(null);
            }}
          >
            <MessageSquareQuote size={13} />
            评论
          </button>
        </div>
      )}

      {draft && (
        <div
          ref={draftRootRef}
          className="quote-draft"
          style={{ left: draft.x, top: draft.y }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <div className="quote-draft__quote" title={draft.quoted}>
            {draft.quoted.length > 120 ? `${draft.quoted.slice(0, 120)}…` : draft.quoted}
          </div>
          <textarea
            ref={inputRef}
            className="quote-draft__input"
            rows={3}
            placeholder="写评论…（Ctrl+Enter 落针）"
            value={draft.comment}
            onChange={(e) => setDraft((d) => (d ? { ...d, comment: e.target.value } : d))}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                setDraft(null);
              }
              if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                e.preventDefault();
                commit(draft);
              }
            }}
          />
          <div className="quote-draft__actions">
            <button
              type="button"
              className="quote-draft__cancel"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setDraft(null)}
            >
              取消
            </button>
            <button
              type="button"
              className="quote-draft__ok"
              onMouseDown={(e) => e.preventDefault()}
              disabled={!draft.comment.trim()}
              onClick={() => commit(draft)}
            >
              落针
            </button>
          </div>
        </div>
      )}
    </>
  );
}

/** Isolated clock — does NOT re-render the event list (fixes selection wipe). */
function SessionActivityBar({
  status,
  lastActivityAt,
}: {
  status: SessionStatus;
  lastActivityAt: number | null;
}) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);
  const health = activityHealth(status, lastActivityAt, now);
  const barLabel = activityBarLabel(status, health);
  if (!barLabel) return null;
  const severity =
    health === "stuck" ? "stuck" : health === "stalled" ? "stalled" : health === "quiet" ? "stale" : "";
  return (
    <div
      className={`session-activity${severity ? ` is-${severity}` : ""} is-${status}`}
      role="status"
      aria-live="polite"
    >
      <span className="session-activity__pulse" aria-hidden />
      <span className="session-activity__label">{barLabel}</span>
      {lastActivityAt != null && (
        <span className="session-activity__ago">
          last update {formatAgo(lastActivityAt, now)}
        </span>
      )}
      {status === "running" && (
        <span className="session-activity__hint">
          {health === "stuck" || health === "stalled"
            ? "Esc×2 or ■ to interrupt"
            : "Esc×2 to interrupt"}
        </span>
      )}
    </div>
  );
}

/** Mid-turn / first-chunk stall card with clear severity + interrupt CTA. */
function StallBanner({
  health,
  midTurn,
  openToolTitle,
  openToolIsSubagent = false,
  lastActivityAt,
  now,
  onInterrupt,
}: {
  health: ActivityHealth;
  midTurn: boolean;
  openToolTitle?: string | null;
  openToolIsSubagent?: boolean;
  lastActivityAt: number | null;
  now: number;
  onInterrupt?: () => void | Promise<void>;
}) {
  const ago = lastActivityAt != null ? formatAgo(lastActivityAt, now) : undefined;
  const copy =
    stallBannerCopy(health, { midTurn, openToolTitle, openToolIsSubagent, ago }) ??
    (midTurn
      ? null
      : {
          title: "Waiting for agent",
          body: "Message sent. Streaming will show thinking / tools / reply here.",
        });
  if (!copy) return null;

  const severity =
    health === "stuck" ? "stuck" : health === "stalled" ? "stalled" : health === "quiet" ? "stale" : "";
  const showInterrupt =
    Boolean(onInterrupt) && (health === "quiet" || health === "stalled" || health === "stuck");

  return (
    <article
      className={`event-card event-card--waiting${severity ? ` is-${severity}` : ""}`}
      role="status"
      aria-live="polite"
    >
      <div className="event-card__icon">
        {health === "live" || health === "idle" ? (
          <span className="activity-dots" aria-hidden>
            <i /><i /><i />
          </span>
        ) : (
          <span className={`event-card__stall-mark is-${severity || "stale"}`} aria-hidden>
            !
          </span>
        )}
      </div>
      <div className="event-card__main">
        <span className="event-card__type">{copy.title}</span>
        <p className="event-card__waiting-copy">{copy.body}</p>
        {showInterrupt && (
          <div className="event-card__stall-actions">
            <button
              type="button"
              className="pill-action event-card__interrupt"
              onClick={() => void onInterrupt?.()}
            >
              <Square size={11} fill="currentColor" />
              Interrupt turn
            </button>
            <span className="event-card__stall-hint">or press Esc twice</span>
          </div>
        )}
      </div>
    </article>
  );
}

/** Map a mode id to a tone category for colored accent. */
function modeTone(modeId: string | null | undefined): string {
  const id = (modeId ?? "").toLowerCase();
  if (!id) return "default";
  if (id.includes("plan")) return "plan";
  if (id.includes("ask") || id.includes("chat") || id.includes("talk")) return "ask";
  if (id.includes("debug") || id.includes("review")) return "debug";
  if (
    id.includes("build") ||
    id.includes("agent") ||
    id.includes("code") ||
    id.includes("edit") ||
    id.includes("default") ||
    id.includes("auto")
  ) {
    return "build";
  }
  return "default";
}
