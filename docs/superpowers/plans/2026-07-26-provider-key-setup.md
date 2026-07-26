# Provider API Key 配置 + 最近使用模型 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) for syntax tracking.

**Goal:** 用户可以在 AgentShell 的 Composer 模型选择器中直接添加 API Key（写入 OpenCode 的 auth.json），无需离开 App。同时增加最近使用模型列表，方便快速切换。

**Architecture:** 后端新增两个 Tauri command（`save_provider_key` / `list_providers`），读取/写入 OpenCode 的 auth.json；前端 Composer 模型选择器始终显示（无 Key 时显示空状态引导），新增 ProviderConfigDialog 组件用于 Key 配置，新增 recentModels.ts 工具用于最近使用模型追踪。

**Tech Stack:** Rust (Tauri), React 18 + TypeScript, existing CSS dialog pattern

---

## 文件结构

| 作用 | 文件 | 类型 |
|------|------|------|
| 写入 auth.json + 读取 provider 列表 | `src-tauri/src/provider_usage.rs` | 修改（追加） |
| Tauri 命令注册 | `src-tauri/src/commands.rs` | 修改 |
| 命令注册表 | `src-tauri/src/main.rs` | 修改 |
| ProviderInfo 类型 | `src/lib/types.ts` | 修改 |
| saveProviderKey / listProviders IPC | `src/lib/api.ts` | 修改 |
| 最近使用模型 localStorage 工具 | `src/lib/recentModels.ts` | 新增 |
| 添加 Key 的对话框组件 | `src/components/ProviderConfigDialog.tsx` | 新增 |
| Composer 模型选择器改造 | `src/components/Composer.tsx` | 修改 |
| 新 UI 样式 | `src/styles/app.css` | 修改 |

---

### Task 1: 后端 — provider_usage.rs 追加写入能力

**Files:**
- Modify: `src-tauri/src/provider_usage.rs` (在文件末尾、test 模块之前追加)
- Test: 同一文件末尾的 `#[cfg(test)] mod tests`

**Context:** 当前 `provider_usage.rs` 已有 `opencode_auth_path()`（查找现有 auth.json）和 `load_auth_json()`（读取）。需要追加：
- `opencode_auth_path_write()` — 获取写入路径（不存在则创建默认路径）
- `write_provider_key_at(path, provider, key)` — 核心写入逻辑（接受路径参数以便测试）
- `write_provider_key(provider, key)` — 无路径重载（使用默认路径）
- `ProviderInfo` 结构体 + `list_providers()` — 列出已配置的 provider
- 单元测试

- [ ] **Step 1.1: 追加 write 工具函数和 ProviderInfo 结构体**

在 `provider_usage.rs` 的 `probe_provider_usage()` 函数之后、`#[cfg(test)]` 之前追加：

