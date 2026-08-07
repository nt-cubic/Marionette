import { memo } from "react";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import { prepareMarkdownForRender } from "../lib/markdownText";
import { linkifyChildren } from "./LinkedText";
import { openExternal } from "../lib/api";

type MarkdownBodyProps = {
  text: string;
  className?: string;
};

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
        components={{
          // Never let a link navigate the app window — hand it to the OS.
          a: ({ href, children }) => (
            <a
              href={href}
              target="_blank"
              rel="noreferrer noopener"
              onClick={(event) => {
                if (!href) return;
                event.preventDefault();
                void openExternal(href);
              }}
            >
              {children}
            </a>
          ),
          img: ({ src, alt }) => (
            <img src={src} alt={alt ?? ""} loading="lazy" className="md-body__img" />
          ),
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
