// Insights › Masking: what masking kept from vendors, request by request.
//
// A page the redact plugin adds to the window (kernel.js), its words in
// i18n.js beside this file. This file registers the page and its markup,
// before app.js; page.js, after app.js, is the page's code. The card on
// top says how much was masked over 7 or 30 days and where it went, the
// list under it each request (or each value) masking changed, and a
// request opened says what it carried, what the vendor got instead, and
// how each value was found and put back. The hooks are what the shell
// asks of it: open, refresh (the window came back), reload (Refresh in
// the sidebar), leave, and focus (a masked call's badge opens its request).
magpie.plugin("redact", (ctx) => {
  ctx.slots.add("page", {
    key: "masking",
    title: "Masking",
    html: `
  <section class="msk-card" id="maskCard"></section>
  <section class="msk-list" id="maskList"></section>
  <section class="msk-one" id="maskOne" hidden></section>
`,
    open() { return maskPageOpen(); },
    refresh(again) { return maskPageLoad(again); },
    reload() { return maskPageLoad(true); },
    leave() { maskTipOff(); },
    focus(key) { maskOneOpen(String(key)); },
  });
});
