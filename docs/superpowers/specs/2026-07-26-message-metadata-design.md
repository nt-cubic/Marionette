# Message Metadata Display Design

## Overview

Add message metadata to the Clean View conversation transcript: timestamps on every You/Reply card, and a compact info bar at the bottom of Reply cards showing mode, agent, model, effort, and duration.

## Changes

### 1. Data Model (`src/lib/types.ts`)

Extend `user_message` and `assistant_message` with optional metadata fields:

```typescript
// user_message additions
{
  agentId?: string;     // "opencode"
  agentLabel?: string;  // "OpenCode"
  modelId?: string;     // "opencode/deepseek-v4-free"
  modelLabel?: string;  // "deepseek-v4-free"
  modeLabel?: string;   // "Plan" / "Build" / "Ask" etc.
  effortLabel?: string; // "High" / "Auto" / "Low"
}

// assistant_message additions (same as above + duration)
{
  agentId?: string;
  agentLabel?: string;
  modelId?: string;
  modelLabel?: string;
  modeLabel?: string;
  effortLabel?: string;
  durationMs?: number;  // 12345 → display "12.3s"
}
```

### 2. Event Creation (`src/lib/acpTranscript.ts`)

- `userMessageEvent()` — accept optional metadata params, store on event
- `assistantMessageEvent()` — accept optional metadata params, store on event

### 3. Event Creation (`src/lib/ptyCleanBridge.ts`)

- `appendEvent()` — accept/pass through optional metadata for assistant_message

### 4. New Component: `MessageTimestamp.tsx`

Pure display component — receives `createdAt: string`, renders `<time>` with smart relative→absolute formatting.

Format rules:
| Timeframe | Display | Example |
|-----------|---------|---------|
| Today | Time | `2:34 PM` |
| Yesterday | Label | `Yesterday` |
| 2–6 days ago | Day name | `Mon` |
| 7+ days (this year) | Month + day | `Jul 15` |
| Previous years | Full date | `Jul 15, 2024` |
| Hover (title) | Full timestamp | `2025/7/26 14:34:23` |

### 5. Component Changes (`src/components/SessionView.tsx`)

**Metadata snapshot flow:**
- When user sends → capture `{agentId, agentLabel, modelId, modelLabel, modeLabel, effortLabel}` from Composer current state
- Store snapshot on `user_message` event
- Also store as `pendingTurnMeta` ref
- When first `assistant_message` chunk arrives → read `pendingTurnMeta`, stamp onto event, record `responseStartedAt`
- When turn completes (next `user_message` or `rpc/response`) → compute `durationMs`, update last `assistant_message`

**Card rendering changes:**
- You card: remove `.event-card__icon`, add `::before` left color bar with `data-mode-tone`
- Reply card: remove `.event-card__icon`, add `.event-card__meta` row with tags

### 6. CSS (`src/styles/app.css`)

- `.event-card--user_message` / `--assistant_message`: remove icon column from grid
- `.event-card--user_message::before`: 3px left color bar (mode color)
- `[data-mode-tone]` variants for the color bar
- `.event-card__timestamp`: top-right time display
- `.event-card__meta`: flex row for bottom tags
- `.meta-tag`: individual tag style
- `.meta-tag--mode`: mode tag with tone color

### Mode Color Palette

| Tone | Color | CSS Variable |
|------|-------|-------------|
| `plan` | `#a78bfa` (purple) | --mode-chip |
| `ask` | `#60a5fa` (blue) | --mode-chip |
| `debug` | `var(--orange)` | --mode-chip |
| `build` | `var(--accent)` | --mode-chip |
| `default` | `var(--text-faint)` | --mode-chip |

### Layout Comparison

**Before:**
```
[icon 30px] [type row: "Reply"     ]
            [markdown body          ]
```

**After — You:**
```
[| type row: "You"        timestamp]
[| markdown body                    ]
```

**After — Reply:**
```
[type row: "Reply"        timestamp]
[markdown body                      ]
[─── meta ──────────────────────────]
[Plan] [OpenCode] [deepseek] [High] [12.3s]
```

### Files Modified

| File | Change |
|------|--------|
| `src/lib/types.ts` | Add metadata fields to event types |
| `src/lib/acpTranscript.ts` | Thread metadata through event creation |
| `src/lib/ptyCleanBridge.ts` | Thread metadata through event creation |
| `src/components/MessageTimestamp.tsx` | New file |
| `src/components/SessionView.tsx` | Add timestamp, metadata tags, color bar, snapshot flow |
| `src/styles/app.css` | Add timestamp, meta, color bar styles |

### Scope

- Only Clean View (not Raw Terminal)
- Timestamp: only `user_message` + `assistant_message` cards
- Metadata tags: only `assistant_message` cards
- Duration: calculated from response start → completion
