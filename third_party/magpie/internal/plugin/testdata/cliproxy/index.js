// A plugin that signs in with a CLI of the vendor's, as the Grok plugin
// runs `grok login`: the link it hands on says the proxy the CLI was
// given, started with the host's environment and with a copy of it, and
// whether Node's fetch was told to take it (NODE_USE_ENV_PROXY).
import { spawn } from "node:child_process"

function proxyOf(env, name = "HTTPS_PROXY") {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, ["-e", `process.stdout.write(process.env.${name} || 'none')`], {
      env,
      stdio: ["ignore", "pipe", "pipe"],
    })
    let out = ""
    child.stdout.on("data", (b) => (out += b))
    child.on("close", () => resolve(out))
  })
}

export const CLIProxyPlugin = async () => ({
  auth: {
    provider: "cliproxy",
    methods: [
      {
        type: "oauth",
        label: "CLI",
        authorize: async () => {
          const inherited = await proxyOf(undefined)
          const copied = await proxyOf({ ...process.env })
          const node = {
            inheritedNode: await proxyOf(undefined, "NODE_USE_ENV_PROXY"),
            copiedNode: await proxyOf({ ...process.env }, "NODE_USE_ENV_PROXY"),
            ownNode: await proxyOf({ ...process.env, NODE_USE_ENV_PROXY: "0" }, "NODE_USE_ENV_PROXY"),
          }
          const q = new URLSearchParams({ inherited, copied, ...node })
          return {
            url: `https://cliproxy.invalid/?${q}`,
            instructions: "",
            method: "code",
            callback: async () => ({ type: "failed" }),
          }
        },
      },
    ],
  },
})
