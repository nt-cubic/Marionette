/**
 * CJK inventory for the i18n conversion (2026-10-30).
 *
 * Lists every source file that still shows Chinese to a user (CJK outside
 * comments), with counts + sample lines — the worklist while batches run and
 * the acceptance gate afterwards:
 *
 *   npx tsx scripts/inventory-zh.mts            # per-file report (default: src)
 *   npx tsx scripts/inventory-zh.mts --rust     # src-tauri/src instead
 *   npx tsx scripts/inventory-zh.mts --check    # exit 1 if any non-allowlisted
 *                                               # frontend file still has CJK
 *
 * Comments are the only place Chinese may live in the end state, so the
 * scanner strips comments (TS/TSX and Rust dialects) and keeps everything
 * else — string literals AND JSX text both count.
 */

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const CJK = /[\u3000-\u303f\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\uff00-\uffef]/;

/** Files/dirs where Chinese may remain (comments aside): dictionaries,
 * i18n.ts (LOCALE_LABELS is each language's native name), and
 * browser-preview mock data only. */
const FRONTEND_ALLOWLIST = [
  /^src[\\/]locales[\\/]/,
  /^src[\\/]lib[\\/]i18n\.ts$/,
  /^src[\\/]lib[\\/]mockData\.ts$/,
];

type Mode = "ts" | "rust";

/**
 * Remove comments while keeping strings and JSX text intact.
 *
 * Template literals need a frame stack (text/code alternating): the CLOSING
 * backtick of `` `theme.${theme}` `` must return to code, not open a new
 * template — a flat "backtick starts template text" scanner swallows the rest
 * of the file as text and stops stripping comments there.
 */
export function stripComments(source: string, mode: Mode): string {
  let out = "";
  let i = 0;
  const n = source.length;
  type Frame = { kind: "text" } | { kind: "code"; braces: number };
  const stack: Frame[] = [{ kind: "code", braces: 0 }];
  const top = () => stack[stack.length - 1]!;
  while (i < n) {
    const c = source[i]!;
    const c2 = source[i + 1];
    if (mode === "ts" && c === "`") {
      if (top().kind === "code") stack.push({ kind: "text" });
      else stack.pop();
      out += c;
      i += 1;
      continue;
    }
    if (top().kind === "text") {
      // Only `${` re-enters code; everything else is literal text.
      if (c === "$" && c2 === "{") {
        stack.push({ kind: "code", braces: 0 });
        out += "${";
        i += 2;
      } else {
        out += c;
        i += 1;
      }
      continue;
    }
    if (c === "/" && c2 === "/") {
      while (i < n && source[i] !== "\n") i += 1;
      continue;
    }
    if (c === "/" && c2 === "*") {
      i += 2;
      // Keep line structure (one \n per consumed newline) so reported line
      // numbers match the source file.
      while (i < n && !(source[i] === "*" && source[i + 1] === "/")) {
        if (source[i] === "\n") out += "\n";
        i += 1;
      }
      i += 2;
      continue;
    }
    if (c === '"' || (mode === "ts" && c === "'")) {
      const quote = c;
      out += c;
      i += 1;
      while (i < n) {
        if (source[i] === "\\") {
          out += source[i]! + (source[i + 1] ?? "");
          i += 2;
          continue;
        }
        out += source[i]!;
        if (source[i] === quote) {
          i += 1;
          break;
        }
        i += 1;
      }
      continue;
    }
    const frame = top();
    if (frame.kind !== "code") {
      out += c;
      i += 1;
      continue;
    }
    if (c === "{") {
      frame.braces += 1;
    } else if (c === "}" && frame.braces > 0) {
      frame.braces -= 1;
    } else if (c === "}" && stack.length > 1) {
      // Closes a `${` interpolation — back to that template's text. The base
      // frame never pops: a stray `}` (regex literals like /\{(\w+)\}/ have
      // no matching `{`) must not crash the scanner, just pass through.
      stack.pop();
    }
    out += c;
    i += 1;
  }
  return out;
}

function walk(root: string, mode: Mode): string[] {
  const files: string[] = [];
  const visit = (dir: string) => {
    for (const entry of readdirSync(dir)) {
      const full = join(dir, entry);
      if (entry === "target" || entry === "node_modules" || entry.startsWith(".")) continue;
      const st = statSync(full);
      if (st.isDirectory()) visit(full);
      else if (mode === "ts" ? /\.(tsx?|mjs)$/.test(entry) : entry.endsWith(".rs")) files.push(full);
    }
  };
  visit(root);
  return files;
}

function main() {
  // exported for debugging (npx tsx -e "import {stripComments} from ...")
  const args = process.argv.slice(2);
  const mode: Mode = args.includes("--rust") ? "rust" : "ts";
  const check = args.includes("--check");
  const root = mode === "ts" ? "src" : join("src-tauri", "src");
  const files = walk(root, mode).sort();

  const rows: { file: string; lines: number[] }[] = [];
  for (const file of files) {
    const rel = relative(".", file).replaceAll("\\", "/");
    if (mode === "ts" && FRONTEND_ALLOWLIST.some((re) => re.test(rel))) continue;
    const stripped = stripComments(readFileSync(file, "utf8"), mode);
    const lines = stripped
      .split(/\r?\n/)
      .map((text, index) => (CJK.test(text) ? index + 1 : 0))
      .filter((n) => n > 0);
    if (lines.length > 0) rows.push({ file: rel, lines });
  }

  let total = 0;
  const fullLines = args.includes("--lines");
  for (const { file, lines } of rows) {
    total += lines.length;
    const sample = fullLines ? lines.join(",") : lines.slice(0, 3).join(",") + (lines.length > 3 ? "…" : "");
    console.log(`${String(lines.length).padStart(4)}  ${file}  [${sample}]`);
  }
  console.log(`\n${rows.length} files, ${total} CJK lines (${mode === "ts" ? "src" : "src-tauri/src"})`);

  if (check && mode === "ts" && rows.length > 0) {
    console.error("\nCHECK FAILED: CJK still present outside comments in the files above.");
    process.exit(1);
  }
}

main();