```rust
/// Like `opencode_auth_path()` but returns the path to *write* to:
/// uses the first existing location, or falls back to the default.
pub fn opencode_auth_path_write() -> PathBuf {
    if let Some(existing) = opencode_auth_path() {
        return existing;
    }
    let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
    let default = home.join(".local").join("share").join("opencode").join("auth.json");
    if let Some(parent) = default.parent() {
        let _ = fs::create_dir_all(parent);
    }
    default
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub provider: String,
    pub label: String,
    pub has_key: bool,
}

/// Provider id → human label (mirrors `provider_label` above).
fn provider_display_name(id: &str) -> &'static str {
    match id {
        "deepseek" => "DeepSeek",
        "openrouter" => "OpenRouter",
        "openai" => "OpenAI",
        "anthropic" => "Anthropic",
        "google" => "Google",
        "xai" => "xAI (Grok)",
        "zai" | "zhipu" => "Z.AI (GLM)",
        "siliconflow" | "siliconflow-cn" => "SiliconFlow",
        other => other,
    }
}

/// Write a provider API key to the given auth file path.
/// If `path` is `None`, uses the default OpenCode auth.json location.
pub fn write_provider_key_at(
    provider: &str,
    key: &str,
    path: Option<&std::path::Path>,
) -> Result<(), String> {
    let path: std::path::PathBuf = match path {
        Some(p) => p.to_path_buf(),
        None => opencode_auth_path_write(),
    };

    // Read existing file or start with empty object
    let mut auth: serde_json::Value = if path.is_file() {
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("读取 auth.json 失败: {e}"))?;
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };

    // Write: auth[provider] = { "key": key }
    auth[provider] = serde_json::json!({ "key": key });

    // Atomic write: temp file → rename
    let temp_path = path.with_extension("json.tmp");
    let json_str = serde_json::to_string_pretty(&auth)
        .map_err(|e| format!("序列化 auth.json 失败: {e}"))?;
    fs::write(&temp_path, &json_str)
        .map_err(|e| format!("写入临时文件失败: {e}"))?;
    fs::rename(&temp_path, &path)
        .map_err(|e| format!("重命名临时文件失败: {e}"))?;

    Ok(())
}

/// Write a provider API key to the default OpenCode auth.json path.
pub fn write_provider_key(provider: &str, key: &str) -> Result<(), String> {
    write_provider_key_at(provider, key, None)
}

/// List all configured providers in auth.json (without exposing keys).
pub fn list_providers() -> Result<Vec<ProviderInfo>, String> {
    let auth = load_auth_json()?;
    let mut providers = Vec::new();
    if let Some(obj) = auth.as_object() {
        for (key, value) in obj {
            let has_key = value.get("key").and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false)
                || value.get("access").and_then(|v| v.as_str())
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);
            providers.push(ProviderInfo {
                provider: key.clone(),
                label: provider_display_name(key).to_string(),
                has_key,
            });
        }
    }
    Ok(providers)
}
```

- [ ] **Step 1.2: 追加单元测试**

在文件末尾的 `#[cfg(test)] mod tests` 中追加：

```rust
#[test]
fn write_and_read_provider_key() {
    use std::fs;
    use std::path::PathBuf;

    let dir = std::env::temp_dir().join(format!("agentshell_test_auth_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let auth_path = dir.join("auth.json");

    // Write a key
    super::write_provider_key_at("deepseek", "sk-test123", Some(&auth_path)).unwrap();

    // Verify file exists and has correct content
    assert!(auth_path.is_file());
    let content = fs::read_to_string(&auth_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        parsed["deepseek"]["key"].as_str(),
        Some("sk-test123")
    );

    // Write another provider
    super::write_provider_key_at("openrouter", "or-test456", Some(&auth_path)).unwrap();
    let content = fs::read_to_string(&auth_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["deepseek"]["key"].as_str(), Some("sk-test123"));
    assert_eq!(parsed["openrouter"]["key"].as_str(), Some("or-test456"));

    // Clean up
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn write_overwrites_existing_key() {
    use std::fs;
    use std::path::PathBuf;

    let dir = std::env::temp_dir().join(format!("agentshell_test_auth_overwrite_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let auth_path = dir.join("auth.json");

    super::write_provider_key_at("deepseek", "sk-old", Some(&auth_path)).unwrap();
    super::write_provider_key_at("deepseek", "sk-new", Some(&auth_path)).unwrap();

    let content = fs::read_to_string(&auth_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["deepseek"]["key"].as_str(), Some("sk-new"));

    let _ = fs::remove_dir_all(&dir);
}
```

- [ ] **Step 1.3: 运行测试**

```bash
cd src-tauri && cargo test -- write_and_read_provider_key write_overwrites_existing_key --nocapture
```

预期：两个测试通过。

- [ ] **Step 1.4: Commit**

```bash
git add src-tauri/src/provider_usage.rs
git commit -m "feat(backend): add provider key write and list functions"
```

---

### Task 2: 后端 — 注册 Tauri commands

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/main.rs`

- [ ] **Step 2.1: 在 commands.rs 追加两个命令**

在文件末尾（`start_claude_login` 函数之后）追加：

```rust
#[tauri::command(async)]
pub fn save_provider_key(provider: String, key: String) -> Result<(), String> {
    crate::provider_usage::write_provider_key(&provider, &key)
}

