//! 跨 agent 模型桥 — L1 注入链（docs/cross-agent-bridge.md §4.3/§4.4/§4.8）。
//!
//! 凭据层（扫描 / DPAPI 存储）在 [`crate::credentials`]，本模块在其上补齐
//! M1 的接线：
//!
//! - **presets 表**：preset id → 双端点（anthropic 兼容给 claude-code 用 env、
//!   OpenAI chat 兼容给 grok 用 owned 表）。端点逐条经官方文档 / 实测核实，
//!   出处与日期写在每条的注释里；核不实的字段是 `None`。
//! - **绑定存储** `~/.marionette/bridge-bindings.json`：会话 → {entryId,
//!   modelId}。原子写、损坏留档重建（镜像 credentials.rs 的立场）。会话删除
//!   不强求清理 —— 残留绑定在 spawn 时解不出条目就跳过注入并记 warn，无害。
//! - **reference 直连**：opencode auth.json 里的 api 条目不复制第二份（§2），
//!   `list_models` 现场扫描并入、`preset_for_opencode_provider` 桥到 preset 表；
//!   绑定校验放行，spawn 时 `peek_reference_secret` 现场读钥匙（OAuth / 混合
//!   条目拒绝）。源文件 / 条目消失 → 解析不出，走残留绑定的降级路径。
//! - **grok owned 表写入器**：往真实 `~/.grok/config.toml` 写
//!   `[model."marionette/<model>"]`，api_key 用 `$MARIONETTE_PROVIDER_KEY`
//!   env 展开（S3 实测有效），**绝不设 `[models] default`**（会让 Grok 退出
//! 自身登录）。原则与所有权文件搬运一致：原文搬运、带所有权头注释、幂等
//!   （一致则零写入）、写前重读容忍 grok 并发自改写、解析失败不写半截。
//! - **去重规则**（§4.8 / 产品定稿）：key-plan → claude-code / grok-build 可
//!   见；reference → 宿主 agent 自己不可见；subscription → M2 前不可见。
//!   **不做任何跨 agent 模型名模糊匹配**。
//! - **codex `providers/set` 框架**：initialize 应答声明 `agentCapabilities.
//!   providers` 时把 openai 槽路由到目标端点（仅 apiType:"openai" / Responses，
//!   S2 实测约束）。M1 没有 Responses 核实过的 preset，路由解析恒为 None、
//!   不会真的发送；框架留给 M2 的翻译引擎。
//! - **M2 翻译引擎伪来源**（`crate::engine`，docs §4.5）：引擎 Running 时
//!   `list_models` 追加 `{entryId:"engine"}` 来源（模型 id = 引擎的
//!   `provider/model` 形态）；绑定不需要凭据（令牌是字面量 `magpie`，
//!   S1：引擎回环任意 token 放行）；claude 注入引擎根（服务 /v1/messages），
//!   grok 复用 owned 表机制但 base 指引擎的 `/v1`、api_key 用字面量。codex
//!   侧因模型 id 问题未接（内建名 vs `provider/model`，见 list_models_at 的
//!   codex 分支注释）。
//!
//! 安全底线（§5）：明文 key 只进 env / owned-section 的 env 引用，绝不进日志
//! （日志只记事件与端点，不记值），绝不复制进 agent 磁盘配置。

use crate::credentials::{AgentScan, CredentialEntry, CredentialKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

// ─── presets 表（§4.3）──────────────────────────────────────────────────────

/// 一个 key 型 plan 的「钥匙→门牌」：preset id → 双端点 + 默认模型。
///
/// 端点核实记录见 [`presets`] 表里每条的注释；`anthropic_base` 是 Claude 系
/// env 注入（`ANTHROPIC_BASE_URL`，客户端自己拼 `/v1/messages`），
/// `chat_base` 是 grok owned 表的 `base_url`（`api_backend =
/// "chat_completions"`，grok 向 base_url 拼 `/chat/completions`，所以形态统一
/// 带 `/v1`）。
#[derive(Clone, Copy, Debug)]
pub struct ProviderPreset {
    pub id: &'static str,
    pub label: &'static str,
    /// Anthropic Messages 兼容端点 base（claude-code 用）。核不实 → None。
    pub anthropic_base: Option<&'static str>,
    /// OpenAI chat-completions 兼容端点 base（grok owned 表用）。核不实 → None。
    pub chat_base: Option<&'static str>,
    /// 默认模型 id 列表（picker 预填；未核实过的确切 id 不列，宁缺毋滥）。
    pub models: &'static [&'static str],
    /// 端点最后一次被核实的日期（§9「坏了好修」）。人工维护的元数据，没有
    /// 运行时读取方 —— 单测锚定它非空。
    #[allow(dead_code)]
    pub last_verified: &'static str,
}

/// preset 表本体。全部端点 2026-10-16 核实（官方文档 WebFetch + 直连探测），
/// 逐条出处见每条上方的注释。
pub fn presets() -> &'static [ProviderPreset] {
    &PRESETS
}

/// 跨用桥「读→外科合并→写」全序列的进程内互斥。两个会话并发 spawn 时会
/// 同时跑 refresh：不加锁的话两次合并各基于各自的旧快照，后写覆盖前写，
/// 一方的托管段就丢了（audit P1-B）。锁只覆盖文件序列，不做长 I/O。
static BRIDGE_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

static PRESETS: [ProviderPreset; 6] = [
    // anthropic：api-docs.deepseek.com/guides/anthropic_api 原文
    // "base_url being https://api.deepseek.com/anthropic"。
    // chat：官方文档同时支持 https://api.deepseek.com 与 …/v1 两种 base；
    // grok 的 chat_completions 后端向 base_url 拼 /chat/completions，取 /v1
    // 形态（与用户 ~/.grok 里已在用的 DeepSeek 表一致），与其余 preset 统一。
    // models：api-docs.deepseek.com/quick_start/pricing（deepseek-chat /
    // deepseek-reasoner 旧名已退役，现役是 deepseek-flash、deepseek-v4-pro）。
    ProviderPreset {
        id: "deepseek",
        label: "DeepSeek",
        anthropic_base: Some("https://api.deepseek.com/anthropic"),
        chat_base: Some("https://api.deepseek.com/v1"),
        models: &["deepseek-flash", "deepseek-v4-pro"],
        last_verified: "2026-10-16",
    },
    // 官方文档（opencode.ai/docs/go）摘要给出的 …/openai/v1 路径实测 404；
    // 以直连探测为准：GET …/inference/go/v1/models → 200（完整模型列表）。
    // 2026-10-16 二次核实（magpie opencode_endpoints.go 实测对照）：
    // ① Messages 路由在网关根上（anthropic base = 根形态，客户端拼 /v1/messages），
    //    /anthropic 路径 magpie 从不使用（此前 /anthropic/v1/messages 的 500 是
    //    兜底路由，不是 Messages 端点）；/inference/go 与 /zen/go 是同一路由两名。
    // ② OpenCode 网关 2026-09 起对缺 x-opencode-session 头的请求答 400 ——
    //    头由 grok_dialect 规整器按会话注入（claude/grok 通道都过规整器）。
    // chat_base 与用户 ~/.grok 里已在用的 opencode-go 表逐字一致。
    // models 取自实测模型列表的常用子集。
    ProviderPreset {
        id: "opencode-go",
        label: "OpenCode Go",
        anthropic_base: Some("https://opencode.ai/inference/go"),
        chat_base: Some("https://opencode.ai/inference/go/v1"),
        models: &[
            "deepseek-v4.1-flash",
            "deepseek-v4-pro",
            "grok-4.7",
            "kimi-k3",
            "longcat-2.0",
        ],
        last_verified: "2026-10-16",
    },
    // docs.bigmodel.cn/cn/coding-plan/quick-start（GLM Coding Plan 官方）：
    // Anthropic 协议 https://open.bigmodel.cn/api/anthropic；OpenAI Chat
    // 协议 https://open.bigmodel.cn/api/coding/paas/v4。注意普通按量 key 用
    // /api/paas/v4。用户 ~/.grok 的 zhipu-* 表曾经实配按量端点；spawn 时若
    // api_key 是已知 Coding Plan 钥匙，rewrite_grok_zhipu_coding_plan_bases
    // 会改到 coding 端点（否则智谱 1113「余额不足或无可用资源包」）。
    // models：动态拉取（GET coding …/models，magpie model_names_test 同款）
    // 为主；静态兜底取实测套餐主打两模型（2026-10-17 本机带 key 实测：
    // chat 与 anthropic 两协议的 /models 目录完全一致，各 11 个 id，均含
    // glm-5.3 / glm-5.3-flash；opencode 目录里的 glm-5.3-highspeed / glm-4.6v
    // 两个端点都不认，不进静态表）。
    ProviderPreset {
        id: "glm",
        label: "智谱 GLM",
        anthropic_base: Some("https://open.bigmodel.cn/api/anthropic"),
        chat_base: Some("https://open.bigmodel.cn/api/coding/paas/v4"),
        models: &["glm-5.3", "glm-5.3-flash"],
        last_verified: "2026-10-17",
    },
    // platform.kimi.ai（官方 Claude Code 指南）：ANTHROPIC_BASE_URL=
    // https://api.moonshot.ai/anthropic（端点 …/anthropic/v1/messages）；
    // platform.kimi.ai/docs/api/overview：OpenAI Chat Completions base
    // https://api.moonshot.ai/v1。国内域名是 api.moonshot.cn，preset 取官方
    // 现行文档的国际域名。models：overview 未列确切 id（在 /v1/models 动态
    // 查）—— 留空。
    ProviderPreset {
        id: "kimi",
        label: "Kimi (Moonshot)",
        anthropic_base: Some("https://api.moonshot.ai/anthropic"),
        chat_base: Some("https://api.moonshot.ai/v1"),
        models: &[],
        last_verified: "2026-10-16",
    },
    // 官方 cookbook（openrouter.ai/docs/cookbook/coding-agents/claude-code-
    // integration）原文：ANTHROPIC_BASE_URL=https://openrouter.ai/api（即官方
    // 所谓 Anthropic Skin，客户端拼 /v1/messages）；官方 quickstart：OpenAI
    // 兼容 base https://openrouter.ai/api/v1。models：目录是 provider/model
    // 动态形态，不设默认列表。
    ProviderPreset {
        id: "openrouter",
        label: "OpenRouter",
        anthropic_base: Some("https://openrouter.ai/api"),
        chat_base: Some("https://openrouter.ai/api/v1"),
        models: &[],
        last_verified: "2026-10-16",
    },
    // docs.x.ai/developers/quickstart：OpenAI 兼容 base https://api.x.ai/v1，
    // 模型 grok-4.7。anthropic：2026-10-16 以 magpie presets.go 实测对照修正 ——
    // Anthropic 兼容端点是**根形态** https://api.x.ai（客户端自拼 /v1/messages；
    // magpie provider.go 的 normalize 注释描述的正是 /v1 形态「模型列表能答、
    // 每条消息 404」的坑：会拼出 /v1/v1/messages）。
    ProviderPreset {
        id: "xai",
        label: "xAI (Grok)",
        anthropic_base: Some("https://api.x.ai"),
        chat_base: Some("https://api.x.ai/v1"),
        models: &["grok-4.7"],
        last_verified: "2026-10-16",
    },
];

pub fn find_preset(id: &str) -> Option<&'static ProviderPreset> {
    presets().iter().find(|preset| preset.id == id)
}

/// opencode auth.json 的 provider 键名 → preset 表的桥（reference 直连的门票）。
///
/// opencode 的 provider id 与我们的 preset id 不同名时也认：kimi 的 auth 键有
/// `moonshot` / `kimi` 两种写法。大小写不敏感。未映射的 provider 返回 `None` ——
/// 凭据页照样展示（bridge_scan），但直连没有核实过的 L1 端点，picker 不列、绑定
/// 拒绝。注意 `zai` **不**映射到 glm：Z.ai（api.z.ai）与智谱 open.bigmodel.cn 是
/// 两个 host（magpie 分列两个 preset），映射等于把别家的 key 发错厂商。
pub fn preset_for_opencode_provider(provider_id: &str) -> Option<&'static ProviderPreset> {
    let key = provider_id.trim().to_ascii_lowercase();
    let preset_id = match key.as_str() {
        "openrouter" => "openrouter",
        "deepseek" => "deepseek",
        "moonshot" | "kimi" => "kimi",
        // 智谱 Coding Plan 在 opencode auth.json 里的键名是 `zhipuai-coding-plan`
        // （`opencode auth login` → 智谱 AI Coding Plan；2026-10-17 本机实测），
        // 与裸 `glm` 同走 open.bigmodel.cn 的 coding 端点族。按量的 `zhipuai`
        // 仍不映射：coding 端点只吃套餐 key，按量 key 发过去就是 1113 余额不足。
        "glm" | "zhipuai-coding-plan" => "glm",
        "xai" => "xai",
        // opencode 自己的 key（Zen/Go 平台）在 auth.json 里的键名是 "opencode"。
        // OAuth 订阅条目在扫描侧已分类成 subscription、进不了 reference，所以
        // 这里映射是安全的：只有 type:"api" 的 Go key 会走到直连。
        "opencode" | "opencode-go" => "opencode-go",
        _ => return None,
    };
    find_preset(preset_id)
}

/// 引擎注册用的归一化：opencode 扫描出的 (provider-id, key) → (preset-id, key)。
/// 未映射的 provider（anthropic / openai / zai …）过滤掉 —— 它们在
/// `magpie_provider_for_preset` 里同样无门（registry 计划只吃 preset id）。
/// 不定序：同 preset 多把 key 时谁先谁后由凭据源层决定（opencode.db 的活
/// 钥匙排在 auth.json 旧条目前，见 credentials::merge_opencode_key_sources），
/// registration_plan 先到先得。
pub fn reference_keys_as_presets(keys: Vec<(String, String)>) -> Vec<(String, String)> {
    keys.into_iter()
        .filter_map(|(provider, key)| {
            let preset = preset_for_opencode_provider(&provider)?;
            Some((preset.id.to_string(), key))
        })
        .collect()
}

/// 解析 reference 的 `文件路径#provider-id` → provider-id。
///
/// 从右往左第一个 `#` 切（`rsplit_once`）：Windows 路径与 provider id 都不含
/// `#`，万一含了也以最后一段为 provider-id。解析不出（缺 ref、缺 `#`）→ None。
fn ref_path_provider(ref_path: Option<&str>) -> Option<String> {
    let (_, provider) = ref_path?.rsplit_once('#')?;
    Some(provider.to_string())
}

// ─── agent 分类（§4.8 规则按 agent 区分）────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeAgent {
    ClaudeCode,
    GrokBuild,
    Codex,
    Opencode,
    /// DeepSeek Harness（@openma/deepseek-harness-acp，2026-10-20 接入）：
    /// 注入通道 = llm-pi-ai 托管路由（crate::dsh_bridge），不走 SpawnInjection。
    Deepseek,
    /// Pi（pi-acp，2026-10-20 接入）：注入通道 = `~/.pi/agent/models.json` 的
    /// providers.marionette（官方兼容端点机制，见 pi docs/models.md），
    /// 不走 SpawnInjection。
    Pi,
    /// ZCode（zcode-acp-server 桥，2026-10-23 接入）：注入通道 =
    /// `~/.zcode/v2/provider_config.json` 个人 provider 层的托管 provider
    /// 「marionette」（openai-chat-completions → 引擎 /v1，本模块
    /// refresh_zcode_marionette_route），不走 SpawnInjection。
    Zcode,
    /// M1 未接线的其余 agent：没有注入通道，列表为空。
    Other,
}

/// 把 agent registry 里的 id 归到桥认识的桶（别名一并收，对齐
/// commands.rs probe_agent_auth 的同名习惯）。
pub fn classify_agent(agent_id: &str) -> BridgeAgent {
    match agent_id {
        "claude-code" | "claude" => BridgeAgent::ClaudeCode,
        "grok-build" | "grok" => BridgeAgent::GrokBuild,
        "codex" | "codex-acp" => BridgeAgent::Codex,
        "opencode" => BridgeAgent::Opencode,
        "deepseek" | "deepseek-acp" => BridgeAgent::Deepseek,
        "pi" | "pi-acp" => BridgeAgent::Pi,
        "zcode" | "zcode-acp" => BridgeAgent::Zcode,
        _ => BridgeAgent::Other,
    }
}

// ─── 去重规则（产品定稿，别加戏）────────────────────────────────────────────

/// 一条凭据对一个询问 agent 是否可见。
///
/// - key-plan：对 claude-code / grok-build 可见（M1 的全部受众）。
/// - reference：**宿主 agent 自己不可见**（host_agent 与询问方相同 → 过滤）；
///   别的 agent 可以看（M2 接线用）。
/// - subscription：不可见（M2 引擎）。
fn entry_visible_to(entry: &CredentialEntry, agent: BridgeAgent) -> bool {
    match entry.kind {
        CredentialKind::KeyPlan => true,
        CredentialKind::Reference => match entry.host_agent.as_deref() {
            Some(host) => classify_agent(host) != agent,
            None => true,
        },
        CredentialKind::Subscription => false,
    }
}

/// 一个 preset 对一个 agent 有没有可用的兼容端点（端点驱动，非名字匹配）。
fn preset_serves_agent(preset: &ProviderPreset, agent: BridgeAgent) -> bool {
    match agent {
        BridgeAgent::ClaudeCode => preset.anthropic_base.is_some(),
        BridgeAgent::GrokBuild => preset.chat_base.is_some(),
        _ => false,
    }
}

// ─── 视图（命令契约：字段名 camelCase，签名逐字对齐前端）────────────────────

/// 扫描报告透传类型（bridge_scan 的返回；即 credentials::scan_all 的形状）。
pub type BridgeScanReport = crate::credentials::ScanReport;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeEntryView {
    pub id: String,
    /// "key-plan" | "subscription" | "reference"（kebab-case，同 §4.2 schema）。
    pub kind: String,
    pub label: String,
    pub preset: Option<String>,
    /// 末四位掩码（如 "…21zz"）；key 太短 / 解密失败 / 无 secret → None。绝无明文。
    pub masked_key: Option<String>,
    pub source: String,
    pub note: Option<String>,
    pub created_at: String,
}

/// 供 picker 绑定的一个模型（entry × model 摊平）。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeModel {
    pub entry_id: String,
    pub preset: String,
    pub label: String,
    pub model_id: String,
}

/// 一个跨用来源（picker 的分组锚点）。key-plan 与 reference 都进 `sources`：
/// 即使某来源的 `models` 为空（preset 没有核实过的默认模型列表），前端也要
/// 知道这个来源存在（例如提示用户手输模型 id）。`from_scan = true` 即
/// reference —— key 住在 opencode 的 auth.json 里，未复制。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeSource {
    pub entry_id: String,
    pub preset: String,
    pub label: String,
    pub from_scan: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeModelsResult {
    pub models: Vec<BridgeModel>,
    pub sources: Vec<BridgeSource>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeSelection {
    pub entry_id: String,
    pub model_id: String,
}

fn kind_str(kind: CredentialKind) -> &'static str {
    match kind {
        CredentialKind::KeyPlan => "key-plan",
        CredentialKind::Subscription => "subscription",
        CredentialKind::Reference => "reference",
    }
}

/// 掩码只露末四位（§5.2）。短于 8 位的 key 全遮 —— 掩码不能等于明文。
fn mask_key(key: &str) -> Option<String> {
    let trimmed = key.trim();
    if trimmed.chars().count() < 8 {
        return None;
    }
    let tail: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Some(format!("…{tail}"))
}

fn entry_view(credentials_path: &Path, entry: &CredentialEntry) -> BridgeEntryView {
    let masked_key = if entry.kind == CredentialKind::KeyPlan {
        crate::credentials::peek_secret_at(credentials_path, &entry.id)
            .ok()
            .flatten()
            .and_then(|key| mask_key(&key))
    } else {
        None
    };
    let note = match entry.kind {
        CredentialKind::KeyPlan => None,
        CredentialKind::Subscription => Some("订阅型，跨用将在下版本开放".to_string()),
        CredentialKind::Reference => Some("key 保存在原 agent 文件里，未复制".to_string()),
    };
    BridgeEntryView {
        id: entry.id.clone(),
        kind: kind_str(entry.kind).to_string(),
        label: entry.label.clone(),
        preset: entry.preset.clone(),
        masked_key,
        source: entry.source.clone(),
        note,
        created_at: entry.created_at.clone(),
    }
}

/// bridge_list_entries 本体（默认路径包装）。
pub fn list_entries() -> Result<Vec<BridgeEntryView>, String> {
    let path = credentials_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    list_entries_at(&path)
}

fn list_entries_at(path: &Path) -> Result<Vec<BridgeEntryView>, String> {
    let store = crate::credentials::load_store_at(path)?;
    Ok(store
        .entries
        .iter()
        .map(|entry| entry_view(path, entry))
        .collect())
}

/// bridge_add_key 本体：校验 preset 在表内，落 DPAPI store，返回新视图。
pub fn add_key(preset: &str, label: Option<&str>, key: &str) -> Result<BridgeEntryView, String> {
    let path = credentials_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    add_key_at(&path, preset, label, key)
}

fn add_key_at(
    path: &Path,
    preset: &str,
    label: Option<&str>,
    key: &str,
) -> Result<BridgeEntryView, String> {
    let known = find_preset(preset)
        .ok_or_else(|| {
            let known: Vec<&str> = presets().iter().map(|p| p.id).collect();
            format!("未知 preset `{preset}`（可选：{}）", known.join(", "))
        })?;
    let label = label
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(known.label);
    crate::credentials::add_key_plan_at(path, preset, label, key)?;
    let store = crate::credentials::load_store_at(path)?;
    let entry = store
        .entries
        .iter()
        .find(|entry| entry.id == preset)
        .ok_or_else(|| "写入后读不回条目".to_string())?;
    Ok(entry_view(path, entry))
}

/// bridge_list_models 本体（默认路径包装）。
pub fn list_models(agent_id: &str) -> BridgeModelsResult {
    let Some(home) = home_dir() else {
        return BridgeModelsResult {
            models: Vec::new(),
            sources: Vec::new(),
            note: Some("无法定位用户目录".to_string()),
        };
    };
    let credentials = home.join(".marionette").join("credentials.json");
    // 真实路径扫描复用 provider_usage 的多路径探测（含 %APPDATA% fallback）。
    let scan = crate::credentials::scan_opencode();
    list_models_at(&credentials, &scan, agent_id)
}

/// 引擎伪来源的 source 行（claude/grok 通用分支与 codex 专用分支共用）。
fn engine_bridge_source() -> BridgeSource {
    BridgeSource {
        entry_id: crate::engine::ENGINE_ENTRY_ID.to_string(),
        preset: crate::engine::ENGINE_PRESET_ID.to_string(),
        label: "翻译引擎（订阅）".to_string(),
        from_scan: false,
    }
}

/// 保留引擎完整目录。显示范围和顺序统一交给模型库偏好。
fn push_engine_models(models: &mut Vec<BridgeModel>) -> bool {
    let engine_ids = crate::engine::catalog_direct();
    for model_id in &engine_ids {
        // Visibility belongs to the model library; do not silently cut off
        // models before the user can choose which ones should be shown.
        models.push(BridgeModel {
            entry_id: crate::engine::ENGINE_ENTRY_ID.to_string(),
            preset: crate::engine::ENGINE_PRESET_ID.to_string(),
            label: model_id.clone(),
            model_id: model_id.clone(),
        });
    }
    false
}


// ─── opencode 原生跨用（provider「marionette」→ 引擎 /v1）───────────────────

/// 构建写进子进程 `OPENCODE_CONFIG_CONTENT` 的内联配置：provider
/// 「marionette」指向引擎 `/v1`（openai-compatible），引擎目录的每个模型
/// 一条（id 原样——多斜杠形态 `grok/grok-4.7` 是 magpie 对 opencode 的
/// 生产用法，见 third_party/magpie internal/agent/agents.go 的同款配方）。
/// `existing` 是调用方环境里已有的内联配置（JSON object）：合并进同一对象
/// 只覆盖 `provider.marionette`，别的键/别的 provider 原样保留；不是合法
/// JSON object 就整体替换。纯函数，测试直接喂夹具。
pub fn opencode_engine_config_content(
    port: u16,
    models: &[String],
    existing: Option<&str>,
) -> String {
    use serde_json::Value;
    let mut root: serde_json::Map<String, Value> = existing
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut provider = serde_json::Map::new();
    provider.insert(
        "npm".to_string(),
        Value::String("@ai-sdk/openai-compatible".to_string()),
    );
    provider.insert(
        "name".to_string(),
        Value::String("Marionette 模型共享".to_string()),
    );
    provider.insert(
        "options".to_string(),
        serde_json::json!({
            "baseURL": format!("http://127.0.0.1:{port}/v1"),
            "apiKey": crate::engine::ENGINE_TOKEN,
        }),
    );
    let mut catalog = serde_json::Map::new();
    for id in models {
        let name = id.rsplit('/').next().unwrap_or(id);
        catalog.insert(id.clone(), serde_json::json!({ "name": name }));
    }
    provider.insert("models".to_string(), Value::Object(catalog));
    // provider 层合并：已有 provider 深对象保留，只覆盖 marionette。
    let mut providers = root
        .remove("provider")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    providers.insert("marionette".to_string(), Value::Object(provider));
    root.insert("provider".to_string(), Value::Object(providers));
    serde_json::to_string(&Value::Object(root)).unwrap_or_default()
}

/// spawn 时刻的真实参数版（引擎没跑或目录为空 → None，不注入）。
pub fn opencode_engine_config(existing: Option<&str>) -> Option<String> {
    let (port, _) = crate::engine::running_base()?;
    let models = crate::engine::catalog_direct();
    if models.is_empty() {
        return None;
    }
    Some(opencode_engine_config_content(port, &models, existing))
}

