// Executed in the website through CDP. Preserve semantic Markdown without
// importing the site's CSS, controls, or syntax-highlighting decorations.
function extractMarkdownInPage(selector) {
  const escape = text => text.replace(/\s+/g, " ")
    .replace(/([\\`*_\[\]<>~|])/g, "\\$1")
    .replace(/(^|\n)(\s*)(#{1,6}|[-+]|\d+[.)])(?=\s)/g, "$1$2\\$3");
  const urlFor = (el, attr) => {
    if (!el.getAttribute(attr)) return "";
    try {
      const url = new URL(el.getAttribute(attr), location.href);
      return /^(https?:|mailto:)$/.test(url.protocol) ? url.href.replace(/[()]/g, c => c === "(" ? "%28" : "%29") : "";
    } catch { return ""; }
  };
  const inlineCode = text => {
    const fence = "`".repeat(Math.max(0, ...(text.match(/`+/g) || []).map(s => s.length)) + 1);
    const padding = /^[` ]|[` ]$/.test(text) ? " " : "";
    return fence + padding + text + padding + fence;
  };
  return JSON.stringify([...document.querySelectorAll(selector)].map(root => {
    const literals = [];
    const literal = text => {
      const token = `\u0000${literals.length}\u0000`;
      literals.push(text);
      return token;
    };
    const children = el => [...el.childNodes].map(render).join("");
    const block = text => `\n\n${text.trim()}\n\n`;
    const render = el => {
      if (el.nodeType === 3) return escape(el.textContent || "");
      if (el.nodeType !== 1) return "";
      const tag = el.tagName;
      if (["SCRIPT", "STYLE", "BUTTON", "SVG", "INPUT", "TEXTAREA"].includes(tag) ||
          el.getAttribute("aria-hidden") === "true" ||
          el.classList.contains("md-code-block-banner")) return "";
      if (tag === "PRE" || el.classList.contains("md-code-block")) {
        const code = el.querySelector("code") || el.querySelector("pre");
        // Some site versions wrap the actual code in a div, alongside a banner.
        const source = code || el.querySelector(".md-code-block-content") || el;
        const text = (source.textContent || "").replace(/\r\n?/g, "\n");
        const banner = el.querySelector(".md-code-block-banner");
        const lang = /(?:language|lang)-([\w+-]+)/.exec(source.className || "")?.[1] ||
          banner?.firstElementChild?.textContent?.trim() || "";
        const info = /^[\w+#.-]+$/.test(lang) ? lang : "";
        const fence = "`".repeat(Math.max(2, ...(text.match(/`+/g) || []).map(s => s.length)) + 1);
        return block(literal(`${fence}${info}\n${text}${text.endsWith("\n") ? "" : "\n"}${fence}`));
      }
      if (tag === "CODE") return inlineCode(el.textContent || "");
      // KaTeX contains several copies of the formula; keep its TeX only once.
      if (el.classList.contains("katex")) {
        const tex = el.querySelector('annotation[encoding="application/x-tex"]')?.textContent;
        return tex ? inlineCode(tex) : escape(el.textContent || "");
      }
      if (tag === "BR") return "\n";
      if (tag === "HR") return block("---");
      if (tag === "IMG") {
        const url = urlFor(el, "src");
        return url ? `![${escape(el.getAttribute("alt") || "图片")}](${url})` : "";
      }
      if (tag === "A") {
        const text = children(el).trim();
        const url = urlFor(el, "href");
        return url && text ? `[${text}](${url})` : text;
      }
      if (["STRONG", "B", "EM", "I", "DEL", "S"].includes(tag)) {
        const text = children(el);
        const mark = ["STRONG", "B"].includes(tag) ? "**" : ["DEL", "S"].includes(tag) ? "~~" : "*";
        return text.trim() ? text.replace(/^(\s*)([\s\S]*?)(\s*)$/, (_, before, body, after) =>
          before + mark + body + mark + after) : text;
      }
      if (/^H[1-6]$/.test(tag)) return block("#".repeat(Number(tag[1])) + " " + children(el).trim());
      if (tag === "UL" || tag === "OL") {
        let number = Number(el.getAttribute("start")) || 1;
        const items = [...el.children].filter(n => n.tagName === "LI").map(item => {
          if (item.hasAttribute("value")) number = Number(item.getAttribute("value")) || number;
          const prefix = tag === "OL" ? `${number++}. ` : "- ";
          const text = children(item).trim().replace(/\n{3,}/g, "\n\n");
          return prefix + text.replace(/\n/g, "\n" + " ".repeat(prefix.length));
        });
        return block(items.join("\n"));
      }
      if (tag === "BLOCKQUOTE") return block(children(el).trim().replace(/^/gm, "> "));
      if (tag === "TABLE") {
        const rows = [...el.querySelectorAll("tr")].filter(row => row.closest("table") === el);
        if (!rows.length) return "";
        const width = Math.max(...rows.map(row => row.cells.length));
        const line = row => "| " + Array.from({ length: width }, (_, i) =>
          row.cells[i] ? children(row.cells[i]).trim().replace(/\s*\n+\s*/g, " ")
            .replace(/(?<!\\)\|/g, "\\|") : "").join(" | ") + " |";
        const align = Array.from({ length: width }, (_, i) => {
          const cell = rows[0].cells[i];
          const value = cell?.getAttribute("align") || cell?.style.textAlign;
          return value === "center" ? ":---:" : value === "right" ? "---:" : "---";
        });
        return block([line(rows[0]), "| " + align.join(" | ") + " |", ...rows.slice(1).map(line)].join("\n"));
      }
      const text = children(el);
      return ["P", "DIV", "SECTION", "ARTICLE", "FIGURE", "FIGCAPTION"].includes(tag) ? block(text) : text;
    };
    // Normalize structural gaps before restoring code, whose whitespace matters.
    return children(root).replace(/\n[ \t]+\n/g, "\n\n").replace(/\n{3,}/g, "\n\n").trim()
      .replace(/\u0000(\d+)\u0000/g, (_, i, offset, all) => {
        const prefix = all.slice(all.lastIndexOf("\n", offset) + 1, offset);
        const indent = prefix.replace(/[^\s>]/g, " ");
        return literals[Number(i)].replace(/\n/g, "\n" + indent);
      });
  }));
}

export function extractMarkdownSnippet(selector) {
  return `(${extractMarkdownInPage})(${JSON.stringify(selector)})`;
}
