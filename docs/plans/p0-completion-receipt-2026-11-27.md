# P0 completion receipt - 2026-11-27

This receipt records verified progress in the current working tree. All ten
functional P0 slices now have implementations; remaining gaps are explicitly
listed as real-device or provider-runtime checks rather than missing code.

| Slice | Status | Verified | Residual / deferred |
|---|---|---|---|
| P0-1 | Complete | Structured five-stage logging plus `scripts/report-subagent-pipeline.mts` landed. Synthetic sandbox and script self-test pass. | No single real 127+ tool worker has yet been captured with all five stages in one log window. Stage 1 can still be supplied manually with `--baseline`; stage 5 is visible only when that worker was opened during the retained log window. |
| P0-2 | Complete | Sole normalized writer is frontend `useSessionRails.persistSessionNow`; live debounce and tee replay share one serialized flush. `npm run build`, `npx tsc --noEmit`, all 12 `verify-transcript-write-owner` checks and focused Rust `chief::tee` tests pass. | Not yet exercised with a GUI-driven long worker. Still unverified on first-open live-rail/tee overlap, a crash halfway through an append, and simultaneous replay from main plus detached windows. |
| P0-8 | Complete | Sanitized fixture from a real tee: 2,144 lines, 145 tool starts, 673 tool updates, 1,300 thought chunks and 24 assistant chunks. `verify-subagent-fixture` and capture `--check` pass. | Sanitization protects JSON values; unusual secret-bearing key names are outside the guarantee. Absolute time and verbatim prose are intentionally not preserved. |
| P0-3 | Complete | Durable content-free tee health, bounded segmented replay, user-visible loss notice and retry path landed. `verify-tee-health` passes; focused Rust `chief::tee` suite passes 9/9. | Real queue-overflow UI was not forced in the desktop build. Events dropped before the tee write are unrecoverable by design. |
| P0-4 | Complete | `workerStreamProjection` uses the same `extractAcpUpdateText` / apply seam, preserves tool updates and reports deferred user echo. Fixture projection keeps 145 tool cards. | Parent wiring is through the existing replay path; a real long OpenCode worker should still be watched end to end. |
| P0-5 | Complete | `subagentObservation` derives latest tool title/status/path and step count from the worker's own `SessionEvent[]`; the parent card renders that snapshot. | Agents that omit tool IDs fall back to exact fingerprints; genuinely ambiguous id-less updates stay one extra step rather than being guessed away. |
| P0-6 | Complete | One six-state projection (`starting/running/waiting_user/completed/failed/cancelled`) is shared by delegated-task UI; proposed/interrupted normalize at the boundary. Covered by `verify-subagent-observability`. | Old persisted task rows may retain retired status strings until their next list refresh. |
| P0-7 | Complete | Completed worker cards retain last assistant text, tool count and duration while the full process remains expandable. | A worker with no assistant body shows an explicit empty-summary state. |
| P0-9 | Complete | `coldStartReplay` plans cursor + uncovered-span replay, dedupes You cards by content/time and tool creations by `toolCallId`; repeated opens consume nothing twice. Covered by fixture replay assertions. | The live-rail/tee overlap remains a race worth watching in a real app restart. |
| P0-10 | Complete | `verify-subagent-observability.mts` drives parse -> replay -> render -> parent count: 2,144 events, 145 tool starts/updates-linked cards, 145 parent steps, one You card, six-state projection and cold-start dedupe all pass. | Fixture uses a fixed 2024 anchor; timestamp-sensitive behavior was tested with an explicit canonical timestamp. |

## Adjacent P1 slices landed in the same wave

| Slice | Status | Verified | Residual / deferred |
|---|---|---|---|
| P1-1 | Code landed | Redacted diagnostic bundle, diagnostic Chat path and Ctrl K entry are implemented. | Needs an end-to-end GUI smoke check from command entry through prefilled draft. |
| P1-2 | Code landed; host wiring pending | Per-question attachment drafts, schema gating and explicit follow-up payload are implemented; elicitation tests pass 11/11. | App must send the follow-up only after the answer is accepted. |
| P1-3 | Complete | Large-paste folding is UTF-8 byte based, content-addressed under `~/.marionette/attachments/pasted-text`, preserves selection replacement and Ctrl/Cmd+Shift+V inline bypass. `verify-pasted-text-fold` passes. | Real OS clipboard staging and a failed-disk-write path still need a desktop manual check. |
| P1-4 | First version complete | Bounded session-reference builder and Composer reference chip/link path are implemented. Thoughts, credentials and raw tool bodies are excluded. | The second-version read-only MCP reader is not implemented; the current snapshot text is visible in the sent You card. |
| P1-6 | Complete | Read-only recent-project discovery is collapsed by default, requires an explicit Add click and reuses the normal project import path. | Large third-party stores should be watched for scan latency; no project defaults or Git URL cloning were added by design. |
| P1-7 | Complete | Browser find/viewport emulation and diff find/ignore-whitespace/colorblind presentation landed; `verify-browser-diff-ux` and existing browser suites pass. | Native child WebView2 keyboard focus needs the wry accelerator bridge before Ctrl+F works while the page body itself owns focus. |
| P1-8 | Code landed; App enqueue pending | Usage hierarchy, overrides, aliases, filters and reset-aware schedule request are implemented. Existing analytics/CSV remain reachable. | App must enqueue the request through the existing scheduled-send path. Provider-reported cost remains authoritative. |
| P1-9 | Pure implementation landed | Versioned prompt-link parser/serializer and F6 focus helper are implemented. | App/main launch routing and the workspace focus registration still need wiring. |
| P1-10 | Capture implementation landed | Window selection, capture dialog, accessibility summary and shortcut descriptor are implemented. | App must register the shortcut and insert the returned attachment into the active draft. |
| P1-11 | State/store landed | Working/settled/snoozed/archived state projection, persistence and `verify-session-states` (16 checks) pass. | App must feed real signals, run auto-settle/wake ticks, pass controls to ProjectShelf and filter archived sessions from palette search. |
| P1-12 | Complete | Resident core is two schemas; optional schemas are searchable with bounded results. `tools_mcp` tests pass 14/14 and preserve all permission paths. | Real-agent context-byte reduction should be sampled in a later live run. |
| P1-13 | Settings/API landed | Cheap text settings dialog/API, separate usage source, title event and branch/commit request APIs are implemented. `cheap_text` tests pass 15/15. | App must mount the settings entry; branch/commit callsites still need host screens. |
| P1-14 | Backend/dialog landed | Project action store, worktree setup hook and ProjectActionsDialog are implemented; `project_actions` tests pass 5/5. | App/ProjectShelf must mount the dialog and expose the action launcher. |

## Task-card cancellation note

P0-2 and P0-8 task cards show `cancelled` because the parent agent interrupted
their extra wrap-up reasoning. That cancellation is not an implementation or
test failure. The working-tree artifacts and the independent checks above are
the acceptance record.

## Remaining release audit

Checkpoint gate: `npx tsc --noEmit`, `npm run build`, Rust `--no-run` and
`npm run verify` (68/68) pass. Focused suites also pass: `chief::tee` 9/9,
`cheap_text` 15/15, `browser::find` 4/4, `tools_mcp` 14/14,
`project_actions` 5/5, `elicitation` 11/11, `diagnostics` 6/6 and
`verify-session-states` 16/16.

This is a stable checkpoint, not a claim that all 45 roadmap items are wired
into the GUI. Real-device checks and the parent-wiring items listed above remain.
