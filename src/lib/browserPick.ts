// Element picker → composer prompt. Pure composition: the annotation drawer
// collects picks and notes, `composePickPrompt` renders them into the
// structured markdown that gets prefilled into the composer (never auto-sent
// — the user reviews it like any other draft).
//
// Page-derived text is data, not instructions: it is truncated at the picker
// (js/picker.js) and quoted here, so a page cannot phrase the prompt for the
// agent. Refs ride along when the world could resolve them; unresolved picks
// fall back to the selector + a fresh browser_snapshot.

import { t } from "./i18n";
import type { BrowserPickInfo, BrowserPickRect, BrowserPickRef } from "./browserTabs";

/** One annotation row the drawer collected (screenshots travel separately). */
export type BrowserPickDraft = {
  pickId: string;
  kind: "element" | "region";
  url: string;
  info?: BrowserPickInfo;
  rect?: BrowserPickRect;
  elements?: BrowserPickInfo[];
  scanCapped?: boolean;
  note: string;
};

/** pickId → resolved aria ref (null when the pick has none usable now). */
export type BrowserPickRefs = Record<string, BrowserPickRef>;

/** `button#login-btn` / `input.name-field` — the identity part of a row. */
export function pickTargetLabel(info: BrowserPickInfo): string {
  const tag = sanitizeLine(info.tag) || "element";
  const id = sanitizeLine(info.id);
  if (id) return `${tag}#${id}`;
  const classes = (info.classes ?? []).map(sanitizeLine).filter(Boolean);
  if (classes.length) return `${tag}.${classes.join(".")}`;
  return tag;
}

/** Region list entries: the concrete elements the user boxed, quoted text. */
export function elementShort(info: BrowserPickInfo): string {
  const text = sanitizeLine(info.text || info.placeholder || "").slice(0, 40);
  return text ? `${pickTargetLabel(info)}「${text}」` : pickTargetLabel(info);
}

/**
 * Page text is data on a single prompt line: the picker already collapses
 * whitespace (js/picker.js `collapse`), and this repeats the contract at the
 * composition boundary so a hostile page cannot break the numbering with a
 * newline of its own.
 */
function sanitizeLine(text: string): string {
  return String(text ?? "").replace(/\s+/g, " ").trim();
}

/**
 * The full annotation prompt. `generation` is the world generation the refs
 * were validated against (verbatim from the snapshot answer); `images` lists
 * the pickIds that got a screenshot attached, in attachment order — image N
 * is the Nth of them.
 */
export function composePickPrompt(
  picks: readonly BrowserPickDraft[],
  refs: BrowserPickRefs,
  generation: string,
  images: readonly string[],
): string {
  const url = picks[0]?.url ?? "";
  const lines: string[] = [t("picker.promptHeader", { url })];
  const mapping: string[] = [];
  let imageNumber = 0;

  picks.forEach((pick, index) => {
    const number = index + 1;
    const hasImage = images.includes(pick.pickId);
    if (hasImage) {
      imageNumber += 1;
      mapping.push(t("picker.promptImageItem", { index: number, image: imageNumber }));
    }
    lines.push("");
    if (pick.kind === "region" && pick.rect) {
      lines.push(
        `${number}. ${t("picker.promptRegion", {
          x: Math.round(pick.rect.x),
          y: Math.round(pick.rect.y),
          width: Math.round(pick.rect.width),
          height: Math.round(pick.rect.height),
        })}`,
      );
      const elements = (pick.elements ?? []).map(elementShort).join(" / ");
      const capped = pick.scanCapped
        ? ` ${t("picker.promptRegionCapped")}`
        : "";
      lines.push(t("picker.promptRegionElements", { elements: elements || "—" }) + capped);
    } else if (pick.info) {
      const text = sanitizeLine(pick.info.text || pick.info.placeholder || "");
      lines.push(`${number}. ${pickTargetLabel(pick.info)}${text ? `「${text}」` : ""}`);
      lines.push(t("picker.promptSelector", { selector: sanitizeLine(pick.info.selector) || "—" }));
      const resolved = refs[pick.pickId];
      lines.push(
        resolved
          ? t("picker.promptRef", { ref: sanitizeLine(resolved.ref), generation: sanitizeLine(generation) })
          : t("picker.promptNoRef"),
      );
    }
    const note = sanitizeLine(pick.note);
    if (note) {
      lines.push(t("picker.promptNote", { note }));
    }
  });

  if (imageNumber > 0) {
    lines.push("");
    lines.push(t("picker.promptImageNote", { mapping: mapping.join("、") }));
  }
  return lines.join("\n");
}
