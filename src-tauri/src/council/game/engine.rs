//! 身份局游戏引擎(简化三国杀 v1)——纯逻辑,零 IO,可密集单测。
//!
//! 调研依据(AvalonBench / Werewolf 实证研究确立的范式):确定性引擎
//! 持有全部真相,LLM 玩家只拿到**自己视角的观测**(私有手牌 + 公开
//! 信息),决策以结构化 JSON 回传,引擎逐张校验结算——非法行动跳过
//! 并留痕,游戏永不因模型胡说而卡死。
//!
//! v1 简化(全部诚实标注在 docs/council/formation.md 与 user-guide):
//! 无武将技能(白板)、无装备、无延时锦囊、无无懈可击;
//! 防御响应(闪/决斗出杀/濒死吃桃)为**自动响应**(有牌即出),
//! 模型只在**自己的回合**做一次「整回合出牌计划」决策;
//! 桃仅自救;杀死反贼奖励摸 3;回合上限防死循环。

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SgsRole {
    Lord,
    Loyalist,
    Rebel,
    Spy,
}

impl SgsRole {
    pub fn label(&self) -> &'static str {
        match self {
            SgsRole::Lord => "主公",
            SgsRole::Loyalist => "忠臣",
            SgsRole::Rebel => "反贼",
            SgsRole::Spy => "内奸",
        }
    }
    pub fn key(&self) -> &'static str {
        match self {
            SgsRole::Lord => "lord",
            SgsRole::Loyalist => "loyalist",
            SgsRole::Rebel => "rebel",
            SgsRole::Spy => "spy",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Card {
    Sha,
    Shan,
    Tao,
    JueDou,
    WuZhong,
    NanMan,
    WanJian,
    ChaiQiao,
    ShunShou,
}

impl Card {
    pub fn label(&self) -> &'static str {
        match self {
            Card::Sha => "杀",
            Card::Shan => "闪",
            Card::Tao => "桃",
            Card::JueDou => "决斗",
            Card::WuZhong => "无中生有",
            Card::NanMan => "南蛮入侵",
            Card::WanJian => "万箭齐发",
            Card::ChaiQiao => "过河拆桥",
            Card::ShunShou => "顺手牵羊",
        }
    }
    pub fn key(&self) -> &'static str {
        match self {
            Card::Sha => "sha",
            Card::Shan => "shan",
            Card::Tao => "tao",
            Card::JueDou => "juedou",
            Card::WuZhong => "wuzhong",
            Card::NanMan => "nanman",
            Card::WanJian => "wanjian",
            Card::ChaiQiao => "chaiqiao",
            Card::ShunShou => "shunshou",
        }
    }
    pub fn from_key(key: &str) -> Option<Card> {
        Some(match key {
            "sha" => Card::Sha,
            "shan" => Card::Shan,
            "tao" => Card::Tao,
            "juedou" => Card::JueDou,
            "wuzhong" => Card::WuZhong,
            "nanman" => Card::NanMan,
            "wanjian" => Card::WanJian,
            "chaiqiao" => Card::ChaiQiao,
            "shunshou" => Card::ShunShou,
            _ => return None,
        })
    }
}

