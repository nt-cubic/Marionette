# -*- coding: utf-8 -*-
# M3: repair-round prompt + real Verifying loop

# 1. prompts.rs — repair round prompt
p = 'src-tauri/src/council/prompts.rs'
s = open(p, encoding='utf-8').read()
addition = '''
/// 修复轮(§4.3 targeted revision):验证失败后,主席根据失败证据修订
/// 计划。要求:只改有问题的步骤(未受影响的保持原 id 原样输出,使已
/// 完成的任务可以保留);每处修改附失败证据的对应关系。
pub fn repair_round_prompt(
    spec: &CouncilSpec,
    chair: &MemberSpec,
    plan: &PlanDoc,
    evidence: &str,
    attempt: u32,
) -> (String, String) {
    let system = format!(
        "{}你是主席。执行验证失败了,你需要**修订实施计划**。规则:\\n- 只修改与失败证据相关的步骤;无关步骤保持 id 与内容不变(系统会保留它们已完成的执行结果)。\\n- 修改/新增的步骤要针对证据里具体的失败原因。\\n- 若失败源于验证命令本身不合理,可以修正 verification 清单并说明理由。{}",
        role_header(spec, chair),
        OUTPUT_DISCIPLINE
    );
    let steps = plan
        .steps
        .iter()
        .map(|s| {
            format!(
                "- [{}] {}(detail: {}; dependsOn: {:?}; acceptance: {})",
                s.id, s.title, s.detail, s.depends_on, s.acceptance
            )
        })
        .collect::<Vec<_>>()
        .join("\\n");
    let user = format!(
        "## 第 {attempt} 次修复尝试\\n\\n## 当前计划\\n{steps}\\n\\n## 验证失败证据\\n{evidence}\\n\\n## 任务原文\\n{}\\n\\n只输出一个 ```json 围栏,内容为修订后的完整计划(与原 schema 相同)。",
        spec.task
    );
    (system, user)
}
'''
anchor = 'pub fn kind_name(kind: MessageKind)'
assert anchor in s
s = s.replace(anchor, addition + '\n' + anchor, 1)
open(p, 'w', encoding='utf-8', newline='\n').write(s)

# 2. types.rs — verify_attempts on CouncilState
p2 = 'src-tauri/src/council/types.rs'
s2 = open(p2, encoding='utf-8').read()
s2 = s2.replace('''    /// 执行阶段的任务账目(Executing 起;M1 run 为空数组)。
    #[serde(default)]
    pub tasks: Vec<crate::council::tasks::TaskProgress>,''',
'''    /// 执行阶段的任务账目(Executing 起;M1 run 为空数组)。
    #[serde(default)]
    pub tasks: Vec<crate::council::tasks::TaskProgress>,
    /// 验证-修复回路已用次数(上限 3,任务书 §4.3)。
    #[serde(default)]
    pub verify_attempts: u32,''')
open(p2, 'w', encoding='utf-8', newline='\n').write(s2)

# 3. mod.rs register verify
p3 = 'src-tauri/src/council/mod.rs'
s3 = open(p3, encoding='utf-8').read()
s3 = s3.replace('pub mod worker;', 'pub mod verify;\npub mod worker;')
open(p3, 'w', encoding='utf-8', newline='\n').write(s3)

# 4. orchestrator.rs — real Verifying arm with bounded repair loop
p4 = 'src-tauri/src/council/orchestrator.rs'
s4 = open(p4, encoding='utf-8').read()
old_verify = '''            RunPhase::Verifying => {
                // M2:自动验证命令执行属 M3;当前把计划声明的验证清单作
                // 为交付证据记录后完成(诚实标注,不伪造验证结果)。
                if let Some(plan) = store::load_plan(run_id)? {
                    if !plan.verification.is_empty() {
                        let msg = new_message(
                            run_id, state.round, "system", "all", MessageKind::Evidence, None,
                            &format!("计划声明的验证命令(自动执行属后续版本):\\n{}", plan.verification.iter().map(|v| format!("- `{v}`")).collect::<Vec<_>>().join("\\n")),
                            None,
                        );
                        persist_message(&mut messages, run_id, emitter, msg);
                    }
                }
                transition(&mut state, run_id, emitter, RunPhase::Completed, Some("执行完成;自动验证属后续版本"));
                emitter.emit(run_id, "finished", json!({ "phase": "completed" }));
                return Ok(());
            }'''
