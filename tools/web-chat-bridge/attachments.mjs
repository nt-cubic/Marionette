// Local attachments for the web bridge. Paths come only from explicit file picks
// in prompt metadata; never scan user text/history for files to upload.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { randomUUID } from "node:crypto";

const IMAGE_EXT = {
  "image/png": ".png", "image/jpeg": ".jpg", "image/webp": ".webp",
  "image/gif": ".gif", "image/bmp": ".bmp", "image/svg+xml": ".svg",
};

export function prepareAttachments(blocks, filePaths = []) {
  const files = [];
  let tempDir;
  const cleanup = () => {
    if (tempDir) {
      // Windows may briefly retain a browser read handle. Cleanup must not mask
      // the turn outcome; only remove our own freshly-created temporary tree.
      try { fs.rmSync(tempDir, { recursive: true, force: true }); } catch { /* best effort */ }
    }
  };
  try {
    if (!Array.isArray(filePaths)) throw new Error("附件路径格式不正确。");
    for (const file of new Set(filePaths)) {
      if (typeof file !== "string" || !path.isAbsolute(file)) throw new Error("附件必须是绝对文件路径。");
      const stat = fs.statSync(file);
      if (!stat.isFile()) throw new Error(`不能上传文件夹：${path.basename(file)}。请先选择其中的文件。`);
      if (!stat.size) throw new Error(`附件是空文件：${path.basename(file)}`);
      files.push({ path: file, name: path.basename(file) });
    }
    for (const block of blocks || []) {
      if (block?.type !== "image") continue;
      const ext = IMAGE_EXT[block.mimeType];
      if (!ext) throw new Error(`暂不支持该图片格式：${block.mimeType}`);
      if (typeof block.data !== "string" || !block.data.length || block.data.length > 17 * 1024 * 1024) {
        throw new Error("图片内容为空或超过 12 MB。请缩小图片后重试。");
      }
      const bytes = Buffer.from(block.data, "base64");
      if (!bytes.length || bytes.length > 12 * 1024 * 1024) throw new Error("图片为空或超过 12 MB。");
      tempDir ??= fs.mkdtempSync(path.join(os.tmpdir(), "marionette-web-upload-"));
      // Keep a useful display name, but use the ACP bytes (including annotated images).
      let name = `image-${files.length + 1}${ext}`;
      if (typeof block.uri === "string") {
        try {
          const sourcePath = block.uri.startsWith("file://")
            ? block.uri.slice(7) : new URL(block.uri).pathname;
          const basename = decodeURIComponent(sourcePath).split(/[\\/]/).pop();
          if (basename) {
            const safe = basename.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").replace(/[. ]+$/, "");
            // Keep the extension consistent with the bytes' advertised MIME type.
            if (safe && safe !== "." && safe !== "..") name = `${path.parse(safe).name}${ext}`;
          }
        } catch { /* Inline image without a usable source name. */ }
      }
      // Separate subdirectories preserve names even when two images share a basename.
      const dir = path.join(tempDir, String(files.length));
      fs.mkdirSync(dir);
      const file = path.join(dir, name);
      fs.writeFileSync(file, bytes);
      files.push({ path: file, name });
    }
    return { files, cleanup };
  } catch (err) {
    cleanup();
    throw err;
  }
}

