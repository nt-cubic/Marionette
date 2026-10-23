// Exercise generic-provider CLI registration against an isolated copy of the
// installed engine and a local fixture server. Uses only dummy credentials.
import assert from "node:assert/strict";
import { execFile as run } from "node:child_process";
import { promisify } from "node:util";
import { createServer } from "node:http";
import { copyFile, mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir, homedir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
const execFile = promisify(run);
const source = process.argv[2] ?? join(homedir(), ".marionette", "engine", "magpie.exe");
const scratch = await mkdtemp(join(tmpdir(), "marionette-source-cli-"));
const server = createServer((request, response) => {
  assert.match(request.headers.authorization ?? "", /^Bearer fixture-key-/);
  response.setHeader("Content-Type", "application/json");
  response.end(JSON.stringify({ data: [{ id: "deepseek-v4.1-flash:1017" }, { id: "other-model" }] }));
});
try {
  const exe = join(scratch, "magpie.exe");
  await copyFile(source, exe);
  await writeFile(join(scratch, ".portable"), "");
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  const env = { ...process.env, MAGPIE_NO_ACCOUNTS: "1", MAGPIE_NO_STATS: "1", NO_COLOR: "1" };
  const call = async args => (await execFile(exe, args, { cwd: scratch, env, windowsHide: true, timeout: 30_000 })).stdout;
  const id = "source-fixture-account";
  await call(["provider", "add", id, `id=${id}`, "name=Ollama fixture", `url=http://127.0.0.1:${port}/v1`, "key=fixture-key-first", "models=deepseek-v4.1-flash:1017"]);
  let models = await call(["models"]);
  assert.ok(models.includes(`${id}/deepseek-v4.1-flash:1017`), "configured model is advertised including its tag");
  await call(["provider", "set", id, `url=http://127.0.0.1:${port}/v1`, "key=fixture-key-rotated", "models=other-model"]);
  models = await call(["models"]);
  assert.ok(models.includes(`${id}/other-model`));
  assert.ok(!models.includes(`${id}/deepseek-v4.1-flash:1017`), "changing picks removes stale models");
  assert.ok(!models.includes(`${id}-2/`), "updates never create duplicate providers");
  await call(["provider", "rm", id]);
  assert.ok(!(await call(["models"])).includes(`${id}/`));
  console.log("Engine source CLI: add, tagged model, key rotation, picks update and removal passed in isolated storage.");
} finally {
  server.close();
  // Verify the final absolute recursive-delete target stays in the test's
  // explicitly chosen temp root and retains the generated fixture prefix.
  const target = resolve(scratch);
  assert.equal(dirname(target).toLowerCase(), resolve(tmpdir()).toLowerCase());
  assert.ok(basename(target).startsWith("marionette-source-cli-"));
  await rm(target, { recursive: true, force: true });
}
