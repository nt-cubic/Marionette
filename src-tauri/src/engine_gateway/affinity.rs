//! 会话粘性（affinity / 路由粘性）——Go `internal/gateway/affinity.go` 的
//! 纯模块移植（Go 是唯一真相源，注释引行号）。核心动机同 Go 头注（:3-27）：
//! 一场对话留在答过它的账号上，厂商为它缓存的整个上下文（每请求全量回读）
//! 才能继续命中，而不是全价重发给别人。
//!
//! 职责（本模块只做这四件，接线在 mod.rs / groups.rs / chat.rs）：
//! - **会话种子提取** [`stick_key`]：从请求头 + 请求体认出「这是哪场对话」
//!   （Go stickKey :553 ← affinityID :10468 ← openCodeSeed :28）。
//!   无种子 → None，不粘。
//! - **答者序表** [`Affinity`]：每会话记最近答过的账号（provider id + model，
//!   [`Affinity::answered`]，Go answered :1120）；路由时把曾答账号按最近
//!   回答序浮到候选最前（[`Affinity::affine`]，Go affine :569 的纯排序子集），
//!   其余候选保持原序。
//! - **回主标记** [`Affinity::back_to_main`]：fallback 答过、主账号就绪后
//!   调用方显式置位，本会话不再浮起（Go backToMain :763 是拿路由 order 的
//!   路由期判定，本模块没有路由上下文，收敛成显式标记；下一次 answered 清掉）。
//! - **落盘** [`Affinity::save`]/[`Affinity::load`]/[`Affinity::flush`]：
//!   `affinity.json` 落在引擎数据目录（config_dir()，与 Go sticksPath :294
//!   同位——providers.json 旁边），原子写 tmp+rename（store.rs 同款），
//!   读失败/坏文件 fail-open 成空表。answered 只进内存；落盘靠显式
//!   flush()/Drop 尽力一次（Go 的 2s 防抖没搬，见差距 7）。
//!
//! 规模控制：每会话记最近 8 个答者（任务口径；Go 每会话只存最后一位 :145）；
//! 全表 4096 会话 LRU（超出丢最老）、答者 30 天过期（Go sticksKept :86 /
//! stickKeep :84）。
//!
//! ## 与 Go 的差距清单（没搬的逐条，改这里前先过一遍）
//!
//! 1. **单答者 → 序表**：Go stick 只记最后一位答者（含 effort/turn/cacheRead/
//!    cacheWrite/hourCache/stoodIn，:145-160）；本表按口径记最近 8 个
//!    (id, model)。affine 的三段匹配（who+model+effort → who+model →
//!    sameVendor，:597-607）简化成两段（id+model 精确 → id 宽匹配），
//!    effort 维度没有；Go 的 rest（远端 magpie 标识）与 who 折叠成一个 id。
//! 2. **Go affine 的全部 keep/why 分支没搬**：mode（session/turn/off/strict）、
//!    within（轮内 tool 回传）、cacheRead/cacheWrite/cacheWorth(1024)/
//!    cacheCold(5m)/chatGPTCold(1h)/deepseekCold/hourCache 的「缓存值不值得
//!    粘」判定（:47-99、:608-717），strict/strictOnly/strictKeep（:461/:518），
//!    reserve/credits/busy/order/back/hours/spent 全部不进本表——只做
//!    「曾答账号浮最前」。answered 也不收 usage/turn 参数（Go
//!    answered(key, c, turn, u) 的缓存读数折算 :1127-1130 没搬）。
//! 3. **withSession**（:1001，会话主对话带路）、**leastHeld/withLead**
//!    （:855/:955，新会话散开/子代理跟主）、**after**（:1094，In turn 轮转）、
//!    **keptAgain**（:1551，粘住账号瞬时失败原位重试）没搬。
//! 4. **unanswered**（:1169，回答中断取消粘性）/ **opened**（:1163，首请求
//!     计数）没搬；Go 的 sticks.opening 表不存在。
//! 5. **stickKey 的 scope 前缀**（`scope|会话`，组/provider 域隔离 :554）没搬：
//!    种子即键。接线方要按组隔离时自己拼前缀。
//! 6. **无种子兜底**：Go 退 conversationID（开场白哈希，gateway.go:10468 起），
//!    本移植返回 None 不粘——两个开场一样的对话会被哈希折成一场粘，
//!    宁可不粘。
//! 7. **落盘节奏**：Go noteSticks 2s 防抖 + flushSticks 收尾（:218-271）；
//!    本模块 answered 只进内存，落盘靠显式 flush()/Drop——进程崩溃丢自
//!    上次落盘之后的记录。**文件格式与 Go 的 map[conv]savedStick（:274）
//!    不互通**：两边交替跑互相读不懂（fail-open 空表后覆盖写）。
//! 8. **refusedSeals / overflowLearned**（:1208-1509，加密 reasoning 拒收
//!     记忆 / 超长拒绝学习）不属本模块，未搬。
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Go sessionHeaders（gateway.go:10379）：各 agent 自报会话 id 的头。
/// `x-opencode-session` 是 openCodeSeed（opencode_session.go:30）的第一
/// 优先，单列在先；这里是从头列表里去掉它之后的余表（按 Go 声明序）。
const SESSION_HEADERS: &[&str] = &[
    "x-session-affinity",
    "x-session-id",
    "session_id",
    "session-id",
    "x-claude-code-session-id",
    "x-conversation-id",
    "x-deepseek-harness-session-id",
];

