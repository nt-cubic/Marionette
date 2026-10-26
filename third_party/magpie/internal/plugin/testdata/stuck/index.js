// A plugin whose vendor never lets it through, as one whose sign-in is
// answered 401 and tried again for ever: with $STUCK_LOAD set its plugin
// function never returns, and with $STUCK_MODELS set its models hook
// never does.
const never = () => new Promise(() => {})

export const StuckPlugin = async () => {
  if (process.env.STUCK_LOAD) await never()
  return {
    config: async (cfg) => {
      cfg.provider = cfg.provider ?? {}
      cfg.provider.stuckco = {
        name: "StuckCo",
        npm: "@ai-sdk/openai-compatible",
        api: "https://stuck.invalid/v1",
        models: { "stuck-1": { name: "Stuck One", limit: { context: 1000, output: 100 } } },
      }
    },
    provider: {
      id: "stuckco",
      async models(provider) {
        if (process.env.STUCK_MODELS) await never()
        return provider.models
      },
    },
    auth: {
      provider: "stuckco",
      methods: [{ type: "api", label: "API key" }],
      loader: async () => ({ baseURL: "https://stuck.invalid/v1" }),
    },
  }
}
