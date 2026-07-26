# Provider API Key 配置 + 最近使用模型

## 问题

用户下载 AgentShell，创建 OpenCode session 后，发现模型选择器一片空白——因为 OpenCode 没有任何 API Key 配置。目前用户只能自己去终端敲 `opencode providers login`，体验断点。

目标：**在 AgentShell 内部完成 API Key 配置，用户不需要离开 App。**

## 用户流程

### 首次使用（无 Key）

1. 用户创建 OpenCode session
2. Composer 的模型选择器**不再隐藏**（当前 `hasModels = false` 时整个选择器消失）
3. 点击模型选择器 → 弹出空状态面板：

```
┌─────────────────────────────────────┐
│ 🔑 还没有配置 API Key              │
│ 选择服务商并输入 Key 即可开始使用   │
│                                    │
│  ┌──── 添加 API Key ────┐         │
│  │                      │         │
│  └──────────────────────┘         │
└─────────────────────────────────────┘
```

4. 点击「添加 API Key」→ 弹出配置对话框

```
┌─── 添加 API Key ───────────────────────┐
│                                        │
│  服务商  [DeepSeek               ▼]    │
│                                        │
│  API Key  [························]   │
│           (粘贴你的 API Key)            │
│                                        │
│  ┌──────────┐  ┌──────────────────┐    │
│  │  取消     │  │  保存并连接      │    │
│  └──────────┘  └──────────────────┘    │
└────────────────────────────────────────┘
```

5. 选择服务商（DeepSeek / OpenAI / Anthropic / OpenRouter 等），粘贴 Key
6. 点击「保存并连接」→ 后端写入 OpenCode 的 `auth.json` → 前端刷新能力 → 模型列表出现

### 已有 Key（日常使用）

模型下拉菜单完整布局：

```
┌─────────────────────────────────────────┐
│  📋 最近使用                            │
│    deepseek-v4-flash           ← 刚刚   │
│    gpt-4o                      ← 5分钟前 │
│    claude-3.5-sonnet           ← 1小时前 │
├─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─┤
│ Search provider or model…               │
├─────────────────────────────────────────┤
│ DeepSeek                                │
│   deepseek-v4-flash                     │
│   deepseek-v4-pro                       │
├─────────────────────────────────────────┤
│ OpenRouter                              │
│   openrouter/gpt-4o                    │
│   openrouter/claude-3.5-sonnet         │
├─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─┤
│ ＋ 添加 API Key…                        │
└─────────────────────────────────────────┘
```

## 技术设计

### 1. 后端 — 新增 Tauri Commands

#### `save_provider_key(provider: String, key: String) -> Result<(), String>`

写入 OpenCode 的 `auth.json`。

**路径规则**（复用 `opencode_auth_path()` 查找逻辑）：
- `~/.local/share/opencode/auth.json`（Linux/macOS）
- `%APPDATA%/opencode/auth.json`（Windows）
- `~/.config/opencode/auth.json`（备选）
- 如果都不存在，使用 `~/.local/share/opencode/auth.json` 并自动创建父目录

**写入格式**（与 OpenCode 原生格式一致）：

```json
{
  "deepseek": { "key": "sk-xxxx" },
  "openrouter": { "key": "sk-xxxx" },
  "openai": { "key": "sk-xxxx" }
}
```

**流程**：
1. 读取已有文件（不存在则视为空 JSON `{}`）
2. 合并：`auth[provider] = { "key": key }`
3. 写回文件（原子写入：先写临时文件再 rename）
4. 返回成功

#### `list_providers() -> Result<Vec<ProviderInfo>, String>`

列出 auth.json 中已配置的 provider。

```rust
struct ProviderInfo {
    provider: String,    // "deepseek"
    label: String,       // "DeepSeek"
    has_key: bool,       // 是否有 key（不返回 key 本身）
    configured_at: Option<String>,  // 配置时间（可选）
}
```

#### 服务商映射表

同时在前端和后端维护一份一致的映射：

| 显示名 | Provider ID | 说明 |
|--------|-------------|------|
| DeepSeek | `deepseek` | 热门，余额可查 |
| OpenRouter | `openrouter` | 聚合多模型，余额可查 |
| OpenAI | `openai` | GPT 系列 |
| Anthropic | `anthropic` | Claude 系列 |
| Google | `google` | Gemini 系列 |
| xAI (Grok) | `xai` | Grok 系列 |
| Z.AI (GLM) | `zai` | 智谱 GLM |
| SiliconFlow | `siliconflow` | 硅基流动 |