// ─── pi 原生跨用（~/.pi/agent/models.json 的 providers.marionette）──────────

/// pi 的 agent 目录：`PI_CODING_AGENT_DIR` > `~/.pi/agent`（与 parsers/pi.rs
/// 的会话目录解析同一套约定，见 pi docs/configuration.md）。
fn pi_agent_dir() -> Option<std::path::PathBuf> {
    if let Some(raw) = std::env::var_os("PI_CODING_AGENT_DIR") {
        if !raw.to_string_lossy().trim().is_empty() {
            return Some(PathBuf::from(raw));
        }
    }
    Some(home_dir()?.join(".pi").join("agent"))
}

pub fn pi_models_path() -> Option<std::path::PathBuf> {
    Some(pi_agent_dir()?.join("models.json"))
}

/// providers.marionette 的内容（pi docs/models.md「Configure a compatible
/// endpoint」形态）。`apiKey` 用官方 `$NAME` env 插值 —— 令牌不落盘，spawn
/// 注入 `MARIONETTE_API_KEY`（与 dsh 桥同一个 env 名）。
fn pi_marionette_provider(port: u16, models: &[String]) -> serde_json::Value {
    serde_json::json!({
        "baseUrl": format!("http://127.0.0.1:{port}/v1"),
        "api": "openai-completions",
        "apiKey": format!("${}", crate::dsh_bridge::MARIONETTE_TOKEN_ENV),
        "models": models
            .iter()
            .map(|id| serde_json::json!({ "id": id }))
            .collect::<Vec<_>>(),
    })
}

/// 纯合并体（测试直接喂夹具）：只增改 `providers.marionette`，用户的其它
/// provider / 顶层键原样保留；原文不是合法 JSON object 就整体替换（pi 对
/// models.json 无注释语法，serde_json 往返安全）。返回 (文本, 是否语义变更)。
fn pi_merge_models_json(
    existing: Option<&str>,
    port: u16,
    models: &[String],
) -> (String, bool) {
    use serde_json::Value;
    let parsed = existing.and_then(|text| serde_json::from_str::<Value>(text).ok());
    let mut root = parsed
        .as_ref()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let before = serde_json::to_value(&root).ok();
    let mut providers = root
        .remove("providers")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    providers.insert(
        crate::dsh_bridge::MARIONETTE_PROVIDER.to_string(),
        pi_marionette_provider(port, models),
    );
    root.insert(
        "providers".to_string(),
        Value::Object(providers),
    );
    let changed = before != serde_json::to_value(&root).ok();
    let text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
    (format!("{text}\n"), changed)
}

/// 移除托管 provider（引擎停用时调用，pi 的 /model 菜单不留连不上的死条目）。
/// 返回 (是否动了文件)。providers 只剩我们一个 → 连 providers 键一起收。
fn pi_remove_marionette_at(path: &Path) -> Result<bool, String> {
    use serde_json::Value;
    let original = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("读取 models.json 失败: {error}")),
    };
    if original.trim().is_empty() {
        return Ok(false);
    }
    let mut root = serde_json::from_str::<Value>(&original)
        .map_err(|e| format!("models.json 解析失败（{e}）— 拒绝写入半截"))?;
    let Some(root_obj) = root.as_object_mut() else {
        return Err("models.json 不是 JSON object — 拒绝写入".to_string());
    };
    let mut providers = root_obj
        .remove("providers")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if providers
        .remove(crate::dsh_bridge::MARIONETTE_PROVIDER)
        .is_none()
    {
        // 没有托管段：把 providers 原样放回，零写入。
        root_obj.insert("providers".to_string(), Value::Object(providers));
        return Ok(false);
    }
    if !providers.is_empty() {
        root_obj.insert("providers".to_string(), Value::Object(providers));
    }
    let text = serde_json::to_string_pretty(&root).unwrap_or_default();
    write_text_atomic(path, &format!("{text}\n"))?;
    Ok(true)
}

/// spawn 时刻入口（acp.rs 的 pi 分支），语义与 dsh_bridge::refresh 对齐：
/// Running 且目录非空 → 写/刷（语义幂等，格式化不 churn）→ Ok(true)；
/// Absent/Failed → 移除托管段 → Ok(false)；Starting/Downloading → 不动。
pub fn refresh_pi_marionette_route() -> Result<bool, String> {
    // 读→合并→写必须整体互斥（见 BRIDGE_WRITE_LOCK）。
    let _guard = BRIDGE_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = pi_models_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    match crate::engine::status() {
        crate::engine::EngineStatus::Running { port, .. } => {
            let models = crate::engine::catalog_direct();
            if models.is_empty() {
                return Ok(false);
            }
            let existing = match fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("读取 models.json 失败: {error}")),
            };
            // 半写坏 / 非 object 的原文不能静默整体替换（用户自建 provider
            // 会丢）——先改名留档，再当作新文件合并。
            let existing = match existing {
                Some(text) if !is_mergeable_object(&text) => {
                    archive_corrupt(&path)?;
                    None
                }
                other => other,
            };
            let (next, changed) = pi_merge_models_json(existing.as_deref(), port, &models);
            if changed {
                write_text_atomic(&path, &next)?;
            }
            Ok(true)
        }
        crate::engine::EngineStatus::Starting
        | crate::engine::EngineStatus::Downloading { .. } => Ok(false),
        crate::engine::EngineStatus::Absent | crate::engine::EngineStatus::Failed { .. } => {
            pi_remove_marionette_at(&path)?;
            Ok(false)
        }
    }
}

// ─── zcode 原生跨用（2026-10-23）───────────────────────────────────────────

/// 引擎目录没报窗口时写进 zcode 的兜底值。**不能写 0**：个人层 zod 对
/// `properties.contextWindow` 是 `number().positive()`，一条 0 会让整层
/// `providerModelRules` 校验失败、zcode 以内存空配置降级——用户自己所有
/// provider/模型设置一起消失（2026-10-23 实测，见 docs/pitfalls-zcode-acp.md 雷 7）。
/// 取 200000 与 zcode 内置 `.*` 兜底规则同值，显示口径一致。
const ZCODE_UNKNOWN_CONTEXT_WINDOW: u64 = 200_000;

/// `~/.zcode/v2/provider_config.json` — zcode 的**个人** provider 层（桌面版
/// 与 CLI 共读；适配器把它经 `ZCODE_PERSONAL_PROVIDER_CONFIG_FILE` 显式递给
/// 后端）。托管写这层而不是账号层 `v2/config.json`（桌面版自己重写账号层，
/// 托管条会被冲掉；适配器的 env 注入也只扫账号层 → 不会劫持默认 provider）。
fn zcode_provider_config_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(
        std::path::PathBuf::from(home)
            .join(".zcode")
            .join("v2")
            .join("provider_config.json"),
    )
}

/// 托管 provider 规则（providerRules 里 providerId = marionette 的一条）。
/// api 用 openai-chat-completions（zcode 官方模板同款协议，zai-standard-api /
/// opencode-go-chat 都在用，引擎 /v1 正好兼容）；apiKey 是引擎常量令牌（非
/// 用户密钥，与 dsh 桥写 .credentials.yaml refs 同口径，值就是 `magpie`）。
/// 字段形状对齐本机实测的用户自建 provider 规则（group/access/api/
/// personalModelIds/modelOrder）。
fn zcode_marionette_provider_rule(port: u16, models: &[CatalogModelRow]) -> serde_json::Value {
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    serde_json::json!({
        "providerId": crate::dsh_bridge::MARIONETTE_PROVIDER,
        "providerName": "Marionette Engine",
        "enabled": true,
        "config": {
            "group": "standard-personal",
            "access": {
                "type": "api-key",
                "apiKey": crate::engine::ENGINE_TOKEN,
            },
            "api": {
                "type": "openai-chat-completions",
                "baseUrl": format!("http://127.0.0.1:{port}/v1"),
            },
            "personalModelIds": ids,
            "modelOrder": ids,
        },
    })
}

/// 托管 providerModelRules：个人层的每模型元数据（显式 providerId+modelId，
/// 不是内置表那种 regex modelRules——那个键名会被个人层 zod 整层拒绝，
/// 实测 2026-10-23：`Unrecognized key: "modelRules"` → 整层降级空配置）。
/// 不写的话 zcode 对陌生模型落内置 `.*` 兜底（200K 窗口、不收图）——1M
/// 模型会被显示错、附图被换成占位文本（与 DSH 的 256K/无图教训同源，见
/// AGENTS.md DSH 节）。形状对齐 zcode-acp-server dist/config/options.js 的
/// loadPersonalModels 读取面 + 本机用户层实测。
fn zcode_marionette_model_rules(models: &[CatalogModelRow]) -> Vec<serde_json::Value> {
    models
        .iter()
        .map(|m| {
            // 引擎没说窗口（0）→ 写兜底值，不能写 0（见 ZCODE_UNKNOWN_CONTEXT_WINDOW：
            // 0 会让 zcode 整层 personal provider 配置解析失败）。
            let context_window = if m.context_window > 0 {
                m.context_window
            } else {
                ZCODE_UNKNOWN_CONTEXT_WINDOW
            };
            let mut config = serde_json::json!({
                "enabled": true,
                "properties": {
                    "contextWindow": context_window,
                    "inputFormat": {
                        "supportsText": true,
                        "supportsImage": m.images,
                        "supportsVideo": false,
                        "supportsAudio": false,
                        "supportsPdf": false,
                    },
                    "outputFormat": { "supportsText": true },
                    "supportsToolCall": true,
                },
            });
            if m.max_output > 0 {
                config["optionSpecs"] = serde_json::json!({
                    "maxOutputTokens": { "max": m.max_output }
                });
            }
            serde_json::json!({
                "providerId": crate::dsh_bridge::MARIONETTE_PROVIDER,
                "modelId": m.id,
                "config": config,
            })
        })
        .collect()
}

/// 纯合并体（测试直接喂夹具）：只动三处托管位 —— providerOrder 里的
/// marionette（**追加**到尾部，不前插：providerOrder 影响默认选择顺序，
/// 不能动用户的默认模型）、providerRules 里 providerId=marionette 的一条、
/// modelConfigRules.providerModelRules 里 providerId=marionette 的条目（按
/// providerId 识别整体替换，引擎目录换血不留旧条）；用户其余内容原样保留
/// （唯一例外：manualProviderModelRules 缺失/非数组时补空数组，见实现处
/// 注释）。原文不是合法 JSON object 就整体替换（该文件无注释语法，
/// serde_json 往返安全）。返回 (文本, 是否语义变更)。
fn zcode_merge_provider_config(
    existing: Option<&str>,
    port: u16,
    models: &[CatalogModelRow],
) -> (String, bool) {
    use serde_json::Value;
    let managed = crate::dsh_bridge::MARIONETTE_PROVIDER;
    let parsed = existing.and_then(|text| serde_json::from_str::<Value>(text).ok());
    let mut root = parsed
        .as_ref()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let before = serde_json::to_value(&root).ok();

    root.insert("schemaVersion".to_string(), serde_json::json!(1));
    let mut config = root
        .remove("config")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    let mut order: Vec<Value> = config
        .remove("providerOrder")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    order.retain(|value| value.as_str() != Some(managed));
    order.push(Value::String(managed.to_string()));
    config.insert("providerOrder".to_string(), Value::Array(order));

    let mut provider_rules_root = config
        .remove("providerConfigRules")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut provider_rules: Vec<Value> = provider_rules_root
        .remove("providerRules")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    provider_rules.retain(|rule| {
        rule.get("providerId").and_then(Value::as_str) != Some(managed)
    });
    provider_rules.push(zcode_marionette_provider_rule(port, models));
    provider_rules_root.insert(
        "providerRules".to_string(),
        Value::Array(provider_rules),
    );
    config.insert(
        "providerConfigRules".to_string(),
        Value::Object(provider_rules_root),
    );

    let mut model_rules_root = config
        .remove("modelConfigRules")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut provider_model_rules: Vec<Value> = model_rules_root
        .remove("providerModelRules")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    provider_model_rules.retain(|rule| {
        rule.get("providerId").and_then(Value::as_str) != Some(managed)
    });
    provider_model_rules.extend(zcode_marionette_model_rules(models));
    model_rules_root.insert(
        "providerModelRules".to_string(),
        Value::Array(provider_model_rules),
    );
    // 新版桌面 schema 把 manualProviderModelRules 也当**必填数组**：桥从头
    // 建文件（重装后 ~/.zcode 全新、桌面还没写过个人层）时缺这个键，整层
    // 个人配置 zod 拒载、内存降级为空——桌面宿主每分钟报 `expected array,
    // received undefined`，会话 turn 全部死在 model_creation 的「Select a
    // model before continuing」（2026-10-24 实测；与雷 7 的未知键/0 窗口
    // 同一失效模式）。已有值原样保留；非数组（null 等）同样兜底回空数组。
    let manual_rules_is_array = model_rules_root
        .get("manualProviderModelRules")
        .map(Value::is_array)
        .unwrap_or(false);
    if !manual_rules_is_array {
        model_rules_root.insert(
            "manualProviderModelRules".to_string(),
            Value::Array(Vec::new()),
        );
    }
    config.insert(
        "modelConfigRules".to_string(),
        Value::Object(model_rules_root),
    );

    root.insert("config".to_string(), Value::Object(config));
    let changed = before != serde_json::to_value(&root).ok();
    let text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
    (format!("{text}\n"), changed)
}

/// 移除托管段（引擎停用时调用，zcode 的模型菜单不留连不上的死条目）。
/// 返回 (是否动了文件)；文件不存在或本来就没有托管段 → false（零写入）。
fn zcode_remove_marionette_at(path: &Path) -> Result<bool, String> {
    use serde_json::Value;
    let original = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("读取 provider_config.json 失败: {error}")),
    };
    if original.trim().is_empty() {
        return Ok(false);
    }
    let mut root = serde_json::from_str::<Value>(&original)
        .map_err(|e| format!("provider_config.json 解析失败（{e}）— 拒绝写入半截"))?;
    let Some(root_obj) = root.as_object_mut() else {
        return Err("provider_config.json 不是 JSON object — 拒绝写入".to_string());
    };
    let managed = crate::dsh_bridge::MARIONETTE_PROVIDER;
    let mut touched = false;

    if let Some(config) = root_obj.get_mut("config").and_then(Value::as_object_mut) {
        if let Some(order) = config.get_mut("providerOrder").and_then(Value::as_array_mut) {
            let before = order.len();
            order.retain(|value| value.as_str() != Some(managed));
            if order.len() != before {
                touched = true;
            }
        }
        if let Some(rules) = config
            .get_mut("providerConfigRules")
            .and_then(Value::as_object_mut)
            .and_then(|rules| rules.get_mut("providerRules"))
            .and_then(Value::as_array_mut)
        {
            let before = rules.len();
            rules.retain(|rule| {
                rule.get("providerId").and_then(Value::as_str) != Some(managed)
            });
            if rules.len() != before {
                touched = true;
            }
        }
        if let Some(rules) = config
            .get_mut("modelConfigRules")
            .and_then(Value::as_object_mut)
            .and_then(|rules| rules.get_mut("providerModelRules"))
            .and_then(Value::as_array_mut)
        {
            let before = rules.len();
            rules.retain(|rule| {
                rule.get("providerId").and_then(Value::as_str) != Some(managed)
            });
            if rules.len() != before {
                touched = true;
            }
        }
    }
    if !touched {
        return Ok(false);
    }
    let text = serde_json::to_string_pretty(&root).unwrap_or_default();
    write_text_atomic(path, &format!("{text}\n"))?;
    Ok(true)
}

/// spawn 时刻入口（acp.rs 的 zcode 分支），语义与 dsh/pi 桥对齐：
/// Running 且目录非空 → 写/刷（幂等）→ Ok(true)；Absent/Failed → 移除托管段
/// → Ok(false)；Starting/Downloading → 不动。模型经 zcode 原生目录广告，id
/// 形如 `marionette\<engineId>`（适配器 configOptions 的 `provider\model`
/// 拼法，前端钉原生偏好用这个前缀）。
pub fn refresh_zcode_marionette_route() -> Result<bool, String> {
    // 读→合并→写必须整体互斥（见 BRIDGE_WRITE_LOCK）。
    let _guard = BRIDGE_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = zcode_provider_config_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    match crate::engine::status() {
        crate::engine::EngineStatus::Running { port, .. } => {
            let models = crate::engine::catalog_direct_entries();
            if models.is_empty() {
                return Ok(false);
            }
            let existing = match fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("读取 provider_config.json 失败: {error}")),
            };
            // 半写坏 / 非 object 的原文不能静默整体替换（用户自建 provider
            // 会丢）——先改名留档，再当作新文件合并。
            let existing = match existing {
                Some(text) if !is_mergeable_object(&text) => {
                    archive_corrupt(&path)?;
                    None
                }
                other => other,
            };
            let (next, changed) = zcode_merge_provider_config(existing.as_deref(), port, &models);
            if changed {
                write_text_atomic(&path, &next)?;
            }
            Ok(true)
        }
        crate::engine::EngineStatus::Starting
        | crate::engine::EngineStatus::Downloading { .. } => Ok(false),
        crate::engine::EngineStatus::Absent | crate::engine::EngineStatus::Failed { .. } => {
            zcode_remove_marionette_at(&path)?;
            Ok(false)
        }
    }
}

fn list_models_at(
    credentials_path: &Path,
    opencode_scan: &AgentScan,
    agent_id: &str,
) -> BridgeModelsResult {
    match classify_agent(agent_id) {
        // codex 只接引擎源（M2 收官）：key/reference 端点全是 chat-completions，
        // 被 codex 的 Responses-only 拒（§4.4 S2）。模型 id 保持 codex 内建名
        // —— 绑定 = providers/set 指引擎 `/v1` + 路由组把内建名别名到所选
        // 引擎模型（engine::codex_alias_sync）。
        BridgeAgent::Codex => {
            if !crate::engine::bridge_ready() {
                return BridgeModelsResult {
                    models: Vec::new(),
                    sources: Vec::new(),
                    note: Some(
                        "模型共享需要翻译引擎 — 请先在设置的凭据页启用（引擎运行中后重开此菜单）"
                            .to_string(),
                    ),
                };
            }
            let mut models = Vec::new();
            let truncated = push_engine_models(&mut models);
            let mut note = if models.is_empty() {
                Some("引擎还没有可用模型（订阅发现进行中，稍后重试）".to_string())
            } else {
                None
            };
            if truncated {
                let suffix = "目录过长已截断，更多模型请用「自定义模型」输入";
                note = Some(match note {
                    Some(existing) => format!("{existing}；{suffix}"),
                    None => suffix.to_string(),
                });
            }
            BridgeModelsResult {
                models,
                sources: vec![engine_bridge_source()],
                note,
            }
        }
        // opencode（2026-10-17 二段定稿）：与 claude/grok/codex **同一 Composer
        // 跨用通道** —— 引擎源照常列出（引擎没跑就给指引，跑起来后前端缓存
        // 失效、重开菜单即现）；选中即绑定 + 重启，spawn 时把引擎目录作为
        // provider「marionette」注入 OPENCODE_CONFIG_CONTENT（acp.rs），前端
        // 把 `marionette/<id>` 钉成原生偏好，重启后原生菜单接管。「reference
        // 对宿主自己不可见」的去重规则照旧（key 型来源不列）。
        //
        // dsh（2026-10-20）：同一引擎源姿势，注入面换成 llm-pi-ai 托管路由
        // （crate::dsh_bridge 写 ~/.dsh/cordis.patch.yml）。模型经 dsh-acp 原生
        // 目录广告，id 是 `marionette::<engineId>`（前端钉偏好用这个前缀）。
        //
        // pi（2026-10-20）：同一引擎源姿势，注入面换成本模块的
        // `~/.pi/agent/models.json` providers.marionette。原生 id 是
        // `marionette/<engineId>`（pi-acp 按 provider/id 拼、首个 `/` 切分），
        // 与 grok/opencode 的原生前缀同形。
        //
        // zcode（2026-10-23）：同一引擎源姿势，注入面换成本模块的
        // `~/.zcode/v2/provider_config.json` 托管 provider。原生 id 是
        // `marionette\<engineId>`（适配器 configOptions 的 `provider\model`
        // 拼法，反斜杠分隔）。
        BridgeAgent::Opencode | BridgeAgent::Deepseek | BridgeAgent::Pi | BridgeAgent::Zcode => {
            let mut models = Vec::new();
            let mut truncated = false;
            let mut sources = Vec::new();
            if crate::engine::bridge_ready() {
                sources.push(engine_bridge_source());
                truncated = push_engine_models(&mut models);
            }
            let mut note = if crate::engine::bridge_ready() {
                None
            } else {
                Some(
                    "模型共享未运行 — Grok/GPT 等订阅模型暂不可见；点左下角模型共享按钮启动（或稍候自动唤醒）后重开此菜单"
                        .to_string(),
                )
            };
            if truncated {
                let suffix = "目录过长已截断，更多模型请用「自定义模型」输入";
                note = Some(match note {
                    Some(existing) => format!("{existing}；{suffix}"),
                    None => suffix.to_string(),
                });
            }
            BridgeModelsResult {
                models,
                sources,
                note,
            }
        }
        // M1 只给 claude-code / grok-build 接了直连注入，codex 走引擎伪来源
        // （上面的分支）；其余 agent 列了也选不上（spawn 时不会有任何注入）。
        BridgeAgent::Other => BridgeModelsResult {
            models: Vec::new(),
            sources: Vec::new(),
            note: Some(
                "该 agent 的模型共享还没接（当前支持 Claude Code、Grok、Codex、OpenCode、DeepSeek、Pi 与 ZCode）"
                    .to_string(),
            ),
        },
        agent => {
            let store = match crate::credentials::load_store_at(credentials_path) {
                Ok(store) => store,
                Err(error) => {
                    return BridgeModelsResult {
                        models: Vec::new(),
                        sources: Vec::new(),
                        note: Some(error),
                    }
                }
            };
            let mut sources: Vec<BridgeSource> = Vec::new();
            // (entry_id, preset, 静态清单) —— 模型目录在收集完来源后统一装配
            // （静态清单 ∪ 动态 /models 拉取，见 assemble_models）。
            let mut source_rows: Vec<(String, &'static ProviderPreset)> = Vec::new();
            // key-plan（凭据库，DPAPI 存的那份）。
            for entry in store
                .entries
                .iter()
                .filter(|entry| entry.kind == CredentialKind::KeyPlan)
                .filter(|entry| entry_visible_to(entry, agent))
            {
                let Some(preset) = entry.preset.as_deref().and_then(find_preset) else {
                    continue;
                };
                if !preset_serves_agent(preset, agent) {
                    continue;
                }
                sources.push(BridgeSource {
                    entry_id: entry.id.clone(),
                    preset: preset.id.to_string(),
                    label: entry.label.clone(),
                    from_scan: false,
                });
                source_rows.push((entry.id.clone(), preset));
            }
            // reference 并入：opencode auth.json 的实时扫描（纯读、不落盘，
            // §2「nothing is stored twice」）。扫描失败（文件损坏等）时 entries
            // 为空 —— 静默跳过该家，store 条目照常。只列映射得到 preset 的；
            // OAuth 条目在扫描侧已分类成 subscription，这里天然进不来。
            for entry in opencode_scan
                .entries
                .iter()
                .filter(|entry| entry.kind == CredentialKind::Reference)
                .filter(|entry| entry_visible_to(entry, agent))
            {
                let Some(provider) = ref_path_provider(entry.ref_path.as_deref()) else {
                    continue;
                };
                let Some(preset) = preset_for_opencode_provider(&provider) else {
                    continue;
                };
                if !preset_serves_agent(preset, agent) {
                    continue;
                }
                sources.push(BridgeSource {
                    entry_id: entry.id.clone(),
                    preset: preset.id.to_string(),
                    label: entry.label.clone(),
                    from_scan: true,
                });
                source_rows.push((entry.id.clone(), preset));
            }
            let (models, truncated) = assemble_models(credentials_path, opencode_scan, &source_rows);
            let (mut models, mut truncated) = (models, truncated);
            // ── M2 翻译引擎伪来源（§4.4 L2）：引擎 Running 才出现 —— 不自动
            //    下载、不自动拉起（启用是凭据页的显式动作）。订阅条目
            //    （claude/codex/grok 登录态）不逐条列出：引擎源出现即代表
            //    它们可跨用，模型 id 是引擎的 `provider/model` 形态。
            if crate::engine::bridge_ready() {
                sources.push(engine_bridge_source());
                truncated |= push_engine_models(&mut models);
            }
            let mut note = if sources.is_empty() {
                Some("还没有可共享的 API Key，请先在设置的凭据页添加".to_string())
            } else if models.is_empty() {
                Some("当前凭证没有可用的共享模型（拉取模型目录失败，可稍后重试或用自定义输入）".to_string())
            } else {
                None
            };
            // 引擎睡着时明说（2026-10-17 用户反馈）：订阅族（Grok/GPT…）
            // 缺席不是没有，是引擎未运行——给出动作指引。
            if !crate::engine::bridge_ready() && note.is_none() {
                note = Some(
                    "模型共享未运行 — Grok/GPT 等订阅模型暂不可见；点左下角模型共享按钮启动（或稍候自动唤醒）后重开此菜单"
                        .to_string(),
                );
            }
            if truncated {
                let suffix = "目录过长已截断，更多模型请用「自定义模型」输入";
                note = Some(match note {
                    Some(existing) => format!("{existing}；{suffix}"),
                    None => suffix.to_string(),
                });
            }
            BridgeModelsResult { models, sources, note }
        }
    }
}

// ─── 动态模型目录（/v1/models，带缓存）──────────────────────────────────────
//
// opencode 里「配了 key 就有完整模型列表」的体验来自它启动时拉 provider 目录；
// 我们对齐：bridge_list_models 用已配的 key GET <chat_base>/models，结果按
// entry 缓存（TTL 10 分钟，失败 90 秒负缓存），与 preset 静态清单取并集。
// 拉取用并发线程 + 3.5s 总预算，赶不上本次装配的线程会把缓存焐热给下一次。

// ─── B4-2 目录指纹漂移日志 ─────────────────────────────────────────────────

/// 存档形状：`{ "version": 1, "sources": { "<entry_id>": [model_id…] } }`。
/// 存清单本身而非只存 hash：diff 精确到 added/removed，且人类可读。
#[derive(serde::Serialize, serde::Deserialize)]
struct FingerprintFile {
    version: u32,
    sources: HashMap<String, Vec<String>>,
}

/// 对 `bridge_list_models` 的结果按 source 做指纹对比（B4-2 防线）：模型清单
/// 变化 → dev.log 记 `[bridge] model catalog drift`（source + added/removed）
/// 并原子更新存档 `~/.marionette/bridge-model-fingerprints.json`。只 diff 本次
/// 询问里出现的来源（不同 agent 看到的来源集合不同，「缺席≠删除」）；无漂移
/// 零写入。任何失败只 warn，不影响目录本身。
pub(crate) fn note_catalog_drift(result: &BridgeModelsResult) {
    let Some(home) = home_dir() else { return };
    let path = home.join(".marionette").join("bridge-model-fingerprints.json");
    if let Err(error) = note_catalog_drift_at(&path, result) {
        crate::debug_log::append(
            "bridge",
            "warn",
            "",
            "model fingerprint drift check failed",
            Some(&error),
        );
    }
}

fn note_catalog_drift_at(path: &Path, result: &BridgeModelsResult) -> Result<(), String> {
    let previous: HashMap<String, Vec<String>> = match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<FingerprintFile>(&text)
            .map_err(|e| format!("指纹存档解析失败: {e}"))?
            .sources,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(e) => return Err(format!("指纹存档读取失败: {e}")),
    };
    // 本次询问的每个来源 → 排序后的模型清单（排序保证 diff 与存档稳定）。
    let mut current: HashMap<String, Vec<String>> = HashMap::new();
    for source in &result.sources {
        let mut ids: Vec<String> = result
            .models
            .iter()
            .filter(|model| model.entry_id == source.entry_id)
            .map(|model| model.model_id.clone())
            .collect();
        ids.sort();
        ids.dedup();
        current.insert(source.entry_id.clone(), ids);
    }
    if previous == current {
        return Ok(());
    }
    for (entry_id, ids) in &current {
        let before: std::collections::BTreeSet<&String> = previous
            .get(entry_id)
            .map(|list| list.iter().collect())
            .unwrap_or_default();
        let now: std::collections::BTreeSet<&String> = ids.iter().collect();
        if before == now {
            continue;
        }
        let added: Vec<&str> = now.difference(&before).map(|s| s.as_str()).collect();
        let removed: Vec<&str> = before.difference(&now).map(|s| s.as_str()).collect();
        crate::debug_log::append(
            "bridge",
            "info",
            "",
            "model catalog drift",
            Some(&format!(
                "source={} added=[{}] removed=[{}]",
                entry_id,
                added.join(","),
                removed.join(","),
            )),
        );
    }
    let body = serde_json::to_string_pretty(&FingerprintFile {
        version: 1,
        sources: current,
    })
    .map_err(|e| format!("指纹存档序列化失败: {e}"))?;
    write_text_atomic(path, &body)
}

