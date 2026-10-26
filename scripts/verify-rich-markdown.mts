/**
 * Rich-markdown pipeline smoke: feature scan, math delimiter rewrite, and the
 * ```spec schema (parse guards + limits).
 *
 * Run: npx tsx scripts/verify-rich-markdown.mts
 */
import assert from "node:assert/strict";
import { normalizeMathDelimiters, scanRichFeatures } from "../src/lib/richMarkdown.ts";
import { parseSpecSpec, SPEC_ACTION_EVENT } from "../src/lib/specSchema.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

check("plain prose has no rich features", () => {
  const f = scanRichFeatures("普通段落，`inline code` 不算围栏，$9.99 也只是美元");
  assert.deepEqual(f, { code: false, math: false, mermaid: false, spec: false });
});

check("single $ is never a math hint (singleDollarTextMath off)", () => {
  assert.equal(scanRichFeatures("run $HOME/bin and pay $5").math, false);
});

check("$$ and \\[ outside fences load math", () => {
  assert.equal(scanRichFeatures("公式：$$E=mc^2$$").math, true);
  assert.equal(scanRichFeatures("行尾出 \\[ x+y \\] 的情况").math, true);
});

check("$$ inside a fence is code, not math", () => {
  const text = ["```bash", "echo $$", "```"].join("\n");
  const f = scanRichFeatures(text);
  assert.equal(f.code, true);
  assert.equal(f.math, false);
});

check("tilde fences count too", () => {
  const f = scanRichFeatures("~~~ts\nconst x = 1;\n~~~");
  assert.equal(f.code, true);
});

check("info string routes mermaid/spec by first token", () => {
  assert.equal(scanRichFeatures("```mermaid theme=dark\ngraph TD\n```").mermaid, true);
  assert.equal(scanRichFeatures("```SPEC\n{}\n```").spec, true);
  assert.equal(scanRichFeatures("```mermaidjs\nx\n```").mermaid, false);
});

check("fence state machine: longer fence closes, inner backticks stay prose", () => {
  const text = "````md\n```mermaid\n````\nafter $$math";
  const f = scanRichFeatures(text);
  // The ```mermaid line is *inside* the ```` fence, so it is not a mermaid
  // fence; the $$ after the closing fence is live math again.
  assert.equal(f.mermaid, false);
  assert.equal(f.code, true);
  assert.equal(f.math, true);
});

check("normalizeMathDelimiters rewrites LaTeX display math", () => {
  // Padding inside the delimiters is preserved — `$$ x $$` is still display
  // math to remark-math, and trimming risks gluing CJK text onto the fence.
  assert.equal(normalizeMathDelimiters("前 \\[ E = mc^2 \\] 后"), "前 $$ E = mc^2 $$ 后");
});

check("normalizeMathDelimiters is a no-op without \\[", () => {
  const text = "$$x$$ and $$y$$";
  assert.equal(normalizeMathDelimiters(text), text);
});

check("normalizeMathDelimiters leaves fenced content untouched", () => {
  const text = ["```js", "const re = /\\[x\\]/;", "```", "\\[ a \\]"].join("\n");
  const out = normalizeMathDelimiters(text);
  assert.ok(out.includes("const re = /\\[x\\]/;"));
  // The trailing `\[ a \]` is a whole-line pair → display block form.
  assert.ok(out.includes("$$\na\n$$"));
});

check("normalizeMathDelimiters spans multiline bodies", () => {
  const text = "\\[\n\\int_0^1 x dx\n\\]";
  assert.equal(normalizeMathDelimiters(text), "$$\n\\int_0^1 x dx\n$$");
});

check("whole-line $$ pairs become display blocks", () => {
  assert.equal(normalizeMathDelimiters("$$E = mc^2$$"), "$$\nE = mc^2\n$$");
  // micromark flow math needs `$$` + end-of-line; the one-liner would render
  // inline without this rewrite.
});

check("prose-embedded $$ pairs stay inline", () => {
  assert.equal(normalizeMathDelimiters("前 $$E$$ 后"), "前 $$E$$ 后");
});

check("display-block rewrite leaves fences and bare $$ alone", () => {
  const text = ["```md", "$$x$$", "```", "$$", "x", "$$"].join("\n");
  assert.equal(normalizeMathDelimiters(text), text);
  // Already-fenced `$$` lines are not `$$ … $$` pairs and must not be touched.
});

