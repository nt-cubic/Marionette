// Installed font choices reach both webviews before their first paint.
// Keep --font and --code as the platform/theme fallbacks: an explicit
// choice is a separate variable, including on Omarchy's body.
window.desktopFonts = (() => {
  let catalogue = null;
  const same = (a, b) => !!a && !!b && ["family", "name", "weight", "style", "stretch"].every((k) => a[k] === b[k]);
  const available = (face) => !face || catalogue === null || catalogue.some((f) => f.styles.some((s) => same(s, face)));
  const quoted = (s) => '"' + s.replace(/[\\"\x00-\x1f\x7f]/g, (c) => "\\" + c.charCodeAt(0).toString(16) + " ") + '"';
  function apply(s) {
    const root = document.documentElement;
    let active = false;
    for (const [key, prefix, fallback] of [["uiFont", "ui", "font"], ["codeFont", "code", "code"]]) {
      const f = !window.bootPrefs?.web && s?.[key];
      const valid = f && typeof f.family === "string" && f.family && ["normal", "italic", "oblique"].includes(f.style)
        && Number.isFinite(f.weight) && f.weight >= 1 && f.weight <= 1000 && Number.isFinite(f.stretch) && f.stretch >= 0;
      const values = valid && available(f) ? {
        font: quoted(f.family) + ", var(--" + fallback + ")", weight: f.weight, style: f.style, stretch: f.stretch + "%",
      } : {};
      if (values.font) active = true;
      for (const part of ["font", "weight", "style", "stretch"]) {
        const name = "--" + prefix + "-" + part;
        if (values[part] !== undefined) root.style.setProperty(name, values[part]);
        else root.style.removeProperty(name);
      }
    }
    if (active) root.setAttribute("data-fonts", "");
    else root.removeAttribute("data-fonts");
    // a wider font widens the tabs too: the header is fitted again (kiwiflysky on X)
    window.fitTop?.();
  }
  return { apply, available, same, catalogue: (list) => { catalogue = list; } };
})();
window.desktopFonts.apply(window.bootPrefs);
