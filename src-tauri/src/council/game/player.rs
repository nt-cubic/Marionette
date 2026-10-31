//! 游戏玩家代理:构造**玩家私有视角** → 模型决策 → 结构化计划解析。
//!
//! 信息隔离与圆桌审议同一思想:每个 agent 只看到自己该看的(私有手牌
//! 与身份;他人只有公开信息),视角由引擎外的这一层构造,引擎不信任
//! 模型输出——非法行动在 play_turn 里逐张校验跳过并留痕。

use super::engine::{Card, GameState, TurnPlan};
use crate::council::types::{CouncilSpec, MemberSpec};

/// 构造某玩家的私有视角文本(进 prompt)。
pub fn observation(_spec: &CouncilSpec, game: &GameState, seat: usize, history: &[String]) -> String {
    let me = &game.players[seat];
    let n = game.players.len();
    let mut out = String::new();
    out.push_str(&format!(
        "## 你的身份(只有你知道)\n你是「{}」。目标:{}\n",
        me.role.label(),
        match me.role {
            crate::council::game::engine::SgsRole::Lord => "活下去;消灭所有反贼与内奸。",
            super::engine::SgsRole::Loyalist => "保护主公;消灭所有反贼与内奸。",
            super::engine::SgsRole::Rebel => "杀死主公。",
            super::engine::SgsRole::Spy => "先借刀消灭反贼与忠臣,最后与主公单挑独活——主公死时若你不是唯一存活者,反贼赢;你要做最后一个活着的人。",
        }
    ));
    out.push_str(&format!(
        "\n## 你的状态\n座位 {}(共 {} 人,环状,距离 1 = 相邻);体力 {}/{}{};手牌:{}\n",
        seat + 1,
        n,
        me.hp,
        me.max_hp,
        if me.revealed { "(身份已亮)" } else { "" },
        if me.hand.is_empty() {
            "(无)".to_string()
        } else {
            me.hand.iter().map(|c| format!("【{}】", c.label())).collect::<Vec<_>>().join(" ")
        }
    ));
    out.push_str("\n## 全场公开信息\n");
    for (i, p) in game.players.iter().enumerate() {
        let rel = if i == seat { 0 } else { game.distance(seat, i) };
        let identity = if p.revealed { p.role.label().to_string() } else { "未知".to_string() };
        out.push_str(&format!(
            "- 座位{} `{}`:{}{} 体力 {}/{} 手牌 {}张 距你 {}\n",
            i + 1,
            p.member_id,
            identity,
            if p.alive { "" } else { "(阵亡)" },
            p.hp,
            p.max_hp,
            p.hand.len(),
            rel
        ));
    }
    out.push_str(&format!(
        "\n## 牌堆/弃牌\n牌堆剩 {} 张,弃牌堆 {} 张,当前第 {} 轮,轮到座位{} 出牌。\n",
        game.deck.len(),
        game.discard.len(),
        game.round,
        game.turn + 1
    ));
    if !history.is_empty() {
        out.push_str("\n## 最近战况(最新在下)\n");
        let start = history.len().saturating_sub(30);
        for h in &history[start..] {
            out.push_str(&format!("- {h}\n"));
        }
    }
    out
}

/// 决策 prompt:规则要点 + 视角 + 输出 schema。
pub fn decision_prompt(obs: &str) -> (String, String) {
    let system = "你在玩简化版三国杀身份局(无武将技能/无装备/无延时锦囊/无无懈;闪与濒死的桃由系统自动打出)。根据你的身份与场面制定**本回合完整计划**——你在扮演隐藏身份的玩家:发言可以误导,但出牌要服务你的真实胜利条件。".to_string();
    let user = format!(
        "{obs}\n\
         ## 可用行动\n\
         - 【杀】(每回合限1,目标须为距离1的相邻存活者;对方有闪则被闪避)\n\
         - 【桃】(自己体力未满时回复1点)\n\
         - 【无中生有】(摸2张)\n\
         - 【决斗】(任意存活者;双方轮流自动出杀,先无杀者受1伤)\n\
         - 【南蛮入侵】(其他所有人须出杀否则受1伤)/【万箭齐发】(其他所有人须出闪否则受1伤)\n\
         - 【过河拆桥】(任意存活者,弃其一张手牌)/【顺手牵羊】(距离1的存活者,获得其一张手牌)\n\
         - 【闪】是响应牌,不能主动使用\n\
         回合结束手牌须弃至体力值。\n\n\
         ## 输出(只输出一个 json 围栏)\n\
         ```json\n\
         {{\"plays\": [{{\"card\": \"sha|tao|wuzhong|juedou|nanman|wanjian|chaiqiao|shunshou\", \"target\": \"成员id或null\"}}],\n\
          \"discard\": [\"多余的牌名,如 shan\"],\n\
          \"say\": \"一句面向全场的发言(可演戏/可表忠/可指认,<=60字)\"}}\n\
         ```\n\
         plays 按顺序执行;没有合适行动就给空数组。"
    );
    (system, user)
}

