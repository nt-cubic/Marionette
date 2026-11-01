// A page as the model reads it: its accessibility tree as indented text,
// each element it can act on given a ref (e3) that stays the element's
// from one snapshot to the next, for as long as the page keeps it.

import { send } from "./cdp.js";

const INTERACTIVE = new Set([
  "button", "link", "textbox", "searchbox", "checkbox", "radio", "combobox",
  "listbox", "option", "menuitem", "menuitemcheckbox", "menuitemradio", "tab",
  "switch", "slider", "spinbutton", "treeitem", "textarea", "ComboBox",
  "PopUpButton", "MenuListPopup", "DisclosureTriangle", "columnheader",
]);
// wrappers that say nothing of their own: their children are shown in
// their place
const TRANSPARENT = new Set([
  "generic", "none", "presentation", "GenericContainer", "Section", "LayoutTable",
  "LayoutTableRow", "LayoutTableCell", "Div", "Span", "Pre", "Abbr", "Ruby",
  "LabelText", "group", "Canvas", "superscript", "subscript", "emphasis", "strong",
  "paragraph", "Paragraph", "time", "mark", "code",
]);
const SKIP = new Set(["InlineTextBox", "LineBreak", "ListMarker"]);
const NAMED_MAX = 200;
const TEXT_MAX = 500;
const STATES = ["checked", "selected", "expanded", "pressed", "disabled", "required", "invalid", "focused", "readonly"];

// elements a page made clickable with a listener of its own (a div with a
// pointer cursor) that the accessibility tree calls generic
const CLICKABLE = `(() => {
  const inter = 'a,button,input,select,textarea,summary,label,[role],[contenteditable=""],[contenteditable="true"]';
  const out = [];
  const all = document.querySelectorAll('body *');
  for (let i = 0; i < all.length && out.length < 300; i++) {
    const el = all[i];
    if (el.closest(inter)) continue;
    const cs = getComputedStyle(el);
    if (cs.cursor !== 'pointer' && !el.hasAttribute('onclick')) continue;
    const p = el.parentElement;
    if (p && p !== document.body && getComputedStyle(p).cursor === 'pointer') continue;
    const r = el.getBoundingClientRect();
    if (r.width < 4 || r.height < 4 || cs.visibility === 'hidden') continue;
    out.push(el);
  }
  return out;
})()`;

async function clickables(tabId) {
  const ids = new Set();
  try {
    const r = await send(tabId, "Runtime.evaluate", { expression: CLICKABLE, objectGroup: "magpie-snap" });
    if (!r.result || !r.result.objectId) return ids;
    const props = await send(tabId, "Runtime.getProperties", { objectId: r.result.objectId, ownProperties: true });
    await Promise.all(props.result.filter((p) => /^\d+$/.test(p.name) && p.value && p.value.objectId).map(async (p) => {
      try {
        const d = await send(tabId, "DOM.describeNode", { objectId: p.value.objectId });
        ids.add(d.node.backendNodeId);
      } catch {}
    }));
  } catch {} finally {
    send(tabId, "Runtime.releaseObjectGroup", { objectGroup: "magpie-snap" }).catch(() => {});
  }
  return ids;
}

// frames of the page's own process, read beside the main one; a frame
// from another site runs in a process of its own, which this tab's
// debugger doesn't reach
async function frames(tabId) {
  const out = new Map(); // owner element's backendNodeId → nodes
  let tree;
  try { tree = (await send(tabId, "Page.getFrameTree")).frameTree; } catch { return out; }
  const kids = [];
  (function walk(f) { for (const c of f.childFrames || []) { kids.push(c.frame); walk(c); } })(tree);
  await Promise.all(kids.slice(0, 20).map(async (f) => {
    try {
      const owner = await send(tabId, "DOM.getFrameOwner", { frameId: f.id });
      const { nodes } = await send(tabId, "Accessibility.getFullAXTree", { frameId: f.id });
      out.set(owner.backendNodeId, nodes);
    } catch {
      try {
        const owner = await send(tabId, "DOM.getFrameOwner", { frameId: f.id });
        out.set(owner.backendNodeId, null);
      } catch {}
    }
  }));
  return out;
}

