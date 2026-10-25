import { memo, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { isTauriRuntime, openExternal } from "../lib/api";
import { copyText } from "../lib/clipboard";
import { patchWebReply, sanitizeWebReply, type WebReply } from "../lib/webReply";
import { MarkdownBody } from "./MarkdownBody";
import { ImageLightbox } from "./ImageLightbox";

const NATIVE_CSS = `
:host {
  display:block; min-width:0 !important; max-width:100% !important;
  contain:layout paint; isolation:isolate; overflow-x:auto;
  font-family:var(--marionette-web-font) !important;
  color:var(--marionette-web-text) !important;
  background:transparent !important;
}
#web-reply-root {
  min-width:0; max-width:100%; overflow-wrap:anywhere; word-break:break-word;
  container-type:inline-size;
  font-family:var(--marionette-web-font); font-size:var(--marionette-web-size);
  color:var(--marionette-web-text); line-height:1.55; background:transparent;
  --font-text-md-size:var(--marionette-web-size) !important;
  --dil-font-text-md-size:var(--marionette-web-size) !important;
}
*,*::before,*::after { box-sizing:border-box; animation:none !important; transition:none !important; }
#web-reply-root [data-d-inline-reveal] { opacity:1 !important; }
#web-reply-root [data-d-component="text"], #web-reply-root [data-d-component="title"] {
  font-family:var(--marionette-web-font) !important;
}
#web-reply-root [data-dil-message-id], #web-reply-root [data-markdown-text-style] {
  font-size:var(--marionette-web-size) !important;
}
#web-reply-root [data-d-component="text"]:not([data-d-size]) { font-size:inherit !important; }
#web-reply-root [data-d-has-border] { border-color:var(--marionette-web-border) !important; }
#web-reply-root > :first-child, #web-reply-root > :first-child > :first-child { margin-top:0 !important; }
#web-reply-root > :last-child { margin-bottom:0 !important; }
#web-reply-root img { max-width:100%; height:auto; cursor:zoom-in; }
#web-reply-root pre { max-width:100%; overflow:auto; white-space:pre; }
#web-reply-root table { max-width:100%; }
#web-reply-root [data-d-component="table"] {
  min-width:0 !important; max-width:100% !important; width:100% !important;
  margin-inline:0 !important; overflow-x:auto;
}
#web-reply-root [data-d-component="code"] { min-width:0; max-width:100%; overflow-x:auto; }
#web-reply-root a { cursor:pointer; }
#web-reply-root button { cursor:default; }
#web-reply-root [data-d-component="grid"], #web-reply-root [data-d-component="row"] { min-width:0; }
#web-reply-root [data-d-component="grid-item"] { min-width:0; }
`;

const localAsset = (path: string) => isTauriRuntime() ? convertFileSrc(path) : `file:///${path.replace(/\\/g, "/")}`;

/** Original site DOM and CSS live in a shadow tree, inside the native card.
 * No iframe, nested vertical scrollbar, or stylesheet leaks into the shell. */
export const WebReplyBody = memo(function WebReplyBody({ reply, text }: { reply: WebReply; text: string }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const latest = useRef(reply);
  latest.current = reply;
  const patchRef = useRef<(() => void) | null>(null);
  const [failed, setFailed] = useState(false);
  const [ready, setReady] = useState(false);
  const [image, setImage] = useState<{ src: string; alt: string } | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let disposed = false;
    setReady(false);
    setFailed(false);
    const shadow = host.shadowRoot || host.attachShadow({ mode: "open" });
    shadow.replaceChildren();
    const sheet = document.createElement("link");
    sheet.rel = "stylesheet";
    const native = document.createElement("style");
    native.textContent = NATIVE_CSS;
    const root = document.createElement("div");
    root.id = "web-reply-root";
    shadow.append(sheet, native, root);

    const syncTheme = () => {
      const parent = host.parentElement || host;
      const style = getComputedStyle(parent);
      const value = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback;
      host.style.setProperty("--marionette-web-text", value("--text", style.color));
      host.style.setProperty("--marionette-web-muted", value("--text-muted", style.color));
      host.style.setProperty("--marionette-web-panel", value("--panel", "transparent"));
      host.style.setProperty("--marionette-web-border", value("--border", "currentColor"));
      host.style.setProperty("--marionette-web-accent", value("--accent", style.color));
      host.style.setProperty("--marionette-web-font", style.fontFamily);
      host.style.setProperty("--marionette-web-size", style.fontSize);
      host.style.colorScheme = getComputedStyle(document.documentElement).colorScheme;
    };
    syncTheme();
    const themeObserver = new MutationObserver(syncTheme);
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["class", "style", "data-theme"] });

    const patch = () => {
      if (disposed) return;
      const selection = (shadow as ShadowRoot & { getSelection?: () => Selection | null }).getSelection?.() || document.getSelection();
      if (selection && !selection.isCollapsed &&
          (root.contains(selection.anchorNode) || root.contains(selection.focusNode))) return;
      try {
        patchWebReply(root, sanitizeWebReply(latest.current.html, localAsset));
      } catch { setFailed(true); }
    };
    patchRef.current = patch;
    patch();
    const resume = () => requestAnimationFrame(patch);
    document.addEventListener("selectionchange", resume);
    shadow.addEventListener("mouseup", resume);
    shadow.addEventListener("keyup", resume);

    const click = (event: Event) => {
      const target = event.composedPath().find(n => n instanceof Element) as Element | undefined;
      if (!target) return;
      const anchor = target.closest("a[href]");
      if (anchor) {
        event.preventDefault();
        const href = anchor.getAttribute("href") || "";
        if (/^https?:\/\//i.test(href)) void openExternal(href).catch(() => undefined);
        else if (href.startsWith("#")) {
          const destination = [...root.querySelectorAll("[id]")].find(el => el.id === href.slice(1));
          destination?.scrollIntoView({ block: "nearest" });
        }
        return;
      }
      const img = target.closest("img");
      if (img?.getAttribute("src")) {
        setImage({ src: img.getAttribute("src")!, alt: img.getAttribute("alt") || "" });
        return;
      }
      const button = target.closest("button");
      if (button && /copy|复制/i.test(`${button.getAttribute("aria-label")} ${button.textContent}`)) {
        for (let scope = button.parentElement; scope && scope !== root; scope = scope.parentElement) {
          const code = scope.querySelector("pre code, pre");
          if (code) { void copyText(code.textContent || ""); break; }
        }
      }
    };
    shadow.addEventListener("click", click);
    sheet.onload = () => { if (!disposed) setReady(true); };
    sheet.onerror = () => { if (!disposed) setFailed(true); };
    sheet.href = localAsset(reply.cssPath);

    return () => {
      disposed = true;
      patchRef.current = null;
      sheet.onload = null;
      sheet.onerror = null;
      themeObserver.disconnect();
      document.removeEventListener("selectionchange", resume);
      shadow.removeEventListener("mouseup", resume);
      shadow.removeEventListener("keyup", resume);
      shadow.removeEventListener("click", click);
    };
  }, [reply.cssPath]);

  useEffect(() => { patchRef.current?.(); }, [reply.html]);

  return (
    <div className="web-reply-body md-body">
      <div ref={hostRef} style={{ display: ready && !failed ? "block" : "none" }} />
      {(!ready || failed) && <MarkdownBody text={text} />}
      {ready && !failed && reply.textLength != null && text.length > reply.textLength &&
        <MarkdownBody text={text.slice(reply.textLength)} />}
      {image && <ImageLightbox src={image.src} alt={image.alt} onClose={() => setImage(null)} />}
    </div>
  );
});
