import {
  DEFAULT_DEV_SERVER_PREFERENCE,
  normalizeLoopbackService,
  readDevServerPreference,
  saveDevServerPreference,
  scanDevServerUrls,
  stripTerminalAnsi,
} from "../src/lib/devServer";

const preferenceStore = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: {
    getItem: (key: string) => preferenceStore.get(key) ?? null,
    setItem: (key: string, value: string) => preferenceStore.set(key, value),
  },
});

function ok(condition: unknown, label: string) {
  if (!condition) throw new Error(label);
  console.log(`  ok  ${label}`);
}

ok(
  normalizeLoopbackService("http://localhost:5173/")?.origin === "http://localhost:5173",
  "localhost service is accepted",
);
ok(
  normalizeLoopbackService("http://0.0.0.0:8000/")?.url === "http://localhost:8000/",
  "wildcard listener maps to localhost",
);
ok(normalizeLoopbackService("http://192.168.1.2:3000") === null, "LAN address is ignored");
ok(normalizeLoopbackService("https://example.com") === null, "public site is ignored");
ok(
  normalizeLoopbackService("http://localhost:3000/. ")?.url === "http://localhost:3000/",
  "prose punctuation is trimmed",
);

const ansi = "\u001b[32mLocal:\u001b[0m http://localhost:5173/\n";
ok(!stripTerminalAnsi(ansi).includes("\u001b"), "ANSI is stripped");
ok(
  scanDevServerUrls(ansi, "t1", "s1")[0]?.origin === "http://localhost:5173",
  "terminal banner produces a candidate",
);
ok(
  scanDevServerUrls(
    "http://localhost:3000 and http://localhost:3000/\n",
    "t1",
    "s1",
  ).length === 1,
  "same origin appears once",
);
ok(
  scanDevServerUrls(
    "api http://localhost:4000/ web http://localhost:3000/\n",
    "t1",
    "s1",
  ).length === 2,
  "two local services are distinct",
);
ok(DEFAULT_DEV_SERVER_PREFERENCE === "notify", "fresh installs notify instead of opening automatically");
ok(readDevServerPreference() === "notify", "missing preference key stays passive");
saveDevServerPreference("open");
ok(readDevServerPreference() === "open", "explicit auto-open opt-in survives");
saveDevServerPreference("off");
ok(readDevServerPreference() === "off", "explicit ignore preference survives");
preferenceStore.set("marionette-dev-server-preview", "invalid");
ok(readDevServerPreference() === "notify", "invalid preference falls back to passive notify");

console.log("\nall checks passed");
