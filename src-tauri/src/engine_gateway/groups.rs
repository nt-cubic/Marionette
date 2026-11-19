//! 组路由（账本 T5）——把 agent 请求里的模型 id 解析成出站目标：
//! `group/<id>`、裸组名、`provider/model` 三形态。
//! Go 参照（只读）：magpie `internal/provider/group.go` 的 GroupFor /
//! routes / Picked / groupsIn、`groupmatch.go`、`groupweight.go`、
//! `grouprename`（Former），网关 `fallback.go` 的 `retryable`，以及种子补丁
//! `tools/magpie-seed/marionette_peel.go` 的 peelAgentModelMark（合同 C7）。
//!
//! 两层：
//! - **纯函数层**（[`plan_with`]）：asked → 有序候选表 [`Candidate`]。routing
//!   取值（providers.json `routing`，Go `Group.Routing`）：
//!   `manual`（只发 pick，无 pick 取首成员）、`weight`（按 `weights` 平滑
//!   加权轮转取首发，其余按列表序做后备）、`rotate`（轮转起点）、其余全部
//!   按列表序（`""`=Smart、`order`、`usage`/`pace`/`free`/`fast`/`cheap`/
//!   `drain`/`expiring`、插件路由 `<plugin>/<name>`）——后几种在 Go 里要读
//!   额度/测速/价格，Rust 引擎没有这些数据源，诚实降级成列表序，失败转移照常。
//!   `off` 成员跳过；`match` 的 glob（`openrouter/*:free`，大小写不敏感、`*` 可
//!   跨 `/`）在已命名成员之后按目录序补成员；嵌套 `group/<id>` 递归展开（Go
//!   maxNest=8，环在闭合处剪掉，`disabled` 的嵌套组跳过）；`former`（改名
//!   谱系）旧 id 仍认。`re:` 正则成员不支持（不引 regex crate）。
//! - **驱动层**（[`drive`] / [`plan_for`] / [`retryable`]）：成员出错（连接失败 /
//!   Go `retryable` 同口径的状态）时换下一个成员。**只在应答头阶段换**——
//!   流式 2xx 一旦把 Stream 交出去就是最终答案，已发出的字节绝不回头。
//!
//! 与 Go 版刻意收敛的差异：Smart/usage/pace/free/fast/cheap/drain/expiring 无
//! 额度数据（见上）；无「休息」记忆（失败成员不会被后续请求降权）；无
//! FirstToken/Patience 等待；无 rules/classifier/auto 派生组；无 Placed 排序；
//! `Within` 继承（空 routing 的嵌套组沿用外层池化路由）不做——嵌套组按自己的
//! routing 走。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::httpd::Reply;
use super::store::{Group, Provider};

/// magpie `GroupPrefix` 同款：组 id 在请求里的前缀形态。
pub const GROUP_PREFIX: &str = "group/";

/// 嵌套组最深层数（Go `maxNest`）。
pub const MAX_NEST: usize = 8;

/// Codex ACP 方括号 effort 的合法集。照抄 Go 侧 provider.MemberEfforts
/// 全表（七档，含 none/minimal）——translate.rs 的
/// peel_agent_model_mark 同表；两处必须一起改。
const EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// 剥掉 agent 自报模型 id 上的「标记后缀」：Claude Code 给 1M 窗口模型
/// 加的 `[1m]`，与 Codex ACP 编码 reasoning effort 的 `[xhigh]` 之类。
/// 为什么：组与来源表里存的是裸名，不剥就对不上（种子补丁
/// marionette_peel.go 的 peelAgentModelMark，锚点见合同 C7——stock magpie
/// 只剥 `[1m]`，Codex 2.x 的 effort 后缀会 404 共享别名）。
///
/// 语义（与 Go 版逐条对齐）：先 trim 空白；剥一个字面 `[1m]`（大小写
/// 敏感，同 Go TrimSuffix）；再看**最后一个** `[..]`——内容小写比较在
/// effort 集合内才剥，且 `[` 不能在首位（`[xhigh]` 整串不是标记，得留
/// 给「来源 id 为空」的判定去拒）。只剥一层：`m[1m][xhigh]` 剥完剩
/// `m[1m]`，非 effort 后缀原样保留。
pub fn peel_mark(id: &str) -> &str {
    let id = id.trim();
    let id = id.strip_suffix("[1m]").unwrap_or(id);
    if let Some(at) = id.rfind('[') {
        if at > 0 && id.ends_with(']') {
            let inner = id[at + 1..id.len() - 1].to_ascii_lowercase();
            if EFFORTS.contains(&inner.as_str()) {
                return &id[..at];
            }
        }
    }
    id
}

/// asked id 的分类：直通 `provider/model`，还是落在某个组上。
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    Provider {
        provider: String,
        model: String,
    },
    /// `group` 是组的规范 id；`member` 是该组 manual/列表序下的首发成员——
    /// 可能是 `provider/model`，也可能是待展开的嵌套 `group/<id>`。
    Group {
        group: String,
        member: String,
    },
}

/// 一个出站候选：已剥标记/后缀的 `provider/model` + 成员语法带来的附加信息。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Candidate {
    pub provider: String,
    /// 厂商认的裸模型名（`:effort`/`:fast`/`#account` 已剥）。
    pub model: String,
    /// 成员固定强度（单档）或可选档位（多档）；空 = 跟 agent。
    pub levels: Vec<String>,
    /// 成员的原始写法（日志/去重/权重键）。
    pub member: String,
    /// `:fast` 标记（codex 订阅账号用）。
    pub fast: bool,
    /// `#account` 钉死的账号（codex 订阅多账号用；其它来源忽略）。
    pub account: Option<String>,
}

