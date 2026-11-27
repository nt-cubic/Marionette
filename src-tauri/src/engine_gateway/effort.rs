//! 组成员的后缀语法与思考强度改写（对齐 magpie `groupeffort.go` /
//! `groupaccount.go` / `groupfast.go` + 网关 `withEffort`）。
//!
//! 为什么必须有：Marionette 的 codex 别名组（`engine::codex_alias_sync`）把
//! 成员写成 `source-x/deepseek-v4.1-flash:1017:max`、`codex/gpt-6-luna:xhigh`
//! ——末尾 `:max` 是**成员固定强度**，不是模型名的一部分。Rust 引擎最初把
//! 它原样发给厂商（`model not found`），所有走别名的 codex 会话整条挂掉。
//!
//! 成员 id 语法（从右往左剥，顺序固定）：
//!   `provider/model[:effort[+effort…]][:fast][#account]`
//! - `#account`：账号钉死（Rust 引擎没有多账号，剥掉忽略）；
//! - `:fast`：优先通道（只对 ChatGPT 账号有意义，剥掉忽略）；
//! - `:level`：最后一段是强度词才算（`:free`/`:7b`/`:1017`/`:0` 是模型自己的
//!   tag）；且整串若本来就是来源有的模型（目录/钉死清单里有）则不拆。
//!   多个强度用 `+` 连（`low+high`）= 跟随 agent 的强度，只在这几档里取最近。

/// 强度词全表，低 → 高（与 groups.rs / translate.rs 的 EFFORTS 同表）。
pub const LEVELS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

fn level_rank(word: &str) -> Option<usize> {
    LEVELS.iter().position(|l| *l == word)
}

/// 拆开后的成员。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedMember {
    /// `provider/model`，已剥所有后缀。
    pub model: String,
    /// 固定强度（单档）或可选档位（多档，低→高）；空 = 跟 agent。
    pub levels: Vec<String>,
    /// `#account` 钉死的账号引用（空 = 没钉；Codex 订阅多账号用，其它来源忽略）。
    pub account: Option<String>,
    /// `:fast` 优先通道标记（只对 ChatGPT 订阅账号有意义；codex 模块可读，
    /// 其余来源忽略）。
    pub fast: bool,
}

/// 解析 `levels` 段：每个 `+` 分段都必须是强度词；返回去重、低→高排序后的档。
fn spell_levels(suffix: &str) -> Option<Vec<String>> {
    let mut ranks: Vec<usize> = Vec::new();
    for part in suffix.to_ascii_lowercase().split('+') {
        let rank = level_rank(part)?;
        if !ranks.contains(&rank) {
            ranks.push(rank);
        }
    }
    if ranks.is_empty() {
        return None;
    }
    ranks.sort_unstable();
    Some(ranks.into_iter().map(|r| LEVELS[r].to_string()).collect())
}

/// 拆成员 id。`has_model(whole_id)` 回答「整串本来就是该来源的一个模型」
/// （Go `isModel`：`ollama` 的 `qwen:7b`、`openrouter` 的 `…:free`）——
/// 是则不拆强度/fast。`group/<id>` 形态的成员原样返回（嵌套组不带后缀）。
pub fn parse_member(id: &str, has_model: &dyn Fn(&str) -> bool) -> ParsedMember {
    let id = id.trim();
    if id.starts_with("group/") {
        return ParsedMember {
            model: id.to_string(),
            ..Default::default()
        };
    }
    // `#account`（账号名里不含 `/`；`m#x` 的 `#` 不在位置 0）
    let mut rest = id;
    let mut account: Option<String> = None;
    if let Some(i) = rest.rfind('#') {
        let acct = rest[i + 1..].trim();
        if i > 0 && !acct.is_empty() && !acct.contains('/') {
            account = Some(acct.to_string());
            rest = &rest[..i];
        }
    }
    // `:fast`
    let mut fast = false;
    if let Some(i) = rest.rfind(':') {
        if i > 0 && rest[i + 1..].eq_ignore_ascii_case("fast") && !has_model(rest) {
            fast = true;
            rest = &rest[..i];
        }
    }
    // `:level`
    if let Some(i) = rest.rfind(':') {
        if i > 0 {
            if let Some(levels) = spell_levels(&rest[i + 1..]) {
                if !has_model(rest) {
                    return ParsedMember {
                        model: rest[..i].to_string(),
                        levels,
                        account,
                        fast,
                    };
                }
            }
        }
    }
    ParsedMember {
        model: rest.to_string(),
        levels: Vec::new(),
        account,
        fast,
    }
}

