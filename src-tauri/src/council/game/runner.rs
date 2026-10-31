//! 游戏对局循环:引擎(真相)+ 玩家代理(决策)+ 消息账本(可观测)。
//!
//! 复用圆桌的基础设施:ModelClient seam(mock 可测)、council-event
//! 事件、append-only 消息账本、取消与预算。恢复语义:game-state.json
//! 存在则从当前局面继续(引擎状态全量序列化;回合从头继续当前 turn)。

use super::engine::{self, GameState, Side, TurnPlan};
use super::player;
use crate::council::orchestrator::CouncilEventEmitter;
use crate::council::store;
use crate::council::types::*;
use crate::council::worker::ModelClient;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_ROUNDS: u32 = 12;
const GAME_STATE_FILE: &str = "game-state.json";
const HISTORY_FILE: &str = "game-history.json";

fn game_state_path(run_id: &str) -> Result<std::path::PathBuf, String> {
    Ok(store::run_dir(run_id)?.join(GAME_STATE_FILE))
}

pub fn save_game(run_id: &str, game: &GameState, history: &[String]) -> Result<(), String> {
    let body = serde_json::to_string_pretty(game).map_err(|e| e.to_string())?;
    crate::storage::write_atomic(&game_state_path(run_id)?, &body).map_err(|e| e.to_string())?;
    let hist = serde_json::to_string_pretty(history).map_err(|e| e.to_string())?;
    crate::storage::write_atomic(&store::run_dir(run_id)?.join(HISTORY_FILE), &hist).map_err(|e| e.to_string())
}