// ---------------------------------------------------------------------------
// providers.json 里 Go-only 字段的读口（走 `extra`，不进 Rust 结构体：写回时
// flatten 原样保留，手写成怪值也不会让整份文件解析失败）
// ---------------------------------------------------------------------------

fn extra_strs(g: &Group, key: &str) -> Vec<String> {
    g.extra
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).collect())
        .unwrap_or_default()
}

fn extra_bool(g: &Group, key: &str) -> bool {
    g.extra.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// 成员权重（Go `MemberWeight`：`max(w, 1)`，没写算 1）。
fn weight_of(g: &Group, member: &str) -> i64 {
    g.extra
        .get("weights")
        .and_then(|w| w.get(member))
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(1)
        .max(1)
}

// ---------------------------------------------------------------------------
// 组查找
// ---------------------------------------------------------------------------

/// 组当前发往的成员（Go `Picked()` 同语义）：pick 仍在 members 里就用
/// pick；不在（手改 providers.json 漂移）回落首成员；组空 → None。
fn picked_member(group: &Group) -> Option<&str> {
    if group.members.iter().any(|m| m == &group.pick) {
        return Some(group.pick.as_str());
    }
    group.members.first().map(String::as_str)
}

/// 按 id 或 name 找组；都没中再认改名前的旧 id（Go `Former`：改名后旧 id
/// 仍路由到它，**直到别的组占了这个 id**——所以 id/name 精确命中永远先于
/// former 谱系）。
fn find_group<'a>(groups: &'a [Group], id: &str) -> Option<&'a Group> {
    groups
        .iter()
        .find(|g| g.id == id || g.name == id)
        .or_else(|| groups.iter().find(|g| g.former.iter().any(|f| f == id)))
}

/// 把 asked 分类成 Route（不做嵌套展开与 provider 存在性校验）。
pub fn route(groups: &[Group], asked: &str) -> Option<Route> {
    // 先剥标记：`group/fast[xhigh]`、` DS Flash[1m] ` 都得先回到裸名才
    // 查得中表。
    let id = peel_mark(asked);
    if let Some(gid) = id.strip_prefix(GROUP_PREFIX) {
        // `group/` 后为空（裸 "group/"）不算组。
        if gid.is_empty() {
            return None;
        }
        let g = find_group(groups, gid)?;
        return Some(Route::Group { group: g.id.clone(), member: head_member(g)?.to_string() });
    }
    if !id.contains('/') {
        // 裸 id：是组 id/name 就走组；带斜杠的永远不可能是裸名形态。
        // （Go GroupFor 对含 '/' 的 id 同样直接拒绝组匹配。）
        let g = find_group(groups, id)?;
        return Some(Route::Group { group: g.id.clone(), member: head_member(g)?.to_string() });
    }
    // `provider/model` 直通：首段是来源 id，其余整体是模型名——
    // openrouter 的 "openrouter/openai/gpt-…" 也是首段切开后剩下的全归
    // 模型（与 chat.rs 旧版 split_once 行为一致）。
    let (provider, model) = id.split_once('/')?;
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    Some(Route::Provider { provider: provider.to_string(), model: model.to_string() })
}

/// 静态首发成员（不推进轮转状态）：manual → pick；其余 → 首个未 off 成员。
fn head_member(g: &Group) -> Option<&str> {
    if g.routing == "manual" {
        return picked_member(g);
    }
    let off = extra_strs(g, "off");
    g.members.iter().map(String::as_str).find(|m| !off.iter().any(|o| o == m))
}

// ---------------------------------------------------------------------------
// match glob
// ---------------------------------------------------------------------------

/// Go compilePattern 的 glob 半边：`*` = 任意一串字符（含 `/`），其余按字面，
/// 整串匹配，大小写不敏感。`re:` 前缀（正则）不支持 → 永不命中。
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.trim().trim_start_matches("magpie/");
    if pattern.starts_with("re:") || !pattern.contains('*') {
        return false;
    }
    let pat = pattern.to_lowercase();
    let text = text.to_lowercase();
    let parts: Vec<&str> = pat.split('*').collect();
    let first = parts[0];
    let last = parts[parts.len() - 1];
    if !text.starts_with(first) {
        return false;
    }
    let mut at = first.len();
    for mid in &parts[1..parts.len() - 1] {
        match text[at..].find(mid) {
            Some(i) => at += i + mid.len(),
            None => return false,
        }
    }
    text.len() >= at + last.len() && text[at..].ends_with(last)
}

// ---------------------------------------------------------------------------
// 路由状态（rotate / weight 的轮转点）
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RouteState {
    /// rotate：已发出多少次请求。
    turn: u64,
    /// weight：平滑加权轮转（nginx smooth WRR）的 current_weight，按成员原文。
    current: HashMap<String, i64>,
}

fn route_state() -> &'static Mutex<HashMap<String, RouteState>> {
    static STATE: OnceLock<Mutex<HashMap<String, RouteState>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 平滑加权轮转选一个下标：`3,1` 依次 a,a,b,a（Go 注释同口径）。
/// `advance=false` 只看不推进（旧入口 resolve_with 用，别让一次请求里
/// 多处解析把轮转点推歪）。
fn wrr_pick(group_id: &str, members: &[String], weights: &[i64], advance: bool) -> usize {
    let Ok(mut all) = route_state().lock() else { return 0 };
    let state = all.entry(group_id.to_string()).or_default();
    let mut cur: HashMap<String, i64> = members
        .iter()
        .map(|m| (m.clone(), state.current.get(m).copied().unwrap_or(0)))
        .collect();
    let total: i64 = weights.iter().sum();
    let mut best = 0usize;
    let mut best_w = i64::MIN;
    for (i, m) in members.iter().enumerate() {
        let c = cur.entry(m.clone()).or_insert(0);
        *c += weights[i];
        if *c > best_w {
            best_w = *c;
            best = i;
        }
    }
    if let Some(c) = cur.get_mut(&members[best]) {
        *c -= total;
    }
    if advance {
        state.current = cur;
    }
    best
}

