/** Native browser surfaces always paint over the app webview. Suspend them
 * while an app modal is open, or a non-modal menu/dialog overlaps the host. */
export function browserHostCovered(host: HTMLElement): boolean {
  const hostRect = host.getBoundingClientRect();
  for (const overlay of document.querySelectorAll<HTMLElement>(
    '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], ' +
    '.project-dialog-backdrop, .permission-dialog-backdrop, .command-palette-backdrop, ' +
    '.image-annotator-backdrop, .image-lightbox',
  )) {
    if (!overlay.getClientRects().length || getComputedStyle(overlay).visibility === "hidden") continue;
    if (overlay.getAttribute("aria-modal") === "true") return true;
    const rect = overlay.getBoundingClientRect();
    if (rect.left < hostRect.right && rect.right > hostRect.left &&
        rect.top < hostRect.bottom && rect.bottom > hostRect.top) return true;
  }
  return false;
}