/// 在 `levels` 里取离 `asked` 最近的一档（Go `Member.Levels` 语义：agent 要的
/// 不在其中就发最近的）。同距取低档。
fn nearest(levels: &[String], asked: &str) -> Option<String> {
    let want = level_rank(asked)? as i64;
    levels
        .iter()
        .filter_map(|l| level_rank(l).map(|r| (r as i64, l)))
        .min_by_key(|(r, _)| ((r - want).abs(), *r))
        .map(|(_, l)| l.clone())
}

/// 把强度词拟合到模型自己的档位表（目录行 `reasoning_levels` 等字段）：
/// 表里有就原样；没有就取最近的。表空/没有强度词 → 原样（不编数）。
pub fn fit_to_model(effort: &str, model_levels: &[String]) -> String {
    let known: Vec<String> = model_levels
        .iter()
        .map(|l| l.to_ascii_lowercase())
        .filter(|l| level_rank(l).is_some())
        .collect();
    if known.is_empty() || known.iter().any(|l| l == effort) {
        return effort.to_string();
    }
    nearest(&known, effort).unwrap_or_else(|| effort.to_string())
}

/// 请求体入站协议。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Proto {
    Chat,
    Responses,
    Anthropic,
}

pub fn budget_of(effort: &str) -> u64 {
    match effort {
        "low" => 4096,
        "medium" => 10000,
        "high" => 24000,
        "xhigh" | "max" => 32000,
        _ => 10000,
    }
}

/// 请求体当前要的强度（自己协议的话）；没要 → None。
pub fn requested(proto: Proto, v: &serde_json::Value) -> Option<String> {
    let word = |s: Option<&str>| {
        s.map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty() && s != "none")
    };
    match proto {
        Proto::Chat => word(v.get("reasoning_effort").and_then(|x| x.as_str())),
        Proto::Responses => word(v.pointer("/reasoning/effort").and_then(|x| x.as_str())),
        Proto::Anthropic => {
            let thinking = v.get("thinking")?;
            match thinking.get("type").and_then(|t| t.as_str()) {
                Some("adaptive") | Some("enabled") => {}
                _ => return None,
            }
            if let Some(e) = word(v.pointer("/output_config/effort").and_then(|x| x.as_str())) {
                return Some(e);
            }
            let budget = thinking
                .get("budget_tokens")
                .and_then(|b| b.as_u64())
                .unwrap_or(0);
            Some(
                match budget {
                    0 => "medium",
                    b if b <= 4096 => "low",
                    b if b <= 12000 => "medium",
                    b if b <= 28000 => "high",
                    _ => "max",
                }
                .to_string(),
            )
        }
    }
}

