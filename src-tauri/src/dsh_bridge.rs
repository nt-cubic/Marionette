//! DSH 跨用桥 —— 用「本次 spawn 的 bundle 层」把 llm-pi-ai 路由 `marionette`
//! 交给 `dsh-acp`（2026-10-22 改版，旧 home patch 方案见
//! `docs/research-dsh-cross-use.md` §8.7）。
//!
//! 为什么不再写 `~/.dsh/cordis.patch.yml`：
//!
//! - dsh 的 patch 层序是 bundle → profile patch → **home patch** → CLI overlay，
//!   同 id 条目的 `config` 是**整条替换**（`cordis-plugin-include` 的
//!   `target[key] = value`）。home 层一旦声明 `llm-pi-ai`，profile 层的同名
//!   `config`（用户在 dsh Web 里配的 provider）就被整条盖掉；
//! - dsh 的 config editor 保存前会把「叠加 home 后的组合值」和表单值做
//!   deep-equal，不等就拒绝落盘（报 "overridden by a home patch or
//!   command-line overlay"）——于是 Web 的「添加提供商」整个失效。
//!
//! 现在改为：每次 spawn 时生成一个最小 bundle（`package.json` 声明
//! `dsh.bundle.patch`，`cordis.patch.yml` 只含 `providers.marionette`），用
//! `dsh-acp --bundle <dir>` 注入。适配器的层序（`bootAcpProfile`）把 bundle
//! 层放在 home patch 之前、且只作用于本次进程——不写任何全局配置文件，
//! dsh Web / desktop 的 `llm-pi-ai` 保持完全可编辑。
//!
//! 代价（已确认接受）：dsh Web / desktop 看不到 `marionette::*` 模型；这些
//! 模型只服务 Marionette 自己拉起的 DSH 会话。
//!
//! 纪律：
//! - bundle 内容由引擎目录确定性生成，逐字节幂等（相同输入零写入）；
//! - 引擎不在跑（Absent/Failed）就删掉 bundle，不留连不上的死路由；
//! - 旧版写进 home patch 的托管块由 [`migrate_legacy_home_patch`] 一次性清掉
//!   （先备份原文件）；dsh-acp 的默认模型若指向已消失的 provider 也一并改指
//!   `marionette`。

use std::fs;
use std::path::{Path, PathBuf};

pub const MARIONETTE_PROVIDER: &str = "marionette";
/// 路由的凭据 ref 名（`apiKeyEnv` 指它；值 = 引擎令牌字面量）。
///
/// **必须是 `<provider>_API_KEY` 形态**：openma dsh-acp 的请求前预检
/// （`credentialEnvNames`）按这个约定自己推 ref 名，**不读路由的
/// apiKeyEnv** ——起名 `MARIONETTE_ENGINE_TOKEN` 时模型广告/切换都正常，
/// 真发消息却在请求前 2ms 被拒 "Authentication required"（2026-10-20 实测
/// 踩坑）。dsh-llm-pi-ai 的路由解析与这个约定殊途同归，一个名字两处都认。
///
/// 现在只靠 spawn 注入的进程 env 解析（env 优先于 `~/.dsh/.credentials.yaml`），
/// 不再往用户的凭据文件里写 ref。
pub const MARIONETTE_TOKEN_ENV: &str = "MARIONETTE_API_KEY";

/// bundle 目录名（`~/.marionette/dsh-acp-bundle`）。
const BUNDLE_DIR_NAME: &str = "dsh-acp-bundle";
const BUNDLE_MANIFEST: &str = "package.json";
const BUNDLE_PATCH: &str = "cordis.patch.yml";

/// 旧 home patch 方案留下的标记行。
const LEGACY_MARKER: &str = "# managed-by: marionette-bridge";
/// 旧方案的说明注释（紧挨标记行，同属托管块）。
const LEGACY_HEADER_PREFIX: &str = "# Marionette model-share route";

/// dsh-acp 持久化的默认模型（provider/model 分开存）。
const STANDALONE_MODEL_FILE: &str = "acp-standalone-model.json";

const MANIFEST_JSON: &str = r#"{
  "name": "marionette-dsh-acp-bundle",
  "version": "0.0.0",
  "private": true,
  "dsh": {
    "bundle": {
      "patch": "./cordis.patch.yml"
    }
  }
}
"#;

