/**
 * Pair the `File`s of a desktop paste with the OS clipboard's file entries,
 * so each one can be attached by its real path, as a drop is.
 *
 * A webview turns each file an Explorer copy put on the clipboard into one
 * `File` carrying its name, size and type but never its path (a folder comes
 * through as one of size 0). So a `File` is an entry when the names agree
 * (NFC — macOS NFD names never reach Windows, but the normalize is free and
 * keeps the port honest) and, for a regular file, the sizes do too. Each
 * entry pairs once, so two same-named files from different folders stay
 * apart.
 *
 * What pairs with nothing has no file behind it (a screenshot, an image
 * copied out of a page) and comes back in `rest`, in paste order — those
 * keep their bytes. Ported from codeg `pasted-file-paths.ts` (Apache-2.0).
 */
import type { ClipboardFileEntry } from "./api";

export function pairPastedFilesWithPaths(
  files: readonly File[],
  entries: readonly ClipboardFileEntry[],
): { paths: string[]; rest: File[] } {
  const unpaired = [...entries];
  const paths: string[] = [];
  const rest: File[] = [];
  for (const file of files) {
    const name = file.name.normalize("NFC");
    const index = unpaired.findIndex(
      (entry) =>
        entry.name.normalize("NFC") === name &&
        (entry.isDir || entry.size === file.size),
    );
    if (index < 0) {
      rest.push(file);
      continue;
    }
    paths.push(unpaired[index]!.path);
    unpaired.splice(index, 1);
  }
  return { paths, rest };
}