struct DynamicEntry {
    fetched_at: std::time::Instant,
    models: Vec<String>,
}

static DYNAMIC_MODELS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, DynamicEntry>>> =
    std::sync::OnceLock::new();

const DYNAMIC_TTL: std::time::Duration = std::time::Duration::from_secs(600);
const DYNAMIC_FAIL_TTL: std::time::Duration = std::time::Duration::from_secs(90);
const DYNAMIC_BUDGET: std::time::Duration = std::time::Duration::from_millis(3500);

fn dynamic_cache() -> &'static std::sync::Mutex<HashMap<String, DynamicEntry>> {
    DYNAMIC_MODELS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// 测试钩子：按 preset 覆盖动态目录的端点（保持测试封闭，不打真网络）。
#[cfg(test)]
static FETCH_BASE_OVERRIDE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, String>>> =
    std::sync::OnceLock::new();

#[cfg(test)]
fn set_fetch_base_override_for_test(preset_id: &str, base: &str) {
    let overrides = FETCH_BASE_OVERRIDE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    overrides
        .lock()
        .unwrap()
        .insert(preset_id.to_string(), base.to_string());
}

/// 测试清理：清掉全部端点覆盖（并行测试不把「指向死端口」的覆盖泄漏给
/// 后面的目录测试）。
#[cfg(test)]
fn clear_fetch_overrides_for_test() {
    if let Some(overrides) = FETCH_BASE_OVERRIDE.get() {
        overrides.lock().unwrap().clear();
    }
}

#[cfg(test)]
fn clear_dynamic_cache_for_test() {
    dynamic_cache().lock().unwrap().clear();
}

/// 动态目录的端点（测试可覆盖；生产 = preset.chat_base）。
fn effective_chat_base(preset: &ProviderPreset) -> Option<String> {
    #[cfg(test)]
    {
        if let Some(overrides) = FETCH_BASE_OVERRIDE.get().and_then(|map| map.lock().ok()) {
            if let Some(base) = overrides.get(preset.id) {
                return Some(base.clone());
            }
        }
    }
    preset.chat_base.map(str::to_string)
}

/// 新鲜缓存（命中 TTL）里的动态清单；没有或过期 → None。
fn fresh_dynamic(entry_id: &str) -> Option<Vec<String>> {
    let cache = dynamic_cache().lock().ok()?;
    let entry = cache.get(entry_id)?;
    let ttl = if entry.models.is_empty() {
        DYNAMIC_FAIL_TTL
    } else {
        DYNAMIC_TTL
    };
    entry.fresh_with(ttl).then(|| entry.models.clone())
}

impl DynamicEntry {
    fn fresh_with(&self, ttl: std::time::Duration) -> bool {
        self.fetched_at.elapsed() < ttl
    }
}

/// GET <chat_base>/models，解析 OpenAI 形状（`data[].id`）或 `models[].id`
/// 变体（deepseek 用后者）。任何失败 → 空表（走负缓存）。
fn fetch_models_from_endpoint(chat_base: &str, key: &str) -> Vec<String> {
    let url = format!("{}/models", chat_base.trim_end_matches('/'));
    let Ok(agent) = crate::http_client::agent() else {
        return Vec::new();
    };
    let Ok(response) = agent
        .get(&url)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(4))
        .call()
    else {
        return Vec::new();
    };
    // 响应带 Bearer key 请求的结果，读取加硬上限（模型目录正常远小于 16MiB），
    // 恶意/坏掉的端点返回几个 GB 也不能把进程内存吃穿。
    const MODELS_CATALOG_MAX_BYTES: u64 = 16 * 1024 * 1024;
    use std::io::Read;
    let reader = response.into_reader();
    let mut text = String::new();
    let mut capped = reader.take(MODELS_CATALOG_MAX_BYTES + 1);
    if capped.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    if text.len() as u64 > MODELS_CATALOG_MAX_BYTES {
        return Vec::new();
    }
    parse_models_catalog(&text)
}

/// 引擎 `/v1/models` 的一条模型（magpie 网关形态）。窗口/输出为 0 = 引擎没说。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogModelRow {
    pub id: String,
    /// `context_window`（magpie 同时写 `context_length` / `max_input_tokens`，
    /// 同值）。dsh 的 llm-pi-ai 对没声明 `contextWindow` 的模型一律落到
    /// 256K 兜底——DeepSeek 实际 1M 的窗口就是这么被显示成 256K 的。
    pub context_window: u64,
    /// `max_output_tokens`（回复上限）。
    pub max_output: u64,
    /// 接受图片输入（`modalities.input` 含 "image"）。dsh 的 llm-pi-ai 对
    /// 没声明 `input` 的模型一律按 `[text]` 处理，DSH 会把用户附图换成
    /// 「image omitted because this model accepts text only」占位文本。
    pub images: bool,
}

/// 目录 JSON → 模型行（保序去重，带窗口/输出上限）。engine.rs 的引擎目录共用。
pub(crate) fn parse_models_catalog_rows(text: &str) -> Vec<CatalogModelRow> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let items = value
        .get("data")
        .and_then(|data| data.as_array())
        .or_else(|| value.get("models").and_then(|models| models.as_array()));
    let Some(items) = items else {
        return Vec::new();
    };
    let positive = |item: &serde_json::Value, keys: &[&str]| -> u64 {
        keys.iter()
            .find_map(|key| {
                item.get(*key)
                    .and_then(|v| v.as_u64())
                    .filter(|v| *v > 0)
            })
            .unwrap_or(0)
    };
    let mut out: Vec<CatalogModelRow> = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(|id| id.as_str()) else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() || out.iter().any(|existing| existing.id == id) {
            continue;
        }
        out.push(CatalogModelRow {
            id: id.to_string(),
            context_window: positive(item, &["context_window", "context_length", "max_input_tokens"]),
            max_output: positive(item, &["max_output_tokens", "max_tokens"]),
            images: catalog_item_takes_images(item),
        });
    }
    out
}

/// 目录条目的图片输入能力：`modalities.input` 含 "image"（`modalities` 本身
/// 就是模态数组的来源按输入模态读）。没声明按文本模型处理——DSH 桥对未知
/// 保持保守结论，图在适配器就地省略，而不是打到上游换 400。
fn catalog_item_takes_images(item: &serde_json::Value) -> bool {
    let Some(modalities) = item.get("modalities") else {
        return false;
    };
    let input = if modalities.is_array() {
        Some(modalities)
    } else {
        modalities.get("input")
    };
    input
        .and_then(|value| value.as_array())
        .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("image")))
}

/// 目录 JSON → 模型 id 列表（保序去重）。engine.rs 的引擎目录共用。
pub(crate) fn parse_models_catalog(text: &str) -> Vec<String> {
    parse_models_catalog_rows(text)
        .into_iter()
        .map(|row| row.id)
        .collect()
}

/// 装配每个来源的最终模型清单（动态 ∪ 静态），并驱动缓存预热。
/// 返回 (models, 是否截断)。
fn assemble_models(
    credentials_path: &Path,
    opencode_scan: &crate::credentials::AgentScan,
    source_rows: &[(String, &'static ProviderPreset)],
) -> (Vec<BridgeModel>, bool) {
    // 需要刷新的来源：缓存不新鲜且取得到 key。
    let mut pending: Vec<(String, &'static ProviderPreset)> = Vec::new();
    for (entry_id, preset) in source_rows {
        if fresh_dynamic(entry_id).is_some() {
            continue;
        }
        if let Some(chat_base) = preset.chat_base {
            if peek_spawn_key_at(credentials_path, opencode_scan, entry_id).is_some() {
                let _ = chat_base; // 仅为可读性：拉取在 warm 线程里做
                pending.push((entry_id.clone(), preset));
            }
        }
    }
    if !pending.is_empty() {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let mut waiting = pending.len();
        for (entry_id, preset) in &pending {
            let entry_id = entry_id.clone();
            let chat_base = effective_chat_base(preset).unwrap_or_default();
            let key =
                peek_spawn_key_at(credentials_path, opencode_scan, &entry_id).unwrap_or_default();
            let done_tx = done_tx.clone();
            std::thread::Builder::new()
                .name("bridge-models-fetch".into())
                .spawn(move || {
                    let models = fetch_models_from_endpoint(&chat_base, &key);
                    if let Ok(mut cache) = dynamic_cache().lock() {
                        cache.insert(
                            entry_id,
                            DynamicEntry {
                                fetched_at: std::time::Instant::now(),
                                models,
                            },
                        );
                    }
                    let _ = done_tx.send(());
                })
                .ok();
        }
        let deadline = std::time::Instant::now() + DYNAMIC_BUDGET;
        while waiting > 0 {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() || done_rx.recv_timeout(remaining).is_err() {
                break;
            }
            waiting -= 1;
        }
    }
    // 装配：动态清单为主，静态清单补差；都空 → 静态（可能也为空）。
    let mut models = Vec::new();
    for (entry_id, preset) in source_rows {
        let dynamic = fresh_dynamic(entry_id).unwrap_or_default();
        let mut ids: Vec<String> = dynamic;
        for static_id in preset.models {
            if !ids.iter().any(|existing| existing == static_id) {
                ids.push(static_id.to_string());
            }
        }
        for model_id in ids {
            // label 用模型 id 本身：动态目录的名字就是用户在 opencode 里看到的
            // 那套（此前用 preset 标签是 bug——一整组全显示同一个名字）。
            models.push(BridgeModel {
                entry_id: entry_id.clone(),
                preset: preset.id.to_string(),
                label: model_id.clone(),
                model_id,
            });
        }
    }
    (models, false)
}

// ─── 路径（默认 = 真实用户目录；*_at 注入路径供测试）────────────────────────

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn credentials_path() -> Option<PathBuf> {
    Some(home_dir()?.join(".marionette").join("credentials.json"))
}

fn bindings_path() -> Option<PathBuf> {
    Some(home_dir()?.join(".marionette").join("bridge-bindings.json"))
}

pub fn grok_config_path() -> Option<PathBuf> {
    Some(home_dir()?.join(".grok").join("config.toml"))
}

/// 临时兄弟文件（进程 × 调用双唯一）—— 与 credentials::temp_sibling 同款教训。
fn temp_sibling(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("bridge-bindings.json");
    path.with_file_name(format!("{name}.{}.{seq}.tmp", std::process::id()))
}

fn write_text_atomic(path: &Path, body: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let temp = temp_sibling(path);
    fs::write(&temp, body).map_err(|e| format!("写入临时文件失败: {e}"))?;
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(format!("重命名临时文件失败: {error}"));
    }
    Ok(())
}

/// dsh_bridge 复用的原子写（同一套临时兄弟文件 + rename 纪律）。
pub(crate) fn write_text_atomic_pub(path: &Path, body: &str) -> Result<(), String> {
    write_text_atomic(path, body)
}

// ─── 绑定存储 `~/.marionette/bridge-bindings.json`（B）───────────────────────

const BINDINGS_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingFile {
    pub version: u32,
    #[serde(default)]
    pub bindings: HashMap<String, BridgeSelection>,
}

/// 损坏的桥配置改名留档（`<stem>.corrupt-<ts>.json`）。绑定是可重选的
/// 状态而非密钥，但立场仍与 credentials 一致：不覆盖原始字节、不静默清空，
/// 留档排查。zcode/pi 的托管合并入口在「原文半写坏要整体重写」前也走这里。
fn archive_corrupt(path: &Path) -> Result<PathBuf, String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("bridge-bindings.json");
    let stem = name.strip_suffix(".json").unwrap_or(name);
    let mut candidate = path.with_file_name(format!("{stem}.corrupt-{ts}.json"));
    let mut n = 1u32;
    while candidate.exists() {
        candidate = path.with_file_name(format!("{stem}.corrupt-{ts}-{n}.json"));
        n += 1;
    }
    fs::rename(path, &candidate)
        .map_err(|e| format!("{name} 已损坏且留档失败（{e}）— 已中止"))?;
    Ok(candidate)
}

/// 现有桥配置是否是可合并的 JSON object。空文件（新装，还没写过）不算损坏。
fn is_mergeable_object(text: &str) -> bool {
    text.trim().is_empty()
        || serde_json::from_str::<serde_json::Value>(text)
            .map(|value| value.is_object())
            .unwrap_or(false)
}

fn empty_bindings() -> BindingFile {
    BindingFile {
        version: BINDINGS_VERSION,
        bindings: HashMap::new(),
    }
}

/// 读取绑定文件：缺失 / 空文件 → 空；解析失败 / 版本不认识 → 留档重建。
pub fn load_bindings_at(path: &Path) -> Result<BindingFile, String> {
    if !path.is_file() {
        return Ok(empty_bindings());
    }
    let text =
        fs::read_to_string(path).map_err(|e| format!("读取 bridge-bindings.json 失败: {e}"))?;
    if text.trim().is_empty() {
        return Ok(empty_bindings());
    }
    match serde_json::from_str::<BindingFile>(&text) {
        Ok(file) if file.version == BINDINGS_VERSION => Ok(file),
        Ok(_) | Err(_) => {
            archive_corrupt(path)?;
            Ok(empty_bindings())
        }
    }
}

pub fn save_bindings_at(path: &Path, file: &BindingFile) -> Result<(), String> {
    let body = serde_json::to_string_pretty(file)
        .map_err(|e| format!("序列化 bridge-bindings.json 失败: {e}"))?;
    write_text_atomic(path, &format!("{body}\n"))
}

/// 查一个会话的绑定（acp.rs spawn 注入的入口；默认路径包装）。
pub fn binding_for(session_id: &str) -> Option<BridgeSelection> {
    let path = bindings_path()?;
    binding_for_at(&path, session_id).ok().flatten()
}

pub fn binding_for_at(path: &Path, session_id: &str) -> Result<Option<BridgeSelection>, String> {
    Ok(load_bindings_at(path)?.bindings.get(session_id).cloned())
}

/// 写 / 清绑定，返回**之前的**绑定（None = 原本没有）。
fn put_binding_at(
    path: &Path,
    session_id: &str,
    selection: Option<&BridgeSelection>,
) -> Result<Option<BridgeSelection>, String> {
    let mut file = load_bindings_at(path)?;
    let previous = file.bindings.remove(session_id);
    if let Some(selection) = selection {
        file.bindings.insert(session_id.to_string(), selection.clone());
    }
    save_bindings_at(path, &file)?;
    Ok(previous)
}

/// bridge_bind 本体（默认路径包装；命令层先把 session 的 agent_id 查出来）。
pub fn bind_session(
    session_id: &str,
    agent_id: Option<&str>,
    selection: Option<BridgeSelection>,
) -> Result<(), String> {
    let bindings = bindings_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    let credentials = credentials_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    let grok_config = grok_config_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    let scan = crate::credentials::scan_opencode();
    bind_session_at(
        &bindings,
        &credentials,
        &grok_config,
        &scan,
        session_id,
        agent_id,
        selection,
    )
}

/// 绑定的全部副作用（路径注入，测试不碰真实目录）：
/// - 绑定成功 → key-plan 记 `credentials::touch_last_used`（best-effort，失败
///   只记日志；reference 不落 store、无 lastUsedAt 可记）。
/// - 解绑（None）且原本有绑定且会话是 grok → 移除 owned 表（表还被 grok
///   advertise 着，留着会在列表里残留死条目）。claude 是 env 注入，进程退出
///   即还原，无需清理。
pub fn bind_session_at(
    bindings_path: &Path,
    credentials_path: &Path,
    grok_config_path: &Path,
    opencode_scan: &AgentScan,
    session_id: &str,
    agent_id: Option<&str>,
    selection: Option<BridgeSelection>,
) -> Result<(), String> {
    match selection {
        Some(selection) => {
            let resolved = validate_selection_at(credentials_path, opencode_scan, &selection)?;
            put_binding_at(bindings_path, session_id, Some(&selection))?;
            // §4.2 lastUsedAt：绑定时记一笔。best-effort —— 存不下不该让绑定
            // 失败。reference 型的条目不住在 store 里（nothing is stored
            // twice），没有地方记，也不该为了记账把它落盘。
            if matches!(resolved, BoundPreset::Store(_)) {
                if let Err(error) =
                    crate::credentials::touch_last_used_at(credentials_path, &selection.entry_id)
                {
                    crate::debug_log::append(
                        "bridge",
                        "warn",
                        session_id,
                        "touch_last_used failed",
                        Some(&error),
                    );
                }
            }
            crate::debug_log::append(
                "bridge",
                "info",
                session_id,
                "model bridge bound",
                Some(&format!(
                    "entry={} model={} agent={}",
                    selection.entry_id,
                    selection.model_id,
                    agent_id.unwrap_or("(unknown)")
                )),
            );
            Ok(())
        }
        None => {
            let previous = put_binding_at(bindings_path, session_id, None)?;
            if previous.is_none() {
                return Ok(());
            }
            let was_grok =
                matches!(classify_agent(agent_id.unwrap_or("")), BridgeAgent::GrokBuild);
            let was_codex = matches!(classify_agent(agent_id.unwrap_or("")), BridgeAgent::Codex);
            if was_grok {
                match remove_grok_model_tables_at(grok_config_path) {
                    Ok(_) => crate::debug_log::append(
                        "bridge",
                        "info",
                        session_id,
                        "grok owned model tables removed on unbind",
                        None,
                    ),
                    Err(error) => crate::debug_log::append(
                        "bridge",
                        "warn",
                        session_id,
                        "failed to remove grok owned model tables",
                        Some(&error),
                    ),
                }
            } else if was_codex {
                // codex 的注入是 spawn 级 providers/set（前端绑/解绑都会重启
                // 进程，还原走重拉），引擎侧的路由组别名在这里清：无人引用
                // 即删，别的会话还指着就保留。
                crate::engine::codex_alias_clear(session_id);
                crate::debug_log::append(
                    "bridge",
                    "info",
                    session_id,
                    "model bridge unbound (codex engine alias cleared)",
                    None,
                );
            } else {
                // claude 是 env 注入，进程退出即消失，无需还原（§4.4）。
                crate::debug_log::append("bridge", "info", session_id, "model bridge unbound", None);
            }
            Ok(())
        }
    }
}

/// 绑定 entry_id 解析出的直连 preset，以及它来自哪一侧。
enum BoundPreset {
    /// 凭据库里的 key-plan（preset 已核实存在）。
    Store(&'static ProviderPreset),
    /// opencode auth.json 现场扫描出的 reference（preset 由 provider 映射命中）。
    Reference(&'static ProviderPreset),
    /// M2 翻译引擎伪来源：不需要凭据（令牌是字面量 `magpie`，见
    /// `peek_spawn_key_at`），model_id 即引擎的 `provider/model` id。
    Engine,
}

/// 绑定 entry_id → 直连 preset 的统一解析（validate / injection 共用）。
///
/// key-plan 走凭据库；reference 匹配实时扫描结果 —— 扫描是 reference 的唯一
/// 权威来源（§2「用时现场读」），auth.json 里的条目消失或改成 OAuth 登录态后
/// 这里就解析不出，Err 给 acp.rs 走「残留绑定降级为普通会话 + warn」的既有
/// 路径。
fn resolve_bound_preset(
    credentials_path: &Path,
    opencode_scan: &AgentScan,
    entry_id: &str,
) -> Result<BoundPreset, String> {
    let store = crate::credentials::load_store_at(credentials_path)?;
    if let Some(entry) = store.entries.iter().find(|entry| entry.id == entry_id) {
        if entry.kind != CredentialKind::KeyPlan {
            return Err(format!(
                "凭证 `{entry_id}` 不是 key 型 plan（kind = {}），无法直连",
                kind_str(entry.kind)
            ));
        }
        let preset_id = entry
            .preset
            .as_deref()
            .ok_or_else(|| format!("凭证 `{entry_id}` 缺少 preset"))?;
        let preset = find_preset(preset_id)
            .ok_or_else(|| format!("凭证 `{entry_id}` 的 preset `{preset_id}` 不在预设表内"))?;
        return Ok(BoundPreset::Store(preset));
    }
    if let Some(entry) = opencode_scan.entries.iter().find(|entry| {
        entry.id == entry_id && entry.kind == CredentialKind::Reference
    }) {
        let provider = ref_path_provider(entry.ref_path.as_deref()).ok_or_else(|| {
            format!("凭证 `{entry_id}` 的 ref 路径解析不出 provider-id，无法直连")
        })?;
        let preset = preset_for_opencode_provider(&provider).ok_or_else(|| {
            format!(
                "opencode provider `{provider}` 未映射到任何直连 preset \
                （没有核实过的 L1 端点），无法直连"
            )
        })?;
        return Ok(BoundPreset::Reference(preset));
    }
    Err(format!(
        "绑定引用的凭证 `{entry_id}` 不存在（凭据库与 opencode auth.json 的扫描里都没有）"
    ))
}

/// 绑定校验：key-plan 须在凭据库内且 preset 在表内；reference 须 ref 路径
/// 解析得出 provider-id 且映射到 preset；**engine 伪来源**（M2）只要求引擎
/// 在 Running（令牌用字面量，见 [`BoundPreset::Engine`]）。返回解析结果，供
/// 调用方区分来源（lastUsedAt 只记 key-plan）。
fn validate_selection_at(
    credentials_path: &Path,
    opencode_scan: &AgentScan,
    selection: &BridgeSelection,
) -> Result<BoundPreset, String> {
    if selection.model_id.trim().is_empty() {
        return Err("model_id 不能为空".to_string());
    }
    if selection.entry_id == crate::engine::ENGINE_ENTRY_ID {
        return if crate::engine::bridge_ready() {
            Ok(BoundPreset::Engine)
        } else {
            Err(
                "翻译引擎未在运行 — 请先在设置的凭据页启用（引擎就绪后重新选择模型）"
                    .to_string(),
            )
        };
    }
    resolve_bound_preset(credentials_path, opencode_scan, &selection.entry_id)
}

// ─── grok owned 表写入器（C / S3 实测约束）──────────────────────────────────

/// owned 段的所有权标记行。整行匹配才认领 / 删除。
const GROK_OWNED_MARKER: &str = "# ── marionette key-bridge (owned) ──";
/// 我们在 grok config 里的模型命名空间前缀。
const GROK_MODEL_PREFIX: &str = "marionette/";
/// grok owned 表引用的 env 名（spawn 时注入明文 key；`$VAR` 展开实测有效）。
pub const GROK_KEY_ENV: &str = "MARIONETTE_PROVIDER_KEY";

/// TOML basic string 转义（表头键与值共用；只转义会破坏引号的两个字符）。
fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn grok_table_name(model_id: &str) -> String {
    format!("{GROK_MODEL_PREFIX}{model_id}")
}

fn grok_table_header(name: &str) -> String {
    format!("[model.\"{}\"]", toml_escape(name))
}

/// 我们想要的 owned 块全文（标记行 + 表头 + 五个字段）。
/// `base_url` 由调用方给：生产路径是 grok_dialect 规整器的环回地址（剥离
/// `model_id` 方言字段后再转发到 preset 的 chat 端点），测试可传任意字面量。
fn grok_owned_block(model_id: &str, preset: &ProviderPreset, base_url: &str, key_env_name: &str) -> String {
    format!(
        concat!(
            "{marker}\n",
            "{header}\n",
            "model = \"{model}\"\n",
            "name = \"{name} (via Marionette)\"\n",
            "base_url = \"{base}\"\n",
            "api_backend = \"chat_completions\"\n",
            "api_key = \"${key_env}\"\n",
        ),
        marker = GROK_OWNED_MARKER,
        header = grok_table_header(&grok_table_name(model_id)),
        model = toml_escape(model_id),
        name = toml_escape(preset.label),
        base = toml_escape(base_url),
        key_env = key_env_name,
    )
}

/// grok channel 的 base_url：走方言规整器（环回，剥 `model_id` 后转发到
/// preset.chat_base）。规整器起不来（端口全被占之类的极端情形）时退回直连
/// 端点——多轮对话会踩 400，但单轮可用，好过完全不可用；退化要留痕。
/// 跨用 channel 的 base：走方言规整器（环回，按路径区分 anthropic/chat 上游，
/// 剥离已知方言字段、按 preset 叠加必需头——opencode-go 的会话头）后再转发。
/// claude 与 grok 共用同一地址（协议由 agent 自拼的路径决定）。规整器起不来
/// （端口全被占之类的极端情形）时退回直连端点——opencode-go 目标会缺会话头
/// 而不可用，其余 preset 多轮可能踩方言 400，但好过完全不可用；退化要留痕。
fn channel_base(preset: &ProviderPreset) -> String {
    let fallback = preset.chat_base.unwrap_or_default().to_string();
    match crate::grok_dialect::channel_base_url(preset.id) {
        Ok(url) => url,
        Err(error) => {
            crate::debug_log::append(
                "bridge",
                "warn",
                "",
                "dialect normalizer unavailable, falling back to direct endpoint",
                Some(&error),
            );
            fallback
        }
    }
}

/// claude 侧的直连回退用 anthropic 端点（claude 拼的是 /v1/messages）。
fn claude_channel_base(preset: &ProviderPreset) -> String {
    let fallback = preset.anthropic_base.unwrap_or_default().to_string();
    match crate::grok_dialect::channel_base_url(preset.id) {
        Ok(url) => url,
        Err(error) => {
            crate::debug_log::append(
                "bridge",
                "warn",
                "",
                "dialect normalizer unavailable, claude falls back to direct anthropic endpoint",
                Some(&error),
            );
            fallback
        }
    }
}

/// 一段文档：首个表头之前的序言段（header = None），或一张表（表头行 + 属于
/// 它的行，表头行自身也在 lines 里）。
#[derive(Debug, Clone)]
struct TomlBlock {
    header: Option<String>,
    lines: Vec<String>,
}

/// 按表头切分 TOML 文本，行级原文搬运。
///
/// 对多行字符串（`"""` / `'''`）里的伪装表头行保持不敏感：进入多行字符串后
/// 逐行只判闭合定界符，不认表头。真实文件另由 toml crate 整体校验兜底
/// （ensure / remove 在手术前后都会校验）。
fn split_blocks(text: &str) -> Vec<TomlBlock> {
    let mut blocks: Vec<TomlBlock> = Vec::new();
    let mut multiline: Option<&'static str> = None;
    // 序言块惰性创建：任何要放行的时刻若还没有块就建一个 header=None 块。
    // 旧实现靠「初始必有一个块」的隐式不变量 + last_mut().unwrap()，panic=abort
    // 下万一被重排破坏就是进程自杀。
    for raw in text.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw).to_string();
        if let Some(delim) = multiline {
            push_split_line(&mut blocks, line.clone());
            if line.matches(delim).count() % 2 == 1 {
                multiline = None;
            }
            continue;
        }
        if line.trim_start().starts_with('[') {
            blocks.push(TomlBlock {
                header: Some(line.trim_start().to_string()),
                lines: vec![line.clone()],
            });
        } else {
            push_split_line(&mut blocks, line.clone());
        }
        for delim in ["\"\"\"", "'''"] {
            if line.matches(delim).count() % 2 == 1 {
                multiline = Some(delim);
            }
        }
    }
    if blocks.is_empty() {
        blocks.push(TomlBlock {
            header: None,
            lines: Vec::new(),
        });
    }
    blocks
}

/// 行放入最后一个块；还没有块（空输入首行）就建序言块。任何路径都不 panic。
fn push_split_line(blocks: &mut Vec<TomlBlock>, line: String) {
    if let Some(last) = blocks.last_mut() {
        last.lines.push(line);
    } else {
        blocks.push(TomlBlock {
            header: None,
            lines: vec![line],
        });
    }
}

/// 解析 `[model."<name>"]` / `[model.<bare>]` 表头 → name。
/// `models`（复数表）与其它表不认 —— 与 grok channel 的语义一致。
fn model_table_name(header: &str) -> Option<String> {
    let header = header.trim();
    if !header.starts_with('[') || header.starts_with("[[") {
        return None;
    }
    let end = header.find(']')?;
    let inner = header[1..end].trim();
    let rest = inner.strip_prefix("model")?;
    let rest = rest.strip_prefix('.')?;
    parse_quoted_key(rest)
}

/// 解析 dotted key 的余段：`"quoted"`（含转义）、`'literal'` 或裸 key。
fn parse_quoted_key(part: &str) -> Option<String> {
    if let Some(quoted) = part.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = quoted.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.push(chars.next()?),
                '"' => return Some(out),
                _ => out.push(c),
            }
        }
        None // 没闭合 —— 不是合法表头
    } else if let Some(literal) = part.strip_prefix('\'') {
        literal.split_once('\'').map(|(name, _)| name.to_string())
    } else {
        part.split('.')
            .next()
            .map(|bare| bare.trim().to_string())
    }
}

