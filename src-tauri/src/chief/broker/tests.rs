//! broker 纯逻辑测试（进程级静态需要沙箱锁串行；不构造 AppHandle）。

use super::*;
use crate::chief::store::{self, SANDBOX_LOCK};
use crate::chief::types::{ChiefConfig, RemoteWorkMode, Task, TaskStatus};

fn lock_sandbox() -> std::sync::MutexGuard<'static, ()> {
    SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn sandbox_root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "chief-broker-test-{}-{}",
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn settings_verdicts_name_the_cause_and_bring_back_the_models() {
    // 点名的模型不被接受：说清「没有改用默认模型」，并带回 agent 的真实
    // 清单——派活的 AI 照着清单重派一次就行。
    let rejected = SettingsError::Model {
        model: "opencode/does-not-exist".into(),
        reason: "model not found".into(),
        available: vec![
            "opencode/grok-code".to_string(),
            "openrouter/mistral/small:free".to_string(),
        ],
    };
    let text = rejected.verdict("opencode");
    assert!(text.contains("opencode 不接受模型「opencode/does-not-exist」"));
    assert!(text.contains("没有改用默认模型"));
    assert!(text.contains("opencode/grok-code"));
    assert!(text.contains("openrouter/mistral/small:free"));
    assert!(!text.contains("共"), "没截断就不报总数");
    // agent 没广告任何模型：别编清单，给出下一步。
    let no_catalog = SettingsError::Model {
        model: "x".into(),
        reason: "unknown model".into(),
        available: vec![],
    };
    let text = no_catalog.verdict("codex");
    assert!(text.contains("没有广告任何模型清单"));
    assert!(text.contains("不指定 model 重派"));
    // 清单很长：如实标总数，只列前 40 个。
    let long = SettingsError::Model {
        model: "x".into(),
        reason: "unknown model".into(),
        available: (0..45).map(|index| format!("provider/model-{index}")).collect(),
    };
    let text = long.verdict("opencode");
    assert!(text.contains("共 45 个"));
    assert!(text.contains("provider/model-39"));
    assert!(!text.contains("provider/model-40"));
    assert!(SettingsError::ReadOnly("no such option".into()).verdict("x").contains("只读"));
}

#[test]
fn model_echo_tolerates_an_effort_suffix() {
    // Codex 把档位编进模型名（`gpt-5.6-luna[xhigh]`）——同一个模型，不算不符。
    assert!(model_echo_matches("gpt-5.6-luna", "gpt-5.6-luna[xhigh]"));
    assert!(model_echo_matches("gpt-5.6-luna[xhigh]", "gpt-5.6-luna"));
    assert!(model_echo_matches("opencode/grok-code", " opencode/grok-code "));
    assert!(!model_echo_matches("openrouter/a:free", "openrouter/b"));
    assert!(!model_echo_matches("gpt-5.6", "gpt-5.6-luna"));
    // 括号不成对就不是档位后缀，原样比。
    assert!(model_echo_matches("weird[name", "weird[name"));
    assert!(!model_echo_matches("weird[name", "weird"));
}

