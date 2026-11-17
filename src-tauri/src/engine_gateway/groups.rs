//! 组路由（账本 T5）——把 agent 请求里的模型 id 解析成出站目标：
//! `group/<id>`、裸组名、`provider/model` 三形态，纯函数、不碰网络。
//! Go 参照（只读）：magpie `internal/provider/group.go` 的 GroupFor /
//! routedGroupOf / Picked / membersFound，以及种子补丁
//! `tools/magpie-seed/marionette_peel.go` 的 peelAgentModelMark（合同 C7）。
//!
//! 与 Go 版刻意收敛的差异（接线时别当成 bug，完整清单在交付报告）：
//! - 单成员语义：取 pick（无 pick 取首成员，Go `Picked()` 同款），不做
//!   Smart/Ordered/Weights 的多成员失败转移——T5 只把「问谁」定下来；
//! - 嵌套组最多展一层（Go 网关 maxNest=8）；防环靠「展过的组记集合」；
//! - 不做 Match globs（`openrouter/*:free`）、Former 改名谱系、auto 派生
//!   组、Off/Disabled 成员过滤——Rust store 里这些字段没消费面。

use super::store::{Group, Provider};

/// magpie `GroupPrefix` 同款：组 id 在请求里的前缀形态。
pub const GROUP_PREFIX: &str = "group/";

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

/// asked id 的分类：直通 `provider/model`，还是落在某个组上。接线层
/// （chat.rs 出站、目录行的组标注、错误信息）需要区分来源时用；
/// resolve 内部也走它，保证两条路径同一套判定。
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    Provider {
        provider: String,
        model: String,
    },
    /// `group` 是组的规范 id；`member` 是该组当前发往的成员——可能是
    /// `provider/model`，也可能是待展开的嵌套 `group/<id>`（展开在
    /// resolve 里做）。
    Group {
        group: String,
        member: String,
    },
}

/// 组当前发往的成员（Go `Picked()` 同语义）：pick 仍在 members 里就用
/// pick；不在（手改 providers.json 漂移——CLI 的 `group add` 会把 pick
/// 强行插回 members，正常运行两者恒一致，这里只是护栏）回落首成员；
/// 组空 → None。
fn picked_member(group: &Group) -> Option<&str> {
    if group.members.iter().any(|m| m == &group.pick) {
        return Some(group.pick.as_str());
    }
    group.members.first().map(String::as_str)
}

/// 按 id 或 name 找组。谓词与 store.group() 一致（routes.rs 的 C4 组宣告
/// 探针也用它）：`group/<id>` 形态与裸名形态共用同一个查找，agent 把组
/// 名直接填进模型字段（ZCode 的模型栏）靠 name 这半边命中。
fn find_group<'a>(groups: &'a [Group], id: &str) -> Option<&'a Group> {
    groups.iter().find(|g| g.id == id || g.name == id)
}

