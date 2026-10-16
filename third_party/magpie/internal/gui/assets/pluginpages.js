// A community plugin's own page in the window (docs/subsystems/
// plugin-runtime.md, P4): package.json's magpie.page, in a tab of the
// sidebar's Plugins group, after the built-in pages. The page runs in a
// frame sandboxed to scripts alone, in an origin of its own: it has no
// way to the window, to magpie's API (apiFramed) or to the network (its
// policy, pluginpage.go). What it asks of the window comes by message,
// from that frame only, and the window answers only what the page may:
// the theme, its own options, and the reads its package declared
// (magpie.permissions), which /api/plugin-pages/read checks again.
// A page that throws, or never says hello, shows the kernel's "This
// page stopped working" with its Try again.
(() => {
  if (mode !== "window") return;
  // how long a page has to say hello before it is taken as broken
  const HELLO_MS = 10e3;
  const pages = new Map(); // key → { entry, frame, hello, timer }
  let ctx = null;
  magpie.plugin("plugin-pages", (c) => { ctx = c; });

  const TOKENS = ["bg", "card", "fg", "fg-2", "muted", "faint", "line", "accent", "accent-soft", "accent-fg", "green", "red", "amber", "font", "code"];
  function theme() {
    const css = getComputedStyle(document.documentElement);
    const tokens = {};
    for (const k of TOKENS) tokens[k] = css.getPropertyValue("--" + k).trim();
    const dark = document.documentElement.dataset.theme === "dark"
      || (document.documentElement.dataset.theme !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
    return { dark, lang: locale, tokens };
  }

  function post(win, msg) { win?.postMessage({ magpie: 1, ...msg }, "*"); }

  // the theme or the language changed: every page open is told
  function pushTheme() {
    const data = theme();
    for (const p of pages.values()) if (p.hello) post(p.frame?.contentWindow, { event: "theme", data });
  }
  new MutationObserver(pushTheme).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "lang", "class", "style"] });
  matchMedia("(prefers-color-scheme: dark)").addEventListener?.("change", pushTheme);

  function broke(key, why) { ctx?.broke(key, new Error(why)); }

  // the page's frame, made the first time the page is opened (and again
  // after Try again): a plugin's page costs nothing until it is looked at
  function open(key) {
    const p = pages.get(key);
    if (!p) return undefined;
    if (p.frame?.isConnected) return p.ready;
    const main = $("#view-" + key);
    const f = document.createElement("iframe");
    f.className = "plugin-frame";
    // scripts, and nothing else: no same origin (no cookie, no storage of
    // the window's, no reach into it), no forms, popups or top navigation
    f.setAttribute("sandbox", "allow-scripts");
    f.setAttribute("referrerpolicy", "no-referrer");
    f.setAttribute("allow", "");
    f.title = p.entry.title;
    f.src = p.entry.url;
    p.frame = f;
    p.hello = false;
    p.ready = new Promise((resolve, reject) => {
      p.said = resolve;
      clearTimeout(p.timer);
      p.timer = setTimeout(() => { if (!p.hello) reject(new Error(t("{name}'s page didn't answer", { name: p.entry.title }))); }, HELLO_MS);
    });
    main.replaceChildren(f);
    return p.ready;
  }

  async function answer(p, m) {
    const e = p.entry;
    switch (m.method) {
      case "hello":
        p.hello = true;
        clearTimeout(p.timer);
        p.said?.();
        return theme();
      case "theme":
        return theme();
      case "options.get":
        return api("plugin-pages/options", { spec: e.spec });
      case "options.set": {
        const o = m.params?.options;
        if (!o || typeof o !== "object" || Array.isArray(o)) throw new Error(t("A page's options are an object"));
        return api("plugin-pages/options", { spec: e.spec, options: o });
      }
      case "read": {
        const name = String(m.params?.name || "");
        // what its package didn't declare is refused here, and again by
        // the server
        if (!e.reads.includes(name)) throw new Error(t("{name} doesn't ask to read {what}", { name: e.title, what: name }));
        return api("plugin-pages/read", { spec: e.spec, read: name, params: m.params?.params || {} });
      }
    }
    throw new Error(t("No such call: {what}", { what: String(m.method) }));
  }

  addEventListener("message", async (ev) => {
    // only a page's own frame is listened to: another window's message,
    // or one that names a page, is nothing
    let key = "", p = null;
    for (const [k, x] of pages) if (x.frame && ev.source === x.frame.contentWindow) { key = k; p = x; break; }
    if (!p) return;
    const m = ev.data;
    if (!m || typeof m !== "object" || m.magpie !== 1 || typeof m.method !== "string") return;
    if (m.method === "error") {
      broke(key, String(m.params?.message || t("The page hit an error")));
      return;
    }
    const src = ev.source;
    try {
      post(src, { id: m.id, result: await answer(p, m) });
    } catch (err) {
      post(src, { id: m.id, error: String(err?.message || err) });
    }
  });

  // the sidebar's tab for each page, in a Plugins group of its own after
  // the built-in pages
  function tab(e) {
    const b = document.createElement("button");
    b.dataset.view = e.key;
    b.dataset.pluginPage = "";
    if (e.icon) {
      const img = document.createElement("img");
      img.className = "nav-ic";
      img.src = e.icon;
      img.alt = "";
      img.width = img.height = 16;
      b.append(img);
    } else {
      const s = svg(PUZZLE, 16, 1.5);
      s.classList.add("nav-ic");
      s.setAttribute("aria-hidden", "true");
      b.append(s);
    }
    b.append(el("span", "", e.title));
    b.onclick = () => { show(e.key); b.blur(); };
    return b;
  }

  async function load() {
    let list;
    try { list = await api("plugin-pages"); } catch { return; }
    // an answer that isn't a list is not known: the tabs drawn stay
    if (!Array.isArray(list)) return;
    const nav = $("#nav");
    const keys = new Set(list.map((e) => e.key));
    // a page whose plugin went off, or away: its tab goes, and the window
    // leaves it if it was open
    for (const b of nav.querySelectorAll("button[data-plugin-page]")) if (!keys.has(b.dataset.view)) b.remove();
    for (const [k, p] of pages) {
      if (keys.has(k)) continue;
      p.frame?.remove();
      p.frame = null;
      p.gone = true;
      if (view === k) show("plugins");
    }
    let head = nav.querySelector(".nav-gl[data-plugin-pages]");
    if (!list.length) { head?.remove(); return; }
    if (!head) {
      head = el("span", "nav-gl", t("Plugins"));
      head.dataset.pluginPages = "";
      head.dataset.en = "Plugins";
      head.setAttribute("data-t", "");
      nav.append(head);
    }
    let at = head;
    for (const e of list) {
      let p = pages.get(e.key);
      if (!p) {
        p = { entry: e };
        pages.set(e.key, p);
        ctx.slots.add("page", { key: e.key, title: e.title, html: "", mount(main) { main.classList.add("plugin-page"); p.frame = null; }, open: () => open(e.key) });
      } else if (p.entry.url !== e.url || p.gone) {
        p.frame?.remove();
        p.frame = null;
      }
      p.gone = false;
      p.entry = e;
      const part = magpie.page(e.key);
      if (part) part.title = e.title;
      let b = nav.querySelector(`button[data-plugin-page][data-view="${e.key}"]`);
      if (!b) b = tab(e);
      b.classList.toggle("on", view === e.key);
      at.after(b);
      at = b;
    }
    // a reload of a plugin's page, or the one the window was left on,
    // comes back to it once its tab is there
    if (view === "agents" && !opened) {
      opened = true;
      let want = params.get("view") || "";
      if (!want) { try { want = localStorage.getItem("magpie.lastView") || ""; } catch { /* none kept */ } }
      if (pages.has(want)) show(want);
    }
  }
  let opened = false;
  window.loadPluginPages = load;
  load();
})();
