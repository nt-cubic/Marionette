import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { copyText } from "../lib/clipboard";

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** Children shown per page in a long array / object before "再显示". */
const PAGE = 50;

/**
 * Split a tool body into a leading text part and a trailing JSON value. The
 * whole body may be JSON, or a header line or two may sit above it. Returns
 * null when no JSON object / array ends the body (clipped JSON stays text).
 */
export function splitJsonTail(text: string): { lead: string; value: Json } | null {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const lines = trimmed.split("\n");
  for (let i = 0; i < lines.length && i < 6; i += 1) {
    const head = lines[i]!.trimStart();
    if (!head.startsWith("{") && !head.startsWith("[")) continue;
    const tail = lines.slice(i).join("\n");
    try {
      const value = JSON.parse(tail) as Json;
      if (value === null || typeof value !== "object") return null;
      // A one-key object or one-item array reads fine as text already.
      const size = Array.isArray(value) ? value.length : Object.keys(value).length;
      if (size < 2 && !lines.slice(i).some((l) => l.length > 120)) return null;
      return { lead: lines.slice(0, i).join("\n").trim(), value };
    } catch {
      // Not JSON from this line — try the next opener.
    }
  }
  return null;
}

function preview(value: Json): string {
  if (Array.isArray(value)) return `[${value.length}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).length}}`;
  return "";
}

function CopyValue({ value }: { value: Json }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      className="json-tree__copy"
      title="复制值"
      aria-label="复制值"
      onClick={(event) => {
        event.stopPropagation();
        const text = typeof value === "string" ? value : JSON.stringify(value, null, 2);
        void copyText(text).then((ok) => {
          if (!ok) return;
          setDone(true);
          window.setTimeout(() => setDone(false), 1200);
        });
      }}
    >
      {done ? <Check size={10} /> : <Copy size={10} />}
    </button>
  );
}

function Leaf({ value }: { value: Json }) {
  if (value === null) return <span className="json-tree__v is-null">null</span>;
  if (typeof value === "boolean") return <span className="json-tree__v is-bool">{String(value)}</span>;
  if (typeof value === "number") return <span className="json-tree__v is-num">{value}</span>;
  return <span className="json-tree__v is-str">"{String(value)}"</span>;
}

function Node({ name, value, path, depth }: { name: string | null; value: Json; path: string; depth: number }) {
  const branch = value !== null && typeof value === "object";
  const [open, setOpen] = useState(depth < 1);
  const [shown, setShown] = useState(PAGE);
  const entries: [string, Json][] = branch
    ? Array.isArray(value)
      ? value.map((v, i) => [String(i), v])
      : Object.entries(value)
    : [];
  return (
    <div className="json-tree__node">
      <div
        className={`json-tree__row${branch ? " is-branch" : ""}`}
        title={path || "(root)"}
        onClick={branch ? () => setOpen((v) => !v) : undefined}
      >
        <span className={`json-tree__chev${branch && open ? " is-open" : ""}`} aria-hidden>
          {branch ? "▸" : ""}
        </span>
        {name != null && <span className="json-tree__k">{name}</span>}
        {name != null && <span className="json-tree__colon">:</span>}
        {branch ? <span className="json-tree__count">{preview(value)}</span> : <Leaf value={value} />}
        <CopyValue value={value} />
      </div>
      {branch && open && (
        <div className="json-tree__kids">
          {entries.slice(0, shown).map(([key, child]) => (
            <Node
              key={key}
              name={key}
              value={child}
              path={Array.isArray(value) ? `${path}[${key}]` : path ? `${path}.${key}` : key}
              depth={depth + 1}
            />
          ))}
          {entries.length > shown && (
            <button type="button" className="json-tree__more" onClick={() => setShown((n) => n + PAGE)}>
              再显示 {Math.min(PAGE, entries.length - shown)} 项（共 {entries.length}）
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * Tool input / output as a collapsible JSON tree: type colors, counts on
 * folded branches, copy any value, long branches paged. Branches mount only
 * when opened, so a large payload costs nothing until you look.
 */
export function JsonTree({ value }: { value: Json }) {
  return (
    <div className="json-tree">
      <Node name={null} value={value} path="" depth={0} />
    </div>
  );
}
