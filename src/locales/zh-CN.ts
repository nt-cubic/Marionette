/**
 * zh-CN dictionary: the source of truth. Merges the parts/* fragment pairs —
 * one fragment per domain/batch, so parallel conversion batches never edit
 * the same file. New keys go into a batch's own fragment; strings reused
 * across surfaces belong in parts/common.* (kept deliberately small).
 */
import { zh as zhCommon } from "./parts/common.zh";
import { zh as zhTheme } from "./parts/theme.zh";
import { zh as zhPalette } from "./parts/palette.zh";
import { zh as zhFeApp } from "./parts/fe-app.zh";
import { zh as zhFeComposer } from "./parts/fe-composer.zh";
import { zh as zhFeShelf } from "./parts/fe-shelf.zh";
import { zh as zhFeDialogs } from "./parts/fe-dialogs.zh";
import { zh as zhFeSession } from "./parts/fe-session.zh";
import { zh as zhFeContext } from "./parts/fe-context.zh";
import { zh as zhFeDockmemo } from "./parts/fe-dockmemo.zh";
import { zh as zhFeDoctab } from "./parts/fe-doctab.zh";
import { zh as zhFeBrowser } from "./parts/fe-browser.zh";
import { zh as zhFeComputer } from "./parts/fe-computer.zh";
import { zh as zhFeLibA } from "./parts/fe-libA.zh";
import { zh as zhFeWorkspace } from "./parts/fe-workspace.zh";
import { zh as zhFeLibB } from "./parts/fe-libB.zh";
import { zh as zhFeLibC } from "./parts/fe-libC.zh";
import { zh as zhFeCouncil } from "./parts/fe-council.zh";
import { zh as zhFeExport } from "./parts/fe-export.zh";
import { zh as zhFeUpdate } from "./parts/fe-update.zh";
import { zh as zhFeUsagedash } from "./parts/fe-usagedash.zh";
import { zh as zhFeFind } from "./parts/fe-find.zh";
import { zh as zhFeAllplugins } from "./parts/fe-allplugins.zh";
import { zh as zhFeRail } from "./parts/fe-rail.zh";
import { zh as zhFeV2Shelf } from "./parts/fe-v2shelf.zh";
import { zh as zhFeV2Transcript } from "./parts/fe-v2transcript.zh";
import { zh as zhFeV2Composer } from "./parts/fe-v2composer.zh";
import { zh as zhFeV2Dialogs } from "./parts/fe-v2dialogs.zh";
import { zh as zhFeV2Chrome } from "./parts/fe-v2chrome.zh";
import { zh as zhFeV2Memo } from "./parts/fe-v2memo.zh";
import { zh as zhFeV2Council } from "./parts/fe-v2council.zh";
import { zh as zhFeV2Dock } from "./parts/fe-v2dock.zh";
import { zh as zhFeV2Usage } from "./parts/fe-v2usage.zh";
import { zh as zhFeV2Pages } from "./parts/fe-v2pages.zh";
import { zh as zhFeUv2Shell } from "./parts/fe-uv2shell.zh";
import { zh as zhFeUv2A } from "./parts/fe-uv2a.zh";
import { zh as zhFeUv2B } from "./parts/fe-uv2b.zh";
import { zh as zhFeUv2C } from "./parts/fe-uv2c.zh";
import { zh as zhFeUv2D } from "./parts/fe-uv2d.zh";
import { zh as zhFeUv2E } from "./parts/fe-uv2e.zh";
import { zh as zhFeSeenshell } from "./parts/fe-seenshell.zh";
import { zh as zhFeSeenrail } from "./parts/fe-seenrail.zh";
import { zh as zhFeSeenview } from "./parts/fe-seenview.zh";
import { zh as zhFeUv2F } from "./parts/fe-uv2f.zh";

export const zh = {
  ...zhCommon,
  ...zhTheme,
  ...zhPalette,
  ...zhFeApp,
  ...zhFeComposer,
  ...zhFeShelf,
  ...zhFeDialogs,
  ...zhFeSession,
  ...zhFeContext,
  ...zhFeDockmemo,
  ...zhFeDoctab,
  ...zhFeBrowser,
  ...zhFeComputer,
  ...zhFeLibA,
  ...zhFeWorkspace,
  ...zhFeLibB,
  ...zhFeLibC,
  ...zhFeCouncil,
  ...zhFeExport,
  ...zhFeUpdate,
  ...zhFeUsagedash,
  ...zhFeFind,
  ...zhFeAllplugins,
  ...zhFeRail,
  ...zhFeV2Shelf,
  ...zhFeV2Transcript,
  ...zhFeV2Composer,
  ...zhFeV2Dialogs,
  ...zhFeV2Chrome,
  ...zhFeV2Memo,
  ...zhFeV2Council,
  ...zhFeV2Dock,
  ...zhFeV2Usage,
  ...zhFeV2Pages,
  ...zhFeUv2Shell,
  ...zhFeUv2A,
  ...zhFeUv2B,
  ...zhFeUv2C,
  ...zhFeUv2D,
  ...zhFeUv2E,
  ...zhFeSeenshell,
  ...zhFeSeenrail,
  ...zhFeSeenview,
  ...zhFeUv2F,
} as const;
