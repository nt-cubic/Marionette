//! DSH（@openma/deepseek-harness-acp）跨用桥 — llm-pi-ai 托管路由写入器。
//!
//! 2026-10-20 定稿（docs/research-dsh-cross-use.md §8）。DSH 的跨用不是
//! ANTHROPIC env 三件套，而是在 dsh 的 home 层 patch
//! （`~/.dsh/cordis.patch.yml`）挂一条 llm-pi-ai 路由 `marionette`：
//!
//! - `api: openai-completions` + `baseURL: http://127.0.0.1:{port}/v1` 指翻译
//!   引擎 —— 与 OpenCode 的 provider「marionette」同款端点/令牌配方；
//! - 模型 id 用引擎目录原样（斜杠形态安全：适配器 `provider::model` 按首个
//!   `::` 切分，2026-10-20 实测 `marionette::group/auto-claude-haiku-5-5`
//!   原样出现在 session/new 的模型选项里）；
//! - 凭据 `apiKeyEnv: MARIONETTE_API_KEY`：dsh-credentials-local 的解析
//!   顺序是「继承进程 env → `~/.dsh/.credentials.yaml` 的 refs → .env 兜底」，
//!   spawn 注入 env 已够；仍把常量令牌（字面量 `magpie`，回环非机密）合并进
//!   refs —— home patch 是 standalone dsh-acp 与 dsh Web/desktop 每个 profile
//!   共享的唯一用户层，写进 refs 后 dsh Web（没有 Marionette 的 env）也能真实
//!   走这条路由。
//!
//! 写入纪律（对齐 `ensure_grok_model_table` 的立场）：
//! - **文本外科手术，不整档重排**：home patch 允许 `!!js` 标签、注释与用户
//!   手写内容，serde_yaml 整体解析重写会砸掉它们 —— 只动定位到的最小区域
//!   （`config.providers.marionette` 子块 / credentials refs 里的一行）。
//! - **幂等**：产物与原文逐字节一致则零写入。
//! - **看不懂就不写**：文件形态超出预期（非顶层数组 / refs 内联非空 /
//!   credentials 是 pre-release 平铺）→ Err 上报，绝不写半截。桥少一条路由
//!   只是少一批模型，砸了用户文件才是事故。
//! - 引擎停用（Absent/Failed）时**移除**托管路由：dsh 的菜单会把路由广告成
//!   原生模型，留着连不上的死条目比没有更糟（与 grok 解绑清表同理由）。

use std::fs;
use std::path::{Path, PathBuf};

/// 托管路由的 provider 名（llm-pi-ai `config.providers` 的键，也是广告 id
/// `provider::model` 的前缀）。
pub const MARIONETTE_PROVIDER: &str = "marionette";
/// 路由的凭据 ref 名（`apiKeyEnv` 指它；值 = 引擎令牌字面量）。
///
/// **必须是 `<provider>_API_KEY` 形态**：openma dsh-acp 的请求前预检
/// （`credentialEnvNames`）按这个约定自己推 ref 名，**不读路由的
/// apiKeyEnv** ——起名 `MARIONETTE_ENGINE_TOKEN` 时模型广告/切换都正常，
/// 真发消息却在请求前 2ms 被拒 "Authentication required"（2026-10-20 实测
/// 踩坑）。dsh-llm-pi-ai 的路由解析与这个约定殊途同归，一个名字两处都认。
pub const MARIONETTE_TOKEN_ENV: &str = "MARIONETTE_API_KEY";
/// 我们创建的 entry 前置标记行（移除时识别「整个 entry 是我们的」）。
const MANAGED_MARKER: &str = "# managed-by: marionette-bridge";

pub fn dsh_patch_path() -> Option<PathBuf> {
    Some(crate::acp::dsh_home()?.join("cordis.patch.yml"))
}

pub fn dsh_credentials_path() -> Option<PathBuf> {
    Some(crate::acp::dsh_home()?.join(".credentials.yaml"))
}

