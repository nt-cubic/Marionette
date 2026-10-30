import { Check, Copy } from "lucide-react";
import { createContext, isValidElement, memo, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import { convertFileSrc } from "@tauri-apps/api/core";
import { isTauriRuntime, readImageDataUrl } from "../lib/api";
import { copyText } from "../lib/clipboard";
import { t, useT } from "../lib/i18n";
import { isImagePath } from "../lib/toolBody";
import { stripFileUri } from "../lib/imageAttachments";
import { prepareMarkdownForRender } from "../lib/markdownText";
import { useMathPack } from "../lib/mathPack";
import { normalizeMathDelimiters, scanRichFeatures } from "../lib/richMarkdown";
import { resolveToolImagePath } from "../lib/toolBody";
import { linkifyChildren, useLinkMenu } from "./LinkedText";
import { MdMermaid } from "./MdMermaid";
import { useShikiHtml } from "./MdShikiCode";
import { SpecCard } from "./SpecCard";
import type { LinkTarget } from "../lib/linkTargets";

type MarkdownBodyProps = {
  text: string;
  className?: string;
};

/** Plain text of rendered markdown children (code blocks hold strings or arrays). */
function nodeText(node: ReactNode): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(nodeText).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) return nodeText(node.props.children);
  return "";
}

/** Fenced blocks taller than this fold to CODE_FOLDED_LINES with "显示全部". */
const CODE_FOLD_AFTER = 18;
const CODE_FOLDED_LINES = 14;

/**
 * Fenced code: a quiet header (language · lines) with copy, and long blocks
 * folded to a few lines under a soft fade until "显示全部".
 *
 * Shiki: once the language is known and the text has been quiet for 200ms,
 * the plain <pre> swaps in place for highlighted HTML (dual theme — CSS
 * picks light/dark). Until then it renders exactly like the old pipeline,
 * so streaming never flashes.
 */