/// Go SessionHeader（gateway.go:10402）：magpie 自己的会话头，sessionOf
/// （gateway.go:10389）把它排在所有 agent 头之前。
const SESSION_HEADER: &str = "X-Magpie-Session";

/// 答者记多久（Go stickKeep :84：30 天——长任务周末/一周后接上仍认得，
/// 过期换账号不用怕它拒收前一位封的 reasoning）。
const STICK_KEEP_MS: u64 = 30 * 24 * 3600 * 1000;

/// 全表保留多少会话，最近的（Go sticksKept :86，落盘裁剪）。
const SESSIONS_KEPT: usize = 4096;

/// 每会话记最近几个答者（任务口径；Go 每会话只存最后一位 :145）。
const ANSWERS_KEPT: usize = 8;

/// 会话种子提取（Go stickKey :553 的简化）：头优先（openCodeSeed :28 →
/// sessionOf :10389 的顺序），头没有再看体——`prompt_cache_key`
/// （chat / responses 两方言同字段，chat.go:76 / responses.go:284），再
/// `metadata.user_id`（anthropic 方言，Claude Code 的用户标识，ir.go:381）。
/// 全都没有 → None 不粘（Go 会退到开场白哈希 conversationID，本移植不做，
/// 见差距清单 6）。有 `x-claude-code-agent-id` 子代理头时追加
/// `|agent:<id>`（stickKey :555-558；id 截 128，Go subagentOf gateway.go:10447
/// ——按字符截，别在 UTF-8 中间落刀）——CC 子代理与主对话各粘各的，
/// 否则子代理的回答会顶掉主对话的粘性（Go :543-552 的原案）。
pub(crate) fn stick_key(headers: &[(String, String)], body: &serde_json::Value) -> Option<String> {
    let mut seed = header(headers, "x-opencode-session")
        .or_else(|| header(headers, SESSION_HEADER))
        .or_else(|| SESSION_HEADERS.iter().find_map(|h| header(headers, h)));
    if seed.is_none() {
        seed = body_str(body, &["prompt_cache_key"])
            .or_else(|| body_str(body, &["metadata", "user_id"]));
    }
    let mut key = seed?.to_string();
    if let Some(sub) = header(headers, "x-claude-code-agent-id") {
        key.push_str("|agent:");
        key.extend(sub.chars().take(128));
    }
    Some(key)
}