/// spawn 时刻入口（acp.rs 的 DSH 分支）：按引擎现状收敛托管路由。
///
/// - Running 且目录非空 → 刷新路由 + 令牌 refs → `Ok(true)`（调用方注入 env）；
/// - Absent / Failed → 移除托管路由（dsh 菜单不留死条目）→ `Ok(false)`；
/// - Starting / Downloading / 目录暂空 → 文件不动（过渡态，下一次 spawn 收敛；
///   与 opencode「引擎睡着就不注入」同款降级姿势）。
pub fn refresh_marionette_route() -> Result<bool, String> {
    let patch = dsh_patch_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    let credentials = dsh_credentials_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    match crate::engine::status() {
        crate::engine::EngineStatus::Running { port, .. } => {
            let models = crate::engine::catalog_direct_entries();
            if models.is_empty() {
                return Ok(false);
            }
            ensure_route_at(&patch, &credentials, port, &models)?;
            Ok(true)
        }
        crate::engine::EngineStatus::Starting
        | crate::engine::EngineStatus::Downloading { .. } => Ok(false),
        crate::engine::EngineStatus::Absent | crate::engine::EngineStatus::Failed { .. } => {
            remove_marionette_route_at(&patch)?;
            Ok(false)
        }
    }
}

/// ensure 的本体（路径注入）：刷新 home patch 的 `marionette` 路由与
/// credentials refs。两个文件各自幂等；任一写入失败 → Err（调用方 warn，
/// 本次 spawn 不注入 env）。
fn ensure_route_at(
    patch_path: &Path,
    credentials_path: &Path,
    port: u16,
    models: &[crate::model_bridge::CatalogModelRow],
) -> Result<(), String> {
    if models.is_empty() {
        return Err("引擎目录为空，拒绝写 dsh 路由".to_string());
    }
    // 先令牌后路由：refs 就位后路由才对外可见，dsh Web 不经历「有路由没凭据」。
    let original = read_optional(credentials_path)?;
    let next = merge_credentials_refs(&original)?;
    if next != original {
        crate::model_bridge::write_text_atomic_pub(credentials_path, &next)
            .map_err(|e| format!("写入 dsh .credentials.yaml 失败: {e}"))?;
    }
    let original = read_optional(patch_path)?;
    let next = merge_home_patch(&original, port, models)?;
    if next != original {
        crate::model_bridge::write_text_atomic_pub(patch_path, &next)
            .map_err(|e| format!("写入 cordis.patch.yml 失败: {e}"))?;
    }
    Ok(())
}

fn read_optional(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("读取 {} 失败: {error}", path.display())),
    }
}

// ─── 行级辅助（只在标准 YAML 缩进形态上工作，认不出就 Err/None）─────────────

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