#[test]
fn wake_inbox_rebuilds_from_undelivered_ledger_events() {
    let guard = lock_sandbox();
    reset_for_tests();
    let root = sandbox_root("wakerebuild");
    store::set_root_override_for_tests(Some(root.clone()));
    // 两个任务：一个有未送达的 completed 事件，一个事件已送达。
    let mut task_a = Task {
        chief_session_id: "session_parent_a".into(),
        status: TaskStatus::Completed,
        ..Default::default()
    };
    task_a.task_id = "task_a".into();
    let mut task_b = Task {
        chief_session_id: "session_parent_b".into(),
        status: TaskStatus::Failed,
        ..Default::default()
    };
    task_b.task_id = "task_b".into();
    store::with_tasks(|tasks| {
        tasks.extend([task_a.clone(), task_b.clone()]);
        Ok(())
    })
    .unwrap();
    let now = now_ms();
    let undelivered = crate::chief::types::TaskEvent {
        event_id: "evt_a".into(),
        seq: 1,
        task_id: "task_a".into(),
        kind: "task.completed".into(),
        status: Some(TaskStatus::Completed),
        summary: None,
        delivered_at: None,
        ts: now,
    };
    let delivered = crate::chief::types::TaskEvent {
        event_id: "evt_b".into(),
        seq: 1,
        task_id: "task_b".into(),
        kind: "task.failed".into(),
        status: Some(TaskStatus::Failed),
        summary: None,
        delivered_at: Some(now),
        ts: now,
    };
    store::append_events(&[undelivered, delivered]).unwrap();
    rebuild_wake_inbox_from_ledger();
    assert_eq!(pending_wake_count("session_parent_a"), 1, "未送达的事件要重建进收件箱");
    assert_eq!(pending_wake_count("session_parent_b"), 0, "已送达的不重建");
    store::set_root_override_for_tests(None);
    reset_for_tests();
    drop(guard);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn bootstrap_queue_keeps_only_the_latest_prompt() {
    let guard = lock_sandbox();
    reset_for_tests();
    queue_bootstrap_text("session_bootstrap", "old");
    queue_bootstrap_text("session_bootstrap", "new");
    assert_eq!(pending_user_count("session_bootstrap"), 1);
    assert_eq!(
        pop_bootstrap_text("session_bootstrap").as_deref(),
        Some("new")
    );
    reset_for_tests();
    drop(guard);
}

///Chief 注册表：显式注册 → 是；config 引用 → 是；其它 → 否。
#[test]
fn chief_registry_gates_sessions() {
    let guard = lock_sandbox();
    let root = sandbox_root("registry");
    store::set_root_override_for_tests(Some(root.clone()));
    staff::set_root_override_for_tests(Some(root.clone()));
    reset_for_tests();
    assert!(!is_chief_session("session_x"));
    register_chief_session("session_x");
    assert!(is_chief_session("session_x"));
    unregister_chief_session("session_x");
    assert!(!is_chief_session("session_x"));
    // config.chief_session_id 也是判据（重启后注册表空但配置还在）。
    store::save_config(&ChiefConfig {
        mode: RemoteWorkMode::Chief,
        chief_session_id: Some("session_cfg".into()),
        ..Default::default()
    })
    .unwrap();
    assert!(is_chief_session("session_cfg"));
    assert!(!is_chief_session("session_worker"));
    reset_for_tests();
    staff::set_root_override_for_tests(None);
    store::set_root_override_for_tests(None);
    drop(guard);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn dismissed_last_staff_does_not_revive_stale_config_session() {
    let guard = lock_sandbox();
    let root = sandbox_root("dismissed-staff");
    store::set_root_override_for_tests(Some(root.clone()));
    staff::set_root_override_for_tests(Some(root.clone()));
    reset_for_tests();
    store::save_config(&ChiefConfig {
        mode: RemoteWorkMode::Chief,
        chief_session_id: Some("session_stale_chief".into()),
        ..Default::default()
    })
    .unwrap();
    assert!(is_chief_session("session_stale_chief"));
    staff::save(&staff::StaffRegistry {
        legacy_migration_completed_at: now_ms(),
        ..Default::default()
    })
    .unwrap();
    assert!(!is_chief_session("session_stale_chief"));
    reset_for_tests();
    staff::set_root_override_for_tests(None);
    store::set_root_override_for_tests(None);
    drop(guard);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn staff_worker_computer_use_follows_project_policy_and_fails_closed() {
    let guard = lock_sandbox();
    let root = sandbox_root("staff-computer-use");
    store::set_root_override_for_tests(Some(root.clone()));
    staff::set_root_override_for_tests(Some(root.clone()));
    reset_for_tests();
    let profile = staff::StaffProfile {
        staff_id: "staff_computer".into(),
        session_id: "session_staff_computer".into(),
        policy: staff::StaffPolicy {
            allowed_projects: vec![crate::chief::types::ProjectPolicy {
                project_id: "project_computer".into(),
                allow_computer_use: false,
                ..Default::default()
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    let mut registry = staff::StaffRegistry::new();
    registry.add(profile.clone()).unwrap();
    staff::save(&registry).unwrap();
    store::with_tasks(|tasks| {
        tasks.push(Task {
            staff_id: Some(profile.staff_id.clone()),
            chief_session_id: profile.session_id.clone(),
            worker_session_id: Some("session_worker_computer".into()),
            project_id: "project_computer".into(),
            ..Default::default()
        });
        Ok(())
    })
    .unwrap();
    assert!(!staff_commands::worker_computer_use_allowed(
        "session_worker_computer"
    ));
    let mut enabled = profile;
    enabled.policy.allowed_projects[0].allow_computer_use = true;
    let mut registry = staff::load();
    registry.update(enabled).unwrap();
    staff::save(&registry).unwrap();
    assert!(staff_commands::worker_computer_use_allowed(
        "session_worker_computer"
    ));
    staff::save(&staff::StaffRegistry {
        legacy_migration_completed_at: now_ms(),
        ..Default::default()
    })
    .unwrap();
    assert!(!staff_commands::worker_computer_use_allowed(
        "session_worker_computer"
    ));
    reset_for_tests();
    staff::set_root_override_for_tests(None);
    store::set_root_override_for_tests(None);
    drop(guard);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn explicit_staff_chat_routes_without_legacy_chief_mode() {
    let guard = lock_sandbox();
    let root = sandbox_root("staff-telegram-route");
    store::set_root_override_for_tests(Some(root.clone()));
    staff::set_root_override_for_tests(Some(root.clone()));
    reset_for_tests();
    store::save_config(&ChiefConfig::default()).unwrap();
    let mut registry = staff::StaffRegistry::new();
    registry
        .add(staff::StaffProfile {
            staff_id: "staff_chat".into(),
            session_id: "session_staff_chat".into(),
            telegram_chat_id: Some(12345),
            ..Default::default()
        })
        .unwrap();
    staff::save(&registry).unwrap();
    assert!(staff_commands::telegram_chat_routes_to_staff(12345));
    assert!(!staff_commands::telegram_chat_routes_to_staff(99999));
    store::save_config(&ChiefConfig {
        mode: RemoteWorkMode::Chief,
        ..Default::default()
    })
    .unwrap();
    assert!(staff_commands::telegram_chat_routes_to_staff(99999));
    reset_for_tests();
    staff::set_root_override_for_tests(None);
    store::set_root_override_for_tests(None);
    drop(guard);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn ordinary_notification_budget_survives_by_being_written_to_staff_profile() {
    let guard = lock_sandbox();
    let root = sandbox_root("staff-notify-budget");
    staff::set_root_override_for_tests(Some(root.clone()));
    reset_for_tests();
    let mut registry = staff::StaffRegistry::new();
    registry
        .add(staff::StaffProfile {
            staff_id: "staff_notify".into(),
            session_id: "session_staff_notify".into(),
            policy: staff::StaffPolicy {
                quiet_hours: String::new(),
                notification_cooldown_minutes: 30,
                max_reports_per_hour: 4,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    staff::save(&registry).unwrap();
    assert!(staff_commands::notification_allowed(
        "session_staff_notify",
        "completed"
    ));
    let saved = staff::load();
    let profile = saved.get("staff_notify").unwrap();
    assert!(profile.notification_window_started_at > 0);
    assert_eq!(profile.notifications_in_window, 1);
    assert!(profile.last_notification_at > 0);
    assert!(!staff_commands::notification_allowed(
        "session_staff_notify",
        "completed"
    ));
    assert!(staff_commands::notification_allowed(
        "session_staff_notify",
        "task.failed"
    ));
    reset_for_tests();
    staff::set_root_override_for_tests(None);
    drop(guard);
    std::fs::remove_dir_all(&root).ok();
}

/// 桌面提醒用只读闸（不占额度）；task.created / task.started 是确认回执，
/// 不占冷却/小时额度也不被它们挡——否则默认 30 分钟冷却会吞掉完成卡。
#[test]
fn desktop_peek_and_task_acks_do_not_consume_report_budget() {
    let guard = lock_sandbox();
    let root = sandbox_root("staff-notify-peek");
    staff::set_root_override_for_tests(Some(root.clone()));
    reset_for_tests();
    let mut registry = staff::StaffRegistry::new();
    registry
        .add(staff::StaffProfile {
            staff_id: "staff_peek".into(),
            session_id: "session_staff_peek".into(),
            policy: staff::StaffPolicy {
                quiet_hours: String::new(),
                notification_cooldown_minutes: 30,
                max_reports_per_hour: 4,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    staff::save(&registry).unwrap();
    // 桌面侧反复探测不消耗额度。
    for _ in 0..3 {
        assert!(staff_commands::notification_would_allow(
            "session_staff_peek",
            "completed"
        ));
    }
    assert_eq!(staff::load().get("staff_peek").unwrap().notifications_in_window, 0);
    // 确认回执既不占额度……
    assert!(staff_commands::notification_allowed("session_staff_peek", "task.created"));
    assert!(staff_commands::notification_allowed("session_staff_peek", "task.started"));
    assert_eq!(staff::load().get("staff_peek").unwrap().notifications_in_window, 0);
    // ……完成汇报照常拿到第一个名额，之后进入冷却，peek 同步反映。
    assert!(staff_commands::notification_allowed("session_staff_peek", "completed"));
    assert!(!staff_commands::notification_would_allow("session_staff_peek", "completed"));
    assert!(staff_commands::notification_would_allow("session_staff_peek", "task.created"));
    reset_for_tests();
    staff::set_root_override_for_tests(None);
    drop(guard);
    std::fs::remove_dir_all(&root).ok();
}

/// 唤醒收件箱与用户队列：FIFO、计数、互不串台。
#[test]
fn wake_inbox_and_user_queue_are_fifo() {
    let guard = lock_sandbox();
    reset_for_tests();
    let chief = "session_chief_q";
    let mut event = TaskEvent {
        event_id: "evt1".into(),
        seq: 1,
        task_id: "task_1".into(),
        kind: "task.completed".into(),
        status: Some(TaskStatus::Completed),
        summary: Some("done".into()),
        delivered_at: None,
        ts: now_ms(),
    };
    queue_wake(chief, event.clone());
    event.event_id = "evt2".into();
    queue_wake(chief, event);
    queue_wake("session_other", TaskEvent {
        event_id: "evt3".into(),
        seq: 1,
        task_id: "task_2".into(),
        kind: "task.failed".into(),
        status: Some(TaskStatus::Failed),
        summary: None,
        delivered_at: None,
        ts: now_ms(),
    });
    assert_eq!(pending_wake_count(chief), 2);
    assert_eq!(pending_wake_count("session_other"), 1);

    queue_user_text(chief, "第一条");
    queue_user_text(chief, "第二条");
    assert_eq!(pending_user_count(chief), 2);
    // 队列顺序由 pop 断言（pop 是私有 fn，测试模块在 broker 内可见）。
    assert_eq!(pop_user_text(chief).as_deref(), Some("第一条"));
    assert_eq!(pop_user_text(chief).as_deref(), Some("第二条"));
    assert_eq!(pop_user_text(chief), None);
    reset_for_tests();
    drop(guard);
}

/// task_send 的 (taskId, commandId) 幂等：重复键跳过，不同键都执行。
#[test]
fn send_command_id_dedup() {
    let guard = lock_sandbox();
    reset_for_tests();
    assert!(mark_sent_command("task_a", "cmd_1"));
    assert!(!mark_sent_command("task_a", "cmd_1"), "同键必须幂等跳过");
    assert!(mark_sent_command("task_a", "cmd_2"));
    assert!(mark_sent_command("task_b", "cmd_1"));
    reset_for_tests();
    drop(guard);
}

/// worker 事件折叠：正文尾部、工具计数、文件去重。
#[test]
fn summarize_worker_events_folds_the_stream() {
    use serde_json::json;
    let events = vec![
        json!({
            "sessionId": "session_w", "kind": "status", "method": "session/update",
            "data": { "params": { "update": { "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": "先看构建日志。" } } } }
        }),
        json!({
            "sessionId": "session_w", "kind": "status", "method": "session/update",
            "data": { "params": { "update": { "sessionUpdate": "tool_call",
                "toolCallId": "t1", "title": "read file",
                "rawInput": { "path": "D:/repo/src/main.rs" } } } }
        }),
        json!({
            "sessionId": "session_w", "kind": "status", "method": "session/update",
            "data": { "params": { "update": { "sessionUpdate": "tool_call",
                "toolCallId": "t2", "title": "write file",
                "rawInput": { "path": "D:/repo/src/main.rs" } } } }
        }),
        json!({
            "sessionId": "session_w", "kind": "status", "method": "session/update",
            "data": { "params": { "update": { "sessionUpdate": "tool_call_update",
                "toolCallId": "t2", "status": "failed" } } }
        }),
        json!({
            "sessionId": "session_w", "kind": "status", "method": "session/update",
            "data": { "params": { "update": { "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": "修好了，测试全绿。" } } } }
        }),
        // 无关事件（非 session/update）不参与折叠
        json!({ "sessionId": "session_w", "kind": "error", "method": null, "data": {} }),
    ];
    let (summary, tool_calls, files, tool_failures) = summarize_worker_events(&events);
    assert_eq!(tool_calls, 2);
    assert_eq!(tool_failures, 1);
    assert_eq!(files, vec!["D:/repo/src/main.rs"], "同一路径去重");
    let summary = summary.expect("assistant text exists");
    assert!(summary.ends_with("修好了，测试全绿。"), "取尾部正文");
    assert!(summary.contains("先看构建日志。"));
}

#[test]
fn summarize_empty_stream_is_honest() {
    let (summary, tool_calls, files, tool_failures) = summarize_worker_events(&[]);
    assert!(summary.is_none());
    assert_eq!((tool_calls, files.len(), tool_failures), (0, 0, 0));
}

/// HOST EVENT 文本：结构化、带信任标签（03-security-permissions.md §8）。
#[test]
fn host_event_text_is_structured_and_tagged() {
    let task = Task {
        task_id: "task_9".into(),
        title: "修复构建".into(),
        artifacts: vec!["触及文件：src/x.rs".into()],
        ..Default::default()
    };
    let event = TaskEvent {
        event_id: "evt".into(),
        seq: 3,
        task_id: "task_9".into(),
        kind: "task.completed".into(),
        status: Some(TaskStatus::Completed),
        summary: Some("测试全绿".into()),
        delivered_at: None,
        ts: now_ms(),
    };
    let text = host_event_text(&event, Some(&task));
    assert!(text.starts_with("[HOST EVENT: task.completed]"));
    assert!(text.contains("taskId: task_9"));
    assert!(text.contains("status: completed"));
    assert!(text.contains("summary: 测试全绿"));
    assert!(text.contains("untrustedContent: false"));
}

/// active 计数：三种进行态计入，终态不计；项目过滤正确。
#[test]
fn active_task_counting_matches_concurrency_rules() {
    use crate::chief::types::TaskStatus::*;
    let guard = lock_sandbox();
    let root = sandbox_root("active_count");
    store::set_root_override_for_tests(Some(root.clone()));
    staff::set_root_override_for_tests(Some(root.clone()));
    reset_for_tests();
    let chief = "session_c";
    let mk = |project: &str, status: TaskStatus| Task {
        chief_session_id: chief.into(),
        project_id: project.into(),
        status,
        ..Default::default()
    };
    let tasks = vec![
        mk("project_b", Proposed),
        mk("project_a", Running),
        mk("project_a", WaitingUser),
        mk("project_a", Completed),
        mk("project_b", Starting),
        // 别的 chief 的任务不算
        Task { chief_session_id: "session_other".into(), status: Running, ..Default::default() },
    ];
    assert_eq!(count_active(&tasks, chief, None), 4);
    assert_eq!(count_active(&tasks, chief, Some("project_a")), 2);
    assert_eq!(count_active(&tasks, chief, Some("project_b")), 2);
    reset_for_tests();
    staff::set_root_override_for_tests(None);
    store::set_root_override_for_tests(None);
    drop(guard);
    std::fs::remove_dir_all(&root).ok();
}

/// tasks_list envelope：只列本 Chief 的任务，status 过滤生效。
#[test]
fn tasks_list_filters_by_chief_and_status() {
    let guard = lock_sandbox();
    let root = sandbox_root("tasks_list");
    store::set_root_override_for_tests(Some(root.clone()));
    reset_for_tests();
    let mine = Task {
        chief_session_id: "session_chief_list".into(),
        status: TaskStatus::Running,
        title: "我的任务".into(),
        ..Default::default()
    };
    let other = Task {
        chief_session_id: "session_other".into(),
        status: TaskStatus::Running,
        ..Default::default()
    };
    store::with_tasks(|tasks| {
        tasks.extend([mine.clone(), other]);
        Ok(())
    })
    .unwrap();
    let value = chief_tasks_list("session_chief_list", None, 20);
    assert_eq!(value.pointer("/ok"), Some(&serde_json::json!(true)));
    let listed = value.pointer("/tasks").unwrap().as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].pointer("/title").and_then(serde_json::Value::as_str), Some("我的任务"));
    // status 过滤：running 命中，completed 空
    let hit = chief_tasks_list("session_chief_list", Some("running"), 20);
    assert_eq!(hit.pointer("/tasks").unwrap().as_array().unwrap().len(), 1);
    let miss = chief_tasks_list("session_chief_list", Some("completed"), 20);
    assert_eq!(miss.pointer("/tasks").unwrap().as_array().unwrap().len(), 0);
    reset_for_tests();
    store::set_root_override_for_tests(None);
    drop(guard);
    std::fs::remove_dir_all(&root).ok();
}
