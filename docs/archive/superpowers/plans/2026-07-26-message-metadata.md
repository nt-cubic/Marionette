# Message Metadata Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add timestamps, mode colors, and reply metadata tags to Clean View message cards.

**Architecture:** Extend `SessionEvent` types with optional metadata fields (`agentId`, `agentLabel`, `modelId`, `modelLabel`, `modeLabel`, `effortLabel`, `durationMs`). Snapshot current Composer config at send time onto `user_message`. `assistant_message` inherits from preceding `user_message`. Duration tracked via ref on turn start/complete.

**Tech Stack:** React, TypeScript, CSS (custom properties for mode colors)

---

### Task 1: Extend Type Definitions

**Files:**
- Modify: `src/lib/types.ts:111-176`

- [ ] **Add metadata fields to `user_message`**

Find the `user_message` type (around line 111-119) and add:

```typescript
| {
    type: "user_message";
    sessionId: string;
    text: string;
    messageId?: string;
    createdAt: string;
    // Metadata snapshot at send time
    agentId?: string;
    agentLabel?: string;
    modelId?: string;
    modelLabel?: string;
    modeLabel?: string;
    effortLabel?: string;
  }
```

- [ ] **Add metadata fields to `assistant_message`**

Find the `assistant_message` type (around line 120-126) and add:

```typescript
| {
    type: "assistant_message";
    sessionId: string;
    text: string;
    messageId?: string;
    createdAt: string;
    // Metadata
    agentId?: string;
    agentLabel?: string;
    modelId?: string;
    modelLabel?: string;
    modeLabel?: string;
    effortLabel?: string;
    durationMs?: number;
  }
```

---

### Task 2: Thread Metadata Through Event Creation (acpTranscript.ts)

**Files:**
- Modify: `src/lib/acpTranscript.ts:409-439`

- [ ] **Update `userMessageEvent()` to accept metadata**

