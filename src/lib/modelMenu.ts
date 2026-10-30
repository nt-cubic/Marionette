import { t } from "./i18n";
import type { LibraryGroup, LibraryRow } from "./modelLibrary";

/**
 * Model-menu presentation helpers. Pure — the library groups stay as the
 * Model Library dialog shows them; only the composer menu reads them this way.
 */

export type MenuRow = LibraryRow & {
  /** Where the row came from once its own group header is folded away (`127.0.0.1:7031`). */
  hint?: string;
};

export type MenuGroup = Omit<LibraryGroup, "rows"> & { rows: MenuRow[] };

const LOOPBACK_HOST = /^(localhost|127(?:\.\d{1,3}){3}|\[::1\]|0\.0\.0\.0)(:\d+)?$/i;

/**
 * Local endpoints usually serve one model each, so the menu grew one header
 * per port (`127.0.0.1:17041`, `127.0.0.1:18080`…) over a single row. A run of
 * such one-row loopback groups folds into one 本机 group; the host moves to the
 * row's trailing hint.
 */
export function foldLocalSingleGroups(groups: LibraryGroup[]): MenuGroup[] {
  const out: MenuGroup[] = [];
  let local: MenuGroup | null = null;
  for (const group of groups) {
    const single = group.rows.length === 1 && LOOPBACK_HOST.test(group.label.trim());
    if (!single) {
      local = null;
      out.push(group);
      continue;
    }
    const row: MenuRow = { ...group.rows[0]!, hint: group.label.trim() };
    if (!local) {
      local = { key: `local:${group.key}`, label: t("modelMenu.localGroup"), rows: [] };
      out.push(local);
    }
    local.rows.push(row);
  }
  // A lone local endpoint keeps its own header — nothing to fold.
  return out.map((group) =>
    group.key.startsWith("local:") && group.rows.length === 1
      ? { key: group.key.slice("local:".length), label: group.rows[0]!.hint ?? group.label, rows: [{ ...group.rows[0]!, hint: undefined }] }
      : group,
  );
}

/**
 * `DeepSeek V4 Flash Vision (Local, 256K)` → name `DeepSeek V4 Flash Vision`,
 * meta `Local, 256K`: the parenthetical is facts about the model, not its name.
 */
export function splitModelMeta(label: string): { name: string; meta: string } {
  const match = /^(.*\S)\s*\(([^()]+)\)\s*$/.exec(label);
  if (!match) return { name: label, meta: "" };
  return { name: match[1]!, meta: match[2]!.trim() };
}