check("spec action event name stays stable (App listens on it)", () => {
  assert.equal(SPEC_ACTION_EVENT, "marionette:spec-action");
});

check("spec: valid card parses with actions and hints", () => {
  const r = parseSpecSpec(
    JSON.stringify({
      type: "card",
      title: "部署摘要",
      fields: [
        { name: "环境", value: "prod", hint: "us-east-1" },
        { name: "版本", value: "v1.2.3" },
      ],
      body: "两个服务已更新",
      actions: [{ label: "回滚", prompt: "请回滚 prod 到 v1.2.2" }],
    }),
  );
  assert.equal(r.ok, true);
  if (r.ok) {
    assert.equal(r.spec.type, "card");
    assert.equal(r.spec.fields.length, 2);
    assert.equal(r.spec.actions?.[0]?.label, "回滚");
  }
});

check("spec: table with tone cells parses", () => {
  const r = parseSpecSpec(
    JSON.stringify({
      type: "table",
      columns: ["检查", "结果"],
      rows: [
        ["构建", { text: "通过", tone: "ok" }],
        ["测试", { text: "跳过", tone: "dim" }],
      ],
    }),
  );
  assert.equal(r.ok, true);
  if (r.ok && r.spec.type === "table") {
    assert.deepEqual(r.spec.rows[0]?.[1], { text: "通过", tone: "ok" });
  }
});

check("spec: steps and charts parse", () => {
  const steps = parseSpecSpec(
    JSON.stringify({ type: "steps", items: [{ label: "拉取", state: "done" }, { label: "构建", state: "active" }] }),
  );
  assert.equal(steps.ok, true);
  const chart = parseSpecSpec(
    JSON.stringify({
      type: "chart",
      chart: "pie",
      series: [{ name: "占比", points: [{ label: "A", value: 3 }, { label: "B", value: 1 }] }],
    }),
  );
  assert.equal(chart.ok, true);
});

check("spec: malformed / streaming JSON fails quietly", () => {
  const r = parseSpecSpec('{"type":"card","fields":[{"name":"a","value":"b"');
  assert.equal(r.ok, false);
  if (!r.ok) assert.match(r.error, /JSON/);
});

check("spec: prototype pollution keys are rejected", () => {
  const r = parseSpecSpec('{"type":"card","__proto__":{"x":1},"fields":[]}');
  assert.equal(r.ok, false);
  if (!r.ok) assert.match(r.error, /原型污染/);
});

check("spec: deep nesting is refused", () => {
  let deep = "1";
  for (let i = 0; i < 80; i++) deep = `[${deep}]`;
  const r = parseSpecSpec(deep);
  assert.equal(r.ok, false);
});

check("spec: shape errors — wrong type, row width, tone, state, value", () => {
  assert.equal(parseSpecSpec('{"type":"poster"}').ok, false);
  const width = parseSpecSpec(
    JSON.stringify({ type: "table", columns: ["a", "b"], rows: [["only-one"]] }),
  );
  assert.equal(width.ok, false);
  const tone = parseSpecSpec(
    JSON.stringify({ type: "table", columns: ["a"], rows: [[{ text: "x", tone: "purple" }]] }),
  );
  assert.equal(tone.ok, false);
  const state = parseSpecSpec(JSON.stringify({ type: "steps", items: [{ label: "a", state: "paused" }] }));
  assert.equal(state.ok, false);
  const value = parseSpecSpec(
    JSON.stringify({ type: "chart", chart: "bar", series: [{ points: [{ label: "a", value: "3" }] }] }),
  );
  assert.equal(value.ok, false);
});

check("spec: raw size cap refuses oversized fences", () => {
  const huge = JSON.stringify({ type: "card", fields: [{ name: "x", value: "y".repeat(200000) }] });
  assert.ok(huge.length > 131072);
  const r = parseSpecSpec(huge);
  assert.equal(r.ok, false);
});

check("spec: action labels require prompts and non-empty strings", () => {
  const missing = parseSpecSpec(
    JSON.stringify({ type: "card", fields: [{ name: "a", value: "b" }], actions: [{ label: "go" }] }),
  );
  assert.equal(missing.ok, false);
  const empty = parseSpecSpec(JSON.stringify({ type: "card", fields: [{ name: "", value: "b" }] }));
  assert.equal(empty.ok, false);
});

console.log(`\nverify-rich-markdown: ${passed} checks passed.`);