```typescript
export function userMessageEvent(
  sessionId: string,
  text: string,
  meta?: {
    agentId?: string;
    agentLabel?: string;
    modelId?: string;
    modelLabel?: string;
    modeLabel?: string;
    effortLabel?: string;
  },
): SessionEvent {
  return {
    type: "user_message",
    sessionId,
    text,
    messageId: `um-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
    createdAt: new Date().toISOString(),
    ...meta,
  };
}
```

- [ ] **Update `assistantMessageEvent()` to accept metadata**

```typescript
export function assistantMessageEvent(
  sessionId: string,
  text: string,
  messageId?: string,
  meta?: {
    agentId?: string;
    agentLabel?: string;
    modelId?: string;
    modelLabel?: string;
    modeLabel?: string;
    effortLabel?: string;
  },
): SessionEvent {
  return {
    type: "assistant_message",
    sessionId,
    text,
    messageId,
    createdAt: new Date().toISOString(),
    ...meta,
  };
}
```

- [ ] **Update `applyAcpPartToEvents()` to inherit metadata from preceding `user_message`**

In `applyAcpPartToEvents()`, when creating a NEW `assistant_message` (line 541: `return [...current, assistantMessageEvent(sessionId, part.text, part.messageId)]`), look up the last user_message in `current` and pass its metadata.

Find the `current` array and scan backwards for the last `user_message` to inherit metadata:

```typescript
// Before the return on line 541, add helper logic:
function metaFromLastUser(events: SessionEvent[], sessionId: string) {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.sessionId === sessionId && e.type === "user_message") {
      return {
        agentId: e.agentId,
        agentLabel: e.agentLabel,
        modelId: e.modelId,
        modelLabel: e.modelLabel,
        modeLabel: e.modeLabel,
        effortLabel: e.effortLabel,
      };
    }
  }
  return undefined;
}
```

Then change line 541 from:
```typescript
return [...current, assistantMessageEvent(sessionId, part.text, part.messageId)];
```
to:
```typescript
const meta = metaFromLastUser(current, sessionId);
return [...current, assistantMessageEvent(sessionId, part.text, part.messageId, meta)];
```

Also apply the same metadata inheritance for the merge path (around line 530-539) — but only the first time a new assistant_message is created, not on subsequent stream merges. The merge path just updates `text`/`messageId` and should keep existing metadata. Since the operator is `{...last, text: nextText, messageId: ...}`, and metadata is already on `last` from creation, no change needed there.

---

### Task 3: Thread Metadata Through Event Creation (ptyCleanBridge.ts)

**Files:**
- Modify: `src/lib/ptyCleanBridge.ts:118-155`

- [ ] **Update `appendEvent()` to accept and inherit metadata**

```typescript
function appendEvent(
  events: SessionEvent[],
  sessionId: string,
  kind: "thought" | "tool" | "assistant_message",
  text: string,
  meta?: {
    agentId?: string;
    agentLabel?: string;
    modelId?: string;
    modelLabel?: string;
    modeLabel?: string;
    effortLabel?: string;
  },
): SessionEvent[] {
  // ... existing logic ...
  
  const createdAt = new Date().toISOString();
  if (kind === "thought") {
    return [...events, { type: "thought", sessionId, text, createdAt }];
  }
  if (kind === "tool") {
    const title = text.replace(/^[◆●]\s*/, "").split(/\s+/).slice(0, 4).join(" ");
    return [...events, { type: "tool_call", sessionId, text, title, status: "running", createdAt }];
  }
  return [...events, { type: "assistant_message", sessionId, text, createdAt, ...meta }];
}
```

- [ ] **Update `ingestPtyOutput()` to find and pass metadata**

In `ingestPtyOutput()`, before calling `appendEvent()`, scan the `events` parameter passed to `mapEvents` for the last `user_message` to get metadata. Use a closure:

```typescript
// Inside the ops.push for assistant_message (line 210):
ops.push((prev) => {
  // Find metadata from last user_message
  let meta: Record<string, string> | undefined;
  for (let i = prev.length - 1; i >= 0; i--) {
    const e = prev[i];
    if (e.sessionId === sessionId && e.type === "user_message") {
      if (e.agentId || e.agentLabel || e.modelId || e.modelLabel || e.modeLabel || e.effortLabel) {
        meta = {
          agentId: e.agentId,
          agentLabel: e.agentLabel,
          modelId: e.modelId,
          modelLabel: e.modelLabel,
          modeLabel: e.modeLabel,
          effortLabel: e.effortLabel,
        };
      }
      break;
    }
  }
  return appendEvent(prev, sessionId, "assistant_message", text, meta as any);
});
```

---

### Task 4: Add Metadata Snapshot at Send Time (App.tsx)

**Files:**
- Modify: `src/app/App.tsx`

- [ ] **Add ref to track current config**

Add a new ref near line 190:
```typescript
/** Snapshot of Composer config taken at send time, used to stamp user_message events. */
const sendMetaRef = useRef<{
  agentId: string;
  agentLabel: string;
  modelId?: string;
  modelLabel?: string;
  modeLabel?: string;
  effortLabel?: string;
} | null>(null);
```

- [ ] **Update `sendMetaRef` when Composer state changes**

Add `trackedModelId` and `trackedModeLabel` refs that get updated from the existing callbacks:

App.tsx already has `activeModelId` state (line 181). We need to also track `activeModeLabel` and `activeEffortLabel`.

The `onSessionPrefsChange` callback (line 2091) already knows about prefs. But the Composer's current *display* values may differ from committed prefs.

Simplest approach: derive the snapshot at send time from what we already know:
- `currentAgent` has `id` and `label`
- `activeModelId` is tracked
- `sessionCapabilities` has `currentMode` but we need the label
- `displaySession` has `preferredEffort`/`preferredEffortId`

Update `sendMetaRef` right before creating the user_message in `performSend()` (line 1726):

```typescript
// In performSend(), before creating user_message:
const session = sessionsRef.current.find((s) => s.id === sid);
const agent = agentsRef.current.find((a) => a.id === session?.agentId);
sendMetaRef.current = {
  agentId: agent?.id ?? "",
  agentLabel: agent?.label ?? "",
  modelId: activeModelId ?? undefined,
  modelLabel: activeModelId ?? undefined,  // or look up model label from sessionCapabilities
  modeLabel: sessionCapabilities?.currentMode ?? undefined,
  effortLabel: displaySession.preferredEffortId ?? undefined,
};
```

Then pass `sendMetaRef.current` to `userMessageEvent()`:

Change line 1726 from:
```typescript
setLiveEvents((current) => [...current, userMessageEvent(sid, composed)]);
```
to:
```typescript
setLiveEvents((current) => [...current, userMessageEvent(sid, composed, sendMetaRef.current ?? undefined)]);
```

Do the same for `handleEditResend` at line 1620:
```typescript
const nextEvents = [...kept, userMessageEvent(sid, newText.trim(), sendMetaRef.current ?? undefined)];
```

And the PTY path at line 1804:
```typescript
setLiveEvents((current) => [...current, userMessageEvent(sid, composed, sendMetaRef.current ?? undefined)]);
```

- [ ] **Track response duration**

Add a ref for turn start tracking near line 190:
```typescript
/** When the current turn's first assistant_message chunk arrived (for duration tracking). */
const turnStartedAtRef = useRef<Record<string, number>>({});
```

In the ACP event listener, after `setLiveEvents` is called with `applyAcpPartToEvents`, check if a new assistant_message was created:

Actually, the cleanest way is to track this in the event callback itself. In the ACP `session/update` handler (line 406-413):

```typescript
setLiveEvents((current) => {
  let next = applyAcpPartToEvents(current, payload.sessionId, extracted);
  // Track turn start for duration
  if (extracted.role === "assistant") {
    const lastBefore = current[current.length - 1];
    const lastAfter = next[next.length - 1];
    const isNewCard = lastAfter?.type === "assistant_message" && 
      lastBefore?.type !== "assistant_message" && 
      lastAfter.sessionId === payload.sessionId;
    if (isNewCard && !turnStartedAtRef.current[payload.sessionId]) {
      turnStartedAtRef.current[payload.sessionId] = Date.now();
    }
  }
  // ... rest of code
});
```

For turn completion, in the `rpc/response` handler (line 416-418), compute duration and update the last assistant_message:

```typescript
if (payload.kind === "response" && payload.method === "rpc/response") {
  // Compute duration and update last assistant_message
  const startedAt = turnStartedAtRef.current[payload.sessionId];
  if (startedAt) {
    const durationMs = Date.now() - startedAt;
    delete turnStartedAtRef.current[payload.sessionId];
    setLiveEvents((current) => {
      const last = current[current.length - 1];
      if (last?.type === "assistant_message" && last.sessionId === payload.sessionId && !last.durationMs) {
        const next = [...current];
        next[next.length - 1] = { ...last, durationMs };
        return next;
      }
      return collapseIntermediateAssistantAsThought(current, payload.sessionId);
    });
  } else {
    setLiveEvents((current) => collapseIntermediateAssistantAsThought(current, payload.sessionId));
  }
  // ...
}
```

Also clear turnStartedAt when a new user_message is sent (in performSend):
```typescript
delete turnStartedAtRef.current[sid];
```

- [ ] **Clean up `sendMetaRef` after turn starts**

Clear `sendMetaRef.current` when the first assistant chunk arrives, so stale metadata isn't reused for the next turn. In the ACP listener:

```typescript
if (extracted.role === "assistant") {
  sendMetaRef.current = null;  // snapshot consumed
  // ...
}
```

---

### Task 5: New MessageTimestamp Component

**Files:**
- Create: `src/components/MessageTimestamp.tsx`

- [ ] **Create the component**

```typescript
import { useMemo } from "react";

