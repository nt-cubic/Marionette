import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import { linkifyChildren } from "./LinkedText";
import { openExternal } from "../lib/api";

type MarkdownBodyProps = {
  text: string;
  className?: string;
};

/**
 * Normalize model/stream text so Markdown does not glue Chinese paragraphs.
 * - remark-breaks turns single \n into <br>
 * - ensure blank line before headings / numbered sections when models omit them
 */
function prepareMarkdown(text: string): string {
  let t = text.replace(/\r\n/g, "\n");
  // Soft-wrap section headers that models stick to previous sentence:
  // "...能力。一、技术" / "...能力。**标题" / "...。## Title"
  t = t.replace(/([。！？；])\s*(?=[一二三四五六七八九十]+[、．.])/g, "$1\n\n");
  t = t.replace(/([。！？；])\s*(?=#{1,6}\s)/g, "$1\n\n");
  t = t.replace(/([。！？；])\s*(?=\*\*[^*\n]{1,40}\*\*)/g, "$1\n\n");
  // "一句话概括把" style glue is model-side; we only fix clear section markers.
  return t;
}

/**
 * Renders assistant/user transcript as Markdown (GFM + soft line breaks).
 * Streaming-friendly: incomplete markdown still shows until closed.
 */
export function MarkdownBody({ text, className }: MarkdownBodyProps) {
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
          code: ({ children, className }) => {
            // Fenced blocks arrive as `pre > code`; inline code is short and
            // single-line, and is where models usually put a path.
            const isInline =
              typeof children === "string" && children.length < 200 && !children.includes("\n");
            return (
              <code className={className}>{isInline ? linkifyChildren(children) : children}</code>
            );
          },
        }}
      >
        {prepareMarkdown(text)}
      </ReactMarkdown>
    </div>
  );
}
