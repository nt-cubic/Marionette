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
import { zh as zhFeLibA } from "./parts/fe-libA.zh";
import { zh as zhFeWorkspace } from "./parts/fe-workspace.zh";
import { zh as zhFeLibB } from "./parts/fe-libB.zh";
import { zh as zhFeLibC } from "./parts/fe-libC.zh";
import { zh as zhFeCouncil } from "./parts/fe-council.zh";

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
  ...zhFeLibA,
  ...zhFeWorkspace,
  ...zhFeLibB,
  ...zhFeLibC,
  ...zhFeCouncil,
} as const;