/// 把 asked 分类成 Route（不做嵌套展开与 provider 存在性校验——那些是
/// resolve 的活，分类本身保持无副作用、可单测）。
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
        return Some(Route::Group { group: g.id.clone(), member: picked_member(g)?.to_string() });
    }
    if !id.contains('/') {
        // 裸 id：是组 id/name 就走组；带斜杠的永远不可能是裸名形态。
        // （Go GroupFor 对含 '/' 的 id 同样直接拒绝组匹配。）
        let g = find_group(groups, id)?;
        return Some(Route::Group { group: g.id.clone(), member: picked_member(g)?.to_string() });
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

/// 组成员展开：成员自己是 `group/<id>` 时跟进。两个护栏——嵌套最多
/// 展一层（任务口径；Go 网关是 maxNest=8 的深走）；展过的组记进
/// visited，环（a 引 b 引 a、自引）在这里断成 None 而不是死循环
/// （Go 侧由 SaveGroup 拒环 + 读取时剪环兜底，我们只有读取侧）。
fn expand(groups: &[Group], outer: &str, member: String) -> Option<String> {
    let mut member = member;
    let mut visited: Vec<String> = vec![outer.to_string()];
    let mut depth = 0usize;
    while let Some(gid) = member.strip_prefix(GROUP_PREFIX) {
        depth += 1;
        if depth > 1 || gid.is_empty() || visited.iter().any(|seen| seen.as_str() == gid) {
            return None;
        }
        visited.push(gid.to_string());
        member = picked_member(find_group(groups, gid)?)?.to_string();
    }
    Some(member)
}

/// 把 asked（模型/组/别名，可带 `[1m]`/`[effort]` 标记）解析成
/// `(provider_id, bare_model)`，chat.rs 出站用的就是这个。
///
/// 顺序（任务口径）：剥标记 → `group/<id>` 形态找组取 pick（无 pick 首
/// 成员），成员可为 `provider/model` 或嵌套 `group/<id>`（最多展一层、
/// 防环）→ 裸 id 是组 id/name 同上 → `provider/model` 直通 → 都不是
/// None。provider 存在性要确认（错来源在这里就 404）；**模型存在性不
/// 查**——目录是活的（live 清单随时变、钉死清单也会被 refresh），错名
/// 留给厂商应答去说。
pub fn resolve(groups: &[Group], providers: &[Provider], asked: &str) -> Option<(String, String)> {
    let (pid, model) = match route(groups, asked)? {
        Route::Provider { provider, model } => (provider, model),
        Route::Group { group, member } => {
            let member = expand(groups, &group, member)?;
            let (p, m) = member.split_once('/')?;
            (p.to_string(), m.to_string())
        }
    };
    if pid.is_empty() || model.is_empty() {
        return None;
    }
    if !providers.iter().any(|p| p.id == pid) {
        return None;
    }
    Some((pid, model))
}

#[cfg(test)]
mod tests {
    use super::super::store::{Group, Provider};
    use super::*;

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

    fn prv(id: &str) -> Provider {
        Provider { id: id.to_string(), name: id.to_string(), ..Default::default() }
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
    fn pick_wins() {
        let ps = [prv("p1"), prv("p2")];
        let mut g = grp("g", &["p1/m1", "p2/m2"]);
        g.pick = "p2/m2".into();
        assert_eq!(resolve(&[g], &ps, "group/g"), Some(("p2".into(), "m2".into())));
        // pick 漂移（不在 members，手改文件才会出现）→ 回落首成员，
        // Go Picked() 同款。
        let mut bad = grp("b", &["p1/m1"]);
        bad.pick = "gone/m".into();
        assert_eq!(resolve(&[bad], &ps, "group/b"), Some(("p1".into(), "m1".into())));
    }

    #[test]
    fn nested_group() {
        let ps = [prv("p2")];
        // 嵌套 group/<id>：外组的成员是另一个组，展开一层取其 pick。
        let gs = [grp("outer", &["group/inner"]), grp("inner", &["p2/m2"])];
        assert_eq!(resolve(&gs, &ps, "group/outer"), Some(("p2".into(), "m2".into())));
        assert_eq!(resolve(&gs, &ps, "outer"), Some(("p2".into(), "m2".into())));
        // 嵌套再嵌套（两层 group/）超出「最多展一层」→ None。
        let deep = [grp("a", &["group/b"]), grp("b", &["group/c"]), grp("c", &["p2/m2"])];
        assert_eq!(resolve(&deep, &ps, "group/a"), None);
    }

    #[test]
    fn cycle_cuts_off() {
        let ps = [prv("p1"), prv("p2")];
        // a 引 b 引 a：不死循环，返回 None。
        let gs = [grp("a", &["group/b"]), grp("b", &["group/a"])];
        assert_eq!(resolve(&gs, &ps, "group/a"), None);
        assert_eq!(resolve(&gs, &ps, "group/b"), None);
        // 自引同样断掉。
        let self_ref = [grp("s", &["group/s", "p1/m1"])];
        assert_eq!(resolve(&self_ref, &ps, "group/s"), None);
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
}
