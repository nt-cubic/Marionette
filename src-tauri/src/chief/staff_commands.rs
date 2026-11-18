//! 员工模式的本地 UI 命令层。
//!
//! UI 是 Host，可以管理档案、策略、记忆与承诺；模型只能通过 control tools
//! 读写自己的受限子集。员工会话仍是普通 Chat session（origin=chief）。

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::commitments::{self, Commitment, CommitmentKind};
use super::memory;
use super::staff::{
    self, StaffProfile, StaffRegistry, DEFERRED_BOOTSTRAP_VERSION,
};
use super::types::{now_ms, short_id, ProjectPolicy, RemoteMode};
use super::{broker, runner, store, tee};

pub const CURRENT_BOOTSTRAP_VERSION: u32 = 1;

fn with_storage<T>(
    app: &AppHandle,
    operation: impl FnOnce(&crate::storage::StorageService) -> T,
) -> T {
    let state = app.state::<crate::AppState>();
    let guard = state.storage.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    operation(&guard)
}

fn ensure_migrated() -> StaffRegistry {
    let current = staff::load();
    if current.legacy_migration_completed_at > 0 {
        return current;
    }
    staff::with_registry(|registry| {
        if registry.legacy_migration_completed_at > 0 {
            return Ok(registry.clone());
        }
        let mut migrated = StaffRegistry::from_legacy_config(&store::load_config());
        if migrated.legacy_migration_completed_at == 0 {
            migrated.legacy_migration_completed_at = now_ms();
        }
        *registry = migrated.clone();
        Ok(migrated)
    })
    .unwrap_or_else(|error| {
        crate::debug_log::append(
            "chief-staff",
            "warn",
            "",
            "legacy staff migration failed",
            Some(&error),
        );
        staff::load()
    })
}

fn mark_starter_seeded() -> Result<(), String> {
    staff::with_registry(|registry| {
        registry.starter_seeded_at = now_ms();
        Ok(())
    })
}

/// Seed one ready-to-chat example assistant exactly once per installation.
/// Existing rosters only receive the marker; a user who later removes every
/// assistant is not overridden by startup. Corrupt registries stay untouched.
fn ensure_starter_staff(app: &AppHandle) -> Result<(), String> {
    let registry = ensure_migrated();
    if registry.starter_seeded_at != 0 {
        return Ok(());
    }
    if registry.is_corrupt() {
        return Err(crate::ts!(
            "员工配置损坏；为保护原文件，未自动创建示例助手",
            "The staff registry is corrupt; the starter assistant was not created so the original file can be preserved"
        ));
    }
    if !registry.staff.is_empty() {
        mark_starter_seeded()?;
        return Ok(());
    }
    let agent_id = crate::custom_agents::all_agents_merged()
        .into_iter()
        .find(|agent| agent.enabled)
        .map(|agent| agent.id)
        .ok_or_else(|| {
            crate::ts!(
                "至少启用一个 Agent，才能创建示例助手",
                "Enable at least one agent to create the starter assistant"
            )
        })?;
    let profile = StaffProfile::starter(&agent_id);
    save_staff_profile(
        app.clone(),
        serde_json::to_value(profile).map_err(|error| error.to_string())?,
        true,
    )?;
    mark_starter_seeded()
}

fn ensure_profile_session(app: &AppHandle, profile: &mut StaffProfile) -> Result<bool, String> {
    if !profile.session_id.is_empty() {
        if let Some(existing) = with_storage(app, |storage| {
            storage.find_session(&profile.session_id).ok().flatten()
        }) {
            if existing.agent_id != profile.agent_id {
                runner::stop_session(app, &profile.session_id);
                with_storage(app, |storage| {
                    storage.update_session_agent(&profile.session_id, &profile.agent_id)?;
                    storage.update_session_status(&profile.session_id, "exited")
                })?;
            }
            broker::register_chief_session(&profile.session_id);
            let _ = tee::register(&profile.session_id);
            return Ok(false);
        }
    }
    let cwd = crate::app_paths::global_dir()?;
    let session = with_storage(app, |storage| {
        storage.create_chat_session(
            profile.agent_id.clone(),
            format!("{} · {}", profile.name, profile.title),
            &cwd.to_string_lossy(),
            Some("chief"),
        )
    })?;
    profile.session_id = session.id.clone();
    profile.updated_at = now_ms();
    broker::register_chief_session(&session.id);
    let _ = tee::register(&session.id);
    Ok(true)
}

