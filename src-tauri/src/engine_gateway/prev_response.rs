//! previous_response_id 的服务端状态（对齐 magpie `internal/gateway/prev_response.go`）。
//!
//! Responses 客户端可以只发本轮新 input、点名上一轮 response 的 id
//! （previous_response_id，OpenAI 自家 API 语义）。网关保存每个 response 的
//! 输入与输出、按 id 还原整段对话后再走翻译/遮罩/上游（Go previousResponse
//! :332）。本模块是这块的**纯逻辑 + 内存态**：存取、LRU、id 生成、
//! not-found 错误形状——不碰 HTTP。
//!
//! ## 与 Go 的差距（读源实锤，如实记录）
//!
//! - **Go 不落盘**：prev_response.go:33 "It is kept in memory alone"——包级
//!   单例 `prevResponses`（:85），进程重启历史全丢，客户端再点名的 id 只能拿
//!   显式 not-found。**本模块同样纯内存、不落盘，行为一致**。
//! - **容量口径不同**：Go 按字节 LRU——`prevCap = 256<<20`（256MB，:45）+
//!   `prevTTL = 6h`（:41），且相邻轮次共享的底层字节块只计一次（prevChunk
//!   :73 / kept :158）；单轮超过 cap/2 直接拒存（:108）。本模块按**条数**
//!   LRU（默认几百条，Go 没有条数上限可抄），无 TTL、无字节核算、不拒存
//!   大轮次——内存口径比 Go 粗，靠条数上限兜底；有压力再补字节口径。
//! - **无 scope 隔离**：Go 的键是 `(scope, id)`（:50；prevScope :218 按
//!   网关 key / 经手 magpie / 来源 IP 三段拼），一个网关 key 读不到另一个
//!   key 的 response。本模块只按 id——Marionette 引擎是单用户本机 loopback
//!   网关，scope 天然退化，多 key 场景由接线层自己加前缀。
//! - **不存路由元数据**：Go prevTurn 还带 upstreamID/route/made（:65-68，
//!   prevRoute :236 = provider id + sha256(key) 前八字节），供 prevDelta
//!   （:538）判断「答它的还是同一个自留 provider+key 时只发增量」。本模块
//!   只存 model/input/output；增量改写、上游拒认回退（prevRefused/prevForeign/
//!   prevElsewhere）、store=false 不保存（:375）全是 HTTP 层的事。
//! - **取回是整值克隆**：Go 的 items 是共享底层字节的 json.RawMessage 切片
//!   （:55）；这里 replay 返回 serde_json::Value 的深拷贝——简单，代价是
//!   大对话取回时的复制开销（条数上限内可接受）。
//! - **evict 是 O(n) 扫描**：Go 用双向链表 O(1)（order list :78）；这里几百
//!   条的 min-last_used 扫描可忽略。
//!
//! id 生成：Go **没有** `response_id.go` 生成器（那个文件是 usage 捕获）；
//! Responses 的 id 在 responses.go:2260/2588 由 `newID()`（anthropic.go:2080）
//! 拼出：`"resp_" + %x(UnixNano) + %08x(进程内原子序号)`——**没有随机段**
//! （时间戳 + 序号保证进程内唯一）。`fresh_id` 1:1 抄这个形状。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// 默认条数上限。Go 没有条数上限（它按 256MB 字节 + 6h TTL 计，见模块头）；
/// 这里取「几百」量级——一整段会话通常几十轮，够绕几圈，重启即清。
pub(crate) const DEFAULT_CAPACITY: usize = 512;

/// 一条被保存的 response：当时请求的 model、它的 input（整段对话条目）与
/// output（回复的输出条目）。Go prevTurn.items 的精简对应（input 与 output
/// 连成一段，:53-56）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StoredResponse {
    pub(crate) model: String,
    pub(crate) input: Value,
    pub(crate) output: Value,
}

/// LRU 计龄条目：`last_used` 是全局单调 tick，越大越新。
#[derive(Debug)]
struct Entry {
    resp: StoredResponse,
    last_used: u64,
}

struct Inner {
    map: HashMap<String, Entry>,
    tick: u64,
}

/// previous_response_id 的服务端保存表：条数上限的 LRU，命中刷新新鲜度
/// （Go prevStore.get 的 MoveToFront，:103）。
pub(crate) struct PrevStore {
    inner: Mutex<Inner>,
    capacity: usize,
}