pub fn bundle_dir() -> Option<PathBuf> {
    Some(crate::app_paths::global_dir().ok()?.join(BUNDLE_DIR_NAME))
}

pub fn dsh_patch_path() -> Option<PathBuf> {
    Some(crate::acp::dsh_home()?.join("cordis.patch.yml"))
}

pub fn dsh_standalone_model_path() -> Option<PathBuf> {
    Some(crate::acp::dsh_home()?.join(STANDALONE_MODEL_FILE))
}

/// spawn 时刻入口（acp.rs 的 DSH 分支）。
///
/// - 引擎 Running 且目录非空 → 重写 bundle → `Ok(Some(dir))`（调用方加
///   `--bundle <dir>` 并注入 [`MARIONETTE_TOKEN_ENV`]）；
/// - 引擎 Absent/Failed → 删 bundle（不留连不上的死路由）→ `Ok(None)`；
/// - Starting/Downloading 或目录暂空 → 文件不动，`Ok(None)`。
pub fn refresh_marionette_route() -> Result<Option<PathBuf>, String> {
    let dir = bundle_dir().ok_or_else(|| "无法定位用户目录".to_string())?;
    match crate::engine::status() {
        crate::engine::EngineStatus::Running { port, .. } => {
            let models = crate::engine::catalog_direct_entries();
            if models.is_empty() {
                return Ok(None);
            }
            ensure_bundle_at(&dir, port, &models)?;
            // 旧 home patch 若还残留（启动迁移失败/旧版本又写过一次），顺手清掉。
            if let Err(error) = migrate_legacy_home_patch() {
                crate::debug_log::append(
                    "bridge",
                    "warn",
                    "",
                    "legacy dsh home-patch cleanup failed",
                    Some(&error),
                );
            }
            match migrate_standalone_default_model(&models) {
                Ok(true) => crate::debug_log::append(
                    "bridge",
                    "info",
                    "",
                    "dsh-acp default model re-pointed at marionette route",
                    None,
                ),
                Ok(false) => {}
                Err(error) => crate::debug_log::append(
                    "bridge",
                    "warn",
                    "",
                    "dsh-acp default model migration failed",
                    Some(&error),
                ),
            }
            Ok(Some(dir))
        }
        crate::engine::EngineStatus::Starting
        | crate::engine::EngineStatus::Downloading { .. } => Ok(None),
        crate::engine::EngineStatus::Absent | crate::engine::EngineStatus::Failed { .. } => {
            remove_bundle_at(&dir)?;
            Ok(None)
        }
    }
}

// ─── bundle 生成 ─────────────────────────────────────────────────────────────

fn ensure_bundle_at(
    dir: &Path,
    port: u16,
    models: &[crate::model_bridge::CatalogModelRow],
) -> Result<(), String> {
    if models.is_empty() {
        return Err("引擎目录为空，拒绝写 dsh 路由".to_string());
    }
    fs::create_dir_all(dir)
        .map_err(|e| format!("创建 bundle 目录 {} 失败: {e}", dir.display()))?;
    write_if_changed(&dir.join(BUNDLE_MANIFEST), MANIFEST_JSON)?;
    write_if_changed(&dir.join(BUNDLE_PATCH), &patch_text(port, models))?;
    Ok(())
}

fn write_if_changed(path: &Path, text: &str) -> Result<(), String> {
    match fs::read_to_string(path) {
        Ok(current) if current == text => Ok(()),
        _ => crate::model_bridge::write_text_atomic_pub(path, text)
            .map_err(|e| format!("写入 {} 失败: {e}", path.display())),
    }
}

fn remove_bundle_at(dir: &Path) -> Result<(), String> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("删除 bundle 目录 {} 失败: {error}", dir.display())),
    }
}

