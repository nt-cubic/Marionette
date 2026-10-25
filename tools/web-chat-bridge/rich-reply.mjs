// Capture the rendered reply and its existing stylesheet rules. No component
// layout is reconstructed here. Styles are content-addressed and reused by all
// replies; only the current turn's HTML travels in ACP snapshots.
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";

export const CHATGPT_REPLY_SELECTOR = 'div[data-message-author-role="assistant"], div[data-markdown-text-style="assistant-message"]';
export const chatgptRootsJs = `([...document.querySelectorAll(${JSON.stringify(CHATGPT_REPLY_SELECTOR)})].filter(n => !n.querySelector(${JSON.stringify(CHATGPT_REPLY_SELECTOR)})))`;

async function captureInPage(indexes, knownSignature, knownImages) {
  const selector = 'div[data-message-author-role="assistant"], div[data-markdown-text-style="assistant-message"]';
  const roots = [...document.querySelectorAll(selector)].filter(n => !n.querySelector(selector));
  const selected = indexes.map(i => roots[i]).filter(Boolean);
  if (!selected.length) return null;
  const cache = window.__marionetteRichReplyCache ??= { sprites: new Map(), failedImages: new Set() };
  const sheetSignature = [...document.styleSheets].map(s => {
    try { return `${s.href}:${s.cssRules.length}`; } catch { return s.href; }
  }).join("|") + document.documentElement.className + document.documentElement.getAttribute("data-theme");
  let css;
  if (knownSignature !== sheetSignature) {
    const rules = [];
    for (const sheet of document.styleSheets) {
      try {
        const base = sheet.href || location.href;
        rules.push([...sheet.cssRules].map(r => r.cssText).join("\n").replace(
          /url\(\s*(["']?)([^"')]+)\1\s*\)/g,
          (whole, quote, value) => {
            try { return `url(${JSON.stringify(new URL(value.trim(), base).href)})`; } catch { return whole; }
          },
        ));
      } catch { /* A cross-origin sheet may not expose cssRules. */ }
    }
    const computed = getComputedStyle(selected[0]);
    const variables = [...computed].filter(n => n.startsWith("--"));
    rules.push(`:host, #web-reply-root {${variables.map(n => `${n}:${computed.getPropertyValue(n)};`).join("")}}`);
    // Translate the source's semantic theme tokens to inherited native colors.
    // Layout, typography hierarchy and colored diagram surfaces stay in the
    // original CSS; no per-component layout rules are invented.
    const native = variables.flatMap(name => {
      if (!name.startsWith("--color-")) return [];
      let value;
      if (/text-(primary|default|prose|heading)|token-text-primary/.test(name)) value = "text";
      else if (/text-(secondary|tertiary|quaternary)|token-text-(secondary|tertiary)/.test(name)) value = "muted";
      else if (/text-info|token-link/.test(name)) value = "accent";
      else if (/border-(light|medium|heavy)|border-(primary|secondary)|token-border|^--color-border$/.test(name)) value = "border";
      else if (/background-(primary|secondary|tertiary)-(surface|soft)|surface-(primary|secondary|tertiary)|token-(bg|main-surface|sidebar-surface)/.test(name)) value = "panel";
      return value ? [`${name}:var(--marionette-web-${value}) !important;`] : [];
    });
    rules.push(`#web-reply-root, #web-reply-root [data-theme] {${native.join("")}}`);
    css = rules.join("\n");
  }
  const copies = selected.map(root => root.cloneNode(true));
  const assets = [];
  for (const clone of copies) {
    clone.querySelectorAll('script, style, link, iframe, object, embed, form, input, textarea').forEach(n => n.remove());
    for (const el of [clone, ...clone.querySelectorAll("*")]) {
      for (const attr of [...el.attributes]) {
        if (/^on/i.test(attr.name) || ["srcdoc", "autofocus", "contenteditable"].includes(attr.name)) el.removeAttribute(attr.name);
      }
      for (const attr of ["href", "src", "poster"]) {
        const value = el.getAttribute(attr);
        if (!value || value.startsWith("#")) continue;
        try {
          const url = new URL(value, location.href);
          if (["http:", "https:", "data:", "blob:"].includes(url.protocol)) el.setAttribute(attr, url.href);
          else el.removeAttribute(attr);
        } catch { el.removeAttribute(attr); }
      }
    }
    // SVG <use> refers to the site's icon sprite. Copy the referenced symbol
    // itself so the exported HTML does not depend on cross-origin SVG access.
    for (const use of clone.querySelectorAll("svg use")) {
      const href = use.getAttribute("href") || use.getAttribute("xlink:href");
      if (!href || href.startsWith("#")) continue;
      try {
        const url = new URL(href, location.href);
        const id = decodeURIComponent(url.hash.slice(1));
        url.hash = "";
        if (url.origin !== location.origin) continue;
        if (!cache.sprites.has(url.href)) {
          cache.sprites.set(url.href, fetch(url.href, { signal: AbortSignal.timeout(5000) })
            .then(r => r.ok ? r.text() : "").catch(() => ""));
        }
        const source = new DOMParser().parseFromString(await cache.sprites.get(url.href), "image/svg+xml").getElementById(id);
        if (!source) continue;
        const group = document.createElementNS("http://www.w3.org/2000/svg", "g");
        for (const attr of [...source.attributes, ...use.attributes]) {
          if (!["href", "xlink:href", "id", "viewBox"].includes(attr.name)) group.setAttribute(attr.name, attr.value);
        }
        for (const child of source.childNodes) group.appendChild(child.cloneNode(true));
        use.replaceWith(group);
      } catch { /* A missing decorative icon must not stop the reply. */ }
    }
    for (const img of clone.querySelectorAll("img")) {
      const src = img.getAttribute("src");
      img.removeAttribute("srcset");
      if (!src) continue;
      if (knownImages[src]) { img.src = knownImages[src]; continue; }
      if (cache.failedImages.has(src)) { img.removeAttribute("src"); continue; }
      try {
        const response = await fetch(src, { signal: AbortSignal.timeout(5000) });
        const blob = await response.blob();
        if (!response.ok || !blob.type.startsWith("image/") || blob.size > 8 * 1024 * 1024) {
          cache.failedImages.add(src);
          img.removeAttribute("src");
          continue;
        }
        const data = await new Promise((resolve, reject) => {
          const reader = new FileReader();
          reader.onload = () => resolve(reader.result);
          reader.onerror = reject;
          reader.readAsDataURL(blob);
        });
        const marker = `marionette-image-${assets.length}`;
        assets.push({ source: src, marker, data });
        img.src = marker;
      } catch { cache.failedImages.add(src); img.removeAttribute("src"); }
    }
  }
  return { html: copies.map(n => n.outerHTML).join("\n"), css, signature: sheetSignature, assets, sourceUrl: location.href };
}

export function createChatgptReplyCapture(stateRoot) {
  const pages = new Map();
  const dir = path.join(stateRoot, "rich-replies");
  const store = (kind, text, ext) => {
    const bytes = Buffer.isBuffer(text) ? text : Buffer.from(text);
    const hash = createHash("sha256").update(bytes).digest("hex");
    const file = path.join(dir, kind, `${hash}.${ext}`);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    if (!fs.existsSync(file)) {
      const temp = `${file}.${process.pid}.tmp`;
      fs.writeFileSync(temp, bytes);
      try { fs.renameSync(temp, file); }
      catch (err) {
        // Another bridge may have stored the exact same stylesheet meanwhile.
        if (!fs.existsSync(file)) throw err;
        try { fs.unlinkSync(temp); } catch { /* best effort */ }
      }
    }
    return file;
  };
  return async (pageRef, indexes, evalJson) => {
    const state = pages.get(pageRef.targetId) || { signature: "", cssPath: "", images: {} };
    const result = await evalJson(pageRef, `(${captureInPage})(${JSON.stringify(indexes)}, ${JSON.stringify(state.signature)}, ${JSON.stringify(state.images)}).then(value => JSON.stringify(value))`);
    if (!result) return null;
    if (result.css) {
      state.cssPath = store("styles", result.css, "css");
      state.signature = result.signature;
    }
    for (const asset of result.assets) {
      const match = /^data:image\/(png|jpeg|gif|webp|svg\+xml|avif|x-icon);base64,(.+)$/s.exec(asset.data);
      if (!match) { result.html = result.html.replaceAll(asset.marker, ""); continue; }
      const ext = match[1] === "svg+xml" ? "svg" : match[1] === "x-icon" ? "ico" : match[1];
      const file = store("assets", Buffer.from(match[2], "base64"), ext);
      const url = pathToFileURL(file).href;
      state.images[asset.source] = url;
      result.html = result.html.replaceAll(asset.marker, url);
    }
    pages.set(pageRef.targetId, state);
    return { version: 1, source: "chatgpt", html: result.html, cssPath: state.cssPath, sourceUrl: result.sourceUrl };
  };
}