impl PrevStore {
    /// 指定条数上限建表。容量 0 = 什么都不存（save 静默 no-op）。
    pub(crate) fn new(capacity: usize) -> PrevStore {
        PrevStore {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                tick: 0,
            }),
            capacity,
        }
    }

    /// 保存一条 response。同 id 再存 = 整条替换（Go put 先 drop 旧条目再
    /// PushFront，:113-117）；超过容量从最老挤起（Go :126-128 从链尾挤）。
    pub(crate) fn save(&self, id: &str, model: &str, input: Value, output: Value) {
        if self.capacity == 0 {
            return;
        }
        let mut inner = self.lock();
        inner.tick += 1;
        let tick = inner.tick;
        inner.map.insert(
            id.to_string(),
            Entry {
                resp: StoredResponse {
                    model: model.to_string(),
                    input,
                    output,
                },
                last_used: tick,
            },
        );
        while inner.map.len() > self.capacity {
            evict_oldest(&mut inner);
        }
    }

    /// 按 id 取回存储的输入输出（调用方把它并进新请求的历史）。
    ///
    /// **miss 语义写死（Go prevLost :305 + writePrevNotFound :313）**：返回
    /// None 时调用方**必须**回显式 not-found 错误（[`prev_not_found`]，Go 是
    /// HTTP 400 + OpenAI 形状，不是 404——OpenAI/Ark 自家口径，prev_response.go
    /// :244 实测 400 PreviousResponseNotFound），**绝不静默重放空历史**：claude/
    /// codex 账号这类不自留 response 的上游拿到被吞掉的 previous_response_id
    /// 会以为对话从零开始。
    pub(crate) fn replay(&self, id: &str) -> Option<StoredResponse> {
        let mut inner = self.lock();
        let tick = {
            inner.tick += 1;
            inner.tick
        };
        let entry = inner.map.get_mut(id)?;
        entry.last_used = tick; // Go get 的 MoveToFront（:103）
        Some(entry.resp.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // 模块内部只做 HashMap 操作，不会 panic，中毒只可能来自外部持锁 panic
        //（不存在）；unwrap 与 Go 的 mu.Lock 语义等价。
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for PrevStore {
    fn default() -> Self {
        PrevStore::new(DEFAULT_CAPACITY)
    }
}

/// 挤掉 last_used 最老的一条（容量几百的线性扫描，见模块头）。
fn evict_oldest(inner: &mut Inner) {
    if let Some(oldest) = inner
        .map
        .iter()
        .min_by_key(|(_, e)| e.last_used)
        .map(|(k, _)| k.clone())
    {
        inner.map.remove(&oldest);
    }
}

/// 新 id：`prefix + 十六进制纳秒时间戳 + 8 位十六进制序号`（Go newID，
/// anthropic.go:2080-2082，`%x%08x`——无随机段；序号 32 位回绕）。Responses
/// 的 response id 传 `"resp_"`（responses.go:2260/2588 同款形状），如
/// `resp_1942ff9b7c1d2e3f00000001`。
pub(crate) fn fresh_id(prefix: &str) -> String {
    static ID_SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed) & 0xffff_ffff;
    format!("{prefix}{nanos:x}{seq:08x}")
}

/// 「网关不持有该 previous_response_id」的显式错误（Go writePrevNotFound
/// :313-316 + writeErrorAs 的 Responses 分支 gateway.go:11430）：HTTP **400**
/// + OpenAI 形状。任务简报写的「404」按 Go 实际口径是 400——语义是显式
/// not-found、绝不静默重放空历史，接线层直接拿 (status, body) 回给客户端。
pub(crate) fn prev_not_found(id: &str) -> (u16, Value) {
    let msg = format!("Previous response with id '{id}' not found.");
    (
        400,
        json!({
            "error": {
                "message": msg,
                "type": "invalid_request_error",
                "code": "previous_response_not_found",
                "param": "previous_response_id",
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn resp(tag: &str) -> (Value, Value) {
        (
            json!([{"type": "message", "role": "user", "content": tag}]),
            json!([{"type": "message", "role": "assistant", "content": tag}]),
        )
    }

    #[test]
    fn save_replay_roundtrip() {
        let store = PrevStore::new(8);
        let (input, output) = resp("hello");
        store.save("resp_a", "grok/4.1", input.clone(), output.clone());
        let got = store.replay("resp_a").expect("刚存的 id 必须命中");
        assert_eq!(got.model, "grok/4.1");
        assert_eq!(got.input, input);
        assert_eq!(got.output, output);
        // 重复取回不消耗（Go get 可反复 MoveToFront）
        assert!(store.replay("resp_a").is_some());
    }

    #[test]
    fn replay_missing_returns_none() {
        let store = PrevStore::new(8);
        store.save("resp_a", "m", json!([]), json!([]));
        // miss 必须是 None——调用方转 prev_not_found，绝不静默空历史
        assert!(store.replay("resp_never").is_none());
        assert!(store.replay("").is_none());
    }

    #[test]
    fn lru_evicts_oldest_insert() {
        let store = PrevStore::new(3);
        for id in ["resp_a", "resp_b", "resp_c"] {
            let (i, o) = resp(id);
            store.save(id, "m", i, o);
        }
        let (i, o) = resp("resp_d");
        store.save("resp_d", "m", i, o);
        assert!(store.replay("resp_a").is_none(), "最老的 a 被挤出");
        for id in ["resp_b", "resp_c", "resp_d"] {
            assert!(store.replay(id).is_some(), "{id} 应还在");
        }
    }

    #[test]
    fn lru_touch_refreshes_recency() {
        let store = PrevStore::new(3);
        for id in ["resp_a", "resp_b", "resp_c"] {
            let (i, o) = resp(id);
            store.save(id, "m", i, o);
        }
        assert!(store.replay("resp_a").is_some(), "摸一下 a，a 变最新");
        let (i, o) = resp("resp_d");
        store.save("resp_d", "m", i, o);
        assert!(
            store.replay("resp_b").is_none(),
            "被挤出的是最久未用的 b，不是 a"
        );
        assert!(store.replay("resp_a").is_some());
    }

    #[test]
    fn save_replaces_same_id() {
        let store = PrevStore::new(2);
        let (i1, o1) = resp("one");
        store.save("resp_a", "m1", i1, o1);
        let (i2, o2) = resp("two");
        store.save("resp_a", "m2", i2.clone(), o2.clone());
        let got = store.replay("resp_a").expect("同 id 仍在");
        assert_eq!(
            (got.model.as_str(), got.input, got.output),
            ("m2", i2, o2),
            "同 id 再存 = 整条替换"
        );
        // 替换不吃额外容量：cap=2 再放一个新 id 也不该挤掉 resp_a
        let (i3, o3) = resp("three");
        store.save("resp_b", "m", i3, o3);
        assert!(store.replay("resp_a").is_some());
    }

    #[test]
    fn capacity_zero_stores_nothing() {
        let store = PrevStore::new(0);
        store.save("resp_a", "m", json!([]), json!([]));
        assert!(store.replay("resp_a").is_none());
    }

    #[test]
    fn concurrent_save_replay_do_not_crash() {
        let store = Arc::new(PrevStore::new(16));
        let a = store.clone();
        let b = store.clone();
        let t1 = thread::spawn(move || {
            for n in 0usize..2000 {
                let id = format!("resp_t1_{n}");
                let (i, o) = resp(&id);
                a.save(&id, "m", i, o);
                if n % 7 == 0 {
                    let _ = a.replay(&format!("resp_t1_{}", n.saturating_sub(1)));
                }
            }
        });
        let t2 = thread::spawn(move || {
            for n in 0..2000 {
                // 与 t1 同一批 id 上并发读 + 自己写自己的
                let _ = b.replay(&format!("resp_t1_{n}"));
                let id = format!("resp_t2_{n}");
                let (i, o) = resp(&id);
                b.save(&id, "m", i, o);
            }
        });
        t1.join().expect("并发 save/replay 不炸");
        t2.join().expect("并发 save/replay 不炸");
        // 容量有界：join 后灌满 capacity 条哨兵，4000 条并发写入必然全被挤出
        for n in 0..16 {
            let id = format!("resp_sentinel_{n}");
            let (i, o) = resp(&id);
            store.save(&id, "m", i, o);
        }
        for id in ["resp_t1_0", "resp_t1_1999", "resp_t2_0", "resp_t2_1999"] {
            assert!(store.replay(id).is_none(), "{id} 应已被哨兵挤出");
        }
        assert!(store.replay("resp_sentinel_15").is_some());
    }

    #[test]
    fn fresh_id_shape_and_unique() {
        // Go 形状：prefix + 小写十六进制纳秒 + 8 位零垫十六进制序号，无随机段
        let id = fresh_id("resp_");
        let rest = id.strip_prefix("resp_").expect("前缀原样保留");
        assert!(
            rest.len() > 8
                && rest
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "全小写 hex、且有序号段：{rest}"
        );
        // 同纳秒下靠序号保证进程内唯一（Go idSeq 同款）
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(fresh_id("resp_")), "连续生成的 id 不得重复");
        }
        assert_eq!(
            fresh_id("rs_").starts_with("rs_"),
            true,
            "前缀可换（rs_ 形状给 reasoning 等）"
        );
    }

    #[test]
    fn prev_not_found_shape() {
        let (status, body) = prev_not_found("resp_abc123");
        assert_eq!(status, 400, "Go 是 400（OpenAI 自家口径），不是 404");
        let err = body.get("error").expect("OpenAI 形状：error 包一层");
        assert_eq!(err["code"], "previous_response_not_found");
        assert_eq!(err["param"], "previous_response_id");
        assert_eq!(err["type"], "invalid_request_error");
        assert_eq!(
            err["message"],
            "Previous response with id 'resp_abc123' not found."
        );
    }
}