new_verify = '''            RunPhase::Verifying => {
                let Some(plan) = store::load_plan(run_id)? else {
                    fail_run(&mut state, run_id, emitter, "验证阶段找不到计划".into());
                    return Ok(());
                };
                let Some(executor) = spec.executor.clone() else {
                    // 无执行器的 run 不该进这里(防御)。
                    transition(&mut state, run_id, emitter, RunPhase::Completed, Some("无执行器,跳过验证"));
                    return Ok(());
                };
                let cwd = std::path::PathBuf::from(&executor.cwd);
                let results = super::verify::run_all(&cwd, &plan.verification, 900);
                let evidence = super::verify::render_evidence(&results);
                let all_ok = results.iter().all(|r| r.ok);
                let msg = new_message(
                    run_id,
                    state.round,
                    "system",
                    "all",
                    MessageKind::Evidence,
                    None,
                    &evidence,
                    Some(json!({ "attempt": state.verify_attempts, "allOk": all_ok })),
                );
                persist_message(&mut messages, run_id, emitter, msg);
                emitter.emit(run_id, "verify", serde_json::to_value(&results).unwrap_or(json!([])));

                if all_ok {
                    transition(&mut state, run_id, emitter, RunPhase::Completed, Some("验证全部通过"));
                    emitter.emit(run_id, "finished", json!({ "phase": "completed" }));
                    return Ok(());
                }
                // 失败 → 受限修复回路(默认上限 3,任务书 §4.3)。
                if state.verify_attempts >= MAX_VERIFY_ATTEMPTS {
                    fail_run(&mut state, run_id, emitter, format!("验证失败且修复次数已用尽({} 次)", MAX_VERIFY_ATTEMPTS));
                    return Ok(());
                }
                state.verify_attempts += 1;
                let chair = chair_of(&spec).ok_or("找不到主席成员")?;
                let (system, user) =
                    prompts::repair_round_prompt(&spec, chair, &plan, &evidence, state.verify_attempts);
                let repaired_raw = match call_model(&mut state, client, handle, &chair.model, &system, &user, "plan-repair") {
                    Ok(out) => out.content,
                    Err(e) => {
                        fail_run(&mut state, run_id, emitter, format!("修复计划生成失败:{e}"));
                        return Ok(());
                    }
                };
                let new_plan = match parse_plan(&repaired_raw) {
                    Ok(p) => p,
                    Err(first) => {
                        emitter.emit(run_id, "warn", json!({ "message": "修复计划 JSON 解析失败,请求一次修复", "error": first }));
                        let (rs, ru) = prompts::plan_repair_prompt(chair, &repaired_raw, &first);
                        match call_model(&mut state, client, handle, &chair.model, &rs, &ru, "plan-repair-retry") {
                            Ok(out) => match parse_plan(&out.content) {
                                Ok(p) => p,
                                Err(second) => {
                                    fail_run(&mut state, run_id, emitter, format!("修复计划两次解析失败:{second}"));
                                    return Ok(());
                                }
                            },
                            Err(e) => {
                                fail_run(&mut state, run_id, emitter, format!("修复计划调用失败:{e}"));
                                return Ok(());
                            }
                        }
                    }
                };
                // 版本化:旧计划已由 Plan 消息留痕;新计划落盘 + Plan 消息。
                store::save_plan(run_id, &new_plan)?;
                let plan_msg = new_message(
                    run_id, state.round, &chair.id, "all", MessageKind::Plan,
                    None, &new_plan.summary,
                    Some(json!({ "planJson": repaired_raw, "repairRound": state.verify_attempts })),
                );
                persist_message(&mut messages, run_id, emitter, plan_msg);
                // 任务账本迁移:同 id 同 detail 的 Merged 任务保留(不重跑);
                // 其余(失败的、detail 变化的、新增的)复位 Pending。
                let old_plan = plan;
                state.tasks = migrate_tasks(&old_plan, &new_plan, &state.tasks);
                let _ = store::save_state(&state);
                emitter.emit(run_id, "warn", json!({
                    "message": format!("验证失败,进入第 {} 次修复:计划已修订,受影响任务复位重跑", state.verify_attempts)
                }));
                transition(&mut state, run_id, emitter, RunPhase::Executing, Some("验证失败,修订计划后重执行"));
                continue;
            }'''
assert old_verify in s4
s4 = s4.replace(old_verify, new_verify, 1)

# constant + migrate_tasks helper
s4 = s4.replace('''// ---------------------------------------------------------------------------
// fan-out job
// ---------------------------------------------------------------------------''',
'''/// 验证-修复回路上限(任务书 §4.3 默认 3)。
pub const MAX_VERIFY_ATTEMPTS: u32 = 3;

/// 修订计划后的任务账本迁移:同 id 且同 detail 的已完成(Merged)任务
/// 保留执行成果;失败/被修改/新增的任务复位 Pending。被删除的步骤出账。
fn migrate_tasks(old_plan: &crate::council::types::PlanDoc, new_plan: &crate::council::types::PlanDoc, current: &[super::tasks::TaskProgress]) -> Vec<super::tasks::TaskProgress> {
    use super::tasks::TaskStatus;
    let mut out = Vec::new();
    for new_step in &new_plan.steps {
        let old_detail = old_plan.steps.iter().find(|s| s.id == new_step.id).map(|s| s.detail.clone());
        let current_prog = current.iter().find(|t| t.step_id == new_step.id);
        let keep = match (current_prog, &old_detail) {
            (Some(prog), Some(old)) => {
                prog.status == TaskStatus::Merged && *old == new_step.detail
            }
            _ => false,
        };
        if keep {
            let mut kept = current_prog.unwrap().clone();
            kept.title = new_step.title.clone();
            out.push(kept);
        } else {
            out.push(super::tasks::TaskProgress {
                step_id: new_step.id.clone(),
                title: new_step.title.clone(),
                status: TaskStatus::Pending,
                session_id: None,
                worktree: None,
                branch: None,
                changes: Vec::new(),
                error: None,
                attempts: current_prog.map(|p| p.attempts).unwrap_or(0),
                notes: String::new(),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// fan-out job
// ---------------------------------------------------------------------------''')

# fix all CouncilState constructors for verify_attempts
for path in ['src-tauri/src/council/store.rs', 'src-tauri/src/council/orchestrator.rs', 'src-tauri/src/council/state.rs']:
    sx = open(path, encoding='utf-8').read()
    sx = sx.replace('''            tasks: Vec::new(),
            updated_at:''', '''            tasks: Vec::new(),
            verify_attempts: 0,
            updated_at:''')
    open(path, 'w', encoding='utf-8', newline='\n').write(sx)

p5 = 'src-tauri/src/commands.rs'
s5 = open(p5, encoding='utf-8').read()
s5 = s5.replace('''        tasks: Vec::new(),
        updated_at: council_types::now_millis(),''', '''        tasks: Vec::new(),
        verify_attempts: 0,
        updated_at: council_types::now_millis(),''')
open(p5, 'w', encoding='utf-8', newline='\n').write(s5)

print('M3 wired')