fn blocks_to_text(blocks: &[TomlBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        for line in &block.lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// 引擎版 owned 块：base_url 指引擎的 `/v1`（grok 的 chat_completions 后端
/// 向 base_url 拼 `/chat/completions`），api_key 用字面量令牌（S1：引擎回环
/// 任意 token 放行——真实订阅凭证在引擎自己的 data 里，永不进 agent 配置）。
fn grok_engine_owned_block(model_id: &str, base_url: &str) -> String {
    format!(
        concat!(
            "{marker}\n",
            "{header}\n",
            "model = \"{model}\"\n",
            "name = \"{name} (via Marionette)\"\n",
            "base_url = \"{base}\"\n",
            "api_backend = \"chat_completions\"\n",
            "api_key = \"{key}\"\n",
        ),
        marker = GROK_OWNED_MARKER,
        header = grok_table_header(&grok_table_name(model_id)),
        model = toml_escape(model_id),
        name = toml_escape("翻译引擎"),
        base = toml_escape(base_url),
        key = toml_escape(crate::engine::ENGINE_TOKEN),
    )
}

/// ensure 的核心（块级替换 + 幂等 + 原文搬运；`desired` 是完整 owned 块全文，
/// `wanted_name` 是表名）。原则见 [`ensure_grok_model_table_at`] 的注释。
fn ensure_grok_block_at(
    config_path: &Path,
    wanted_name: &str,
    desired: &str,
) -> Result<bool, String> {
    let original = match fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("读取 grok config.toml 失败: {error}")),
    };
    if !original.trim().is_empty() {
        toml::from_str::<toml::Value>(&original)
            .map_err(|e| format!("grok config.toml 解析失败（{e}）— 拒绝写入半截"))?;
    }

    let mut blocks = split_blocks(&original);
    let existing = blocks.iter().position(|block| {
        block
            .header
            .as_deref()
            .and_then(model_table_name)
            .map(|name| name == wanted_name)
            .unwrap_or(false)
    });

    match existing {
        Some(index) => {
            let block = &blocks[index];
            // 已存在的块开头可能带我们旧版的标记行；比较时剥离，正文（表头+
            // 字段）逐字节一致才算幂等。
            let body: String = block
                .lines
                .iter()
                .filter(|line| line.trim() != GROK_OWNED_MARKER)
                .map(|line| format!("{line}\n"))
                .collect();
            let desired_body: String = desired
                .lines()
                .filter(|line| !line.trim().is_empty() && *line != GROK_OWNED_MARKER)
                .map(|line| format!("{line}\n"))
                .collect();
            if body == desired_body {
                return Ok(false);
            }
            blocks[index] = TomlBlock {
                header: block.header.clone(),
                lines: desired.lines().map(str::to_string).collect(),
            };
        }
        None => {
            // 追加到文件尾。原文件非空且无结尾换行时补一个空行，避免表头粘行。
            if !original.trim().is_empty() && !original.ends_with('\n') {
                if let Some(last) = blocks.last_mut() {
                    last.lines.push(String::new());
                }
            }
            blocks.push(TomlBlock {
                header: Some(grok_table_header(wanted_name)),
                lines: desired.lines().map(str::to_string).collect(),
            });
        }
    }

    let output = blocks_to_text(&blocks);
    toml::from_str::<toml::Value>(&output)
        .map_err(|e| format!("生成的 grok config.toml 未通过 TOML 校验（{e}）— 已放弃写入"))?;
    write_text_atomic(config_path, &output)?;
    Ok(true)
}

/// ensure 的本体：写 / 更新 grok config.toml 里的 owned 表。
///
/// 返回 `Ok(true)` = 写入了；`Ok(false)` = 已存在且逐字节一致（零写入）。
/// 原则（S3 实测约束）：
/// - **原文搬运**其余内容（用户自定义表、grok 自己写的 `[marketplace]` 段）。
/// - **绝不设 `[models] default`**（会让 Grok 退出自身登录，§10.6）。
/// - **幂等**：一致则不碰文件。
/// - **写前重读**：每次调用现读现写（无全文缓存），修改是块级替换，容忍
///   grok 并发自改写。
/// - **解析失败不写半截**：原文或产物任一过不了 toml 校验就中止且不动文件。
pub fn ensure_grok_model_table_at(
    config_path: &Path,
    model_id: &str,
    preset: &ProviderPreset,
    key_env_name: &str,
) -> Result<bool, String> {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return Err("model_id 不能为空".to_string());
    }
    if preset.chat_base.is_none() {
        return Err(format!(
            "preset `{}` 没有核实过的 chat 端点，拒绝写 grok 表",
            preset.id
        ));
    }
    let desired = grok_owned_block(model_id, preset, &channel_base(preset), key_env_name);
    ensure_grok_block_at(config_path, &grok_table_name(model_id), &desired)
}

/// 引擎版 ensure 的路径注入体：引擎不在 Running → Err（残留绑定降级路径）。
pub fn ensure_grok_engine_table_at(
    config_path: &Path,
    model_id: &str,
    base_url: &str,
) -> Result<bool, String> {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return Err("model_id 不能为空".to_string());
    }
    let desired = grok_engine_owned_block(model_id, base_url);
    ensure_grok_block_at(config_path, &grok_table_name(model_id), &desired)
}

/// remove 的本体：删掉**所有** `marionette/` 前缀的 owned 表 + 所有 owned
/// 标记行（表删光了标记必然孤立，含残留的半个 owned 段）。返回是否动了文件。
pub fn remove_grok_model_tables_at(config_path: &Path) -> Result<bool, String> {
    let original = match fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("读取 grok config.toml 失败: {error}")),
    };
    if original.trim().is_empty() {
        return Ok(false);
    }
    toml::from_str::<toml::Value>(&original)
        .map_err(|e| format!("grok config.toml 解析失败（{e}）— 拒绝写入半截"))?;

    let mut blocks = split_blocks(&original);
    // 只删 marionette/ 前缀的 model 表；序言、非 model 表（[cli]/[models]/
    // [marketplace]/用户自定义表）一律保留。
    blocks.retain(|block| {
        match block.header.as_deref().and_then(model_table_name) {
            Some(name) => !name.starts_with(GROK_MODEL_PREFIX),
            None => true,
        }
    });
    for block in &mut blocks {
        block.lines.retain(|line| line.trim() != GROK_OWNED_MARKER);
    }
    // 尾部只剩空行的段收掉，避免文件尾堆积空行。
    while blocks.len() > 1 {
        let last_is_empty = blocks
            .last()
            .map(|block| block.lines.iter().all(|line| line.trim().is_empty()))
            .unwrap_or(false);
        if last_is_empty {
            blocks.pop();
        } else {
            break;
        }
    }
    let output = blocks_to_text(&blocks);
    if output == original {
        return Ok(false);
    }
    if !output.trim().is_empty() {
        toml::from_str::<toml::Value>(&output).map_err(|e| {
            format!("生成的 grok config.toml 未通过 TOML 校验（{e}）— 已放弃写入")
        })?;
    }
    write_text_atomic(config_path, &output)?;
    Ok(true)
}

/// 真实路径包装：ensure（key env 固定 `MARIONETTE_PROVIDER_KEY`）。M2：引擎
/// 绑定走 `preset_id == "engine"` 分支 —— base_url 指引擎的 `/v1`、api_key
/// 用字面量令牌（acp.rs 的 SpawnInjection::GrokTable 传进来的就是它）。
pub fn ensure_grok_model_table(model_id: &str, preset_id: &str) -> Result<bool, String> {
    let path = grok_config_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    if preset_id == crate::engine::ENGINE_PRESET_ID {
        let base_url = crate::engine::engine_v1_url().ok_or_else(|| {
            "翻译引擎未在运行，无法写 grok 引擎表（绑定已降级）".to_string()
        })?;
        return ensure_grok_engine_table_at(&path, model_id, &base_url);
    }
    let preset = find_preset(preset_id).ok_or_else(|| format!("未知 preset `{preset_id}`"))?;
    ensure_grok_model_table_at(&path, model_id, preset, GROK_KEY_ENV)
}

/// 真实路径包装：删除全部 marionette/ owned 表。目前没有调用方（解绑路径
/// 走 `bind_session` → `remove_grok_model_tables_at` 的路径注入版）；保留
/// 是为了给未来的全局清理（如设置页「移除全部跨用表」）一个直接入口。
#[allow(dead_code)]
pub fn remove_grok_model_tables() -> Result<bool, String> {
    let path = grok_config_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    remove_grok_model_tables_at(&path)
}

/// 智谱 Coding Plan 的 OpenAI Chat 端点。套餐 key 打到按量 `/api/paas/v4`
/// 会 429 / code 1113「余额不足或无可用资源包,请充值。」——ZCode 能用、Grok
/// 自定义 `zhipu-glm-*` 表不能用，就是这个差。
const ZHIPU_PAYG_CHAT: &str = "https://open.bigmodel.cn/api/paas/v4";
const ZHIPU_CODING_CHAT: &str = "https://open.bigmodel.cn/api/coding/paas/v4";

/// Grok spawn 时：把用户 `~/.grok/config.toml` 里、api_key 是已知 Coding Plan
/// 钥匙、却指向按量 `/api/paas/v4` 的 model 表改到 coding 端点。
///
/// 只动匹配到套餐钥匙的表；按量钥匙、`$ENV` 展开（我们自己的 owned 表）、
/// 已经是 coding 端点的，一律不碰。失败不阻塞 spawn。
pub fn rewrite_grok_zhipu_coding_plan_bases() -> Result<bool, String> {
    let path = grok_config_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    rewrite_grok_zhipu_coding_plan_bases_at(&path, &glm_coding_plan_keys())
}

pub fn rewrite_grok_zhipu_coding_plan_bases_at(
    config_path: &Path,
    coding_plan_keys: &[String],
) -> Result<bool, String> {
    if coding_plan_keys.is_empty() {
        return Ok(false);
    }
    let original = match fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("读取 grok config.toml 失败: {error}")),
    };
    if original.trim().is_empty() {
        return Ok(false);
    }
    toml::from_str::<toml::Value>(&original)
        .map_err(|e| format!("grok config.toml 解析失败（{e}）— 拒绝写入半截"))?;

    let keys: HashSet<&str> = coding_plan_keys
        .iter()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
        .collect();
    if keys.is_empty() {
        return Ok(false);
    }

    let mut blocks = split_blocks(&original);
    let mut changed = false;
    for block in &mut blocks {
        if block.header.as_deref().and_then(model_table_name).is_none() {
            continue;
        }
        let Some(api_key) = toml_field_in_block(block, "api_key").map(str::to_string) else {
            continue;
        };
        if api_key.starts_with('$') || !keys.contains(api_key.trim()) {
            continue;
        }
        let Some(base) = toml_field_in_block(block, "base_url").map(str::to_string) else {
            continue;
        };
        if !is_zhipu_payg_chat(&base) {
            continue;
        }
        for line in &mut block.lines {
            if toml_field_name(line) != Some("base_url") {
                continue;
            }
            let rewritten = rewrite_zhipu_payg_line(line);
            if rewritten != *line {
                *line = rewritten;
                changed = true;
            }
        }
    }
    if !changed {
        return Ok(false);
    }

    let output = blocks_to_text(&blocks);
    toml::from_str::<toml::Value>(&output).map_err(|e| {
        format!("生成的 grok config.toml 未通过 TOML 校验（{e}）— 已放弃写入")
    })?;
    write_text_atomic(config_path, &output)?;
    Ok(true)
}

fn glm_coding_plan_keys() -> Vec<String> {
    let mut keys: Vec<String> = crate::credentials::peek_zcode_coding_plan_keys()
        .into_iter()
        .filter(|(preset, key)| preset == "glm" && !key.trim().is_empty())
        .map(|(_, key)| key)
        .collect();
    for (provider, key) in crate::credentials::peek_opencode_api_keys() {
        if key.trim().is_empty() {
            continue;
        }
        if preset_for_opencode_provider(&provider).is_some_and(|preset| preset.id == "glm") {
            keys.push(key);
        }
    }
    if let Some(path) = credentials_path() {
        if let Ok(store) = crate::credentials::load_store_at(&path) {
            for entry in store.entries.iter().filter(|entry| {
                entry.kind == CredentialKind::KeyPlan && entry.preset.as_deref() == Some("glm")
            }) {
                if let Ok(Some(key)) = crate::credentials::peek_secret_at(&path, &entry.id) {
                    if !key.trim().is_empty() {
                        keys.push(key);
                    }
                }
            }
        }
    }
    keys
}

fn is_zhipu_payg_chat(url: &str) -> bool {
    url.trim().trim_end_matches('/') == ZHIPU_PAYG_CHAT
}

fn rewrite_zhipu_payg_line(line: &str) -> String {
    line.replace(&format!("{ZHIPU_PAYG_CHAT}/"), &format!("{ZHIPU_CODING_CHAT}/"))
        .replace(ZHIPU_PAYG_CHAT, ZHIPU_CODING_CHAT)
}

fn toml_field_name(line: &str) -> Option<&str> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') {
        return None;
    }
    let (key, _) = t.split_once('=')?;
    Some(key.trim())
}

fn toml_field_in_block<'a>(block: &'a TomlBlock, name: &str) -> Option<&'a str> {
    for line in &block.lines {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some((key, rest)) = t.split_once('=') else {
            continue;
        };
        if key.trim() != name {
            continue;
        }
        return Some(toml_unquote(rest.trim()));
    }
    None
}

fn toml_unquote(value: &str) -> &str {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('"') {
        return inner.split('"').next().unwrap_or(inner);
    }
    if let Some(inner) = value.strip_prefix('\'') {
        return inner.split('\'').next().unwrap_or(inner);
    }
    value.split_whitespace().next().unwrap_or(value)
}

// ─── spawn 注入解析（D；acp.rs 的 env 注入段调用）───────────────────────────

/// 一个会话该拿到的注入形态。key 明文**不在这里** —— acp.rs 用
/// `peek_spawn_key`（key-plan = 凭据库 DPAPI 现解；reference = 现场读原文件）
/// 解出后直接放进 env，不在结构体间传递。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnInjection {
    /// claude-code：三个 env（base / token / model）。§4.8：跨用会话**不发**
    /// set_model，模型由 env 钉住。
    ClaudeEnv {
        base_url: String,
        model: String,
    },
    /// grok：先 ensure_grok_model_table（`marionette/<model_id>` 表），再 env
    /// MARIONETTE_PROVIDER_KEY=key（表里的 `$MARIONETTE_PROVIDER_KEY` 展开）。
    /// §4.8：grok 相反，**必须发** set_model 选 `marionette/<model>`（前端职责）。
    GrokTable {
        model_id: String,
        preset_id: String,
    },
}

/// 按注入策略把 (preset, agent, model) 解析成注入形态。
/// None = 该 agent 没有对应端点或不归 M1 管 —— 静默跳过。
fn injection_for(
    preset: &ProviderPreset,
    agent: BridgeAgent,
    model_id: &str,
) -> Option<SpawnInjection> {
    match agent {
        BridgeAgent::ClaudeCode => preset
            .anthropic_base
            .map(|_direct| SpawnInjection::ClaudeEnv {
                // 走规整器：opencode-go 的 /anthropic 端点要求会话头（claude
                // 不会自发带），且未来 claude 侧方言（billing 行等）在此落地。
                base_url: claude_channel_base(preset),
                model: model_id.to_string(),
            }),
        BridgeAgent::GrokBuild => preset
            .chat_base
            .is_some()
            .then(|| SpawnInjection::GrokTable {
                model_id: model_id.to_string(),
                preset_id: preset.id.to_string(),
            }),
        _ => None,
    }
}

/// spawn 注入解析（默认路径包装；acp.rs 用）。
/// key-plan 与 reference 统一走 [`resolve_bound_preset`]：解析不出条目 / 映射
/// 不上 preset → Err 说明为什么跳过（绑定可能引用了已消失的凭证或 auth.json
/// 里改成了登录态 —— 残留无害，但要留下线索）。
pub fn injection_for_agent(
    agent_id: &str,
    entry_id: &str,
    model_id: &str,
) -> Result<Option<SpawnInjection>, String> {
    let path = credentials_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    let scan = crate::credentials::scan_opencode();
    injection_for_agent_at(&path, &scan, agent_id, entry_id, model_id)
}

pub fn injection_for_agent_at(
    credentials_path: &Path,
    opencode_scan: &AgentScan,
    agent_id: &str,
    entry_id: &str,
    model_id: &str,
) -> Result<Option<SpawnInjection>, String> {
    let agent = classify_agent(agent_id);
    if !matches!(agent, BridgeAgent::ClaudeCode | BridgeAgent::GrokBuild) {
        return Ok(None);
    }
    // ── M2 翻译引擎伪来源：不经 preset 表 —— claude 拿引擎根（引擎在根上
    //    服务 /v1/messages），grok 复用 owned 表机制但 base_url 指引擎的
    //    /v1、api_key 用字面量（见 ensure_grok_model_table 的 engine 分支）。
    //    引擎不在 Running → Err（acp.rs 走残留绑定降级路径：warn + 普通会话）。
    if entry_id == crate::engine::ENGINE_ENTRY_ID {
        if !crate::engine::bridge_ready() {
            return Err(format!(
                "翻译引擎未在运行（绑定 `{}` 已失效）— 降级为普通会话",
                crate::engine::ENGINE_ENTRY_ID
            ));
        }
        let Some(port) = crate::engine::running_base().map(|(port, _)| port) else {
            return Err("翻译引擎状态不可用 — 降级为普通会话".to_string());
        };
        return Ok(match agent {
            BridgeAgent::ClaudeCode => Some(SpawnInjection::ClaudeEnv {
                base_url: format!("http://127.0.0.1:{port}"),
                model: model_id.to_string(),
            }),
            BridgeAgent::GrokBuild => Some(SpawnInjection::GrokTable {
                model_id: model_id.to_string(),
                preset_id: crate::engine::ENGINE_PRESET_ID.to_string(),
            }),
            _ => unreachable!("claude/grok 已在上方过滤"),
        });
    }
    // Reference 与 KeyPlan 走同一套注入（claude env / grok owned 表）——
    // 唯一的区别是钥匙来源（见 peek_spawn_key），注入形态完全一致。
    // （引擎伪来源已在上方单独返回，resolve_bound_preset 不产出 Engine。）
    let preset = match resolve_bound_preset(credentials_path, opencode_scan, entry_id)? {
        BoundPreset::Store(preset) | BoundPreset::Reference(preset) => preset,
        BoundPreset::Engine => unreachable!("engine entry is handled above"),
    };
    Ok(injection_for(preset, agent, model_id))
}

// ─── reference 现场读钥匙（C；§2「用时现场读原文件」）──────────────────────

