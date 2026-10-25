/** A ChatGPT DOM snapshot; plain text remains on the assistant event. */
export type WebReply = {
  version: 1;
  source: "chatgpt";
  html: string;
  /** Content-addressed local CSS shared by replies, not repeated in JSONL. */
  cssPath: string;
  sourceUrl: string;
  /** Distinguishes later bridge notices from the HTML snapshot's own text. */
  textLength?: number;
};

export function parseWebReply(value: unknown): WebReply | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const r = value as Record<string, unknown>;
  if (r.version !== 1 || r.source !== "chatgpt" || typeof r.html !== "string" ||
      !r.html || r.html.length > 2_000_000 || typeof r.cssPath !== "string" ||
      r.cssPath.length > 2048 ||
      !/^(?:[a-z]:[\\/]|\/)/i.test(r.cssPath) ||
      !/[\\/]rich-replies[\\/]styles[\\/][a-f0-9]{64}\.css$/i.test(r.cssPath) ||
      typeof r.sourceUrl !== "string" || !/^https:\/\/chatgpt\.com(?:\/|$)/.test(r.sourceUrl)) return undefined;
  return { version: 1, source: "chatgpt", html: r.html, cssPath: r.cssPath, sourceUrl: r.sourceUrl,
    ...(typeof r.textLength === "number" && Number.isInteger(r.textLength) && r.textLength >= 0
      ? { textLength: r.textLength } : {}),
  };
}

const TAGS = new Set(("div span p h1 h2 h3 h4 h5 h6 ul ol li dl dt dd a strong b em i u s del " +
  "small sub sup mark code pre blockquote br hr table thead tbody tfoot tr th td caption colgroup col " +
  "figure figcaption img picture source details summary section article header footer time abbr kbd samp " +
  "button svg g path rect circle ellipse line polyline polygon text tspan defs symbol use title desc " +
  "clipPath mask linearGradient radialGradient stop pattern").toLowerCase().split(" "));
const DROP = new Set(["script", "style", "link", "meta", "base", "iframe", "object", "embed", "form", "input", "textarea", "select", "foreignobject", "animate", "animatetransform", "set"]);
const ATTRS = new Set(("class id style title role dir lang hidden open colspan rowspan start value type " +
  "width height alt viewbox preserveaspectratio fill fill-rule fill-opacity stroke stroke-width stroke-linecap " +
  "stroke-linejoin stroke-dasharray stroke-dashoffset stroke-opacity opacity d x y x1 x2 y1 y2 cx cy r rx ry " +
  "points transform clip-path clip-rule mask offset stop-color stop-opacity gradientunits gradienttransform " +
  "patternunits patterntransform text-anchor dominant-baseline xmlns").split(" "));

/** Second boundary: revived history is just as untrusted as the live DOM. */
export function sanitizeWebReply(
  html: string,
  assetUrl: (path: string) => string,
): DocumentFragment {
  const parsed = new DOMParser().parseFromString(html, "text/html");
  for (const el of [...parsed.body.querySelectorAll("*")]) {
    const tag = el.localName.toLowerCase();
    if (DROP.has(tag)) { el.remove(); continue; }
    if (!TAGS.has(tag)) { el.replaceWith(...el.childNodes); continue; }
    for (const attr of [...el.attributes]) {
      const name = attr.name.toLowerCase();
      if (name === "href" || name === "xlink:href") {
        const href = attr.value.trim();
        if (href.startsWith("#") || (tag === "a" && /^https?:\/\//i.test(href))) {
          el.setAttribute("href", href);
          if (name !== "href") el.removeAttribute(attr.name);
        } else el.removeAttribute(attr.name);
      } else if (name === "src" && tag === "img") {
        const src = attr.value;
        if (/^file:\/\/\//i.test(src)) {
          const raw = decodeFilePath(src);
          if (/[\\/]rich-replies[\\/]assets[\\/][a-f0-9]{64}\.(png|jpeg|gif|webp|svg|avif|ico)$/i.test(raw)) {
            el.setAttribute("src", assetUrl(raw));
          } else el.removeAttribute(attr.name);
        } else if (!/^data:image\/(png|jpeg|gif|webp|avif);base64,/i.test(src)) {
          // Remote images must be exported to the bridge asset cache first.
          el.removeAttribute(attr.name);
        }
      } else if (!ATTRS.has(name) && !name.startsWith("data-") && !name.startsWith("aria-")) {
        el.removeAttribute(attr.name);
      }
    }
    if (tag === "button") { el.setAttribute("type", "button"); el.setAttribute("tabindex", "-1"); }
  }
  const result = document.createDocumentFragment();
  result.append(...parsed.body.childNodes);
  return result;
}

function decodeFilePath(src: string): string {
  try {
    const url = new URL(src);
    if (url.hostname && url.hostname !== "localhost") return "";
    return decodeURIComponent(url.pathname).replace(/^\/([a-z]:)/i, "$1");
  } catch { return ""; }
}

/** Patch live nodes instead of replacing the whole response on every tick. */
export function patchWebReply(parent: Node, incoming: Node): void {
  const children = [...incoming.childNodes];
  children.forEach((next, index) => {
    const current = parent.childNodes[index];
    if (!current) { parent.appendChild(next.cloneNode(true)); return; }
    if (current.nodeType !== next.nodeType || current.nodeName !== next.nodeName) {
      parent.replaceChild(next.cloneNode(true), current);
      return;
    }
    if (current.nodeType === Node.TEXT_NODE) {
      if (current.nodeValue !== next.nodeValue) current.nodeValue = next.nodeValue;
    } else if (current instanceof Element && next instanceof Element) {
      for (const attr of [...current.attributes]) {
        if (!next.hasAttribute(attr.name)) current.removeAttribute(attr.name);
      }
      for (const attr of [...next.attributes]) {
        if (current.getAttribute(attr.name) !== attr.value) current.setAttribute(attr.name, attr.value);
      }
      patchWebReply(current, next);
    }
  });
  while (parent.childNodes.length > children.length) parent.removeChild(parent.lastChild!);
}