#[tauri::command(async)]
pub fn list_providers() -> Result<Vec<crate::provider_usage::ProviderInfo>, String> {
    crate::provider_usage::list_providers()
}
```

- [ ] **Step 2.2: 在 main.rs 注册**

在 `main.rs` 的 `invoke_handler` 列表中添加：

```rust
commands::save_provider_key,
commands::list_providers,
```

与其他命令并列（按字母顺序插到 `commands::scan_project_context` 之前）。

- [ ] **Step 2.3: 验证编译**

```bash
cd src-tauri && cargo build 2>&1 | tail -5
```

预期：编译成功，无 warning。

- [ ] **Step 2.4: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/main.rs
git commit -m "feat(backend): register save_provider_key and list_providers commands"
```

---

### Task 3: 前端 — types.ts + api.ts

**Files:**
- Modify: `src/lib/types.ts`
- Modify: `src/lib/api.ts`

- [ ] **Step 3.1: 追加 ProviderInfo 类型**

在 `src/lib/types.ts` 末尾追加：

```typescript
export type ProviderInfo = {
  provider: string;
  label: string;
  hasKey: boolean;
};
```

- [ ] **Step 3.2: 追加 IPC 调用**

在 `src/lib/api.ts` 末尾追加：

```typescript
import type { ProviderInfo } from "./types";

export async function saveProviderKey(
  provider: string,
  key: string,
): Promise<void> {
  await invoke("save_provider_key", { provider, key });
}

export async function listProviders(): Promise<ProviderInfo[]> {
  return invoke("list_providers");
}
```

注意：`import type { ProviderInfo }` 需要检查文件中是否已有 `import type` 语句。如果已有，合并；如果没有，添加到文件顶部其他 import 附近。

- [ ] **Step 3.3: 验证 TypeScript 编译**

```bash
npx tsc --noEmit 2>&1 | head -20
```

预期：无类型错误。

- [ ] **Step 3.4: Commit**

```bash
git add src/lib/types.ts src/lib/api.ts
git commit -m "feat(frontend): add ProviderInfo type and IPC calls"
```

---

### Task 4: 前端 — recentModels.ts 工具

**Files:**
- Create: `src/lib/recentModels.ts`

- [ ] **Step 4.1: 创建 recentModels.ts**

```typescript
const STORAGE_KEY = "agentshell-recent-models";
const MAX_ENTRIES = 5;

type RecentModelEntry = {
  modelId: string;
  lastUsedAt: number;
};

/** Record a model usage (push to front, deduplicate, trim to MAX_ENTRIES). */
export function recordModelUsage(modelId: string): void {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    const entries: RecentModelEntry[] = raw ? JSON.parse(raw) : [];
    const filtered = entries.filter((e) => e.modelId !== modelId);
    filtered.unshift({ modelId, lastUsedAt: Date.now() });
    localStorage.setItem(STORAGE_KEY, JSON.stringify(filtered.slice(0, MAX_ENTRIES)));
  } catch {
    // localStorage might be disabled or full
  }
}

/** Get recent models that are still in the valid set, sorted by recency. */
export function getRecentModels(
  validModelIds: ReadonlySet<string>,
): { modelId: string; lastUsedAt: number }[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const entries: RecentModelEntry[] = JSON.parse(raw);
    return entries.filter((e) => validModelIds.has(e.modelId));
  } catch {
    return [];
  }
}
```

- [ ] **Step 4.2: Commit**

```bash
git add src/lib/recentModels.ts
git commit -m "feat(frontend): add recentModels utility for localStorage model tracking"
```

---

### Task 5: 前端 — ProviderConfigDialog 组件

**Files:**
- Create: `src/components/ProviderConfigDialog.tsx`

- [ ] **Step 5.1: 创建 ProviderConfigDialog.tsx**

