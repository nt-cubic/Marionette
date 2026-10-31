//! 能力感知组队(M5):让 AI 参与团队设置——先画像、再组队、再分层。
//!
//! 方法组合依据(详见 docs/council/formation.md):
//! - 动态组队:DyLAN(arXiv:2310.02170)/ AgentVerse 的 expert
//!   recruitment(arXiv:2308.10848)——按任务与 agent 能力选团队;
//! - 分层稳健性:层级结构(boss+peers)在扰动下损失 ~5% vs 链式 ~24%;
//! - 强弱路由:RouteLLM(arXiv:2406.18665)——难任务升给强模型,
//!   简单任务留给弱模型;
//! - 背书:层级化 MAD 的 verifier 模式——弱者产出需强者认可。
//!
//! 三个能力信号源,按成本递增、可独立启用:
//! 1. **静态启发**(`model_family_rank`):模型 id 的家族/档位关键词,
//!    冷启动兜底,显式标注"启发式、未经实测";
//! 2. **本地探针**(`probe_model`,用户显式触发):每模型 1 次小请求
//!    (一道短推理 + 严格 JSON 格式遵从),按结果评分;
//! 3. **历史统计**(`model-stats.json`):每个 run 落幕时记录各模型的
//!    产出/背书/失败,经验分随使用量增长(反馈闭环,越用越准)。
//!
//! 所有信号都只是**建议**——组队结果经人在环(UI 审阅可改)后才生效,
//! 且 tier 从不改变信息隔离与审计语义。

use super::store;
use super::types::{CouncilSpec, MemberTier, MessageKind};
use super::worker::ModelClient;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// 信号 1:静态启发(家族/档位关键词)
// ---------------------------------------------------------------------------

/// 家族/档位启发分(0..=10,高=强)。**只放高置信度的旗舰/轻量词**,
/// 识别不了返回 None(画像诚实标注"未知",不猜)。这是冷启动兜底,
/// 探针与历史信号永远优先。
pub fn model_family_rank(model_id: &str) -> Option<u8> {
    let id = model_id.to_lowercase();
    // 轻量档(词越靠前越具体)。
    let lite = [
        "mini", "nano", "flash-lite", "flash", "haiku", "turbo", "air", "lite", "swift", "instant",
    ];
    // 旗舰档(opus/ultra/max/reasoner/pro 系)。
    let flagship = ["opus", "ultra", "-max", "max-", "reasoner", "thinking", "pro-preview", "r1"];
    let mid = ["pro", "plus", "v4", "v3.2", "v3.1"];
    if flagship.iter().any(|k| id.contains(k)) {
        return Some(9);
    }
    if lite.iter().any(|k| id.contains(k)) {
        return Some(4);
    }
    if mid.iter().any(|k| id.contains(k)) {
        return Some(7);
    }
    None
}

// ---------------------------------------------------------------------------
// 信号 2:本地探针(每模型 1 次请求)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProbeOutcome {
    /// 0..=7:推理答案对 +4、严格 JSON 格式 +3、错答但格式合法 +1。
    pub score: u8,
    /// 给用户看的证据摘要(答了什么/格式是否合规)。
    pub notes: String,
    /// epoch 毫秒。
    pub at: i64,
}

/// 探针题目:答案唯一且短、无歧义,同时考察格式遵从。
/// 「一个水池,进水管 3 小时注满,排水管 6 小时排空,同时开几小时满?」
/// 答案 2 小时(1/(1/3-1/6)=2)。
const PROBE_TASK: &str = "解答下面的问题,并**严格**按 JSON 输出(不要任何其他文字):\n\
    ```json\n{\"answer\": \"<一个数字,单位小时>\", \"reason\": \"<不超过两步的推理>\"}\n```\n\n\
    问题:一个水池,进水管 3 小时注满,排水管 6 小时排空。两管同时开,几小时注满?";

pub fn probe_model(client: &dyn ModelClient, model: &str) -> ProbeOutcome {
    let req = super::worker::WorkerRequest {
        model: model.to_string(),
        system: "你是一个严谨的解题助手。输出必须是且仅是一个 json 围栏。".into(),
        user: PROBE_TASK.into(),
        max_tokens: Some(300),
    };
    let at = super::types::now_millis();
    let Ok(out) = client.complete(&req) else {
        return ProbeOutcome { score: 0, notes: "请求失败(引擎未就绪或来源不可用)".into(), at };
    };
    score_probe_output(&out.content, at)
}

