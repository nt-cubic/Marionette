// Element picker → composer prompt. Pure composition: the annotation drawer
// collects picks and notes, `composePickPrompt` renders them into the
// structured markdown that gets prefilled into the composer (never auto-sent
// — the user reviews it like any other draft).
//
// Page-derived text is untrusted data. The prompt states that boundary
// explicitly, strips sensitive URL parts, and keeps every page field on one
// line. Refs ride along when the world could resolve them; unresolved picks
// fall back to the selector + a fresh browser_snapshot.

import { t } from "./i18n";
import type {
  BrowserPickInfo,
  BrowserPickMode,
  BrowserPickRect,
  BrowserPickRef,
} from "./browserTabs";

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

/** One row as it lives in the drawer. State is owned by App so switching
 * between the browser page and a conversation cannot discard the batch. */
export type BrowserPickRow = BrowserPickDraft & {
  screenshot: string | null;
  screenshotPath?: string;
  screenshotPending?: boolean;
  screenshotFailed?: boolean;
};

export type BrowserPickModes = Record<string, BrowserPickMode | null>;
export type BrowserPickRowsByTab = Record<string, BrowserPickRow[]>;

/** Must match `PICK_LIMIT` in js/picker.js and `PICK_RETAIN_CAP` in Rust. */
export const BROWSER_PICK_LIMIT = 24;

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

const SENSITIVE_KEY =
  /(?:auth|credential|csrf|otp|pass(?:code|word|wd)?|secret|session|sig(?:nature)?|token|xsrf|api[-_.]?key|access[-_.]?key|(?:^|[-_.])code(?:$|[-_.]))/i;

function looksSecret(value: string): boolean {
  const text = value.trim().replace(/=+$/, "");
  if (text.length < 24) return false;
  if (/^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(text)) return true;
  if (/^[A-Fa-f0-9]{24,}$/.test(text)) return true;
  return /^[A-Za-z0-9_-]{24,}$/.test(text) && /\d/.test(text) && /[A-Za-z]/.test(text);
}

function redactParams(params: URLSearchParams): void {
  for (const key of [...params.keys()]) {
    const values = params.getAll(key);
    if (SENSITIVE_KEY.test(key) || values.some(looksSecret)) {
      params.delete(key);
      params.append(key, "[redacted]");
    }
  }
}

/** URL shown to the model: preserve routes that identify the page, scrub
 * credentials, secret-named parameters and long token-shaped path/hash parts. */
export function redactBrowserUrl(raw: string): string {
  const fallback = sanitizeLine(raw).slice(0, 1000);
  try {
    const url = new URL(raw);
    if (url.username) url.username = "[redacted]";
    if (url.password) url.password = "[redacted]";
    redactParams(url.searchParams);
    url.pathname = url.pathname
      .split("/")
      .map((segment) => {
        let decoded = segment;
        try {
          decoded = decodeURIComponent(segment);
        } catch {
          /* malformed escape stays opaque */
        }
        return looksSecret(segment) || looksSecret(decoded) ? "[redacted]" : segment;
      })
      .join("/");
    if (url.hash.length > 1) {
      const fragment = url.hash.slice(1);
      if (fragment.includes("=")) {
        const params = new URLSearchParams(fragment);
        redactParams(params);
        url.hash = params.toString();
      } else if (looksSecret(fragment)) {
        url.hash = "[redacted]";
      }
    }
    return url.toString().split("%5Bredacted%5D").join("[redacted]");
  } catch {
    return fallback;
  }
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
  const lines: string[] = [t("picker.promptHeader"), t("picker.promptUntrusted")];
  const mapping: string[] = [];
  let imageNumber = 0;
  let currentUrl = "";

  picks.forEach((pick, index) => {
    const number = index + 1;
    const hasImage = images.includes(pick.pickId);
    if (hasImage) {
      imageNumber += 1;
      mapping.push(t("picker.promptImageItem", { index: number, image: imageNumber }));
    }
    const safeUrl = redactBrowserUrl(pick.url) || "—";
    if (safeUrl !== currentUrl) {
      currentUrl = safeUrl;
      lines.push("", t("picker.promptPage", { url: safeUrl }));
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