type MessageTimestampProps = {
  createdAt: string;
};

/**
 * Formats a timestamp for display in message cards.
 * 
 * Rules:
 * - Today:        "2:34 PM" (time only)
 * - Yesterday:    "Yesterday"
 * - 2-6 days ago: "Mon" (day name)
 * - 7+ days (this year): "Jul 15"
 * - Older:        "Jul 15, 2024"
 * - Hover tooltip: full absolute timestamp
 */
function formatMessageTime(iso: string): { display: string; tooltip: string } {
  const date = new Date(iso);
  const now = new Date();
  const tooltip = date.toLocaleString();

  // Same day → time only
  if (date.toDateString() === now.toDateString()) {
    return {
      display: date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", hour12: true }),
      tooltip,
    };
  }

  const diffDays = Math.floor((now.getTime() - date.getTime()) / (1000 * 60 * 60 * 24));

  // Yesterday
  if (diffDays === 1) {
    return { display: "Yesterday", tooltip };
  }

  // Within a week → day name
  if (diffDays < 7) {
    return {
      display: date.toLocaleDateString(undefined, { weekday: "short" }),
      tooltip,
    };
  }

  // Same year → month + day
  if (date.getFullYear() === now.getFullYear()) {
    return {
      display: date.toLocaleDateString(undefined, { month: "short", day: "numeric" }),
      tooltip,
    };
  }

  // Different year → full
  return {
    display: date.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" }),
    tooltip,
  };
}

