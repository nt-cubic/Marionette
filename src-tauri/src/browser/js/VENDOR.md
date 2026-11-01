# Vendored from Codeg (Apache-2.0)

- Source repo: <https://github.com/spacering-net/codeg>（本地克隆 `D:\Projects\codeg`）
- Version: v0.41.3（origin/main = `ddd1f09f`，2026-10-30 复核，两文件与 v0.40.0 tree hash 一致）
- Upstream license: Apache-2.0（与 Codeg 主仓相同；保留本声明即满足署名要求）

| 文件 | 上游路径 | 说明 |
|---|---|---|
| `agent.bundle.js` | `src-tauri/src/browser/js/agent.bundle.js` | esbuild 产物（提交进仓，上游同款）；入口 `browser-agent/src/index.ts`，内含 vendored Playwright v1.63.0 ariaSnapshot 九文件（byte-for-byte）。暴露 `globalThis.__codegAgent = { snapshot, elementForRef, act, locate, rectOf }`，只在 isolated world 里可见。 |
| `helper.js` | `src/browser-injected/helper.js` | 页面→宿主消息通道（SPA 导航、标题、手势），通过 `__codegSend` binding 上报。 |

**名字保留 `__codegAgent` / `__codegSend` / world 名 `codeg`**：bundle 是压缩产物，运行时全局名与
binding 名在内部多处出现，改名的收益（品牌）远小于改错的风险。它们只存在于 isolated world，
页面脚本不可见，不影响用户页面。上游 bundle 构建流程见 `browser-agent/` 与
`scripts/build-browser-agent.mjs`（升级 aria 引擎时照上游流程重出产物，别手改这份文件）。
