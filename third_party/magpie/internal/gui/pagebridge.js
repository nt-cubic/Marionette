// The page's side of a plugin page's messages to magpie's window
// (docs/subsystems/plugin-runtime.md, P4). A plugin's page loads it with
// <script src="_magpie/bridge.js"></script>; it is served beside every
// page's own files.
//
//   await magpie.ready            the window's theme, once it has answered
//   magpie.theme                  { dark, lang, tokens: { bg, fg, … } }
//   magpie.onTheme(fn)            fn(theme) on each change of it
//   await magpie.options.get()    the plugin's own options (plugins.json)
//   await magpie.options.set(o)   sets them, and answers with them
//   await magpie.read(name, p)    a read its package.json's
//                                 magpie.permissions declares
//
// The page can't reach the network or magpie's API itself: everything goes
// through the window, which answers only what the package declared. An
// error the page doesn't catch shows the window's "This page stopped
// working" in its place.
(() => {
  const waiting = new Map();
  let next = 0;
  const themed = [];
  function call(method, params) {
    const id = ++next;
    return new Promise((resolve, reject) => {
      waiting.set(id, { resolve, reject });
      parent.postMessage({ magpie: 1, id, method, params }, "*");
    });
  }
  function paint(theme) {
    const root = document.documentElement;
    root.dataset.theme = theme.dark ? "dark" : "light";
    // the frame's canvas matches the window's, not a white sheet in dark
    root.style.colorScheme = theme.dark ? "dark" : "light";
    root.lang = theme.lang || "en";
    for (const [k, v] of Object.entries(theme.tokens || {})) root.style.setProperty("--magpie-" + k, v);
  }
  addEventListener("message", (e) => {
    if (e.source !== parent) return;
    const m = e.data;
    if (!m || m.magpie !== 1) return;
    if (m.event === "theme") {
      api.theme = m.data;
      paint(m.data);
      for (const fn of themed) { try { fn(m.data); } catch (err) { report(err); } }
      return;
    }
    const w = waiting.get(m.id);
    if (!w) return;
    waiting.delete(m.id);
    if (m.error) w.reject(new Error(m.error)); else w.resolve(m.result);
  });
  function report(err) {
    parent.postMessage({ magpie: 1, method: "error", params: { message: String(err?.message || err) } }, "*");
  }
  // a script's error reaches a sandboxed page only as "Script error." (its
  // files are another origin's to it); the window then says "The page hit
  // an error" in the reader's language, and the console has the rest
  addEventListener("error", (e) => report(e.error || (e.message === "Script error." ? "" : e.message)));
  addEventListener("unhandledrejection", (e) => report(e.reason));
  const api = {
    theme: null,
    onTheme(fn) { themed.push(fn); },
    options: {
      get: () => call("options.get"),
      set: (options) => call("options.set", { options }),
    },
    read: (name, params) => call("read", { name, params }),
  };
  api.ready = call("hello").then((theme) => { api.theme = theme; paint(theme); return theme; });
  window.magpie = api;
})();