/// bundle 的 patch 文本。
///
/// 行里**故意不写 `name`**：patch 按 `id` 定位既有条目，而适配器的
/// `rewriteBundleRows` 会把任何非绝对 `name` 当成 bundle 自己的插件去
/// `require.resolve`，解析不到就抛错。
fn patch_text(port: u16, models: &[crate::model_bridge::CatalogModelRow]) -> String {
    let mut lines = vec![
        "# Generated by Marionette for one `dsh-acp --bundle` spawn — do not edit."
            .to_string(),
        "# Regenerated on every spawn; the directory is removed while the engine is not running."
            .to_string(),
        "# No `name`: this row targets the existing llm-pi-ai entry by id, and the adapter"
            .to_string(),
        "# would try to require.resolve a name against this bundle's package.json."
            .to_string(),
        "- id: llm-pi-ai".to_string(),
        "  config:".to_string(),
        "    providers:".to_string(),
        format!("      {MARIONETTE_PROVIDER}:"),
        "        displayName: Marionette 模型共享".to_string(),
        format!("        apiKeyEnv: {MARIONETTE_TOKEN_ENV}"),
        "        api: openai-completions".to_string(),
        format!("        baseURL: http://127.0.0.1:{port}/v1"),
        "        models:".to_string(),
    ];
    for model in models {
        lines.push(format!(
            "          - id: \"{}\"",
            escape_double_quoted(&model.id)
        ));
        if model.images {
            lines.push("            input: [text, image]".to_string());
        }
        if model.context_window > 0 {
            lines.push(format!("            contextWindow: {}", model.context_window));
        }
        if model.max_output > 0 {
            lines.push(format!("            maxTokens: {}", model.max_output));
        }
    }
    join_lines(&lines)
}

fn escape_double_quoted(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn join_lines(lines: &[String]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

// ─── 旧 home patch 迁移 ──────────────────────────────────────────────────────

/// 一次性迁移：把旧版写进 `~/.dsh/cordis.patch.yml` 的托管块整段删掉。
///
/// 只删「紧邻 [`LEGACY_MARKER`] 且 id 为 llm-pi-ai」的条目；用户自己写的
/// llm-pi-ai 条目（没有标记行）一律不动。删之前把原文件备份为
/// `cordis.patch.yml.marionette-bak`，因为托管块里可能被用户手加过 provider。
pub fn migrate_legacy_home_patch() -> Result<bool, String> {
    match dsh_patch_path() {
        Some(path) => migrate_legacy_patch_at(&path),
        None => Ok(false),
    }
}

fn migrate_legacy_patch_at(path: &Path) -> Result<bool, String> {
    let original = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("读取 {} 失败: {error}", path.display())),
    };
    let Some(next) = strip_managed_block(&original) else {
        return Ok(false);
    };
    let backup = path.with_extension("marionette-bak");
    fs::write(&backup, &original)
        .map_err(|e| format!("备份 {} 失败: {e}", backup.display()))?;
    crate::model_bridge::write_text_atomic_pub(path, &next)
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    crate::debug_log::append(
        "bridge",
        "info",
        "",
        "removed legacy llm-pi-ai block from dsh home patch",
        Some(&backup.display().to_string()),
    );
    Ok(true)
}

/// 文本外科：删掉带标记的 llm-pi-ai 条目（含说明注释与相邻空行）。
/// 没有可删内容 → None；产物始终是可加载的 patch 文档。
fn strip_managed_block(original: &str) -> Option<String> {
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    let spans = entry_spans(&lines);
    let mut removed = false;
    for (start, end) in spans.into_iter().rev() {
        // 标记行：向上跳过空行后的第一个非空行。
        let mut marker = start;
        while marker > 0 && is_blank(&lines[marker - 1]) {
            marker -= 1;
        }
        if marker == 0 || lines[marker - 1].trim() != LEGACY_MARKER {
            continue;
        }
        let Some((id, _)) = entry_identity(&lines, start, end) else {
            continue;
        };
        if id != "llm-pi-ai" {
            continue;
        }
        let mut from = marker - 1;
        if from > 0 && lines[from - 1].trim_start().starts_with(LEGACY_HEADER_PREFIX) {
            from -= 1;
        }
        while from > 0 && is_blank(&lines[from - 1]) {
            from -= 1;
        }
        lines.drain(from..end);
        removed = true;
    }
    if !removed {
        return None;
    }
    while lines.first().is_some_and(|line| is_blank(line)) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| is_blank(line)) {
        lines.pop();
    }
    if !lines.iter().any(|line| is_entry_start(line)) {
        return Some("[]\n".to_string());
    }
    Some(join_lines(&lines))
}

// ─── dsh-acp 默认模型迁移 ────────────────────────────────────────────────────