function MdCodeBlock({ children }: { children?: ReactNode }) {
  const text = nodeText(children).replace(/\n$/, "");
  const lines = text ? text.split("\n").length : 0;
  const child = Array.isArray(children) ? children[0] : children;
  const codeClass = isValidElement<{ className?: string }>(child) ? child.props.className ?? "" : "";
  const lang = /language-([\w+#.-]+)/.exec(codeClass)?.[1] ?? "";
  const html = useShikiHtml(lang.toLowerCase(), text);
  const foldable = lines > CODE_FOLD_AFTER;
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const t = useT();
  const foldedStyle = foldable && !open ? { maxHeight: `${CODE_FOLDED_LINES * 1.45 + 1.4}em` } : undefined;
  return (
    <div className={`md-code${foldable && !open ? " is-folded" : ""}`}>
      <div className="md-code__bar">
        <span className="md-code__lang">{[lang, lines > 1 ? t("md.lineCount", { n: lines }) : ""].filter(Boolean).join(" · ")}</span>
        <button
          type="button"
          className={`pill-action pill-action--sm md-code__copy${copied ? " is-copied" : ""}`}
          title={t("md.copyCode")}
          aria-label={copied ? t("md.copied") : t("md.copyCode")}
          onClick={() => {
            void copyText(text).then((ok) => {
              if (!ok) return;
              setCopied(true);
              window.clearTimeout(timer.current);
              timer.current = window.setTimeout(() => setCopied(false), 1400);
            });
          }}
        >
          {copied ? <Check size={11} /> : <Copy size={11} />}
        </button>
      </div>
      {html ? (
        <div
          className={`md-code__hl${foldable && !open ? " is-clipped" : ""}`}
          style={foldedStyle}
          dangerouslySetInnerHTML={{ __html: html }}
        />
      ) : (
        <pre style={foldedStyle}>{children}</pre>
      )}
      {foldable && (
        <button type="button" className="md-code__more" onClick={() => setOpen((value) => !value)}>
          {open ? t("ui.collapse") : t("md.showAllLines", { n: lines })}
        </button>
      )}
    </div>
  );
}

/** Language + text of a fenced block, as seen by the `pre` override. */
function fenceInfo(children: ReactNode): { lang: string; text: string } {
  const text = nodeText(children).replace(/\n$/, "");
  const child = Array.isArray(children) ? children[0] : children;
  const codeClass = isValidElement<{ className?: string }>(child) ? child.props.className ?? "" : "";
  const lang = /language-([\w+#.-]+)/.exec(codeClass)?.[1] ?? "";
  return { lang, text };
}

/**
 * `pre` override: fenced blocks route by info string. `mermaid` / `spec`
 * become their rich cards (engine chunks load lazily and only then); every
 * other fence keeps the code-card chrome above.
 */
function MdPre({ children }: { children?: ReactNode }) {
  const { lang, text } = fenceInfo(children);
  const route = lang.toLowerCase();
  if (route === "mermaid") return <MdMermaid code={text} />;
  if (route === "spec") return <SpecCard raw={text} />;
  return <MdCodeBlock>{children}</MdCodeBlock>;
}

/** The only scheme the webview can hand straight to a browser. */
const HTTP_URL_RE = /^https?:\/\//i;

/**
 * Schemes that may survive into an href (audit P1-C). Anything else —
 * `javascript:`, `vbscript:`, `data:text/html`, unknown schemes — collapses
 * to `#`, so the attribute can never carry executable content even before
 * MdLink's own interception. Local targets (`file:`, UNC, drive letters) are
 * the other allowed family; bare relative paths stay as-is.
 */
function safeUrl(url: string): string {
  const schemeMatch = /^\s*([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(url);
  if (!schemeMatch) return url; // relative path: no scheme
  const scheme = schemeMatch[1]!.toLowerCase();
  // Single-letter "schemes" are Windows drive paths (`D:\x`, `C:/y`).
  if (scheme.length === 1) return url;
  if (scheme === "http" || scheme === "https") return url;
  if (scheme === "file") return url;
  if (scheme === "mailto") return url;
  return "#";
}

/**
 * micromark percent-encodes link destinations at parse time (backslash →
 * `%5C`, space → `%20`, non-ASCII → UTF-8 escapes) before urlTransform runs.
 * Local targets are paths, not URLs, so decode them back — `decodeURI` is the
 * exact inverse (micromark even encodes literal `%` as `%25`, so round-trips
 * are lossless). http(s) links keep their encoding untouched.
 */
function decodeLocalTarget(url: string): string {
  if (HTTP_URL_RE.test(url)) return url;
  // Deny-by-scheme first: decoding must not give `java%73cript:` a pass.
  const safe = safeUrl(url);
  if (safe === "#") return "#";
  try {
    return decodeURI(safe);
  } catch {
    return safe;
  }
}

/**
 * Markdown link — same click behaviour as linkified prose: never navigate the
 * app window, hand the target to the OS, and on failure show the menu with the
 * reason instead of silently doing nothing.
 */
function MdLink({ href, children }: { href?: string; children?: ReactNode }) {
  const { openMenu, primaryAction, renderMenu } = useLinkMenu();
  const imagePath = href ? stripFileUri(href) : "";
  if (imagePath && isImagePath(imagePath)) {
    return <PreviewImage src={href} alt={nodeText(children)} />;
  }
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
/**
 * Bounded LRU (audit P1-A): the cache holds base64 payloads (megabytes each),
 * so an unbounded map grows monotonically over a long session. 50 entries
 * covers a heavily illustrated transcript while keeping the ceiling at a few
 * hundred MB worst case.
 */
const IMAGE_CACHE_LIMIT = 50;
const imageDataUrlCache = new Map<string, Promise<LoadedImage>>();

function loadImage(path: string): Promise<LoadedImage> {
  const cached = imageDataUrlCache.get(path);
  if (cached) {
    // Refresh recency (Map iterates in insertion order → delete+re-insert).
    imageDataUrlCache.delete(path);
    imageDataUrlCache.set(path, cached);
    return cached;
  }
  if (imageDataUrlCache.size >= IMAGE_CACHE_LIMIT) {
    const oldest = imageDataUrlCache.keys().next().value;
    if (oldest !== undefined) imageDataUrlCache.delete(oldest);
  }
  const pending = readImageDataUrl(path).then((result) => ({
    dataUrl: result.dataUrl,
    // The reader may have matched a near-miss name; show the file it used.
    path: result.path || path,
  }));
  imageDataUrlCache.set(path, pending);
  // A failed read is not cached: the file may be written a moment later.
  pending.catch(() => imageDataUrlCache.delete(path));
  return pending;
}

/** Paths the asset protocol can serve when the byte reader refuses a format. */
function isAbsolutePath(value: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(value) || value.startsWith("\\\\") || value.startsWith("/");
}

/**
 * Inline image — click expands it in place inside the reply.
 * Right-click opens the menu (Open with system viewer / Show in Explorer /
 * Copy path). stopPropagation so a click inside a markdown link
 * (`[![alt](img)](url)`) doesn't fire twice.
 *
 * Display src: http(s) loads natively; a local file cannot be loaded by the
 * webview by URL, so its bytes come through the same reader the tool cards
 * use. The click/right-click target is the resolved absolute path — Open /
 * Copy / Show in Explorer must see the real filesystem path.
 */
export function PreviewImage({ src, alt }: { src?: string; alt?: string }) {
  const cwd = useContext(MarkdownImageCwdContext);
  const { openMenu, primaryAction, renderMenu } = useLinkMenu();
  const [expanded, setExpanded] = useState(false);
  const [loaded, setLoaded] = useState<LoadedImage | null>(null);
  const [failed, setFailed] = useState(false);
  const remote = Boolean(
    src &&
      (HTTP_URL_RE.test(src) || src.startsWith("data:") || src.startsWith("blob:")),
  );
  const localPath = src && !remote ? resolveToolImagePath(stripFileUri(src), cwd) : src;
  const target: LinkTarget | null = src && !/^data:image\//i.test(src)
    ? {
        kind: remote ? "url" : "path",
        // Keep exactly what the agent sent for the visible/copyable source.
        raw: src,
        start: 0,
        end: src.length,
      }
    : null;
  const caption = imageCaption(alt, src);

  useEffect(() => {
    setExpanded(false);
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

  useEffect(() => {
    if (!expanded) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setExpanded(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [expanded]);

  const displaySrc = remote ? localPath : loaded?.dataUrl;
  // A near-miss match means the file read is not the path in the reply.
  const shownPath = loaded?.path ?? localPath;
  const t = useT();

  return (
    <>
      <figure className={expanded ? "md-image-card is-expanded" : "md-image-card"}>
        <div className="md-image-card__frame">
          {displaySrc && !failed ? (
            <img
              src={displaySrc}
              alt={caption}
              loading="lazy"
              className={expanded ? "md-body__img is-expanded" : "md-body__img"}
              title={shownPath}
              role="button"
              tabIndex={0}
              aria-expanded={expanded}
              aria-label={expanded ? t("md.collapseImageAria", { caption }) : t("md.enlargeImageAria", { caption })}
              onClick={(event) => {
                event.preventDefault();
                event.stopPropagation();
                setExpanded((current) => !current);
              }}
              onKeyDown={(event) => {
                if (event.key !== "Enter" && event.key !== " ") return;
                event.preventDefault();
                setExpanded((current) => !current);
              }}
              onContextMenu={(event) => {
                if (!target) return;
                event.preventDefault();
                event.stopPropagation();
                void openMenu(event, target);
              }}
              onError={() => setFailed(true)}
            />
          ) : (
            <div className="md-image-card__unavailable">
              <span>{failed ? t("md.imageReadFailed") : t("md.imageReading")}</span>
              {target && (
                <button
                  type="button"
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    void primaryAction(event, target);
                  }}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    void openMenu(event, target);
                  }}
                >
                  {t("md.openOriginalLink")}
                </button>
              )}
            </div>
          )}
        </div>
        {caption && (
          <figcaption className="md-image-card__caption">
            {target ? (
              <button
                type="button"
                title={src}
                onClick={(event) => void primaryAction(event, target)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  void openMenu(event, target);
                }}
              >
                {caption}
              </button>
            ) : caption}
          </figcaption>
        )}
      </figure>
      {renderMenu()}
    </>
  );
}

function imageCaption(alt: string | undefined, src: string | undefined): string {
  const label = (alt ?? "").trim();
  if (label && !/^image$/i.test(label)) return label;
  const source = src ?? "";
  if (/^data:image\//i.test(source)) {
    const subtype = source.match(/^data:image\/([\w.+-]+)/i)?.[1]?.replace(/^jpeg$/i, "jpg");
    return subtype ? `image.${subtype}` : t("md.imageFallbackAlt");
  }
  const path = stripFileUri(source).split(/[?#]/, 1)[0] ?? "";
  const name = path.split(/[\\/]/).filter(Boolean).pop() ?? "";
  if (name && !/^data:/i.test(name)) return name;
  return label || t("md.imageFallbackAlt");
}

/**
 * Renders assistant/user transcript as GitHub Flavored Markdown.
 * Stack is the usual one: react-markdown + remark-gfm. remark-breaks turns
 * single newlines into <br>, which chat UIs expect (GFM itself does not).
 * Streaming-friendly: incomplete markdown still shows until closed.
 *
 * Pre-pass (prepareMarkdownForRender) only repairs invalid model output so GFM
 * can parse it (fences glued to Chinese, short table delimiter rows). It does
 * not invent a dialect.
 *
 * Rich pipeline (research-codeg.md B1/B3/B4): a cheap scan (richMarkdown.ts)
 * decides per message whether the heavy engines are worth loading. Math adds
 * remark-math (singleDollarTextMath off — `$9.99` stays literal) plus
 * rehype-katex and rewrites LaTeX `\[ … \]` to `$$ … $$`; fenced info strings
 * route `mermaid`/`spec` to their cards, and every other fenced language gets
 * shiki highlighting once the text has been quiet. Everything degrades to the
 * plain pipeline, so a failed lazy chunk load never breaks the transcript.
 *
 * Memoized on text/className: parent stream ticks must not rebuild the markdown
 * tree for unchanged cards — that remounts text nodes and wipes browser selection.
 */
export const MarkdownBody = memo(function MarkdownBody({ text, className }: MarkdownBodyProps) {
  // Hooks first: text can flip empty ⇄ non-empty between renders, so no early
  // return may come before them.
  const features = useMemo(() => scanRichFeatures(text), [text]);
  const math = useMathPack(features.math);
  const remarkPlugins = useMemo(() => {
    if (!math) return [remarkGfm, remarkBreaks];
    const mathPlugin = [
      math.remarkMath,
      { singleDollarTextMath: false },
    ] as [typeof math.remarkMath, { singleDollarTextMath: boolean }];
    return [remarkGfm, remarkBreaks, mathPlugin];
  }, [math]);
  const rehypePlugins = useMemo(() => (math ? [math.rehypeKatex] : []), [math]);
  const source = useMemo(
    () => (features.math ? normalizeMathDelimiters(prepareMarkdownForRender(text)) : prepareMarkdownForRender(text)),
    [text, features.math],
  );
  if (!text) return null;

  return (
    <div className={className ? `md-body ${className}` : "md-body"}>
      <ReactMarkdown
          remarkPlugins={remarkPlugins}
          rehypePlugins={rehypePlugins}
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
          // GFM `start` must survive so lists that begin at 10 / 100 keep their
          // markers. Default padding used to clip two-digit numbers.
          ol: ({ node: _olNode, start, children, className, ...props }) => (
            <ol
              {...props}
              className={className ? `md-body__ol ${className}` : "md-body__ol"}
              start={typeof start === "number" ? start : undefined}
            >
              {children}
            </ol>
          ),
          // Paths / URLs written as prose become clickable too.
          p: ({ children }) => {
            const plain = typeof children === "string" ? children.trim() : "";
            // A standalone image path is an image block. Paths embedded in prose
            // remain ordinary links, and inline/fenced code is never linkified.
            if (plain && isImagePath(plain)) return <PreviewImage src={plain} />;
            const childList = Array.isArray(children) ? children : [children];
            if (
              childList.some(
                (child) =>
                  isValidElement<{ href?: string }>(child) &&
                  (child.type === PreviewImage ||
                    (child.type === MdLink &&
                      typeof child.props.href === "string" &&
                      isImagePath(stripFileUri(child.props.href)))),
              )
            ) {
              return <div className="md-body__p">{linkifyChildren(children)}</div>;
            }
            return <p className="md-body__p">{linkifyChildren(children)}</p>;
          },
          li: ({ children }) => <li className="md-body__li">{linkifyChildren(children)}</li>,
          td: ({ children }) => <td>{linkifyChildren(children)}</td>,
          pre: ({ children }) => <MdPre>{children}</MdPre>,
          code: ({ children, className: codeClassName }) => {
            // Keep code literal: paths in inline/fenced code are examples, not
            // image references or links to activate.
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
                {isInline ? textContent || children : children}
              </code>
            );
          },
          }}
        >
          {source}
      </ReactMarkdown>
    </div>
  );
});
