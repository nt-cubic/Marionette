// A plugin that leaves renewing its sign-ins to magpie (auth.refresh)
// against a vendor that spends a refresh token once: $RACE_TOKEN_URL
// exchanges it, and answers one sent again, or never given, with
// invalid_grant, which signs the account out. Requests go to $FAKE_BASE.
export const RacePlugin = async () => ({
  config: async (cfg) => {
    cfg.provider = cfg.provider ?? {}
    cfg.provider.raceco = {
      name: "RaceCo",
      npm: "@ai-sdk/openai-compatible",
      api: "https://race.invalid/v1",
      models: { "race-1": { name: "Race One", limit: { context: 1000, output: 100 } } },
    }
  },
  auth: {
    provider: "raceco",
    methods: [{ type: "api", label: "API key" }],
    async refresh(auth) {
      const res = await fetch(process.env.RACE_TOKEN_URL, { method: "POST", body: auth.refresh })
      const t = await res.json()
      if (t.error === "invalid_grant") throw Object.assign(new Error("RaceCo: invalid_grant"), { signIn: "expired" })
      if (!res.ok) throw new Error("RaceCo: " + res.status)
      return { access: t.access, refresh: t.refresh, expires: Date.now() + 3600e3 }
    },
    loader: async (getAuth) => ({
      baseURL: process.env.FAKE_BASE,
      async fetch(url, init) {
        const a = await getAuth()
        const h = new Headers(init.headers)
        h.set("authorization", "Bearer " + a.access)
        // the test this host runs for (testenv.FakeVendor)
        if (process.env.FAKE_RUN) h.set("x-fake-run", process.env.FAKE_RUN)
        return fetch(url, { ...init, headers: h })
      },
    }),
  },
})