/// 头取值：大小写不敏感（Go http.Header.Get 同义），值 trim 后空视为没有。
fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

/// 体的点路径取字符串值（Go gjson 的只读子集：非字符串不认，trim 后空视为无）。
fn body_str<'a>(body: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut v = body;
    for key in path {
        v = v.get(*key)?;
    }
    v.as_str().map(str::trim).filter(|s| !s.is_empty())
}

/// 一条答过记录（Go stick :145 的简化：去 effort/turn/缓存读数/hourCache/stoodIn）。
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
struct StickEntry {
    /// provider id（Go 的 who/rest 折叠成一个：本表不区分远端 rest 标识）。
    id: String,
    /// 它答的时候用的模型（Go stick.model：一个账号上可能有多个模型）。
    model: String,
    /// 它最近一次回答，epoch 毫秒（Go stick.at）。
    at: u64,
}

/// 一个会话的粘性：最近答过的账号序（**表尾最新**）+ 回主标记。
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
struct SessionStick {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    answers: Vec<StickEntry>,
    /// fallback 答过后回主（Go backToMain :763 的显式标记形态）：置位期间
    /// affine 不浮起任何曾答账号（回主后由路由自己的顺序决定），下一次
    /// answered 清掉。落盘随表走——重启后仍不粘到 fallback。
    #[serde(default, skip_serializing_if = "is_false")]
    back_to_main: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// 粘性表（Go 的全局 sticks :192 收成可持有对象）：进程内 Mutex 表 + 落盘。
/// 线程安全（Send+Sync），网关里共享一份。
pub(crate) struct Affinity {
    /// affinity.json 的位置；None = 无引擎数据目录（config_dir 缺失），
    /// 纯内存态（flush 静默跳过）。
    path: Option<PathBuf>,
    /// 会话键（stick_key 的返回值）→ 答者序。
    table: Mutex<HashMap<String, SessionStick>>,
    /// 串行化落盘，一次一写最后者胜（Go sticksSaving :208）。
    saving: Mutex<()>,
}

impl Affinity {
    /// 从引擎数据目录读（config_dir()/affinity.json，Go sticksPath :294——
    /// providers.json 旁边）。目录缺失/读失败/坏文件 → 空表（fail-open，
    /// Go stickOf :314 decode 失败即 miss 同义，坏文件不炸网关）。
    pub(crate) fn load() -> Affinity {
        match super::config_dir() {
            Some(dir) => Affinity::load_at(&dir.join("affinity.json")),
            None => Affinity {
                path: None,
                table: Mutex::new(HashMap::new()),
                saving: Mutex::new(()),
            },
        }
    }

    /// 指定文件路径读（测试缝 / 自定义数据目录）。读入时清过期答者、
    /// 裁每会话 8 条与全表 4096（外部写进来的超限也收敛）。
    pub(crate) fn load_at(path: &std::path::Path) -> Affinity {
        let mut table: HashMap<String, SessionStick> = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let now = now_ms();
        for st in table.values_mut() {
            sanitize_session(st, now);
        }
        trim_table(&mut table, now);
        Affinity {
            path: Some(path.to_path_buf()),
            table: Mutex::new(table),
            saving: Mutex::new(()),
        }
    }

    /// 记一笔「本会话最近答过的账号」（Go answered :1120）。同一 (id, model)
    /// 重复回答只挪序不重复记；每会话留最近 8 个；新回答清掉回主标记
    /// （粘性重新建立）。只进内存——落盘靠 flush()/Drop（见差距清单 7）。
    pub(crate) fn answered(&self, session: &str, provider_id: &str, model: &str) {
        let now = now_ms();
        let mut table = self.table.lock().unwrap_or_else(|p| p.into_inner());
        let stick = table.entry(session.to_string()).or_default();
        stick.answers.retain(|e| fresh(e.at, now));
        stick
            .answers
            .retain(|e| e.id != provider_id || e.model != model);
        stick.answers.push(StickEntry {
            id: provider_id.to_string(),
            model: model.to_string(),
            at: now,
        });
        let over = stick.answers.len().saturating_sub(ANSWERS_KEPT);
        stick.answers.drain(..over); // 丢最老（表头）
        stick.back_to_main = false;
        // 全表超限：先丢过期，再丢最老（Go answeredStanding :1144-1150 同款两步）
        if table.len() > SESSIONS_KEPT {
            trim_table(&mut table, now);
        }
    }

