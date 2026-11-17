// Zero-dependency static server for the real-machine browser fixture pass
// (docs/research/ego-lite 06 §8). Node only — no install step, no framework.
//   node tools/serve-browser-fixtures.mjs [port]
// Defaults to 127.0.0.1:8899, loopback only. Every fixture is a static HTML
// file; nothing here is a test runner — the assertions live in the manual
// pass (docs/CURRENT.md browser section).
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "browser-fixtures");
const port = Number(process.argv[2] ?? 8899);
if (!Number.isInteger(port) || port < 1024 || port > 65535) {
  console.error("usage: node tools/serve-browser-fixtures.mjs [port 1024-65535]");
  process.exit(1);
}

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
};

createServer(async (req, res) => {
  try {
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    let path = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, "");
    if (path === "" || path.endsWith("/")) path += "index.html";
    const file = join(root, path);
    if (!file.startsWith(root)) {
      res.writeHead(403).end("forbidden");
      return;
    }
    const body = await readFile(file);
    res.writeHead(200, { "content-type": MIME[file.slice(file.lastIndexOf("."))] ?? "application/octet-stream" });
    res.end(body);
  } catch {
    res.writeHead(404).end("no such fixture");
  }
}).listen(port, "127.0.0.1", () => {
  console.log(`browser fixtures on http://127.0.0.1:${port}/basic-form.html`);
});
