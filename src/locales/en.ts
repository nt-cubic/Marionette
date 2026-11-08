/**
 * English dictionary. `satisfies Record<keyof typeof zh, string>` makes a
 * missing translation a compile error, so a batch that adds zh keys without
 * en ones fails `tsc` in its own gate.
 */
import type { zh } from "./zh-CN";
import { en as enCommon } from "./parts/common.en";
import { en as enTheme } from "./parts/theme.en";
import { en as enPalette } from "./parts/palette.en";
import { en as enFeApp } from "./parts/fe-app.en";
import { en as enFeComposer } from "./parts/fe-composer.en";
import { en as enFeShelf } from "./parts/fe-shelf.en";
import { en as enFeDialogs } from "./parts/fe-dialogs.en";
import { en as enFeSession } from "./parts/fe-session.en";
import { en as enFeContext } from "./parts/fe-context.en";
import { en as enFeDockmemo } from "./parts/fe-dockmemo.en";
import { en as enFeDoctab } from "./parts/fe-doctab.en";
import { en as enFeBrowser } from "./parts/fe-browser.en";
import { en as enFeComputer } from "./parts/fe-computer.en";
import { en as enFeLibA } from "./parts/fe-libA.en";
import { en as enFeWorkspace } from "./parts/fe-workspace.en";
import { en as enFeLibB } from "./parts/fe-libB.en";
import { en as enFeLibC } from "./parts/fe-libC.en";
import { en as enFeCouncil } from "./parts/fe-council.en";
import { en as enFeExport } from "./parts/fe-export.en";
import { en as enFeUpdate } from "./parts/fe-update.en";
import { en as enFeUsagedash } from "./parts/fe-usagedash.en";
import { en as enFeFind } from "./parts/fe-find.en";
import { en as enFeAllplugins } from "./parts/fe-allplugins.en";
import { en as enFeRail } from "./parts/fe-rail.en";
import { en as enFeV2Shelf } from "./parts/fe-v2shelf.en";
import { en as enFeV2Transcript } from "./parts/fe-v2transcript.en";
import { en as enFeV2Composer } from "./parts/fe-v2composer.en";
import { en as enFeV2Dialogs } from "./parts/fe-v2dialogs.en";
import { en as enFeV2Chrome } from "./parts/fe-v2chrome.en";
import { en as enFeV2Memo } from "./parts/fe-v2memo.en";
import { en as enFeV2Council } from "./parts/fe-v2council.en";
import { en as enFeV2Dock } from "./parts/fe-v2dock.en";
import { en as enFeV2Usage } from "./parts/fe-v2usage.en";
import { en as enFeV2Pages } from "./parts/fe-v2pages.en";
import { en as enFeUv2Shell } from "./parts/fe-uv2shell.en";
import { en as enFeUv2A } from "./parts/fe-uv2a.en";
import { en as enFeUv2B } from "./parts/fe-uv2b.en";
import { en as enFeUv2C } from "./parts/fe-uv2c.en";
import { en as enFeUv2D } from "./parts/fe-uv2d.en";
import { en as enFeUv2E } from "./parts/fe-uv2e.en";

export const en = {
  ...enCommon,
  ...enTheme,
  ...enPalette,
  ...enFeApp,
  ...enFeComposer,
  ...enFeShelf,
  ...enFeDialogs,
  ...enFeSession,
  ...enFeContext,
  ...enFeDockmemo,
  ...enFeDoctab,
  ...enFeBrowser,
  ...enFeComputer,
  ...enFeLibA,
  ...enFeWorkspace,
  ...enFeLibB,
  ...enFeLibC,
  ...enFeCouncil,
  ...enFeExport,
  ...enFeUpdate,
  ...enFeUsagedash,
  ...enFeFind,
  ...enFeAllplugins,
  ...enFeRail,
  ...enFeV2Shelf,
  ...enFeV2Transcript,
  ...enFeV2Composer,
  ...enFeV2Dialogs,
  ...enFeV2Chrome,
  ...enFeV2Memo,
  ...enFeV2Council,
  ...enFeV2Dock,
  ...enFeV2Usage,
  ...enFeV2Pages,
  ...enFeUv2Shell,
  ...enFeUv2A,
  ...enFeUv2B,
  ...enFeUv2C,
  ...enFeUv2D,
  ...enFeUv2E,
} satisfies Record<keyof typeof zh, string>;