/// 按成员强度改写请求（Go `withEffort` 同语义）：**只改本来就要思考的
/// 请求**——没要思考的（会话标题等）保持没有。固定单档 = 发这一档；多档 =
/// 跟 agent，只在这几档里取最近。`model_levels` 是目录里该模型自报的档位
/// （空 = 不知道，原样发）。
pub fn apply(
    proto: Proto,
    body: &[u8],
    member_levels: &[String],
    model_levels: &[String],
) -> Option<Vec<u8>> {
    if member_levels.is_empty() {
        return None;
    }
    let mut v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let asked = requested(proto, &v)?;
    let chosen = if member_levels.len() == 1 {
        member_levels[0].clone()
    } else {
        nearest(member_levels, &asked)?
    };
    let sent = fit_to_model(&chosen, model_levels);
    let map = v.as_object_mut()?;
    match proto {
        Proto::Chat => {
            map.insert("reasoning_effort".into(), serde_json::Value::String(sent));
        }
        Proto::Responses => {
            let reasoning = map
                .entry("reasoning")
                .or_insert_with(|| serde_json::json!({}));
            reasoning
                .as_object_mut()?
                .insert("effort".into(), serde_json::Value::String(sent));
        }
        Proto::Anthropic => {
            let adaptive = map
                .get("thinking")
                .and_then(|t| t.get("type"))
                .and_then(|t| t.as_str())
                == Some("adaptive");
            if adaptive {
                let oc = map
                    .entry("output_config")
                    .or_insert_with(|| serde_json::json!({}));
                oc.as_object_mut()?
                    .insert("effort".into(), serde_json::Value::String(sent));
            } else {
                let mut budget = budget_of(&sent);
                if let Some(max) = map.get("max_tokens").and_then(|m| m.as_u64()) {
                    budget = budget.min(max.saturating_sub(1));
                }
                if budget < 1024 {
                    return None;
                }
                map.insert(
                    "thinking".into(),
                    serde_json::json!({ "type": "enabled", "budget_tokens": budget }),
                );
                if let Some(oc) = map.get_mut("output_config").and_then(|o| o.as_object_mut()) {
                    if oc.contains_key("effort") {
                        oc.insert("effort".into(), serde_json::Value::String(sent));
                    }
                }
            }
        }
    }
    serde_json::to_vec(&v).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> bool {
        false
    }

    #[test]
    fn member_suffixes() {
        // 用户机上真实存在的两个别名组成员
        let m = parse_member(
            "source-73800e7663517058/deepseek-v4.1-flash:1017:max",
            &none,
        );
        assert_eq!(m.model, "source-73800e7663517058/deepseek-v4.1-flash:1017");
        assert_eq!(m.levels, vec!["max"]);
        let m = parse_member("codex/gpt-6-luna:xhigh", &none);
        assert_eq!(m.model, "codex/gpt-6-luna");
        assert_eq!(m.levels, vec!["xhigh"]);
        // 模型自己的 tag 不是强度
        for id in ["ollama/qwen:7b", "openrouter/x/y:free", "p/m:1017", "p/m:0"] {
            assert_eq!(
                parse_member(id, &none),
                ParsedMember {
                    model: id.into(),
                    ..Default::default()
                }
            );
        }
        // 多档 + fast + account
        let m = parse_member("p/m:High+low:fast#acct", &none);
        assert_eq!(m.model, "p/m");
        assert_eq!(m.levels, vec!["low", "high"]);
        assert_eq!(m.account.as_deref(), Some("acct"));
        assert!(m.fast);
        assert_eq!(parse_member("codex/m:high", &none).account, None);
        // 整串本来是个模型 → 不拆
        let has = |id: &str| id == "p/m:high";
        assert_eq!(parse_member("p/m:high", &has).model, "p/m:high");
        // 嵌套组原样
        assert_eq!(parse_member("group/x", &none).model, "group/x");
    }

    #[test]
    fn apply_only_when_reasoning_asked() {
        let lv = vec!["max".to_string()];
        // chat：要了思考 → 改；没要 → 不动（None）
        let out = apply(
            Proto::Chat,
            br#"{"model":"m","reasoning_effort":"low"}"#,
            &lv,
            &[],
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out).unwrap()["reasoning_effort"],
            "max"
        );
        assert!(apply(Proto::Chat, br#"{"model":"m"}"#, &lv, &[]).is_none());
        assert!(apply(Proto::Chat, br#"{"reasoning_effort":"none"}"#, &lv, &[]).is_none());
        // responses：只改 effort，reasoning 其余字段保留
        let out = apply(
            Proto::Responses,
            br#"{"reasoning":{"effort":"low","summary":"auto"}}"#,
            &["xhigh".to_string()],
            &[],
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reasoning"]["effort"], "xhigh");
        assert_eq!(v["reasoning"]["summary"], "auto");
        // 多档：agent 要 medium，成员只有 low/high → 同距取低
        let out = apply(
            Proto::Chat,
            br#"{"reasoning_effort":"medium"}"#,
            &["low".to_string(), "high".to_string()],
            &[],
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out).unwrap()["reasoning_effort"],
            "low"
        );
    }

    #[test]
    fn anthropic_shapes() {
        let lv = vec!["high".to_string()];
        let out = apply(
            Proto::Anthropic,
            br#"{"thinking":{"type":"enabled","budget_tokens":2000},"max_tokens":64000}"#,
            &lv,
            &[],
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out).unwrap()["thinking"]["budget_tokens"],
            24000
        );
        let out = apply(
            Proto::Anthropic,
            br#"{"thinking":{"type":"adaptive"}}"#,
            &lv,
            &[],
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out).unwrap()["output_config"]["effort"],
            "high"
        );
        assert!(apply(Proto::Anthropic, br#"{"messages":[]}"#, &lv, &[]).is_none());
    }

    #[test]
    fn fit_to_model_levels() {
        let model = vec!["low".to_string(), "high".to_string()];
        assert_eq!(fit_to_model("high", &model), "high");
        assert_eq!(fit_to_model("xhigh", &model), "high");
        assert_eq!(fit_to_model("max", &[]), "max");
    }
}
