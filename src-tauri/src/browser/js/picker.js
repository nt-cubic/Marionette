// Marionette element picker — the user-side "point at a thing and say what
// should change" overlay, the browser toolbar's annotate mode. Injected into
// the agent's isolated world (never the page's), at document start and, for a
// document already open, on first use — the same shape as `js/probe.js`.
//
// Why a separate file: the aria engine in `agent.bundle.js` is a vendored
// esbuild product and stays byte-for-byte upstream (see VENDOR.md). The
// picker is pure UI for the person driving the browser, so it lives here and
// only *reads* the engine's published API: a picked element is mapped to an
// aria ref through the `_ariaRef` expando that the engine (running in this
// same isolated world) caches on visited elements, validated against the
// published `elementForRef`. No bundle internals, no rebuild.
//
// Everything the user sees is painted inside a shadow root: the page's CSS
// cannot reach in, and the page's scripts see one aria-hidden host div.
// While picking, a full-screen host turns the page beneath into a read-only
// hit-test surface: real pointer events target the overlay, synthetic events
// are ignored, and the underlying control never receives activation. Picks
// travel back through the `__marionettePick` binding (our own name; the
// vendored `__codegSend` is helper.js's and stays theirs).
(() => {
  if (globalThis.__marionettePicker) return;

  const BINDING = "__marionettePick";
  const HOST_Z = 2147483647;
  // A page can be enormous; bounding every node is a one-shot cost the user
  // just paid for with a drag, but it still gets a ceiling.
  const REGION_SCAN_CAP = 20000;
  const REGION_MAX_ELEMENTS = 20;
  const REGION_MIN_DRAG = 6;
  // Kept in sync with the frontend drawer and the Rust payload ring. A batch
  // this small is still generous for a design review and bounds the screenshot
  // data URLs that otherwise live in React state.
  const PICK_LIMIT = 24;
  const REGION_SELECTOR =
    "a,button,input,textarea,select,img,svg,iframe,frame,[role],[tabindex],h1,h2,h3,h4,h5,h6,p,li,label";

  // ── state ────────────────────────────────────────────────────────────────
  let active = false;
  let mode = "element"; // "element" | "region"
  let hoverEl = null;
  let dragging = false;
  let dragStart = null; // {x, y}
  // Set by the region scan when it hit the ceiling — the element list is then
  // honest about being a sample rather than everything.
  let scanCapped = false;
  let docToken = Math.random().toString(36).slice(2, 8);
  let pickCounter = 0;
  // Picked elements are retained so `resolveRefs` can map them to aria refs
  // when the user finally presses "send to conversation" — possibly after
  // picking several more and writing notes. A navigation replaces the whole
  // world (and this script with it), which is the natural clear.
  const picked = new Map();

  // ── overlay (shadow DOM) ─────────────────────────────────────────────────
  let host = null;
  let outline = null;
  let outlineLabel = null;
  let dragBox = null;
  let dragLabel = null;
  let captureHidden = false;
  let previousFocus = null;

  function ensureOverlay() {
    if (host && host.isConnected) return true;
    try {
      host = document.createElement("div");
      host.setAttribute("aria-hidden", "true");
      host.setAttribute("data-marionette-picker", "");
      host.tabIndex = -1;
      host.style.cssText =
        "position:fixed!important;inset:0!important;width:auto!important;" +
        "height:auto!important;margin:0!important;padding:0!important;border:0!important;" +
        "background:none!important;z-index:" + HOST_Z + "!important;" +
        "pointer-events:auto!important;display:none!important;cursor:crosshair!important;";
      const shadow = host.attachShadow({ mode: "closed" });
      const style = document.createElement("style");
      style.textContent =
        ":host{all:initial;cursor:crosshair}" +
        ".mp-outline{position:fixed;pointer-events:none;border:2px solid #4a9eff;" +
        "background:rgba(74,158,255,.14);box-sizing:border-box;border-radius:2px;display:none}" +
        ".mp-drag{position:fixed;pointer-events:none;border:1.5px dashed #4a9eff;" +
        "background:rgba(74,158,255,.10);box-sizing:border-box;display:none}" +
        ".mp-label{position:fixed;pointer-events:none;max-width:60vw;white-space:nowrap;" +
        "overflow:hidden;text-overflow:ellipsis;font:11px/1.6 -apple-system,'Segoe UI',sans-serif;" +
        "color:#fff;background:#1f2733;border:1px solid #4a9eff;border-radius:3px;" +
        "padding:0 6px;display:none}";
      shadow.appendChild(style);
      outline = document.createElement("div");
      outline.className = "mp-outline";
      shadow.appendChild(outline);
      dragBox = document.createElement("div");
      dragBox.className = "mp-drag";
      shadow.appendChild(dragBox);
      outlineLabel = document.createElement("div");
      outlineLabel.className = "mp-label";
      shadow.appendChild(outlineLabel);
      dragLabel = document.createElement("div");
      dragLabel.className = "mp-label";
      shadow.appendChild(dragLabel);
      document.documentElement.appendChild(host);
      return true;
    } catch {
      host = null;
      return false;
    }
  }

  function showOverlay() {
    if (host) {
      host.style.display = "";
      captureHidden = false;
    }
  }

  function place(box, node, label, labelNode) {
    node.style.display = "block";
    node.style.left = box.x + "px";
    node.style.top = box.y + "px";
    node.style.width = Math.max(box.width, 2) + "px";
    node.style.height = Math.max(box.height, 2) + "px";
    if (labelNode) {
      labelNode.style.display = "block";
      labelNode.textContent = label;
      const above = box.y > 22;
      labelNode.style.top = above
        ? Math.max(box.y - 20, 0) + "px"
        : Math.min(box.y + box.height + 2, window.innerHeight - 20) + "px";
      const width = labelNode.offsetWidth || 120;
      labelNode.style.left = Math.min(Math.max(box.x, 4), Math.max(window.innerWidth - width - 4, 4)) + "px";
    }
  }

  function hideHover() {
    if (outline) outline.style.display = "none";
    if (outlineLabel) outlineLabel.style.display = "none";
    hoverEl = null;
  }

  function hideDrag() {
    if (dragBox) dragBox.style.display = "none";
    if (dragLabel) dragLabel.style.display = "none";
  }

  // ── page facts (capped at the boundary) ──────────────────────────────────
  function collapse(text) {
    return String(text || "").replace(/\s+/g, " ").trim();
  }

  function isElement(node) {
    return !!node && node.nodeType === 1 && typeof node.getBoundingClientRect === "function";
  }

  function viewportRect(el) {
    const rect = el.getBoundingClientRect();
    let x = rect.x;
    let y = rect.y;
    let win = el.ownerDocument && el.ownerDocument.defaultView;
    for (let depth = 0; win && win.frameElement && depth < 12; depth++) {
      const frame = win.frameElement;
      const outer = frame.getBoundingClientRect();
      x += outer.x + (frame.clientLeft || 0);
      y += outer.y + (frame.clientTop || 0);
      win = frame.ownerDocument && frame.ownerDocument.defaultView;
    }
    return { x, y, width: rect.width, height: rect.height };
  }

  function textOf(el, limit) {
    const tag = String(el.tagName || "").toLowerCase();
    if (tag === "input" || tag === "textarea") {
      // Values can hold what the page autofilled — placeholders describe the
      // control without copying secrets into a prompt draft.
      return collapse(el.getAttribute("placeholder") || el.getAttribute("aria-label") || "");
    }
    if (tag === "select") {
      const option = el.selectedOptions && el.selectedOptions[0];
      return collapse(option ? option.text : el.getAttribute("aria-label") || "");
    }
    return collapse(el.textContent).slice(0, limit);
  }

  function describe(el, textLimit) {
    const box = viewportRect(el);
    const tag = String(el.tagName || "").toLowerCase();
    const info = {
      tag: tag,
      id: el.id ? String(el.id) : "",
      classes: el.classList
        ? Array.from(el.classList).slice(0, 3).map(String)
        : [],
      text: textOf(el, textLimit),
      rect: {
        x: Math.round(box.x),
        y: Math.round(box.y),
        width: Math.round(box.width),
        height: Math.round(box.height),
      },
      dpr: window.devicePixelRatio || 1,
      selector: selectorOf(el),
    };
    if (el.getAttribute) {
      const href = el.getAttribute("href");
      if (tag === "a" && href) {
        try {
          info.href = String(new URL(href, location.href)).slice(0, 300);
        } catch {
          info.href = collapse(href).slice(0, 300);
        }
      }
      const placeholder = el.getAttribute("placeholder");
      if (placeholder) info.placeholder = collapse(placeholder).slice(0, 80);
    }
    return info;
  }

  function deepElementFromPoint(x, y) {
    let doc = document;
    let localX = x;
    let localY = y;
    for (let depth = 0; doc && depth < 12; depth++) {
      let el = doc.elementFromPoint(localX, localY);
      for (let shadowDepth = 0; el && el.shadowRoot && shadowDepth < 12; shadowDepth++) {
        let inner = null;
        try {
          inner = el.shadowRoot.elementFromPoint(localX, localY);
        } catch {
          break;
        }
        if (!inner || inner === el) break;
        el = inner;
      }
      if (el && (el.tagName === "IFRAME" || el.tagName === "FRAME")) {
        try {
          const frameDoc = el.contentDocument;
          if (frameDoc) {
            const box = el.getBoundingClientRect();
            localX -= box.x + (el.clientLeft || 0);
            localY -= box.y + (el.clientTop || 0);
            doc = frameDoc;
            continue;
          }
        } catch {
          /* cross-origin frame: the iframe element remains the target */
        }
      }
      return isElement(el) ? el : null;
    }
    return null;
  }

  // The full-screen host is the input barrier. Hide it for one synchronous hit
  // test, then restore it before returning to the browser event loop.
  function elementAtEvent(event) {
    if (!host) return null;
    const previous = host.style.getPropertyValue("pointer-events");
    const priority = host.style.getPropertyPriority("pointer-events");
    host.style.setProperty("pointer-events", "none", "important");
    let el = null;
    try {
      el = deepElementFromPoint(event.clientX, event.clientY);
    } finally {
      host.style.setProperty("pointer-events", previous || "auto", priority || "important");
    }
    return el && el !== host ? el : null;
  }

  function localSelector(el, root) {
    if (el.id) {
      const id = "#" + CSS.escape(String(el.id));
      if (root.querySelectorAll(id).length === 1) return id;
    }
    if (el.getAttribute) {
      for (const attr of ["data-testid", "data-test", "data-cy"]) {
        const value = el.getAttribute(attr);
        if (value) {
          const sel =
            "[" + attr + '="' + String(value).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"]';
          if (root.querySelectorAll(sel).length === 1) return sel;
        }
      }
    }
    const parts = [];
    let node = el;
    for (let depth = 0; node && node.nodeType === 1 && depth < 12; depth++) {
      const tag = String(node.tagName).toLowerCase();
      if (tag === "html") break;
      if (node.id) {
        parts.unshift("#" + CSS.escape(String(node.id)));
        break;
      }
      let segment = tag;
      const parent = node.parentElement;
      if (parent) {
        const same = Array.from(parent.children).filter(
          (child) => String(child.tagName).toLowerCase() === tag,
        );
        if (same.length > 1) segment += ":nth-of-type(" + (same.indexOf(node) + 1) + ")";
      }
      parts.unshift(segment);
      if (tag === "body" || parent === root || !parent) break;
      node = parent;
    }
    return parts.join(" > ") || String(el.tagName || "").toLowerCase();
  }

  function selectorOf(el) {
    try {
      const parts = [];
      let node = el;
      for (let depth = 0; node && depth < 12; depth++) {
        const root = node.getRootNode();
        parts.unshift(localSelector(node, root));
        // Open shadow roots and same-origin frames both need their owner as a
        // prefix. Closed roots and cross-origin documents remain opaque; their
        // top-level host/frame is the stable target in those cases.
        if (root && root.nodeType === 11 && root.host) {
          node = root.host;
          continue;
        }
        const win = node.ownerDocument && node.ownerDocument.defaultView;
        node = win && win.frameElement ? win.frameElement : null;
      }
      return parts.join(" >> ");
    } catch {
      return String(el.tagName || "").toLowerCase();
    }
  }

  // ── back channel ─────────────────────────────────────────────────────────
  function send(message) {
    try {
      if (typeof globalThis[BINDING] === "function") {
        globalThis[BINDING](JSON.stringify(message));
      }
    } catch {
      /* a missing or failed binding only loses this one report */
    }
  }

  function nextPickId() {
    return "p" + docToken + "-" + ++pickCounter;
  }

  function hoverLabel(el) {
    const info = describe(el, 60);
    const bits = [info.tag];
    if (info.id) bits.push("#" + info.id);
    else if (info.classes.length) bits.push("." + info.classes.join("."));
    const box = info.rect;
    bits.push(box.width + "×" + box.height);
    return { info: info, label: bits.join(" ") };
  }

  function pickElement(el) {
    const pickId = nextPickId();
    picked.set(pickId, el);
    const info = describe(el, 120);
    place(info.rect, outline, hoverLabel(el).label, outlineLabel);
    send({ kind: "pick", pickId: pickId, info: info });
    stopAtLimit();
  }

  function stopAtLimit() {
    if (pickCounter < PICK_LIMIT) return;
    send({ kind: "limit", limit: PICK_LIMIT });
    stop();
  }

  // ── region mode ──────────────────────────────────────────────────────────
  function regionElements(rect) {
    const found = [];
    let scanned = 0;
    scanCapped = false;
    const all = [];
    function collect(root, depth) {
      if (!root || depth > 12 || scanned >= REGION_SCAN_CAP) return;
      let list;
      try {
        list = root.querySelectorAll(REGION_SELECTOR);
      } catch {
        return;
      }
      for (const el of list) {
        if (scanned >= REGION_SCAN_CAP) {
          scanCapped = true;
          return;
        }
        scanned++;
        all.push(el);
        if (el.shadowRoot) collect(el.shadowRoot, depth + 1);
      }
      for (const frame of root.querySelectorAll("iframe,frame")) {
        let frameBox;
        try {
          frameBox = viewportRect(frame);
        } catch {
          continue;
        }
        const right = rect.x + rect.width;
        const bottom = rect.y + rect.height;
        if (
          frameBox.width < 1 ||
          frameBox.height < 1 ||
          frameBox.x + frameBox.width <= rect.x ||
          frameBox.x >= right ||
          frameBox.y + frameBox.height <= rect.y ||
          frameBox.y >= bottom
        ) {
          continue;
        }
        try {
          if (frame.contentDocument) collect(frame.contentDocument, depth + 1);
          else scanCapped = true;
        } catch {
          scanCapped = true;
        }
      }
    }
    collect(document, 0);
    const right = rect.x + rect.width;
    const bottom = rect.y + rect.height;
    for (const el of all) {
      if (el === host) continue;
      let box;
      try {
        box = viewportRect(el);
      } catch {
        continue;
      }
      if (box.width < 4 || box.height < 4) continue;
      if (box.right <= rect.x || box.x >= right || box.bottom <= rect.y || box.y >= bottom) {
        continue;
      }
      found.push({ el: el, area: box.width * box.height });
    }
    // Smallest first: the concrete controls the user boxed, not the page
    // wrappers that happen to contain them.
    found.sort((a, b) => a.area - b.area);
    return found.slice(0, REGION_MAX_ELEMENTS).map((item) => describe(item.el, 60));
  }

  // ── listeners (the full-screen host is the input barrier) ─────────────────
  function stopEvent(event) {
    event.stopPropagation();
    event.stopImmediatePropagation();
    event.preventDefault();
  }

  function canPick() {
    if (pickCounter < PICK_LIMIT) return true;
    stopAtLimit();
    return false;
  }

  function onPointerMove(event) {
    if (!active || !event.isTrusted) return;
    event.stopImmediatePropagation();
    if (dragging && mode === "region" && dragStart) {
      const rect = {
        x: Math.min(dragStart.x, event.clientX),
        y: Math.min(dragStart.y, event.clientY),
        width: Math.abs(event.clientX - dragStart.x),
        height: Math.abs(event.clientY - dragStart.y),
      };
      place(rect, dragBox, rect.width + "×" + rect.height, dragLabel);
      return;
    }
    if (mode !== "element") return;
    const el = elementAtEvent(event);
    if (!el || el === hoverEl) return;
    hoverEl = el;
    const named = hoverLabel(el);
    const box = named.info.rect;
    if (box.width <= 0 || box.height <= 0) {
      hideHover();
      return;
    }
    place(box, outline, named.label, outlineLabel);
  }

  function onPointerDown(event) {
    if (!active || !event.isTrusted || event.button !== 0) return;
    stopEvent(event);
    if (!canPick()) return;
    try {
      host.setPointerCapture(event.pointerId);
    } catch {
      /* pointer capture is best-effort; host still covers the viewport */
    }
    if (mode === "element") {
      const el = elementAtEvent(event);
      if (el) pickElement(el);
      return;
    }
    if (mode === "region") {
      dragging = true;
      dragStart = { x: event.clientX, y: event.clientY };
      hideHover();
    }
  }

  function onPointerUp(event) {
    if (!active || !event.isTrusted) return;
    stopEvent(event);
    try {
      host.releasePointerCapture(event.pointerId);
    } catch {
      /* not captured */
    }
    if (dragging && mode === "region" && dragStart) {
      dragging = false;
      const rect = {
        x: Math.min(dragStart.x, event.clientX),
        y: Math.min(dragStart.y, event.clientY),
        width: Math.abs(event.clientX - dragStart.x),
        height: Math.abs(event.clientY - dragStart.y),
      };
      dragStart = null;
      hideDrag();
      if (rect.width < REGION_MIN_DRAG || rect.height < REGION_MIN_DRAG) return;
      const pickId = nextPickId();
      const elements = regionElements(rect);
      send({
        kind: "region",
        pickId: pickId,
        rect: rect,
        dpr: window.devicePixelRatio || 1,
        elements: elements,
        scanCapped: scanCapped || undefined,
      });
      stopAtLimit();
    }
  }

  function onPointerCancel(event) {
    if (!event.isTrusted) return;
    stopEvent(event);
    try {
      host.releasePointerCapture(event.pointerId);
    } catch {
      /* not captured */
    }
    dragging = false;
    dragStart = null;
    hideDrag();
  }

  function onScroll() {
    if (!active) return;
    hideHover();
    if (!dragging) hideDrag();
  }

  function onKeyDown(event) {
    if (!active || !event.isTrusted) return;
    if (event.key === "Escape") {
      stopEvent(event);
      send({ kind: "cancel" });
      stop();
    } else {
      // No page shortcut or form submission runs while picking. The toolbar's
      // Stop button and Esc are the two explicit ways out.
      stopEvent(event);
    }
  }

  // Everything else the pointer or a stray Enter could trigger on the page is
  // swallowed while picking — pointing must never click.
  function swallow(event) {
    if (!active) return;
    stopEvent(event);
  }

  const HOST_LISTENERS = [
    ["pointermove", onPointerMove, true],
    ["pointerdown", onPointerDown, true],
    ["pointerup", onPointerUp, true],
    ["pointercancel", onPointerCancel, true],
    ["mousedown", swallow, true],
    ["mouseup", swallow, true],
    ["click", swallow, true],
    ["dblclick", swallow, true],
    ["auxclick", swallow, true],
    ["contextmenu", swallow, true],
  ];

  function attach() {
    for (const [type, handler, capture] of HOST_LISTENERS) {
      host.addEventListener(type, handler, capture);
    }
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("scroll", onScroll, true);
  }

  function detach() {
    for (const [type, handler, capture] of HOST_LISTENERS) {
      host.removeEventListener(type, handler, capture);
    }
    window.removeEventListener("keydown", onKeyDown, true);
    window.removeEventListener("scroll", onScroll, true);
  }

  // ── public surface ───────────────────────────────────────────────────────
  function start(nextMode) {
    mode = nextMode === "region" ? "region" : "element";
    if (!ensureOverlay()) {
      return { ok: false, error: "cannot attach the picker overlay to this document" };
    }
    showOverlay();
    if (!active) {
      previousFocus = document.activeElement;
      attach();
      active = true;
    }
    try {
      host.focus({ preventScroll: true });
    } catch {
      /* older engines do not take focus options */
    }
    hideHover();
    hideDrag();
    return { ok: true, mode: mode };
  }

  function stop() {
    if (!active) return { ok: true };
    detach();
    active = false;
    dragging = false;
    dragStart = null;
    hideHover();
    hideDrag();
    if (host) host.style.display = "none";
    if (
      previousFocus &&
      previousFocus !== host &&
      previousFocus.isConnected &&
      typeof previousFocus.focus === "function"
    ) {
      try {
        previousFocus.focus({ preventScroll: true });
      } catch {
        /* focus restoration is convenience only */
      }
    }
    previousFocus = null;
    return { ok: true };
  }

  function clear() {
    stop();
    picked.clear();
    pickCounter = 0;
    return { ok: true };
  }

  function setMode(nextMode) {
    mode = nextMode === "region" ? "region" : "element";
    hideHover();
    hideDrag();
    return { ok: true, mode: mode };
  }

  // Screenshot support: the highlight must not photobomb the crop the user
  // is about to attach. Hide → capture → restore, always in pairs.
  function captureHide() {
    if (host && !captureHidden) {
      host.style.display = "none";
      captureHidden = true;
    }
    return true;
  }

  function captureRestore() {
    if (host && captureHidden) {
      host.style.display = active ? "" : "none";
      captureHidden = false;
    }
    return true;
  }

  function resolveRefs(generation) {
    const agent = globalThis.__codegAgent;
    const out = {};
    for (const [id, el] of picked) {
      let entry = null;
      try {
        if (el && el.isConnected) {
          const cached = el._ariaRef; // the engine's per-element cache (this world)
          const ref = cached && typeof cached.ref === "string" ? cached.ref : "";
          if (ref && agent && typeof agent.elementForRef === "function") {
            if (agent.elementForRef(generation, ref) === el) {
              entry = { ref: ref, role: String(cached.role || ""), name: String(cached.name || "") };
            }
          }
        }
      } catch {
        entry = null;
      }
      out[id] = entry;
    }
    return out;
  }

  globalThis.__marionettePicker = {
    start: start,
    stop: stop,
    clear: clear,
    setMode: setMode,
    resolveRefs: resolveRefs,
    captureHide: captureHide,
    captureRestore: captureRestore,
  };
})();