const clip = (s, n) => (s.length > n ? s.slice(0, n - 1) + "…" : s);
const val = (v) => (v && v.value !== undefined && v.value !== null ? String(v.value) : "");
const q = (s) => JSON.stringify(s);

// take reads the page and gives each actionable element its ref.
export async function take(t, opts = {}) {
  const tabId = t.tabId;
  const [{ nodes }, clickable, inner, info] = await Promise.all([
    send(tabId, "Accessibility.getFullAXTree"),
    clickables(tabId),
    frames(tabId),
    send(tabId, "Runtime.evaluate", { expression: "[location.href, document.title]", returnByValue: true }).then((r) => r.result.value).catch(() => ["", ""]),
  ]);
  const [url, title] = info;
  // a new document starts its refs over; the same one keeps them
  const doc = url.split("#")[0];
  if (doc !== t.doc) {
    t.doc = doc;
    t.keys = new Map();
    t.next = 1;
  }
  const refs = new Map();
  const seen = new Map(); // role|name → how many so far, for the nth
  const lines = [];
  let interactive = 0;

  const index = (list) => {
    const m = new Map();
    for (const n of list) m.set(n.nodeId, n);
    return m;
  };

  const refFor = (role, name, backend) => {
    const k = role + "|" + name;
    const nth = seen.get(k) || 0;
    seen.set(k, nth + 1);
    const key = k + "|" + nth;
    let ref = t.keys.get(key);
    if (!ref || refs.has(ref)) {
      ref = "e" + t.next++;
      t.keys.set(key, ref);
    }
    refs.set(ref, { backend, role, name, key });
    return ref;
  };

  // text a node's subtree reads as, for an element that has no name
  const textOf = (byId, n, depth = 0) => {
    if (!n || depth > 12) return "";
    const r = val(n.role);
    if (r === "StaticText") return val(n.name);
    let s = "";
    for (const c of n.childIds || []) {
      const x = textOf(byId, byId.get(c), depth + 1);
      if (x) s += (s ? " " : "") + x;
      if (s.length > NAMED_MAX) break;
    }
    return s;
  };

  const walk = (byId, n, depth, named) => {
    if (!n || depth > 60) return;
    const role = val(n.role);
    const children = () => { for (const c of n.childIds || []) walk(byId, byId.get(c), depth, named); };
    if (SKIP.has(role)) return;
    if (n.ignored) {
      for (const c of n.childIds || []) walk(byId, byId.get(c), depth, named);
      return;
    }
    const name = clip(val(n.name).replace(/\s+/g, " ").trim(), NAMED_MAX);
    if (role === "StaticText") {
      const text = name;
      if (!text || (named && named.includes(text))) return;
      if (!opts.interactive) lines.push("  ".repeat(depth) + "- text: " + clip(text, TEXT_MAX));
      return;
    }
    if (role === "RootWebArea" || role === "WebArea") {
      for (const c of n.childIds || []) walk(byId, byId.get(c), depth, named);
      return;
    }
    const backend = n.backendDOMNodeId;
    const isClick = backend && clickable.has(backend);
    const props = {};
    for (const p of n.properties || []) props[p.name] = p.value && p.value.value;
    const acts = INTERACTIVE.has(role) || isClick || (props.focusable && props.editable);
    if (TRANSPARENT.has(role) && !name && !acts) {
      // an iframe's document is read under its element
      if (backend && inner.has(backend)) return frame(backend, depth);
      return children();
    }
    if (role === "Iframe" || role === "iframe") {
      lines.push("  ".repeat(depth) + "- iframe" + (name ? " " + q(name) : ""));
      if (backend && inner.has(backend)) frame(backend, depth + 1);
      return;
    }
    let shown = isClick && TRANSPARENT.has(role) ? "clickable" : role;
    let label = name;
    if (isClick && !label) label = clip(textOf(byId, n).replace(/\s+/g, " ").trim(), NAMED_MAX);
    let line = "- " + shown + (label ? " " + q(label) : "");
    const states = [];
    for (const s of STATES) {
      const v = props[s];
      if (v === true || v === "true" || (s === "checked" && v === "mixed")) states.push(s === "checked" && v === "mixed" ? "checked=mixed" : s);
    }
    if (props.level && role === "heading") states.unshift("level=" + props.level);
    if (states.length) line += " [" + states.join(", ") + "]";
    if (acts && backend && !props.disabled) {
      line += " [ref=" + refFor(shown, label, backend) + "]";
      interactive++;
    } else if ((role === "image" || role === "img") && backend) {
      line += " [ref=" + refFor(role, label, backend) + "]";
    }
    const v = val(n.value);
    if (v && v !== name) line += ": " + q(clip(v.replace(/\s+/g, " "), TEXT_MAX));
    if (!opts.interactive || line.includes("[ref=") || role === "heading") lines.push("  ".repeat(depth) + line);
    const under = label ? (named ? named + "\n" + label : label) : named;
    for (const c of n.childIds || []) walk(byId, byId.get(c), opts.interactive ? depth : depth + 1, under);
  };

  const frame = (backend, depth) => {
    const list = inner.get(backend);
    if (!list) {
      lines.push("  ".repeat(depth) + "- (a frame from another site: not readable here; use a screenshot, or open its address in a tab)");
      return;
    }
    const byId = index(list);
    walk(byId, list.find((n) => !n.parentId) || list[0], depth, "");
  };

  const byId = index(nodes);
  let root = nodes.find((n) => !n.parentId) || nodes[0];
  if (opts.ref) {
    const r = t.refs.get(opts.ref);
    const n = r && nodes.find((x) => x.backendDOMNodeId === r.backend);
    if (!n) throw new Error(`ref ${opts.ref} isn't on the page now: take a snapshot without ref`);
    root = n;
  }
  walk(byId, root, 0, "");
  // the old refs stay good for elements still on the page that this
  // snapshot didn't show (a scoped one)
  if (opts.ref) for (const [k, v] of refs) t.refs.set(k, v);
  else t.refs = refs;

  const max = opts.maxChars || 40000;
  let body = lines.join("\n");
  let cut = "";
  if (body.length > max) {
    const at = body.lastIndexOf("\n", max);
    body = body.slice(0, at > 0 ? at : max);
    cut = `\n… (cut at ${max} characters of ${lines.join("\n").length}: pass interactive: true for only what can be acted on, or ref to read one part)`;
  }
  return { url, title, text: body + cut, count: refs.size, interactive };
}