/// 现场读一条 reference 的 key 明文。按 ref_path 找到 auth.json、按 provider-id
/// 取条目；条目语义复用 `provider_usage::strict_api_key_of` —— OAuth / 混合
/// 条目拒绝（直连需要真 API key），key 有效性规则与 `auth_key_for` 一致。
/// 读到的明文**绝不打日志、绝不落盘**，只进调用方注入的子进程 env（§5）。
pub fn peek_reference_secret(entry: &CredentialEntry) -> Result<String, String> {
    let ref_path = entry
        .ref_path
        .as_deref()
        .ok_or_else(|| format!("reference `{}` 没有 ref 路径", entry.id))?;
    let (path, provider) = ref_path
        .rsplit_once('#')
        .ok_or_else(|| format!("ref 路径不是 `文件路径#provider-id` 形态: {ref_path}"))?;
    let text = fs::read_to_string(path)
        .map_err(|e| format!("读取 reference 源文件失败（`{provider}`）: {e}"))?;
    let auth: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("reference 源文件解析失败（`{provider}`）: {e}"))?;
    let value = auth
        .get(provider)
        .ok_or_else(|| format!("`{provider}` 条目已不在 reference 源文件里"))?;
    crate::provider_usage::strict_api_key_of(value)
        .map_err(|e| format!("`{provider}` 的 key 现场读失败: {e}"))
}

/// spawn 注入的钥匙统一解析（acp.rs 的注入段调用）：key-plan → 凭据库 DPAPI
/// 现解；reference → 现场扫描匹配后读原文件。明文只在内存与子进程 env（§5）。
pub fn peek_spawn_key(entry_id: &str) -> Option<String> {
    let path = credentials_path()?;
    let scan = crate::credentials::scan_opencode();
    peek_spawn_key_at(&path, &scan, entry_id)
}

fn peek_spawn_key_at(
    credentials_path: &Path,
    opencode_scan: &AgentScan,
    entry_id: &str,
) -> Option<String> {
    // M2 引擎伪来源：令牌是字面量 `magpie`（S1：网关回环任意 token 放行，
    // 真实订阅凭证永不进 agent 配置）。引擎是否在跑由 injection_for_agent_at
    // 判定——那边 Err 时这把钥匙根本不会被用。
    if entry_id == crate::engine::ENGINE_ENTRY_ID {
        return Some(crate::engine::ENGINE_TOKEN.to_string());
    }
    // key-plan：store 现解（错误按既有语义 flat 成 None）。
    if let Some(key) = crate::credentials::peek_secret_at(credentials_path, entry_id)
        .ok()
        .flatten()
    {
        return Some(key);
    }
    // reference：扫描结果里按 id 匹配后现场读。匹配不到 / 读不出（源文件没了、
    // 条目改成 OAuth）→ None —— acp.rs 侧配合 injection 的结果走降级路径。
    let entry = opencode_scan
        .entries
        .iter()
        .find(|entry| entry.id == entry_id && entry.kind == CredentialKind::Reference)?;
    peek_reference_secret(entry).ok()
}

// ─── codex providers/set 框架（D；M1 只搭框架不触发）────────────────────────

/// `providers/set` 的参数（S2 实测处方：codex-acp 只认 `apiType:"openai"` =
/// Responses 协议；key 经 headers 进程内传递，零落盘）。acp.rs 在 initialize
/// 应答后调用。
pub(crate) fn codex_providers_set_params(base_url: &str, api_key: &str) -> serde_json::Value {
    serde_json::json!({
        "providerId": "openai",
        "apiType": "openai",
        "baseUrl": base_url,
        "headers": { "Authorization": format!("Bearer {api_key}") },
    })
}

/// codex 绑定 → (baseUrl, key)。引擎伪来源：baseUrl 用引擎的 `/v1` 根 ——
/// codex-acp 把它写进 `model_providers.custom-gateway` 的 `base_url`
/// （wire_api 固定 `responses`），codex 自己向 `<baseUrl>/responses` 发请求，
/// 而 magpie 恰好服务 `POST /v1/responses`（2026-10-16 源码核实）。key 用
/// 引擎令牌字面量（回环任意 token 放行）。key-plan/reference 端点仍是
/// chat-completions，被 codex 的 Responses-only 拒（§4.4 S2）→ None。
pub(crate) fn codex_bridge_route(selection: &BridgeSelection) -> Option<(String, String)> {
    if selection.entry_id != crate::engine::ENGINE_ENTRY_ID {
        return None;
    }
    crate::engine::engine_v1_url().map(|base| (base, crate::engine::ENGINE_TOKEN.to_string()))
}

/// codex × 引擎绑定：把会话当前内建名同步成引擎路由组别名。调用点 = acp.rs
/// 的 session/new 之后、codex 原生切模型之后；解绑清理走 `bind_session_at`。
/// 非 codex / 未绑引擎 / caps 没报当前模型 → 静默 no-op（codex 的 model
/// select 必带 currentValue，None 只是防御）。
pub(crate) fn sync_codex_engine_alias(
    session_id: &str,
    agent_id: Option<&str>,
    current_model: Option<&str>,
) {
    let Some(path) = bindings_path() else { return };
    sync_codex_engine_alias_at(&path, session_id, agent_id, current_model);
}

fn sync_codex_engine_alias_at(
    bindings_path: &Path,
    session_id: &str,
    agent_id: Option<&str>,
    current_model: Option<&str>,
) {
    if classify_agent(agent_id.unwrap_or("")) != BridgeAgent::Codex {
        return;
    }
    let Some(current) = current_model else { return };
    let Some(selection) = binding_for_at(bindings_path, session_id).ok().flatten() else {
        return;
    };
    if selection.entry_id != crate::engine::ENGINE_ENTRY_ID {
        return;
    }
    crate::engine::codex_alias_sync(session_id, current, &selection.model_id);
}

/// initialize 应答是否声明了 `agentCapabilities.providers`（providers/set 的
/// 前置条件，S2 实测）。
pub(crate) fn advertises_providers(initialize_result: &serde_json::Value) -> bool {
    initialize_result
        .get("agentCapabilities")
        .and_then(|caps| caps.get("providers"))
        .map(|providers| !providers.is_null())
        .unwrap_or(false)
}

// ─── 命令包装（commands.rs 调）──────────────────────────────────────────────

/// bridge_delete_entry：直接落到 credentials 存储。
pub fn delete_entry(id: &str) -> Result<(), String> {
    crate::credentials::delete_entry(id)
}

