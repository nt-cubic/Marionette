import WebSocket from "ws";

const list = await (await fetch("http://127.0.0.1:9222/json")).json();
const page = list.find((t) => t.url?.includes("5173")) || list.find((t) => t.type === "page");
if (!page?.webSocketDebuggerUrl) {
  console.error("No CDP page found", list.map((t) => t.url));
  process.exit(1);
}

const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const mid = ++id;
    const onMsg = (data) => {
      const msg = JSON.parse(data.toString());
      if (msg.id === mid) {
        ws.off("message", onMsg);
        resolve(msg);
      }
    };
    ws.on("message", onMsg);
    ws.send(JSON.stringify({ id: mid, method, params }));
    setTimeout(() => reject(new Error(`timeout ${method}`)), 8000);
  });

const logs = [];
ws.on("message", (data) => {
  const msg = JSON.parse(data.toString());
  if (msg.method === "Runtime.consoleAPICalled") {
    const text = (msg.params.args || [])
      .map((a) => a.value ?? a.description ?? JSON.stringify(a))
      .join(" ");
    logs.push(`${msg.params.type}: ${text}`);
  }
  if (msg.method === "Runtime.exceptionThrown") {
    const d = msg.params.exceptionDetails;
    logs.push(`EXCEPTION: ${d?.exception?.description || d?.text || JSON.stringify(d)}`);
  }
});

await new Promise((resolve, reject) => {
  ws.once("open", resolve);
  ws.once("error", reject);
});

await send("Runtime.enable");
await send("Page.enable");
await send("Network.enable");
await send("Page.reload", { ignoreCache: true });
await new Promise((r) => setTimeout(r, 5000));

const eval2 = await send("Runtime.evaluate", {
  expression: `({
    html: (document.getElementById("root")?.innerHTML || "").slice(0, 2000),
    text: (document.body?.innerText || "").slice(0, 1000),
    childCount: document.getElementById("root")?.childElementCount ?? -1,
  })`,
  returnByValue: true,
});

console.log("STATE:", JSON.stringify(eval2.result?.result?.value, null, 2));
console.log("LOGS:\n" + logs.slice(-40).join("\n"));
ws.close();
process.exit(0);
