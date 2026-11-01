// Full dialog-flow walkthrough for E2E (MARIONETTE_AUTO_CU_CHECK=1): drives
// the EXACT library calls ComputerShareDialog makes, in order, logging every
// step so the harness can assert on dev.log. Product UI never fires it.

import {
  computerInstallDriver,
  computerListShareable,
  computerRevokeAll,
  computerShare,
} from "./computer";

export type CuLog = (step: string, detail: unknown) => void;

export async function runCuWalkthrough(log: CuLog): Promise<void> {
  log("start", {});

  // 1) First open of the dialog: fresh listing (driver NOT installed in a
  //    clean profile) — must degrade gracefully, driver.installed=false.
  let state = await computerListShareable();
  log("list#1", {
    installed: state?.driver?.installed ?? null,
    windows: state?.windows?.length ?? null,
    helperError: state?.helperError ?? null,
  });

  // 2) Install click → full download (or already-installed early return).
  const installResult = await computerInstallDriver();
  log("install", installResult ?? "invoke returned null");

  // 3) Post-install listing — this is the step that used to flip back to the
  //    install button; must now report installed=true with a window list.
  state = await computerListShareable();
  log("list#2", {
    installed: state?.driver?.installed ?? null,
    windows: state?.windows?.length ?? null,
    helperError: state?.helperError ?? null,
  });

  // 4) Share the first grantable window read-only.
  const first = state?.windows?.find((w) => w.grantable);
  if (first && first.processStartedAtMs != null) {
    try {
      await computerShare(
        { kind: "window", pid: first.pid, startedAtMs: first.processStartedAtMs, windowId: first.windowId },
        "read",
      );
      log("share", { ok: true, title: first.title });
    } catch (error) {
      log("share-failed", String(error));
    }
    // 5) Listing must reflect the grant.
    state = await computerListShareable();
    const after = state?.windows?.find((w) => w.windowId === first.windowId);
    log("list#3", { shared: after?.shared ?? null, helperError: state?.helperError ?? null });
  } else {
    log("share-skipped", "no grantable window with startedAtMs");
  }

  // 6) Revoke everything; listing back to clean.
  await computerRevokeAll();
  state = await computerListShareable();
  const stillShared = state?.windows?.some((w) => w.shared && w.shared !== "none");
  log("revoke", { stillShared: stillShared ?? null });

  // Geometry: where does the dialog ACTUALLY sit? (E2E visual check —
  // centered vs off-screen, buttons inside the viewport.)
  await new Promise((r) => setTimeout(r, 600));
  const rect = document.querySelector(".computer-dialog")?.getBoundingClientRect();
  const backdrop = document.querySelector(".project-dialog-backdrop")?.getBoundingClientRect();
  log("geometry", {
    dialog: rect ? { x: rect.x, y: rect.y, w: rect.width, h: rect.height } : null,
    backdrop: backdrop ? { x: backdrop.x, y: backdrop.y, w: backdrop.width, h: backdrop.height } : null,
    viewport: { w: window.innerWidth, h: window.innerHeight },
    rows: document.querySelectorAll(".computer-window").length,
    footVisible: Boolean(document.querySelector(".computer-dialog__foot")),
  });

  log("done", {});
}
