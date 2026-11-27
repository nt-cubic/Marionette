/**
 * P1-9 - launch handoff adapter.
 *
 * The current single-instance handoff writes a plain open-path string to
 * `pending-open-path.txt` and emits the same string on `marionette-open-path`.
 * P1-9 introduces a schema-versioned JSON request without breaking those
 * payloads:
 *
 *   { "v": 1, "kind": "focus" }
 *   { "v": 1, "kind": "open-path", "path": "D:\\work" }
 *   { "v": 1, "kind": "prompt-link", "url": "marionette://new?v=1&prompt=..." }
 *
 * `parseLaunchHandoffPayload` accepts all of the following:
 * - the two current payloads (empty string, a bare path),
 * - a prompt-link URL directly,
 * - the versioned JSON above (as a string or an already-parsed object).
 *
 * It never throws and never treats a malformed prompt link as a filesystem
 * path. The result is data only: opening/prefilling stays with the caller.
 */

import {
  isPromptLinkUrl,
  parsePromptLink,
  serializePromptLink,
  type PromptLink,
  type PromptLinkInput,
  type PromptLinkReport,
} from "./promptLink";

export const LAUNCH_REQUEST_VERSION = 1;

export type LaunchRequestV1 =
  | { v: typeof LAUNCH_REQUEST_VERSION; kind: "focus" }
  | { v: typeof LAUNCH_REQUEST_VERSION; kind: "open-path"; path: string }
  | { v: typeof LAUNCH_REQUEST_VERSION; kind: "prompt-link"; url: string };