fn rotate_pick(group_id: &str, len: usize, advance: bool) -> usize {
    let Ok(mut all) = route_state().lock() else { return 0 };
    let state = all.entry(group_id.to_string()).or_default();
    let at = (state.turn % len as u64) as usize;
    if advance {
        state.turn += 1;
    }
    at
}

// ---------------------------------------------------------------------------
// 计划
// ---------------------------------------------------------------------------

/// plan 所需的外部事实（均为闭包：纯函数层不碰网络/目录）。
pub struct PlanCtx<'a> {
    /// 整串 `provider/model` 是不是该来源本来就有的模型（成员后缀拆分用）。
    pub has_model: &'a dyn Fn(&str) -> bool,
    /// 目录里所有 `provider/model`（只在某个组带 `match` 时才调用——可能联网）。
    pub catalog_ids: &'a dyn Fn() -> Vec<String>,
}

/// 组此刻的成员 id 序列（成员原文，含后缀与嵌套 `group/..`），已按 routing
/// 排好首发顺序。manual = 只有 pick。
fn ordered_members(ctx: &PlanCtx, g: &Group, advance: bool) -> Vec<String> {
    let mut list: Vec<String> = if g.routing == "manual" {
        picked_member(g).map(|p| vec![p.to_string()]).unwrap_or_default()
    } else {
        let off = extra_strs(g, "off");
        g.members.iter().filter(|m| !off.iter().any(|o| o == *m)).cloned().collect()
    };
    // match：已命名成员之后，按目录序补上命中的模型（Go withMatches）。
    let patterns = extra_strs(g, "match");
    if !patterns.is_empty() && g.routing != "manual" {
        let named: Vec<String> = g
            .members
            .iter()
            .map(|m| super::effort::parse_member(m, ctx.has_model).model)
            .collect();
        let off = extra_strs(g, "off");
        for id in (ctx.catalog_ids)() {
            if named.iter().any(|n| n == &id) || list.iter().any(|m| m == &id) || off.iter().any(|o| o == &id) {
                continue;
            }
            if patterns.iter().any(|p| glob_match(p, &id)) {
                list.push(id);
            }
        }
    }
    if list.len() < 2 {
        return list;
    }
    let first = match g.routing.as_str() {
        "weight" => {
            let weights: Vec<i64> = list.iter().map(|m| weight_of(g, m)).collect();
            wrr_pick(&g.id, &list, &weights, advance)
        }
        "rotate" => rotate_pick(&g.id, list.len(), advance),
        _ => 0,
    };
    if first > 0 {
        let head = list.remove(first);
        list.insert(0, head);
    }
    list
}

fn expand_group(
    ctx: &PlanCtx,
    groups: &[Group],
    g: &Group,
    depth: usize,
    visited: &mut Vec<String>,
    advance: bool,
    out: &mut Vec<Candidate>,
) {
    for member in ordered_members(ctx, g, advance) {
        let member = member.trim().to_string();
        if let Some(gid) = member.strip_prefix(GROUP_PREFIX) {
            // 嵌套：深度封顶；环在闭合处剪掉；关掉的组被跳过（Go 同款）。
            if depth + 1 > MAX_NEST || gid.is_empty() {
                continue;
            }
            let Some(inner) = find_group(groups, gid) else { continue };
            if extra_bool(inner, "disabled") || visited.iter().any(|v| v == &inner.id) {
                continue;
            }
            visited.push(inner.id.clone());
            expand_group(ctx, groups, inner, depth + 1, visited, advance, out);
            visited.pop();
            continue;
        }
        let parsed = super::effort::parse_member(&member, ctx.has_model);
        let Some((p, m)) = parsed.model.split_once('/') else { continue };
        if p.is_empty() || m.is_empty() {
            continue;
        }
        let cand = Candidate {
            provider: p.to_string(),
            model: m.to_string(),
            levels: parsed.levels,
            member: member.clone(),
            fast: parsed.fast,
            account: parsed.account,
        };
        let dup = out.iter().any(|c| {
            c.provider == cand.provider && c.model == cand.model && c.levels == cand.levels && c.account == cand.account
        });
        if !dup {
            out.push(cand);
        }
    }
}