/// 标准牌堆(75 张;比例为标准军争前的身份局常用配置)。
pub fn standard_deck() -> Vec<Card> {
    let mut deck = Vec::new();
    for (card, n) in [
        (Card::Sha, 30),
        (Card::Shan, 15),
        (Card::Tao, 8),
        (Card::JueDou, 3),
        (Card::WuZhong, 4),
        (Card::NanMan, 3),
        (Card::WanJian, 1),
        (Card::ChaiQiao, 6),
        (Card::ShunShou, 5),
    ] {
        for _ in 0..n {
            deck.push(card);
        }
    }
    deck
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    pub member_id: String,
    pub role: SgsRole,
    pub revealed: bool,
    pub hp: i32,
    pub max_hp: i32,
    pub hand: Vec<Card>,
    pub alive: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    LordLoyalist,
    Rebel,
    Spy,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GameOutcome {
    pub winner: Side,
    /// 人话战报(身份全亮)。
    pub summary: String,
}

/// 一条已结算的游戏事件(runner 把它转成消息账本里的 GameAction)。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GameEvent {
    /// epoch 毫秒(runner 补)。
    pub at: i64,
    /// 行动者 member_id(系统结算可为 "system")。
    pub actor: String,
    /// 目标(可空)。
    pub target: Option<String>,
    /// 事件正文(人话,进时间线)。
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GameState {
    pub players: Vec<PlayerState>,
    pub deck: Vec<Card>,
    pub discard: Vec<Card>,
    /// 当前回合玩家下标(座次)。
    pub turn: usize,
    pub round: u32,
    pub finished: Option<GameOutcome>,
    /// 本回合已出的杀数(每回合限 1)。
    pub sha_played_this_turn: u32,
}

/// 身份分配:4 人 = 主/忠/反/内;5 人 = 主/忠/反/反/内(标准)。
/// 其他人数 v1 不支持(诚实拒绝而非乱配)。
pub fn assign_roles(member_ids: &[String]) -> Result<Vec<SgsRole>, String> {
    let roles = match member_ids.len() {
        4 => vec![SgsRole::Lord, SgsRole::Loyalist, SgsRole::Rebel, SgsRole::Spy],
        5 => vec![SgsRole::Lord, SgsRole::Loyalist, SgsRole::Rebel, SgsRole::Rebel, SgsRole::Spy],
        n => return Err(format!("身份局 v1 支持 4 或 5 名玩家,当前 {n} 人")),
    };
    Ok(roles)
}

/// 开局:洗牌(seed 可复现)、发初始手牌(主公 5 张 / 4 血,其余 4 张 / 3 血)。
pub fn new_game(member_ids: &[String], rng: &mut impl RngLike) -> Result<GameState, String> {
    let roles = assign_roles(member_ids)?;
    let mut deck = standard_deck();
    rng.shuffle(&mut deck);
    let mut players = Vec::new();
    for (id, role) in member_ids.iter().zip(roles) {
        let (max_hp, draw) = if role == SgsRole::Lord { (4, 5) } else { (3, 4) };
        let hand: Vec<Card> = deck.split_off(deck.len() - draw);
        players.push(PlayerState {
            member_id: id.clone(),
            role,
            revealed: role == SgsRole::Lord,
            hp: max_hp,
            max_hp,
            hand,
            alive: true,
        });
    }
    Ok(GameState {
        players,
        deck,
        discard: Vec::new(),
        turn: 0,
        round: 1,
        finished: None,
        sha_played_this_turn: 0,
    })
}

/// 极简 RNG 抽象(引擎不依赖外部 crate;runner 注入 LCG,测试注入固定序)。
pub trait RngLike {
    fn next_u64(&mut self) -> u64;
    fn shuffle<T>(&mut self, items: &mut [T]) {
        // Fisher–Yates,用 next_u64 取模。
        let mut i = items.len();
        while i > 1 {
            i -= 1;
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
    fn range(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }
}

/// 进程级可复现 LCG(种子=run 创建时间毫秒,由 runner 传入)。
pub struct Lcg(pub u64);
impl RngLike for Lcg {
    fn next_u64(&mut self) -> u64 {
        // Numerical Recipes 64-bit LCG。
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0
    }
}

impl GameState {
    /// 回合开始摸牌(runner 调用;事件进同一账本)。
    pub fn draw_cards_public(&mut self, player: usize, n: usize, events: &mut Vec<GameEvent>) {
        self.draw_cards(player, n, events);
    }

    pub fn current(&self) -> &PlayerState {
        &self.players[self.turn]
    }

    pub fn alive_indices(&self) -> Vec<usize> {
        self.players.iter().enumerate().filter(|(_, p)| p.alive).map(|(i, _)| i).collect()
    }

    /// 座位距离(环状,1 = 相邻)。
    pub fn distance(&self, from: usize, to: usize) -> usize {
        let n = self.players.len();
        let d = (from + n - to) % n;
        let d = d.min(n - d);
        d.max(1)
    }

    fn draw_cards(&mut self, player: usize, n: usize, events: &mut Vec<GameEvent>) {
        for _ in 0..n {
            if self.deck.is_empty() {
                if self.discard.is_empty() {
                    events.push(GameEvent {
                        at: 0,
                        actor: "system".into(),
                        target: None,
                        text: "牌堆与弃牌堆均空,无法再摸牌".into(),
                    });
                    return;
                }
                // 洗弃牌堆为新牌堆。
                let mut rng = Lcg(self.round as u64 ^ self.deck.len() as u64 ^ 0x9E3779B97F4A7C15);
                let mut revived = std::mem::take(&mut self.discard);
                rng.shuffle(&mut revived);
                self.deck = revived;
                events.push(GameEvent {
                    at: 0,
                    actor: "system".into(),
                    target: None,
                    text: "弃牌堆洗回牌堆".into(),
                });
            }
            let card = self.deck.pop().unwrap();
            self.players[player].hand.push(card);
        }
        events.push(GameEvent {
            at: 0,
            actor: "system".into(),
            target: Some(self.players[player].member_id.clone()),
            text: format!("{} 摸 {} 张牌(手牌 {})", self.players[player].member_id, n, self.players[player].hand.len()),
        });
    }

    /// 濒死结算:自动出桃自救(有桃即出;v1 简化)。返回 Some(死者下标)。
    fn dying_check(&mut self, player: usize, events: &mut Vec<GameEvent>) -> Option<usize> {
        if self.players[player].hp > 0 {
            return None;
        }
        let has_tao = self.players[player].hand.iter().any(|c| *c == Card::Tao);
        if has_tao {
            if let Some(pos) = self.players[player].hand.iter().position(|c| *c == Card::Tao) {
                let tao = self.players[player].hand.remove(pos);
                self.discard.push(tao);
                self.players[player].hp = 1;
                events.push(GameEvent {
                    at: 0,
                    actor: self.players[player].member_id.clone(),
                    target: Some(self.players[player].member_id.clone()),
                    text: format!("{} 濒死,自动使用【桃】恢复至 1 体力", self.players[player].member_id),
                });
                return None;
            }
        }
        // 死亡:亮身份,弃全部手牌。
        self.players[player].alive = false;
        self.players[player].revealed = true;
        let role = self.players[player].role;
        let id = self.players[player].member_id.clone();
        let cards = std::mem::take(&mut self.players[player].hand);
        self.discard.extend(cards);
        events.push(GameEvent {
            at: 0,
            actor: "system".into(),
            target: Some(id.clone()),
            text: format!("{} 阵亡,身份亮明:{}", id, role.label()),
        });
        Some(player)
    }

    /// 伤害 + 濒死 + 击杀奖惩(杀反贼者摸 3)。
    fn damage(&mut self, target: usize, amount: i32, events: &mut Vec<GameEvent>, killer: Option<usize>) {
        let id = self.players[target].member_id.clone();
        self.players[target].hp -= amount;
        events.push(GameEvent {
            at: 0,
            actor: "system".into(),
            target: Some(id.clone()),
            text: format!("{} 受 {} 点伤害(体力 {})", id, amount, self.players[target].hp),
        });
        if let Some(dead) = self.dying_check(target, events) {
            if let Some(k) = killer {
                if self.players[dead].role == SgsRole::Rebel {
                    let kid = self.players[k].member_id.clone();
                    events.push(GameEvent {
                        at: 0,
                        actor: "system".into(),
                        target: Some(kid.clone()),
                        text: format!("{} 击杀反贼,奖励摸 3 张", kid),
                    });
                    self.draw_cards(k, 3, events);
                }
            }
        }
    }

    /// 结算一张已宣告的行动(引擎已校验合法)。返回是否继续处理后续。
    fn resolve_card(&mut self, actor: usize, card: Card, target: Option<usize>, events: &mut Vec<GameEvent>) {
        let actor_id = self.players[actor].member_id.clone();
        // 出牌进弃牌堆由各分支处理。
        match card {
            Card::Sha => {
                self.sha_played_this_turn += 1;
                // 精确移除一张杀。
                if let Some(pos) = self.players[actor].hand.iter().position(|c| *c == Card::Sha) {
                    self.discard.push(self.players[actor].hand.remove(pos));
                }
                let t = target.unwrap();
                let tid = self.players[t].member_id.clone();
                events.push(GameEvent {
                    at: 0,
                    actor: actor_id.clone(),
                    target: Some(tid.clone()),
                    text: format!("{} 对 {} 使用【杀】", actor_id, tid),
                });
                // 自动响应:有闪即出。
                if let Some(pos) = self.players[t].hand.iter().position(|c| *c == Card::Shan) {
                    self.discard.push(self.players[t].hand.remove(pos));
                    events.push(GameEvent {
                        at: 0,
                        actor: tid.clone(),
                        target: Some(tid.clone()),
                        text: format!("{} 自动打出【闪】,【杀】被闪避", tid),
                    });
                } else {
                    self.damage(t, 1, events, Some(actor));
                }
            }
            Card::Tao => {
                if let Some(pos) = self.players[actor].hand.iter().position(|c| *c == Card::Tao) {
                    self.discard.push(self.players[actor].hand.remove(pos));
                }
                self.players[actor].hp = (self.players[actor].hp + 1).min(self.players[actor].max_hp);
                events.push(GameEvent {
                    at: 0,
                    actor: actor_id.clone(),
                    target: Some(actor_id.clone()),
                    text: format!("{} 使用【桃】,体力回复至 {}", actor_id, self.players[actor].hp),
                });
            }
            Card::WuZhong => {
                if let Some(pos) = self.players[actor].hand.iter().position(|c| *c == Card::WuZhong) {
                    self.discard.push(self.players[actor].hand.remove(pos));
                }
                events.push(GameEvent {
                    at: 0,
                    actor: actor_id.clone(),
                    target: Some(actor_id.clone()),
                    text: format!("{} 使用【无中生有】", actor_id),
                });
                self.draw_cards(actor, 2, events);
            }
            Card::JueDou => {
                if let Some(pos) = self.players[actor].hand.iter().position(|c| *c == Card::JueDou) {
                    self.discard.push(self.players[actor].hand.remove(pos));
                }
                let t = target.unwrap();
                let tid = self.players[t].member_id.clone();
                events.push(GameEvent {
                    at: 0,
                    actor: actor_id.clone(),
                    target: Some(tid.clone()),
                    text: format!("{} 对 {} 使用【决斗】", actor_id, tid),
                });
                // 从目标开始轮流自动出杀,先无杀者受 1 伤。
                let mut turn_side_is_target = true;
                loop {
                    let side = if turn_side_is_target { t } else { actor };
                    if let Some(pos) = self.players[side].hand.iter().position(|c| *c == Card::Sha) {
                        self.discard.push(self.players[side].hand.remove(pos));
                        events.push(GameEvent {
                            at: 0,
                            actor: self.players[side].member_id.clone(),
                            target: Some(tid.clone()),
                            text: format!("{} 自动打出【杀】应战", self.players[side].member_id),
                        });
                        turn_side_is_target = !turn_side_is_target;
                    } else {
                        let loser = side;
                        events.push(GameEvent {
                            at: 0,
                            actor: "system".into(),
                            target: Some(self.players[loser].member_id.clone()),
                            text: format!("{} 无【杀】应战", self.players[loser].member_id),
                        });
                        let opponent = if turn_side_is_target { actor } else { t };
                        self.damage(loser, 1, events, Some(opponent));
                        break;
                    }
                }
            }
            Card::NanMan | Card::WanJian => {
                let is_nanman = card == Card::NanMan;
                if let Some(pos) = self.players[actor].hand.iter().position(|c| *c == card) {
                    self.discard.push(self.players[actor].hand.remove(pos));
                }
                events.push(GameEvent {
                    at: 0,
                    actor: actor_id.clone(),
                    target: None,
                    text: format!("{} 使用【{}】", actor_id, card.label()),
                });
                let need = if is_nanman { Card::Sha } else { Card::Shan };
                let name = if is_nanman { "杀" } else { "闪" };
                // 按座次结算其他存活玩家。
                let others: Vec<usize> = self
                    .players
                    .iter()
                    .enumerate()
                    .filter(|(i, p)| *i != actor && p.alive)
                    .map(|(i, _)| i)
                    .collect();
                for t in others {
                    if !self.players[t].alive {
                        continue; // 可能已被本张锦囊打死(连锁濒死不会,但防御性)
                    }
                    if let Some(pos) = self.players[t].hand.iter().position(|c| *c == need) {
                        self.discard.push(self.players[t].hand.remove(pos));
                        events.push(GameEvent {
                            at: 0,
                            actor: self.players[t].member_id.clone(),
                            target: Some(self.players[t].member_id.clone()),
                            text: format!("{} 自动打出【{}】响应", self.players[t].member_id, name),
                        });
                    } else {
                        self.damage(t, 1, events, Some(actor));
                    }
                }
            }
            Card::ChaiQiao | Card::ShunShou => {
                let is_shun = card == Card::ShunShou;
                if let Some(pos) = self.players[actor].hand.iter().position(|c| *c == card) {
                    self.discard.push(self.players[actor].hand.remove(pos));
                }
                let t = target.unwrap();
                let tid = self.players[t].member_id.clone();
                events.push(GameEvent {
                    at: 0,
                    actor: actor_id.clone(),
                    target: Some(tid.clone()),
                    text: format!("{} 对 {} 使用【{}】", actor_id, tid, card.label()),
                });
                if !self.players[t].hand.is_empty() {
                    // 拆/顺:丢弃牌堆顶(对方手牌随机张——信息上等价,结算最简)。
                    let pos = self.players[t].hand.len() - 1;
                    let got = self.players[t].hand.remove(pos);
                    if is_shun {
                        self.players[actor].hand.push(got);
                        events.push(GameEvent {
                            at: 0,
                            actor: "system".into(),
                            target: Some(tid.clone()),
                            text: format!("{} 获得对方一张手牌(【{}】)", actor_id, got.label()),
                        });
                    } else {
                        self.discard.push(got);
                        events.push(GameEvent {
                            at: 0,
                            actor: "system".into(),
                            target: Some(tid.clone()),
                            text: format!("{} 的一张手牌被弃置", tid),
                        });
                    }
                }
            }
            Card::Shan => {
                // 闪不是主动牌:合法性校验应已拒绝,防御性落牌。
                events.push(GameEvent {
                    at: 0,
                    actor: actor_id,
                    target: None,
                    text: "【闪】只能在被【杀】指定时打出,已退回".into(),
                });
            }
        }
    }

    /// 校验一张计划行动是否合法(玩家视角可解释的原因)。
    pub fn validate_play(&self, actor: usize, card: Card, target: Option<usize>) -> Result<(), String> {
        if self.finished.is_some() {
            return Err("对局已结束".into());
        }
        let p = &self.players[actor];
        if !p.alive {
            return Err("你已阵亡".into());
        }
        if !p.hand.contains(&card) {
            return Err(format!("你没有【{}】", card.label()));
        }
        match card {
            Card::Sha => {
                if self.sha_played_this_turn >= 1 {
                    return Err("每回合只能使用一张【杀】".into());
                }
                let t = target.ok_or("【杀】需要指定目标")?;
                if t == actor {
                    return Err("不能对自己使用【杀】".into());
                }
                if !self.players[t].alive {
                    return Err("目标已阵亡".into());
                }
                if self.distance(actor, t) > 1 {
                    return Err("【杀】只能指定距离 1 的相邻玩家".into());
                }
            }
            Card::Tao => {
                if p.hp >= p.max_hp {
                    return Err("体力已满,【桃】无处可用".into());
                }
            }
            Card::JueDou | Card::ChaiQiao => {
                let t = target.ok_or_else(|| format!("【{}】需要指定目标", card.label()))?;
                if t == actor {
                    return Err("不能对自己使用".into());
                }
                if !self.players[t].alive {
                    return Err("目标已阵亡".into());
                }
            }
            Card::ShunShou => {
                let t = target.ok_or("【顺手牵羊】需要指定目标")?;
                if t == actor {
                    return Err("不能对自己使用".into());
                }
                if !self.players[t].alive {
                    return Err("目标已阵亡".into());
                }
                if self.distance(actor, t) > 1 {
                    return Err("【顺手牵羊】只能指定距离 1 的相邻玩家".into());
                }
                if self.players[t].hand.is_empty() {
                    return Err("目标没有手牌".into());
                }
            }
            Card::WuZhong | Card::NanMan | Card::WanJian => {}
            Card::Shan => return Err("【闪】是响应牌,不能主动使用".into()),
        }
        Ok(())
    }

    /// 结算一个完整回合计划:逐张校验(非法跳过并留痕)→ 弃牌阶段(自动
    /// 弃至体力数,弃哪些由计划 discard 指定,不足/非法自动补)→ 胜负检查。
    pub fn play_turn(
        &mut self,
        plan: &TurnPlan,
        max_rounds: u32,
        events: &mut Vec<GameEvent>,
    ) {
        let actor = self.turn;
        let actor_id = self.players[actor].member_id.clone();
        self.sha_played_this_turn = 0;
        if let Some(say) = plan.say.as_deref().filter(|s| !s.trim().is_empty()) {
            let say: String = say.chars().take(300).collect();
            events.push(GameEvent {
                at: 0,
                actor: actor_id.clone(),
                target: None,
                text: format!("{} 发言:「{}」", actor_id, say),
            });
        }
        for play in &plan.plays {
            if self.finished.is_some() || !self.players[actor].alive {
                break;
            }
            let card = match Card::from_key(&play.card) {
                Some(c) => c,
                None => {
                    events.push(GameEvent {
                        at: 0,
                        actor: actor_id.clone(),
                        target: None,
                        text: format!("未知牌名「{}」,跳过", play.card),
                    });
                    continue;
                }
            };
            let target = match play.target.as_deref().and_then(|id| self.players.iter().position(|p| p.member_id == id)) {
                Some(t) => Some(t),
                None => None,
            };
            match self.validate_play(actor, card, target) {
                Ok(()) => self.resolve_card(actor, card, target, events),
                Err(reason) => events.push(GameEvent {
                    at: 0,
                    actor: actor_id.clone(),
                    target: play.target.clone(),
                    text: format!("尝试使用【{}】失败:{}", card.label(), reason),
                }),
            }
            if self.check_finished().is_some() {
                break;
            }
        }
        if self.finished.is_some() {
            return;
        }
        // 弃牌阶段:手牌 > 体力 → 按计划 discard(可校验的牌名)先弃,再自动补。
        let over = self.players[actor].hand.len() as i32 - self.players[actor].hp;
        if over > 0 {
            let mut discarded = 0;
            let mut wanted: Vec<Card> = plan
                .discard
                .iter()
                .filter_map(|k| Card::from_key(k))
                .collect();
            while discarded < over {
                let take = wanted.pop().or_else(|| self.players[actor].hand.last().copied());
                match take {
                    Some(c) => {
                        if let Some(pos) = self.players[actor].hand.iter().position(|x| *x == c) {
                            self.discard.push(self.players[actor].hand.remove(pos));
                            discarded += 1;
                        } else {
                            // 计划里的牌不在手:自动改弃手牌末张。
                            if let Some(last) = self.players[actor].hand.pop() {
                                self.discard.push(last);
                                discarded += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    None => break,
                }
            }
            events.push(GameEvent {
                at: 0,
                actor: actor_id.clone(),
                target: Some(actor_id.clone()),
                text: format!("{} 弃牌至体力上限(手牌 {})", actor_id, self.players[actor].hand.len()),
            });
        }
        // 胜负(反贼+内奸全灭)。
        if self.check_finished().is_some() {
            return;
        }
        // 回合推进:跳过阵亡者;一圈回来 round+1;上限判主忠胜。
        loop {
            self.turn = (self.turn + 1) % self.players.len();
            if self.players[self.turn].alive {
                break;
            }
        }
        if self.turn == 0 || self.round_end_passed() {
            self.round += 1;
        }
        if self.round > max_rounds {
            self.finished = Some(GameOutcome {
                winner: Side::LordLoyalist,
                summary: format!("达到回合上限({max_rounds}),判主公方胜(存活即守成)"),
            });
        }
    }

    fn round_end_passed(&self) -> bool {
        // turn 回到座次最小存活者即算过圈;v1 用简单近似:turn==0。
        self.turn == 0
    }

    /// 胜负检查(主公死/反贼+内奸全灭)。返回 Some(outcome) 并写入状态。
    pub fn check_finished(&mut self) -> Option<GameOutcome> {
        let lord_alive = self.players.iter().any(|p| p.role == SgsRole::Lord && p.alive);
        let rebels_alive = self.players.iter().any(|p| p.role == SgsRole::Rebel && p.alive);
        let spy_alive = self.players.iter().any(|p| p.role == SgsRole::Spy && p.alive);
        let outcome = if !lord_alive {
            let alive_roles: Vec<&str> = self.players.iter().filter(|p| p.alive).map(|p| p.role.label()).collect();
            if !spy_alive || alive_roles.len() != 1 {
                Some((Side::Rebel, format!("主公阵亡,反贼阵营获胜(存活:{})", alive_roles.join("、"))))
            } else {
                Some((Side::Spy, "主公阵亡且内奸最后独活——内奸获胜".to_string()))
            }
        } else if !rebels_alive && !spy_alive {
            Some((Side::LordLoyalist, "反贼与内奸全部阵亡——主公方获胜".to_string()))
        } else {
            None
        };
        if let Some((winner, summary)) = outcome {
            self.finished = Some(GameOutcome { winner, summary });
            self.players.iter_mut().for_each(|p| p.revealed = true);
        }
        self.finished.clone()
    }
}

/// 模型回传的整回合计划。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlan {
    #[serde(default)]
    pub plays: Vec<PlannedPlay>,
    #[serde(default)]
    pub discard: Vec<String>,
    #[serde(default)]
    pub say: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlannedPlay {
    pub card: String,
    pub target: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedRng(u64);
    impl RngLike for FixedRng {
        fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(1);
            self.0
        }
    }

    fn ids5() -> Vec<String> {
        ["p1", "p2", "p3", "p4", "p5"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn roles_and_deal_are_standard() {
        let mut rng = FixedRng(42);
        let g = new_game(&ids5(), &mut rng).unwrap();
        assert_eq!(g.players.len(), 5);
        assert_eq!(g.players.iter().filter(|p| p.role == SgsRole::Lord).count(), 1);
        assert_eq!(g.players.iter().filter(|p| p.role == SgsRole::Rebel).count(), 2);
        assert_eq!(g.players.iter().filter(|p| p.role == SgsRole::Spy).count(), 1);
        let lord = g.players.iter().find(|p| p.role == SgsRole::Lord).unwrap();
        assert_eq!((lord.max_hp, lord.hand.len()), (4, 5));
        assert!(lord.revealed);
        assert!(!g.players[1].revealed, "非主公身份暗置");
        assert_eq!(g.players[1].hand.len(), 4);
        // 75 张发 21 张。
        assert_eq!(g.deck.len(), 75 - 21);
        // 4 人局也支持;3 人拒绝。
        assert!(new_game(&ids5()[..4].to_vec(), &mut FixedRng(1)).is_ok());
        assert!(new_game(&ids5()[..3].to_vec(), &mut FixedRng(1)).is_err());
    }

    #[test]
    fn distance_is_circular() {
        let mut rng = FixedRng(7);
        let g = new_game(&ids5(), &mut rng).unwrap();
        assert_eq!(g.distance(0, 1), 1);
        assert_eq!(g.distance(0, 4), 1, "环状:相邻两侧都是 1");
        assert_eq!(g.distance(0, 2), 2);
        assert_eq!(g.distance(0, 0), 1, "自身距离定义为 1(不可对自己出杀由 validate 拦)");
    }

    #[test]
    fn sha_rules_enforced() {
        let mut rng = FixedRng(3);
        let mut g = new_game(&ids5(), &mut rng).unwrap();
        g.turn = 0;
        g.players[0].hand = vec![Card::Sha, Card::Sha];
        // 距离 2 的目标被拒。
        assert!(g.validate_play(0, Card::Sha, Some(2)).is_err());
        // 相邻 OK。
        assert!(g.validate_play(0, Card::Sha, Some(1)).is_ok());
        // 打出一张后第二张被拒(每回合限一)。
        let mut events = Vec::new();
        g.resolve_card(0, Card::Sha, Some(1), &mut events);
        assert!(g.validate_play(0, Card::Sha, Some(1)).is_err());
    }

    #[test]
    fn sha_blocked_by_shan_and_tao_saves_dying() {
        let mut rng = FixedRng(9);
        let mut g = new_game(&ids5(), &mut rng).unwrap();
        // 构造:0 杀 1(相邻);1 有闪 → 无伤。
        g.players[0].hand = vec![Card::Sha];
        g.players[1].hand = vec![Card::Shan];
        g.players[1].hp = 1;
        let mut events = Vec::new();
        g.resolve_card(0, Card::Sha, Some(1), &mut events);
        assert_eq!(g.players[1].hp, 1, "闪避成功");
        assert!(g.players[1].alive);
        // 1 无闪有桃 → 伤害触发濒死自动桃。
        g.players[0].hand = vec![Card::Sha];
        g.sha_played_this_turn = 0;
        g.players[1].hand = vec![Card::Tao];
        g.resolve_card(0, Card::Sha, Some(1), &mut events);
        assert_eq!(g.players[1].hp, 1, "濒死自动桃回到 1");
        // 1 无牌 → 阵亡亮明反贼身份,0 摸 3。
        g.players[0].hand = vec![Card::Sha];
        g.sha_played_this_turn = 0;
        g.players[1].hand = vec![];
        g.players[1].role = SgsRole::Rebel;
        let before = g.players[0].hand.len();
        g.resolve_card(0, Card::Sha, Some(1), &mut events);
        assert!(!g.players[1].alive);
        assert!(g.players[1].revealed);
        assert_eq!(g.players[0].hand.len(), before - 1 + 3, "杀消耗1张+奖励摸3");
    }

    #[test]
    fn lord_death_ends_game_with_spy_or_rebel_win() {
        let mut rng = FixedRng(11);
        let mut g = new_game(&ids5(), &mut rng).unwrap();
        // 只留主公 + 内奸,主公死 → 内奸胜。
        for (i, p) in g.players.iter_mut().enumerate() {
            p.alive = i == 0 || p.role == SgsRole::Spy;
        }
        g.players[0].hp = 0;
        g.players[0].hand = vec![];
        let mut events = Vec::new();
        g.dying_check(0, &mut events);
        let out = g.check_finished().unwrap();
        assert_eq!(out.winner, Side::Spy);
        // 主公死但反贼也活 → 反贼胜。
        let mut g2 = new_game(&ids5(), &mut FixedRng(12)).unwrap();
        g2.players[0].hp = 0;
        g2.players[0].hand = vec![];
        g2.dying_check(0, &mut events);
        let out2 = g2.check_finished().unwrap();
        assert_eq!(out2.winner, Side::Rebel);
    }

    #[test]
    fn rebel_spy_wipe_is_lord_win() {
        let mut rng = FixedRng(13);
        let mut g = new_game(&ids5(), &mut rng).unwrap();
        for p in g.players.iter_mut() {
            if p.role == SgsRole::Rebel || p.role == SgsRole::Spy {
                p.alive = false;
            }
        }
        let out = g.check_finished().unwrap();
        assert_eq!(out.winner, Side::LordLoyalist);
    }

    #[test]
    fn play_turn_discards_to_hp_and_advances() {
        let mut rng = FixedRng(21);
        let mut g = new_game(&ids5(), &mut rng).unwrap();
        g.turn = 0;
        g.players[0].hand = vec![Card::Shan, Card::Shan, Card::Shan, Card::Shan, Card::Shan];
        g.players[0].hp = 3;
        let plan = TurnPlan { plays: vec![], discard: vec!["shan".into()], say: Some("先观望".into()) };
        let mut events = Vec::new();
        g.play_turn(&plan, 12, &mut events);
        assert_eq!(g.players[0].hand.len(), 3, "弃至体力 3");
        assert_eq!(g.turn, 1, "轮到下家");
        assert!(events.iter().any(|e| e.text.contains("发言")));
    }

    #[test]
    fn nanman_asks_all_others_for_sha() {
        let mut rng = FixedRng(25);
        let mut g = new_game(&ids5(), &mut rng).unwrap();
        g.players[0].hand = vec![Card::NanMan];
        for (i, p) in g.players.iter_mut().enumerate() {
            if i == 0 { continue; }
            p.hand = if i == 1 { vec![Card::Sha] } else { vec![] };
            p.hp = 1;
        }
        let mut events = Vec::new();
        g.resolve_card(0, Card::NanMan, None, &mut events);
        assert_eq!(g.players[1].hp, 1, "出杀者无伤");
        assert!(!g.players[2].alive && !g.players[3].alive && !g.players[4].alive, "无杀者全部阵亡");
        // 全灭的可能是反贼+内奸 → 对局应已结束(由 resolve 链路中的检查或显式调用)。
        g.check_finished();
        assert!(g.finished.is_some());
    }

    #[test]
    fn invalid_plays_are_skipped_with_reasons() {
        let mut rng = FixedRng(31);
        let mut g = new_game(&ids5(), &mut rng).unwrap();
        g.turn = 0;
        g.players[0].hand = vec![Card::Shan, Card::Tao];
        g.players[0].hp = g.players[0].max_hp;
        let plan = TurnPlan {
            plays: vec![
                PlannedPlay { card: "fireball".into(), target: None },          // 未知牌
                PlannedPlay { card: "shan".into(), target: None },              // 响应牌
                PlannedPlay { card: "tao".into(), target: None },               // 满血
                PlannedPlay { card: "sha".into(), target: Some("nobody".into()) }, // 无此玩家
            ],
            discard: vec![],
            say: None,
        };
        let mut events = Vec::new();
        g.play_turn(&plan, 12, &mut events);
        assert!(events.iter().any(|e| e.text.contains("未知牌名")));
        assert!(events.iter().any(|e| e.text.contains("响应牌")));
        assert!(events.iter().any(|e| e.text.contains("体力已满")));
        assert!(events.iter().any(|e| e.text.contains("失败")), "非法行动都留痕");
        assert!(g.players[0].hand.contains(&Card::Shan), "未非法消耗手牌");
    }

    #[test]
    fn deck_reshuffles_from_discard() {
        let mut rng = FixedRng(37);
        let mut g = new_game(&ids5(), &mut rng).unwrap();
        g.deck.clear();
        g.discard = vec![Card::Sha, Card::Tao, Card::Shan];
        let mut events = Vec::new();
        g.draw_cards(0, 2, &mut events);
        assert_eq!(g.players[0].hand.len(), 5 + 2, "洗回后摸到牌(座次0是主公,起手5张)");
        assert!(events.iter().any(|e| e.text.contains("洗回")));
    }

    #[test]
    fn round_cap_forces_lord_win() {
        let mut rng = FixedRng(41);
        let mut g = new_game(&ids5(), &mut rng).unwrap();
        // 无人出牌,纯 pass 若干回合。
        let plan = TurnPlan::default();
        let mut events = Vec::new();
        for _ in 0..(3 * 12 + 1) {
            if g.finished.is_some() { break; }
            g.play_turn(&plan, 3, &mut events);
        }
        assert!(g.finished.is_some());
        assert_eq!(g.finished.unwrap().winner, Side::LordLoyalist);
    }
}
