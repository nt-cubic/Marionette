/**
 * verify-pasted-files — 粘贴文件按真实路径附加（读自 codeg pasted-file-paths 的移植）
 *
 * lib/pastedFiles.ts 的配对：webview 把资源管理器复制的文件变成只有
 * 名字/大小/类型的 File（目录 size 0），配对规则 = 名字 NFC 相同 +（普通文件）
 * 大小相同；每个条目只配一次（两个同名文件分属两个文件夹时不会串）。
 * 配不上的进 rest（截图/网页拷图——没有文件在后面），保持粘贴顺序。
 *
 * 跑法：npx tsx scripts/verify-pasted-files.mts
 */

import { pairPastedFilesWithPaths } from "../src/lib/pastedFiles";
import type { ClipboardFileEntry } from "../src/lib/api";

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  if (ok) {
    console.log(`  ok  ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
  }
}

const entry = (path: string, size: number, isDir = false): ClipboardFileEntry => {
  const name = path.split(/[\\/]/).pop() ?? path;
  return { path, name, size, isDir };
};

// 名字+大小都合 → 按真实路径配对；配不上的保持顺序进 rest。
{
  const files = [new File(["x".repeat(10)], "a.txt"), new File([""], "shot.png")];
  const entries = [entry("D:\\docs\\a.txt", 10), entry("D:\\docs\\b.txt", 3)];
  const { paths, rest } = pairPastedFilesWithPaths(files, entries);
  check("名字+大小相同 → 配对", paths.length === 1 && paths[0] === "D:\\docs\\a.txt", JSON.stringify(paths));
  check("大小不同不配对 → 进 rest", rest.length === 1 && rest[0]!.name === "shot.png");
}

// 目录按名字配对（webview 给目录的 File size 0，条目也当 0 对待）。
{
  const files = [new File([""], "assets")];
  const entries = [entry("D:\\proj\\assets", 0, true), entry("D:\\assets.txt", 5)];
  const { paths, rest } = pairPastedFilesWithPaths(files, entries);
  check("目录按名字配对", paths.length === 1 && paths[0] === "D:\\proj\\assets", JSON.stringify(paths));
  check("目录不落在 rest", rest.length === 0);
}

// 同名不同文件夹：一对一，不串。
{
  const files = [new File(["ab"], "n.txt"), new File(["ab"], "n.txt")];
  const entries = [entry("D:\\one\\n.txt", 2), entry("D:\\two\\n.txt", 2)];
  const { paths, rest } = pairPastedFilesWithPaths(files, entries);
  check(
    "两个同名同大小文件各配各的",
    paths.length === 2 && rest.length === 0 && paths.includes("D:\\one\\n.txt") && paths.includes("D:\\two\\n.txt"),
    JSON.stringify(paths),
  );
}

// 条目数量少于文件：多余文件进 rest，保持粘贴顺序。
{
  const files = [new File(["a"], "x.txt"), new File(["b"], "y.txt"), new File(["c"], "z.txt")];
  const entries = [entry("D:\\y.txt", 1)];
  const { paths, rest } = pairPastedFilesWithPaths(files, entries);
  check("只有能配的配", paths.length === 1 && paths[0] === "D:\\y.txt");
  check("rest 保持粘贴顺序", rest.map((f) => f.name).join(",") === "x.txt,z.txt");
}

// Unicode 名（NFC 归一）：全角/组合形式不影响名字比对（实测 NFC 输入恒等；
// NFD 输入也被归一后配上）。
{
  const files = [new File(["q"], "需求.md")];
  const entries = [entry("D:\\docs\\需求.md", 1)];
  const { paths } = pairPastedFilesWithPaths(files, entries);
  check("中文文件名配对", paths.length === 1 && paths[0] === "D:\\docs\\需求.md");
}

// 空输入。
{
  const { paths, rest } = pairPastedFilesWithPaths([], [entry("D:\\a", 1)]);
  check("无文件 → 无配对", paths.length === 0 && rest.length === 0);
  const empty = pairPastedFilesWithPaths([new File([""], "a")], []);
  check("无条目 → 全进 rest", empty.paths.length === 0 && empty.rest.length === 1);
}

if (failures > 0) {
  console.error(`\n${failures} 项失败`);
  process.exit(1);
}
console.log("\npasted-files 全部通过");
