//! 员工注册表与 Host 强制策略。
//!
//! 员工不是新的 agent runtime：一条档案绑定普通 ACP 会话，策略与身份
//! 校验由 Host 负责。注册表持久化到 `~/.marionette/chief/staff.json`，
//! 坏档按空表 fail-open；写入沿用 chief store 的原子替换纪律。
//!
//! `StaffPolicy` 在档案 JSON 中扁平存放（与 docs/plans/plan-employee-mode.md
//! §5.1 的示例一致），Rust 代码里仍保留独立类型，避免策略字段散落到
//! UI、broker 和工具实现。

use super::types::{now_ms, short_id, ChiefConfig, ProjectPolicy};
use chrono::{Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Mutex;

const STAFF_REGISTRY_VERSION: u32 = 1;
pub const DEFERRED_BOOTSTRAP_VERSION: u32 = u32::MAX;
const MAX_STAFF: usize = 128;
const MAX_STAFF_FIELD_BYTES: usize = 16 * 1024;
const MAX_ALLOWED_PROJECTS: usize = 128;
const CORRUPT_REGISTRY_MARKER: u64 = u64::MAX;

static REGISTRY_LOCK: Mutex<()> = Mutex::new(());
static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 测试沙箱覆写；生产路径始终来自 `app_paths::global_dir()`.
#[cfg(test)]
pub fn set_root_override_for_tests(path: Option<PathBuf>) {
    if let Ok(mut slot) = ROOT_OVERRIDE.lock() {
        *slot = path;
    }
}

fn chief_root() -> Result<PathBuf, String> {
    if let Ok(slot) = ROOT_OVERRIDE.lock() {
        if let Some(path) = slot.as_ref() {
            return Ok(path.clone());
        }
    }
    crate::app_paths::global_dir().map(|dir| dir.join("chief"))
}

/// 注册表文件路径。公开给命令/健康检查读取；不公开写入捷径。
pub fn staff_path() -> Result<PathBuf, String> {
    Ok(chief_root()?.join("staff.json"))
}

fn warn(message: &str, detail: Option<&str>) {
    crate::debug_log::append("chief-staff", "warn", "", message, detail);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StaffAutonomy {
    Manual,
    #[default]
    Assisted,
    Autonomous,
}

/// 一名员工的 Host 策略。所有执行、通知和预算入口都应读取这一份。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct StaffPolicy {
    pub autonomy: StaffAutonomy,
    /// 空表表示未授权任何项目；不要用空表表示“全部”。
    pub allowed_projects: Vec<ProjectPolicy>,
    pub max_active_tasks: u32,
    pub max_turns_per_task: u32,
    pub max_wall_minutes_per_task: u32,
    /// 0 = 不设员工独立上限，仍受 Host 总预算约束。
    pub max_model_calls_per_task: u32,
    /// 0.0 = 不设员工独立上限。
    pub max_cost_per_task_usd: f64,
    pub max_reports_per_hour: u32,
    pub notification_cooldown_minutes: u32,
    /// `HH:MM-HH:MM`；空串表示未配置勿扰。
    pub quiet_hours: String,
    pub allow_background_work: bool,
    pub allow_telegram: bool,
    /// Host 级持久急停。员工工具不能修改此字段。
    pub paused_until_user_resumes: bool,
}

impl Default for StaffPolicy {
    fn default() -> Self {
        Self {
            autonomy: StaffAutonomy::Assisted,
            allowed_projects: Vec::new(),
            max_active_tasks: 1,
            max_turns_per_task: 30,
            max_wall_minutes_per_task: 120,
            max_model_calls_per_task: 0,
            max_cost_per_task_usd: 0.0,
            max_reports_per_hour: 4,
            notification_cooldown_minutes: 30,
            quiet_hours: "23:00-08:00".to_string(),
            allow_background_work: true,
            allow_telegram: false,
            paused_until_user_resumes: false,
        }
    }
}

impl StaffPolicy {
    /// 把外部输入收敛到可执行范围。调用方仍应在保存前校验身份唯一性。
    pub fn normalized(mut self) -> Self {
        self.allowed_projects = self
            .allowed_projects
            .into_iter()
            .filter_map(normalize_project_policy)
            .take(MAX_ALLOWED_PROJECTS)
            .collect();
        dedupe_project_policies(&mut self.allowed_projects);
        self.max_active_tasks = self.max_active_tasks.clamp(1, 3);
        self.max_turns_per_task = self.max_turns_per_task.clamp(1, 500);
        self.max_wall_minutes_per_task = self.max_wall_minutes_per_task.clamp(1, 10_080);
        self.max_model_calls_per_task = self.max_model_calls_per_task.min(100_000);
        if !self.max_cost_per_task_usd.is_finite() || self.max_cost_per_task_usd < 0.0 {
            self.max_cost_per_task_usd = 0.0;
        } else {
            self.max_cost_per_task_usd = self.max_cost_per_task_usd.min(1_000_000.0);
        }
        self.max_reports_per_hour = self.max_reports_per_hour.min(120);
        self.notification_cooldown_minutes = self.notification_cooldown_minutes.min(1_440);
        self.quiet_hours = self.quiet_hours.trim().to_string();
        if !self.quiet_hours.is_empty() && !valid_quiet_hours(&self.quiet_hours) {
            self.quiet_hours = StaffPolicy::default().quiet_hours;
        }
        if self.quiet_hours.len() > 32 {
            self.quiet_hours = StaffPolicy::default().quiet_hours;
        }
        self
    }

    pub fn max_active_tasks(&self) -> u32 {
        self.max_active_tasks.clamp(1, 3)
    }

    pub fn policy_for(&self, project_id: &str) -> Option<&ProjectPolicy> {
        self.allowed_projects
            .iter()
            .find(|policy| policy.project_id == project_id)
    }

    /// Channel-neutral Host notification gate. Priority kinds bypass quiet
    /// hours and rate budgets, but never a persistent pause. Channel switches
    /// such as `allowTelegram` are enforced by their respective delivery path.
    pub fn notification_allowed(&self, kind: &str, now_ms: u64) -> bool {
        if self.paused_until_user_resumes {
            return false;
        }
        if matches!(
            kind,
            "task.waiting_user"
                | "task.failed"
                | "task.interrupted"
                | "commitment_due"
                | "permission"
                | "question"
                | "plan"
        ) {
            return true;
        }
        if self.quiet_hours.is_empty() {
            return true;
        }
        let Some((start, end)) = self
            .quiet_hours
            .split_once('-')
            .and_then(|(start, end)| Some((hhmm_minutes(start)?, hhmm_minutes(end)?)))
        else {
            return true;
        };
        let Some(now) = Local.timestamp_millis_opt(now_ms as i64).single() else {
            return true;
        };
        let minute = now.hour() * 60 + now.minute();
        let quiet = if start <= end {
            minute >= start && minute < end
        } else {
            minute >= start || minute < end
        };
        !quiet
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StaffProfile {
    pub staff_id: String,
    pub name: String,
    pub title: String,
    pub persona: String,
    pub agent_id: String,
    pub model: Option<String>,
    /// 持久 ACP 会话；注册表内必须唯一。
    pub session_id: String,
    pub telegram_chat_id: Option<i64>,
    pub memory_root: String,
    pub commitment_ledger: String,
    /// Version of the product bootstrap prompt already injected into the session.
    pub bootstrap_version: u32,
    /// Durable ordinary-notification rate accounting. Priority attention events
    /// do not consume these fields.
    pub notification_window_started_at: u64,
    pub notifications_in_window: u32,
    pub last_notification_at: u64,
    pub created_at: u64,
    pub updated_at: u64,
    /// 扁平展开到档案 JSON；代码里使用 `profile.policy` 访问。
    #[serde(flatten)]
    pub policy: StaffPolicy,
}

impl Default for StaffProfile {
    fn default() -> Self {
        let now = now_ms();
        Self {
            staff_id: short_id("staff"),
            name: "员工".to_string(),
            title: "员工".to_string(),
            persona: String::new(),
            agent_id: "codex".to_string(),
            model: None,
            session_id: String::new(),
            telegram_chat_id: None,
            memory_root: String::new(),
            commitment_ledger: "commitments.json".to_string(),
            bootstrap_version: 0,
            notification_window_started_at: 0,
            notifications_in_window: 0,
            last_notification_at: 0,
            created_at: now,
            updated_at: now,
            policy: StaffPolicy::default(),
        }
    }
}

impl StaffProfile {
    /// First-run example assistant. It is a real persistent staff profile, but
    /// deliberately starts without project access and defers its bootstrap
    /// prompt until the user first opens the conversation.
    pub fn starter(agent_id: &str) -> Self {
        let mut profile = Self::default();
        profile.name = "Mari".to_string();
        profile.title = crate::ts!("你的助手", "Your assistant");
        profile.persona = crate::ts!(
            "你是 Mari，用户的本地助手。先理解目标，再决定自己处理还是请项目里的 agent 执行。\
             汇报要短，结论尽量带文件、测试或 diff 证据。只在答案会改变结果时提问。\
             没有用户任务或已批准的事时保持待机，不制造工作。",
            "You are Mari, the user's local assistant. Understand the goal first, then decide \
             whether to handle it yourself or delegate to an agent working in a project. Keep \
             reports short and ground conclusions in files, tests, or diffs when possible. Ask \
             only when the answer changes what you should do. Stay idle when there is no user task \
             or approved follow-up; do not invent work."
        );
        profile.agent_id = agent_id.to_string();
        profile.bootstrap_version = DEFERRED_BOOTSTRAP_VERSION;
        profile.policy.allowed_projects.clear();
        profile.policy.autonomy = StaffAutonomy::Assisted;
        profile.policy.max_active_tasks = 1;
        profile.policy.allow_background_work = true;
        profile.policy.allow_telegram = false;
        profile.policy.paused_until_user_resumes = false;
        profile.normalized()
    }
}

impl StaffProfile {
    pub fn normalized(mut self) -> Self {
        self.staff_id = self.staff_id.trim().to_string();
        self.name = self.name.trim().to_string();
        self.title = self.title.trim().to_string();
        self.agent_id = self.agent_id.trim().to_string();
        self.model = self
            .model
            .map(|model| model.trim().to_string())
            .filter(|m| !m.is_empty());
        self.session_id = self.session_id.trim().to_string();
        self.memory_root = self.memory_root.trim().to_string();
        self.commitment_ledger = self.commitment_ledger.trim().to_string();
        self.policy = self.policy.normalized();
        if self.name.is_empty() {
            self.name = "员工".to_string();
        }
        if self.title.is_empty() {
            self.title = "员工".to_string();
        }
        if self.agent_id.is_empty() {
            self.agent_id = "codex".to_string();
        }
        if self.memory_root.is_empty() {
            self.memory_root = format!("memory/{}", self.staff_id);
        }
        if self.commitment_ledger.is_empty() {
            self.commitment_ledger = "commitments.json".to_string();
        }
        if self.updated_at == 0 {
            self.updated_at = now_ms();
        }
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_id("staffId", &self.staff_id)?;
        validate_id("sessionId", &self.session_id)?;
        if self.name.is_empty() {
            return Err("员工 name 不能为空".to_string());
        }
        if self.agent_id.is_empty() {
            return Err(format!("员工 {} 的 agentId 不能为空", self.staff_id));
        }
        for (label, value) in [
            ("name", self.name.as_str()),
            ("title", self.title.as_str()),
            ("persona", self.persona.as_str()),
            ("memoryRoot", self.memory_root.as_str()),
            ("commitmentLedger", self.commitment_ledger.as_str()),
        ] {
            if value.len() > MAX_STAFF_FIELD_BYTES {
                return Err(format!("员工 {} 的 {label} 超过大小上限", self.staff_id));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StaffRegistry {
    pub version: u32,
    /// 0 = legacy single-Chief migration has not run.
    pub legacy_migration_completed_at: u64,
    /// Set once after the starter assistant is seeded (or an existing roster
    /// proves this install needs no starter). Deletion must not reseed it.
    pub starter_seeded_at: u64,
    /// Host-wide emergency stop; new staff inherit it immediately.
    pub paused_until_user_resumes: bool,
    pub staff: Vec<StaffProfile>,
}

impl Default for StaffRegistry {
    fn default() -> Self {
        Self {
            version: STAFF_REGISTRY_VERSION,
            legacy_migration_completed_at: 0,
            starter_seeded_at: 0,
            paused_until_user_resumes: false,
            staff: Vec::new(),
        }
    }
}

// 注册表 CRUD 尚未全部接线；接上消费点后删除本属性（惯例同 credentials.rs）。
#[allow(dead_code)]
impl StaffRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn normalized(mut self) -> Self {
        self.version = STAFF_REGISTRY_VERSION;
        self.staff = self
            .staff
            .into_iter()
            .take(MAX_STAFF)
            .map(StaffProfile::normalized)
            .collect();
        if self.paused_until_user_resumes {
            for profile in &mut self.staff {
                profile.policy.paused_until_user_resumes = true;
            }
        }
        if !self.staff.is_empty()
            && matches!(
                self.legacy_migration_completed_at,
                0 | CORRUPT_REGISTRY_MARKER
            )
        {
            self.legacy_migration_completed_at = now_ms();
        }
        self
    }

    /// 空注册表合法；一旦有条目，staffId 和 sessionId 都必须唯一。
    pub fn validate(&self) -> Result<(), String> {
        if self.staff.len() > MAX_STAFF {
            return Err(format!("员工数量超过上限 {MAX_STAFF}"));
        }
        let mut staff_ids = std::collections::HashSet::new();
        let mut session_ids = std::collections::HashSet::new();
        let mut telegram_chat_ids = std::collections::HashSet::new();
        for profile in &self.staff {
            profile.validate()?;
            if !staff_ids.insert(profile.staff_id.as_str()) {
                return Err(format!("重复的 staffId: {}", profile.staff_id));
            }
            if !session_ids.insert(profile.session_id.as_str()) {
                return Err(format!("重复的 sessionId: {}", profile.session_id));
            }
            if let Some(chat_id) = profile.telegram_chat_id {
                if !telegram_chat_ids.insert(chat_id) {
                    return Err(format!("重复的 telegramChatId: {chat_id}"));
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, staff_id: &str) -> Option<&StaffProfile> {
        self.staff
            .iter()
            .find(|profile| profile.staff_id == staff_id)
    }

    pub fn get_mut(&mut self, staff_id: &str) -> Option<&mut StaffProfile> {
        self.staff
            .iter_mut()
            .find(|profile| profile.staff_id == staff_id)
    }

    pub fn find_by_session_id(&self, session_id: &str) -> Option<&StaffProfile> {
        self.staff
            .iter()
            .find(|profile| profile.session_id == session_id)
    }

    pub fn is_staff_session(&self, session_id: &str) -> bool {
        self.find_by_session_id(session_id).is_some()
    }

    pub fn add(&mut self, profile: StaffProfile) -> Result<(), String> {
        let profile = profile.normalized();
        if self.get(&profile.staff_id).is_some() {
            return Err(format!("staffId 已存在: {}", profile.staff_id));
        }
        if self.find_by_session_id(&profile.session_id).is_some() {
            return Err(format!("sessionId 已绑定其他员工: {}", profile.session_id));
        }
        let mut next = self.clone();
        next.staff.push(profile);
        next.validate()?;
        *self = next.normalized();
        Ok(())
    }

    pub fn upsert(&mut self, profile: StaffProfile) -> Result<(), String> {
        let profile = profile.normalized();
        let mut next = self.clone();
        if let Some(index) = next
            .staff
            .iter()
            .position(|current| current.staff_id == profile.staff_id)
        {
            next.staff[index] = profile;
        } else {
            next.staff.push(profile);
        }
        next.validate()?;
        *self = next.normalized();
        Ok(())
    }

    pub fn update(&mut self, profile: StaffProfile) -> Result<(), String> {
        let profile = profile.normalized();
        let Some(index) = self
            .staff
            .iter()
            .position(|current| current.staff_id == profile.staff_id)
        else {
            return Err(format!("员工不存在: {}", profile.staff_id));
        };
        let mut next = self.clone();
        next.staff[index] = profile;
        next.validate()?;
        *self = next.normalized();
        Ok(())
    }

    pub fn remove(&mut self, staff_id: &str) -> Option<StaffProfile> {
        let index = self
            .staff
            .iter()
            .position(|profile| profile.staff_id == staff_id)?;
        Some(self.staff.remove(index))
    }

    pub fn staff_ids(&self) -> impl Iterator<Item = &str> {
        self.staff.iter().map(|profile| profile.staff_id.as_str())
    }

    pub fn is_corrupt(&self) -> bool {
        self.legacy_migration_completed_at == CORRUPT_REGISTRY_MARKER
    }

    /// 旧版只有单个 `config.chiefSessionId`。只创建一个稳定员工；已有
    /// 注册表时该函数返回注册表本身，调用方不应在启动恢复中重复迁移。
    pub fn from_legacy_chief_session(session_id: Option<&str>) -> Self {
        let Some(session_id) = session_id.map(str::trim).filter(|id| !id.is_empty()) else {
            return Self::default();
        };
        let staff_id = legacy_staff_id(session_id);
        let profile = StaffProfile {
            staff_id,
            name: "总管".to_string(),
            title: "总管".to_string(),
            persona: String::new(),
            agent_id: "codex".to_string(),
            model: None,
            session_id: session_id.to_string(),
            telegram_chat_id: None,
            memory_root: String::new(),
            commitment_ledger: "commitments.json".to_string(),
            bootstrap_version: 0,
            notification_window_started_at: 0,
            notifications_in_window: 0,
            last_notification_at: 0,
            created_at: now_ms(),
            updated_at: now_ms(),
            policy: StaffPolicy::default(),
        }
        .normalized();
        Self {
            version: STAFF_REGISTRY_VERSION,
            legacy_migration_completed_at: now_ms(),
            starter_seeded_at: 0,
            paused_until_user_resumes: false,
            staff: vec![profile],
        }
    }

    pub fn from_legacy_config(config: &ChiefConfig) -> Self {
        let mut registry = Self::from_legacy_chief_session(config.chief_session_id.as_deref());
        if let Some(profile) = registry.staff.first_mut() {
            if let Some(agent_id) = config.chief_agent_id.as_deref() {
                profile.agent_id = agent_id.to_string();
            }
            profile.model = config.chief_model.clone();
            profile.policy.allowed_projects = config
                .allowed_projects
                .iter()
                .filter(|project| project.enabled)
                .cloned()
                .collect();
            profile.policy.max_active_tasks = config.max_concurrent_workers.clamp(1, 3);
            // 旧 Chief 的 Telegram 任务卡是既有行为，迁移时不能无声关掉。
            profile.policy.allow_telegram = true;
            profile.updated_at = now_ms();
        }
        registry.legacy_migration_completed_at = now_ms();
        registry
    }

    pub fn load() -> Self {
        load()
    }

    pub fn save(&self) -> Result<(), String> {
        save(self)
    }
}

fn normalize_project_policy(mut policy: ProjectPolicy) -> Option<ProjectPolicy> {
    policy.project_id = policy.project_id.trim().to_string();
    if policy.project_id.is_empty() {
        return None;
    }
    policy.allowed_agents = policy
        .allowed_agents
        .into_iter()
        .map(|agent| agent.trim().to_string())
        .filter(|agent| !agent.is_empty())
        .take(64)
        .collect();
    policy.default_agent_id = policy
        .default_agent_id
        .map(|agent| agent.trim().to_string())
        .filter(|agent| !agent.is_empty());
    policy.default_model = policy
        .default_model
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty());
    policy.max_concurrent_workers = policy.max_concurrent_workers.clamp(1, 3);
    policy.max_task_minutes = policy.max_task_minutes.clamp(1, 480);
    Some(policy)
}

fn dedupe_project_policies(values: &mut Vec<ProjectPolicy>) {
    let mut seen = std::collections::HashSet::new();
    values.retain(|value| seen.insert(value.project_id.clone()));
}

fn validate_id(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.len() > 160
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(format!("非法 {label}: {value:?}"));
    }
    Ok(())
}

fn valid_quiet_hours(value: &str) -> bool {
    let Some((start, end)) = value.split_once('-') else {
        return false;
    };
    valid_hhmm(start) && valid_hhmm(end)
}

fn valid_hhmm(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.chars().all(|c| c.is_ascii_digit())
        && minute.chars().all(|c| c.is_ascii_digit())
        && hour.parse::<u8>().is_ok_and(|h| h < 24)
        && minute.parse::<u8>().is_ok_and(|m| m < 60)
}

fn hhmm_minutes(value: &str) -> Option<u32> {
    let (hour, minute) = value.split_once(':')?;
    let hour = hour.parse::<u32>().ok()?;
    let minute = minute.parse::<u32>().ok()?;
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

fn legacy_staff_id(session_id: &str) -> String {
    let suffix: String = session_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(64)
        .collect();
    if suffix.is_empty() {
        short_id("staff_legacy")
    } else {
        format!("staff_legacy_{suffix}")
    }
}

fn decode_registry(raw: &str) -> Result<StaffRegistry, String> {
    let value: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let registry = if value.is_array() {
        let staff = serde_json::from_value::<Vec<StaffProfile>>(value)
            .map_err(|error| error.to_string())?;
        StaffRegistry {
            version: STAFF_REGISTRY_VERSION,
            legacy_migration_completed_at: 0,
            starter_seeded_at: 0,
            paused_until_user_resumes: false,
            staff,
        }
    } else {
        serde_json::from_value::<StaffRegistry>(value).map_err(|error| error.to_string())?
    };
    registry.validate()?;
    Ok(registry.normalized())
}

fn load_unlocked() -> StaffRegistry {
    let corrupt = || StaffRegistry {
        version: STAFF_REGISTRY_VERSION,
        legacy_migration_completed_at: CORRUPT_REGISTRY_MARKER,
        starter_seeded_at: CORRUPT_REGISTRY_MARKER,
        paused_until_user_resumes: false,
        staff: Vec::new(),
    };
    let path = match staff_path() {
        Ok(path) => path,
        Err(_) => return StaffRegistry::default(),
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return StaffRegistry::default()
        }
        Err(error) => {
            warn(
                "staff.json read failed — control surface disabled until repaired",
                Some(&error.to_string()),
            );
            return corrupt();
        }
    };
    match decode_registry(&raw) {
        Ok(registry) => registry,
        Err(error) => {
            warn(
                "staff.json unreadable — preserving file and disabling control surface",
                Some(&error),
            );
            corrupt()
        }
    }
}

/// fail-open 读取；注册表损坏不应阻断会话启动。
pub fn load() -> StaffRegistry {
    let _guard = REGISTRY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    load_unlocked()
}

fn save_unlocked(registry: &StaffRegistry) -> Result<(), String> {
    let registry = registry.clone().normalized();
    registry.validate()?;
    let path = staff_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("chief staff: mkdir failed: {error}"))?;
    }
    let body = serde_json::to_string_pretty(&registry).map_err(|error| error.to_string())?;
    crate::storage::write_atomic(&path, &body)
}

pub fn save(registry: &StaffRegistry) -> Result<(), String> {
    let _guard = REGISTRY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    save_unlocked(registry)
}

/// 原子读-改-写；CRUD 操作优先走这里，避免两个窗口互相覆盖。
pub fn with_registry<T>(
    operation: impl FnOnce(&mut StaffRegistry) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = REGISTRY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut registry = load_unlocked();
    let result = operation(&mut registry)?;
    save_unlocked(&registry)?;
    Ok(result)
}

#[allow(dead_code)]
pub fn load_registry() -> StaffRegistry {
    load()
}

#[allow(dead_code)]
pub fn save_registry(registry: &StaffRegistry) -> Result<(), String> {
    save(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chief::store::SANDBOX_LOCK;
    use std::path::PathBuf;
    use std::sync::MutexGuard;

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("marionette-chief-staff-{name}-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lock() -> MutexGuard<'static, ()> {
        SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn profile(staff_id: &str, session_id: &str) -> StaffProfile {
        StaffProfile {
            staff_id: staff_id.into(),
            session_id: session_id.into(),
            ..Default::default()
        }
    }

    #[test]
    fn policy_defaults_and_clamps_are_safe() {
        let policy = StaffPolicy {
            max_active_tasks: 99,
            max_turns_per_task: 0,
            max_wall_minutes_per_task: u32::MAX,
            max_cost_per_task_usd: f64::NAN,
            max_reports_per_hour: 999,
            quiet_hours: "not-a-range".into(),
            ..Default::default()
        }
        .normalized();
        assert_eq!(policy.max_active_tasks(), 3);
        assert_eq!(policy.max_turns_per_task, 1);
        assert_eq!(policy.max_wall_minutes_per_task, 10_080);
        assert_eq!(policy.max_reports_per_hour, 120);
        assert_eq!(policy.max_cost_per_task_usd, 0.0);
        assert_eq!(policy.quiet_hours, "23:00-08:00");
    }

    #[test]
    fn telegram_switch_does_not_disable_channel_neutral_notifications() {
        let policy = StaffPolicy {
            allow_telegram: false,
            quiet_hours: String::new(),
            ..Default::default()
        };
        assert!(policy.notification_allowed("completed", now_ms()));
        let paused = StaffPolicy {
            paused_until_user_resumes: true,
            ..policy
        };
        assert!(!paused.notification_allowed("completed", now_ms()));
    }

    #[test]
    fn registry_roundtrip_is_camel_case_and_crud_works() {
        let _guard = lock();
        let root = sandbox("roundtrip");
        set_root_override_for_tests(Some(root.clone()));
        let mut registry = StaffRegistry::new();
        registry.starter_seeded_at = 1234;
        registry.add(profile("staff_a", "session_a")).unwrap();
        assert!(registry.add(profile("staff_a", "session_b")).is_err());
        assert!(registry.add(profile("staff_b", "session_a")).is_err());
        save(&registry).unwrap();
        let loaded = load();
        assert_eq!(loaded.staff.len(), 1);
        assert_eq!(loaded.starter_seeded_at, 1234);
        assert!(loaded.is_staff_session("session_a"));
        let json = std::fs::read_to_string(staff_path().unwrap()).unwrap();
        assert!(json.contains("\"staffId\""));
        assert!(json.contains("\"allowedProjects\""));
        assert!(!json.contains("\"policy\""));
        // 坏档 fail-open。
        std::fs::write(staff_path().unwrap(), "{broken").unwrap();
        let corrupt = load();
        assert!(corrupt.staff.is_empty());
        assert_eq!(
            corrupt.legacy_migration_completed_at,
            CORRUPT_REGISTRY_MARKER
        );
        assert!(corrupt.is_corrupt());
        set_root_override_for_tests(None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn starter_profile_is_ready_to_chat_without_extra_setup() {
        let profile = StaffProfile::starter("opencode");
        assert_eq!(profile.name, "Mari");
        assert_eq!(profile.agent_id, "opencode");
        assert!(profile.model.is_none());
        assert_eq!(profile.bootstrap_version, DEFERRED_BOOTSTRAP_VERSION);
        assert!(profile.policy.allowed_projects.is_empty());
        assert_eq!(profile.policy.autonomy, StaffAutonomy::Assisted);
        assert_eq!(profile.policy.max_active_tasks, 1);
        assert!(profile.policy.allow_background_work);
        assert!(!profile.policy.allow_telegram);
        assert!(!profile.policy.paused_until_user_resumes);
        assert!(profile.session_id.is_empty());
    }

    #[test]
    fn legacy_session_migrates_once_to_a_profile() {
        let registry = StaffRegistry::from_legacy_chief_session(Some("session_chief_1"));
        assert_eq!(registry.staff.len(), 1);
        assert_eq!(registry.staff[0].session_id, "session_chief_1");
        assert!(registry.staff[0].staff_id.starts_with("staff_legacy_"));
        assert!(registry.validate().is_ok());
        assert!(StaffRegistry::from_legacy_chief_session(None)
            .staff
            .is_empty());
    }

    #[test]
    fn deleting_the_last_staff_keeps_the_migration_barrier() {
        let _guard = lock();
        let root = sandbox("last-staff");
        set_root_override_for_tests(Some(root.clone()));
        let mut registry = StaffRegistry::from_legacy_chief_session(Some("session_old_chief"));
        registry.remove("staff_legacy_sessionoldchief");
        save(&registry).unwrap();
        let loaded = load();
        assert!(loaded.staff.is_empty());
        assert!(loaded.legacy_migration_completed_at > 0);
        set_root_override_for_tests(None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn global_pause_is_inherited_by_people_hired_after_the_stop() {
        let _guard = lock();
        let root = sandbox("global-pause");
        set_root_override_for_tests(Some(root.clone()));
        let mut registry = StaffRegistry {
            paused_until_user_resumes: true,
            ..Default::default()
        };
        registry.add(profile("staff_new", "session_new")).unwrap();
        save(&registry).unwrap();
        let loaded = load();
        assert!(loaded.paused_until_user_resumes);
        assert!(
            loaded
                .get("staff_new")
                .unwrap()
                .policy
                .paused_until_user_resumes
        );
        set_root_override_for_tests(None);
        let _ = std::fs::remove_dir_all(root);
    }
}
