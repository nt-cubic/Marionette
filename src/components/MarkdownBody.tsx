import { memo, useEffect, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import { convertFileSrc } from "@tauri-apps/api/core";
import { prepareMarkdownForRender } from "../lib/markdownText";
import { linkifyChildren, useLinkMenu } from "./LinkedText";
import type { LinkTarget } from "../lib/linkTargets";

type MarkdownBodyProps = {
  text: string;
  className?: string;
};

/** The only scheme the webview can hand straight to a browser. */
const HTTP_URL_RE = /^https?:\/\//i;

/**
 * micromark percent-encodes link destinations at parse time (backslash →
 * `%5C`, space → `%20`, non-ASCII → UTF-8 escapes) before urlTransform runs.
 * Local targets are paths, not URLs, so decode them back — `decodeURI` is the
 * exact inverse (micromark even encodes literal `%` as `%25`, so round-trips
 * are lossless). http(s) links keep their encoding untouched.
 */
function decodeLocalTarget(url: string): string {
  if (HTTP_URL_RE.test(url)) return url;
  try {
    return decodeURI(url);
  } catch {
    return url;
  }
}

/**
 * Markdown link — same click behaviour as linkified prose: never navigate the
 * app window, hand the target to the OS, and on failure show the menu with the
 * reason instead of silently doing nothing.
 */
function MdLink({ href, children }: { href?: string; children?: ReactNode }) {
  const { openMenu, primaryAction, renderMenu } = useLinkMenu();
  const target: LinkTarget | null = href
    ? {
        kind: HTTP_URL_RE.test(href) ? "url" : "path",
        raw: href,
        start: 0,
        end: href.length,
      }
    : null;
  return (
    <>
      <a
        href={href}
        target="_blank"
        rel="noreferrer noopener"
        onClick={(event) => {
          if (!target) return;
          event.preventDefault();
          void primaryAction(event, target);
        }}
        onContextMenu={(event) => {
          if (!target) return;
          event.preventDefault();
          void openMenu(event, target);
        }}
      >
        {children}
      </a>
      {renderMenu()}
    </>
  );
}

/**
 * Inline image — single click toggles zoom (600px cap ↔ natural size),
 * right-click opens the menu (Open with system viewer / Show in Explorer /
 * Copy path). No delay: there is no double-click action to disambiguate.
 * stopPropagation so a click inside a markdown link (`[![alt](img)](url)`)
 * doesn't fire twice.
 *
 * Display src: http(s) loads natively; local paths can't be loaded by the
 * webview directly, so they go through the asset protocol (convertFileSrc).
 * The click/right-click target stays the raw path — Open / Copy / Show in
 * Explorer must see the real filesystem path, not the asset URL.
 */
function MdImage({ src, alt }: { src?: string; alt?: string }) {
  const { openMenu, renderMenu } = useLinkMenu();
  const [zoomed, setZoomed] = useState(false);
  const target: LinkTarget | null = src
    ? {
        kind: HTTP_URL_RE.test(src) ? "url" : "path",
        raw: src,
        start: 0,
        end: src.length,
      }
    : null;
  const displaySrc = (() => {
    if (!src || HTTP_URL_RE.test(src)) return src;
    if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
      return convertFileSrc(src.replace(/^file:\/\//i, ""));
    }
    return src;
  })();
  useEffect(() => {
    setZoomed(false);
  }, [src]);
  return (
    <>
      <img
        src={displaySrc}
        alt={alt ?? ""}
        loading="lazy"
        className={zoomed ? "md-body__img md-body__img--zoomed" : "md-body__img"}
        title={src}
        onClick={(event) => {
          if (!target) return;
          event.preventDefault();
          event.stopPropagation();
          setZoomed((z) => !z);
        }}
        onContextMenu={(event) => {
          if (!target) return;
          event.preventDefault();
          event.stopPropagation();
          void openMenu(event, target);
        }}
      />
      {renderMenu()}
    </>
  );
}

/**
 * Renders assistant/user transcript as Markdown (GFM + soft line breaks).
 * Streaming-friendly: incomplete markdown still shows until closed.
 *
 * Pre-pass (prepareMarkdownForRender) repairs model quirks: fences glued to
 * Chinese prose, section headers stuck to prior sentences, short table rows.
 *
 * Memoized on text/className: parent stream ticks must not rebuild the markdown
 * tree for unchanged cards — that remounts text nodes and wipes browser selection.
 */
export const MarkdownBody = memo(function MarkdownBody({ text, className }: MarkdownBodyProps) {
  if (!text) return null;

  return (
    <div className={className ? `md-body ${className}` : "md-body"}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkBreaks]}
        // Default urlTransform kills any href whose "protocol" isn't http(s) —
        // `file:///D:/x` and even `D:\x` drive paths render as href="". Local
        // targets are the whole point here, and clicks never navigate the
        // window (MdLink always preventDefaults and hands off to the OS), so
        // the sanitizing transform is replaced with a local-target decoder.
        urlTransform={decodeLocalTarget}
        components={{
          // Never let a link navigate the app window — hand it to the OS.
          a: MdLink,
          img: MdImage,
          // Paths / URLs written as prose become clickable too.
          p: ({ children }) => <p className="md-body__p">{linkifyChildren(children)}</p>,
          li: ({ children }) => <li className="md-body__li">{linkifyChildren(children)}</li>,
          td: ({ children }) => <td>{linkifyChildren(children)}</td>,
          code: ({ children, className: codeClassName }) => {
            // Fenced blocks are `pre > code` (often `language-*`); inline is
            // short / single-line — that's where models usually put a path.
            const textContent = Array.isArray(children)
              ? children.map(String).join("")
              : typeof children === "string"
                ? children
                : "";
            const isBlock =
              Boolean(codeClassName?.includes("language-")) || textContent.includes("\n");
            const isInline = !isBlock && textContent.length > 0 && textContent.length < 200;
            return (
              <code className={codeClassName}>
                {isInline ? linkifyChildren(textContent || children) : children}
              </code>
            );
          },
        }}
      >
        {prepareMarkdownForRender(text)}
      </ReactMarkdown>
    </div>
  );
});