// ─── 测试 ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_bridge_{tag}_{}_{}",
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

    // ─── presets 完整性 ─────────────────────────────────────────────────────

    /// 每个 preset 至少一个非 None base（否则它谁也服务不了）；id 唯一；
    /// DeepSeek 双端点这两个已知锚点不许被改坏。
    #[test]
    fn presets_table_is_complete_and_anchored() {
        let table = presets();
        assert!(
            table.len() >= 6,
            "至少六家：deepseek/opencode-go/glm/kimi/openrouter/xai"
        );
        let mut ids: Vec<&str> = table.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), table.len(), "preset id 必须唯一");
        for preset in table {
            assert!(
                preset.anthropic_base.is_some() || preset.chat_base.is_some(),
                "preset `{}` 两个端点都是 None —— 至少要有一个核实过的 base",
                preset.id
            );
            assert!(!preset.last_verified.is_empty(), "preset `{}` 缺 last_verified", preset.id);
        }
        for required in ["deepseek", "opencode-go", "glm", "kimi", "openrouter", "xai"] {
            assert!(find_preset(required).is_some(), "缺 preset `{required}`");
        }
        let deepseek = find_preset("deepseek").unwrap();
        assert_eq!(deepseek.anthropic_base, Some("https://api.deepseek.com/anthropic"));
        assert_eq!(deepseek.chat_base, Some("https://api.deepseek.com/v1"));
        // 端点必须是 https 绝对 URL（要写进 grok 表 / claude env 的东西）。
        for preset in table {
            for base in [preset.anthropic_base, preset.chat_base].into_iter().flatten() {
                assert!(
                    base.starts_with("https://"),
                    "preset `{}` 的 base 不是 https: {base}",
                    preset.id
                );
            }
        }
    }

    /// xai 的 anthropic 端点是根形态（2026-10-16 以 magpie presets.go 实测对照
    /// 修正）：/v1 形态会拼出 /v1/v1/messages——模型列表能答、每条消息 404。
    #[test]
    fn xai_anthropic_is_root_form() {
        let xai = find_preset("xai").unwrap();
        assert_eq!(xai.anthropic_base, Some("https://api.x.ai"));
        assert_eq!(xai.chat_base, Some("https://api.x.ai/v1"));
    }

    // ─── 绑定生命周期 ───────────────────────────────────────────────────────

    #[test]
    fn binding_lifecycle_roundtrips_and_validates() {
        let dir = temp_dir("bindings");
        let bindings = dir.join("bridge-bindings.json");
        let credentials = dir.join("credentials.json");
        // 假 opencode home（本测试没有 auth.json fixture → 空扫描，足矣）。
        let scan = crate::credentials::scan_opencode_at(&dir);
        crate::credentials::add_key_plan_at(&credentials, "opencode-go", "OpenCode Go", "sk-bind-9f2c11")
            .unwrap();

        // 绑定 → 文件是 camelCase schema。
        let selection = BridgeSelection {
            entry_id: "opencode-go".to_string(),
            model_id: "deepseek-v4-pro".to_string(),
        };
        bind_session_at(
            &bindings,
            &credentials,
            &dir.join("config.toml"),
            &scan,
            "sess-1",
            Some("claude-code"),
            Some(selection.clone()),
        )
        .unwrap();
        let raw = fs::read_to_string(&bindings).unwrap();
        assert!(
            raw.contains("\"entryId\": \"opencode-go\"")
                && raw.contains("\"modelId\": \"deepseek-v4-pro\""),
            "camelCase schema 缺失: {raw}"
        );
        assert!(raw.contains("\"version\": 1"));
        assert_eq!(binding_for_at(&bindings, "sess-1").unwrap().as_ref(), Some(&selection));

        // 绑定记了 touch_last_used。
        let store = crate::credentials::load_store_at(&credentials).unwrap();
        assert!(
            store.entries.iter().any(|entry| entry.last_used_at.is_some()),
            "绑定应记 touch_last_used"
        );

        // 重绑覆盖，不会累积。
        let selection2 = BridgeSelection {
            entry_id: "opencode-go".to_string(),
            model_id: "deepseek-v4.1-flash".to_string(),
        };
        bind_session_at(
            &bindings,
            &credentials,
            &dir.join("config.toml"),
            &scan,
            "sess-1",
            Some("claude-code"),
            Some(selection2.clone()),
        )
        .unwrap();
        assert_eq!(binding_for_at(&bindings, "sess-1").unwrap().as_ref(), Some(&selection2));

        // 校验：未知 entry / 非 key-plan / 空 model 拒绝且文件不动。
        let before = fs::read_to_string(&bindings).unwrap();
        assert!(bind_session_at(
            &bindings,
            &credentials,
            &dir.join("config.toml"),
            &scan,
            "sess-2",
            Some("claude-code"),
            Some(BridgeSelection {
                entry_id: "nope".to_string(),
                model_id: "x".to_string(),
            })
        )
        .is_err());
        let subscription_store = serde_json::json!({
            "version": 1,
            "entries": [{
                "id": "claude/login", "kind": "subscription", "label": "Claude Code 登录",
                "source": "scan:claude", "createdAt": "0"
            }]
        });
        fs::write(
            dir.join("credentials-sub.json"),
            serde_json::to_string_pretty(&subscription_store).unwrap(),
        )
        .unwrap();
        assert!(bind_session_at(
            &bindings,
            &dir.join("credentials-sub.json"),
            &dir.join("config.toml"),
            &scan,
            "sess-2",
            Some("claude-code"),
            Some(BridgeSelection {
                entry_id: "claude/login".to_string(),
                model_id: "x".to_string(),
            })
        )
        .is_err());
        assert!(bind_session_at(
            &bindings,
            &credentials,
            &dir.join("config.toml"),
            &scan,
            "sess-2",
            Some("claude-code"),
            Some(BridgeSelection {
                entry_id: "opencode-go".to_string(),
                model_id: "  ".to_string(),
            })
        )
        .is_err());
        assert_eq!(fs::read_to_string(&bindings).unwrap(), before, "校验失败不能动文件");

        // 解绑 → 绑定清空；再解绑（原本没有）也 OK。
        bind_session_at(
            &bindings,
            &credentials,
            &dir.join("config.toml"),
            &scan,
            "sess-1",
            Some("claude-code"),
            None,
        )
        .unwrap();
        assert_eq!(binding_for_at(&bindings, "sess-1").unwrap(), None);
        bind_session_at(
            &bindings,
            &credentials,
            &dir.join("config.toml"),
            &scan,
            "sess-1",
            Some("claude-code"),
            None,
        )
        .unwrap();

        // 损坏 → 留档重建，原始字节一份不丢。
        let corrupt = r#"{"version":1,"bindings":{"sess-x":{"entryId":"#;
        fs::write(&bindings, corrupt).unwrap();
        let file = load_bindings_at(&bindings).unwrap();
        assert!(file.bindings.is_empty());
        let archives: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("bridge-bindings.corrupt-"))
            .collect();
        assert_eq!(archives.len(), 1, "应恰好留档一份: {archives:?}");
        assert_eq!(
            fs::read_to_string(dir.join(archives.into_iter().next().unwrap())).unwrap(),
            corrupt,
            "留档必须逐字节保留"
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// grok 解绑语义（F）：绑定置 None 且原本是 grok 绑定 → 移除 owned 表；
    /// claude 解绑不动 grok 文件。
    #[test]
    fn unbind_removes_grok_tables_only_for_grok_sessions() {
        let dir = temp_dir("unbind");
        let bindings = dir.join("bridge-bindings.json");
        let credentials = dir.join("credentials.json");
        let grok_config = dir.join("config.toml");
        let scan = crate::credentials::scan_opencode_at(&dir);
        crate::credentials::add_key_plan_at(&credentials, "deepseek", "DeepSeek", "sk-unbind-aabb99")
            .unwrap();
        let selection = BridgeSelection {
            entry_id: "deepseek".to_string(),
            model_id: "deepseek-flash".to_string(),
        };

        // grok 会话：绑定 → 表由 spawn 时写入（这里模拟一次 spawn 的产物）。
        bind_session_at(
            &bindings,
            &credentials,
            &grok_config,
            &scan,
            "grok-sess",
            Some("grok-build"),
            Some(selection),
        )
        .unwrap();
        let preset = find_preset("deepseek").unwrap();
        ensure_grok_model_table_at(&grok_config, "deepseek-flash", preset, GROK_KEY_ENV).unwrap();
        assert!(grok_config.is_file());
        // 解绑 → owned 表被移除（否则 grok 列表里残留死条目）。
        bind_session_at(
            &bindings,
            &credentials,
            &grok_config,
            &scan,
            "grok-sess",
            Some("grok-build"),
            None,
        )
        .unwrap();
        assert!(
            !fs::read_to_string(&grok_config).unwrap().contains("marionette/"),
            "grok 解绑应移除 owned 表"
        );

        // claude 会话解绑：不动 grok 文件（env 注入进程级，无需还原）。
        bind_session_at(
            &bindings,
            &credentials,
            &grok_config,
            &scan,
            "claude-sess",
            Some("claude-code"),
            Some(BridgeSelection {
                entry_id: "deepseek".to_string(),
                model_id: "deepseek-flash".to_string(),
            }),
        )
        .unwrap();
        ensure_grok_model_table_at(&grok_config, "deepseek-flash", preset, GROK_KEY_ENV).unwrap();
        bind_session_at(
            &bindings,
            &credentials,
            &grok_config,
            &scan,
            "claude-sess",
            Some("claude-code"),
            None,
        )
        .unwrap();
        assert!(
            fs::read_to_string(&grok_config).unwrap().contains("marionette/"),
            "claude 解绑不该动 grok 的 owned 表"
        );

        fs::remove_dir_all(&dir).ok();
    }

    // ─── grok owned 表写入器三件套 ───────────────────────────────────────────

    const USER_CONFIG: &str = "# grok fixture — user's own file\n\
[cli]\n\
auto_update = true\n\
\n\
[models]\n\
default = \"grok-4.6\"\n\
\n\
[model.home-qwen]\n\
model = \"qwen3.8-27b\"\n\
base_url = \"http://100.94.137.45:8082/v1\"\n\
name = \"Qwen3.8 (Tailscale)\"\n\
api_key = \"user-own-key\"\n\
\n\
[model.\"zhipu-glm-4.7\"]\n\
model = \"glm-4.7\"\n\
base_url = \"https://open.bigmodel.cn/api/paas/v4\"\n\
api_key = \"user-own-key\"\n\
api_backend = \"chat_completions\"\n\
\n\
[marketplace]\n\
official_marketplace_auto_installed = true\n";

    #[test]
    fn grok_writer_creates_table_and_preserves_user_content() {
        let dir = temp_dir("grok_write");
        let config = dir.join("config.toml");
        fs::write(&config, USER_CONFIG).unwrap();

        let preset = find_preset("deepseek").unwrap();
        assert!(ensure_grok_model_table_at(&config, "deepseek-flash", preset, GROK_KEY_ENV).unwrap());

        // 期望的 base：方言规整器的环回地址（剥 model_id 后转发到 chat 端点）。
        let expected_base = crate::grok_dialect::channel_base_url("deepseek").unwrap();
        assert!(expected_base.starts_with("http://127.0.0.1:"));
        assert!(expected_base.contains("/p/deepseek"));

        let text = fs::read_to_string(&config).unwrap();
        // owned 表内容逐字段正确。
        assert!(text.contains(GROK_OWNED_MARKER));
        assert!(text.contains("[model.\"marionette/deepseek-flash\"]"));
        assert!(text.contains("model = \"deepseek-flash\""));
        assert!(text.contains("name = \"DeepSeek (via Marionette)\""));
        assert!(text.contains(&format!("base_url = \"{expected_base}\"")));
        assert!(text.contains("api_backend = \"chat_completions\""));
        assert!(text.contains("api_key = \"$MARIONETTE_PROVIDER_KEY\""));
        // 用户内容原样保留。
        assert!(text.contains("[model.home-qwen]"));
        assert!(text.contains("user-own-key"));
        assert!(text.contains("[marketplace]"));
        assert!(text.contains("# grok fixture — user's own file"));
        // 绝不新增 [models] default（S3：设 default 会让 Grok 退出自身登录）。
        assert!(text.contains("default = \"grok-4.6\""));
        assert_eq!(text.matches("default =").count(), 1);
        // 产物是合法 TOML，解析后我们的表字段正确。
        let value: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            value["model"]["marionette/deepseek-flash"]["base_url"],
            toml::Value::from(expected_base)
        );
        assert_eq!(
            value["model"]["marionette/deepseek-flash"]["api_key"],
            toml::Value::from("$MARIONETTE_PROVIDER_KEY")
        );
        assert_eq!(value["model"]["home-qwen"]["api_key"], toml::Value::from("user-own-key"));

        fs::remove_dir_all(&dir).ok();
    }

    /// 文件不存在 = 空文档：只写我们的表，不带任何用户内容。
    #[test]
    fn grok_writer_treats_missing_config_as_empty_document() {
        let dir = temp_dir("grok_missing");
        let config = dir.join("config.toml");
        let preset = find_preset("opencode-go").unwrap();
        assert!(ensure_grok_model_table_at(&config, "grok-4.7", preset, GROK_KEY_ENV).unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.starts_with(GROK_OWNED_MARKER));
        let expected_base = crate::grok_dialect::channel_base_url("opencode-go").unwrap();
        assert!(text.contains(&format!("base_url = \"{expected_base}\"")));
        toml::from_str::<toml::Value>(&text).unwrap();
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grok_writer_is_idempotent() {
        let dir = temp_dir("grok_idempotent");
        let config = dir.join("config.toml");
        fs::write(&config, USER_CONFIG).unwrap();
        let preset = find_preset("deepseek").unwrap();
        assert!(ensure_grok_model_table_at(&config, "deepseek-flash", preset, GROK_KEY_ENV).unwrap());
        let after_first = fs::read_to_string(&config).unwrap();
        // 幂等：已存在且一致 → 零写入（返回 false，文件字节不动）。
        assert!(!ensure_grok_model_table_at(&config, "deepseek-flash", preset, GROK_KEY_ENV).unwrap());
        assert_eq!(fs::read_to_string(&config).unwrap(), after_first);
        fs::remove_dir_all(&dir).ok();
    }

    /// 内容漂移（端点/键名变了）→ 原位替换，同名的表仍只有一张；换模型 →
    /// 追加第二张表，两张共存（多会话各绑各的模型）。
    #[test]
    fn grok_writer_replaces_stale_block_in_place() {
        let dir = temp_dir("grok_replace");
        let config = dir.join("config.toml");
        fs::write(&config, USER_CONFIG).unwrap();
        let deepseek = find_preset("deepseek").unwrap();
        ensure_grok_model_table_at(&config, "deepseek-flash", deepseek, GROK_KEY_ENV).unwrap();
        // 手动把 base_url 改成旧值（模拟端点/端口漂移后的刷新）。
        let current_base = crate::grok_dialect::channel_base_url("deepseek").unwrap();
        let stale = fs::read_to_string(&config)
            .unwrap()
            .replace(&current_base, "https://old.example.invalid/v1");
        fs::write(&config, stale).unwrap();
        assert!(ensure_grok_model_table_at(&config, "deepseek-flash", deepseek, GROK_KEY_ENV).unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert_eq!(
            text.matches("[model.\"marionette/deepseek-flash\"]").count(),
            1,
            "同名的表只能有一张"
        );
        assert!(text.contains(&current_base), "刷新后应回到规整器地址");
        assert!(!text.contains("old.example.invalid"));
        assert!(text.contains("user-own-key"), "替换不得伤及用户表");
        toml::from_str::<toml::Value>(&text).unwrap();

        ensure_grok_model_table_at(&config, "deepseek-v4-pro", deepseek, GROK_KEY_ENV).unwrap();
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("[model.\"marionette/deepseek-flash\"]"));
        assert!(text.contains("[model.\"marionette/deepseek-v4-pro\"]"));
        fs::remove_dir_all(&dir).ok();
    }

    /// S3 实测：grok 会并发改写自己的 config.toml（追加段）。写前重读 + 块级
    /// 替换 ⇒ 外部新增内容一条不丢。
    #[test]
    fn grok_writer_tolerates_concurrent_external_edits() {
        let dir = temp_dir("grok_concurrent");
        let config = dir.join("config.toml");
        fs::write(&config, USER_CONFIG).unwrap();
        let preset = find_preset("deepseek").unwrap();
        ensure_grok_model_table_at(&config, "deepseek-flash", preset, GROK_KEY_ENV).unwrap();

        // grok 自写：追加一段（grok 1.x 实测追加 [marketplace] 类段）。
        let mut concurrent = fs::read_to_string(&config).unwrap();
        concurrent.push_str("\n[privacy]\nprivacy_banner_acked = \"2026-10-07T01:21:40Z\"\n");
        fs::write(&config, concurrent).unwrap();

        // 再 ensure（幂等路径）：不丢新增、不重写文件。
        ensure_grok_model_table_at(&config, "deepseek-flash", preset, GROK_KEY_ENV).unwrap();
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("privacy_banner_acked"), "并发新增的段不能丢");
        assert!(text.contains("marionette/deepseek-flash"));
        toml::from_str::<toml::Value>(&text).unwrap();

        // 内容漂移（key env 变了）+ 并发新增共存：替换我们的块，保留新增。
        let drifted = text.replace("$MARIONETTE_PROVIDER_KEY", "$OLD_ENV");
        fs::write(&config, drifted).unwrap();
        assert!(ensure_grok_model_table_at(&config, "deepseek-flash", preset, GROK_KEY_ENV).unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("$MARIONETTE_PROVIDER_KEY"));
        assert!(!text.contains("$OLD_ENV"));
        assert!(text.contains("privacy_banner_acked"), "替换路径也不能丢并发新增");
        fs::remove_dir_all(&dir).ok();
    }

    /// 解析失败不写半截：原文过不了 TOML 校验 → 报错且文件一字不动。
    #[test]
    fn grok_writer_refuses_to_touch_unparseable_config() {
        let dir = temp_dir("grok_corrupt");
        let config = dir.join("config.toml");
        let broken = "[model.\"x\"\nbroken = = toml\n";
        fs::write(&config, broken).unwrap();
        let preset = find_preset("deepseek").unwrap();
        let error =
            ensure_grok_model_table_at(&config, "deepseek-flash", preset, GROK_KEY_ENV).unwrap_err();
        assert!(error.contains("解析失败"), "unexpected: {error}");
        assert_eq!(fs::read_to_string(&config).unwrap(), broken, "失败路径不能动文件");
        // remove 同样拒绝。
        let error = remove_grok_model_tables_at(&config).unwrap_err();
        assert!(error.contains("解析失败"), "unexpected: {error}");
        assert_eq!(fs::read_to_string(&config).unwrap(), broken);
        fs::remove_dir_all(&dir).ok();
    }

    /// remove：删所有 marionette/ 表 + 孤立标记行，用户表逐字节保留；幂等。
    #[test]
    fn grok_remover_deletes_owned_tables_and_orphan_markers_only() {
        let dir = temp_dir("grok_remove");
        let config = dir.join("config.toml");
        let deepseek = find_preset("deepseek").unwrap();
        ensure_grok_model_table_at(&config, "deepseek-flash", deepseek, GROK_KEY_ENV).unwrap();
        ensure_grok_model_table_at(&config, "deepseek-v4-pro", deepseek, GROK_KEY_ENV).unwrap();
        // 人为制造孤立标记行（表被外部删了一半）。
        let mut text = fs::read_to_string(&config).unwrap();
        text.push_str(&format!("\n{GROK_OWNED_MARKER}\n"));
        fs::write(&config, text).unwrap();

        assert!(remove_grok_model_tables_at(&config).unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert!(!text.contains("marionette/"), "所有 marionette/ 表都应删除:\n{text}");
        assert!(!text.contains(GROK_OWNED_MARKER), "孤立标记行也应删除:\n{text}");
        assert!(text.trim().is_empty(), "这个文件里只有我们的东西，删完应为空:\n{text}");
        // 幂等：再删零写入。
        assert!(!remove_grok_model_tables_at(&config).unwrap());

        // 与用户内容混排：只删我们的。
        fs::write(&config, USER_CONFIG).unwrap();
        ensure_grok_model_table_at(&config, "deepseek-flash", deepseek, GROK_KEY_ENV).unwrap();
        assert!(remove_grok_model_tables_at(&config).unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert!(!text.contains("marionette/"));
        assert!(text.contains("user-own-key"));
        assert!(text.contains("[marketplace]"));
        assert!(text.contains("default = \"grok-4.6\""));
        toml::from_str::<toml::Value>(&text).unwrap();
        fs::remove_dir_all(&dir).ok();
    }

    /// 多行字符串里伪装成表头的行不能骗过切块器。
    #[test]
    fn grok_writer_ignores_fake_headers_inside_multiline_strings() {
        let dir = temp_dir("grok_multiline");
        let config = dir.join("config.toml");
        fs::write(
            &config,
            concat!(
                "[model.user-ml]\n",
                "model = \"m\"\n",
                "description = \"\"\"\n",
                "[model.\"marionette/fake\"]\n",
                "api_key = \"not-real\"\n",
                "\"\"\"\n",
            ),
        )
        .unwrap();
        let preset = find_preset("deepseek").unwrap();
        ensure_grok_model_table_at(&config, "deepseek-flash", preset, GROK_KEY_ENV).unwrap();
        let text = fs::read_to_string(&config).unwrap();
        // 伪表头仍在用户的字符串里（原文搬运），真表只被认出一张。
        assert_eq!(
            text.matches("marionette/").count(),
            2,
            "字符串里一个 + 我们的表一个:\n{text}"
        );
        let value: toml::Value = toml::from_str(&text).unwrap();
        assert!(value["model"]["user-ml"]["description"]
            .as_str()
            .unwrap()
            .contains("[model.\"marionette/fake\"]"));
        // remove 只删真表（伪表头在字符串值里，不是 model 下的表）。
        assert!(remove_grok_model_tables_at(&config).unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("marionette/fake"), "用户字符串里的伪表头必须保留");
        assert!(!text.contains("[model.\"marionette/deepseek-flash\"]"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grok_rewrites_coding_plan_key_off_payg_endpoint() {
        let dir = temp_dir("grok_zhipu_coding");
        let config = dir.join("config.toml");
        fs::write(&config, USER_CONFIG).unwrap();
        let plan = "sk-zhipu-plan";
        let text = fs::read_to_string(&config).unwrap().replace("user-own-key", plan);
        fs::write(&config, &text).unwrap();

        assert!(rewrite_grok_zhipu_coding_plan_bases_at(&config, &[plan.to_string()]).unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("https://open.bigmodel.cn/api/coding/paas/v4"));
        assert!(
            !text.contains("https://open.bigmodel.cn/api/paas/v4\""),
            "pay-as-you-go chat URL must be gone:\n{text}"
        );
        assert!(text.contains("http://100.94.137.45:8082/v1"), "unrelated tables stay");
        assert!(text.contains("default = \"grok-4.6\""));
        // 幂等
        assert!(!rewrite_grok_zhipu_coding_plan_bases_at(&config, &[plan.to_string()]).unwrap());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grok_leaves_payg_key_on_payg_endpoint() {
        let dir = temp_dir("grok_zhipu_payg");
        let config = dir.join("config.toml");
        fs::write(&config, USER_CONFIG).unwrap();
        assert!(!rewrite_grok_zhipu_coding_plan_bases_at(
            &config,
            &["sk-other-plan".to_string()]
        )
        .unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("https://open.bigmodel.cn/api/paas/v4"));
        assert!(!text.contains("/api/coding/paas/v4"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grok_skips_env_expanded_owned_tables() {
        let dir = temp_dir("grok_zhipu_env");
        let config = dir.join("config.toml");
        fs::write(
            &config,
            "[model.\"marionette/glm-5.3\"]\n\
             model = \"glm-5.3\"\n\
             base_url = \"https://open.bigmodel.cn/api/paas/v4\"\n\
             api_key = \"$MARIONETTE_PROVIDER_KEY\"\n",
        )
        .unwrap();
        assert!(!rewrite_grok_zhipu_coding_plan_bases_at(
            &config,
            &["$MARIONETTE_PROVIDER_KEY".to_string()]
        )
        .unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("/api/paas/v4"));
        fs::remove_dir_all(&dir).ok();
    }

    // ─── 去重规则 ───────────────────────────────────────────────────────────

    fn fixture_store(dir: &Path) -> PathBuf {
        let path = dir.join("credentials.json");
        crate::credentials::add_key_plan_at(&path, "opencode-go", "OpenCode Go", "sk-dedup-8811aa")
            .unwrap();
        // 直接构造存储文件：key-plan / reference(宿主 opencode) / subscription /
        // key-plan(默认模型为空的 glm)。
        let store = serde_json::json!({
            "version": 1,
            "entries": [
                { "id": "opencode-go", "kind": "key-plan", "label": "OpenCode Go",
                  "preset": "opencode-go", "secret": path_ok_secret(), "source": "manual", "createdAt": "1" },
                { "id": "opencode/deepseek", "kind": "reference", "label": "deepseek",
                  "ref": "~/.local/share/opencode/auth.json#deepseek",
                  "hostAgent": "opencode", "source": "scan:opencode", "createdAt": "1" },
                { "id": "grok/login", "kind": "subscription", "label": "Grok 订阅",
                  "hostAgent": "grok", "source": "scan:grok", "createdAt": "1" },
                { "id": "glm", "kind": "key-plan", "label": "智谱 GLM",
                  "preset": "glm", "source": "manual", "createdAt": "1" }
            ]
        });
        fs::write(&path, serde_json::to_string_pretty(&store).unwrap()).unwrap();
        path
    }

    /// 一个与真实密文形态一致的假 secret（前缀对、内容非空；用不上解密 —
    /// 列表模型不走 maskedKey）。跨平台测试里 peek 失败也无妨。
    fn path_ok_secret() -> String {
        if cfg!(windows) { "dpapi:QUJDRA==" } else { "plain:QUJDRA==" }.to_string()
    }

    #[test]
    fn dedup_rules_follow_the_product_table() {
        let dir = temp_dir("dedup");
        let credentials = fixture_store(&dir);
        // 假 home 下没有 opencode auth.json → 扫描恒空：这里只验 store 侧的
        // 去重；reference 并入由 list_models_merges_scanned_references 验。
        let scan = crate::credentials::scan_opencode_at(&dir);
        // note/目录断言依赖「引擎未 Running」。absent 守卫与 test_running
        // 共享串行锁：拿不到就等，杜绝并行测试的 Running 缝互踩。
        let _absent = crate::engine::test_absent_guard();

        // claude / grok：看到两个 key-plan 的模型。glm 的静态兜底是套餐主打
        // 两模型（2026-10-17 端点实测；动态 /models 拉取为主、静态兜底）。
        for agent in ["claude-code", "grok-build", "grok"] {
            let result = list_models_at(&credentials, &scan, agent);
            // 测试态引擎未 Running → 引擎指引 note（2026-10-17 起：明说订阅族
            // 缺席的原因，而不是静默少一组）。
            assert!(
                result.note.as_deref().is_some_and(|n| n.contains("模型共享未运行")),
                "agent {agent}: note = {:?}",
                result.note
            );
            let models: Vec<&str> = result.models.iter().map(|m| m.model_id.as_str()).collect();
            assert_eq!(
                models,
                vec![
                    "deepseek-v4.1-flash",
                    "deepseek-v4-pro",
                    "grok-4.7",
                    "kimi-k3",
                    "longcat-2.0",
                    "glm-5.3",
                    "glm-5.3-flash"
                ]
            );
            assert!(result
                .models
                .iter()
                .all(|m| m.entry_id == "opencode-go" || m.entry_id == "glm"));
            // reference / subscription 一概不出现。
            assert!(result.models.iter().all(|m| m.entry_id != "opencode/deepseek"));
            assert!(result.models.iter().all(|m| m.entry_id != "grok/login"));
        }

        // codex：引擎没跑 → 空 + 指引文案（别名同）；opencode：空 + 指定文案。
        for codex_alias in ["codex", "codex-acp"] {
            let codex = list_models_at(&credentials, &scan, codex_alias);
            assert!(codex.models.is_empty());
            assert_eq!(
                codex.note.as_deref(),
                Some("模型共享需要翻译引擎 — 请先在设置的凭据页启用（引擎运行中后重开此菜单）")
            );
        }
        let opencode = list_models_at(&credentials, &scan, "opencode");
        assert!(opencode.models.is_empty());
        assert_eq!(
            opencode.note.as_deref(),
            Some("模型共享未运行 — Grok/GPT 等订阅模型暂不可见；点左下角模型共享按钮启动（或稍候自动唤醒）后重开此菜单")
        );

        // dsh（2026-10-20）：与 opencode 同一引擎源通道（注入面在
        // dsh_bridge 的 llm-pi-ai 托管路由，不走 SpawnInjection）。
        assert_eq!(classify_agent("deepseek"), BridgeAgent::Deepseek);
        assert_eq!(classify_agent("deepseek-acp"), BridgeAgent::Deepseek);
        let dsh = list_models_at(&credentials, &scan, "deepseek");
        assert!(dsh.models.is_empty());
        assert_eq!(dsh.note.as_deref(), opencode.note.as_deref());

        // pi（2026-10-20）：同一引擎源通道（注入面 = models.json 托管 provider）。
        assert_eq!(classify_agent("pi"), BridgeAgent::Pi);
        assert_eq!(classify_agent("pi-acp"), BridgeAgent::Pi);
        let pi = list_models_at(&credentials, &scan, "pi");
        assert!(pi.models.is_empty());
        assert_eq!(pi.note.as_deref(), opencode.note.as_deref());

        // zcode（2026-10-23）：同一引擎源通道（注入面 = provider_config.json
        // 托管 provider，见 refresh_zcode_marionette_route）。
        assert_eq!(classify_agent("zcode"), BridgeAgent::Zcode);
        assert_eq!(classify_agent("zcode-acp"), BridgeAgent::Zcode);
        let zcode = list_models_at(&credentials, &scan, "zcode");
        assert!(zcode.models.is_empty());
        assert_eq!(zcode.note.as_deref(), opencode.note.as_deref());

        // 其它 agent：没有注入通道，空 + 说明。
        let other = list_models_at(&credentials, &scan, "gemini");
        assert!(other.models.is_empty());
        assert!(other.note.unwrap().contains("还没接"));

        // 规则本体：reference 对宿主不可见、对别人可见；subscription 一律不可见。
        let host = crate::credentials::CredentialEntry {
            custom_provider: None,
            id: "opencode/deepseek".into(),
            kind: CredentialKind::Reference,
            label: "deepseek".into(),
            preset: None,
            secret: None,
            ref_path: None,
            host_agent: Some("opencode".into()),
            source: "scan:opencode".into(),
            created_at: "0".into(),
            last_used_at: None,
        };
        assert!(!entry_visible_to(&host, BridgeAgent::Opencode), "宿主自己不可见");
        assert!(entry_visible_to(&host, BridgeAgent::ClaudeCode), "别的 agent 可见（M2 用）");
        assert!(
            !entry_visible_to(
                &crate::credentials::CredentialEntry {
                    id: "grok/login".into(),
                    kind: CredentialKind::Subscription,
                    label: "Grok 订阅".into(),
                    host_agent: Some("grok".into()),
                    ..host.clone()
                },
                BridgeAgent::ClaudeCode
            ),
            "subscription M2 前不可见"
        );

        // 没有 key-plan 凭证 → 引导文案。
        let empty = dir.join("credentials-empty.json");
        let result = list_models_at(&empty, &scan, "claude-code");
        assert!(result.models.is_empty());
        assert!(result.note.unwrap().contains("添加"));

        // 端点驱动过滤：缺 anthropic_base 的 preset 对 claude 不可见（合成
        // preset 验规则本体，不动产品表）。
        let chat_only = ProviderPreset {
            id: "chat-only",
            label: "ChatOnly",
            anthropic_base: None,
            chat_base: Some("https://chat-only.example/v1"),
            models: &["m1"],
            last_verified: "test",
        };
        assert!(!preset_serves_agent(&chat_only, BridgeAgent::ClaudeCode));
        assert!(preset_serves_agent(&chat_only, BridgeAgent::GrokBuild));
        assert!(injection_for(&chat_only, BridgeAgent::ClaudeCode, "m1").is_none());
        assert!(injection_for(&chat_only, BridgeAgent::GrokBuild, "m1").is_some());

        fs::remove_dir_all(&dir).ok();
    }

    // ─── spawn 注入解析 + providers/set 框架 ────────────────────────────────

    #[test]
    fn spawn_injection_resolves_per_agent() {
        let dir = temp_dir("inject");
        let credentials = dir.join("credentials.json");
        let scan = crate::credentials::scan_opencode_at(&dir);
        crate::credentials::add_key_plan_at(&credentials, "opencode-go", "OpenCode Go", "sk-inject-4f9a2c")
            .unwrap();
        crate::credentials::add_key_plan_at(&credentials, "deepseek", "DeepSeek", "sk-inject-ds-9933")
            .unwrap();

        // claude → env 形态（base 走规整器：opencode-go 的 anthropic 端点要求
        // 会话头，claude 不自带，由规整器按 preset 叠加）。
        assert_eq!(
            injection_for_agent_at(&credentials, &scan, "claude-code", "opencode-go", "deepseek-v4-pro")
                .unwrap(),
            Some(SpawnInjection::ClaudeEnv {
                base_url: crate::grok_dialect::channel_base_url("opencode-go").unwrap(),
                model: "deepseek-v4-pro".into(),
            })
        );
        // 别名 claude 同样命中。
        assert!(matches!(
            injection_for_agent_at(&credentials, &scan, "claude", "deepseek", "deepseek-flash").unwrap(),
            Some(SpawnInjection::ClaudeEnv { .. })
        ));

        // grok → owned 表形态（chat base）。
        assert_eq!(
            injection_for_agent_at(&credentials, &scan, "grok-build", "deepseek", "deepseek-flash").unwrap(),
            Some(SpawnInjection::GrokTable {
                model_id: "deepseek-flash".into(),
                preset_id: "deepseek".into(),
            })
        );

        // 非 M1 agent → None；引用死条目 → Err（带线索）。
        assert_eq!(injection_for_agent_at(&credentials, &scan, "codex", "deepseek", "x").unwrap(), None);
        assert_eq!(injection_for_agent_at(&credentials, &scan, "deepseek", "deepseek", "x").unwrap(), None);
        let error = injection_for_agent_at(&credentials, &scan, "claude-code", "ghost", "x").unwrap_err();
        assert!(error.contains("不存在"), "unexpected: {error}");

        fs::remove_dir_all(&dir).ok();
    }

    // ─── M2 翻译引擎伪来源（crate::engine 的接线规则）───────────────────────

    /// 可见性：引擎不在 Running → 完全不出现（不自动下载）；Running → 追加
    /// engine 源，模型 id 原样（`provider/model` 形态）、完整目录不截断。
    #[test]
    fn engine_source_appears_only_when_running() {
        let dir = temp_dir("engvis");
        let credentials = dir.join("credentials.json");
        let scan = crate::credentials::scan_opencode_at(&dir);

        // 不 Running → 与 M1 结果一致，engine 源不出现。absent 守卫确保断言
        // 窗口内没有并行测试的 Running 缝（TestHandle 与守卫共享串行锁）。
        let absent = crate::engine::test_absent_guard();
        let result = list_models_at(&credentials, &scan, "claude-code");
        assert!(
            !result
                .sources
                .iter()
                .any(|s| s.entry_id == crate::engine::ENGINE_ENTRY_ID),
            "引擎未就绪时不能出现 engine 源"
        );
        drop(absent);

        // Running → engine 源出现，模型挂在 engine entry 上。
        let _engine = crate::engine::test_running(vec![
            "grok/grok-4.7".to_string(),
            "claude/claude-opus-4-6".to_string(),
        ]);
        // opencode（2026-10-17 起）与 claude/grok/codex 同款：引擎 Running 时
        // Composer 跨用段同样列出引擎源（key 型来源仍不列——宿主去重规则）。
        let oc = list_models_at(&credentials, &scan, "opencode");
        assert!(
            oc.sources
                .iter()
                .any(|s| s.entry_id == crate::engine::ENGINE_ENTRY_ID),
            "opencode 在引擎 Running 时应列出 engine 源"
        );
        assert_eq!(
            oc.models
                .iter()
                .filter(|m| m.entry_id == crate::engine::ENGINE_ENTRY_ID)
                .count(),
            2
        );
        assert!(oc.note.is_none());
        let result = list_models_at(&credentials, &scan, "claude-code");
        let engine_source = result
            .sources
            .iter()
            .find(|s| s.entry_id == crate::engine::ENGINE_ENTRY_ID)
            .expect("引擎 Running 时 engine 源应出现");
        assert_eq!(engine_source.preset, crate::engine::ENGINE_PRESET_ID);
        assert_eq!(engine_source.label, "翻译引擎（订阅）");
        assert!(!engine_source.from_scan);
        let ids: Vec<&str> = result
            .models
            .iter()
            .filter(|m| m.entry_id == crate::engine::ENGINE_ENTRY_ID)
            .map(|m| m.model_id.as_str())
            .collect();
        assert_eq!(ids, vec!["grok/grok-4.7", "claude/claude-opus-4-6"]);
        assert!(result
            .models
            .iter()
            .filter(|m| m.entry_id == crate::engine::ENGINE_ENTRY_ID)
            .all(|m| m.label == m.model_id));

        // 完整目录由模型库管理显示。（先 drop 前一个句柄——TestHandle 持有
        // 引擎测试的串行锁，两个句柄同时活着会自死锁。）
        drop(_engine);
        let many: Vec<String> = (0..70).map(|i| format!("p/model-{i}")).collect();
        let _engine_many = crate::engine::test_running(many);
        let result = list_models_at(&credentials, &scan, "grok-build");
        assert_eq!(
            result
                .models
                .iter()
                .filter(|m| m.entry_id == crate::engine::ENGINE_ENTRY_ID)
                .count(),
            70
        );
        assert!(!result.note.as_deref().unwrap_or_default().contains("截断"));

        // 大小来源都完整保留；`group/` 路由组不进菜单。
        drop(_engine_many);
        let mut mixed: Vec<String> = (0..70).map(|i| format!("p/model-{i}")).collect();
        mixed.push("grok/grok-4.5".to_string());
        mixed.push("grok/grok-4.6".to_string());
        mixed.push("group/auto-grok-4-7".to_string());
        let _engine_mixed = crate::engine::test_running(mixed);
        let result = list_models_at(&credentials, &scan, "grok-build");
        let engine_models: Vec<_> = result
            .models
            .iter()
            .filter(|m| m.entry_id == crate::engine::ENGINE_ENTRY_ID)
            .collect();
        assert_eq!(engine_models.len(), 72, "尾部 provider 不被别家体量砍穿");
        assert!(engine_models.iter().any(|m| m.model_id == "grok/grok-4.5"));
        assert!(engine_models.iter().any(|m| m.model_id == "grok/grok-4.6"));
        assert!(
            engine_models.iter().all(|m| !m.model_id.starts_with("group/")),
            "路由组不进跨用菜单"
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// engine 绑定：不在 Running 拒（消息指路）；Running 放行且不需要任何凭据
    /// （不进 store、不记 lastUsedAt）；注入两臂的形态；钥匙是字面量令牌。
    #[test]
    fn engine_binding_and_injection_shapes() {
        let dir = temp_dir("engbind");
        let credentials = dir.join("credentials.json");
        let scan = crate::credentials::scan_opencode_at(&dir);
        let bindings = dir.join("bridge-bindings.json");
        let grok_config = dir.join("config.toml");

        // 不 Running：绑定拒绝、文件不动；注入解析走降级路径（Err 带指引）。
        // absent 守卫确保断言窗口内没有并行测试的 Running 缝。
        let absent = crate::engine::test_absent_guard();
        let error = bind_session_at(
            &bindings,
            &credentials,
            &grok_config,
            &scan,
            "s1",
            Some("claude-code"),
            Some(BridgeSelection {
                entry_id: crate::engine::ENGINE_ENTRY_ID.to_string(),
                model_id: "grok/grok-4.7".to_string(),
            }),
        )
        .unwrap_err();
        assert!(error.contains("未在运行"), "unexpected: {error}");
        assert!(binding_for_at(&bindings, "s1").unwrap().is_none());
        let error = injection_for_agent_at(&credentials, &scan, "claude-code", "engine", "x").unwrap_err();
        assert!(error.contains("降级"), "unexpected: {error}");
        drop(absent);

        // Running：绑定成功（不落 store、无 lastUsedAt 可记）。
        let _engine = crate::engine::test_running(Vec::new());
        bind_session_at(
            &bindings,
            &credentials,
            &grok_config,
            &scan,
            "s1",
            Some("claude-code"),
            Some(BridgeSelection {
                entry_id: crate::engine::ENGINE_ENTRY_ID.to_string(),
                model_id: "grok/grok-4.7".to_string(),
            }),
        )
        .unwrap();
        assert_eq!(
            binding_for_at(&bindings, "s1").unwrap().map(|s| s.entry_id),
            Some("engine".to_string())
        );
        let store = crate::credentials::load_store_at(&credentials).unwrap();
        assert!(store.entries.is_empty(), "engine 绑定不产生任何凭据条目");

        // claude 臂：env 形态，base 是引擎根（引擎在根上服务 /v1/messages）。
        assert_eq!(
            injection_for_agent_at(&credentials, &scan, "claude-code", "engine", "grok/grok-4.7")
                .unwrap(),
            Some(SpawnInjection::ClaudeEnv {
                base_url: "http://127.0.0.1:39999".into(),
                model: "grok/grok-4.7".into(),
            })
        );
        // grok 臂：owned 表形态，preset_id 指到 engine 分支。
        assert_eq!(
            injection_for_agent_at(&credentials, &scan, "grok-build", "engine", "grok/grok-4.7")
                .unwrap(),
            Some(SpawnInjection::GrokTable {
                model_id: "grok/grok-4.7".into(),
                preset_id: crate::engine::ENGINE_PRESET_ID.to_string(),
            })
        );
        // 无注入通道的 agent → None（同既有语义）。
        assert_eq!(
            injection_for_agent_at(&credentials, &scan, "deepseek", "engine", "x").unwrap(),
            None
        );
        // 钥匙：字面量令牌（引擎回环任意 token 放行——真实订阅凭证不外流）。
        assert_eq!(
            peek_spawn_key_at(&credentials, &scan, crate::engine::ENGINE_ENTRY_ID).unwrap(),
            crate::engine::ENGINE_TOKEN
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// codex 跨用目录只含引擎源：引擎不在 → note 指引（不自动拉起）；Running
    /// → 仅 engine 源 + 引擎模型（key/reference 端点进不了 codex，§4.4 S2）。
    #[test]
    fn codex_lists_the_engine_source_only() {
        let dir = temp_dir("codexsrc");
        let credentials = dir.join("credentials.json");
        let scan = crate::credentials::scan_opencode_at(&dir);

        // 不 Running：空目录 + 指引 note。absent 守卫杜绝并行 Running 缝。
        let absent = crate::engine::test_absent_guard();
        let result = list_models_at(&credentials, &scan, "codex");
        assert!(result.sources.is_empty() && result.models.is_empty());
        assert!(
            result.note.as_deref().unwrap_or_default().contains("翻译引擎"),
            "unexpected note: {:?}",
            result.note
        );
        drop(absent);

        // Running：engine 源 + 引擎模型，别名 codex-acp 也归同一桶。
        let _engine = crate::engine::test_running(vec![
            "grok/grok-4.7".to_string(),
            "kimi/k2.6".to_string(),
        ]);
        for agent in ["codex", "codex-acp"] {
            let result = list_models_at(&credentials, &scan, agent);
            assert_eq!(result.sources.len(), 1, "codex 只列引擎源");
            assert_eq!(result.sources[0].entry_id, crate::engine::ENGINE_ENTRY_ID);
            let ids: Vec<&str> = result
                .models
                .iter()
                .map(|m| m.model_id.as_str())
                .collect();
            assert_eq!(ids, vec!["grok/grok-4.7", "kimi/k2.6"]);
            assert!(result.note.is_none(), "unexpected note: {:?}", result.note);
        }

        fs::remove_dir_all(&dir).ok();
    }

    /// codex 的 providers/set 路由：引擎绑定 → (引擎 `/v1` 根, 字面量令牌)
    /// —— codex-acp 把 baseUrl 写进 base_url、wire_api 固定 responses，codex
    /// 自己拼 `/responses`；key 型条目仍然解析不出路由。
    #[test]
    fn codex_engine_route_points_at_the_engine_v1_root() {
        let _engine = crate::engine::test_running(Vec::new());
        let (base_url, token) = codex_bridge_route(&BridgeSelection {
            entry_id: crate::engine::ENGINE_ENTRY_ID.to_string(),
            model_id: "grok/grok-4.7".to_string(),
        })
        .expect("engine 绑定必须解析出路由");
        assert_eq!(base_url, "http://127.0.0.1:39999/v1");
        assert_eq!(token, crate::engine::ENGINE_TOKEN);
    }

    /// codex × 引擎别名同步的门控：非 codex / 无绑定 / caps 没报当前模型 →
    /// 不碰引擎；codex + 引擎绑定 + 当前模型 → 按处方建组（CLI 参数由引擎
    /// 测试缝记录）。
    #[test]
    fn sync_codex_engine_alias_gates_on_agent_binding_and_model() {
        let dir = temp_dir("codexsync");
        let credentials = dir.join("credentials.json");
        let scan = crate::credentials::scan_opencode_at(&dir);
        let bindings = dir.join("bridge-bindings.json");
        let grok_config = dir.join("config.toml");
        put_binding_at(
            &bindings,
            "s1",
            Some(&BridgeSelection {
                entry_id: crate::engine::ENGINE_ENTRY_ID.to_string(),
                model_id: "grok/grok-4.7".to_string(),
            }),
        )
        .unwrap();

        let calls: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = calls.clone();
        let _engine = crate::engine::test_running_with_cli(Vec::new(), std::sync::Arc::new(
            move |args| {
                recorder
                    .lock()
                    .unwrap()
                    .push(args.iter().map(|s| s.to_string()).collect());
                Ok(())
            },
        ));

        // 非 codex / 无当前模型 / 未绑定的会话 → 零 CLI。
        sync_codex_engine_alias_at(&bindings, "s1", Some("claude-code"), Some("m"));
        sync_codex_engine_alias_at(&bindings, "s1", Some("codex"), None);
        sync_codex_engine_alias_at(&bindings, "s-other", Some("codex"), Some("m"));
        assert!(calls.lock().unwrap().is_empty(), "门控条件下不能碰引擎");

        // codex + 引擎绑定 → 组处方（id=内建名、pick=引擎模型、manual 路由
        // 由 pick 自带）；幂等重放不重复跑 CLI。
        sync_codex_engine_alias_at(&bindings, "s1", Some("codex"), Some("gpt-5.2-codex"));
        assert_eq!(
            calls.lock().unwrap()[0],
            vec![
                "group".to_string(),
                "add".to_string(),
                "Marionette gpt-5.2-codex".to_string(),
                "models=grok/grok-4.7".to_string(),
                "id=gpt-5.2-codex".to_string(),
                "pick=grok/grok-4.7".to_string(),
            ]
        );
        sync_codex_engine_alias_at(&bindings, "s1", Some("codex"), Some("gpt-5.2-codex"));
        assert_eq!(calls.lock().unwrap().len(), 1, "同会话同模型重放是 no-op");

        // 解绑（None）→ codex 分支清别名（无人引用即删组）。
        bind_session_at(&bindings, &credentials, &grok_config, &scan, "s1", Some("codex"), None)
            .unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls[1],
            vec!["group".to_string(), "rm".to_string(), "gpt-5.2-codex".to_string()]
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// B4-2 指纹漂移：首问落基线；清单不变零写入；变化 → 存档更新且 diff
    /// 精确（added/removed）；空结果不落档。
    #[test]
    fn catalog_drift_persists_and_diffs() {
        let dir = temp_dir("drift");
        let path = dir.join("bridge-model-fingerprints.json");
        let source = BridgeSource {
            entry_id: "engine".to_string(),
            preset: "engine".to_string(),
            label: "翻译引擎（订阅）".to_string(),
            from_scan: false,
        };
        let result_of = |ids: &[&str]| BridgeModelsResult {
            models: ids
                .iter()
                .map(|id| BridgeModel {
                    entry_id: "engine".to_string(),
                    preset: "engine".to_string(),
                    label: id.to_string(),
                    model_id: id.to_string(),
                })
                .collect(),
            sources: vec![source.clone()],
            note: None,
        };

        // 首问：落基线（排序稳定，与传入顺序无关）。
        note_catalog_drift_at(&path, &result_of(&["b-model", "a-model"])).unwrap();
        let baseline = fs::read_to_string(&path).unwrap();
        assert!(baseline.contains("\"a-model\""));
        assert!(
            baseline.find("\"a-model\"").unwrap() < baseline.find("\"b-model\"").unwrap(),
            "清单按排序落档"
        );

        // 不变：内容零变化（原子写会换 inode/时间戳，比较内容本身）。
        note_catalog_drift_at(&path, &result_of(&["a-model", "b-model"])).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), baseline);

        // 变化：新档反映差集（c 加入、a 移除）。
        note_catalog_drift_at(&path, &result_of(&["b-model", "c-model"])).unwrap();
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("\"c-model\"") && !updated.contains("\"a-model\""));

        // 空结果：不创建档案（首问无来源 = 无可对比）。
        let empty_path = dir.join("empty.json");
        note_catalog_drift_at(
            &empty_path,
            &BridgeModelsResult { models: Vec::new(), sources: Vec::new(), note: None },
        )
        .unwrap();
        assert!(!empty_path.exists());

        fs::remove_dir_all(&dir).ok();
    }

    /// 引擎版 grok owned 表：base 指引擎 `/v1`、api_key 是字面量令牌（不走
    /// $VAR）；用户内容保留、绝不设 `[models] default`、幂等、端口漂移原位
    /// 替换；remove 把 `marionette/` 前缀的引擎表一并清掉。
    #[test]
    fn grok_engine_table_uses_engine_base_and_literal_token() {
        let dir = temp_dir("engtable");
        let config = dir.join("config.toml");
        fs::write(&config, USER_CONFIG).unwrap();
        let _engine = crate::engine::test_running(Vec::new());

        assert!(ensure_grok_engine_table_at(&config, "grok/grok-4.7", "http://127.0.0.1:39999/v1").unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("[model.\"marionette/grok/grok-4.7\"]"));
        assert!(text.contains("model = \"grok/grok-4.7\""));
        assert!(text.contains("base_url = \"http://127.0.0.1:39999/v1\""));
        assert!(text.contains(&format!("api_key = \"{}\"", crate::engine::ENGINE_TOKEN)));
        assert!(!text.contains("$MARIONETTE_PROVIDER_KEY"), "引擎表不用 env 引用");
        assert!(text.contains("user-own-key"), "用户内容原样保留");
        assert_eq!(text.matches("default =").count(), 1, "绝不新增 [models] default");
        toml::from_str::<toml::Value>(&text).unwrap();

        // 幂等；端口漂移 → 原位替换（引擎换了端口，表跟着走）。
        assert!(!ensure_grok_engine_table_at(&config, "grok/grok-4.7", "http://127.0.0.1:39999/v1").unwrap());
        assert!(ensure_grok_engine_table_at(&config, "grok/grok-4.7", "http://127.0.0.1:40001/v1").unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("40001"));
        assert!(!text.contains("39999"));

        // remove 清掉引擎表（marionette/ 前缀），用户表保留。
        assert!(remove_grok_model_tables_at(&config).unwrap());
        let text = fs::read_to_string(&config).unwrap();
        assert!(!text.contains("marionette/"));
        assert!(text.contains("user-own-key"));
        toml::from_str::<toml::Value>(&text).unwrap();

        fs::remove_dir_all(&dir).ok();
    }

    // ─── reference 直连（opencode auth.json 现场读）──────────────────────────

    /// 假 opencode home：deepseek / openrouter / google 是 api 条目，anthropic
    /// 是 OAuth 登录态，hybrid 是「type api 但带 refresh」的混合条目（按 OAuth
    /// 优先的规则归登录态）。key 全是假 key，仅测试用。
    fn fixture_opencode_home(dir: &Path) -> PathBuf {
        let home = dir.join("opencode-home");
        let auth = home.join(".local").join("share").join("opencode").join("auth.json");
        fs::create_dir_all(auth.parent().unwrap()).unwrap();
        fs::write(
            &auth,
            serde_json::json!({
                "deepseek": { "type": "api", "key": "sk-fixture-ds-0a1b2c" },
                "openrouter": { "type": "api", "key": "sk-fixture-or-3d4e5f" },
                "google": { "type": "api", "key": "sk-fixture-google-6d7e8f" },
                "anthropic": { "type": "oauth", "refresh": "rt-fixture", "access": "at-fixture" },
                "hybrid": { "type": "api", "key": "sk-fixture-hy-9f0a1b", "refresh": "rt-fixture" }
            })
            .to_string(),
        )
        .unwrap();
        home
    }

    /// 映射表覆盖规格清单的全部键名（含别名），大小写不敏感；未映射 → None。
    /// opencode 原生跨用配置：provider「marionette」指引擎 /v1、api_key 是
    /// 字面量令牌、模型 id 原样（多斜杠）；已有内联配置深合并（别人的
    /// provider 保留），坏 JSON 整体替换。
    #[test]
    fn opencode_engine_config_merges_and_shapes() {
        let content = opencode_engine_config_content(
            3425,
            &[
                "grok/grok-4.7".to_string(),
                "codex/gpt-5.6-luna".to_string(),
                "opencode-go/glm-5.3".to_string(),
            ],
            Some(r#"{"autoupdate":false,"provider":{"mine":{"name":"Keep Me"}}}"#),
        );
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        // 用户自己的键与 provider 原样保留。
        assert_eq!(parsed["autoupdate"], false);
        assert_eq!(parsed["provider"]["mine"]["name"], "Keep Me");
        // marionette provider 形状（magpie 同款配方）。
        let provider = &parsed["provider"]["marionette"];
        assert_eq!(provider["npm"], "@ai-sdk/openai-compatible");
        assert_eq!(provider["options"]["baseURL"], "http://127.0.0.1:3425/v1");
        assert_eq!(provider["options"]["apiKey"], crate::engine::ENGINE_TOKEN);
        // 模型 id 原样、名字取最后一段。
        assert_eq!(provider["models"]["grok/grok-4.7"]["name"], "grok-4.7");
        assert_eq!(provider["models"]["codex/gpt-5.6-luna"]["name"], "gpt-5.6-luna");
        assert_eq!(provider["models"]["opencode-go/glm-5.3"]["name"], "glm-5.3");

        // 坏 JSON existing → 整体替换（只剩 marionette）。
        let replaced =
            opencode_engine_config_content(3425, &["grok/grok-4.7".to_string()], Some("not json"));
        let parsed: serde_json::Value = serde_json::from_str(&replaced).unwrap();
        assert!(parsed.get("autoupdate").is_none());
        assert!(parsed["provider"]["marionette"]["models"]["grok/grok-4.7"].is_object());
    }

    // ─── pi models.json 托管写入器 ───────────────────────────────────────────

    /// 用户已有 ollama provider：只增改 marionette，别家与顶层键原样保留。
    #[test]
    fn pi_merge_preserves_user_providers() {
        let existing = r#"{
  "$schema": "https://pi.dev/schemas/models.schema.json",
  "providers": {
    "ollama": {
      "baseUrl": "http://localhost:11434/v1",
      "api": "openai-completions",
      "apiKey": "ollama",
      "models": [{ "id": "qwen2.5-coder:7b" }]
    }
  }
}"#;
        let models = vec!["group/auto-grok-4-7".to_string()];
        let (merged, changed) = pi_merge_models_json(Some(existing), 3425, &models);
        assert!(changed);
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(
            parsed["$schema"],
            "https://pi.dev/schemas/models.schema.json"
        );
        assert_eq!(
            parsed["providers"]["ollama"]["baseUrl"],
            "http://localhost:11434/v1"
        );
        assert_eq!(
            parsed["providers"]["marionette"]["baseUrl"],
            "http://127.0.0.1:3425/v1"
        );
        assert_eq!(
            parsed["providers"]["marionette"]["apiKey"],
            "$MARIONETTE_API_KEY"
        );
        assert_eq!(
            parsed["providers"]["marionette"]["models"][0]["id"],
            "group/auto-grok-4-7"
        );
        // 语义幂等：同参数再合 → 不再标记变更（不 churn 用户格式）。
        let (_, again) = pi_merge_models_json(Some(&merged), 3425, &models);
        assert!(!again);
        // 端口变了 → 变更标记 + 新 URL。
        let (next, port_changed) = pi_merge_models_json(Some(&merged), 3499, &models);
        assert!(port_changed);
        assert!(next.contains("3499"));
    }

    /// 空文件/缺文件 → 全新文档；移除时保用户 provider、收空 providers。
    #[test]
    fn pi_remove_strips_managed_and_keeps_user_providers() {
        let dir = temp_dir("pi_remove");
        let path = dir.join("models.json");
        // 没有文件 → 移除是 no-op。
        assert!(!pi_remove_marionette_at(&path).unwrap());

        let existing = r#"{
  "providers": {
    "ollama": { "baseUrl": "http://localhost:11434/v1" },
    "marionette": { "baseUrl": "http://127.0.0.1:3425/v1" }
  }
}"#;
        fs::write(&path, existing).unwrap();
        assert!(pi_remove_marionette_at(&path).unwrap());
        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(after["providers"].get("marionette").is_none());
        assert_eq!(
            after["providers"]["ollama"]["baseUrl"],
            "http://localhost:11434/v1"
        );

        // 只剩托管段 → providers 键整个收掉。
        let only = r#"{ "providers": { "marionette": { "baseUrl": "http://x" } } }"#;
        fs::write(&path, only).unwrap();
        assert!(pi_remove_marionette_at(&path).unwrap());
        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(after.get("providers").is_none());
        // 再删一次 → no-op（字节稳定）。
        assert!(!pi_remove_marionette_at(&path).unwrap());
        fs::remove_dir_all(&dir).ok();
    }

    // ─── zcode provider_config.json 托管写入器 ─────────────────────────────

    /// 用户已有自建 provider + 自定义 modelRule：只动三处托管位，别家原样；
    /// marionette **追加**到 providerOrder 尾部（不前插，不动默认模型）。
    #[test]
    fn zcode_merge_preserves_user_layers() {
        let existing = r#"{
  "schemaVersion": 1,
  "config": {
    "providerOrder": ["58062756-dead-beef", "bigmodel-api"],
    "providerConfigRules": {
      "providerRules": [
        {
          "providerId": "58062756-dead-beef",
          "providerName": "Deepseek",
          "enabled": false,
          "config": {
            "group": "standard-personal",
            "access": { "type": "api-key", "apiKey": "sk-user" },
            "api": { "type": "anthropic-messages", "baseUrl": "https://api.deepseek.com/anthropic" },
            "personalModelIds": ["deepseek-v4-pro"],
            "modelOrder": ["deepseek-v4-pro"]
          }
        }
      ]
    },
    "modelConfigRules": {
      "providerModelRules": [
        { "providerId": "58062756-dead-beef", "modelId": "deepseek-v4-pro", "config": { "properties": { "contextWindow": 128000 } } }
      ]
    }
  }
}"#;
        let models = vec![
            CatalogModelRow {
                id: "glm-5.3".to_string(),
                context_window: 1_000_000,
                max_output: 65536,
                images: true,
            },
            CatalogModelRow {
                id: "grok-4.7".to_string(),
                context_window: 256_000,
                max_output: 0,
                images: false,
            },
        ];
        let (merged, changed) = zcode_merge_provider_config(Some(existing), 3425, &models);
        assert!(changed);
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();

        // providerOrder：用户顺序在前，marionette 追加在尾。
        let order: Vec<&str> = parsed["config"]["providerOrder"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(order, vec!["58062756-dead-beef", "bigmodel-api", "marionette"]);

        // 用户 provider 规则原样 + 托管规则在列、字段形状对齐官方模板。
        let rules = parsed["config"]["providerConfigRules"]["providerRules"]
            .as_array()
            .unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["providerId"], "58062756-dead-beef");
        assert_eq!(rules[0]["config"]["access"]["apiKey"], "sk-user");
        let managed = &rules[1];
        assert_eq!(managed["providerId"], "marionette");
        assert_eq!(managed["providerName"], "Marionette Engine");
        assert_eq!(managed["enabled"], true);
        assert_eq!(managed["config"]["api"]["type"], "openai-chat-completions");
        assert_eq!(
            managed["config"]["api"]["baseUrl"],
            "http://127.0.0.1:3425/v1"
        );
        assert_eq!(
            managed["config"]["access"]["apiKey"],
            crate::engine::ENGINE_TOKEN
        );
        assert_eq!(managed["config"]["personalModelIds"][0], "glm-5.3");

        // 用户 modelRule 保留；托管条目是显式 providerId+modelId（个人层
        // schema 不收内置表那种 regex modelRules——整层会被 zod 拒掉）。
        let model_rules = parsed["config"]["modelConfigRules"]["providerModelRules"]
            .as_array()
            .unwrap();
        let managed_rules: Vec<&serde_json::Value> = model_rules
            .iter()
            .filter(|r| r["providerId"] == "marionette")
            .collect();
        assert_eq!(managed_rules.len(), 2);
        assert_eq!(
            model_rules
                .iter()
                .filter(|r| r["providerId"] != "marionette")
                .count(),
            1,
            "user's own providerModelRule must survive"
        );
        // 窗口/图片按目录行写；max_output=0 → 不写 optionSpecs。
        let glm_rule = managed_rules
            .iter()
            .find(|r| r["modelId"] == "glm-5.3")
            .unwrap();
        assert_eq!(glm_rule["config"]["properties"]["contextWindow"], 1_000_000);
        assert_eq!(
            glm_rule["config"]["properties"]["inputFormat"]["supportsImage"],
            true
        );
        assert_eq!(glm_rule["config"]["optionSpecs"]["maxOutputTokens"]["max"], 65536);
        let grok_rule = managed_rules
            .iter()
            .find(|r| r["modelId"] == "grok-4.7")
            .unwrap();
        assert!(grok_rule["config"].get("optionSpecs").is_none());

        // 语义幂等：同参数再合 → 不再标记变更。
        let (_, again) = zcode_merge_provider_config(Some(&merged), 3425, &models);
        assert!(!again);
        // 引擎目录换血：旧托管条目清掉，用户条目不动。
        let models2 = vec![CatalogModelRow {
            id: "glm-5.3".to_string(),
            context_window: 1_000_000,
            max_output: 65536,
            images: true,
        }];
        let (next, roster_changed) = zcode_merge_provider_config(Some(&merged), 3425, &models2);
        assert!(roster_changed);
        let parsed2: serde_json::Value = serde_json::from_str(&next).unwrap();
        let rules2 = parsed2["config"]["modelConfigRules"]["providerModelRules"]
            .as_array()
            .unwrap();
        assert_eq!(
            rules2
                .iter()
                .filter(|r| r["providerId"] == "marionette" && r["modelId"] == "grok-4.7")
                .count(),
            0,
            "stale managed model entry must be swept"
        );
        assert_eq!(
            rules2
                .iter()
                .filter(|r| r["providerId"] != "marionette")
                .count(),
            1
        );
    }

    /// 引擎没报窗口（0）时不能把 0 原样写进个人层：zcode 的 zod 要求
    /// contextWindow > 0，一条 0 就让整层降级空配置，用户所有 provider 消失。
    #[test]
    fn zcode_model_rules_fallback_when_context_window_unknown() {
        let models = vec![CatalogModelRow {
            id: "mystery-model".to_string(),
            context_window: 0,
            max_output: 0,
            images: false,
        }];
        let rules = zcode_marionette_model_rules(&models);
        assert_eq!(rules.len(), 1);
        let window = rules[0]["config"]["properties"]["contextWindow"]
            .as_u64()
            .expect("contextWindow must be a number");
        assert_eq!(window, ZCODE_UNKNOWN_CONTEXT_WINDOW);
        assert!(window > 0, "zcode schema requires contextWindow > 0");
        // max_output 未知时同样不写 0 的 optionSpecs。
        assert!(rules[0]["config"].get("optionSpecs").is_none());

        // 合并结果里也不允许出现 0（含用户已有文件被整体重写的情况）。
        let (merged, _) = zcode_merge_provider_config(None, 3425, &models);
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let window = parsed["config"]["modelConfigRules"]["providerModelRules"][0]["config"]
            ["properties"]["contextWindow"]
            .as_u64()
            .unwrap();
        assert_eq!(window, ZCODE_UNKNOWN_CONTEXT_WINDOW);
    }

    /// 移除：三处托管位全清、用户内容保真；没文件/没托管段 → no-op。
    #[test]
    fn zcode_remove_strips_managed_bits() {
        let dir = temp_dir("zcode_remove");
        let path = dir.join("provider_config.json");
        // 没有文件 → no-op。
        assert!(!zcode_remove_marionette_at(&path).unwrap());

        let models = vec![CatalogModelRow {
            id: "glm-5.3".to_string(),
            context_window: 200_000,
            max_output: 0,
            images: false,
        }];
        let user_fixture = r#"{
  "schemaVersion": 1,
  "config": {
    "providerOrder": ["user-provider"],
    "providerConfigRules": {
      "providerRules": [
        { "providerId": "user-provider", "providerName": "User", "enabled": true, "config": {} }
      ]
    },
    "modelConfigRules": {
      "providerModelRules": [
        { "providerId": "user-provider", "modelId": "user-model", "config": {} }
      ]
    }
  }
}"#;
        // 先把用户 fixture 合成托管后的形态（引擎在跑的写入路径）。
        let (merged, _) = zcode_merge_provider_config(Some(user_fixture), 3425, &models);
        fs::write(&path, &merged).unwrap();
        assert!(zcode_remove_marionette_at(&path).unwrap());
        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let order: Vec<&str> = after["config"]["providerOrder"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(order, vec!["user-provider"]);
        assert_eq!(
            after["config"]["providerConfigRules"]["providerRules"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let model_rules: Vec<&str> = after["config"]["modelConfigRules"]["providerModelRules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["modelId"].as_str().unwrap())
            .collect();
        assert_eq!(model_rules, vec!["user-model"]);
        // 再删一次 → no-op。
        assert!(!zcode_remove_marionette_at(&path).unwrap());
        fs::remove_dir_all(&dir).ok();
    }

    /// 新版桌面 schema 要求 modelConfigRules.manualProviderModelRules 是
    /// 必填数组（2026-10-24 重装后桥从头建文件缺该键 → 整层个人配置拒载、
    /// 内存降级为空 → turn 死在 model_creation「Select a model before
    /// continuing」）。缺失/非数组补空数组，已有值原样保留。
    #[test]
    fn zcode_merge_backfills_manual_provider_model_rules() {
        let models = vec![CatalogModelRow {
            id: "glm-5.3".to_string(),
            context_window: 200_000,
            max_output: 0,
            images: false,
        }];

        // 从头建文件（无既有内容，重装后首次 spawn 的场景）→ 键存在且为数组。
        let (fresh, changed) = zcode_merge_provider_config(None, 3425, &models);
        assert!(changed);
        let parsed: serde_json::Value = serde_json::from_str(&fresh).unwrap();
        assert!(parsed["config"]["modelConfigRules"]["manualProviderModelRules"].is_array());

        // 桌面写过的文件带 manualProviderModelRules 条目 → 合并后原样保留。
        let desktop_fixture = r#"{
  "schemaVersion": 1,
  "config": {
    "providerOrder": ["user-provider"],
    "providerConfigRules": {
      "providerRules": [
        { "providerId": "user-provider", "providerName": "User", "enabled": true, "config": {} }
      ]
    },
    "modelConfigRules": {
      "providerModelRules": [
        { "providerId": "user-provider", "modelId": "user-model", "config": {} }
      ],
      "manualProviderModelRules": [
        { "providerId": "user-provider", "modelId": "manual-model", "config": {} }
      ]
    }
  }
}"#;
        let (merged, _) = zcode_merge_provider_config(Some(desktop_fixture), 3425, &models);
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let manual: Vec<&str> = parsed["config"]["modelConfigRules"]["manualProviderModelRules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["modelId"].as_str().unwrap())
            .collect();
        assert_eq!(manual, vec!["manual-model"]);

        // 旧版文件缺该键 → 合并时补空数组（桌面 zod 必填）。
        let legacy_fixture = r#"{
  "schemaVersion": 1,
  "config": {
    "providerConfigRules": {
      "providerRules": [
        { "providerId": "user-provider", "providerName": "User", "enabled": true, "config": {} }
      ]
    },
    "modelConfigRules": {
      "providerModelRules": [
        { "providerId": "user-provider", "modelId": "user-model", "config": {} }
      ]
    }
  }
}"#;
        let (merged, changed) = zcode_merge_provider_config(Some(legacy_fixture), 3425, &models);
        assert!(changed);
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(
            parsed["config"]["modelConfigRules"]["manualProviderModelRules"],
            serde_json::json!([])
        );
    }

    #[test]
    fn opencode_provider_map_is_case_insensitive_and_covers_the_bridge() {
        for (provider, expected) in [
            ("openrouter", "openrouter"),
            ("deepseek", "deepseek"),
            ("moonshot", "kimi"),
            ("kimi", "kimi"),
            ("glm", "glm"),
            // opencode auth login「智谱 AI Coding Plan」落在 auth.json 的键名
            //（2026-10-17 本机实测）——claude 跨用菜单看不到 GLM 的直接根因。
            ("zhipuai-coding-plan", "glm"),
            ("xai", "xai"),
            ("opencode", "opencode-go"),
            ("OPENCODE-GO", "opencode-go"),
            ("OpenRouter", "openrouter"),
            ("DeepSeek", "deepseek"),
            ("Moonshot", "kimi"),
            ("KIMI", "kimi"),
            ("GLM", "glm"),
            ("XAI", "xai"),
        ] {
            let preset = preset_for_opencode_provider(provider)
                .unwrap_or_else(|| panic!("`{provider}` 应映射到 `{expected}`"));
            assert_eq!(preset.id, expected, "provider `{provider}`");
            assert_eq!(preset.id, find_preset(expected).unwrap().id);
        }
        // 未映射 → None：凭据页展示，但没有 L1 端点、不直连。zai 是 Z.ai
        // （api.z.ai）的键，与智谱 open.bigmodel.cn 不同 host，映射等于把别家
        // 的 key 发错厂商（magpie 分列两个 preset，我们只展不连）；按量的
        // zhipuai 同样不映射 —— coding 端点只吃套餐 key。
        for unknown in [
            "google",
            "anthropic",
            "openai",
            "vercel",
            "zai",
            "zai-coding-plan",
            "zhipuai",
            "",
        ] {
            assert!(
                preset_for_opencode_provider(unknown).is_none(),
                "`{unknown}` 不在映射表里，不该命中"
            );
        }
    }

    /// 引擎注册的引用归一化：原始 provider id → preset id；未映射过滤；
    /// 保序（同 preset 多 key 的优先序由凭据源层保证，见
    /// credentials::tests::merge_prefers_db_live_keys）。
    #[test]
    fn reference_keys_as_presets_normalizes_for_registration() {
        let out = reference_keys_as_presets(vec![
            ("zhipuai-coding-plan".to_string(), "glm-key".to_string()),
            ("deepseek".to_string(), "ds-key".to_string()),
            ("moonshot".to_string(), "kimi-key".to_string()),
            ("anthropic".to_string(), "oauth-key".to_string()),
            ("zai".to_string(), "zai-key".to_string()),
        ]);
        assert_eq!(
            out,
            vec![
                ("glm".to_string(), "glm-key".to_string()),
                ("deepseek".to_string(), "ds-key".to_string()),
                ("kimi".to_string(), "kimi-key".to_string()),
            ]
        );
        assert!(reference_keys_as_presets(Vec::new()).is_empty());
    }

    /// 目录测试的串行锁：动态缓存与端点覆盖表是**模块级**全局（fn 内的
    /// static 各是各的锁，互不了斥——两个目录测试曾因此互踩覆盖表）。一切
    /// 会触发 fetch 的 list_models_at 测试都必须持有它。
    static CATALOG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 动态目录：/models 拉取结果与静态清单并集；label 用模型 id（回归：
    /// 此前整组都显示 preset 名，一排「DeepSeek」）。
    #[test]
    fn dynamic_catalog_merges_and_labels_use_model_id() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let _fetch_guard = CATALOG_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // note 断言依赖引擎 Absent；与 test_running 共享串行锁防互踩。
        let _absent = crate::engine::test_absent_guard();
        let dir = temp_dir("dyncat");
        let home = fixture_opencode_home(&dir);
        let scan = crate::credentials::scan_opencode_at(&home);
        let credentials = dir.join("credentials.json");

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf); // 读完请求头
            let body = br#"{"data":[{"id":"deepseek-chat"},{"id":"deepseek-reasoner"}]}"#;
            let head = format!(
                "HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: {}
Connection: close

",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
        });

        for preset in presets() {
            set_fetch_base_override_for_test(preset.id, "http://127.0.0.1:1");
        }
        set_fetch_base_override_for_test("deepseek", &format!("http://127.0.0.1:{port}/v1"));
        clear_dynamic_cache_for_test();

        let result = list_models_at(&credentials, &scan, "grok-build");
        // 测试态引擎未 Running → 引擎指引 note（2026-10-17 起）。
        assert!(
            result.note.as_deref().is_some_and(|n| n.contains("模型共享未运行")),
            "note = {:?}",
            result.note
        );
        let ds: Vec<&BridgeModel> = result
            .models
            .iter()
            .filter(|m| m.entry_id == "opencode/deepseek")
            .collect();
        let ids: Vec<&str> = ds.iter().map(|m| m.model_id.as_str()).collect();
        assert!(ids.contains(&"deepseek-chat"), "ids = {ids:?}");
        assert!(ids.contains(&"deepseek-reasoner"), "ids = {ids:?}");
        assert!(ids.contains(&"deepseek-flash"), "静态清单要并集保留: {ids:?}");
        for model in &ds {
            assert_eq!(model.label, model.model_id, "label 必须是模型 id，不是 preset 名");
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_both_catalog_shapes() {
        assert_eq!(
            parse_models_catalog(r#"{"data":[{"id":"a"},{"id":"b"},{"id":"a"}]}"#),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            parse_models_catalog(r#"{"models":[{"id":"deepseek-chat"}]}"#),
            vec!["deepseek-chat".to_string()]
        );
        assert!(parse_models_catalog("not json").is_empty());
    }

    /// magpie 网关 /v1/models 的行带窗口/输出上限（gateway.go modelObject：
    /// context_window + context_length + max_input_tokens 同值、max_output_tokens）。
    /// dsh 桥靠它写 contextWindow，否则 llm-pi-ai 把一切当 256K。
    #[test]
    fn parse_rows_carries_window_and_output() {
        let rows = parse_models_catalog_rows(
            r#"{"data":[
                {"id":"deepseek/deepseek-flash","context_window":1000000,"context_length":1000000,"max_input_tokens":1000000,"max_output_tokens":384000},
                {"id":"relay/mystery"},
                {"id":"grok/grok-4.7@500k","context_window":500000,"max_output_tokens":0}
            ]}"#,
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0],
            CatalogModelRow {
                id: "deepseek/deepseek-flash".into(),
                context_window: 1_000_000,
                max_output: 384_000,
                images: false,
            }
        );
        assert_eq!(rows[1].context_window, 0, "没说的窗口是 0（不写 contextWindow）");
        assert_eq!(rows[1].max_output, 0);
        assert_eq!(rows[2].context_window, 500_000);
        assert_eq!(rows[2].max_output, 0, "max_output_tokens 0 = 未披露");
    }

    /// 图片能力来自 `modalities.input`（magpie `/v1/models` 的形状）：DSH 桥
    /// 靠它决定是否给模型条目写 `input: [text, image]`。没声明（或只列 text）
    /// 的来源按文本模型处理。
    #[test]
    fn parses_image_capability_from_modalities() {
        let rows = parse_models_catalog_rows(
            r#"{"data":[
                {"id":"opencode-go/deepseek-v4.1-flash","modalities":{"input":["text","image"],"output":["text"]}},
                {"id":"opencode-go/deepseek-v4-pro","modalities":{"input":["text"],"output":["text"]}},
                {"id":"plain/m1"},
                {"id":"array/m1","modalities":["text","image"]}
            ]}"#,
        );
        let images: Vec<(String, bool)> = rows.into_iter().map(|row| (row.id, row.images)).collect();
        assert_eq!(
            images,
            vec![
                ("opencode-go/deepseek-v4.1-flash".to_string(), true),
                ("opencode-go/deepseek-v4-pro".to_string(), false),
                ("plain/m1".to_string(), false),
                ("array/m1".to_string(), true),
            ]
        );
    }
    /// reference 并入 list_models：grok / claude 看到 opencode auth.json 里映射
    /// 得到 preset 的条目（模型列表为空的来源也要出现）；宿主 opencode 自己
    /// 不可见；未映射 provider 与 OAuth 条目不出现；扫描失败静默跳过该家、
    /// store 条目照常。
    #[test]
    fn list_models_merges_scanned_references() {
        let _fetch_guard = CATALOG_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // 目录/来源断言依赖引擎 Absent；与 test_running 共享串行锁防互踩。
        let _absent = crate::engine::test_absent_guard();
        let dir = temp_dir("reflist");
        let home = fixture_opencode_home(&dir);
        let scan = crate::credentials::scan_opencode_at(&home);
        assert!(scan.error.is_none());
        let credentials = dir.join("credentials.json");
        crate::credentials::add_key_plan_at(&credentials, "xai", "xAI", "sk-fixture-xai-889900")
            .unwrap();
        // 测试封闭性：把所有 preset 的动态目录端点指向必然拒连的本地端口，
        // 走「拉取失败 → 静态清单」路径（动态路径由专属测试用本地监听验证）。
        for preset in presets() {
            set_fetch_base_override_for_test(preset.id, "http://127.0.0.1:1");
        }
        clear_dynamic_cache_for_test();

        for agent in ["grok-build", "claude-code"] {
            let result = list_models_at(&credentials, &scan, agent);
            // 测试态引擎未 Running → 引擎指引 note（2026-10-17 起）。
            assert!(
                result.note.as_deref().is_some_and(|n| n.contains("模型共享未运行")),
                "{agent}: note = {:?}",
                result.note
            );
            // 来源：store 的 xai + 扫描的 deepseek / openrouter。openrouter 的
            // preset 没有默认模型列表，来源照样要报（契约：空也要让前端知道）。
            let mut source_ids: Vec<&str> =
                result.sources.iter().map(|s| s.entry_id.as_str()).collect();
            source_ids.sort_unstable();
            assert_eq!(
                source_ids,
                vec!["opencode/deepseek", "opencode/openrouter", "xai"],
                "{agent}"
            );
            assert!(result.sources.iter().any(|s| s.entry_id == "opencode/deepseek"
                && s.preset == "deepseek"
                && s.from_scan));
            assert!(result.sources.iter().any(|s| s.entry_id == "xai" && !s.from_scan));
            // google（未映射）、anthropic / hybrid（OAuth）一概不出现。
            // 模型：xai 的 grok-4.7 + deepseek 的两个，各自挂在 entry 上。
            let mut model_ids: Vec<&str> = result.models.iter().map(|m| m.model_id.as_str()).collect();
            model_ids.sort_unstable();
            assert_eq!(model_ids, vec!["deepseek-flash", "deepseek-v4-pro", "grok-4.7"], "{agent}");
            assert!(result
                .models
                .iter()
                .filter(|m| m.entry_id == "opencode/deepseek")
                .all(|m| m.preset == "deepseek"));
            // 序列化无明文（reference 不带 key，key-plan 是密文）；契约字段名
            // （camelCase）逐字钉住 —— 前端并行开发中。
            let leaked = serde_json::to_string(&result).unwrap();
            assert!(!leaked.contains("sk-fixture"), "{agent} 的结果泄漏了明文 key");
            assert!(leaked.contains("\"sources\":"), "{agent}: {leaked}");
            assert!(leaked.contains("\"entryId\":\"opencode/deepseek\""), "{agent}");
            assert!(leaked.contains("\"fromScan\":true"), "{agent}");
        }

        // 宿主 opencode 自己：空 + 原生管理文案（去重规则不变）。
        let self_view = list_models_at(&credentials, &scan, "opencode");
        assert!(self_view.models.is_empty());
        assert!(self_view.sources.is_empty());
        assert_eq!(
            self_view.note.as_deref(),
            Some("模型共享未运行 — Grok/GPT 等订阅模型暂不可见；点左下角模型共享按钮启动（或稍候自动唤醒）后重开此菜单")
        );

        // 扫描失败（auth.json 损坏）→ 静默跳过该家，store 条目照常。
        let auth = home.join(".local").join("share").join("opencode").join("auth.json");
        fs::write(&auth, "{broken").unwrap();
        let broken = crate::credentials::scan_opencode_at(&home);
        assert!(broken.error.is_some(), "损坏的 auth.json 应报 error");
        let result = list_models_at(&credentials, &broken, "grok-build");
        let source_ids: Vec<&str> = result.sources.iter().map(|s| s.entry_id.as_str()).collect();
        assert_eq!(source_ids, vec!["xai"], "扫描坏了不该连 store 条目也没了");
        assert!(result.models.iter().all(|m| m.entry_id == "xai"));
        // 测试态引擎未 Running → 引擎指引 note（2026-10-17 起）。
        assert!(result.note.as_deref().is_some_and(|n| n.contains("模型共享未运行")));

        // 只有「模型列表为空」的来源时：models 空 + 说明，但来源在。
        let home_or = dir.join("opencode-or-only");
        let or_auth = home_or.join(".local").join("share").join("opencode").join("auth.json");
        fs::create_dir_all(or_auth.parent().unwrap()).unwrap();
        fs::write(
            &or_auth,
            r#"{"openrouter": {"type": "api", "key": "sk-fixture-or-3d4e5f"}}"#,
        )
        .unwrap();
        let scan_or = crate::credentials::scan_opencode_at(&home_or);
        let empty_store = dir.join("credentials-none.json");
        let result = list_models_at(&empty_store, &scan_or, "grok-build");
        assert!(result.models.is_empty());
        assert_eq!(
            result.note.as_deref(),
            Some("当前凭证没有可用的共享模型（拉取模型目录失败，可稍后重试或用自定义输入）")
        );
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.sources[0].entry_id, "opencode/openrouter");
        assert!(result.sources[0].from_scan);
        assert_eq!(result.sources[0].preset, "openrouter");

        // 全局状态复位（死端口覆盖 / 失败缓存不能泄漏给并行的目录测试）。
        clear_fetch_overrides_for_test();
        clear_dynamic_cache_for_test();
        fs::remove_dir_all(&dir).ok();
    }

    /// 2026-10-17 实修回归：opencode「智谱 Coding Plan」的 auth 键名是
    /// `zhipuai-coding-plan`，此前映射表只认裸 `glm` —— claude 跨用菜单整个
    /// 看不到 GLM（凭据页有、picker 无），引擎注册也整体跳过。
    #[test]
    fn zhipu_coding_plan_reference_reaches_claude_menu() {
        let _fetch_guard = CATALOG_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // sources.len()==1 断言依赖引擎 Absent；与 test_running 共享串行锁。
        let _absent = crate::engine::test_absent_guard();
        let dir = temp_dir("zhipu-ref");
        let home = dir.join("opencode-home");
        let auth = home.join(".local").join("share").join("opencode").join("auth.json");
        fs::create_dir_all(auth.parent().unwrap()).unwrap();
        fs::write(
            &auth,
            r#"{"zhipuai-coding-plan": {"type": "api", "key": "sk-fixture-glm-1a2b3c"}}"#,
        )
        .unwrap();
        let scan = crate::credentials::scan_opencode_at(&home);
        assert!(scan.error.is_none());
        // 动态拉取指向必然拒连的本地端口 → 走静态兜底清单。
        set_fetch_base_override_for_test("glm", "http://127.0.0.1:1");
        clear_dynamic_cache_for_test();

        let credentials = dir.join("credentials.json");
        let result = list_models_at(&credentials, &scan, "claude-code");
        assert_eq!(result.sources.len(), 1, "glm 来源要出现（现修前这里是 0）");
        assert_eq!(result.sources[0].entry_id, "opencode/zhipuai-coding-plan");
        assert_eq!(result.sources[0].preset, "glm");
        assert!(result.sources[0].from_scan);
        let models: Vec<&str> = result.models.iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(models, vec!["glm-5.3", "glm-5.3-flash"]);
        assert!(result.models.iter().all(|m| m.preset == "glm"));

        clear_fetch_overrides_for_test();
        clear_dynamic_cache_for_test();
        fs::remove_dir_all(&dir).ok();
    }

    /// reference 绑定校验：映射命中放行（绑定文件里只有 entryId，key 不复制）；
    /// 未映射 → 拒且消息说明没有 L1 端点；OAuth / 混合 / 消失的条目 → 拒。
    #[test]
    fn reference_binding_validates_mapping() {
        let dir = temp_dir("refbind");
        let home = fixture_opencode_home(&dir);
        let scan = crate::credentials::scan_opencode_at(&home);
        let credentials = dir.join("credentials.json");
        let bindings = dir.join("bridge-bindings.json");
        let grok_config = dir.join("config.toml");

        // 有映射 → 绑定成功。
        bind_session_at(
            &bindings,
            &credentials,
            &grok_config,
            &scan,
            "s1",
            Some("grok-build"),
            Some(BridgeSelection {
                entry_id: "opencode/deepseek".to_string(),
                model_id: "deepseek-flash".to_string(),
            }),
        )
        .unwrap();
        let bound = binding_for_at(&bindings, "s1").unwrap().expect("应已绑定");
        assert_eq!(bound.entry_id, "opencode/deepseek");
        // reference 不进 store（nothing is stored twice）。
        let store = crate::credentials::load_store_at(&credentials).unwrap();
        assert!(store.entries.is_empty());

        // 未映射 provider → 拒，消息说明原因。
        let error = bind_session_at(
            &bindings,
            &credentials,
            &grok_config,
            &scan,
            "s2",
            Some("grok-build"),
            Some(BridgeSelection {
                entry_id: "opencode/google".to_string(),
                model_id: "x".to_string(),
            }),
        )
        .unwrap_err();
        assert!(
            error.contains("未映射") && error.contains("L1"),
            "unexpected: {error}"
        );
        assert!(binding_for_at(&bindings, "s2").unwrap().is_none(), "失败的绑定不能落盘");

        // OAuth 条目 / 混合条目（扫描侧归 subscription，不是 reference）与
        // 彻底消失的条目 → 一律按「不存在」拒。
        for ghost in ["opencode/anthropic", "opencode/hybrid", "opencode/nope"] {
            let error = bind_session_at(
                &bindings,
                &credentials,
                &grok_config,
                &scan,
                "s3",
                Some("grok-build"),
                Some(BridgeSelection {
                    entry_id: ghost.to_string(),
                    model_id: "x".to_string(),
                }),
            )
            .unwrap_err();
            assert!(error.contains("不存在"), "`{ghost}`: unexpected: {error}");
        }

        fs::remove_dir_all(&dir).ok();
    }

    /// 现场读钥匙：api 条目取到；OAuth / 混合条目拒；文件缺失 / 条目不在 /
    /// 缺 ref / 坏格式 → Err。结果只进断言，不进任何日志（防泄漏由序列化断言
    /// 与 grep 守卫另测）。
    #[test]
    fn peek_reference_secret_reads_api_and_rejects_oauth() {
        let dir = temp_dir("refpeek");
        let home = fixture_opencode_home(&dir);
        let auth = home.join(".local").join("share").join("opencode").join("auth.json");
        let scan = crate::credentials::scan_opencode_at(&home);

        let ref_entry = |provider: &str| CredentialEntry {
            custom_provider: None,
            id: format!("opencode/{provider}"),
            kind: CredentialKind::Reference,
            label: provider.to_string(),
            preset: None,
            secret: None,
            ref_path: Some(format!("{}#{provider}", auth.display())),
            host_agent: Some("opencode".to_string()),
            source: "scan:opencode".to_string(),
            created_at: "0".to_string(),
            last_used_at: None,
        };

        // api 条目 → 取到（假）key。
        assert_eq!(
            peek_reference_secret(&ref_entry("deepseek")).unwrap(),
            "sk-fixture-ds-0a1b2c"
        );
        // OAuth 条目 → 拒（直连需要真 API key，access token 顶不了）。
        let error = peek_reference_secret(&ref_entry("anthropic")).unwrap_err();
        assert!(error.contains("api"), "unexpected: {error}");
        // 混合条目（type api + refresh）→ 拒：refresh 丢了不可恢复，OAuth 优先。
        let error = peek_reference_secret(&ref_entry("hybrid")).unwrap_err();
        assert!(error.contains("api"), "unexpected: {error}");
        // 条目不在文件里 → Err。
        let error = peek_reference_secret(&ref_entry("ghost")).unwrap_err();
        assert!(error.contains("已不在"), "unexpected: {error}");
        // 文件缺失 → Err。
        let mut missing = ref_entry("deepseek");
        missing.ref_path =
            Some(format!("{}#deepseek", dir.join("no-such-file.json").display()));
        let error = peek_reference_secret(&missing).unwrap_err();
        assert!(error.contains("读取"), "unexpected: {error}");
        // 缺 ref / 坏格式（没有 #）→ Err。
        let mut no_ref = ref_entry("deepseek");
        no_ref.ref_path = None;
        assert!(peek_reference_secret(&no_ref).is_err());
        let mut bad_shape = ref_entry("deepseek");
        bad_shape.ref_path = Some(auth.display().to_string());
        assert!(peek_reference_secret(&bad_shape).is_err());

        // 扫描出来的真条目与 peek 端到端对得上。
        let scanned = scan
            .entries
            .iter()
            .find(|entry| entry.id == "opencode/deepseek")
            .expect("扫描应含 opencode/deepseek");
        assert_eq!(peek_reference_secret(scanned).unwrap(), "sk-fixture-ds-0a1b2c");

        // peek_spawn_key：key-plan 走 store DPAPI，reference 现场读。
        let credentials = dir.join("credentials.json");
        crate::credentials::add_key_plan_at(&credentials, "deepseek", "DeepSeek", "sk-fixture-store-ds")
            .unwrap();
        assert_eq!(
            peek_spawn_key_at(&credentials, &scan, "deepseek").unwrap(),
            "sk-fixture-store-ds"
        );
        assert_eq!(
            peek_spawn_key_at(&credentials, &scan, "opencode/deepseek").unwrap(),
            "sk-fixture-ds-0a1b2c"
        );
        assert_eq!(peek_spawn_key_at(&credentials, &scan, "opencode/ghost"), None);

        fs::remove_dir_all(&dir).ok();
    }

    /// reference 的注入形态与 key-plan 完全一致（claude env / grok owned 表）；
    /// 源文件消失 → Err（残留绑定降级为普通会话 + warn 的上游信号），
    /// key-plan 条目不受影响。
    #[test]
    fn reference_injection_matches_key_plan_shape() {
        let dir = temp_dir("refinject");
        let home = fixture_opencode_home(&dir);
        let auth = home.join(".local").join("share").join("opencode").join("auth.json");
        let scan = crate::credentials::scan_opencode_at(&home);
        let credentials = dir.join("credentials.json");
        crate::credentials::add_key_plan_at(&credentials, "deepseek", "DeepSeek", "sk-fixture-store-77aa")
            .unwrap();

        // 同一个 preset（deepseek）：reference 与 key-plan 的注入逐字段相等。
        for agent in ["claude-code", "grok-build", "claude", "grok"] {
            let via_reference =
                injection_for_agent_at(&credentials, &scan, agent, "opencode/deepseek", "deepseek-flash")
                    .unwrap();
            let via_store =
                injection_for_agent_at(&credentials, &scan, agent, "deepseek", "deepseek-flash")
                    .unwrap();
            assert_eq!(via_reference, via_store, "agent {agent}");
        }
        assert_eq!(
            injection_for_agent_at(&credentials, &scan, "claude-code", "opencode/deepseek", "deepseek-flash")
                .unwrap(),
            Some(SpawnInjection::ClaudeEnv {
                base_url: crate::grok_dialect::channel_base_url("deepseek").unwrap(),
                model: "deepseek-flash".into(),
            })
        );
        assert_eq!(
            injection_for_agent_at(&credentials, &scan, "grok-build", "opencode/deepseek", "deepseek-flash")
                .unwrap(),
            Some(SpawnInjection::GrokTable {
                model_id: "deepseek-flash".into(),
                preset_id: "deepseek".into(),
            })
        );

        // 源文件消失 → Err；store 里的 key-plan 照常解析、钥匙照常可取。
        fs::remove_file(&auth).unwrap();
        let gone = crate::credentials::scan_opencode_at(&home);
        let error =
            injection_for_agent_at(&credentials, &gone, "claude-code", "opencode/deepseek", "x")
                .unwrap_err();
        assert!(error.contains("不存在"), "unexpected: {error}");
        assert!(
            injection_for_agent_at(&credentials, &gone, "claude-code", "deepseek", "x")
                .unwrap()
                .is_some()
        );
        assert_eq!(peek_spawn_key_at(&credentials, &gone, "opencode/deepseek"), None);

        fs::remove_dir_all(&dir).ok();
    }

    /// providers/set 参数逐字段对齐 S2 实测处方（providerId/apiType/baseUrl/
    /// headers.Authorization Bearer）。key 只进 headers，不进任何日志路径。
    #[test]
    fn codex_providers_set_params_match_the_s2_prescription() {
        let params = codex_providers_set_params("https://engine.local/v1", "sk-codex-test");
        assert_eq!(
            params,
            serde_json::json!({
                "providerId": "openai",
                "apiType": "openai",
                "baseUrl": "https://engine.local/v1",
                "headers": { "Authorization": "Bearer sk-codex-test" }
            })
        );
        // key/reference 端点（chat-completions）解析不出 codex 路由；引擎伪
        // 来源的路由形态由 codex_engine_route 专测验证。
        assert!(codex_bridge_route(&BridgeSelection {
            entry_id: "opencode-go".into(),
            model_id: "x".into(),
        })
        .is_none());
        // 能力声明的判定：agentCapabilities.providers 存在且非 null。
        assert!(advertises_providers(&serde_json::json!({
            "agentCapabilities": { "providers": { "supportedApiTypes": ["openai"] } }
        })));
        assert!(!advertises_providers(&serde_json::json!({
            "agentCapabilities": { "loadSession": true }
        })));
        assert!(!advertises_providers(&serde_json::json!({
            "agentCapabilities": { "providers": null }
        })));
    }

    // ─── 视图与掩码 ─────────────────────────────────────────────────────────

    #[test]
    fn entries_are_masked_and_never_carry_plaintext() {
        let dir = temp_dir("views");
        let credentials = dir.join("credentials.json");
        crate::credentials::add_key_plan_at(&credentials, "deepseek", "DeepSeek", "sk-view-77aa21zz")
            .unwrap();

        let views = list_entries_at(&credentials).unwrap();
        assert_eq!(views.len(), 1);
        let view = &views[0];
        assert_eq!(view.id, "deepseek");
        assert_eq!(view.kind, "key-plan");
        assert_eq!(view.preset.as_deref(), Some("deepseek"));
        assert_eq!(view.masked_key.as_deref(), Some("…21zz"), "末四位掩码");
        // 全序列化无明文。
        let leaked = serde_json::to_string(&views).unwrap();
        assert!(!leaked.contains("sk-view-77aa21zz"));

        // 短 key 全遮（掩码不能等于明文）。
        assert_eq!(mask_key("short"), None);
        assert_eq!(mask_key("sk-1234abcd"), Some("…abcd".to_string()));

        // add_key 校验 preset。
        let error = add_key_at(&credentials, "not-a-preset", None, "sk-x-123456").unwrap_err();
        assert!(error.contains("未知 preset"), "unexpected: {error}");

        fs::remove_dir_all(&dir).ok();
    }
}