// find is the element a ref names: its node if still there, else the one
// element the page now has with the same role and name, else an error
// that says what the page has.
export async function find(t, ref) {
  const m = /^(?:ref=)?(e\d+)$/i.exec(String(ref || "").trim());
  if (!m) throw new Error(`"${ref}" isn't a ref: refs look like e12, from the latest snapshot`);
  ref = m[1].toLowerCase();
  const r = t.refs.get(ref);
  if (!r) throw new Error(`ref ${ref} isn't in the latest snapshot of this tab: take a snapshot and use a ref from it`);
  try {
    const o = await send(t.tabId, "DOM.resolveNode", { backendNodeId: r.backend, objectGroup: "magpie-act" });
    const c = await send(t.tabId, "Runtime.callFunctionOn", { objectId: o.object.objectId, functionDeclaration: "function(){return this.isConnected}", returnByValue: true });
    if (c.result.value) return { ...r, ref, objectId: o.object.objectId };
  } catch {}
  // the page drew it again: the same role and name, if only one has it
  await take(t, { maxChars: 1 });
  const same = [...t.refs].filter(([, v]) => v.role === r.role && v.name === r.name);
  if (same.length === 1) {
    const [nref, v] = same[0];
    const o = await send(t.tabId, "DOM.resolveNode", { backendNodeId: v.backend, objectGroup: "magpie-act" });
    return { ...v, ref: nref, objectId: o.object.objectId };
  }
  if (same.length > 1) throw new Error(`${ref} (${r.role} "${r.name}") was redrawn and ${same.length} elements now match it: take a snapshot and pick one`);
  throw new Error(`${ref} (${r.role} "${r.name}") is no longer on the page: take a snapshot`);
}
