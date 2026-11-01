import { GATEWAY } from "./config.js";

const zh = /^zh/i.test(navigator.language);
const say = zh
  ? { on: "已连接 Magpie", off: "未连接 Magpie", onHint: "通过 Magpie 运行的智能体可以使用这个浏览器里的标签页。它们打开的标签页放在 “Magpie” 分组里。", offHint: "请确认 Magpie 正在运行。扩展会自动重连。" }
  : { on: "Connected to Magpie", off: "Not connected to Magpie", onHint: "Agents you run through Magpie can use this browser's tabs. Tabs they open go in the \"Magpie\" group.", offHint: "Check that Magpie is running. The extension reconnects by itself." };

async function show() {
  const { status } = await chrome.storage.session.get("status");
  const on = !!(status && status.connected);
  document.getElementById("dot").classList.toggle("on", on);
  document.getElementById("state").textContent = on ? say.on : say.off;
  document.getElementById("hint").textContent = on ? say.onHint : say.offHint;
}

const gw = document.getElementById("gw");
chrome.storage.local.get("gateway").then(({ gateway }) => { gw.value = gateway || GATEWAY; });
document.getElementById("save").addEventListener("click", async () => {
  const v = gw.value.trim();
  if (!v || v === GATEWAY) await chrome.storage.local.remove("gateway");
  else await chrome.storage.local.set({ gateway: v.replace(/^http/, "ws") });
  chrome.runtime.sendMessage({ type: "reconnect" }).catch(() => {});
});
chrome.storage.onChanged.addListener((c, area) => { if (area === "session" && c.status) show(); });
show();