    /// 把候选按「曾答账号浮最前」重排（Go affine :569 的纯排序子集）：按
    /// 最近回答序（最新在前）逐条浮起命中候选——先 (id, model) 精确匹配
    /// （Go :598），没有再退化到 id 宽匹配（Go :600 sameVendor 的近似：
    /// 模型下线/换模型时仍按账号粘）；未命中的候选保持原序（Go :723 只把
    /// 命中的挪到最前）。无粘性 / 回主标记置位 / 记录全过期 → 原样返回
    /// （拷贝；Go 里对应 Why="main"/超期不 keep 的分支）。
    pub(crate) fn affine(&self, session: &str, candidates: &[(String, String)]) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::with_capacity(candidates.len());
        let mut used = vec![false; candidates.len()];
        let table = self.table.lock().unwrap_or_else(|p| p.into_inner());
        let Some(stick) = table.get(session) else {
            return candidates.to_vec(); // 没人答过：first（Go :741-750 的新会话散开没搬）
        };
        // 过期粘性视作没有（Go :576 time.Since(st.at) > stickKeep → had=false）
        let recent: Vec<&StickEntry> = stick
            .answers
            .iter()
            .filter(|e| fresh(e.at, now_ms()))
            .collect();
        if stick.back_to_main || recent.is_empty() {
            return candidates.to_vec();
        }
        for entry in recent.into_iter().rev() {
            // 表尾最新 → rev 迭代即「最近回答序」
            let hit = (0..candidates.len())
                .find(|&i| !used[i] && candidates[i].0 == entry.id && candidates[i].1 == entry.model)
                .or_else(|| {
                    (0..candidates.len()).find(|&i| !used[i] && candidates[i].0 == entry.id)
                });
            if let Some(i) = hit {
                used[i] = true;
                out.push(candidates[i].clone());
            }
        }
        for (i, c) in candidates.iter().enumerate() {
            if !used[i] {
                out.push(c.clone());
            }
        }
        out
    }

    /// fallback 答过、主账号就绪后回主（Go backToMain :763 的显式形态）：
    /// 置位后 affine 不再浮起该会话的曾答账号，直到下一次 answered。
    /// 未知会话 no-op（没答过就无所谓回主）。
    pub(crate) fn back_to_main(&self, session: &str) {
        let mut table = self.table.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(stick) = table.get_mut(session) {
            stick.back_to_main = true;
        }
    }

    /// 立即落盘（原子写 tmp+rename，store.rs 同款；tmp 名带 pid 防同进程
    /// 互吞 rename）。写前清过期答者、裁每会话 8 / 全表 4096——顺手把
    /// 内存表也裁掉（Go saveSticks :349-359 落盘才裁、内存留给
    /// answeredStanding 清，这里合并成一处，语义等价）。
    pub(crate) fn save(&self) -> std::io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(()); // 无落盘路径（无数据目录）：纯内存态
        };
        let now = now_ms();
        // 表锁内取裁剪后的快照（Go saveSticks :347-353 锁内收集），锁外写
        let snapshot = {
            let mut table = self.table.lock().unwrap_or_else(|p| p.into_inner());
            for st in table.values_mut() {
                sanitize_session(st, now);
            }
            trim_table(&mut table, now);
            table.clone()
        };
        let body = serde_json::to_vec(&snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        // 一次一写，最后者胜（Go sticksSaving :208）；不与表锁互相等待
        let _one_write = self.saving.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, path)
    }

    /// 收尾尽力落盘（错误吞掉）：shutdown 路径与 Drop 用（Go flushSticks :252
    /// ——交接/重启后粘性还在）。粘性是优化不是账本，写丢了下场请求重新
    /// 路由而已，不值得为它报错。
    pub(crate) fn flush(&self) {
        let _ = self.save();
    }
}