// Serialize the same DOM reader for both sites. The scope includes the editor,
// attachment previews and submit control, so old messages cannot satisfy readiness.
function inspectComposer(siteKey, names) {
  const editor = siteKey === "chatgpt"
    ? document.querySelector('#prompt-textarea, .ProseMirror[contenteditable="true"]')
    : document.querySelector('#chat-input') || [...document.querySelectorAll("textarea")].find(e => e.offsetParent);
  if (!editor) return { error: "找不到网页输入框。" };
  let scope = editor.closest('form') || editor.closest('[data-composer-surface-variant], [data-composer-root]');
  if (!scope && siteKey === "deepseek") {
    scope = editor.parentElement;
    while (scope && scope !== document.body && !scope.querySelector('input[type="file"]')) scope = scope.parentElement;
    // The DeepSeek attachment tray is a sibling above the editor/toolbar body.
    if (scope && scope !== document.body) scope = scope.parentElement;
  }
  if (!scope || scope === document.body) scope = editor.closest('[data-composer-body]') || editor.parentElement.parentElement;
  // File inputs can live outside the composer (React portals).
  const inputs = [...document.querySelectorAll('input[type="file"]')].filter(e => !e.disabled && !e.hasAttribute('capture'));
  const send = siteKey === "chatgpt"
    ? scope.querySelector('button[data-testid="send-button"], button[aria-label="发送"], button[aria-label="发送消息"], button[aria-label="发送提示"], button[aria-label="Send"], button[aria-label="Send prompt"], button[aria-label="Send message"], button[type="submit"]')
    : scope.querySelector('[role="button"].ds-button--primary.ds-button--circle, button[type="submit"]');
  const enabled = !!send && !send.disabled && send.getAttribute("aria-disabled") !== "true" &&
    !send.classList.contains("ds-button--disabled");
  const visible = e => {
    // DS retains loading overlays in the DOM after upload, hiding them with
    // opacity/visibility/scale. getClientRects().length alone still sees them.
    const rect = e.getBoundingClientRect();
    if (!rect.width || !rect.height) return false;
    if (typeof e.checkVisibility === "function" &&
        !e.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) return false;
    for (let node = e; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (node.hidden || node.getAttribute("aria-hidden") === "true" ||
          style.display === "none" || style.visibility === "hidden" ||
          style.visibility === "collapse" || Number(style.opacity) === 0) return false;
    }
    return true;
  };
  const busy = [...scope.querySelectorAll('[role="progressbar"], [aria-busy="true"], [class*="spinner" i], [class*="spin_" i], .ds-loading, .ds-icon-loading, [data-testid*="loading" i], .animate-spin')].some(visible);
  const previews = scope.cloneNode(true);
  // The user's prompt can itself contain filenames or words like "上传失败";
  // those are neither attachment acknowledgements nor upload errors.
  previews.querySelectorAll('textarea, [contenteditable], input').forEach(e => e.remove());
  const attributes = [...previews.querySelectorAll('[title], [aria-label], img[alt]')];
  const previewText = [previews.textContent, ...attributes.map(e => [e.title, e.getAttribute("aria-label"), e.getAttribute("alt")].join(" "))].join("\n");
  const seen = names.map(name => previewText.includes(name));
  const uploadError = /(?:上传失败|解析失败|不支持.{0,30}(?:格式|文件)|超出.{0,30}限制|文件.{0,20}(?:过大|太大)|upload failed|unsupported file|file too large|failed to upload|unable to upload|upload limit)[^\n]{0,100}/i;
  // .ds-message is also used for ordinary conversation bubbles, so those must
  // never be treated as upload errors.
  const errors = [...document.querySelectorAll('[role="alert"], [data-sonner-toast], .ds-toast')]
    .filter(visible).map(e => e.textContent.trim()).filter(text => uploadError.test(text));
  const localError = (previews.textContent || "").match(uploadError)?.[0];
  if (localError) errors.push(localError);
  return {
    inputs: inputs.map(e => ({ accept: e.accept, multiple: e.multiple })),
    seen, enabled, busy, errors,
  };
}

function accepts(accept, file) {
  if (!accept.trim()) return true;
  const ext = path.extname(file.name).toLowerCase();
  const mime = Object.entries(IMAGE_EXT).find(([, suffix]) => suffix === ext)?.[0]
    || (ext === ".jpeg" ? "image/jpeg" : "application/octet-stream");
  return accept.toLowerCase().split(",").some(token => {
    const type = token.trim();
    return type === "*/*" || type === ext || type === mime ||
      (type.endsWith("/*") && mime.startsWith(type.slice(0, -1)));
  });
}

