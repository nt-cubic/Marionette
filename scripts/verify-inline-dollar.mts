/**
 * verify-inline-dollar — 守卫式单美元行内公式（读自 codeg inline-dollar-math 的移植）
 *
 * lib/inlineDollarMath.ts：只有紧贴、同行、像 TeX 的 `$…$` 才重写成 `$$…$$`
 * 行内公式（remark-math 的 singleDollarTextMath 保持关闭）；货币对、shell
 * 展开、代码/HTML/display 围栏内、读不懂的一律保持字面。richMarkdown 的
 * scan 把围栏外的守卫候选当数学提示（裸 `$` 金额仍不提示）。
 *
 * 跑法：npx tsx scripts/verify-inline-dollar.mts
 */

import {
  hasInlineDollarMathCandidate,
  normalizeInlineDollarMath,
} from "../src/lib/inlineDollarMath";
import { scanRichFeatures } from "../src/lib/richMarkdown";

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  if (ok) {
    console.log(`  ok  ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
  }
}

function rewrites(name: string, input: string, want: string) {
  const got = normalizeInlineDollarMath(input);
  check(name, got === want, got);
}
function literal(name: string, input: string) {
  const got = normalizeInlineDollarMath(input);
  check(name, got === input, got);
}

// ── 1. 守卫命中的公式 → `$$` 行内 ────────────────────────────────────────
rewrites("单个字母符号 $T$", "正文 $T$。", "正文 $$T$$。");
rewrites("双字母符号 $AV$", "正文 $AV$。", "正文 $$AV$$。");
rewrites("含空格的符号差 $A - T$", "正文 $A - T$。", "正文 $$A - T$$。");
rewrites("TeX 命令 $\\Gamma^0, \\Gamma^C$", "正文 $\\Gamma^0, \\Gamma^C$。", "正文 $$\\Gamma^0, \\Gamma^C$$。");
rewrites("上标命令 $\\mathcal{U}^{AV}$", "正文 $\\mathcal{U}^{AV}$。", "正文 $$\\mathcal{U}^{AV}$$。");
rewrites("多项式 $x^2 + y^2$", "正文 $x^2 + y^2$。", "正文 $$x^2 + y^2$$。");
rewrites("$E=mc^2$", "正文 $E=mc^2$。", "正文 $$E=mc^2$$。");
rewrites("函数记法 $f(x)$", "正文 $f(x)$。", "正文 $$f(x)$$。");
rewrites("闭符后的问号/叹号允许", "Is it $x$? Then $n$!", "Is it $$x$$? Then $$n$$!");
rewrites("跨过货币后继续并改写多个公式", "$5,$x^2$ and $AV$", "$5,$$x^2$$ and $$AV$$");
rewrites("公式内的转义美元保留", "$x + \\$5$ and $T$", "$$x + \\$5$$ and $$T$$");
rewrites("display 围栏更长闭符后继续", "$$\nx^2\n$$$\n\n$T$", "$$\nx^2\n$$$\n\n$$T$$");

// 幂等：再跑一遍不变。
{
  const once = normalizeInlineDollarMath("$x + \\$5$ and $T$");
  check("幂等", normalizeInlineDollarMath(once) === once);
}

// ── 2. 字面场景（两个守卫面：货币 & shell；加上代码/围栏保护） ────────────
literal("缩进代码里的 $AV$", '    echo "$AV$"');
literal("围栏代码内", "```tex\n$AV$");
literal("列表里的缩进代码", "- item\n\n      $AV$");
literal("货币与变量混排", "Costs $25. Set $HOME and $1.");
literal("货币对 $5 to $10", "$5 to $10");
literal("货币区间 $5-$10", "$5-$10");
literal("shell 路径 $HOME/$USER", "$HOME/$USER");
literal("shell 大括号 ${HOME}/${USER}", "${HOME}/${USER}");
literal("$PATH:$HOME", "$PATH:$HOME");
literal("US$5 前缀货币", "US$5");
literal("echo $A$B 无分隔", "echo $A$B");
literal("$FOO + $BAR 变量名", "$FOO + $BAR");
literal("转义美元 \\$x^2\\$", "\\$x^2\\$");
literal("开符后带空格 $ x$", "$ x$");
literal("闭符前带空格 $x $", "$x $");
literal("跨行 $x\\ny$", "$x\ny$");
literal("裸金额 $25$", "$25$");
literal("普通散文 $ordinary prose$", "$ordinary prose$");
literal("未闭的流式片段 An unfinished $x^2", "An unfinished $x^2");
literal("display 内嵌单美元 $$x + $y$ $$", "$$x + $y$ $$");
literal("display 围栏 $$\\nx + $y$\\n$$", "$$\nx + $y$\n$$");
literal("未闭 display 围栏 $$\\nx + $y$", "$$\nx + $y$");
literal("三美元围栏 $$$…$$$", "$$$\nx + $y$\n$$$");
literal("NUL 分隔 $x\\0y$", "$x\0CBLK0\0y$");
literal("闭符前的转义美元并成 $$$", "$a\\$$ end");
literal("<pre> 原始 HTML", "<pre>\n$x^2$\n</pre>");
literal("<div> 原始 HTML", "<div>\n$AV$ text\n</div>");
literal('闭符后接大括号展开 "$prefix${suffix}"', 'echo "$prefix${suffix}"');
literal("闭符后接 $@", "echo $A$@");
literal("命令替换 $(…)/$(…)", '$(git rev-parse --show-toplevel)/$(basename "$PWD")');
literal("短替换对 $(a)-$(b)", "(a)-(b)");

// 行内代码保护：候选存在时才 parse，保护反引号内的 `$T$`。
rewrites("行内代码内的美元不动，外面的改写", "`$T$` and $AV$", "`$T$` and $$AV$$");

// ── 3. 候选探测（免 parse 的超集） ────────────────────────────────────────
check("候选：正文 $T$", hasInlineDollarMathCandidate("正文 $T$。"));
check("候选：货币后的公式", hasInlineDollarMathCandidate("$5,$x^2$ and $AV$"));
check("候选：代码里的也算（超集，只为提示加载）", hasInlineDollarMathCandidate("```tex\n$AV$"));
check("非候选：货币与变量", !hasInlineDollarMathCandidate("Costs $25. Set $HOME and $1."));
check("非候选：货币区间", !hasInlineDollarMathCandidate("$5-$10"));
check("非候选：$A$B", !hasInlineDollarMathCandidate("echo $A$B"));
check("非候选：闭符转义美元", !hasInlineDollarMathCandidate("$a\\$$"));

// ── 4. scan 集成：围栏外候选 → math 提示 ─────────────────────────────────
check("scan：守卫候选是数学提示", scanRichFeatures("正文 $x^2$ 继续").math === true);
check("scan：裸货币不是提示", scanRichFeatures("Costs $5 to $10 total").math === false);
check("scan：围栏内的候选不提示", scanRichFeatures("```bash\n$AV$\n```").math === false);
check("scan：围栏内候选 + 围栏外候选 → 提示", scanRichFeatures("```bash\n$AV$\n```\n后文 $T$").math === true);
check("scan：$$ 提示不受影响", scanRichFeatures("$$\nx^2\n$$").math === true);

if (failures > 0) {
  console.error(`\n${failures} 项失败`);
  process.exit(1);
}
console.log("\ninline-dollar 全部通过");