export function MessageTimestamp({ createdAt }: MessageTimestampProps) {
  const { display, tooltip } = useMemo(() => formatMessageTime(createdAt), [createdAt]);

  return (
    <time className="event-card__timestamp" dateTime={createdAt} title={tooltip}>
      {display}
    </time>
  );
}
```

---

### Task 6: Update SessionView.tsx — Card Rendering

**Files:**
- Modify: `src/components/SessionView.tsx`

- [ ] **Import `MessageTimestamp`**

Add import at top:
```typescript
import { MessageTimestamp } from "./MessageTimestamp";
```

- [ ] **Add modeTone helper (local copy or import)**

Add or import the `modeTone` function from Composer.tsx. Since it's not exported, add a local copy in SessionView.tsx:

```typescript
function modeTone(modeId: string | null | undefined): string {
  const id = (modeId ?? "").toLowerCase();
  if (!id) return "default";
  if (id.includes("plan")) return "plan";
  if (id.includes("ask") || id.includes("chat") || id.includes("talk")) return "ask";
  if (id.includes("debug") || id.includes("review")) return "debug";
  if (id.includes("build") || id.includes("agent") || id.includes("code") ||
      id.includes("edit") || id.includes("default") || id.includes("auto")) return "build";
  return "default";
}
```

- [ ] **Update You card (user_message)**

Find the user_message rendering section (around line 847-957). Make these changes:

1. Add `data-mode-tone` attribute to the `<article>` element:
```tsx
<article
  className={`event-card event-card--${event.type}${isEditing ? " is-editing" : ""}`}
  key={`${event.type}-${event.createdAt}-${index}`}
  id={isUser ? userKey : undefined}
  data-mode-tone={event.type === "user_message" && "modeLabel" in event ? modeTone(event.modeLabel) : undefined}
>
```

2. Remove the icon div from both You and Reply:
```tsx
{/* Delete this block for user_message and assistant_message */}
{event.type !== "user_message" && event.type !== "assistant_message" && (
  <div className="event-card__icon">
    <FileText size={15} />
  </div>
)}
```

3. Add `MessageTimestamp` to the `event-card__type-row`:
```tsx
<div className="event-card__type-row">
  <span className="event-card__type">{label}</span>
  <MessageTimestamp createdAt={event.createdAt} />
  {isUser && onEditResend && !isEditing && (
    // ... existing edit button
  )}
</div>
```

- [ ] **Update Reply card — Add metadata tags**

After the MarkdownBody for `assistant_message`, before closing `</div>` (the event-card__main), add:

```tsx
{event.type === "assistant_message" && (
  <div className="event-card__meta">
    {(event as any).modeLabel && (
      <span
        className="meta-tag meta-tag--mode"
        data-mode-tone={modeTone((event as any).modeLabel)}
      >
        {(event as any).modeLabel}
      </span>
    )}
    {(event as any).agentLabel && (
      <span className="meta-tag">{(event as any).agentLabel}</span>
    )}
    {(event as any).modelLabel && (
      <span className="meta-tag">{(event as any).modelLabel}</span>
    )}
    {(event as any).effortLabel && (
      <span className="meta-tag">{(event as any).effortLabel}</span>
    )}
    {(event as any).durationMs != null && (
      <span className="meta-tag meta-tag--duration">
        {((event as any).durationMs / 1000).toFixed(1)}s
      </span>
    )}
  </div>
)}
```

---

### Task 7: CSS Styles

**Files:**
- Modify: `src/styles/app.css`

- [ ] **Remove icon for You + Reply cards**

Find `.event-card` (line 2028) — the grid layout stays at `30px minmax(0, 1fr)` for other card types. Hide icon for You and Reply:

```css
/* Hide icon for You + Reply cards */
.event-card--user_message .event-card__icon,
.event-card--assistant_message .event-card__icon {
  display: none;
}
```

- [ ] **You card left color bar**

```css
.event-card--user_message {
  position: relative;
  overflow: hidden;
}

.event-card--user_message::before {
  content: "";
  position: absolute;
  left: 0;
  top: 4px;
  bottom: 4px;
  width: 3px;
  border-radius: 0 2px 2px 0;
  pointer-events: none;
  z-index: 1;
}