export async function uploadAttachments(pageRef, siteKey, files, evalJson, cancelled) {
  const inspect = () => evalJson(pageRef, `JSON.stringify((${inspectComposer})(${JSON.stringify(siteKey)}, ${JSON.stringify(files.map(f => f.name))}))`);
  let before = await inspect();
  if (before.error) throw new Error(before.error);
  // A failed/cancelled upload must be resolved in the page before another send;
  // otherwise stale attachments could silently accompany a different message.
  const pending = await evalJson(pageRef, `JSON.stringify(window.__marionettePendingUpload || [])`);
  if (pending.length) throw new Error("上次的附件尚未确认发送。请在浏览器移除输入框中的附件并刷新该标签页，再重发。");
  // The editor can mount before React attaches its hidden file inputs. Some
  // ChatGPT layouts only mount them after the add-context menu opens.
  const inputDeadline = Date.now() + 10_000;
  const revealAfter = Date.now() + 1200;
  let revealed = false;
  let attemptedReveal = false;
  try {
    while (!before.inputs.length && Date.now() < inputDeadline) {
      if (cancelled()) return false;
      if (siteKey === "chatgpt" && !attemptedReveal && Date.now() >= revealAfter) {
        attemptedReveal = true;
        const result = await evalJson(pageRef, `(() => {
          const editor = document.querySelector('#prompt-textarea, .ProseMirror[contenteditable="true"]');
          const scope = editor?.closest('form') || editor?.closest('[data-composer-surface-variant], [data-composer-root]');
          const button = scope?.querySelector('button[data-composer-navigation-target="add-context"], button[data-testid="composer-plus-btn"], button[aria-label="添加文件等内容"], button[aria-label="Add photos & files"], button[aria-label="Add files and more"]');
          if (!button || button.disabled || button.getAttribute('aria-disabled') === 'true' || button.getAttribute('aria-expanded') === 'true') return JSON.stringify({opened: false});
          button.click();
          return JSON.stringify({opened: true});
        })()`);
        revealed = result.opened;
      }
      await new Promise(resolve => setTimeout(resolve, 250));
      before = await inspect();
      if (before.error) throw new Error(before.error);
    }
    if (cancelled()) return false;
    if (!before.inputs.length) throw new Error("网页附件上传入口未加载，请在浏览器点击输入框旁的＋检查是否支持上传，再重试。");
    const groups = new Map();
    for (const file of files) {
      // Prefer the general file picker: ChatGPT also exposes separate photo/video
      // pickers, which would split a mixed image/document batch unnecessarily.
      const general = before.inputs.findIndex(input => !input.accept.trim());
      const index = general >= 0 ? general : before.inputs.findIndex(input => accepts(input.accept, file));
      if (index < 0) throw new Error(`网页不接受附件格式：${file.name}`);
      const group = groups.get(index) || [];
      group.push(file.path);
      groups.set(index, group);
    }
    // Validate the whole batch before starting any upload.
    for (const [index, paths] of groups) {
      if (paths.length > 1 && !before.inputs[index].multiple) throw new Error("这个网页上传入口一次只能选择一个文件。");
    }
    await evalJson(pageRef, `(() => { window.__marionettePendingUpload = ${JSON.stringify(files.map(f => f.name))}; return "{}"; })()`);
    for (const [index, paths] of groups) {
      if (cancelled()) return false;
      const marker = randomUUID();
      const marked = await evalJson(pageRef, `(() => {
        const input = [...document.querySelectorAll('input[type="file"]')].filter(e => !e.disabled && !e.hasAttribute('capture'))[${index}];
        if (!input) return JSON.stringify({ok: false});
        input.setAttribute('data-marionette-upload', ${JSON.stringify(marker)});
        return JSON.stringify({ok: true});
      })()`);
      if (!marked.ok) throw new Error("网页上传入口已变化，请刷新后重试。");
      const { root } = await pageRef.cdp.send("DOM.getDocument", {}, pageRef.sessionId);
      const { nodeId } = await pageRef.cdp.send("DOM.querySelector", {
        nodeId: root.nodeId, selector: `[data-marionette-upload="${marker}"]`,
      }, pageRef.sessionId);
      if (!nodeId) throw new Error("网页上传入口已失效。");
      // Chromium triggers the real input/change handlers and the site's own uploader.
      await pageRef.cdp.send("DOM.setFileInputFiles", { nodeId, files: paths }, pageRef.sessionId);
    }
    const oldErrors = new Set(before.errors);
    const deadline = Date.now() + 120_000;
    let readySince = 0;
    while (Date.now() < deadline) {
      if (cancelled()) return false;
      const status = await inspect();
      if (status.error) throw new Error(status.error);
      const error = status.errors.find(text => !oldErrors.has(text));
      if (error) throw new Error(`网页附件处理失败：${error.slice(0, 150)}`);
      if (status.seen.every(Boolean) && status.enabled && !status.busy) {
        readySince ||= Date.now();
        if (Date.now() - readySince >= 1500) return true;
      } else readySince = 0;
      await new Promise(resolve => setTimeout(resolve, 400));
    }
    throw new Error("等待附件上传/解析超时，消息未发送。请到浏览器检查附件状态，移除附件并刷新后重试。");
  } finally {
    if (revealed) {
      await evalJson(pageRef, `(() => {
        const editor = document.querySelector('#prompt-textarea, .ProseMirror[contenteditable="true"]');
        const scope = editor?.closest('form') || editor?.closest('[data-composer-surface-variant], [data-composer-root]');
        const button = scope?.querySelector('button[data-composer-navigation-target="add-context"][aria-expanded="true"], button[data-testid="composer-plus-btn"][aria-expanded="true"], button[aria-label="添加文件等内容"][aria-expanded="true"], button[aria-label="Add photos & files"][aria-expanded="true"], button[aria-label="Add files and more"][aria-expanded="true"]');
        button?.click();
        return '{}';
      })()`).catch(() => {}); // Menu cleanup must not mask an upload failure.
    }
  }
}

export async function clearUploadedAttachments(pageRef, evalJson) {
  await evalJson(pageRef, `(() => { delete window.__marionettePendingUpload; return "{}"; })()`);
}
