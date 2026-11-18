// Regression for the browser ref lease (docs/research/ego-lite M0) and the
// agent-facing guidance that ships with it. The lease itself is Rust and
// covered by cargo tests; what this pins is the cross-file contract the unit
// tests cannot see: the host no longer invalidates a snapshot's generation
// after every action, the invalidation hooks are wired into both navigation
// paths, the tool descriptions tell the model a generation survives
// actions, and the vendored bundle still enforces the page-side checks the
// whole design leans on (a bundle upgrade that loosens them must fail here).
// Run: npx tsx scripts/verify-browser-refs.mts
import { readFileSync } from "node:fs";

let failed = 0;
function check(name: string, ok: boolean, detail?: unknown) {
  if (ok) console.log("  ok ", name);
  else {
    failed += 1;
    console.log("  FAIL", name, detail ?? "");
  }
}

const surface = readFileSync(new URL("../src-tauri/src/browser/surface.rs", import.meta.url), "utf8");
const toolsMcp = readFileSync(new URL("../src-tauri/src/tools_mcp.rs", import.meta.url), "utf8");
const bundle = readFileSync(new URL("../src-tauri/src/browser/js/agent.bundle.js", import.meta.url), "utf8");
const refLease = readFileSync(new URL("../src-tauri/src/browser/ref_lease.rs", import.meta.url), "utf8");

/** Slice one Rust function body out of a file: from `fn name(` to the next
 *  top-level `pub fn` / `fn` at column 0. Good enough for pinned source
 *  checks — a false slice fails loudly (empty string), never silently. */
function fnBody(source: string, name: string): string {
  const start = source.indexOf(`fn ${name}(`);
  if (start < 0) return "";
  const rest = source.slice(start);
  const next = rest.slice(1).search(/\n(?:pub )?fn /);
  return next < 0 ? rest : rest.slice(0, next + 1);
}

// ── the act path keeps the epoch stable (M0 core) ────────────────────────────
const act = fnBody(surface, "act");
check("act is found", act.length > 0);
const actBump = "generation.set(surface.generation.get() + 1)";
const actBumps = act.split(actBump).length - 1;
const guardAt = act.indexOf("if bump_generation_after_act()");
check(
  "act no longer bumps the generation unconditionally",
  actBumps === 0 || (actBumps === 1 && guardAt >= 0 && act.indexOf(actBump) > guardAt),
  "the only permitted bump is the one inside the MARIONETTE_BROWSER_REF_LEASE=0 rollback guard",
);
check(
  "the rollback bump is flag-guarded",
  act.includes("if bump_generation_after_act()") && surface.includes("fn bump_generation_after_act() -> bool {"),
  "MARIONETTE_BROWSER_REF_LEASE=0 must restore the old bump exactly",
);
check(
  "act consults the lease before touching CDP",
  act.includes("ref_lease::fast_refusal") && act.indexOf("ref_lease::fast_refusal") < act.indexOf("CdpLease::take()"),
  "a known-dead generation must answer without a CDP round trip",
);
check(
  "act folds its answer back into the lease",
  act.includes("ref_lease::note_action"),
);

// ── every snapshot still bumps; the hooks are wired ──────────────────────────
for (const name of ["snapshot", "pick_resolve"]) {
  const body = fnBody(surface, name);
  check(`${name} still bumps the generation`, body.includes("generation.set(surface.generation.get() + 1)"), "each real snapshot must move the epoch");
}
check(
  "snapshot adopts the lease",
  fnBody(surface, "snapshot").includes("lease_adopt"),
);
check(
  "pick_resolve adopts with the user-annotation cause",
  fnBody(surface, "pick_resolve").includes("LeaseDeath::UserAnnotation"),
  "the annotate flow's hidden snapshot replaces the agent's refs; the lease must say so",
);
check(
  "document navigation kills the lease",
  fnBody(surface, "apply_document_navigation").includes("LeaseDeath::DocumentNavigation"),
);
check(
  "route navigation kills the lease",
  fnBody(surface, "apply_route_navigation").includes("LeaseDeath::RouteNavigation"),
);
check(
  "set_control_value goes through the same lease treatment",
  fnBody(surface, "set_control_value").includes("ref_lease::fast_refusal") && fnBody(surface, "set_control_value").includes("ref_lease::note_action"),
);
check(
  "the lease never judges a generation valid",
  !refLease.includes("fn is_valid") && !refLease.includes("fn is_current"),
  "one-sided rule: only deaths the host witnessed may short-answer; everything else is the page's verdict",
);

