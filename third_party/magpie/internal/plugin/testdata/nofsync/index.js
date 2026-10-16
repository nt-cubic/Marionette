// A plugin signing in with a key to a made-up provider, which makes the
// disk refuse to flush a file (fs.fsyncSync throws) while the file named
// by $FSYNC_FAIL is there: the host shares node:fs with it, so its writes
// of plugin-auth.json meet the same disk.
import fs from "node:fs"

const fsync = fs.fsyncSync
fs.fsyncSync = (fd) => {
  if (process.env.FSYNC_FAIL && fs.existsSync(process.env.FSYNC_FAIL)) throw Object.assign(new Error("EIO: i/o error, fsync"), { code: "EIO" })
  return fsync(fd)
}

export const NoFsyncPlugin = async () => ({
  config: async (cfg) => {
    cfg.provider = cfg.provider ?? {}
    cfg.provider.syncco = {
      name: "SyncCo",
      npm: "@ai-sdk/openai-compatible",
      api: "https://sync.invalid/v1",
      models: { "sync-1": { name: "Sync One", limit: { context: 1000, output: 100 } } },
    }
  },
  auth: {
    provider: "syncco",
    methods: [{ type: "api", label: "API key" }],
  },
})