/// 路由从 home 层挪走后，`acp-standalone-model.json` 里指向 `opencode-go`
/// 之类的默认模型会指向一个不存在的 provider；改指 `marionette` 路由上等价
/// 的引擎模型 id（`<provider>/<model>`），没有等价项就落目录第一个。
fn migrate_standalone_default_model(
    models: &[crate::model_bridge::CatalogModelRow],
) -> Result<bool, String> {
    match dsh_standalone_model_path() {
        Some(path) => migrate_standalone_model_at(&path, models),
        None => Ok(false),
    }
}

fn migrate_standalone_model_at(
    path: &Path,
    models: &[crate::model_bridge::CatalogModelRow],
) -> Result<bool, String> {
    if models.is_empty() {
        return Ok(false);
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("读取 {} 失败: {error}", path.display())),
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(false);
    };
    let provider = value
        .get("provider")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_string();
    let model = value
        .get("model")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_string();
    if model.is_empty() {
        return Ok(false);
    }
    let routes_marionette = provider == MARIONETTE_PROVIDER
        && models.iter().any(|row| row.id == model);
    let routes_native = provider == "deepseek-official" || provider == "deepseek";
    if routes_marionette || routes_native {
        return Ok(false);
    }
    let candidate = format!("{provider}/{model}");
    let target = if models.iter().any(|row| row.id == candidate) {
        candidate
    } else {
        models[0].id.clone()
    };
    value["provider"] = serde_json::Value::String(MARIONETTE_PROVIDER.to_string());
    value["model"] = serde_json::Value::String(target);
    let next = serde_json::to_string(&value)
        .map_err(|e| format!("序列化 {STANDALONE_MODEL_FILE} 失败: {e}"))?;
    if next == text {
        return Ok(false);
    }
    crate::model_bridge::write_text_atomic_pub(path, &next)
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    Ok(true)
}

// ─── 行级辅助（只在标准 YAML 缩进形态上工作，认不出就跳过）───────────────────

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// 顶层数组条目边界：column 0 的 `- ` / `-`。
fn is_entry_start(line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed == "-" || trimmed.starts_with("- ")
}

/// `key: value` 行 → (缩进, key)；空行 / 注释 / `- ` 列表行 → None。
fn key_of(line: &str) -> Option<(usize, String)> {
    if is_blank(line) || is_comment(line) || line.trim_start().starts_with("- ") {
        return None;
    }
    let indent = indent_of(line);
    let body = line.trim_start();
    let (key, _) = body.split_once(':')?;
    let key = key.trim().trim_matches(|c| c == '\'' || c == '"');
    if key.is_empty() {
        None
    } else {
        Some((indent, key.to_string()))
    }
}

/// `key: value` 冒号后的标量值（去引号 / 去尾注释）。
fn scalar_value(line: &str) -> String {
    let body = line.trim_start();
    let (_, rest) = body.split_once(':').unwrap_or(("", body));
    let mut value = rest.trim();
    if let Some(hash) = value.find(" #") {
        value = value[..hash].trim();
    }
    value.trim_matches(|c| c == '\'' || c == '"').to_string()
}

/// 定位全部顶层条目 (start, end)。
fn entry_spans(lines: &[String]) -> Vec<(usize, usize)> {
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_entry_start(line))
        .map(|(i, _)| i)
        .collect();
    let mut spans = Vec::with_capacity(starts.len());
    for (idx, &start) in starts.iter().enumerate() {
        let end = starts.get(idx + 1).copied().unwrap_or(lines.len());
        spans.push((start, end));
    }
    spans
}

