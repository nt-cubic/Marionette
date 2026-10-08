/**
 * Markdown-image path smoke test.
 *
 * Run: npx tsx scripts/image-path-smoke.mts
 *
 * A reply names its screenshots the way the agent saw them — relative to the
 * session cwd. The webview can only load an absolute path, so this is the
 * join that decides whether a reply's images show up at all. Paths below are
 * the real ones from a Unity project's transcript.
 */
import assert from "node:assert/strict";
import { resolveToolImagePath } from "../src/lib/toolBody.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const CWD = "D:\\Projects\\Jingzhe\\Unity_Jingzhe";
const RELATIVE = "Docs/关卡系统/迷宫HUD_v21移植/实施记录/shots/21-3.1_rest.png";
const ABSOLUTE = CWD + "\\" + RELATIVE.replace(/\//g, "\\");

console.log("relative paths join the session cwd");
check("a relative path resolves against a Windows cwd", () => {
  assert.equal(resolveToolImagePath(RELATIVE, CWD), ABSOLUTE);
});
check("an absolute Windows path is left alone", () => {
  assert.equal(resolveToolImagePath(ABSOLUTE, CWD), ABSOLUTE);
});
check("a file:// absolute path loses the scheme and keeps its slashes", () => {
  const url = `file:///${ABSOLUTE.replace(/\\/g, "/")}`;
  assert.equal(resolveToolImagePath(url, CWD), ABSOLUTE.replace(/\\/g, "/"));
});
check("an UNC path is left alone", () => {
  const unc = "\\\\nas\\shots\\a.png";
  assert.equal(resolveToolImagePath(unc, CWD), unc);
});
check("backslashes and forward slashes both land on the cwd separator", () => {
  assert.equal(resolveToolImagePath(RELATIVE.replace(/\//g, "\\"), CWD), ABSOLUTE);
});
check("a leading ./ does not survive", () => {
  assert.equal(resolveToolImagePath(`./${RELATIVE}`, CWD), ABSOLUTE);
});
check("a cwd with a trailing separator does not double it", () => {
  assert.equal(resolveToolImagePath(RELATIVE, `${CWD}\\`), ABSOLUTE);
});
check("with no cwd the path is returned untouched", () => {
  assert.equal(resolveToolImagePath(RELATIVE, null), RELATIVE);
  assert.equal(resolveToolImagePath(RELATIVE, ""), RELATIVE);
});
check("a POSIX cwd keeps forward slashes", () => {
  assert.equal(
    resolveToolImagePath("shots/a.png", "/home/me/project"),
    "/home/me/project/shots/a.png",
  );
});

console.log(`\n${passed} checks passed.`);
