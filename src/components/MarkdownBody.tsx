import { createContext, memo, useContext, useEffect, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import { convertFileSrc } from "@tauri-apps/api/core";
import { isTauriRuntime, readImageDataUrl } from "../lib/api";
import { stripFileUri } from "../lib/imageAttachments";
import { prepareMarkdownForRender } from "../lib/markdownText";
import { resolveToolImagePath } from "../lib/toolBody";
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
 * Working directory for markdown images.
 *
 * A reply names its screenshots the way the agent saw them — relative to the
 * session cwd (`Docs/关卡/…/shot.png`). The webview cannot resolve those and
 * the asset protocol needs an absolute path, so the loader joins them here.
 */
export const MarkdownImageCwdContext = createContext<string | null>(null);

/** Resolved path → its bytes, so scrolling and re-renders never re-read. */
type LoadedImage = { dataUrl: string; path: string };
const imageDataUrlCache = new Map<string, Promise<LoadedImage>>();

function loadImage(path: string): Promise<LoadedImage> {
  const cached = imageDataUrlCache.get(path);
  if (cached) return cached;
  const pending = readImageDataUrl(path).then((result) => ({
    dataUrl: result.dataUrl,
    // The reader may have matched a near-miss name; show the file it used.
    path: result.path || path,
  }));
  // A failed read is not cached: the file may be written a moment later.
  pending.catch(() => imageDataUrlCache.delete(path));
  return pending;
}

/** Paths the asset protocol can serve when the byte reader refuses a format. */
function isAbsolutePath(value: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(value) || value.startsWith("\\\\") || value.startsWith("/");
}

/**
 * Inline image — single click toggles zoom (600px cap ↔ natural size),
 * right-click opens the menu (Open with system viewer / Show in Explorer /
 * Copy path). No delay: there is no double-click action to disambiguate.
 * stopPropagation so a click inside a markdown link (`[![alt](img)](url)`)
 * doesn't fire twice.
 *
 * Display src: http(s) loads natively; a local file cannot be loaded by the
 * webview by URL, so its bytes come through the same reader the tool cards
 * use. The click/right-click target is the resolved absolute path — Open /
 * Copy / Show in Explorer must see the real filesystem path.
 */
export function PreviewImage({ src, alt }: { src?: string; alt?: string }) {
  const cwd = useContext(MarkdownImageCwdContext);
  const { openMenu, primaryAction, renderMenu } = useLinkMenu();
  const [zoomed, setZoomed] = useState(false);
  const [loaded, setLoaded] = useState<LoadedImage | null>(null);
  const [failed, setFailed] = useState(false);
  const remote = Boolean(
    src &&
      (HTTP_URL_RE.test(src) || src.startsWith("data:") || src.startsWith("blob:")),
  );
  const localPath = src && !remote ? resolveToolImagePath(stripFileUri(src), cwd) : src;
  const target: LinkTarget | null = localPath
    ? {
        kind: remote ? "url" : "path",
        raw: localPath,
        start: 0,
        end: localPath.length,
      }
    : null;

  useEffect(() => {
    setZoomed(false);
    if (!src || remote) {
      setLoaded(null);
      setFailed(false);
      return;
    }
    if (!isTauriRuntime()) {
      // Plain-browser preview has no filesystem: keep the path visible instead
      // of an empty <img>.
      setFailed(true);
      return;
    }
    let cancelled = false;
    setFailed(false);
    setLoaded(null);
    void loadImage(localPath ?? "")
      .then((image) => {
        if (!cancelled) setLoaded(image);
      })
      .catch(() => {
        if (cancelled) return;
        // The reader refuses some formats / very large files; the asset
        // protocol can still serve an absolute one.
        if (localPath && isAbsolutePath(localPath)) {
          setLoaded({ dataUrl: convertFileSrc(localPath), path: localPath });
          return;
        }
        setFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [localPath, remote, src]);

  const displaySrc = remote ? localPath : loaded?.dataUrl;
  // A near-miss match means the file read is not the path in the reply.
  const shownPath = loaded?.path ?? localPath;

  return (
    <>
      {displaySrc ? (
        <img
          src={displaySrc}
          alt={alt ?? ""}
          loading="lazy"
          className={zoomed ? "md-body__img md-body__img--zoomed" : "md-body__img"}
          title={shownPath}
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
      ) : (
        <button
          type="button"
          className="md-body__img-fallback"
          title={shownPath}
          onClick={(event) => {
            if (!target) return;
            event.preventDefault();
            event.stopPropagation();
            void primaryAction(event, target);
          }}
          onContextMenu={(event) => {
            if (!target) return;
            event.preventDefault();
            event.stopPropagation();
            void openMenu(event, target);
          }}
        >
          <span className="md-body__img-fallback__alt">{alt || "图片"}</span>
          <span className="md-body__img-fallback__hint">
            {failed ? "读不到这张图，点开用系统查看器" : "正在读取…"}
          </span>
        </button>
      )}
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
          img: PreviewImage,
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
