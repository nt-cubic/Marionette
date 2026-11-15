// Marionette page probes — read-only side questions an agent asks between two
// snapshots. Injected into the agent's isolated world (never the page's), at
// document start and, for a document already open, on first use.
//
// Why this file exists: the aria engine in `agent.bundle.js` is a vendored
// esbuild product and stays byte-for-byte upstream (see VENDOR.md). Everything
// that is "cheap question about the page" rather than "render the tree" lives
// here so that bundle never has to be rebuilt for our sake.
//
// The questions come from Cloudflare's kitesurf (packages/engine/src/cdp):
// `Kitesurf.getPageState` answered with a DOM mutation counter instead of a
// tree, and `Kitesurf.setControlValue` wrote a control by node id instead of
// synthesizing keystrokes. Both translate directly: ask "did anything change"
// for the price of one eval, and set a checkbox/select without a click.
(() => {
  if (globalThis.__marionetteProbe) return;

  // One counter for the document's whole life. `attributes` is in here because
  // a framework flipping `checked` / `value` / `class` is exactly the change an
  // agent must not miss; the records themselves are discarded, so a page
  // animating at 60fps costs a number, not a queue.
  let seq = 0;
  try {
    const observer = new MutationObserver(() => {
      seq += 1;
    });
    observer.observe(document, {
      childList: true,
      subtree: true,
      attributes: true,
      characterData: true,
    });
  } catch {
    /* a document that refuses observation simply never reports a change */
  }

  // The aria bundle's ref table. Reached through its own published global so
  // this script needs no knowledge of its internals.
  function elementForRef(generation, ref) {
    const agent = globalThis.__codegAgent;
    if (!agent || typeof agent.elementForRef !== "function") return null;
    try {
      return agent.elementForRef(generation, ref) || null;
    } catch {
      return null;
    }
  }

  function state() {
    return {
      seq: seq,
      url: String(location.href),
      title: String(document.title || ""),
      readyState: String(document.readyState),
      scrollX: Math.round(window.scrollX || 0),
      scrollY: Math.round(window.scrollY || 0),
      viewport: {
        width: window.innerWidth,
        height: window.innerHeight,
        dpr: window.devicePixelRatio,
      },
      focused: document.hasFocus(),
    };
  }

  // "Has the page settled" — the question that stops an agent from reading a
  // skeleton screen. Stylesheets are not observable, so this claims less than
  // the truth: `settled` is false whenever something it can see is pending.
  function ready() {
    let images = 0;
    try {
      for (const image of document.images) if (!image.complete) images += 1;
    } catch {
      /* a hostile collection iteration just leaves the count at zero */
    }
    let fonts = "unknown";
    try {
      fonts = document.fonts ? String(document.fonts.status) : "unknown";
    } catch {
      fonts = "unknown";
    }
    const readyState = String(document.readyState);
    return {
      seq: seq,
      settled: readyState === "complete" && images === 0 && fonts !== "loading",
      readyState: readyState,
      images: images,
      fonts: fonts,
    };
  }

  function fire(el, type) {
    el.dispatchEvent(new Event(type, { bubbles: true, composed: true }));
  }

  // Write a control the way its own framework would see it: the native value
  // setter (a plain assignment is invisible to React-style value tracking),
  // then input + change. A checkbox goes through `click()` so its own
  // activation behaviour runs — a boolean write alone leaves listeners cold.
  function setValue(generation, ref, value) {
    const el = elementForRef(generation, ref);
    if (!el) {
      return {
        ok: false,
        error: "stale",
        detail:
          ref +
          " does not name an element on the page as it is now; take a new snapshot",
      };
    }
    if (el.disabled) {
      return { ok: false, error: "disabled", detail: ref + " is disabled" };
    }
    const tag = el.tagName ? String(el.tagName).toLowerCase() : "";
    const type = el.type ? String(el.type).toLowerCase() : "";

    if (tag === "input" && (type === "checkbox" || type === "radio")) {
      const want =
        value === true || value === "true" || value === 1 || value === "1";
      if (el.checked !== want) el.click();
      return { ok: true, seq: seq, value: el.checked };
    }

    if (tag === "select") {
      const want = (Array.isArray(value) ? value : [value]).map((v) =>
        String(v)
      );
      let changed = false;
      for (const option of el.options) {
        const label = String(option.text || "").trim();
        const on =
          want.indexOf(String(option.value)) !== -1 ||
          want.indexOf(label) !== -1;
        if (option.selected !== on) {
          option.selected = on;
          changed = true;
        }
      }
      if (changed) {
        fire(el, "input");
        fire(el, "change");
      }
      const chosen = [];
      for (const option of el.selectedOptions || el.options) {
        if (option.selected) chosen.push(String(option.value));
      }
      return { ok: true, seq: seq, changed: changed, value: chosen };
    }

    if (!("value" in el)) {
      return {
        ok: false,
        error: "not-editable",
        detail: ref + " takes no value",
      };
    }

    const text = value === null || value === undefined ? "" : String(value);
    if (el.value === text) {
      return { ok: true, seq: seq, changed: false, value: String(el.value) };
    }
    const proto =
      tag === "textarea"
        ? HTMLTextAreaElement.prototype
        : tag === "select"
          ? HTMLSelectElement.prototype
          : HTMLInputElement.prototype;
    let setter = null;
    try {
      setter = Object.getOwnPropertyDescriptor(proto, "value").set;
    } catch {
      setter = null;
    }
    try {
      if (setter) setter.call(el, text);
      else el.value = text;
    } catch {
      el.value = text;
    }
    fire(el, "input");
    fire(el, "change");
    return { ok: true, seq: seq, changed: true, value: String(el.value) };
  }

  globalThis.__marionetteProbe = { state, ready, setValue };
})();