/// 核心：asked → 有序候选表。`advance` = 是否推进 rotate/weight 的轮转点
/// （真正出站的那一次才推进）。`providers` 过滤由调用方决定（候选里 provider
/// 不存在的成员在这里**不**滤，[`plan_with`] 再滤）。
fn plan_raw(groups: &[Group], asked: &str, ctx: &PlanCtx, advance: bool) -> Vec<Candidate> {
    let id = peel_mark(asked);
    let group = if let Some(gid) = id.strip_prefix(GROUP_PREFIX) {
        if gid.is_empty() {
            return Vec::new();
        }
        find_group(groups, gid)
    } else if !id.contains('/') {
        find_group(groups, id)
    } else {
        let Some((p, m)) = id.split_once('/') else { return Vec::new() };
        if p.is_empty() || m.is_empty() {
            return Vec::new();
        }
        return vec![Candidate {
            provider: p.to_string(),
            model: m.to_string(),
            member: id.to_string(),
            ..Default::default()
        }];
    };
    let Some(g) = group else { return Vec::new() };
    // 顶层被关掉的组：请求被挡回（Go：turned away with a word）。
    if extra_bool(g, "disabled") {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut visited = vec![g.id.clone()];
    expand_group(ctx, groups, g, 0, &mut visited, advance, &mut out);
    out
}

/// 出站用候选表：provider 不存在的成员跳过（Go 的成员解析失败同款），推进
/// 轮转状态。空 = 无处可发（调用方 404）。
pub fn plan_with(groups: &[Group], providers: &[Provider], asked: &str, ctx: &PlanCtx) -> Vec<Candidate> {
    plan_raw(groups, asked, ctx, true)
        .into_iter()
        .filter(|c| providers.iter().any(|p| p.id == c.provider))
        .collect()
}

/// 把 asked（模型/组/别名，可带 `[1m]`/`[effort]` 标记）解析成
/// `(provider_id, bare_model)`——**首发**候选，不推进轮转。
pub fn resolve(groups: &[Group], providers: &[Provider], asked: &str) -> Option<(String, String)> {
    resolve_with(groups, providers, asked, &|_| false).map(|(p, m, _)| (p, m))
}

/// [`resolve`] + 成员后缀语法（effort.rs）：组成员 `provider/model:max#acct`
/// 剥成裸模型 + 强度档。`has_model(whole_id)` 回答整串是不是该来源本来就有
/// 的模型（`qwen:7b`、`…:free` 不能被当强度剥）。直通 `provider/model` 形态
/// 不拆后缀——只有组成员才带强度语法。
///
/// 语义：只看**首发**候选（不做失败转移、不推进 rotate/weight）；首发的来源
/// 不存在 → None（codex_account::intercept 靠这个判「真实来源接不住」）。
/// 带 `match` 的组此处不展开目录（无 catalog 闭包），只算已命名成员。
pub fn resolve_with(
    groups: &[Group],
    providers: &[Provider],
    asked: &str,
    has_model: &dyn Fn(&str) -> bool,
) -> Option<(String, String, Vec<String>)> {
    let no_ids = || Vec::new();
    let ctx = PlanCtx { has_model, catalog_ids: &no_ids };
    let head = plan_raw(groups, asked, &ctx, false).into_iter().next()?;
    if !providers.iter().any(|p| p.id == head.provider) {
        return None;
    }
    Some((head.provider, head.model, head.levels))
}

// ---------------------------------------------------------------------------
// 驱动层：失败转移
// ---------------------------------------------------------------------------

/// 一次成员尝试的结果。
pub enum Attempt {
    /// 终局（成功，或不该换成员的失败）。
    Done(Reply),
    /// 该成员失败得值得换下一个（Go `retryable`）；Reply 是它的错误应答，
    /// 若已是最后一个成员就原样交给 agent。
    Retry(Reply),
}

/// 依次尝试候选，直到某个给出终局。**只在应答头阶段换成员**——`f` 返回
/// `Done(Reply::Stream)` 即交出流，之后不可能再换（字节已经上路）。
pub fn drive(cands: &[Candidate], mut f: impl FnMut(&Candidate) -> Attempt) -> Reply {
    let mut last: Option<Reply> = None;
    for cand in cands {
        match f(cand) {
            Attempt::Done(reply) => return reply,
            Attempt::Retry(reply) => last = Some(reply),
        }
    }
    last.unwrap_or_else(|| Reply::error(404, "no member to send to"))
}

/// Go `retryable` 的口径：换个成员可能做得更好的失败——厂商忙/欠费/挂了，
/// 或这把钥匙/这个来源服务不了——**不是请求本身的错**。会话超窗（tooLong）
/// 在每个成员上一样长，不换（agent 该去压缩）。
pub fn retryable(status: u16, body: &[u8]) -> bool {
    if status < 400 {
        return false;
    }
    if super::bodycap::window_full(status, body) {
        return false;
    }
    match status {
        401 | 402 | 403 | 404 | 408 | 429 => true,
        s if s >= 500 => true,
        400 | 422 => {
            // 400/422 里只有措辞说明「这个成员不行」才换：额度/欠费/模型不在。
            let lower = String::from_utf8_lossy(body).to_lowercase();
            const WORDS: [&str; 18] = [
                "quota",
                "insufficient",
                "balance",
                "credit",
                "billing",
                "余额",
                "额度",
                "欠费",
                "model_not_found",
                "model not found",
                "does not exist",
                "not supported",
                "unknown model",
                "invalid model",
                "no such model",
                "model is not available",
                "not available in",
                "unsupported model",
            ];
            WORDS.iter().any(|w| lower.contains(w))
        }
        _ => false,
    }
}

/// 网关层入口：从 store 快照 groups/providers（锁内只克隆，锁外解析，目录
/// 联网也在锁外），算出出站候选表。
pub fn plan_for(gw: &super::Gateway, asked: &str) -> Vec<Candidate> {
    let (groups, providers) = {
        let Ok(store) = gw.store.lock() else { return Vec::new() };
        (store.groups().to_vec(), store.providers().to_vec())
    };
    let known = |id: &str| gw.catalog.knows(&providers, id);
    let ids = || -> Vec<String> {
        providers
            .iter()
            .flat_map(|p| {
                gw.catalog.listed(p).into_iter().map(move |row| format!("{}/{}", p.id, row.id))
            })
            .collect()
    };
    plan_with(&groups, &providers, asked, &PlanCtx { has_model: &known, catalog_ids: &ids })
}

/// 按候选的成员强度改写请求体（Go `withEffort`）：每个候选各算各的——
/// 失败转移到下一个成员时它带的是自己的固定强度，不是首发成员的。
pub fn body_for(
    gw: &super::Gateway,
    proto: super::effort::Proto,
    cand: &Candidate,
    body: &[u8],
) -> Vec<u8> {
    if cand.levels.is_empty() {
        return body.to_vec();
    }
    let model_levels = gw.catalog.levels_of(&cand.provider, &cand.model);
    super::effort::apply(proto, body, &cand.levels, &model_levels).unwrap_or_else(|| body.to_vec())
}

#[cfg(test)]
mod tests {
    use super::super::store::{Group, Provider, ProvidersFile};
    use super::*;
    use serde_json::json;

    fn grp(id: &str, members: &[&str]) -> Group {
        Group {
            id: id.to_string(),
            name: id.to_string(),
            members: members.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn grp_named(id: &str, name: &str, members: &[&str]) -> Group {
        Group { name: name.to_string(), ..grp(id, members) }
    }

    fn with_extra(mut g: Group, key: &str, value: serde_json::Value) -> Group {
        g.extra.insert(key.to_string(), value);
        g
    }

    fn prv(id: &str) -> Provider {
        Provider { id: id.to_string(), name: id.to_string(), ..Default::default() }
    }

    fn no_model(_: &str) -> bool {
        false
    }

    /// 测试用 ctx：目录里有给定的 id 列表。
    fn plan(groups: &[Group], providers: &[Provider], asked: &str, ids: &[&str]) -> Vec<(String, String)> {
        let owned: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        let listing = || owned.clone();
        let ctx = PlanCtx { has_model: &no_model, catalog_ids: &listing };
        plan_with(groups, providers, asked, &ctx)
            .into_iter()
            .map(|c| (c.provider, c.model))
            .collect()
    }

    fn pm(p: &str, m: &str) -> (String, String) {
        (p.to_string(), m.to_string())
    }

    #[test]
    fn passthrough() {
        let ps = [prv("openai")];
        assert_eq!(resolve(&[], &ps, "openai/gpt-6"), Some(("openai".into(), "gpt-6".into())));
        // 多段斜杠：首段是来源，其余整体是模型名（openrouter 形态）。
        assert_eq!(
            resolve(&[], &ps, "openai/org/gpt-x"),
            Some(("openai".into(), "org/gpt-x".into()))
        );
        // 来源不存在 → None（模型存在性不查，但来源要确认）。
        assert_eq!(resolve(&[], &ps, "nope/m"), None);
        assert_eq!(resolve(&[], &[], "openai/gpt-6"), None);
    }

    #[test]
    fn group_by_id() {
        let gs = [grp("fast", &["p1/m1", "p2/m2"])];
        let ps = [prv("p1"), prv("p2")];
        // 前缀形态与裸 id 形态都命中；无 pick 取首成员。
        assert_eq!(resolve(&gs, &ps, "group/fast"), Some(("p1".into(), "m1".into())));
        assert_eq!(resolve(&gs, &ps, "fast"), Some(("p1".into(), "m1".into())));
        // 未知组 → None。
        assert_eq!(resolve(&gs, &ps, "group/other"), None);
    }

    #[test]
    fn group_by_name() {
        let gs = [grp_named("ds", "DS Flash", &["p1/m1", "p2/m2"])];
        let ps = [prv("p1"), prv("p2")];
        // agent 把组名直接填进模型字段（ZCode 模型栏）的形态。
        assert_eq!(resolve(&gs, &ps, "DS Flash"), Some(("p1".into(), "m1".into())));
        // group/<name> 也认（store.group() 同款 id-or-name 谓词）。
        assert_eq!(resolve(&gs, &ps, "group/DS Flash"), Some(("p1".into(), "m1".into())));
    }

    #[test]
    fn manual_pick_wins_and_smart_ignores_it() {
        let ps = [prv("p1"), prv("p2")];
        let mut g = grp("g", &["p1/m1", "p2/m2"]);
        g.pick = "p2/m2".into();
        // 没写 routing = Smart：pick 只是留着给以后切 manual（Go 语义），
        // 列表序 + 失败转移。
        assert_eq!(plan(&[g.clone()], &ps, "group/g", &[]), vec![pm("p1", "m1"), pm("p2", "m2")]);
        // manual：只发 pick，没有后备。
        g.routing = "manual".into();
        assert_eq!(plan(&[g.clone()], &ps, "group/g", &[]), vec![pm("p2", "m2")]);
        assert_eq!(resolve(&[g], &ps, "group/g"), Some(("p2".into(), "m2".into())));
        // pick 漂移（不在 members，手改文件才会出现）→ 回落首成员，
        // Go Picked() 同款。
        let mut bad = grp("b", &["p1/m1"]);
        bad.routing = "manual".into();
        bad.pick = "gone/m".into();
        assert_eq!(resolve(&[bad], &ps, "group/b"), Some(("p1".into(), "m1".into())));
    }

    #[test]
    fn ordered_group_lists_every_member_in_order() {
        let ps = [prv("p1"), prv("p2"), prv("p3")];
        let mut g = grp("g", &["p1/a", "p2/b", "p3/c"]);
        g.routing = "order".into();
        assert_eq!(
            plan(&[g], &ps, "g", &[]),
            vec![pm("p1", "a"), pm("p2", "b"), pm("p3", "c")]
        );
        // 来源不存在的成员被跳过，不卡住整条链。
        let g = grp("h", &["gone/x", "p2/b"]);
        assert_eq!(plan(&[g], &ps, "h", &[]), vec![pm("p2", "b")]);
    }

    #[test]
    fn off_members_are_skipped_but_manual_pick_is_not_filtered() {
        let ps = [prv("p1"), prv("p2")];
        let g = with_extra(grp("g", &["p1/a", "p2/b"]), "off", json!(["p1/a"]));
        assert_eq!(plan(&[g], &ps, "g", &[]), vec![pm("p2", "b")]);
    }

    #[test]
    fn weighted_group_is_smooth_and_proportional() {
        let ps = [prv("p1"), prv("p2")];
        let mut g = grp("wgt-test", &["p1/a", "p2/b"]);
        g.routing = "weight".into();
        let g = with_extra(g, "weights", json!({ "p1/a": 3 }));
        let gs = [g];
        let heads: Vec<String> = (0..8)
            .map(|_| plan(&gs, &ps, "wgt-test", &[])[0].0.clone())
            .collect();
        // 3:1 → 8 次里 6 个 p1、2 个 p2，且 p2 不连发（平滑）。
        assert_eq!(heads.iter().filter(|h| *h == "p1").count(), 6);
        assert_eq!(heads.iter().filter(|h| *h == "p2").count(), 2);
        assert!(!heads.windows(2).any(|w| w[0] == "p2" && w[1] == "p2"));
        // 首发之外，另一个成员仍在后备里。
        assert_eq!(plan(&gs, &ps, "wgt-test", &[]).len(), 2);
        // 只看不推进的旧入口不动轮转点。
        let before = resolve(&gs, &ps, "wgt-test");
        let after = resolve(&gs, &ps, "wgt-test");
        assert_eq!(before, after);
    }

    #[test]
    fn rotate_group_moves_the_start() {
        let ps = [prv("p1"), prv("p2"), prv("p3")];
        let mut g = grp("rot-test", &["p1/a", "p2/b", "p3/c"]);
        g.routing = "rotate".into();
        let gs = [g];
        let heads: Vec<String> = (0..4)
            .map(|_| plan(&gs, &ps, "rot-test", &[])[0].0.clone())
            .collect();
        assert_eq!(heads, vec!["p1", "p2", "p3", "p1"]);
    }

    #[test]
    fn nested_groups_expand_in_order_up_to_max_nest() {
        let ps = [prv("p1"), prv("p2")];
        // 外组：先内组（自己的两个成员），再 p1/z。
        let gs = [grp("outer", &["group/inner", "p1/z"]), grp("inner", &["p2/m2", "p2/m3"])];
        assert_eq!(
            plan(&gs, &ps, "group/outer", &[]),
            vec![pm("p2", "m2"), pm("p2", "m3"), pm("p1", "z")]
        );
        // 多层（Go maxNest=8，旧版只展一层）。
        let deep = [grp("a", &["group/b"]), grp("b", &["group/c"]), grp("c", &["p2/m2"])];
        assert_eq!(resolve(&deep, &ps, "group/a"), Some(("p2".into(), "m2".into())));
        // 超过 8 层：被截断，不死循环。
        let mut chain: Vec<Group> = (0..12).map(|i| grp(&format!("n{i}"), &[&format!("group/n{}", i + 1)])).collect();
        chain.push(grp("n12", &["p2/deep"]));
        assert!(plan(&chain, &ps, "group/n0", &[]).is_empty());
        // 8 层以内可达。
        let mut ok: Vec<Group> = (0..8).map(|i| grp(&format!("k{i}"), &[&format!("group/k{}", i + 1)])).collect();
        ok.push(grp("k8", &["p2/deep"]));
        assert_eq!(plan(&ok, &ps, "group/k0", &[]), vec![pm("p2", "deep")]);
    }

    #[test]
    fn cycle_cuts_off() {
        let ps = [prv("p1"), prv("p2")];
        // a 引 b 引 a：不死循环，闭合处剪掉 → 无候选。
        let gs = [grp("a", &["group/b"]), grp("b", &["group/a"])];
        assert_eq!(resolve(&gs, &ps, "group/a"), None);
        assert_eq!(resolve(&gs, &ps, "group/b"), None);
        // 自引：只剪掉自引那一条，其余成员照常。
        let self_ref = [grp("s", &["group/s", "p1/m1"])];
        assert_eq!(resolve(&self_ref, &ps, "group/s"), Some(("p1".into(), "m1".into())));
        // 环里夹着真成员：环不挡路。
        let mixed = [grp("x", &["group/y", "p1/m1"]), grp("y", &["group/x", "p2/m2"])];
        assert_eq!(plan(&mixed, &ps, "group/x", &[]), vec![pm("p2", "m2"), pm("p1", "m1")]);
    }

    #[test]
    fn disabled_group_is_refused_and_skipped_when_nested() {
        let ps = [prv("p1"), prv("p2")];
        let off = with_extra(grp("off", &["p1/a"]), "disabled", json!(true));
        let outer = grp("outer", &["group/off", "p2/b"]);
        let gs = [off, outer];
        assert!(plan(&gs, &ps, "group/off", &[]).is_empty());
        assert_eq!(plan(&gs, &ps, "group/outer", &[]), vec![pm("p2", "b")]);
    }

    #[test]
    fn match_glob_adds_catalog_models_after_named() {
        let ps = [prv("openrouter"), prv("p1")];
        let g = with_extra(grp("free", &["p1/own"]), "match", json!(["openrouter/*:free"]));
        let ids = ["openrouter/a:free", "openrouter/b:paid", "openrouter/c:FREE", "p1/own"];
        assert_eq!(
            plan(&[g], &ps, "group/free", &ids),
            vec![pm("p1", "own"), pm("openrouter", "a:free"), pm("openrouter", "c:FREE")]
        );
        // 已命名成员不被 match 再加一次；re: 正则暂不支持（不命中）。
        let g = with_extra(grp("g", &["openrouter/a:free"]), "match", json!(["openrouter/*", "re:.*"]));
        let got = plan(&[g], &ps, "group/g", &["openrouter/a:free", "openrouter/z"]);
        assert_eq!(got, vec![pm("openrouter", "a:free"), pm("openrouter", "z")]);
        // 没写 match 的组不会去碰目录。
        let boom = || -> Vec<String> { panic!("catalog must not be asked") };
        let ctx = PlanCtx { has_model: &no_model, catalog_ids: &boom };
        let _ = plan_with(&[grp("plain", &["p1/x"])], &ps, "plain", &ctx);
    }

    #[test]
    fn glob_semantics() {
        assert!(glob_match("openrouter/*:free", "openrouter/meta/llama:free"));
        assert!(glob_match("*-free", "x/y-FREE"));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXcYYb"));
        assert!(!glob_match("openrouter/*:free", "openrouter/x:paid"));
        assert!(!glob_match("no-star", "no-star"));
    }

    #[test]
    fn former_names_still_route() {
        let ps = [prv("p1")];
        let mut renamed = grp("new-id", &["p1/m"]);
        renamed.former = vec!["old-id".into()];
        let gs = [renamed];
        assert_eq!(resolve(&gs, &ps, "group/old-id"), Some(("p1".into(), "m".into())));
        assert_eq!(resolve(&gs, &ps, "old-id"), Some(("p1".into(), "m".into())));
        // 别的组占了这个 id 之后，旧名归新主人（精确命中先于谱系）。
        let gs = [gs[0].clone(), grp("old-id", &["p1/other"])];
        assert_eq!(resolve(&gs, &ps, "old-id"), Some(("p1".into(), "other".into())));
    }

    #[test]
    fn peel_marks() {
        assert_eq!(peel_mark(" m[1m] "), "m");
        assert_eq!(peel_mark("m[xhigh]"), "m");
        // effort 大小写不敏感。
        assert_eq!(peel_mark("m[XHigh]"), "m");
        assert_eq!(peel_mark("m[MAX]"), "m");
        // 七档全表（上游 MemberEfforts 含 none/minimal——translate.rs 同表）。
        assert_eq!(peel_mark("m[none]"), "m");
        assert_eq!(peel_mark("m[minimal]"), "m");
        // 未知后缀不剥；[1m] 本身大小写敏感（Go TrimSuffix 同款）。
        assert_eq!(peel_mark("m[banana]"), "m[banana]");
        assert_eq!(peel_mark("m[1M]"), "m[1M]");
        // 整串方括号（首位）不是标记。
        assert_eq!(peel_mark("[xhigh]"), "[xhigh]");
        // 只剥最后一个、且只剥一层。
        assert_eq!(peel_mark("m[1m][xhigh]"), "m[1m]");
        assert_eq!(peel_mark("m[a][high]"), "m[a]");
        // 没闭合 / 没方括号 / 空串。
        assert_eq!(peel_mark("m[x"), "m[x");
        assert_eq!(peel_mark("m"), "m");
        assert_eq!(peel_mark(""), "");
        // 多字节正文不踩 char boundary。
        assert_eq!(peel_mark("模型[high]"), "模型");
    }

    #[test]
    fn peel_then_group() {
        let gs = [grp_named("ds-flash", "DS Flash", &["p1/m1"])];
        let ps = [prv("p1")];
        assert_eq!(resolve(&gs, &ps, "group/ds-flash[xhigh]"), Some(("p1".into(), "m1".into())));
        assert_eq!(resolve(&gs, &ps, " DS Flash[1m] "), Some(("p1".into(), "m1".into())));
        // 直通形态同样先剥。
        assert_eq!(resolve(&gs, &ps, "p1/m1[1m]"), Some(("p1".into(), "m1".into())));
    }

    #[test]
    fn member_effort_suffix_is_stripped_not_sent_to_vendor() {
        // 用户机上真实存在的 codex 别名组形态：成员末尾 `:max` / `:xhigh`
        // 是固定强度，厂商只该看到裸模型（ollama 的 `:1017` tag 保留）。
        let ps = [prv("source-73800e7663517058"), prv("codex")];
        let gs = [
            grp("gpt-6.1-terra", &["source-73800e7663517058/deepseek-v4.1-flash:1017:max"]),
            grp("gpt-6.1-sol", &["codex/gpt-6-luna:xhigh"]),
        ];
        let got = resolve_with(&gs, &ps, "gpt-6.1-terra", &|_| false).unwrap();
        assert_eq!(got, ("source-73800e7663517058".into(), "deepseek-v4.1-flash:1017".into(), vec!["max".to_string()]));
        // 旧入口同样不再把强度发给厂商
        assert_eq!(
            resolve(&gs, &ps, "gpt-6.1-sol"),
            Some(("codex".into(), "gpt-6-luna".into()))
        );
        // 整串是来源已知模型 → 不拆
        let known = |id: &str| id == "codex/gpt-6-luna:xhigh";
        assert_eq!(
            resolve_with(&gs, &ps, "gpt-6.1-sol", &known).unwrap().1,
            "gpt-6-luna:xhigh"
        );
    }

    #[test]
    fn candidate_keeps_fast_and_account_for_codex() {
        let ps = [prv("codex")];
        let gs = [grp("g", &["codex/gpt-6:high:fast#work"])];
        let ctx = PlanCtx { has_model: &no_model, catalog_ids: &|| Vec::new() };
        let c = &plan_with(&gs, &ps, "g", &ctx)[0];
        assert_eq!(c.model, "gpt-6");
        assert_eq!(c.levels, vec!["high"]);
        assert!(c.fast);
        assert_eq!(c.account.as_deref(), Some("work"));
    }

    #[test]
    fn edges() {
        let ps = [prv("p1")];
        // 空成员组 / 空串 / 裸 "group/" / provider/model 空段。
        assert_eq!(resolve(&[grp("empty", &[])], &ps, "group/empty"), None);
        assert_eq!(resolve(&[], &ps, ""), None);
        assert_eq!(resolve(&[], &ps, "group/"), None);
        assert_eq!(resolve(&[], &ps, "p1/"), None);
        assert_eq!(resolve(&[], &ps, "/m"), None);
        // 组成员指向不存在的来源 → None。
        assert_eq!(resolve(&[grp("g", &["dead/m"])], &ps, "group/g"), None);
    }

    fn cand(p: &str) -> Candidate {
        Candidate { provider: p.into(), model: "m".into(), ..Default::default() }
    }

    fn reply_status(r: &Reply) -> u16 {
        match r {
            Reply::Json { status, .. } | Reply::Stream { status, .. } => *status,
        }
    }

    #[test]
    fn drive_fails_over_until_done_and_returns_last_error() {
        let cands = [cand("a"), cand("b"), cand("c")];
        let mut asked = Vec::new();
        let r = drive(&cands, |c| {
            asked.push(c.provider.clone());
            if c.provider == "c" {
                Attempt::Done(Reply::json(200, &json!({"ok": true})))
            } else {
                Attempt::Retry(Reply::error(503, "down"))
            }
        });
        assert_eq!(asked, vec!["a", "b", "c"]);
        assert_eq!(reply_status(&r), 200);
        // 全挂：最后一个成员的错误交给 agent。
        let r = drive(&cands, |c| {
            Attempt::Retry(Reply::error(if c.provider == "c" { 502 } else { 503 }, "down"))
        });
        assert_eq!(reply_status(&r), 502);
        // 终局即停：不再问后面的成员。
        let mut n = 0;
        let _ = drive(&cands, |_| {
            n += 1;
            Attempt::Done(Reply::error(400, "bad request"))
        });
        assert_eq!(n, 1);
        assert_eq!(reply_status(&drive(&[], |_| unreachable!())), 404);
    }

    #[test]
    fn retryable_matches_go_status_classes() {
        for s in [401u16, 402, 403, 404, 408, 429, 500, 502, 503, 529] {
            assert!(retryable(s, br#"{"error":"x"}"#), "{s} should fail over");
        }
        for s in [200u16, 201, 400, 405, 409, 413, 422] {
            assert!(!retryable(s, br#"{"error":"bad parameter"}"#), "{s} should not fail over");
        }
        // 400/422 只有措辞说明「这个成员不行」才换。
        assert!(retryable(400, br#"{"error":{"message":"You exceeded your current quota"}}"#));
        assert!(retryable(400, br#"{"error":{"message":"The model `x` does not exist"}}"#));
        assert!(retryable(422, br#"{"detail":"model_not_found"}"#));
        // 会话超窗在每个成员上一样长：不换。
        assert!(!retryable(400, b"This model's maximum context length is 8192 tokens"));
        assert!(!retryable(400, b"prompt is too long"));
    }

    #[test]
    fn rich_group_round_trips_through_store_types() {
        // Go 写的、带一堆 Rust 不认识字段的组：读一遍再写回，字段一个不丢
        // （Rust 写盘绝不能毁掉 Go-only 字段）。
        let raw = json!({
            "providers": [{ "id": "p1", "name": "P1", "key": "k", "chat": "http://x/v1", "someGoField": {"a": 1} }],
            "groups": [{
                "id": "rich",
                "name": "Rich",
                "note": "mine",
                "members": ["p1/a:high", "group/other", "p1/b#acct"],
                "former": ["old"],
                "match": ["p1/*:free"],
                "matched": ["p1/z:free"],
                "placed": ["p1/a:high"],
                "routing": "weight",
                "affinity": "session",
                "routedAt": "2026-11-01T00:00:00Z",
                "sink": true,
                "noSink": false,
                "firstToken": 12,
                "patience": 30,
                "off": ["p1/b#acct"],
                "pick": "p1/a:high",
                "rules": [{ "when": "x", "use": "p1/a:high" }],
                "classifier": "p1/c",
                "intentSure": 0.5,
                "effort": "auto",
                "context": 100000,
                "compact": 80000,
                "levels": ["low", "high"],
                "fast": ["p1/a:high"],
                "ultra": [],
                "weights": { "p1/a:high": 3 },
                "effortMaps": { "p1/a:high": { "high": "xhigh" } },
                "family": "f",
                "unlists": true,
                "accounts": ["p1/abc"],
                "keys": ["k1"],
                "redact": "on",
                "hidden": false,
                "seen": ["p1/a"],
                "disabled": false,
                "futureField": [1, 2, 3]
            }],
            "order": ["p1"],
            "groupOrder": ["rich"],
            "noAutoGroups": true
        });
        let file: ProvidersFile = serde_json::from_value(raw.clone()).unwrap();
        let back = serde_json::to_value(&file).unwrap();
        let g_in = &raw["groups"][0];
        let g_out = &back["groups"][0];
        for (k, v) in g_in.as_object().unwrap() {
            // Rust 对 omitempty 空串/空数组省略与 Go 同语义；空数组 ultra 属 extra 原样保留。
            assert_eq!(g_out.get(k), Some(v), "group field {k} lost or changed");
        }
        for k in ["order", "groupOrder", "noAutoGroups"] {
            assert_eq!(back.get(k), raw.get(k), "top-level {k} lost");
        }
        assert_eq!(back["providers"][0]["someGoField"], raw["providers"][0]["someGoField"]);
        // 同一份组也能被路由层读懂：权重/off 来自 extra。
        let ps = [prv("p1")];
        let ctx = PlanCtx { has_model: &no_model, catalog_ids: &|| Vec::new() };
        let cands = plan_with(&file.groups, &ps, "rich", &ctx);
        // b 被 off；group/other 不存在被跳过；只剩 a（强度 high）。
        assert_eq!(cands.len(), 1);
        assert_eq!((cands[0].model.as_str(), cands[0].levels.clone()), ("a", vec!["high".to_string()]));
    }

    #[test]
    fn group_missing_name_does_not_break_the_file() {
        // Go 老文件里组缺 name 不该让整份 providers.json 解析失败
        // （读失败 fail-open 成空表 = 全部来源消失）。
        let raw = json!({ "providers": [], "groups": [{ "id": "g", "members": ["p/m"] }] });
        let file: ProvidersFile = serde_json::from_value(raw).unwrap();
        assert_eq!(file.groups[0].id, "g");
    }
}
