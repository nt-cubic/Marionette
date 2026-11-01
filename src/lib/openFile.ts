// Routing for "what happens when the user clicks a file path in the UI".
//
// Markdown/text files open inside Marionette as a document pseudo-tab
// (see DocPage); everything else keeps the open-in-default-app behaviour.
// The doc-tab state lives in the main window's App, so this module is a tiny
// registry: App installs the opener on mount (detached windows never do, and
// clicks there fall through to the OS handler — same posture as the memo tab).

const DOC_EXTS = new Set(["md", "markdown", "mdx", "txt"]);

/** Does this path name a file the in-app document tab can render? */
export function isDocPath(path: string): boolean {
  const dot = path.lastIndexOf(".");
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (dot <= slash + 1) return false; // no extension (or a dot inside a directory name)
  return DOC_EXTS.has(path.slice(dot + 1).toLowerCase());
}

type DocTabOpener = (absolutePath: string) => void;

let docTabOpener: DocTabOpener | null = null;

/** Installed by the main window's App; `null` uninstalls on unmount. */
export function setDocTabOpener(opener: DocTabOpener | null): void {
  docTabOpener = opener;
}

/**
 * Hand a *resolved absolute* file path to the document tab.
 *
 * Returns false when there is no tab host (detached window) or the path is
 * not a document — callers then fall through to their existing open flow.
 * `isDocPath` is checked here again so call sites can use this as the single
 * gate after their cheap pre-check.
 */
export function openInDocTabIfEligible(absolutePath: string): boolean {
  if (!docTabOpener || !isDocPath(absolutePath)) return false;
  docTabOpener(absolutePath);
  return true;
}