impl Drop for Affinity {
    /// 进程退出尽力写一次（Go「flushSticks, as the gateway ends, loses
    /// nothing」:215-217；写失败静默）。
    fn drop(&mut self) {
        self.flush();
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 答者还在保鲜期内（Go stickKeep 30 天：回答会续期，长任务接上仍认得）。
fn fresh(at: u64, now: u64) -> bool {
    now.saturating_sub(at) <= STICK_KEEP_MS
}

/// 会话还有没有活着的答者记录（新鲜度 = 最近一次回答时间）。
fn session_fresh(st: &SessionStick, now: u64) -> bool {
    st.answers.iter().map(|e| e.at).max().is_some_and(|at| fresh(at, now))
}

/// 单会话收敛：清过期/无 id 的答者，留最近 ANSWERS_KEPT 条。
fn sanitize_session(st: &mut SessionStick, now: u64) {
    st.answers.retain(|e| !e.id.is_empty() && fresh(e.at, now));
    let over = st.answers.len().saturating_sub(ANSWERS_KEPT);
    st.answers.drain(..over);
}

/// 全表规模控制（Go answeredStanding :1144-1150 的超限清理 + saveSticks
/// :354-359 的最近 4096）：先丢整体过期（最近一次回答超 30 天）的会话，
/// 仍超限再按最近回答时间丢最老。
fn trim_table(table: &mut HashMap<String, SessionStick>, now: u64) {
    table.retain(|_, st| session_fresh(st, now));
    if table.len() > SESSIONS_KEPT {
        let mut recency: Vec<(String, u64)> = table
            .iter()
            .map(|(k, st)| (k.clone(), st.answers.iter().map(|e| e.at).max().unwrap_or(0)))
            .collect();
        recency.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0))); // 新→旧，同刻按键名稳定
        for (k, _) in recency.into_iter().skip(SESSIONS_KEPT) {
            table.remove(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// 候选 (provider, model)。
    fn c(p: &str, m: &str) -> (String, String) {
        (p.to_string(), m.to_string())
    }

    fn cs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(p, m)| c(p, m)).collect()
    }

