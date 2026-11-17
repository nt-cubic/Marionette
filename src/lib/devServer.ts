export type DevServerPreference = "off" | "notify" | "open";

export type DevServerCandidate = {
  url: string;
  origin: string;
  terminalId: string;
  sessionId: string;
};

const PREF_KEY = "marionette-dev-server-preview";
const URL_RE = /https?:\/\/[^\s<>"'`]+/giu;

export function stripTerminalAnsi(text: string): string {
  return text
    .replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, "")
    .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
    .replace(/\x1b[()][A-Za-z0-9]/g, "");
}

function trimPunctuation(raw: string): string {
  let out = raw;
  while (out.length > 0) {
    const last = out[out.length - 1]!;
    if (".,;:!?'\"".includes(last)) {
      out = out.slice(0, -1);
      continue;
    }
    if (last === ")" && (out.match(/\)/g)?.length ?? 0) > (out.match(/\(/g)?.length ?? 0)) {
      out = out.slice(0, -1);
      continue;
    }
    if (last === "]" && (out.match(/\]/g)?.length ?? 0) > (out.match(/\[/g)?.length ?? 0)) {
      out = out.slice(0, -1);
      continue;
    }
    if (last === "}" && (out.match(/}/g)?.length ?? 0) > (out.match(/{/g)?.length ?? 0)) {
      out = out.slice(0, -1);
      continue;
    }
    break;
  }
  return out;
}

export function normalizeLoopbackService(raw: string): { url: string; origin: string } | null {
  const candidate = trimPunctuation(raw.trim());
  try {
    const url = new URL(candidate);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    const host = url.hostname.toLowerCase();
    const loopback =
      host === "localhost" ||
      host.endsWith(".localhost") ||
      host === "127.0.0.1" ||
      host === "[::1]" ||
      host === "::1";
    const wildcard = host === "0.0.0.0" || host === "[::]" || host === "::";
    if (!loopback && !wildcard) return null;
    if (wildcard) url.hostname = "localhost";
    url.hash = "";
    return { url: url.toString(), origin: url.origin };
  } catch {
    return null;
  }
}

export function scanDevServerUrls(
  output: string,
  terminalId: string,
  sessionId: string,
): DevServerCandidate[] {
  const text = stripTerminalAnsi(output);
  const seen = new Set<string>();
  const out: DevServerCandidate[] = [];
  for (const match of text.matchAll(URL_RE)) {
    const normalized = normalizeLoopbackService(match[0]);
    if (!normalized || seen.has(normalized.origin)) continue;
    seen.add(normalized.origin);
    out.push({ ...normalized, terminalId, sessionId });
  }
  return out;
}

export function readDevServerPreference(): DevServerPreference {
  try {
    const value = localStorage.getItem(PREF_KEY);
    return value === "off" || value === "open" || value === "notify" ? value : "notify";
  } catch {
    return "notify";
  }
}

export function saveDevServerPreference(preference: DevServerPreference): void {
  try {
    localStorage.setItem(PREF_KEY, preference);
  } catch {
    // localStorage can be disabled; the in-memory selection still applies.
  }
}
