import { useEffect, useId, useRef, useState } from "react";

type MermaidApi = typeof import("mermaid").default;

let mermaidLoader: Promise<MermaidApi> | null = null;
let mermaidTheme: "dark" | "default" =
  document.documentElement.dataset.theme === "white" ? "default" : "dark";

function desiredTheme(): "dark" | "default" {
  return document.documentElement.dataset.theme === "white" ? "default" : "dark";
}

function loadMermaid(): Promise<MermaidApi> {
  mermaidLoader ??= import("mermaid").then((module) => {
    const mermaid = module.default;
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: mermaidTheme,
    });
    return mermaid;
  });
  return mermaidLoader;
}

/**
 * ```mermaid fences render as diagrams (the heaviest lazy chunk of the
 * pipeline — only loaded when a fence actually claims to be one).
 *
 * Streaming rule: parse first, render only what parses. A half-streamed
 * diagram fails `mermaid.parse` cheaply, so the card waits quietly with the
 * source visible instead of flashing syntax errors at the user; once the
 * fence is complete (and 350ms quiet) the SVG takes its place. A previously
 * rendered diagram stays up while the agent rewrites the source, so an edit
 * in flight never blanks the card.
 *
 * securityLevel "strict" sanitizes labels and disables click callbacks —
 * the SVG is injected via dangerouslySetInnerHTML, so strict is load-bearing.
 * The dim ⇄ white theme flip re-renders through a MutationObserver on
 * documentElement's data-theme attribute.
 */
export function MdMermaid({ code }: { code: string }) {
  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const rawId = useId();
  const domId = `mmd${rawId.replace(/[^a-zA-Z0-9]/g, "")}`;
  const seq = useRef(0);
  const [themeTick, setThemeTick] = useState(0);

  useEffect(() => {
    const observer = new MutationObserver(() => setThemeTick((tick) => tick + 1));
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void loadMermaid()
        .then(async (mermaid) => {
          const want = desiredTheme();
          if (want !== mermaidTheme) {
            mermaidTheme = want;
            mermaid.initialize({
              startOnLoad: false,
              securityLevel: "strict",
              theme: want,
            });
          }
          const parsed = await mermaid.parse(code, { suppressErrors: true });
          if (!parsed) {
            // Streaming or malformed — quiet wait, keep the previous SVG.
            if (!cancelled) setError(null);
            return;
          }
          const renderId = `${domId}-${(seq.current += 1)}`;
          try {
            const { svg: rendered } = await mermaid.render(renderId, code);
            if (!cancelled) {
              setError(null);
              setSvg(rendered);
            }
          } catch (err) {
            // mermaid leaves a temp <div id="d<id>"> behind on failure.
            document.getElementById(`d${renderId}`)?.remove();
            if (!cancelled) {
              setSvg(null);
              setError(err instanceof Error ? err.message : String(err));
            }
          }
        })
        .catch(() => {
          if (!cancelled) setError("mermaid 引擎加载失败");
        });
    }, 350);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [code, domId, themeTick]);

  if (svg) {
    return <div className="md-mermaid" dangerouslySetInnerHTML={{ __html: svg }} />;
  }
  return (
    <div className={`md-mermaid${error ? " md-mermaid--error" : " md-mermaid--pending"}`}>
      <div className="md-mermaid__note">
        {error ? `mermaid 渲染失败：${error}` : "mermaid 图表 · 等待语法完整"}
      </div>
      <pre className="md-mermaid__src">{code}</pre>
    </div>
  );
}
