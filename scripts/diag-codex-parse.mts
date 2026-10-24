// Run Marionette's real codex /status parsers on the live capture.
import { parseCodexStatusRateLimits, mergeUsageFromText } from "../src/lib/usage.ts";

const live = "**Model:** gpt-6.1-sol[high]  \n**Directory:** D:\\Projects\\Marionette  \n**Approval:** on-request  \n**Sandbox:** workspace-write  \n**Account:** ChatGPT plus (songfanzhen@gmail.com)  \n**Session:** `01a16e0f-f843-7f13-aee5-ef0d74180468`  \n  \n**Token usage:** data not available yet  \n**Context window:** data not available yet  \n**codex 5h limit:** 91% left (resets 03:11 on 10\u670825\u65e5)  \n**codex Weekly limit:** 90% left (resets 14:42 on 10\u670831\u65e5)  \n**codex Credits:** 0";

console.log("parseCodexStatusRateLimits:", JSON.stringify(parseCodexStatusRateLimits(live), null, 2));
console.log("mergeUsageFromText windows:", JSON.stringify(mergeUsageFromText(undefined, live)?.windows ?? null, null, 2));