/// 条目的 id 与子键缩进。`- id: x` 在条目首行，或 `-` 后另起 `id:` 行。
fn entry_identity(lines: &[String], start: usize, end: usize) -> Option<(String, usize)> {
    let dash_body = lines[start].trim_start().strip_prefix('-')?.trim_start();
    if let Some((key, _)) = dash_body.split_once(':') {
        if key.trim() == "id" {
            return Some((scalar_value(&lines[start]), 2));
        }
    }
    for i in (start + 1)..end {
        if let Some((indent, key)) = key_of(&lines[i]) {
            if key == "id" {
                return Some((scalar_value(&lines[i]), indent));
            }
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_dsh_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn models() -> Vec<crate::model_bridge::CatalogModelRow> {
        use crate::model_bridge::CatalogModelRow;
        vec![
            CatalogModelRow {
                id: "group/auto-claude-haiku-5-5".to_string(),
                context_window: 0,
                max_output: 0,
                images: false,
            },
            CatalogModelRow {
                id: "opencode-go/grok-4.7".to_string(),
                context_window: 500_000,
                max_output: 250_000,
                images: true,
            },
            CatalogModelRow {
                id: "deepseek/deepseek-flash".to_string(),
                // 引擎没说窗口（0）→ 不写 contextWindow，dsh 落自己的兜底。
                context_window: 0,
                max_output: 0,
                images: false,
            },
        ]
    }

    // ── bundle ──────────────────────────────────────────────────────────────

    #[test]
    fn writes_bundle_manifest_and_patch() {
        let dir = temp_dir("bundle").join(BUNDLE_DIR_NAME);
        ensure_bundle_at(&dir, 3425, &models()).unwrap();
        let manifest = fs::read_to_string(dir.join(BUNDLE_MANIFEST)).unwrap();
        assert!(manifest.contains("\"dsh\""));
        assert!(manifest.contains("./cordis.patch.yml"));
        let patch = fs::read_to_string(dir.join(BUNDLE_PATCH)).unwrap();
        assert!(patch.contains("- id: llm-pi-ai"));
        assert!(patch.contains("      marionette:"));
        assert!(patch.contains("baseURL: http://127.0.0.1:3425/v1"));
        assert!(patch.contains("apiKeyEnv: MARIONETTE_API_KEY"));
        assert!(patch.contains("- id: \"group/auto-claude-haiku-5-5\""));
        assert!(patch.contains("contextWindow: 500000"));
        assert!(patch.contains("maxTokens: 250000"));
        assert_eq!(
            patch.matches("contextWindow:").count(),
            1,
            "只有引擎披露窗口的模型才写 contextWindow"
        );
        // 不写 name：适配器会拿它去 require.resolve。
        assert!(
            !patch.lines().any(|line| line.trim_start().starts_with("name:")),
            "bundle patch 不应声明 name:\n{patch}"
        );
    }

    /// 引擎目录标了 text+image 的模型必须带 `input: [text, image]`——pi-ai 对
    /// 自定义路由默认 `input: [text]`，DSH 会据此把附图换成占位文本。
    #[test]
    fn writes_image_capability_only_for_vision_models() {
        let dir = temp_dir("vision").join(BUNDLE_DIR_NAME);
        ensure_bundle_at(&dir, 3425, &models()).unwrap();
        let patch = fs::read_to_string(dir.join(BUNDLE_PATCH)).unwrap();
        assert_eq!(patch.matches("input: [text, image]").count(), 1);
        assert!(patch.contains(
            "          - id: \"opencode-go/grok-4.7\"\n            input: [text, image]\n"
        ));
    }

    #[test]
    fn bundle_is_idempotent_and_tracks_port_changes() {
        let dir = temp_dir("idem").join(BUNDLE_DIR_NAME);
        ensure_bundle_at(&dir, 3425, &models()).unwrap();
        let patch = dir.join(BUNDLE_PATCH);
        let first = fs::read_to_string(&patch).unwrap();
        ensure_bundle_at(&dir, 3425, &models()).unwrap();
        assert_eq!(fs::read_to_string(&patch).unwrap(), first);
        ensure_bundle_at(&dir, 3499, &models()).unwrap();
        let second = fs::read_to_string(&patch).unwrap();
        assert!(second.contains("http://127.0.0.1:3499/v1"));
        assert!(!second.contains("3425"));
    }

    #[test]
    fn removes_bundle_directory() {
        let dir = temp_dir("remove").join(BUNDLE_DIR_NAME);
        ensure_bundle_at(&dir, 3425, &models()).unwrap();
        assert!(dir.join(BUNDLE_PATCH).exists());
        remove_bundle_at(&dir).unwrap();
        assert!(!dir.exists());
        // 已不存在 → 静默成功（引擎停在 Absent 反复 spawn 的路径）。
        remove_bundle_at(&dir).unwrap();
    }

    // ── 旧 home patch 迁移 ──────────────────────────────────────────────────

    #[test]
    fn migration_strips_managed_block_and_keeps_user_rows() {
        let dir = temp_dir("legacy");
        let patch = dir.join("cordis.patch.yml");
        fs::write(
            &patch,
            r#"# Marionette model-share route (managed block below — regenerated on each dsh spawn while the engine runs)
# managed-by: marionette-bridge
- id: llm-pi-ai
  name: "@deepseek-ai/dsh-llm-pi-ai"
  config:
    providers:
      marionette:
        apiKeyEnv: MARIONETTE_API_KEY
        baseURL: http://127.0.0.1:3425/v1
      opencode-go:
        apiKeyEnv: OPENCODE_GO_API_KEY
- id: ui-theme
  name: "@deepseek-ai/dsh-client-ui-theme"
  config:
    preference: light
"#,
        )
        .unwrap();
        assert!(migrate_legacy_patch_at(&patch).unwrap());
        let after = fs::read_to_string(&patch).unwrap();
        assert!(!after.contains("llm-pi-ai"));
        assert!(!after.contains(LEGACY_MARKER));
        assert!(after.contains("- id: ui-theme"));
        assert!(after.contains("preference: light"));
        // 原文件留档（托管块里可能被用户手加过 provider）。
        let backup = path_text(&patch.with_extension("marionette-bak"));
        assert!(backup.contains("opencode-go"));
        // 幂等：再跑一遍不动文件。
        assert!(!migrate_legacy_patch_at(&patch).unwrap());
    }

    #[test]
    fn migration_removes_only_marked_rows() {
        let dir = temp_dir("legacy_hand");
        let patch = dir.join("cordis.patch.yml");
        let user_owned = r#"- id: llm-pi-ai
  name: "@deepseek-ai/dsh-llm-pi-ai"
  config:
    providers:
      opencode-go:
        apiKeyEnv: OPENCODE_GO_API_KEY
"#;
        fs::write(&patch, user_owned).unwrap();
        assert!(!migrate_legacy_patch_at(&patch).unwrap());
        assert_eq!(fs::read_to_string(&patch).unwrap(), user_owned);
    }

    #[test]
    fn migration_leaves_empty_patch_placeholder() {
        let dir = temp_dir("legacy_empty");
        let patch = dir.join("cordis.patch.yml");
        fs::write(
            &patch,
            "# Marionette model-share route (managed)\n# managed-by: marionette-bridge\n- id: llm-pi-ai\n  config:\n    providers:\n      marionette:\n        apiKeyEnv: MARIONETTE_API_KEY\n",
        )
        .unwrap();
        assert!(migrate_legacy_patch_at(&patch).unwrap());
        assert_eq!(fs::read_to_string(&patch).unwrap(), "[]\n");
    }

    fn path_text(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    // ── 默认模型迁移 ────────────────────────────────────────────────────────

    fn write_model(path: &Path, body: &str) {
        fs::write(path, body).unwrap();
    }

    #[test]
    fn model_migration_repoints_removed_route() {
        let dir = temp_dir("model");
        let path = dir.join(STANDALONE_MODEL_FILE);
        write_model(
            &path,
            r#"{"provider":"opencode-go","model":"grok-4.7","reasoningEffort":"high"}"#,
        );
        assert!(migrate_standalone_model_at(&path, &models()).unwrap());
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["provider"], "marionette");
        assert_eq!(value["model"], "opencode-go/grok-4.7");
        assert_eq!(value["reasoningEffort"], "high");
        // 已经是 marionette 的等价模型 → 再跑不动文件。
        assert!(!migrate_standalone_model_at(&path, &models()).unwrap());
    }

    #[test]
    fn model_migration_keeps_native_and_falls_back() {
        let dir = temp_dir("model_native");
        let native = dir.join("native.json");
        write_model(&native, r#"{"provider":"deepseek","model":"deepseek-flash"}"#);
        assert!(!migrate_standalone_model_at(&native, &models()).unwrap());

        let fallback = dir.join("fallback.json");
        write_model(
            &fallback,
            r#"{"provider":"opencode-go","model":"unknown-model"}"#,
        );
        // 引擎目录里没有 group/*（catalog_direct_entries 已过滤），这里也照着来。
        let catalog: Vec<_> = models()
            .into_iter()
            .filter(|row| !row.id.starts_with("group/"))
            .collect();
        assert!(migrate_standalone_model_at(&fallback, &catalog).unwrap());
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&fallback).unwrap()).unwrap();
        assert_eq!(value["provider"], "marionette");
        // 没有等价 id → 目录第一个。
        assert_eq!(value["model"], "opencode-go/grok-4.7");
    }
}