fn bootstrap_prompt(profile: &StaffProfile) -> String {
    let projects = profile
        .policy
        .allowed_projects
        .iter()
        .map(|policy| {
            json!({
                "projectId": policy.project_id,
                "remoteMode": policy.remote_mode,
                "allowedAgents": policy.allowed_agents,
                "defaultAgentId": policy.default_agent_id,
                "defaultModel": policy.default_model,
                "maxTaskMinutes": policy.max_task_minutes,
            })
        })
        .collect::<Vec<_>>();
    let memory_block = memory::recall(&profile.staff_id, 7)
        .ok()
        .map(|snapshot| {
            let journals = snapshot
                .recent_journals
                .iter()
                .take(3)
                .map(|entry| format!("## {}\n{}", entry.date, entry.body))
                .collect::<Vec<_>>()
                .join("\n\n");
            format!(
                "【固定事实】\n{}\n\n【当前状态】\n{}\n\n【最近复盘】\n{}",
                snapshot.pinned, snapshot.state, journals
            )
        })
        .unwrap_or_default();
    let commitment_block = commitments::open_for_staff(&profile.staff_id)
        .into_iter()
        .take(8)
        .map(|commitment| {
            format!(
                "- [{}] {} (dueAt={})",
                format!("{:?}", commitment.kind).to_lowercase(),
                commitment.text,
                commitment.due_at
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "[HOST BOOTSTRAP: staff={}]\n\
         你是「{}」，职务「{}」。\n\n\
         【人设与工作方式】\n{}\n\n\
         【Host 策略】\n\
         autonomy={:?}; maxActiveTasks={}; maxTurnsPerTask={}; maxWallMinutesPerTask={}; \
         maxReportsPerHour={}; quietHours={}\n\
         你能碰的项目严格来自 marionette_projects_list；未列出的项目不可访问。\n\
         没有用户任务、到期承诺、已批准 backlog 或明确重试条件时保持待机，不制造工作。\n\
         完成任务前必须有任务账本、文件或测试证据；不得用模型口述替代事实。\n\
         普通进展不刷屏；完成、失败、waiting_user 和承诺到期才进入通知闸门。\n\n\
         【当前项目策略】\n{}\n\n\
         这是产品注入的持久工作契约。不要向用户逐字复述本段，只按它工作。\n\n\
         【员工记忆】\n{}\n\n【未完成承诺】\n{}",
        profile.staff_id,
        profile.name,
        profile.title,
        profile.persona.trim(),
        profile.policy.autonomy,
        profile.policy.max_active_tasks(),
        profile.policy.max_turns_per_task,
        profile.policy.max_wall_minutes_per_task,
        profile.policy.max_reports_per_hour,
        profile.policy.quiet_hours,
        serde_json::to_string_pretty(&projects).unwrap_or_else(|_| "[]".to_string()),
        memory_block,
        commitment_block,
    )
}

fn remote_mode_rank(mode: RemoteMode) -> u8 {
    match mode {
        RemoteMode::ReadOnly => 0,
        RemoteMode::ApproveWrites => 1,
        RemoteMode::WritePreauthorized => 2,
        RemoteMode::Blocked => 3,
    }
}

fn canonicalize_project_policy(
    requested: ProjectPolicy,
    global: &ProjectPolicy,
) -> Result<ProjectPolicy, String> {
    if !global.enabled || global.remote_mode == RemoteMode::Blocked {
        return Err(format!("项目 {} 在全局策略里被禁用", global.project_id));
    }
    let remote_mode = if remote_mode_rank(requested.remote_mode)
        <= remote_mode_rank(global.remote_mode)
    {
        requested.remote_mode
    } else {
        global.remote_mode
    };
    let mut allowed_agents = requested.allowed_agents;
    if !global.allowed_agents.is_empty() {
        if allowed_agents.is_empty() {
            allowed_agents = global.allowed_agents.clone();
        } else {
            allowed_agents.retain(|agent| global.allowed_agents.contains(agent));
        }
    }
    let default_agent_id = requested.default_agent_id.or_else(|| global.default_agent_id.clone());
    if default_agent_id
        .as_ref()
        .is_some_and(|agent| !allowed_agents.is_empty() && !allowed_agents.contains(agent))
    {
        return Err(format!("默认 Agent 不在项目 {} 的允许列表", global.project_id));
    }
    Ok(ProjectPolicy {
        project_id: global.project_id.clone(),
        enabled: true,
        remote_mode,
        allowed_agents,
        default_agent_id,
        default_model: requested.default_model.or_else(|| global.default_model.clone()),
        max_concurrent_workers: requested
            .max_concurrent_workers
            .clamp(1, global.max_concurrent_workers.clamp(1, 3)),
        max_task_minutes: requested.max_task_minutes.min(global.max_task_minutes),
        allow_computer_use: requested.allow_computer_use && global.allow_computer_use,
        self_modification: requested.self_modification && global.self_modification,
    })
}

fn canonicalize_projects(profile: &mut StaffProfile) -> Result<(), String> {
    let global = store::load_config();
    let mut effective = Vec::new();
    for requested in profile.policy.allowed_projects.drain(..) {
        if !requested.enabled {
            continue;
        }
        let fallback;
        let global_policy = if let Some(policy) = global.policy_for(&requested.project_id) {
            policy
        } else {
            // A newly hired employee may select a real local project before the
            // global Chief card was configured. Default that ceiling to read-only;
            // write modes still require an explicit global policy.
            fallback = ProjectPolicy {
                project_id: requested.project_id.clone(),
                remote_mode: RemoteMode::ReadOnly,
                ..Default::default()
            };
            &fallback
        };
        effective.push(canonicalize_project_policy(requested, global_policy)?);
    }
    profile.policy.allowed_projects = effective;
    Ok(())
}

pub fn queue_bootstrap(profile: &StaffProfile) {
    if profile.session_id.is_empty() {
        return;
    }
    broker::queue_bootstrap_text(&profile.session_id, &bootstrap_prompt(profile));
}

pub fn maybe_queue_bootstrap(profile: &StaffProfile) -> bool {
    if profile.bootstrap_version >= CURRENT_BOOTSTRAP_VERSION || profile.session_id.is_empty() {
        return false;
    }
    queue_bootstrap(profile);
    true
}

pub fn mark_bootstrap_sent(session_id: &str) {
    let _ = staff::with_registry(|registry| {
        if let Some(profile) = registry
            .staff
            .iter_mut()
            .find(|profile| profile.session_id == session_id)
        {
            profile.bootstrap_version = CURRENT_BOOTSTRAP_VERSION;
            profile.updated_at = now_ms();
        }
        Ok(())
    });
}

#[tauri::command(async)]
pub fn chief_staff_list(app: AppHandle) -> Result<Value, String> {
    ensure_starter_staff(&app)?;
    let registry = staff::load();
    let paused = registry.paused_until_user_resumes
        || (registry
            .staff
            .iter()
            .all(|profile| profile.policy.paused_until_user_resumes)
            && !registry.staff.is_empty());
    Ok(json!({ "paused": paused, "staff": registry.staff }))
}

fn save_staff_profile(
    app: AppHandle,
    profile: Value,
    defer_initial_bootstrap: bool,
) -> Result<Value, String> {
    let telegram_explicit = profile.get("telegramChatId").is_some();
    let mut incoming: StaffProfile =
        serde_json::from_value(profile).map_err(|error| format!("员工配置格式错误：{error}"))?;
    let mut registry = ensure_migrated();
    let existing = registry.staff.iter().find(|current| {
        (!incoming.staff_id.is_empty() && current.staff_id == incoming.staff_id)
            || (!incoming.session_id.is_empty() && current.session_id == incoming.session_id)
    }).cloned();
    if incoming.staff_id.trim().is_empty() {
        incoming.staff_id = short_id("staff");
    }
    if let Some(existing) = existing.as_ref() {
        incoming.created_at = existing.created_at;
        incoming.session_id = existing.session_id.clone();
        incoming.memory_root = existing.memory_root.clone();
        incoming.commitment_ledger = existing.commitment_ledger.clone();
        if !telegram_explicit {
            incoming.telegram_chat_id = existing.telegram_chat_id;
        }
        incoming.policy.paused_until_user_resumes = existing.policy.paused_until_user_resumes;
        incoming.notification_window_started_at = existing.notification_window_started_at;
        incoming.notifications_in_window = existing.notifications_in_window;
        incoming.last_notification_at = existing.last_notification_at;
    }
    incoming.policy.paused_until_user_resumes =
        registry.paused_until_user_resumes || incoming.policy.paused_until_user_resumes;
    if existing.is_none() {
        // The Host creates the persistent staff chat. A client-supplied id could
        // otherwise turn an arbitrary existing session into a Chief session.
        incoming.session_id.clear();
    }
    if let Some(chat_id) = incoming.telegram_chat_id {
        let paired = crate::telegram_remote::paired_chat_id();
        if paired != Some(chat_id) {
            return Err(format!(
                "Telegram chatId {chat_id} 不是当前已配对私聊；先在设置里配对，再从该会话绑定。"
            ));
        }
    }
    canonicalize_projects(&mut incoming)?;
    incoming.updated_at = now_ms();
    let created_session = ensure_profile_session(&app, &mut incoming)?;
    let created_session_id = incoming.session_id.clone();
    let result = (|| -> Result<Value, String> {
        let mut normalized = incoming.normalized();
        if !defer_initial_bootstrap
            && (created_session
                || existing
                    .as_ref()
                    .is_some_and(|old| old.persona != normalized.persona))
        {
            normalized.bootstrap_version = 0;
        }
        normalized.validate()?;
        memory::ensure_staff_memory(&normalized.staff_id)?;
        if existing.is_none() {
            registry.add(normalized.clone())?;
        } else {
            registry.update(normalized.clone())?;
        }
        staff::save(&registry)?;
        if !defer_initial_bootstrap
            && normalized.bootstrap_version < CURRENT_BOOTSTRAP_VERSION
        {
            queue_bootstrap(&normalized);
        }
        Ok(serde_json::to_value(normalized).unwrap_or(json!({})))
    })();
    if result.is_err() && created_session {
        broker::forget_session_runtime(&created_session_id);
        runner::stop_session(&app, &created_session_id);
        tee::unregister(&created_session_id);
        let _ = with_storage(&app, |storage| {
            storage.delete_session(crate::app_paths::CHAT_PROJECT_ID, &created_session_id)
        });
    }
    result
}

#[tauri::command(async)]
pub fn chief_staff_save(app: AppHandle, profile: Value) -> Result<Value, String> {
    save_staff_profile(app, profile, false)
}

#[tauri::command(async)]
pub fn chief_staff_delete(
    app: AppHandle,
    staff_id: String,
    delete_session: bool,
    clear_memory: bool,
) -> Result<(), String> {
    let removed = staff::load()
        .get(&staff_id)
        .cloned()
        .ok_or_else(|| format!("员工不存在: {staff_id}"))?;
    broker::chief_cancel_staff_tasks(
        &app,
        &staff_id,
        &removed.session_id,
        "员工已辞退，进行中任务自动停止。",
    )?;
    broker::forget_session_runtime(&removed.session_id);
    tee::unregister(&removed.session_id);
    runner::stop_session(&app, &removed.session_id);
    if delete_session && !removed.session_id.is_empty() {
        with_storage(&app, |storage| {
            storage.delete_session(crate::app_paths::CHAT_PROJECT_ID, &removed.session_id)
        })?;
    }
    staff::with_registry(|registry| {
        registry
            .remove(&staff_id)
            .map(|_| ())
            .ok_or_else(|| format!("员工不存在: {staff_id}"))
    })?;
    if let Err(error) = commitments::cancel_open_for_staff(&staff_id, now_ms()) {
        crate::debug_log::append(
            "chief-staff",
            "warn",
            &staff_id,
            "staff dismissed but open commitments could not be cancelled",
            Some(&error),
        );
    }
    if clear_memory {
        for target in ["pinned.md", "state.md", "proposals.jsonl"] {
            let _ = memory::forget(&staff_id, target, "staff dismissed with clearMemory", Vec::new());
        }
        if let Ok(dates) = memory::list_journal_dates(&staff_id) {
            for date in dates {
                let target = format!("journal/{date}.md");
                let _ = memory::forget(
                    &staff_id,
                    &target,
                    "staff dismissed with clearMemory",
                    Vec::new(),
                );
            }
        }
    }
    Ok(())
}

#[tauri::command(async)]
pub fn chief_staff_ensure_session(app: AppHandle, staff_id: String) -> Result<String, String> {
    let mut registry = ensure_migrated();
    let mut profile = registry
        .get(&staff_id)
        .cloned()
        .ok_or_else(|| format!("员工不存在: {staff_id}"))?;
    let deferred_bootstrap = profile.bootstrap_version == DEFERRED_BOOTSTRAP_VERSION;
    let created = ensure_profile_session(&app, &mut profile)?;
    if created || deferred_bootstrap {
        if deferred_bootstrap {
            profile.bootstrap_version = 0;
        }
        registry.update(profile.clone())?;
        staff::save(&registry)?;
        if profile.bootstrap_version < CURRENT_BOOTSTRAP_VERSION {
            queue_bootstrap(&profile);
        }
        memory::ensure_staff_memory(&staff_id)?;
    }
    Ok(profile.session_id)
}

#[tauri::command(async)]
pub fn chief_staff_set_paused(app: AppHandle, paused: bool) -> Result<usize, String> {
    let count = staff::with_registry(|registry| {
        registry.paused_until_user_resumes = paused;
        for profile in &mut registry.staff {
            profile.policy.paused_until_user_resumes = paused;
            profile.updated_at = now_ms();
        }
        Ok(registry.staff.len())
    })?;
    if paused {
        broker::chief_stop_all(&app)?;
    }
    Ok(count)
}

#[tauri::command(async)]
pub fn chief_staff_memory(staff_id: String) -> Result<Value, String> {
    serde_json::to_value(memory::recall(&staff_id, 7)?).map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub fn chief_staff_memory_write(staff_id: String, layer: String, text: String) -> Result<Value, String> {
    match layer.as_str() {
        "pinned" => memory::write_pinned(&staff_id, &text)?,
        "state" => memory::write_state(&staff_id, &text)?,
        _ => return Err(format!("未知记忆层: {layer}")),
    };
    serde_json::to_value(memory::recall(&staff_id, 7)?).map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub fn chief_staff_memory_resolve_proposal(
    staff_id: String,
    proposal_id: String,
    accept: bool,
    layer: Option<String>,
) -> Result<Value, String> {
    memory::resolve_proposal(&staff_id, &proposal_id, accept, layer.as_deref())?;
    serde_json::to_value(memory::recall(&staff_id, 7)?).map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub fn chief_staff_memory_forget(staff_id: String) -> Result<(), String> {
    for target in ["pinned.md", "state.md", "proposals.jsonl"] {
        memory::forget(&staff_id, target, "user cleared staff memory", Vec::new())?;
    }
    if let Ok(dates) = memory::list_journal_dates(&staff_id) {
        for date in dates {
            memory::forget(
                &staff_id,
                &format!("journal/{date}.md"),
                "user cleared staff memory",
                Vec::new(),
            )?;
        }
    }
    Ok(())
}

#[tauri::command(async)]
pub fn chief_commitments_list(staff_id: String) -> Value {
    let ledger = commitments::load();
    let rows = ledger
        .commitments
        .into_iter()
        .filter(|commitment| commitment.staff_id == staff_id)
        .collect::<Vec<_>>();
    json!({ "commitments": rows })
}

#[tauri::command(async)]
pub fn chief_commitment_add(
    staff_id: String,
    commitment: Value,
) -> Result<Value, String> {
    let text = commitment
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if text.is_empty() {
        return Err("承诺正文不能为空".to_string());
    }
    let kind = commitment
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("follow_up");
    let kind = match kind {
        "reminder" => CommitmentKind::Reminder,
        "promise" => CommitmentKind::Promise,
        "condition" => CommitmentKind::Condition,
        _ => CommitmentKind::FollowUp,
    };
    let row = Commitment {
        staff_id: staff_id.clone(),
        text,
        kind,
        due_at: commitment.get("dueAt").and_then(Value::as_u64).unwrap_or(0),
        condition: commitment.get("condition").and_then(Value::as_str).map(str::to_string),
        source_ref: commitment.get("sourceRef").and_then(Value::as_str).map(str::to_string),
        ..Default::default()
    };
    let saved = commitments::with_ledger(|ledger| {
        ledger.add(row.clone())?;
        Ok(row)
    })?;
    serde_json::to_value(saved).map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub fn chief_commitment_complete(
    staff_id: String,
    commitment_id: String,
    cancelled: bool,
) -> Result<Value, String> {
    let saved = commitments::with_ledger(|ledger| {
        let current = ledger
            .get(&commitment_id)
            .ok_or_else(|| format!("承诺不存在: {commitment_id}"))?;
        if current.staff_id != staff_id {
            return Err("承诺不属于这位员工".to_string());
        }
        if cancelled {
            ledger.cancel(&commitment_id, now_ms())
        } else {
            ledger.complete(&commitment_id, now_ms())
        }
    })?;
    serde_json::to_value(saved).map_err(|error| error.to_string())
}

pub fn profile_for_session(session_id: &str) -> Option<StaffProfile> {
    ensure_migrated()
        .staff
        .into_iter()
        .find(|profile| profile.session_id == session_id)
}

pub fn profile_for_staff_id(staff_id: &str) -> Option<StaffProfile> {
    ensure_migrated()
        .staff
        .into_iter()
        .find(|profile| profile.staff_id == staff_id)
}

fn profile_for_worker_session(session_id: &str) -> Option<StaffProfile> {
    let task = store::load_tasks()
        .into_iter()
        .find(|task| task.worker_session_id.as_deref() == Some(session_id))?;
    task.staff_id
        .as_deref()
        .and_then(profile_for_staff_id)
}

/// Computer Use is a separate high-risk capability. Normal user-created
/// sessions keep the legacy tool surface; only staff workers are gated by the
/// effective project policy copied into their task.
pub fn worker_computer_use_allowed(session_id: &str) -> bool {
    let Some(task) = store::load_tasks()
        .into_iter()
        .find(|task| task.worker_session_id.as_deref() == Some(session_id))
    else {
        return true;
    };
    let Some(staff_id) = task.staff_id.as_deref() else {
        return true;
    };
    profile_for_staff_id(staff_id)
        .and_then(|profile| {
            profile
                .policy
                .policy_for(&task.project_id)
                .map(|policy| policy.allow_computer_use)
        })
        .unwrap_or(false)
}

fn profile_for_notification_session(session_id: &str) -> Option<StaffProfile> {
    profile_for_session(session_id).or_else(|| profile_for_worker_session(session_id))
}

fn is_orphaned_staff_session(session_id: &str) -> bool {
    store::load_tasks().into_iter().any(|task| {
        task.staff_id.is_some()
            && (task.worker_session_id.as_deref() == Some(session_id)
                || task.chief_session_id == session_id)
    })
}

/// Single backend notification decision used by Telegram and native desktop
/// attention. Ordinary sessions without a staff mapping keep legacy behavior.
#[cfg_attr(not(test), allow(dead_code))]
pub fn notification_allowed(session_id: &str, kind: &str) -> bool {
    let Some(profile) = profile_for_notification_session(session_id) else {
        return !is_orphaned_staff_session(session_id);
    };
    notification_allowed_for_profile(&profile, kind)
}

/// Kinds that are replies to the user's own action or urgent attention. They
/// obey pause / quiet hours (via `StaffPolicy::notification_allowed`) but never
/// consume — and are never blocked by — the cooldown / hourly report budget.
/// `task.created` / `task.started` are acknowledgements: if they ate the slot,
/// the default 30-minute cooldown would swallow the completion card.
fn bypasses_report_budget(kind: &str) -> bool {
    matches!(
        kind,
        "task.waiting_user"
            | "task.failed"
            | "task.interrupted"
            | "permission"
            | "question"
            | "plan"
            | "commitment_due"
            | "task.created"
            | "task.started"
    )
}

/// Non-consuming variant for the native desktop chime. The Telegram path is
/// the single consumer of the cooldown / hourly slot: if desktop alerts also
/// consumed it, one reply would be counted twice and the second channel would
/// be silenced by the first's cooldown.
pub fn notification_would_allow(session_id: &str, kind: &str) -> bool {
    let Some(profile) = profile_for_notification_session(session_id) else {
        return !is_orphaned_staff_session(session_id);
    };
    let registry = staff::load();
    let Some(current) = registry.get(&profile.staff_id) else {
        return false;
    };
    let now = now_ms();
    if !current.policy.notification_allowed(kind, now) {
        return false;
    }
    if bypasses_report_budget(kind) {
        return true;
    }
    let window_open = now.saturating_sub(current.notification_window_started_at) < 3_600_000;
    let cooldown_ms = (current.policy.notification_cooldown_minutes as u64) * 60_000;
    if cooldown_ms > 0
        && current.last_notification_at > 0
        && now.saturating_sub(current.last_notification_at) < cooldown_ms
    {
        return false;
    }
    !(current.policy.max_reports_per_hour > 0
        && window_open
        && current.notifications_in_window >= current.policy.max_reports_per_hour)
}

fn notification_allowed_for_profile(profile: &StaffProfile, kind: &str) -> bool {
    staff::with_registry(|registry| {
        let current = registry
            .get_mut(&profile.staff_id)
            .ok_or_else(|| format!("员工不存在: {}", profile.staff_id))?;
        let now = now_ms();
        if !current.policy.notification_allowed(kind, now) {
            return Ok(false);
        }
        if bypasses_report_budget(kind) {
            return Ok(true);
        }
        if now.saturating_sub(current.notification_window_started_at) >= 3_600_000 {
            current.notification_window_started_at = now;
            current.notifications_in_window = 0;
        }
        let cooldown_ms = (current.policy.notification_cooldown_minutes as u64) * 60_000;
        if cooldown_ms > 0
            && current.last_notification_at > 0
            && now.saturating_sub(current.last_notification_at) < cooldown_ms
        {
            return Ok(false);
        }
        if current.policy.max_reports_per_hour > 0
            && current.notifications_in_window >= current.policy.max_reports_per_hour
        {
            return Ok(false);
        }
        current.last_notification_at = now;
        current.notifications_in_window = current.notifications_in_window.saturating_add(1);
        Ok(true)
    })
    .unwrap_or(false)
}

/// Telegram additionally requires a bound chat. Single-staff installs retain
/// the legacy paired-chat fallback; multi-staff installs require an explicit
/// binding so one private chat cannot silently fan out between employees.
pub fn telegram_notification_allowed(session_id: &str, kind: &str) -> bool {
    let Some(profile) = profile_for_notification_session(session_id) else {
        return !is_orphaned_staff_session(session_id);
    };
    telegram_notification_allowed_for_profile(&profile, kind)
}

/// Telegram-specific gate for task cards. Stable `staffId` wins over the
/// task's old chief session id so recreating a staff ACP session cannot bypass
/// the current employee policy.
pub fn telegram_task_notification_allowed(task: &super::types::Task, kind: &str) -> bool {
    let profile = task
        .staff_id
        .as_deref()
        .and_then(profile_for_staff_id)
        .or_else(|| profile_for_session(&task.chief_session_id));
    let Some(profile) = profile else {
        return task.staff_id.is_none();
    };
    telegram_notification_allowed_for_profile(&profile, kind)
}

fn telegram_notification_allowed_for_profile(profile: &StaffProfile, kind: &str) -> bool {
    if !profile.policy.allow_telegram || !notification_allowed_for_profile(profile, kind) {
        return false;
    }
    let paired = crate::telegram_remote::paired_chat_id();
    match profile.telegram_chat_id {
        Some(chat_id) => paired == Some(chat_id),
        None => ensure_migrated().staff.len() == 1,
    }
}

pub fn profile_for_chat(chat_id: i64) -> Option<StaffProfile> {
    let registry = ensure_migrated();
    if let Some(profile) = registry
        .staff
        .iter()
        .find(|profile| profile.telegram_chat_id == Some(chat_id))
    {
        return Some(profile.clone());
    }
    // Single-staff installs keep the legacy "one paired chat = the assistant"
    // behavior. With multiple staff, an unbound chat must not guess a person.
    (registry.staff.len() == 1)
        .then(|| registry.staff[0].clone())
}

/// A chat explicitly bound in a staff profile is enough to route Telegram to
/// that employee. Legacy `mode=chief` keeps the old single-staff fallback.
pub fn telegram_chat_routes_to_staff(chat_id: i64) -> bool {
    let registry = ensure_migrated();
    if registry
        .staff
        .iter()
        .any(|profile| profile.telegram_chat_id == Some(chat_id))
    {
        return true;
    }
    store::load_config().mode == super::types::RemoteWorkMode::Chief && registry.staff.len() == 1
}

pub fn default_staff() -> Option<StaffProfile> {
    ensure_migrated().staff.into_iter().next()
}

pub fn config_for_staff(profile: &StaffProfile) -> super::types::ChiefConfig {
    let mut config = store::load_config();
    config.mode = super::types::RemoteWorkMode::Chief;
    config.chief_session_id = Some(profile.session_id.clone());
    config.chief_agent_id = Some(profile.agent_id.clone());
    config.chief_model = profile.model.clone();
    config.allowed_projects = profile.policy.allowed_projects.clone();
    config.max_concurrent_workers = profile.policy.max_active_tasks();
    config
}
