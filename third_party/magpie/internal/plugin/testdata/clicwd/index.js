// A plugin that starts programs as the chat page plugin starts the user's
// MCP servers: with no folder of its own (node:child_process spawn and
// execSync, Bun.spawn), and once with a folder it chose. The link it hands
// on says the folder each one ran in, and the PWD it was given.
import { execSync, spawn } from "node:child_process"
import os from "node:os"

const script = "process.stdout.write(process.cwd() + '\\n' + (process.env.PWD || ''))"

function where(opts) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, ["-e", script], { stdio: ["ignore", "pipe", "pipe"], ...opts })
    let out = ""
    child.stdout.on("data", (b) => (out += b))
    child.on("close", () => resolve(out))
  })
}

export const CLICwdPlugin = async () => ({
  auth: {
    provider: "clicwd",
    methods: [
      {
        type: "oauth",
        label: "CLI",
        authorize: async () => {
          const bun = Bun.spawn([process.execPath, "-e", script], { stdout: "pipe", stderr: "pipe" })
          const q = new URLSearchParams({
            spawned: await where({}),
            withEnv: await where({ env: { ...process.env } }),
            sync: execSync(`"${process.execPath}" -e "${script.replace(/"/g, '\\"')}"`, { stdio: ["ignore", "pipe", "pipe"] }).toString(),
            bun: await new Response(bun.stdout).text(),
            own: await where({ cwd: os.tmpdir() }),
            ownTarget: os.tmpdir(),
          })
          return {
            url: `https://clicwd.invalid/?${q}`,
            instructions: "",
            method: "code",
            callback: async () => ({ type: "failed" }),
          }
        },
      },
    ],
  },
})