export type LaunchHandoff =
  | { kind: "none" }
  | { kind: "focus" }
  | { kind: "open-path"; path: string }
  | {
      kind: "prompt-link";
      link: PromptLink;
      report: PromptLinkReport;
      /** Canonical sanitized wire URL when one is available. */
      url?: string;
    }
  | { kind: "invalid"; reason: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function pathHandoff(raw: unknown): LaunchHandoff {
  if (typeof raw !== "string") {
    return { kind: "invalid", reason: "path-not-a-string" };
  }
  const path = raw.trim();
  if (!path) return { kind: "invalid", reason: "empty-path" };
  // A filesystem path should not carry terminal/control bytes. Reject rather
  // than silently mangle a path the user may not expect to change.
  if (/[\u0000-\u001f\u007f-\u009f]/.test(path)) {
    return { kind: "invalid", reason: "path-contains-control" };
  }
  return { kind: "open-path", path };
}

function promptLinkFromUrl(raw: string): LaunchHandoff {
  const parsed = parsePromptLink(raw);
  if (!parsed.ok) {
    return { kind: "invalid", reason: `prompt-link:${parsed.reason}` };
  }
  return {
    kind: "prompt-link",
    link: parsed.link,
    report: parsed.report,
    url: raw.trim(),
  };
}

/**
 * Read a structured prompt-link object:
 * `{ prompt, project?, session?, agent?, model? }`, or an object that carries
 * `url` / `link` instead. Forbidden fields are ignored rather than copied.
 */
function promptLinkFromPayload(payload: Record<string, unknown>): LaunchHandoff {
  const url = optionalString(payload["url"]) ?? optionalString(payload["link"]);
  if (url) return promptLinkFromUrl(url);

  const nestedLink = payload["promptLink"] ?? payload["prompt_link"];
  if (typeof nestedLink === "string") return promptLinkFromUrl(nestedLink);
  if (isRecord(nestedLink)) return promptLinkFromPayload(nestedLink);
  if (isRecord(payload["link"])) return promptLinkFromPayload(payload["link"]);

  const rawPrompt = payload["prompt"];
  const nested = isRecord(rawPrompt) ? rawPrompt : null;
  if (typeof rawPrompt !== "string" && nested === null) {
    return { kind: "invalid", reason: "prompt-link-without-prompt" };
  }
  const source: Record<string, unknown> = nested ?? payload;
  const input: PromptLinkInput = {
    prompt:
      typeof rawPrompt === "string"
        ? rawPrompt
        : optionalString(source["prompt"]),
    project: optionalString(source["project"]),
    session: optionalString(source["session"]),
    agent: optionalString(source["agent"]),
    model: optionalString(source["model"]),
  };
  const built = serializePromptLink(input);
  const link = built.link;
  if (
    !link.prompt &&
    !link.project &&
    !link.session &&
    !link.agent &&
    !link.model
  ) {
    return { kind: "invalid", reason: "empty-prompt-link" };
  }
  return {
    kind: "prompt-link",
    link,
    report: built.report,
    url: built.url,
  };
}

function fromLaunchObject(payload: Record<string, unknown>): LaunchHandoff {
  const version = payload["v"] ?? payload["version"];
  if (version === undefined || version === null) {
    return { kind: "invalid", reason: "missing-version" };
  }
  if (String(version) !== String(LAUNCH_REQUEST_VERSION)) {
    return { kind: "invalid", reason: `unsupported-version:${String(version)}` };
  }

  const kind = String(payload["kind"] ?? payload["type"] ?? "")
    .trim()
    .toLowerCase()
    .replace(/_/g, "-");

  if (kind === "focus" || kind === "activate") return { kind: "focus" };
  if (kind === "open-path" || kind === "open" || kind === "path") {
    return pathHandoff(payload["path"]);
  }
  if (kind === "prompt-link" || kind === "promptlink" || kind === "prompt") {
    return promptLinkFromPayload(payload);
  }

  // Tolerate a versioned request that omitted `kind` but has a recognizable
  // payload. This keeps forward/backward compatibility cheap.
  const nested =
    payload["promptLink"] ??
    payload["prompt_link"] ??
    payload["link"];
  if (typeof nested === "string") {
    if (isPromptLinkUrl(nested)) return promptLinkFromUrl(nested);
    return { kind: "invalid", reason: "prompt-link-invalid-url" };
  }
  if (isRecord(nested)) return promptLinkFromPayload(nested);
  if (typeof payload["path"] === "string") return pathHandoff(payload["path"]);
  if (
    typeof payload["prompt"] === "string" ||
    isRecord(payload["prompt"])
  ) {
    return promptLinkFromPayload(payload);
  }
  if (typeof payload["url"] === "string" && isPromptLinkUrl(payload["url"])) {
    return promptLinkFromUrl(payload["url"]);
  }
  return { kind: "invalid", reason: "unrecognized-launch-request" };
}

/**
 * Adapter for the current `take_launch_open_path` / `marionette-open-path`
 * string payload. `null` means "no handoff was queued"; an empty string is the
 * existing "second instance started without a path, just focus" sentinel.
 */
export function parseLaunchHandoffPayload(raw: unknown): LaunchHandoff {
  if (raw === null || raw === undefined) return { kind: "none" };
  if (typeof raw !== "string") {
    if (isRecord(raw)) return fromLaunchObject(raw);
    return { kind: "invalid", reason: "payload-not-string" };
  }

  const text = raw.trim();
  if (!text) return { kind: "focus" };
  if (isPromptLinkUrl(text)) return promptLinkFromUrl(text);

  if (text.startsWith("{") || text.startsWith("[")) {
    try {
      const parsed = JSON.parse(text) as unknown;
      if (isRecord(parsed)) return fromLaunchObject(parsed);
      if (typeof parsed === "string") return pathHandoff(parsed);
      return { kind: "invalid", reason: "json-not-object" };
    } catch {
      return { kind: "invalid", reason: "malformed-json" };
    }
  }

  // A non-prompt URL scheme is not a Windows path. Single-letter schemes are
  // drive letters (`C:\...`) and must keep flowing through the path branch.
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(text);
  if (scheme) {
    const name = scheme[1] ?? "";
    if (name.length > 1) {
      return { kind: "invalid", reason: `unsupported-url-scheme:${name.toLowerCase()}` };
    }
  }

  // Backward compatibility: every other non-empty payload is the current
  // raw open-path handoff.
  return pathHandoff(text);
}

/**
 * Mirror of the Rust-side writer. Only whitelisted payloads are emitted:
 * prompt links are re-sanitized so a permission/auto-send parameter cannot
 * survive a round trip.
 */
export function serializeLaunchRequest(request: LaunchRequestV1): string {
  if (request.kind === "open-path") {
    const handoff = pathHandoff(request.path);
    if (handoff.kind !== "open-path") {
      throw new Error(`serializeLaunchRequest: ${handoff.kind === "invalid" ? handoff.reason : "invalid path"}`);
    }
    return JSON.stringify({
      v: LAUNCH_REQUEST_VERSION,
      kind: "open-path",
      path: handoff.path,
    });
  }
  if (request.kind === "prompt-link") {
    const handoff = promptLinkFromUrl(request.url);
    if (handoff.kind !== "prompt-link") {
      throw new Error(
        `serializeLaunchRequest: ${
          handoff.kind === "invalid" ? handoff.reason : handoff.kind
        }`,
      );
    }
    const canonical = serializePromptLink(handoff.link);
    return JSON.stringify({
      v: LAUNCH_REQUEST_VERSION,
      kind: "prompt-link",
      url: canonical.url,
    });
  }
  return JSON.stringify({ v: LAUNCH_REQUEST_VERSION, kind: "focus" });
}