/// 解析模型输出为 TurnPlan;失败时返回 None(runner 用空计划兜底——
/// 等价「摸2张后弃牌过」,游戏永不卡死)。
pub fn parse_plan(raw: &str) -> Option<TurnPlan> {
    let fence = raw
        .find("```json")
        .and_then(|start| raw[start + 7..].find("```").map(|end| &raw[start + 7..start + 7 + end]))
        .map(|s| s.trim())
        .unwrap_or(raw.trim());
    let v: serde_json::Value = serde_json::from_str(fence).ok()?;
    let mut plan = TurnPlan::default();
    match v.get("plays") {
        None => {} // 缺 plays = 本回合过(纯发言合法)
        Some(arr) => {
            let arr = match arr.as_array() { Some(a) => a, None => return None }; // 非数组=格式错误
            for item in arr {
                let card = match item.get("card").and_then(|c| c.as_str()) { Some(c) => c.to_string(), None => return None };
                let target = item
                    .get("target")
                    .and_then(|t| t.as_str())
                    .filter(|s| !s.is_empty() && *s != "null")
                    .map(|s| s.to_string());
                plan.plays.push(super::engine::PlannedPlay { card, target });
            }
        }
    }
    if let Some(arr) = v.get("discard").and_then(|d| d.as_array()) {
        plan.discard = arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
    }
    plan.say = v.get("say").and_then(|s| s.as_str()).map(|s| s.to_string());
    Some(plan)
}

/// 成员的座位提示文案(进 UI/模板说明)。
pub fn seat_hint(members: &[MemberSpec]) -> String {
    members
        .iter()
        .enumerate()
        .map(|(i, m)| format!("座位{}={}", i + 1, m.display_name))
        .collect::<Vec<_>>()
        .join("、")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::council::game::engine::{new_game, Lcg};

    fn spec_5() -> CouncilSpec {
        let mut spec = crate::council::types::tests::sample_spec();
        spec.members.extend([
            crate::council::types::MemberSpec {
                id: "p4".into(),
                display_name: "玩家四".into(),
                model: "m/4".into(),
                role: "r".into(),
                is_chair: false,
                tier: crate::council::types::MemberTier::Lead,
            },
            crate::council::types::MemberSpec {
                id: "p5".into(),
                display_name: "玩家五".into(),
                model: "m/5".into(),
                role: "r".into(),
                is_chair: false,
                tier: crate::council::types::MemberTier::Lead,
            },
        ]);
        spec
    }

    #[test]
    fn observation_hides_others_hands_and_roles() {
        let spec = spec_5();
        let ids: Vec<String> = spec.members.iter().map(|m| m.id.clone()).collect();
        let mut game = new_game(&ids, &mut Lcg(5)).unwrap();
        let obs = observation(&spec, &game, 2, &[]);
        // 自己身份与手牌可见。
        assert!(obs.contains("## 你的身份"));
        assert!(obs.contains("手牌:"));
        // 他人手牌只有数量:「手牌:」前缀全文只出现一次(自己那行)。
        assert_eq!(obs.matches("手牌:").count(), 1, "他人手牌内容不可见,只有『手牌 N张』");
        let lord = game.players.iter().find(|p| p.role == crate::council::game::engine::SgsRole::Lord).unwrap();
        let lord_seat = game.players.iter().position(|p| p.role == crate::council::game::engine::SgsRole::Lord).unwrap();
        if lord_seat != 0 {
            let obs0 = observation(&spec, &game, 0, &[]);
            assert!(obs0.contains(&format!("`{}`:主公", lord.member_id)), "主公身份对所有人公开");
            let _ = lord;
        }
        // 未知身份者显示「未知」。
        assert!(obs.contains("未知") || game.players.iter().filter(|p| !p.revealed).count() == 0);
    }

    #[test]
    fn observation_includes_distance_and_history() {
        let spec = spec_5();
        let ids: Vec<String> = spec.members.iter().map(|m| m.id.clone()).collect();
        let game = new_game(&ids, &mut Lcg(6)).unwrap();
        let obs = observation(&spec, &game, 0, &["p1 对 p2 使用【杀】".to_string()]);
        assert!(obs.contains("距你"));
        assert!(obs.contains("p1 对 p2 使用【杀】"));
    }

    #[test]
    fn parse_plan_accepts_full_and_rejects_garbage() {
        let plan = parse_plan(
            "```json\n{\"plays\":[{\"card\":\"sha\",\"target\":\"gpt-reviewer\"},{\"card\":\"wuzhong\",\"target\":null}],\"discard\":[\"shan\"],\"say\":\"此人就是反贼!\"}\n```",
        )
        .unwrap();
        assert_eq!(plan.plays.len(), 2);
        assert_eq!(plan.plays[0].target.as_deref(), Some("gpt-reviewer"));
        assert!(plan.plays[1].target.is_none());
        assert_eq!(plan.discard, vec!["shan".to_string()]);
        assert!(plan.say.unwrap().contains("反贼"));

        assert!(parse_plan("我觉得应该先杀他").is_none());
        assert!(parse_plan("{\"plays\": \"not-array\"}").is_none());
    }
}