```tsx
import { useState } from "react";
import { saveProviderKey } from "../lib/api";

const PROVIDERS = [
  { id: "deepseek", label: "DeepSeek" },
  { id: "openrouter", label: "OpenRouter" },
  { id: "openai", label: "OpenAI" },
  { id: "anthropic", label: "Anthropic" },
  { id: "google", label: "Google" },
  { id: "xai", label: "xAI (Grok)" },
  { id: "zai", label: "Z.AI (GLM)" },
  { id: "siliconflow", label: "SiliconFlow" },
];

type Props = {
  onClose: () => void;
  onSaved: () => void;
};

export function ProviderConfigDialog({ onClose, onSaved }: Props) {
  const [provider, setProvider] = useState(PROVIDERS[0].id);
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  const handleSave = async () => {
    const trimmed = apiKey.trim();
    if (!trimmed) {
      setError("请输入 API Key");
      return;
    }
    setSaving(true);
    setError("");
    try {
      await saveProviderKey(provider, trimmed);
      onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="project-dialog-backdrop" onClick={onClose}>
      <div className="project-dialog provider-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="project-dialog__header">添加 API Key</div>
        <div className="project-dialog__body">
          <label className="provider-dialog__field">
            <span className="provider-dialog__label">服务商</span>
            <select
              className="provider-dialog__select"
              value={provider}
              onChange={(e) => setProvider(e.target.value)}
              disabled={saving}
            >
              {PROVIDERS.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
            </select>
          </label>
          <label className="provider-dialog__field">
            <span className="provider-dialog__label">API Key</span>
            <input
              className="provider-dialog__input"
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="粘贴你的 API Key"
              disabled={saving}
              autoFocus
            />
          </label>
          {error && <div className="provider-dialog__error">{error}</div>}
        </div>
        <div className="project-dialog__actions">
          <button
            className="project-dialog__cancel"
            type="button"
            onClick={onClose}
            disabled={saving}
          >
            取消
          </button>
          <button
            className="project-dialog__submit"
            type="button"
            onClick={handleSave}
            disabled={saving || !apiKey.trim()}
          >
            {saving ? "保存中…" : "保存并连接"}
          </button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 5.2: Commit**

```bash
git add src/components/ProviderConfigDialog.tsx
git commit -m "feat(frontend): add ProviderConfigDialog component"
```

---

### Task 6: 前端 — Composer 模型选择器改造

**Files:**
- Modify: `src/components/Composer.tsx`

这是最关键的任务。需要：
1. 让模型选择器始终显示（有 caps 时，即使 models 为空）
2. 无模型时显示空状态引导
3. 有模型时底部加「添加 API Key…」入口
4. 顶部加「最近使用」区域
5. 模型切换成功时记录到 localStorage

- [ ] **Step 6.1: 添加 import 和状态**

在 Composer.tsx 顶部 import 区域追加：

```tsx
import { ProviderConfigDialog } from "./ProviderConfigDialog";
import { recordModelUsage, getRecentModels } from "../lib/recentModels";
```

在 Composer 函数组件的 `useState` 区域追加：

```tsx
const [showProviderDialog, setShowProviderDialog] = useState(false);
```

- [ ] **Step 6.2: 修改模型选择器始终显示**

找到条件 `{(hasModels || hasEffort) && (`，改为：

```tsx
{/* Always show when caps exist (even empty for add-key prompt), or has effort */}
{(hasModels || hasEffort || (caps && caps.models.length === 0)) && (
```

- [ ] **Step 6.3: 模型下拉菜单内 — 无模型时显示空状态**

在模型下拉菜单的 JSX 中，当前逻辑是：

```tsx
{menu === "model" && (
  <div className="composer-menu composer-menu--models" ...>
    {hasModels && (
      <>
        {/* search + model list */}
      </>
    )}
    {hasEffort && (...)}
  </div>
)}
```

改为：

```tsx
{menu === "model" && (
  <div className="composer-menu composer-menu--models" ...>
    {!hasModels && caps?.models.length === 0 ? (
      /* ── Empty state: no models ── */
      <div className="composer-menu__empty-state">
        <div className="composer-menu__empty-icon">🔑</div>
        <div className="composer-menu__empty-title">还没有配置 API Key</div>
        <div className="composer-menu__empty-desc">
          选择服务商并输入 Key 即可开始使用
        </div>
        <button
          className="composer-menu__add-key-btn"
          type="button"
          onClick={() => { setMenu(null); setShowProviderDialog(true); }}
        >
          添加 API Key
        </button>
      </div>
    ) : (
      <>
        {hasModels && (
          <>
            {/* ── Recent models ── */}
            {recentModels.length > 0 && (
              <div className="composer-menu__recent">
                <span className="composer-menu__label">最近使用</span>
                {recentModels.map((entry) => {
                  const model = caps?.models.find((m) => m.id === entry.modelId);
                  if (!model) return null;
                  return (
                    <button
                      key={entry.modelId}
                      className={displayModel === entry.modelId ? "is-selected" : ""}
                      type="button"
                      role="option"
                      aria-selected={displayModel === entry.modelId}
                      onClick={() => {
                        const prev = displayModel;
                        pinOptions({ model: entry.modelId });
                        setCurrentModel(entry.modelId);
                        setMenu(null);
                        void commitUpdate({ model: entry.modelId }, () => {
                          unpinOption("model");
                          setCurrentModel(prev);
                        });
                      }}
                    >
                      <span className="composer-menu__model-name">
                        {shortModelName(model)}
                      </span>
                      <span className="composer-menu__model-recent-hint">
                        {formatRecentTime(entry.lastUsedAt)}
                      </span>
                    </button>
                  );
                })}
              </div>
            )}
            {recentModels.length > 0 && <span className="composer-menu__divider" />}

            {/* ── Search ── */}
            <div className="composer-menu__search">
              <Search size={13} aria-hidden />
              <input
                ref={modelSearchRef}
                type="search"
                value={modelQuery}
                placeholder="Search provider or model…"
                aria-label="Search models"
                onChange={(e) => setModelQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    e.stopPropagation();
                    setMenu(null);
                  }
                }}
              />
            </div>
            <div className="composer-menu__scroll">
              {/* ...existing modelGroups rendering... */}
            </div>

            {/* ── Add API Key entry at bottom ── */}
            <span className="composer-menu__divider" />
            <button
              className="composer-menu__add-key-item"
              type="button"
              onClick={() => { setMenu(null); setShowProviderDialog(true); }}
            >
              ＋ 添加 API Key…
            </button>
          </>
        )}
        {hasEffort && (
          {/* ...existing effort section... */}
        )}
      </>
    )}
  </div>
)}
```

需要添加 `formatRecentTime` 辅助函数：

```tsx
function formatRecentTime(timestamp: number): string {
  const diff = Date.now() - timestamp;
  if (diff < 60_000) return "刚刚";
  if (diff < 3600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3600_000)} 小时前`;
  return `${Math.floor(diff / 86_400_000)} 天前`;
}
```

以及计算 `recentModels` 的 useMemo：

```tsx
const recentModels = useMemo(() => {
  if (!caps?.models?.length) return [];
  const validIds = new Set(caps.models.map((m) => m.id));
  return getRecentModels(validIds);
}, [caps?.models]);
```

- [ ] **Step 6.4: 模型切换成功时记录最近使用**

在 `commitUpdate` 函数的成功路径中（`try` 块末尾，`persistPrefs` 调用之前或之后），追加：

```tsx
// 记录最近使用模型
if (typeof patch.model === "string") {
  recordModelUsage(patch.model);
}
```

注意：找到 `commitUpdate` 中 `persistPrefs(prefsPatch)` 的位置，在其附近追加。

- [ ] **Step 6.5: 渲染 ProviderConfigDialog**

在 Composer 的 JSX 末尾（`</footer>` 之前）追加：

```tsx
{showProviderDialog && (
  <ProviderConfigDialog
    onClose={() => setShowProviderDialog(false)}
    onSaved={() => {
      setShowProviderDialog(false);
      // Refresh capabilities to pick up new models
      if (sessionId && !sessionId.startsWith("session-empty-")) {
        loadCapabilities(sessionId);
      }
    }}
  />
)}
```

- [ ] **Step 6.6: 验证编译**

```bash
npx tsc --noEmit 2>&1 | head -30
```

预期：无类型错误。

- [ ] **Step 6.7: Commit**

```bash
git add src/components/Composer.tsx
git commit -m "feat(frontend): update Composer model selector with empty state, recent models, add key entry"
```

---

### Task 7: CSS 样式

**Files:**
- Modify: `src/styles/app.css`

- [ ] **Step 7.1: 追加新 UI 样式**

在 `app.css` 末尾追加：

```css
/* ── Provider Config Dialog ── */
.provider-dialog {
  min-width: 400px;
}

.provider-dialog__field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-bottom: 14px;
}

.provider-dialog__label {
  font-size: 13px;
  font-weight: 500;
  color: var(--text-secondary);
}

.provider-dialog__select,
.provider-dialog__input {
  padding: 8px 10px;
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--surface-secondary);
  color: var(--text-primary);
  font-size: 14px;
  outline: none;
  transition: border-color 0.15s;
}

.provider-dialog__select:focus,
.provider-dialog__input:focus {
  border-color: var(--accent);
}

.provider-dialog__error {
  color: var(--danger);
  font-size: 13px;
  padding: 6px 0;
}

/* ── Model Dropdown Empty State ── */
.composer-menu__empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 24px 16px;
  text-align: center;
}

.composer-menu__empty-icon {
  font-size: 28px;
  line-height: 1;
}

.composer-menu__empty-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
}

.composer-menu__empty-desc {
  font-size: 12px;
  color: var(--text-tertiary);
  margin-bottom: 4px;
}

.composer-menu__add-key-btn {
  padding: 6px 16px;
  border-radius: 6px;
  border: none;
  background: var(--accent);
  color: #fff;
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  transition: opacity 0.15s;
}

.composer-menu__add-key-btn:hover {
  opacity: 0.9;
}

/* ── Recent Models ── */
.composer-menu__recent {
  padding: 4px 0;
}

.composer-menu__recent .composer-menu__label {
  display: block;
  padding: 4px 8px;
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  color: var(--text-tertiary);
}

.composer-menu__recent button {
  display: flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
  padding: 5px 8px;
  border: none;
  background: none;
  color: var(--text-primary);
  font-size: 13px;
  cursor: pointer;
  text-align: left;
  border-radius: 4px;
}

.composer-menu__recent button:hover,
.composer-menu__recent button.is-selected {
  background: var(--hover);
}

.composer-menu__model-recent-hint {
  font-size: 11px;
  color: var(--text-tertiary);
  white-space: nowrap;
  margin-left: 8px;
}

/* ── Add Key item in model list ── */
.composer-menu__add-key-item {
  display: flex;
  align-items: center;
  width: 100%;
  padding: 7px 8px;
  border: none;
  background: none;
  color: var(--accent);
  font-size: 13px;
  cursor: pointer;
  text-align: left;
  border-radius: 4px;
  gap: 4px;
}

.composer-menu__add-key-item:hover {
  background: var(--hover);
}
```

- [ ] **Step 7.2: 验证编译**

```bash
npx tsc --noEmit 2>&1 | head -10
```

预期：无错误。

- [ ] **Step 7.3: Commit**

```bash
git add src/styles/app.css
git commit -m "style: add provider dialog, empty state, recent models, add key styles"
```

---

## 自查清单

1. **Spec 覆盖度：**
   - [x] 无 Key 时显示空状态引导 — Task 6.3
   - [x] 添加 API Key 对话框（选服务商、输入 Key、保存） — Task 5
   - [x] 保存到 OpenCode auth.json — Task 1
   - [x] 保存后刷新模型列表 — Task 6.5
   - [x] 最近使用模型列表（最近 5 个，localStorage） — Task 4 + Task 6.3
   - [x] 有模型时底部「添加 API Key」入口 — Task 6.3
   - [x] 模型切换时记录最近使用 — Task 6.4
   - [x] list_providers 命令 — Task 1 + Task 2
   - [x] ProviderInfo 类型 — Task 3

2. **占位符扫描：** 无 TBD、TODO 或占位符。

3. **类型一致性：** `ProviderInfo` 在 Rust 端和 TypeScript 端的字段一致（provider/label/hasKey），函数签名匹配。
