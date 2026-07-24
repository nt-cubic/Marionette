import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";

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
          a: ({ href, children }) => (
            <a href={href} target="_blank" rel="noreferrer noopener">
              {children}
            </a>
          ),
          img: ({ src, alt }) => (
            <img src={src} alt={alt ?? ""} loading="lazy" className="md-body__img" />
          ),
          p: ({ children }) => <p className="md-body__p">{children}</p>,
          li: ({ children }) => <li className="md-body__li">{children}</li>,
        }}
      >
        {prepareMarkdown(text)}
      </ReactMarkdown>
    </div>
  );
}