pub fn load_game(run_id: &str) -> Option<(GameState, Vec<String>)> {
    let path = game_state_path(run_id).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let game: GameState = serde_json::from_str(&text).ok()?;
    let history = std::fs::read_to_string(store::run_dir(run_id).ok()?.join(HISTORY_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    Some((game, history))
}

/// 运行整局(阻塞;由 orchestrator 的 run 线程调用)。返回人话结果。
pub fn run_game(
    emitter: &CouncilEventEmitter,
    run_id: &str,
    spec: &CouncilSpec,
    client: &dyn ModelClient,
    cancel: &AtomicBool,
    seed: u64,
) -> Result<(), String> {
    let member_ids: Vec<String> = spec.members.iter().map(|m| m.id.clone()).collect();
    // 开局或恢复。
    let (mut game, mut history) = match load_game(run_id) {
        Some(restored) => (restored.0, restored.1),
        None => {
            let g = engine::new_game(&member_ids, &mut engine::Lcg(seed))?;
            (g, Vec::new())
        }
    };
    // 开局公告(恢复时不重复)。
    if history.is_empty() {
        let seat_list = player::seat_hint(&spec.members);
        let mut opening = format!("身份局开局(简化规则):座次 {seat_list}。");
        if let Some(lord) = game.players.iter().find(|p| p.revealed) {
            opening.push_str(&format!(" 主公是 `{}`(身份亮明);其余身份暗置。", lord.member_id));
        }
        let msg = new_game_message(run_id, "system", None, &opening);
        store::append_message(run_id, &msg).ok();
        emitter.emit(run_id, "message", serde_json::to_value(&msg).unwrap_or(json!({})));
        history.push(opening.clone());
    }

    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err("__cancelled__".into());
        }
        if game.finished.is_some() {
            break;
        }
        // 预算闸(复用 spec.budget.requests)。
        // 预算检查由调用方在转移前做过;这里每回合再核一次,触顶按平局收尾。
        let seat = game.turn;
        let member = spec
            .members
            .iter()
            .find(|m| m.id == game.players[seat].member_id)
            .ok_or("座位与成员对不上")?;
        // 视角 → 决策。
        let obs = player::observation(spec, &game, seat, &history);
        let (system, user) = player::decision_prompt(&obs);
        let req = crate::council::worker::WorkerRequest {
            model: member.model.clone(),
            system,
            user,
            max_tokens: Some(900),
        };
        let plan: TurnPlan = match client.complete(&req) {
            Ok(out) => {
                let est = estimate_tokens(out.prompt_chars + out.output_chars);
                bump_budget(run_id, est)?;
                player::parse_plan(&out.content).unwrap_or_default()
            }
            Err(e) => {
                // 决策失败:留痕并按「过」处理(摸2弃2 由 play_turn 的弃牌阶段兜底——
                // 注意 play_turn 不含摸牌,摸牌在回合开始做)。
                let note = format!("`{}` 决策调用失败({}),本回合自动过", member.id, e);
                history.push(note.clone());
                let msg = new_game_message(run_id, "system", Some(member.id.clone()), &note);
                store::append_message(run_id, &msg).ok();
                emitter.emit(run_id, "message", serde_json::to_value(&msg).unwrap_or(json!({})));
                TurnPlan::default()
            }
        };
        // 回合开始:摸 2 → 结算计划 → 事件落账。
        let mut events = Vec::new();
        game.draw_cards_public(seat, 2, &mut events);
        let before_round = game.round;
        game.play_turn(&plan, MAX_ROUNDS, &mut events);
        for ev in &mut events {
            ev.at = now_millis();
            let msg = new_game_message(run_id, &ev.actor, ev.target.clone(), &ev.text);
            store::append_message(run_id, &msg).ok();
            emitter.emit(run_id, "message", serde_json::to_value(&msg).unwrap_or(json!({})));
            history.push(ev.text.clone());
        }
        // 预算触顶检查(下一回合还要调用)。
        if let Some(state) = store::load_state(run_id).ok() {
            if let Some(max) = spec.budget.max_requests {
                if state.budget_usage.requests >= max {
                    game.finished = Some(engine::GameOutcome {
                        winner: Side::LordLoyalist,
                        summary: format!("请求预算触顶({max}),对局提前结束,判主公方胜(存活即守成)"),
                    });
                }
            }
        }
        save_game(run_id, &game, &history)?;
        // 回合变更事件(UI 轨道)。
        if game.round != before_round && game.finished.is_none() {
            emitter.emit(run_id, "phase", json!({ "phase": "playing", "round": game.round }));
        }
    }

    // 终局:身份全亮 + 战报。
    if let Some(outcome) = &game.finished {
        let mut reveal = String::from("终局身份揭示:");
        for p in &game.players {
            reveal.push_str(&format!(" `{}`={}", p.member_id, p.role.label()));
        }
        reveal.push_str(&format!("\n{}", outcome.summary));
        let msg = new_game_message(run_id, "system", None, &reveal);
        store::append_message(run_id, &msg).ok();
        emitter.emit(run_id, "message", serde_json::to_value(&msg).unwrap_or(json!({})));
        history.push(reveal.clone());
        save_game(run_id, &game, &history)?;
    }
    Ok(())
}

fn new_game_message(run_id: &str, actor: &str, target: Option<String>, text: &str) -> CouncilMessage {
    CouncilMessage {
        message_id: format!("gm-{}-{}", actor, now_millis() as u64 ^ text.len() as u64),
        run_id: run_id.to_string(),
        round: 0,
        from: actor.to_string(),
        to: target.unwrap_or_else(|| "all".to_string()),
        kind: MessageKind::GameAction,
        reply_to: None,
        content: text.to_string(),
        timestamp: now_millis(),
        meta: None,
    }
}

/// 累加预算用量(请求 +1,tokens 估算)。读-改-写走 state.json。
fn bump_budget(run_id: &str, tokens_est: u64) -> Result<(), String> {
    let mut state = store::load_state(run_id)?;
    state.budget_usage.requests += 1;
    state.budget_usage.tokens_est += tokens_est;
    if state.budget_usage.started_at == 0 {
        state.budget_usage.started_at = now_millis();
    }
    state.updated_at = now_millis();
    store::save_state(&state)
}
