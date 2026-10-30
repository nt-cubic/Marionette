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
import { en as enFeLibA } from "./parts/fe-libA.en";
import { en as enFeWorkspace } from "./parts/fe-workspace.en";
import { en as enFeLibB } from "./parts/fe-libB.en";
import { en as enFeLibC } from "./parts/fe-libC.en";

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
  ...enFeLibA,
  ...enFeWorkspace,
  ...enFeLibB,
  ...enFeLibC,
} satisfies Record<keyof typeof zh, string>;
