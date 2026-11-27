# Subagent replay fixture (P0-8)

This directory holds a **sanitized recording of a real delegated-worker event
stream**. It exists so the parse -> replay -> render chain can be exercised
against realistic interleaving instead of hand-written protocol frames.

It is not the P0-10 `verify-subagent-observability.mts` slice. This fixture and
its loader test deliberately stop at "the data survives the chain".

## Files

| File | What it is |
|---|---|
| `subagent-worker.events.jsonl` | 2,144 JSONL frames in the chief tee shape `{ sessionId, kind, method, data, seq, ts }` |
| `subagent-worker.manifest.json` | provenance, source counts, fixture counts, transforms, sanitizer limits |
| `subagentFixture.ts` | pure loader/summary/validator (no Node imports) |
| `../capture-subagent-fixture.ts` | regenerate the fixture from a real tee |
| `../verify-subagent-fixture.mts` | focused loader + replay self-test |

## Provenance

| Field | Value |
|---|---|
| Source kind | chief worker tee (`~/.marionette/chief/sessions/<id>.events.jsonl`) |
| Source size | 2,152 lines / 3,126,977 bytes |
| Source tool calls | 145 `tool_call` starts, 145 distinct ids |
| Source date | 2026-11-27 (derived from the source events; the fixture itself uses a fixed 2024-01-01 anchor) |
| Fixture | 2,144 lines, 145 `tool_call` starts, 673 `tool_call_update`, 1,300 thought chunks, 24 assistant chunks, 1 turn completion |
| Sanitizer | `scripts/capture-subagent-fixture.ts` v1 |

The fixture and manifest do not record the absolute source path or session id.
The capture script's default `--source` names the local tee only for convenient
regeneration; pass `--source` to use another worker.

## What survives

- Event kinds and their envelope shape (`session/update`, `turn/complete`).
- `tool_call` / `tool_call_update` linkage: every update id has a prior start,
  and starts keep their first-seen order.
- Statuses (`pending` / `in_progress` / `completed` / terminal), tool kinds
  (`execute` / `read` / `edit` / `search`), locations, and delta/snapshot
  semantics (chunks are still chunks; identical bodies still compare equal).
- Stream ordering across user seed, thoughts, assistant text, tools, and the
  turn boundary.
- 100+ tool starts, so a regression cannot pass by collapsing everything into
  one thought or one tool card.

## What is removed or replaced

- Prompts, assistant/thought bodies, tool input and output.
- File paths, repo names, usernames, URLs and query strings.
- Original ids (tool/message/session) are remapped, but equality relationships
  are preserved.
- Timestamps are anchored to a fixed epoch; only relative gaps remain.
- Absolute run date is reduced to `capturedAt: "2026-11-27"` in the manifest.

## The one synthetic frame

Tee files record agent `session/update` notifications, **not** the user prompt.
A single `user_message_chunk` seed is prepended with the fixed content
`[redacted user prompt]` so the replay's user-echo branch has something to
consume before agent activity. Everything else in the JSONL comes from the real
source file. This is recorded under `synthetic` in the manifest.

## Validation

```powershell
npx tsx scripts/verify-subagent-fixture.mts
npx tsx scripts/capture-subagent-fixture.ts --check
```

`verify-subagent-fixture.mts` asserts:

- counts: 145 tool starts, 145 distinct ids, 673 updates, 1,300 thoughts,
  24 assistant chunks, 1 user seed, 1 turn completion;
- ordering: seq is strictly increasing, the user seed is first, every update
  follows its start;
- replay: `extractAcpUpdateText` + `applyAcpPartToEvents` produce 145 tool
  cards in the same order (not one collapsed card), plus assistant/thought
  cards;
- redaction: no known original path/id/repo/URL string, and every string value
  is a marker or a safe protocol label.

`--check` regenerates in memory and fails if the committed JSONL or manifest
differs. It does not write.

## Regeneration

```powershell
npx tsx scripts/capture-subagent-fixture.ts --source <worker.events.jsonl>
```

The default source is the local chief tee used for this capture. Output is
byte-stable for the same source: ids are assigned in first-seen order, text is
content-addressed, seq is dense from 1, and timestamps are anchored. If the
source changes, update the manifest counts and re-run the self-test.

`capture-subagent-fixture.ts` intentionally uses `.ts` (not `.mts`) so
`scripts/verify-all.mjs`, which globs `.mts`/`.mjs`, does not regenerate
fixtures during `npm run verify`. Run it explicitly with `npx tsx`. The
`verify-subagent-fixture.mts` loader test *is* part of the aggregate.

## Consuming from TypeScript

```ts
import { readFileSync } from "node:fs";
import {
  parseSubagentFixture,
  summarizeSubagentFixture,
  validateSubagentFixture,
} from "./scripts/fixtures/subagentFixture.ts";

const text = readFileSync("scripts/fixtures/subagent-worker.events.jsonl", "utf8");
const events = parseSubagentFixture(text);
const summary = summarizeSubagentFixture(events);
const validation = validateSubagentFixture(events);
```

`subagentFixture.ts` has no Node imports, so the fixture text can be parsed by
any TypeScript consumer that already has it.

## Sanitizer limits

- Structural JSON keys are preserved; a secret embedded in an unusual key name
  would not be scrubbed.
- Whitespace-only chunks are canonicalized to one space.
- Absolute run time is not preserved.
- Only ACP `session/update` kinds and `turn/complete` are kept; rpc/config/
  usage/process noise is dropped.
- Text equality is preserved as equality classes; individual characters are
  not.
- The user seed is synthetic because tee files do not carry user prompts.