/// 行数组拼回文本（统一收尾换行；首写规范化，其后逐字节幂等）。
fn join_lines(lines: &[String]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn escape_double_quoted(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn token_ref_line(indent: usize) -> String {
    format!(
        "{}{MARIONETTE_TOKEN_ENV}: {}",
        " ".repeat(indent),
        crate::engine::ENGINE_TOKEN
    )
}

/// marionette 路由子块（`indent` = providers 子键的缩进列）。
///
/// 每个模型带 `contextWindow`/`maxTokens`（引擎说了才写）：dsh 的 llm-pi-ai
/// 对缺声明的模型一律落 256K 兜底——1M 的 DeepSeek 会被当成 256K 显示，
/// 自动压缩阈值也跟着错。
fn marionette_lines(
    indent: usize,
    port: u16,
    models: &[crate::model_bridge::CatalogModelRow],
) -> Vec<String> {
    let pad = " ".repeat(indent);
    let pad2 = " ".repeat(indent + 2);
    let pad4 = " ".repeat(indent + 4);
    let pad5 = " ".repeat(indent + 6);
    let mut block = vec![
        format!("{pad}{MARIONETTE_PROVIDER}:"),
        format!("{pad2}displayName: Marionette 模型共享"),
        format!("{pad2}apiKeyEnv: {MARIONETTE_TOKEN_ENV}"),
        format!("{pad2}api: openai-completions"),
        format!("{pad2}baseURL: http://127.0.0.1:{port}/v1"),
        format!("{pad2}models:"),
    ];
    for model in models {
        block.push(format!("{pad4}- id: \"{}\"", escape_double_quoted(&model.id)));
        if model.images {
            block.push(format!("{pad5}input: [text, image]"));
        }
        if model.context_window > 0 {
            block.push(format!("{pad5}contextWindow: {}", model.context_window));
        }
        if model.max_output > 0 {
            block.push(format!("{pad5}maxTokens: {}", model.max_output));
        }
    }
    block
}

/// 我们整段新建的 llm-pi-ai entry（挂在 MANAGED_MARKER 之后）。
fn managed_entry_lines(
    port: u16,
    models: &[crate::model_bridge::CatalogModelRow],
) -> Vec<String> {
    let mut entry = vec![
        "- id: llm-pi-ai".to_string(),
        "  name: \"@deepseek-ai/dsh-llm-pi-ai\"".to_string(),
        "  config:".to_string(),
        "    providers:".to_string(),
    ];
    entry.extend(marionette_lines(6, port, models));
    entry
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
    // id 在后续行：以条目内第一个键行的缩进为子键缩进；第一个键不是 id
    // （patch 行约定 id 在最前）→ 认不出。
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

/// 找最后一个 `id: llm-pi-ai` 条目 → (start, end, 子键缩进)。
fn find_last_llm_entry(lines: &[String]) -> Option<(usize, usize, usize)> {
    for (s, e) in entry_spans(lines).into_iter().rev() {
        if let Some((id, child_indent)) = entry_identity(lines, s, e) {
            if id == "llm-pi-ai" {
                return Some((s, e, child_indent));
            }
        }
    }
    None
}

/// 在 [from, end) 里找缩进恰为 `indent` 的 `key:` 行；遇到更浅的键行即止。
fn find_key_line(
    lines: &[String],
    from: usize,
    end: usize,
    indent: usize,
    key: &str,
) -> Option<usize> {
    for i in from..end {
        if let Some((line_indent, line_key)) = key_of(&lines[i]) {
            if line_indent < indent {
                return None;
            }
            if line_indent == indent && line_key == key {
                return Some(i);
            }
        }
    }
    None
}

/// 一个键行的块尾：空行与**更深缩进**的内容（含深注释）算块内；同缩进/
/// 更浅的键行或浅注释终止块。
fn block_end(lines: &[String], key_line: usize, end: usize, key_indent: usize) -> usize {
    let mut i = key_line + 1;
    while i < end {
        let line = &lines[i];
        if is_blank(line) || indent_of(line) > key_indent {
            i += 1;
            continue;
        }
        break;
    }
    i
}

/// 块内第一个子键的缩进（没有子键 → None）。
fn first_child_indent(lines: &[String], from: usize, end: usize) -> Option<usize> {
    (from..end).find_map(|i| key_of(&lines[i]).map(|(indent, _)| indent))
}

// ─── home patch 合并 ─────────────────────────────────────────────────────────

fn merge_home_patch(
    original: &str,
    port: u16,
    models: &[crate::model_bridge::CatalogModelRow],
) -> Result<String, String> {
    let trimmed = original.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        let mut fresh = vec![
            "# Marionette model-share route (managed block below — regenerated on each dsh spawn while the engine runs)"
                .to_string(),
            MANAGED_MARKER.to_string(),
        ];
        fresh.extend(managed_entry_lines(port, models));
        return Ok(join_lines(&fresh));
    }
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    if !lines.iter().any(|line| is_entry_start(line)) {
        return Err(
            "cordis.patch.yml 不是顶层数组形态（找不到 column-0 的 `- ` 条目）— 拒绝写入"
                .to_string(),
        );
    }
    let Some((s, e, child_indent)) = find_last_llm_entry(&lines) else {
        // 没有 llm-pi-ai 条目：文件尾追加托管 entry（composeEntries 按 id
        // 后写者胜，追加即生效）。
        if !lines.last().is_some_and(|line| is_blank(line)) {
            lines.push(String::new());
        }
        lines.push(MANAGED_MARKER.to_string());
        lines.extend(managed_entry_lines(port, models));
        return Ok(join_lines(&lines));
    };

    if let Some(config_line) = find_key_line(&lines, s + 1, e, child_indent, "config") {
        let config_end = block_end(&lines, config_line, e, child_indent);
        let providers_indent =
            first_child_indent(&lines, config_line + 1, config_end).unwrap_or(child_indent + 2);
        if let Some(providers_line) =
            find_key_line(&lines, config_line + 1, e, providers_indent, "providers")
        {
            let providers_end = block_end(&lines, providers_line, e, providers_indent);
            let marionette_indent = first_child_indent(&lines, providers_line + 1, providers_end)
                .unwrap_or(providers_indent + 2);
            if let Some(marionette_line) =
                find_key_line(&lines, providers_line + 1, e, marionette_indent, MARIONETTE_PROVIDER)
            {
                // 已有托管块：整块替换（含我们写过的模型列表；端口/目录变更
                // 都在新块里）。
                let replace_end = block_end(&lines, marionette_line, e, marionette_indent);
                lines.splice(
                    marionette_line..replace_end,
                    marionette_lines(marionette_indent, port, models),
                );
            } else {
                // 用户已有 providers（如 opencode-go）：块尾插入，别家原样。
                let insert_at = block_end(&lines, providers_line, e, providers_indent);
                let block = marionette_lines(marionette_indent, port, models);
                lines.splice(insert_at..insert_at, block);
            }
        } else {
            // 有 config 无 providers：config 块尾补 providers + marionette。
            let insert_at = block_end(&lines, config_line, e, child_indent);
            let mut addition = vec![format!("{}providers:", " ".repeat(providers_indent))];
            addition.extend(marionette_lines(providers_indent + 2, port, models));
            lines.splice(insert_at..insert_at, addition);
        }
    } else {
        // 条目里连 config 都没有：entry 尾部补齐三层。
        let mut addition = vec![
            format!("{}config:", " ".repeat(child_indent)),
            format!("{}providers:", " ".repeat(child_indent + 2)),
        ];
        addition.extend(marionette_lines(child_indent + 4, port, models));
        lines.splice(e..e, addition);
    }
    Ok(join_lines(&lines))
}

/// 移除托管路由：我们的 entry 整段删（含标记行）；用户的 entry 只删
/// `marionette` 子块（providers 空了落成 `providers: {}`，保持合法 YAML）。
/// 返回是否动了文件。
fn remove_marionette_route_at(patch_path: &Path) -> Result<bool, String> {
    let original = read_optional(patch_path)?;
    if original.trim().is_empty() {
        return Ok(false);
    }
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    let Some((s, e, child_indent)) = find_last_llm_entry(&lines) else {
        return Ok(false);
    };
    let Some(config_line) = find_key_line(&lines, s + 1, e, child_indent, "config") else {
        return Ok(false);
    };
    let config_end = block_end(&lines, config_line, e, child_indent);
    let providers_indent =
        first_child_indent(&lines, config_line + 1, config_end).unwrap_or(child_indent + 2);
    let Some(providers_line) =
        find_key_line(&lines, config_line + 1, e, providers_indent, "providers")
    else {
        return Ok(false);
    };
    let providers_end = block_end(&lines, providers_line, e, providers_indent);
    let Some(marionette_indent) = first_child_indent(&lines, providers_line + 1, providers_end)
    else {
        return Ok(false);
    };
    let Some(marionette_line) =
        find_key_line(&lines, providers_line + 1, e, marionette_indent, MARIONETTE_PROVIDER)
    else {
        return Ok(false);
    };
    let replace_end = block_end(&lines, marionette_line, e, marionette_indent);
    let removed = replace_end - marionette_line;
    lines.drain(marionette_line..replace_end);
    let e = e - removed; // drain 只发生在 entry 内，end 等量左移。

    // providers 还有没有别的子键？
    let providers_end = block_end(&lines, providers_line, e, providers_indent);
    let has_children = first_child_indent(&lines, providers_line + 1, providers_end).is_some();
    if !has_children {
        let entry_is_ours = s > 0 && lines[s - 1].trim() == MANAGED_MARKER;
        if entry_is_ours {
            // 整段删除：标记行 + entry（含它前面的一个分隔空行）。
            let mut drain_from = s - 1;
            if drain_from > 0 && is_blank(&lines[drain_from - 1]) {
                drain_from -= 1;
            }
            lines.drain(drain_from..e);
            // 条目删空了 → `[]` 占位，保持「合法的空 patch」。
            if !lines.iter().any(|line| is_entry_start(line)) {
                lines.push("[]".to_string());
            }
        } else {
            // 用户的 entry：providers 空映射（llm-pi-ai 零路由 = 休眠）。
            lines[providers_line] = format!("{}providers: {{}}", " ".repeat(providers_indent));
        }
    }
    let next = join_lines(&lines);
    if next == original {
        return Ok(false);
    }
    crate::model_bridge::write_text_atomic_pub(patch_path, &next)
        .map_err(|e| format!("写入 cordis.patch.yml 失败: {e}"))?;
    Ok(true)
}

// ─── credentials refs 合并（`version: 1` 文档，只动 refs 里的一行）──────────

fn merge_credentials_refs(original: &str) -> Result<String, String> {
    if original.trim().is_empty() {
        return Ok(join_lines(&[
            "version: 1".to_string(),
            "refs:".to_string(),
            token_ref_line(2),
        ]));
    }
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    let top_key = |line: &str| -> Option<String> {
        let (indent, key) = key_of(line)?;
        (indent == 0).then_some(key)
    };
    if !lines.iter().any(|line| top_key(line).as_deref() == Some("version")) {
        return Err(
            "credentials.yaml 是 pre-release 平铺形态（无 version 键）— 不写入，等 dsh 自己迁移"
                .to_string(),
        );
    }
    let Some(refs_line) = lines.iter().position(|line| top_key(line).as_deref() == Some("refs"))
    else {
        // 没有 refs 段：文档尾补一个（version/records 原样）。
        lines.push("refs:".to_string());
        lines.push(token_ref_line(2));
        return Ok(join_lines(&lines));
    };
    let inline = scalar_value(&lines[refs_line]);
    if !inline.is_empty() && inline != "{}" {
        return Err("refs 是内联非空形态 — 不识别，拒绝写入".to_string());
    }
    if inline == "{}" {
        lines[refs_line] = "refs:".to_string();
        lines.insert(refs_line + 1, token_ref_line(2));
        return Ok(join_lines(&lines));
    }
    // refs 块内：已有我们的 ref → 改值（一致则原文返回，零写入）；没有 →
    // 块尾插入。插入缩进跟随块内既有子键（混缩进会破 YAML）。
    let mut refs_child_indent: Option<usize> = None;
    for i in (refs_line + 1)..lines.len() {
        if is_blank(&lines[i]) || is_comment(&lines[i]) {
            continue;
        }
        let Some((indent, key)) = key_of(&lines[i]) else {
            continue;
        };
        if indent == 0 {
            // 到下一个顶层键：这里就是 refs 块尾。
            let at = refs_child_indent.unwrap_or(2);
            lines.insert(i, token_ref_line(at));
            return Ok(join_lines(&lines));
        }
        if refs_child_indent.is_none() {
            refs_child_indent = Some(indent);
        }
        if key == MARIONETTE_TOKEN_ENV {
            let desired = token_ref_line(indent);
            if lines[i] == desired {
                return Ok(original.to_string());
            }
            lines[i] = desired;
            return Ok(join_lines(&lines));
        }
    }
    let at = refs_child_indent.unwrap_or(2);
    lines.push(token_ref_line(at));
    Ok(join_lines(&lines))
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

    // ── home patch ──────────────────────────────────────────────────────────

    #[test]
    fn creates_fresh_patch_from_missing_file() {
        let dir = temp_dir("fresh");
        let patch = dir.join("cordis.patch.yml");
        ensure_route_at(&patch, &dir.join(".credentials.yaml"), 3425, &models()).unwrap();
        let text = fs::read_to_string(&patch).unwrap();
        assert!(text.contains(MANAGED_MARKER));
        assert!(text.contains("- id: llm-pi-ai"));
        assert!(text.contains("baseURL: http://127.0.0.1:3425/v1"));
        assert!(text.contains("- id: \"group/auto-claude-haiku-5-5\""));
        assert!(text.contains("apiKeyEnv: MARIONETTE_API_KEY"));
        // 窗口/输出上限跟着模型走：引擎说了的写、没说的不写（llm-pi-ai 兜底 256K）。
        assert!(text.contains("contextWindow: 500000"));
        assert!(text.contains("maxTokens: 250000"));
        assert_eq!(
            text.matches("contextWindow:").count(),
            1,
            "只有引擎披露窗口的模型才写 contextWindow"
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// 引擎目录标了 text+image 的模型必须带 `input: [text, image]`——pi-ai 对
    /// 自定义路由默认 `input: [text]`，DSH 会据此把附图换成
    /// 「image omitted because this model accepts text only」占位文本；纯文本
    /// 模型不写（否则图会打到上游换回 400，而不是被 DSH 就地省略）。
    #[test]
    fn writes_image_capability_only_for_vision_models() {
        let dir = temp_dir("vision");
        let patch = dir.join("cordis.patch.yml");
        ensure_route_at(&patch, &dir.join(".credentials.yaml"), 3425, &models()).unwrap();
        let text = fs::read_to_string(&patch).unwrap();
        assert_eq!(
            text.matches("input: [text, image]").count(),
            1,
            "只有引擎标了 image 的模型才写 input"
        );
        assert!(
            text.contains(
                "          - id: \"opencode-go/grok-4.7\"\n            input: [text, image]\n"
            ),
            "vision model 要声明 input 且与 id 同列缩进:\n{text}"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn merge_is_idempotent_and_tracks_port_changes() {
        let dir = temp_dir("idem");
        let patch = dir.join("cordis.patch.yml");
        let creds = dir.join(".credentials.yaml");
        ensure_route_at(&patch, &creds, 3425, &models()).unwrap();
        let first = fs::read_to_string(&patch).unwrap();
        // 同参数重跑 → 零写入（字节一致）。
        ensure_route_at(&patch, &creds, 3425, &models()).unwrap();
        assert_eq!(fs::read_to_string(&patch).unwrap(), first);
        // 端口变了 → baseURL 换新，其余不动。
        ensure_route_at(&patch, &creds, 3499, &models()).unwrap();
        let second = fs::read_to_string(&patch).unwrap();
        assert!(second.contains("http://127.0.0.1:3499/v1"));
        assert!(!second.contains("3425"));
        fs::remove_dir_all(&dir).ok();
    }

    /// 用户已有的 llm-pi-ai entry（Web 侧手写 opencode-go 的形态）：只添
    /// marionette 子块，opencode-go 与其余行逐字保留。
    #[test]
    fn merges_into_user_llm_entry_preserving_other_providers() {
        let user = r#"# user's own patch
- id: ui-settings-general
  name: "@deepseek-ai/dsh-client-ui-settings-general"
  config:
    welcomeNoticeVersion: 2026-09-28.1
- id: llm-pi-ai
  name: "@deepseek-ai/dsh-llm-pi-ai"
  config:
    providers:
      opencode-go:
        apiKeyEnv: OPENCODE_GO_API_KEY
- id: agent-default-model
  name: "@deepseek-ai/dsh-agent-default-model"
  config:
    provider: opencode-go
"#;
        let merged = merge_home_patch(user, 3425, &models()).unwrap();
        // opencode-go 原样保留，且仍在 marionette 之前。
        let go = merged.find("opencode-go:").unwrap();
        let mar = merged.find("      marionette:").unwrap();
        assert!(go < mar, "用户 provider 应原位保留");
        assert!(merged.contains("apiKeyEnv: OPENCODE_GO_API_KEY"));
        assert!(merged.contains("provider: opencode-go"));
        assert!(merged.contains("displayName: Marionette 模型共享"));
        // 幂等。
        assert_eq!(merge_home_patch(&merged, 3425, &models()).unwrap(), merged);
        // 移除后 opencode-go 仍在（只删 marionette 子块，providers 非空）。
        let dir = temp_dir("user_entry");
        let patch = dir.join("cordis.patch.yml");
        fs::write(&patch, &merged).unwrap();
        assert!(remove_marionette_route_at(&patch).unwrap());
        let after = fs::read_to_string(&patch).unwrap();
        assert!(!after.contains("marionette"));
        assert!(after.contains("apiKeyEnv: OPENCODE_GO_API_KEY"));
        fs::remove_dir_all(&dir).ok();
    }

    /// 只有别的条目、没有 llm-pi-ai：尾部追加托管 entry，其余行不动。
    #[test]
    fn appends_entry_when_llm_row_absent() {
        let user = r#"# keep me: !!js weirdness is fine
- id: timer
  name: '@deepseek-ai/cordis-plugin-timer'
"#;
        let merged = merge_home_patch(user, 3425, &models()).unwrap();
        assert!(merged.starts_with("# keep me"));
        assert!(merged.contains("!!js weirdness"));
        assert!(merged.contains("- id: timer"));
        assert!(merged.contains(MANAGED_MARKER));
        assert!(merged.contains("- id: llm-pi-ai"));
        // 移除 → 托管 entry 整段消失，用户内容保留。
        let dir = temp_dir("append");
        let patch = dir.join("cordis.patch.yml");
        fs::write(&patch, &merged).unwrap();
        assert!(remove_marionette_route_at(&patch).unwrap());
        let after = fs::read_to_string(&patch).unwrap();
        assert!(!after.contains("llm-pi-ai"));
        assert!(after.contains("- id: timer"));
        assert!(after.contains("!!js weirdness"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_non_array_patch() {
        let error = merge_home_patch("version: 1\nrefs: {}\n", 3425, &models());
        assert!(error.is_err());
    }

    // ── credentials ─────────────────────────────────────────────────────────

    #[test]
    fn credentials_merge_preserves_user_refs() {
        let user = r#"version: 1
records:
  client-connection/browser-session:
    kind: browser
refs:
  OPENCODE_GO_API_KEY: sk-user-secret
"#;
        let merged = merge_credentials_refs(user).unwrap();
        assert!(merged.contains("OPENCODE_GO_API_KEY: sk-user-secret"));
        assert!(merged.contains("  MARIONETTE_API_KEY: magpie"));
        // 幂等：再跑一遍字节不变。
        assert_eq!(merge_credentials_refs(&merged).unwrap(), merged);
    }

    #[test]
    fn credentials_creates_and_replaces_token() {
        // 缺失 → 最小 v1 文档。
        let fresh = merge_credentials_refs("").unwrap();
        assert_eq!(
            fresh,
            "version: 1\nrefs:\n  MARIONETTE_API_KEY: magpie\n"
        );
        // 已有旧值（假想的历史令牌）→ 改成引擎令牌。
        let stale = "version: 1\nrefs:\n  MARIONETTE_API_KEY: old\n";
        let merged = merge_credentials_refs(stale).unwrap();
        assert!(merged.contains("MARIONETTE_API_KEY: magpie"));
        assert!(!merged.contains("old"));
    }

    #[test]
    fn credentials_rejects_flat_layout() {
        assert!(merge_credentials_refs("OPENCODE_GO_API_KEY: sk-x\n").is_err());
    }

    #[test]
    fn credentials_appends_refs_section_when_absent() {
        let user = "version: 1\nrecords: {}\n";
        let merged = merge_credentials_refs(user).unwrap();
        assert!(merged.contains("records: {}"));
        assert!(merged.contains("refs:\n  MARIONETTE_API_KEY: magpie"));
    }
}