Provider ID 与 `provider_usage.rs` 中的 `auth_key_for()` 别名表对齐。

### 2. 前端 — Composer 改造

#### 2.1 模型选择器始终显示

**文件**: `src/components/Composer.tsx`

**改动**：第 1324 行条件渲染逻辑

```tsx
// 修改前
{(hasModels || hasEffort) && ( /* model selector */ )}

// 修改后
{/* Always show: models, model list empty state, or just effort */}
{(hasModels || hasEffort || (caps && caps.models.length === 0)) && (
  /* model selector */
)}
```

当 `caps.models.length === 0` 时，下拉菜单内显示空状态提示 + 「添加 API Key」按钮。

#### 2.2 模型下拉菜单新增顶部「最近使用」区域

**数据存储**: `localStorage`，key `agentshell-recent-models`

```typescript
// 格式：有序数组，索引越小越新
type RecentModelEntry = {
  modelId: string;
  lastUsedAt: number; // Date.now()
};

// 上限 5 个，去重（同 modelId 只保留最新那次）
```

**记录时机**：用户在 Composer 切换模型成功（`commitUpdate` 成功后）→ 记录到 localStorage

**渲染逻辑**：菜单打开时读取 localStorage，过滤掉当前 `caps.models` 中不存在的 model ID（避免显示已失效的模型），取前 5 个

#### 2.3 模型下拉菜单底部「添加 API Key」入口

当 `caps.models.length > 0` 时，在模型分组列表底部加分隔线 + 按钮。

### 3. 前端 — 新增 ProviderConfigDialog 组件

**文件**: `src/components/ProviderConfigDialog.tsx`

功能：
- 服务商下拉选择
- API Key 输入（`type="password"`，占位符提示）
- 保存按钮 → 调用 `save_provider_key`
- 取消按钮
- 加载状态（保存中）
- 错误状态（保存失败显示原因）
- 成功 → 关闭弹窗 → 触发能力刷新

复用现有的对话框样式（`project-dialog-backdrop` / `project-dialog` CSS 类）。

### 4. API 层

**文件**: `src/lib/api.ts`

新增两个 IPC 调用：

```typescript
export async function saveProviderKey(
  provider: string,
  key: string,
): Promise<void>;

export async function listProviders(): Promise<ProviderInfo[]>;
```

## 文件变更清单

| 文件 | 改动 |
|------|------|
| `src-tauri/src/commands.rs` | 新增 `save_provider_key` 和 `list_providers` 命令 |
| `src-tauri/src/main.rs` | 注册两个新命令 |
| `src-tauri/src/provider_usage.rs` | 提取 `opencode_auth_path()` 为公共函数；新增 `write_provider_key()` |
| `src/components/Composer.tsx` | 模型选择器始终显示；新增空状态 + 最近使用列表 + 底部添加入口 |
| `src/components/ProviderConfigDialog.tsx` | **新文件** — 添加 API Key 对话框 |
| `src/lib/api.ts` | 新增 `saveProviderKey` / `listProviders` |
| `src/lib/types.ts` | 新增 `ProviderInfo` 类型 |
| `src/styles/app.css` | 可能需少量样式补充（空状态、最近使用区域） |

## 最近使用模型实现细节

### 存储

```typescript
// src/lib/recentModels.ts — 新工具模块

const STORAGE_KEY = "agentshell-recent-models";
const MAX_ENTRIES = 5;

type RecentModelEntry = {
  modelId: string;
  lastUsedAt: number;
};

export function recordModelUsage(modelId: string): void;
export function getRecentModels(validModelIds: Set<string>): RecentModelEntry[];
```

### 记录时机

在 `Composer.tsx` 的 `commitUpdate` 成功回调中调用 `recordModelUsage(modelId)`。

### 去重

- 同一个 modelId 重复使用 → 更新时间戳，移到数组最前
- 上限 5 个 → 超出的移除最旧条目

### 过滤

显示前过滤掉当前 `caps.models` 中不存在的 ID（agent 切换或 Key 变更后旧模型失效）

## 未包含在 v1 的功能

- ✗ Key 编辑（可删了重加）
- ✗ Key 删除
- ✗ 测试连接（验证 Key 是否有效）
- ✗ 手动固定模型到常用列表
- ✗ 跨设备同步