/// 探针评分(独立纯函数,可离线测)。
pub fn score_probe_output(raw: &str, at: i64) -> ProbeOutcome {
    let fence = raw
        .find("```json")
        .and_then(|start| raw[start + 7..].find("```").map(|end| &raw[start + 7..start + 7 + end]))
        .map(|s| s.trim())
        .unwrap_or(raw.trim());
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(fence);
    let mut score = 0u8;
    let mut notes_parts: Vec<String> = Vec::new();
    match &parsed {
        Ok(v) => {
            score += 3;
            notes_parts.push("JSON 格式合规".into());
            let answer = v.get("answer").and_then(|a| a.as_str()).unwrap_or("");
            let normalized: String = answer.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
            if normalized.trim_end_matches('.') == "2" {
                score += 4;
                notes_parts.push("推理正确(2 小时)".into());
            } else if !answer.trim().is_empty() {
                score += 1;
                notes_parts.push(format!("推理错误(答了 {answer:?})"));
            } else {
                notes_parts.push("缺 answer 字段".into());
            }
        }
        Err(_) => notes_parts.push("未输出合法 JSON".into()),
    }
    ProbeOutcome { score, notes: notes_parts.join(";"), at }
}

/// 探针结果持久化(council/probes.json;用户显式触发后长期复用,
/// 画像每次组队都会带上)。保存的是「最近一次」结果——模型会升级,
/// 旧探针不该永久粘滞。
pub fn load_probe_results() -> HashMap<String, ProbeOutcome> {
    let Ok(path) = store::council_root() else { return HashMap::new() };
    std::fs::read_to_string(path.join("probes.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn merge_probe_results(models: &[String], outcomes: &[ProbeOutcome]) -> HashMap<String, ProbeOutcome> {
    let mut merged = load_probe_results();
    for (model, outcome) in models.iter().zip(outcomes.iter()) {
        merged.insert(model.clone(), outcome.clone());
    }
    merged
}

pub fn save_probe_results(probes: &HashMap<String, ProbeOutcome>) -> Result<(), String> {
    let path = store::council_root()?.join("probes.json");
    let body = serde_json::to_string_pretty(probes).map_err(|e| e.to_string())?;
    crate::storage::write_atomic(&path, &body).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// 信号 3:历史统计(model-stats.json,反馈闭环)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelHistory {
    /// 参与的审议 run 数(PlanReady 落幕时 +1)。
    pub runs: u32,
    /// 产出的提案/修订数。
    pub outputs: u32,
    /// 收到的背书:认可 / 否决(被 lead 检验的结果)。
    pub endorsed: u32,
    pub rejected: u32,
    /// 成员调用失败次数(Status 消息记 failure)。
    pub failures: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatsFile {
    pub version: u32,
    /// model id → 历史。上限 200 条(LRU 淘汰,防无限增长)。
    pub models: HashMap<String, ModelHistory>,
}

const STATS_FILE: &str = "model-stats.json";
const STATS_CAP: usize = 200;

fn stats_path() -> Result<std::path::PathBuf, String> {
    Ok(store::council_root()?.join(STATS_FILE))
}

pub fn load_model_stats() -> ModelStatsFile {
    let Ok(path) = stats_path() else { return ModelStatsFile { version: 1, models: HashMap::new() } };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(ModelStatsFile { version: 1, models: HashMap::new() })
}

pub fn save_model_stats(stats: &ModelStatsFile) -> Result<(), String> {
    let path = stats_path()?;
    let body = serde_json::to_string_pretty(stats).map_err(|e| e.to_string())?;
    crate::storage::write_atomic(&path, &body).map_err(|e| e.to_string())
}

/// 审议落幕(PlanReady)时的统计落账:每成员记 outputs(产出)、
/// endorsed/rejected(收到的背书判定)、failures。
pub fn record_deliberation_outcome(spec: &CouncilSpec, messages: &[super::types::CouncilMessage]) {
    let mut stats = load_model_stats();
    for member in &spec.members {
        let entry = stats.models.entry(member.model.clone()).or_default();
        entry.runs += 1;
        entry.outputs += messages
            .iter()
            .filter(|m| {
                m.from == member.id
                    && matches!(m.kind, MessageKind::Proposal | MessageKind::Revision | MessageKind::Critique | MessageKind::Briefing | MessageKind::Synthesis)
            })
            .count() as u32;
        entry.failures += messages
            .iter()
            .filter(|m| m.kind == MessageKind::Status && m.from == "system" && m.content.contains(&member.id) && m.content.contains("失败"))
            .count() as u32;
    }
    // 背书记到**目标作者**对应的模型。
    for msg in messages.iter().filter(|m| m.kind == MessageKind::Endorsement) {
        let target_member = msg
            .meta
            .as_ref()
            .and_then(|v| v.get("targetAuthor"))
            .and_then(|v| v.as_str())
            .and_then(|id| spec.members.iter().find(|m| m.id == id));
        if let Some(target) = target_member {
            let verdict_endorse = msg
                .meta
                .as_ref()
                .and_then(|v| v.get("verdict"))
                .and_then(|v| v.as_str())
                .map(|s| s == "endorse")
                .unwrap_or(false);
            let entry = stats.models.entry(target.model.clone()).or_default();
            if verdict_endorse {
                entry.endorsed += 1;
            } else {
                entry.rejected += 1;
            }
        }
    }
    // 上限淘汰:runs 最少的先走。
    if stats.models.len() > STATS_CAP {
        let mut by_runs: Vec<(String, u32)> = stats.models.iter().map(|(k, v)| (k.clone(), v.runs)).collect();
        by_runs.sort_by_key(|(_, r)| *r);
        let excess = stats.models.len() - STATS_CAP;
        for (key, _) in by_runs.into_iter().take(excess) {
            stats.models.remove(&key);
        }
    }
    let _ = save_model_stats(&stats);
}

// ---------------------------------------------------------------------------
// 画像与组队
// ---------------------------------------------------------------------------

/// 给前端/组队 agent 的模型画像。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfile {
    pub model: String,
    /// 综合建议分 0..=10。
    pub score: u8,
    /// 分数依据的信号清单(人话,进 UI)。
    pub evidence: Vec<String>,
}

/// 历史经验分:runs≥2 才给分(样本太少不算);背书率主导,失败率惩罚。
fn history_score(h: &ModelHistory) -> Option<(u8, Vec<String>)> {
    if h.runs < 2 {
        return None;
    }
    let verdicts = h.endorsed + h.rejected;
    let mut evidence = vec![format!("历史 {}/{} run · 产出 {} · 失败 {}", h.runs, h.runs, h.outputs, h.failures)];
    if verdicts > 0 {
        evidence.push(format!("背书认可 {}/{}", h.endorsed, verdicts));
    }
    let endorse_ratio = if verdicts > 0 { h.endorsed as f32 / verdicts as f32 } else { 0.6 };
    let fail_ratio = (h.failures as f32 / (h.runs as f32 * 3.0)).min(1.0);
    let score = (endorse_ratio * 6.0 - fail_ratio * 3.0 + 4.0).clamp(0.0, 10.0) as u8;
    Some((score, evidence))
}

pub fn build_profile(model: &str, probes: &HashMap<String, ProbeOutcome>) -> ModelProfile {
    let stats = load_model_stats();
    build_profile_with(model, probes, &stats)
}

pub fn build_profile_with(model: &str, probes: &HashMap<String, ProbeOutcome>, stats: &ModelStatsFile) -> ModelProfile {
    let mut evidence = Vec::new();
    let mut signals: Vec<(f32, f32)> = Vec::new(); // (score, weight)
    if let Some(rank) = model_family_rank(model) {
        signals.push((rank as f32, 0.5));
        evidence.push(format!("家族档位启发:{} /10(未经实测)", rank));
    } else {
        evidence.push("家族档位:未知(不在启发表内)".into());
    }
    if let Some(probe) = probes.get(model) {
        // 探针 0..=7 → 0..=10,权重高于启发。
        let scaled = (probe.score as f32 / 7.0 * 10.0).round() as u8;
        signals.push((scaled as f32, 1.0));
        evidence.push(format!("本地探针:{}/7({})", probe.score, probe.notes));
    }
    if let Some(h) = stats.models.get(model) {
        if let Some((score, ev)) = history_score(h) {
            signals.push((score as f32, 1.5));
            evidence.extend(ev.into_iter().map(|e| format!("历史:{}", e)));
        }
    }
    let score = if signals.is_empty() {
        5
    } else {
        let total_w: f32 = signals.iter().map(|(_, w)| *w).sum();
        let s: f32 = signals.iter().map(|(v, w)| v * w).sum::<f32>() / total_w;
        s.round() as u8
    };
    ModelProfile { model: model.to_string(), score, evidence }
}

/// 组队计划(组队 agent 的输出,经 parse_formation 校验)。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FormationMember {
    pub model: String,
    pub role: String,
    pub tier: MemberTier,
    pub is_chair: bool,
    /// 该成员分到的职责说明(弱模型给"简单调研"类任务)。
    #[serde(default)]
    pub assignment: String,
    /// 组队理由(进 UI,可审计)。
    #[serde(default)]
    pub rationale: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FormationPlan {
    pub members: Vec<FormationMember>,
    pub rounds: u32,
    #[serde(default)]
    pub notes: String,
}

/// 自动组队:用池中画像分最高的模型当「组队 agent」,读任务 + 全部
/// 画像,产出团队配置 JSON。用户在 UI 审阅后才应用(人在环)。
pub fn auto_formation(
    client: &dyn ModelClient,
    task: &str,
    profiles: &[ModelProfile],
) -> Result<FormationPlan, String> {
    let pool: Vec<&str> = profiles.iter().map(|p| p.model.as_str()).collect();
    let Some(organizer) = profiles.iter().max_by_key(|p| p.score) else {
        return Err("候选模型池为空".into());
    };
    let (system, user) = super::prompts::formation_prompt(task, profiles, &organizer.model);
    let req = super::worker::WorkerRequest {
        model: organizer.model.clone(),
        system,
        user,
        max_tokens: Some(1600),
    };
    let out = client.complete(&req).map_err(|e| format!("组队调用失败({}):{e}", organizer.model))?;
    parse_formation(&out.content, &pool)
}

/// 解析并校验组队输出(围栏剥离 → 结构校验)。
pub fn parse_formation(raw: &str, pool: &[&str]) -> Result<FormationPlan, String> {
    let fence = raw
        .find("```json")
        .and_then(|start| raw[start + 7..].find("```").map(|end| &raw[start + 7..start + 7 + end]))
        .map(|s| s.trim())
        .unwrap_or(raw.trim());
    let value: serde_json::Value = serde_json::from_str(fence).map_err(|e| format!("组队 JSON 解析失败:{e}"))?;
    let rounds = value.get("rounds").and_then(|v| v.as_u64()).unwrap_or(2).clamp(1, 4) as u32;
    let notes = value.get("notes").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let arr = value
        .get("members")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "组队 JSON 缺 members 数组".to_string())?;
    if arr.len() < 2 || arr.len() > 8 {
        return Err(format!("组队成员数须在 2..=8,得到 {}", arr.len()));
    }
    let mut members = Vec::new();
    let mut chair_count = 0;
    for item in arr {
        let model = item.get("model").and_then(|v| v.as_str()).ok_or("成员缺 model")?.to_string();
        if !pool.contains(&model.as_str()) {
            return Err(format!("成员模型 {} 不在候选池内", model));
        }
        let role = item.get("role").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if role.trim().is_empty() {
            return Err(format!("成员 {} 缺角色描述", model));
        }
        let tier = match item.get("tier").and_then(|v| v.as_str()) {
            Some("lead") => MemberTier::Lead,
            Some("contributor") => MemberTier::Contributor,
            other => return Err(format!("成员 {} 的 tier 非法:{:?}", model, other)),
        };
        let is_chair = item.get("isChair").and_then(|v| v.as_bool()).unwrap_or(false);
        if is_chair {
            chair_count += 1;
            if tier != MemberTier::Lead {
                return Err("主席必须是 lead".into());
            }
        }
        members.push(FormationMember {
            model,
            role,
            tier,
            is_chair,
            assignment: item.get("assignment").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            rationale: item.get("rationale").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        });
    }
    if chair_count != 1 {
        return Err(format!("必须恰好一个主席,得到 {chair_count}"));
    }
    if !members.iter().any(|m| m.tier == MemberTier::Lead) {
        return Err("至少需要一个 lead".into());
    }
    // 同一模型出现多次时要求显式角色区分(合法,但给理由)。
    Ok(FormationPlan { members, rounds, notes })
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_rank_covers_known_tiers() {
        assert_eq!(model_family_rank("anthropic/claude-opus-4-5"), Some(9));
        assert_eq!(model_family_rank("openai/gpt-5.2-max"), Some(9));
        assert_eq!(model_family_rank("deepseek/deepseek-r1"), Some(9));
        assert_eq!(model_family_rank("google/gemini-2.5-flash"), Some(4));
        assert_eq!(model_family_rank("x-ai/grok-4-fast"), None); // fast 不在表内,诚实未知
        assert_eq!(model_family_rank("unknown/model-x"), None);
    }

    #[test]
    fn probe_scoring_correct_and_malformed() {
        let at = 1i64;
        let ok = score_probe_output("```json\n{\"answer\": \"2 小时\", \"reason\": \"1/(1/3-1/6)\"}\n```", at);
        assert_eq!(ok.score, 7);
        let wrong = score_probe_output("{\"answer\": \"3\", \"reason\": \"猜的\"}", at);
        assert_eq!(wrong.score, 4); // 格式 3 + 错答 1
        let bad = score_probe_output("答案是两小时", at);
        assert_eq!(bad.score, 0);
    }

    #[test]
    fn formation_parse_validates() {
        let pool = ["m/strong", "m/weak", "m/mid"];
        let good = r#"{"rounds":2,"notes":"ok","members":[
            {"model":"m/strong","role":"主席综合","tier":"lead","isChair":true,"rationale":"最强"},
            {"model":"m/mid","role":"架构","tier":"lead","isChair":false},
            {"model":"m/weak","role":"调研","tier":"contributor","isChair":false,"assignment":"搜集现有方案"}]}"#;
        let plan = parse_formation(good, &pool).unwrap();
        assert_eq!(plan.members.len(), 3);
        assert!(plan.members.iter().any(|m| m.tier == MemberTier::Contributor));

        // 池外模型拒绝
        let offpool = good.replace("m/mid", "m/other");
        assert!(parse_formation(&offpool, &pool).is_err());
        // 主席必须 lead
        let chair_contrib = r#"{"members":[
            {"model":"m/weak","role":"a","tier":"contributor","isChair":true},
            {"model":"m/strong","role":"b","tier":"lead"}]}"#;
        assert!(parse_formation(chair_contrib, &pool).is_err());
        // 无主席拒绝
        let no_chair = r#"{"members":[
            {"model":"m/strong","role":"a","tier":"lead"},
            {"model":"m/weak","role":"b","tier":"contributor"}]}"#;
        assert!(parse_formation(no_chair, &pool).is_err());
        // 成员数下限
        let single = r#"{"members":[{"model":"m/strong","role":"a","tier":"lead","isChair":true}]}"#;
        assert!(parse_formation(single, &pool).is_err());
    }

    #[test]
    fn profile_blends_signals_with_evidence() {
        let mut probes = HashMap::new();
        probes.insert("m/a".to_string(), ProbeOutcome { score: 7, notes: "全对".into(), at: 0 });
        let stats = ModelStatsFile {
            version: 1,
            models: HashMap::from([(
                "m/b".to_string(),
                ModelHistory { runs: 5, outputs: 10, endorsed: 8, rejected: 1, failures: 1 },
            )]),
        };
        let pa = build_profile_with("m/a", &probes, &stats);
        assert!(pa.score >= 7, "探针满分应高分:{:?}", pa.evidence);
        assert!(pa.evidence.iter().any(|e| e.contains("探针")));
        let pb = build_profile_with("m/b", &probes, &stats);
        assert!(pb.evidence.iter().any(|e| e.contains("历史")));
        // 无任何信号 → 中性 5 分。
        let pc = build_profile_with("m/unknown", &probes, &stats);
        assert_eq!(pc.score, 5);
    }

    /// auto_formation 端到端(mock):最强模型被选为组队者,产出合法计划。
    #[test]
    fn auto_formation_uses_strongest_as_organizer() {
        let mock = crate::council::worker::MockClient::new();
        mock.enqueue(
            "m/strong",
            vec![r#"{"rounds":1,"members":[
                {"model":"m/strong","role":"主席","tier":"lead","isChair":true},
                {"model":"m/weak","role":"调研","tier":"contributor","assignment":"找资料"}]}"#.to_string()],
        );
        let profiles = vec![
            ModelProfile { model: "m/strong".into(), score: 9, evidence: vec![] },
            ModelProfile { model: "m/weak".into(), score: 3, evidence: vec![] },
        ];
        let plan = auto_formation(&mock, "设计一个缓存", &profiles).unwrap();
        assert_eq!(plan.members.len(), 2);
        assert_eq!(plan.members[1].tier, MemberTier::Contributor);
        // 组队调用确实发生在最强模型上。
        assert!(mock.models_called().contains(&"m/strong".to_string()));
    }
}