.event-card--user_message[data-mode-tone="plan"]::before  { background: #a78bfa; }
.event-card--user_message[data-mode-tone="ask"]::before   { background: #60a5fa; }
.event-card--user_message[data-mode-tone="debug"]::before { background: var(--orange); }
.event-card--user_message[data-mode-tone="build"]::before { background: var(--accent); }
.event-card--user_message[data-mode-tone="default"]::before { background: var(--text-faint); }
```

- [ ] **Timestamp**

```css
.event-card__timestamp {
  flex: 0 0 auto;
  color: var(--text-faint);
  font-size: 11px;
  white-space: nowrap;
  cursor: default;
}

.event-card__timestamp:hover {
  color: var(--text-muted);
}
```

- [ ] **Meta tags row**

```css
.event-card__meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  margin-top: 8px;
  padding-top: 8px;
  border-top: 1px solid var(--border);
}
```

- [ ] **Meta tag base style**

```css
.meta-tag {
  display: inline-flex;
  align-items: center;
  height: 20px;
  padding: 0 7px;
  border-radius: 4px;
  background: var(--panel-soft);
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 500;
  white-space: nowrap;
  line-height: 1;
}
```

- [ ] **Mode tag — colored**

```css
.meta-tag--mode {
  background: color-mix(in srgb, var(--mode-chip, var(--text-faint)) 12%, transparent);
  color: var(--mode-chip, var(--text-faint));
}

.meta-tag--mode[data-mode-tone="plan"]  { --mode-chip: #a78bfa; }
.meta-tag--mode[data-mode-tone="ask"]   { --mode-chip: #60a5fa; }
.meta-tag--mode[data-mode-tone="debug"] { --mode-chip: var(--orange); }
.meta-tag--mode[data-mode-tone="build"] { --mode-chip: var(--accent); }
.meta-tag--mode[data-mode-tone="default"] { --mode-chip: var(--text-faint); }
```

- [ ] **Duration tag — optional accent**

```css
.meta-tag--duration {
  font-variant-numeric: tabular-nums;
}
```

---

### Task 8: Add `activeModelId` tracking for send snapshot

**Files:**
- Modify: `src/app/App.tsx`

- [ ] **Track `activeModeLabel` and `activeEffortLabel` from Composer**

Add state near line 181:
```typescript
const [activeModeLabel, setActiveModeLabel] = useState<string | null>(null);
const [activeEffortLabel, setActiveEffortLabel] = useState<string | null>(null);
```

Pass these to Composer... wait, Composer doesn't expose these directly. Instead, derive them from existing state:

In `sendMetaRef` update in `performSend()`:

```typescript
// Before creating user_message:
const caps = sessionCapabilities;
const modeId = caps?.currentMode;
const modeLabel = modeId ? (caps?.modes.find(m => m.id === modeId)?.label ?? modeId) : undefined;
const effortId = displaySession.preferredEffortId;
const effortLabel = effortId
  ? (caps?.effortOptions?.find(o => o.id === effortId)?.label ?? effortId)
  : displaySession.preferredEffort != null
    ? effortLabel(displaySession.preferredEffort)
    : undefined;

sendMetaRef.current = {
  agentId: currentAgent.id,
  agentLabel: currentAgent.label,
  modelId: activeModelId ?? undefined,
  modelLabel: activeModelId ?? undefined,
  modeLabel: modeLabel,
  effortLabel: effortLabel,
};
```

We also need the `effortLabel` helper function. It's in Composer.tsx but not exported. Add a local copy or export it.

Add a simple helper in App.tsx:
```typescript
function effortLabel(value: number): string {
  if (value <= 0.1) return "Low";
  if (value >= 0.9) return "High";
  if (Math.abs(value - 0.5) < 0.1) return "Auto";
  return value < 0.5 ? "Medium" : "High";
}
```

---

### Task 9: Verify and Fix TypeScript Issues

- [ ] **Build the project**

Run: `cd D:\Project\AgentsShell && npx tsc --noEmit`

Expected: No type errors.

If there are type errors from the `(event as any).xxx` patterns in SessionView.tsx, fix them by adding proper type narrowing.

- [ ] **Test the UI**

Run: `npm run dev` (or whatever dev command)

Expected: App loads, You/Reply cards show timestamps, mode colors, metadata tags.

---

### Summary of All Files Changed

| File | Change |
|------|--------|
| `src/lib/types.ts` | Add metadata fields to `user_message` + `assistant_message` |
| `src/lib/acpTranscript.ts` | Update `userMessageEvent()`, `assistantMessageEvent()`, add `metaFromLastUser()`, update `applyAcpPartToEvents()` |
| `src/lib/ptyCleanBridge.ts` | Update `appendEvent()` signature, update `ingestPtyOutput()` to pass metadata |
| `src/components/MessageTimestamp.tsx` | **New** — smart time display component |
| `src/components/SessionView.tsx` | Add timestamp, metadata tags, mode color bar, modeTone helper, remove icon for You/Reply |
| `src/styles/app.css` | Add styles for timestamp, meta tags, mode color bar |
| `src/app/App.tsx` | Add `sendMetaRef`, `turnStartedAtRef`, snapshot logic, duration tracking, effortLabel helper |