    /// 无落盘路径的纯内存表。
    fn mem() -> Affinity {
        Affinity {
            path: None,
            table: Mutex::new(HashMap::new()),
            saving: Mutex::new(()),
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "marionette-affinity-{}-{}",
            tag,
            std::process::id()
        ))
    }

    // —— 会话种子提取 ——

    #[test]
    fn seed_from_body_prompt_cache_key() {
        // chat / responses 两方言同字段（chat.go:76 / responses.go:284）
        assert_eq!(
            stick_key(&[], &json!({"prompt_cache_key": "thread-1", "messages": []})).as_deref(),
            Some("thread-1")
        );
        assert_eq!(
            stick_key(&[], &json!({"prompt_cache_key": "  ", "input": []})),
            None,
            "空白值等同没有"
        );
        assert_eq!(stick_key(&[], &json!({"messages": []})), None, "无种子不粘");
    }

    #[test]
    fn seed_from_body_metadata_user_id() {
        // anthropic 方言（Claude Code 的用户标识，ir.go:381）
        assert_eq!(
            stick_key(&[], &json!({"metadata": {"user_id": "user_77"}, "messages": []})).as_deref(),
            Some("user_77")
        );
        assert_eq!(stick_key(&[], &json!({"metadata": {}})), None);
        // 同在体里：prompt_cache_key 优先于 metadata.user_id
        assert_eq!(
            stick_key(&[], &json!({"prompt_cache_key": "t", "metadata": {"user_id": "u"}})).as_deref(),
            Some("t")
        );
    }

    #[test]
    fn seed_header_priority_case_and_body_fallback() {
        // x-opencode-session 第一优先（openCodeSeed :30）→ X-Magpie-Session
        // （sessionOf :10390 排最前）→ sessionHeaders 列表序
        let h = headers(&[("x-magpie-session", "ms"), ("x-opencode-session", "oc"), ("x-session-id", "sid")]);
        assert_eq!(stick_key(&h, &json!({})).as_deref(), Some("oc"));
        let h = headers(&[("x-magpie-session", "ms"), ("x-session-id", "sid")]);
        assert_eq!(stick_key(&h, &json!({})).as_deref(), Some("ms"));
        let h = headers(&[("x-session-affinity", "pi"), ("x-session-id", "sid")]);
        assert_eq!(stick_key(&h, &json!({})).as_deref(), Some("pi"));
        let h = headers(&[("x-deepseek-harness-session-id", "dsh")]);
        assert_eq!(stick_key(&h, &json!({})).as_deref(), Some("dsh"));
        // 大小写不敏感 + 值 trim（Go http.Header / TrimSpace 同义）
        let h = headers(&[("X-SESSION-ID", "  sid  ")]);
        assert_eq!(stick_key(&h, &json!({})).as_deref(), Some("sid"));
        // 空值跳过看下一个
        let h = headers(&[("x-opencode-session", "  "), ("x-session-id", "sid")]);
        assert_eq!(stick_key(&h, &json!({})).as_deref(), Some("sid"));
        // 头优先于体的 prompt_cache_key
        let h = headers(&[("x-opencode-session", "oc-s1")]);
        assert_eq!(
            stick_key(&h, &json!({"prompt_cache_key": "thread-9"})).as_deref(),
            Some("oc-s1")
        );
    }

    #[test]
    fn seed_subagent_suffix() {
        // CC 子代理头（stickKey :555-558）：同会话 id 下子代理与主对话分键
        let h = headers(&[("x-session-id", "s1"), ("x-claude-code-agent-id", "sub7")]);
        assert_eq!(stick_key(&h, &json!({})).as_deref(), Some("s1|agent:sub7"));
        // 截 128（Go subagentOf :10447；按字符截保 UTF-8 边界，别裸截字节）
        let long = "x".repeat(300);
        let h = headers(&[("x-claude-code-agent-id", &long)]);
        let key = stick_key(&h, &json!({"prompt_cache_key": "p"})).unwrap();
        assert_eq!(key, format!("p|agent:{}", "x".repeat(128)));
        // 子代理头自己不成种：没别的种子照样 None
        let h = headers(&[("x-claude-code-agent-id", "sub7")]);
        assert_eq!(stick_key(&h, &json!({})), None);
    }

    // —— affine 排序 ——

    #[test]
    fn affine_floats_recent_answerers_first() {
        let aff = mem();
        aff.answered("s", "p1", "m1");
        aff.answered("s", "p2", "m2");
        aff.answered("s", "p1", "m1"); // p1 重复回答 → 挪到最新，不重复记
        {
            let table = aff.table.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(table["s"].answers.len(), 2, "同 (id, model) 只记一条");
        }
        let cands = cs(&[("p3", "m3"), ("p1", "m1"), ("p2", "m2")]);
        // 最近回答序：p1（最新）→ p2；其余 p3 保持原序在后
        assert_eq!(aff.affine("s", &cands), cs(&[("p1", "m1"), ("p2", "m2"), ("p3", "m3")]));
        // 没人答过的会话：原样
        assert_eq!(aff.affine("other", &cands), cands);
    }

    #[test]
    fn affine_prefers_exact_model_then_provider_wide() {
        // 精确优先：同 provider 两模型，精确命中的先浮（Go :598 先于 :600）
        let aff = mem();
        aff.answered("s", "p1", "mB");
        let cands = cs(&[("p0", "m0"), ("p1", "mA"), ("p1", "mB")]);
        assert_eq!(aff.affine("s", &cands), cs(&[("p1", "mB"), ("p0", "m0"), ("p1", "mA")]));
        // 宽匹配：答过的模型不在候选里，仍按账号粘（模型下线场景，Go :600）
        let aff = mem();
        aff.answered("s", "p1", "mB");
        let cands = cs(&[("p1", "mA"), ("p2", "mZ")]);
        assert_eq!(aff.affine("s", &cands), cs(&[("p1", "mA"), ("p2", "mZ")]));
    }

    #[test]
    fn affine_back_to_main_flag_skips_until_next_answer() {
        let aff = mem();
        aff.answered("s", "p1", "m1");
        aff.back_to_main("s");
        let cands = cs(&[("p2", "m2"), ("p1", "m1")]);
        assert_eq!(
            aff.affine("s", &cands),
            cands,
            "回主标记期间不浮起（Go Why=\"main\" :651-655 不 keep）"
        );
        aff.answered("s", "p1", "m1"); // 新回答清标记，粘性重建
        assert_eq!(aff.affine("s", &cands), cs(&[("p1", "m1"), ("p2", "m2")]));
        aff.back_to_main("unknown-session"); // 未知会话 no-op 不炸
    }

    #[test]
    fn expired_answers_do_not_stick() {
        let aff = mem();
        aff.answered("s", "p1", "m1");
        // 白盒塞老时间戳：31 天前 > stickKeep 30 天（Go :576 超期视作没有）
        {
            let mut table = aff.table.lock().unwrap_or_else(|p| p.into_inner());
            table.get_mut("s").unwrap().answers[0].at = now_ms() - STICK_KEEP_MS - 60_000;
        }
        let cands = cs(&[("p1", "m1"), ("p2", "m2")]);
        assert_eq!(aff.affine("s", &cands), cands, "过期粘性视作没有");
    }

    // —— 落盘 ——

    #[test]
    fn save_load_roundtrip_and_drop_flush() {
        let dir = temp_dir("rt");
        let path = dir.join("affinity.json");
        {
            let aff = Affinity::load_at(&path); // 文件还不存在 → 空表
            aff.answered("s1", "p1", "m1");
            aff.answered("s1", "p2", "m2");
            aff.back_to_main("s2"); // 未知会话 no-op，不落记录
            aff.save().expect("save");
            // save 后再记一笔，只靠 Drop 落盘（收尾尽力路径）
            aff.answered("s1", "p3", "m3");
        } // ← Drop：flush
        let aff = Affinity::load_at(&path);
        let cands = cs(&[("p2", "mX"), ("p1", "m1"), ("p3", "m3"), ("p2", "m2")]);
        let out = aff.affine("s1", &cands);
        assert_eq!(out[..3], cs(&[("p3", "m3"), ("p2", "m2"), ("p1", "m1")])[..], "Drop 落盘的那笔也在");
        assert_eq!(out[3], c("p2", "mX"), "没答过的候选垫底保持原序");
        assert_eq!(aff.affine("s2", &cands), cands, "未知会话无粘性");
        drop(aff);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_file_fails_open() {
        let dir = temp_dir("bad");
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("affinity.json");
        // 不是 JSON
        std::fs::write(&path, b"{not json at all").expect("write");
        let cands = cs(&[("p1", "m1")]);
        assert_eq!(
            Affinity::load_at(&path).affine("whatever", &cands),
            cands,
            "坏文件 fail-open 成空表"
        );
        // 是 JSON 但条目缺字段（Go decodeJSON 整档失败同口径：整表 fail-open）
        std::fs::write(&path, br#"{"s": {"answers": [{"model": "m", "at": 123}]}}"#).expect("write");
        assert_eq!(
            Affinity::load_at(&path).affine("s", &cands),
            cands,
            "缺字段的文件整表 fail-open"
        );
        // 空文件 / 空对象都是合法空表
        std::fs::write(&path, b"").expect("write");
        assert_eq!(Affinity::load_at(&path).affine("s", &cands), cands);
        std::fs::write(&path, b"{}").expect("write");
        assert_eq!(Affinity::load_at(&path).affine("s", &cands), cands);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caps_eight_per_session() {
        let aff = mem();
        for i in 0..10 {
            aff.answered("s", &format!("p{i}"), "m");
        }
        {
            let table = aff.table.lock().unwrap_or_else(|p| p.into_inner());
            let st = &table["s"];
            assert_eq!(st.answers.len(), ANSWERS_KEPT, "每会话最多 8 个答者");
            assert_eq!(st.answers[0].id, "p2", "丢最老");
            assert_eq!(st.answers[7].id, "p9", "留最新");
        }
        // 读入时也裁：外部写进来的 10 条收敛到最近 8 条
        let dir = temp_dir("cap8");
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("affinity.json");
        let now = now_ms();
        let entries: Vec<StickEntry> = (0..10)
            .map(|i| StickEntry { id: format!("p{i}"), model: "m".into(), at: now + i })
            .collect();
        std::fs::write(
            &path,
            serde_json::to_vec(&HashMap::from([("s".to_string(), SessionStick { answers: entries, back_to_main: false })]))
                .expect("json"),
        )
        .expect("write");
        let re = Affinity::load_at(&path);
        {
            let table = re.table.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(table["s"].answers.len(), ANSWERS_KEPT);
            assert_eq!(table["s"].answers[0].id, "p2");
        }
        drop(re);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lru_trims_table_to_4096_on_save() {
        let dir = temp_dir("lru");
        let path = dir.join("affinity.json");
        let aff = Affinity::load_at(&path);
        {
            // 白盒塞 4100 个会话，显式时间戳拉开先后（同毫秒打不出 LRU 序）
            let mut table = aff.table.lock().unwrap_or_else(|p| p.into_inner());
            let base = now_ms();
            for i in 0..4100u64 {
                table.insert(
                    format!("sess-{i:05}"),
                    SessionStick {
                        answers: vec![StickEntry { id: "p".into(), model: "m".into(), at: base + i }],
                        back_to_main: false,
                    },
                );
            }
        }
        aff.save().expect("save");
        let re = Affinity::load_at(&path);
        {
            let table = re.table.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(table.len(), SESSIONS_KEPT, "全表 LRU 上限（Go sticksKept :86）");
            assert!(!table.contains_key("sess-00000"), "丢最老");
            assert!(table.contains_key("sess-04099"), "留最新");
        }
        drop(re);
        {
            // save 顺手把内存活表也裁了（见 save 注释）
            let table = aff.table.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(table.len(), SESSIONS_KEPT);
        }
        drop(aff);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_reads_config_dir_affinity_json() {
        // 沙箱：XDG_CONFIG_HOME 是进程级 env，走 mod.rs 的跨模块共享串行锁
        // （quotas.rs / presets.rs 同款；绝不碰真实 ~/.config/magpie）
        let _lock = super::super::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = temp_dir("xdg");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("magpie")).expect("dir");
        let fresh_at = now_ms();
        std::fs::write(
            dir.join("magpie").join("affinity.json"),
            format!(r#"{{"s1": {{"answers": [{{"id": "p1", "model": "m1", "at": {}}}]}}}}"#, fresh_at),
        )
        .expect("write");
        let prior = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let aff = Affinity::load(); // config_dir() = XDG_CONFIG_HOME/magpie
        match &prior {
            Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let cands = cs(&[("p2", "m2"), ("p1", "m1")]);
        assert_eq!(aff.affine("s1", &cands), cs(&[("p1", "m1"), ("p2", "m2")]), "从 config_dir 读到粘性");
        drop(aff); // Drop 写回沙箱目录内，不外漏
        let _ = std::fs::remove_dir_all(&dir);
    }
}
