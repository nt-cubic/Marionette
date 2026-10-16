// A plugin giving tools (OpenCode's tool hook), which magpie offers every
// agent through its plugin tools MCP server. A real plugin writes
//
//   import { tool } from "@opencode-ai/plugin"
//   tool({ description, args: { text: tool.schema.string().describe("…") }, execute })
//
// tool.schema is zod. With no node_modules here, z below is the slice of
// zod 4 the host reads: each schema's _zod.def, its description, and
// safeParse.
const ok = (data) => ({ success: true, data })
const no = (message) => ({ success: false, error: { issues: [{ path: [], message }] } })
const base = (def, check, extra = {}) => {
  const s = {
    _zod: { def },
    ...extra,
    safeParse: (v) => check(v),
    describe(d) { return { ...this, description: d } },
    optional() { return base({ type: "optional", innerType: this }, (v) => (v === undefined ? ok(undefined) : this.safeParse(v)), { _zod: { def: { type: "optional", innerType: this }, optin: "optional" } }) },
    default(x) { return base({ type: "default", innerType: this, defaultValue: x }, (v) => (v === undefined ? ok(x) : this.safeParse(v)), { _zod: { def: { type: "default", innerType: this, defaultValue: x }, optin: "optional" } }) },
  }
  return s
}
const z = {
  string: () => base({ type: "string" }, (v) => (typeof v === "string" ? ok(v) : no("expected string"))),
  number: () => base({ type: "number" }, (v) => (typeof v === "number" ? ok(v) : no("expected number"))),
  boolean: () => base({ type: "boolean" }, (v) => (typeof v === "boolean" ? ok(v) : no("expected boolean"))),
  enum: (vals) => base({ type: "enum", entries: Object.fromEntries(vals.map((v) => [v, v])) }, (v) => (vals.includes(v) ? ok(v) : no("expected one of " + vals.join(", ")))),
  array: (el) => base({ type: "array", element: el }, (v) => (Array.isArray(v) ? ok(v) : no("expected array"))),
}

export const ToolsExample = async () => ({
  tool: {
    // zod arguments: checked, defaults filled in, before execute
    shout: {
      description: "Says the text back in capitals.",
      args: {
        text: z.string().describe("What to say"),
        times: z.number().default(1).describe("How many times"),
        style: z.enum(["plain", "bang"]).optional(),
      },
      execute: async (args, ctx) => {
        const one = args.text.toUpperCase() + (args.style === "bang" ? "!" : "")
        return Array(args.times).fill(one).join(" ") + ` (for ${ctx.agent || "nobody"})`
      },
    },
    // a JSON Schema, and an answer that is no string
    add: {
      description: "Adds numbers.",
      args: { type: "object", properties: { xs: { type: "array", items: { type: "number" } } }, required: ["xs"] },
      execute: async ({ xs }) => ({ sum: xs.reduce((a, b) => a + b, 0) }),
    },
    // one that runs past its time
    sleep: {
      description: "Sleeps for ms.",
      args: { ms: z.number() },
      execute: (args, ctx) => new Promise((res) => {
        const t = setTimeout(() => res("woke"), args.ms)
        ctx.abort.addEventListener("abort", () => clearTimeout(t))
      }),
    },
    fail: { description: "Fails.", args: {}, execute: async () => { throw new Error("it broke") } },
    // not tools: a name an MCP client can't take, and no execute
    "bad name!": { description: "x", args: {}, execute: async () => "x" },
    noexec: { description: "x", args: {} },
  },
})
