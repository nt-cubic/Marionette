// Settings › Privacy's Instead of masking, send to (the redact plugin's part
// of Settings). Run after app.js, whose helpers it shares (el, icon,
// openPicker, modelOpt, prefWrite, api), and before start.js draws
// Settings. It wraps renderRedact, so app.js carries none of it: the row
// shows while anything is masked, after the masked-words rows.

// redactRouteRow: where a request masking would change goes instead
// (redactRouteTo, jessyoungyu on X): Off masks it as before; a model whose
// provider the user set to Send requests unmasked (maskRouteModels, on
// this computer or the network) gets it as written, so 「百度这家公司怎么样」
// is answered about Baidu, not a placeholder. Picked in the app's menu,
// saved on its own (settings/redact-route).
function redactRouteRow(s) {
  const models = s.maskRouteModels || [];
  const v = s.redactRouteTo || "";
  const b = el("button", "rt-cond on");
  b.type = "button";
  b.setAttribute("aria-label", t("Model for requests masking would change"));
  const m = models.find((x) => x.id === v);
  if (v) b.append(icon(m?.icon || "generic"), el("span", "", m ? `${m.name || m.id} · ${m.providerName}` : v));
  else b.append(el("span", "", t("Off")));
  b.disabled = !models.length && !v;
  b.onclick = (ev) => openPicker({ id: "", name: "", fields: [] }, { key: "redactRouteTo", label: "model", value: v, options: [
    { value: "", label: t("Off"), note: t("Masked and sent to the model asked for, as before"), icon: "generic", reset: true },
    ...models.map(modelOpt)],
  onPick: (id) => {
    if (id === v) return;
    prefWrite(api("settings/redact-route", { model: id }));
  } }, b, ev);
  const sub = models.length || v
    ? t("A request masking would change goes as written to this model, which you set to take requests unmasked, so it answers about the real names. If it fails, the request fails: it is never masked and sent elsewhere")
    : t("Only a model you set to take requests unmasked can be picked: in the editor of a provider on this computer or your network, turn on Send requests unmasked");
  const r = el("div", "row pref"), who = el("div", "who"), val = el("div", "val");
  who.append(el("div", "name", t("Instead of masking, send to")), el("div", "sub", sub));
  val.append(b);
  r.append(who, val);
  r.id = "redactRouteRow";
  const after = $("#redactWordsWholeRow");
  if (after) after.after(r); else $("#redactList").append(r);
}
{
  const draw = renderRedact;
  renderRedact = function (s, keep) {
    draw(s, keep);
    if (s.redact || s.redactPersonal || (s.redactWords || []).length) redactRouteRow(s);
  };
}