// ── the agent-facing guidance says refs survive actions ──────────────────────
check(
  "the generation param description promises continuity",
  toolsMcp.includes("One generation covers any number of consecutive actions"),
  "models re-snapshot defensively unless told the generation is reusable",
);
check(
  "the snapshot description points at the cheap change probe first",
  toolsMcp.includes("browser_page_state's `seq` tells you whether anything moved"),
);
check(
  "browser_help exists and is generated from the schema table",
  toolsMcp.includes("\"browser_help\"") && toolsMcp.includes("fn help_payload(topic: &str)"),
);
check(
  "help ships waits and batch topics now that the tools exist",
  (() => {
    // Only the shipping code counts — the Rust tests legitimately quote
    // those words too, but they are inside #[cfg(test)].
    const shipped = toolsMcp.slice(0, toolsMcp.indexOf("#[cfg(test)]"));
    return shipped.includes('"waits"') && shipped.includes('"batch"') && shipped.includes('HELP_WAITS') && shipped.includes('HELP_BATCH');
  })(),
);

// ── M2 typed batch: closed op set, budget, terminal snapshot ─────────────────
const batchRust = readFileSync(new URL("../src-tauri/src/browser/batch.rs", import.meta.url), "utf8");
check("batch caps the chain at 32 steps", batchRust.includes("pub const MAX_STEPS: usize = 32"));
check(
  "batch rejects a mid-chain snapshot",
  batchRust.includes("must be the final step") && batchRust.includes("replaces the ref"),
  "a snapshot replaces the world's ref table; later ref steps would be doomed",
);
check(
  "batch has no shell/evaluate escape hatch",
  (() => {
    // The tests that assert rejection legitimately quote the words.
    const shipping = batchRust.slice(0, batchRust.indexOf("#[cfg(test)]"));
    return !shipping.includes("\"evaluate\"") && !shipping.includes("\"shell\"") && !shipping.includes("\"command\"");
  })(),
);
check(
  "the batch executor stops on refusals regardless of stopOnError",
  toolsMcp.includes('stop = Some(("refused", index))'),
);
check(
  "the batch deadline freezes while a permission card is pending",
  toolsMcp.includes("saturating_duration_since") && toolsMcp.includes("deadline = Instant::now() + remaining"),
);
check(
  "waits poll on the worker thread, never on the main thread",
  toolsMcp.includes("std::thread::sleep(Duration::from_millis(POLL_EVERY_MS))") && !surface.includes("thread::sleep"),
  "a main-thread sleep is the watchdog's MAIN THREAD STALLED",
);
check(
  "waits hard-cap their deadlines",
  batchRust.includes("pub const MAX_WAIT_MS: u64 = 15_000") && batchRust.includes("pub const MAX_LOAD_WAIT_MS: u64 = 30_000"),
);

// ── M3 dialog: gate + honest resolution ─────────────────────────────────────
check(
  "renderer-bound evals fast-fail while a dialog blocks the page",
  fnBody(surface, "evaluate_in_world_raw").includes("dialog_block_note_for"),
  "an eval against a dialog-blocked renderer hangs inside the process-wide CDP lease",
);
check(
  "screenshot capture paths are gated by the dialog mirror too",
  fnBody(surface, "screenshot").includes("dialog_block_note_for") &&
    fnBody(surface, "pick_finalize").includes("dialog_block_note_for"),
  "Page.captureScreenshot stalls behind an open dialog while holding the CDP lease",
);
check(
  "the dialog mirror clears when the document navigates",
  fnBody(surface, "apply_document_navigation").includes("dialog.borrow_mut()"),
);
check(
  "Page.javascriptDialogOpening is a zero-CDP event branch",
  surface.includes('"Page.javascriptDialogOpening"') && surface.includes("dialog_from_event(&value"),
);
check(
  "browser_dialog reports honestly when the dialog is already gone",
  fnBody(surface, "resolve_dialog").includes('"present": false'),
);
check(
  "nothing answers a dialog automatically",
  !surface.includes("auto_accept") && !toolsMcp.slice(0, toolsMcp.indexOf("#[cfg(test)]")).includes('"accept": true'),
);
check(
  "success responses stamp the generation back",
  refLease.includes("generationStillValid"),
);

// ── the page-side contract the whole design leans on ─────────────────────────
// refsAreCurrent is the only judge of freshness. If a bundle upgrade drops or
// loosens any of these checks, the host's lease assumptions need a re-audit.
check(
  "bundle checks the exact token",
  bundle.includes("generation !== refsToken"),
);
check(
  "bundle checks the url",
  bundle.includes("location.href !== refsTakenAt"),
);
check(
  "bundle checks history length",
  bundle.includes("history.length !== refsHistoryLength"),
);
check(
  "bundle checks the popstate/hashchange ticks",
  bundle.includes("navTicks !== refsNavTicks"),
);
check(
  "bundle checks the navigation entry id",
  bundle.includes("navigationEntryId() !== refsEntryId"),
);
check(
  "bundle checks the element is still connected",
  bundle.includes("element?.isConnected"),
);
check(
  "only a snapshot rewrites the token (act must not)",
  !/\bact\([^)]*\)[\s\S]{0,400}refsToken\s*=/.test(bundle),
);

console.log(failed === 0 ? "\nall checks passed" : `\n${failed} check(s) FAILED`);
process.exit(failed === 0 ? 0 : 1);
