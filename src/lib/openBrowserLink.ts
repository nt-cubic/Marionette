// The main window supplies the browser workspace; detached windows can fall
// back to the system browser when there is no local workspace host.
type BrowserLinkOpener = (url: string) => Promise<void>;

let opener: BrowserLinkOpener | null = null;

export function setBrowserLinkOpener(next: BrowserLinkOpener | null): void {
  opener = next;
}

export async function openInBrowserWorkspace(url: string): Promise<boolean> {
  if (!/^https?:\/\//i.test(url) || !opener) return false;
  await opener(url);
  return true;
}
