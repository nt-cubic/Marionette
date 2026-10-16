// A plugin whose OAuth method asks nothing and tells OpenCode's CLI from
// its TUI by the inputs, as opencode-antigravity-auth does: given an object
// (opencode auth login) it asks its questions on the terminal and answers
// only once signed in; given none (/connect) it gives the page to open.
export const CLIFlowPlugin = async () => ({
  auth: {
    provider: "cliflow",
    methods: [
      {
        type: "oauth",
        label: "OAuth",
        authorize: async (inputs) => {
          if (inputs) {
            process.stderr.write("Project ID (leave blank to use your default project): ")
            await new Promise(() => {}) // the terminal's answer, never coming
          }
          return {
            url: "https://cliflow.invalid/auth",
            instructions: "Paste the code",
            method: "code",
            callback: async (code) => (code === "good" ? { type: "success", refresh: "r", access: "a", expires: 0, accountId: "me@cliflow" } : { type: "failed" }),
          }
        },
      },
    ],
  },
})
