//! usage 台账——magpie `internal/usage` 的单模块 Rust 瘦身版（engine-rust
//! 批次的纯模块：mod.rs 注册与端点接线在主会话做，本模块只依赖 std +
//! serde/serde_json，不 import 主程序其它模块）。
//!
//! Go 侧链路（行号按本仓库 third_party/magpie 当前 checkout）：网关在
//! serve 尽头落一行 `usage.Record`（成功路径 gateway.go:4685 一带的
//! `rec := usage.Record{RouteID, Time, Agent, Provider, Model, Input,
//! Output, CacheRead, CacheWrite, Millis, Status, Session, Kind, …}`）；
//! 提前拒答（未出站就回绝）走 `logged`/`turnedAway` 这对闭包
//! （gateway.go:2672-2690——简报里的 :2619/:2627 是同一对，行号有漂移），
//! 落一行 `Rejected: true` 的记录。两路都经 `appendUsage`（lan.go:866：
//! session/子代理/调用方钥匙在这里补齐）交给 `usage.Append`
//! （usage.go:325）。简报称其为 sqlite 台账；本 checkout 的 Append 实际
//! 是 `usage.jsonl` 的 JSONL 追加写（usage.go:315 `Path()`，错误全部吞掉
//! ——「记账绝不能弄断一次调用」，usage.go:323-324），加速层另有日志索引
//! （log_index.go/ledger.go）。Rust 版与 JSONL 主账同构，不带索引。
//!
//! ## 捕获层（2026-11-18 接上，Go usageSniffer/usageEncoder 的最小组合）
//!
//! usage 四桶从**客户端面应答流**里提：Go 在 relay 拷贝路径上挂
//! `usageSniffer`（sniff.go:11——逐行读正在拷给客户端的字节，SSE 按
//! `data:` 事件解析、非 SSE 整块缓存到尾部一次解析），`usageEncoder`
//! （response_id.go:8）在流编码器外再包一层读回执 id。Rust 对应
//! [`TapReader`]：包在 relay 链最外层（chat.rs 的
//! `TapReader(ClosingReader(ChattidyReader(...)))`、responses.rs /
//! messages.rs 的翻译输出流），逐字节过一遍**不复制载荷**——只有含
//! `"usage"` 关键字的行才解析 JSON（chat.rs chattidy 同款触发序），
//! 桶按方言提（chat：sniff.go:187-203 + chat.go:1142/1183 cUsage；
//! responses：sniff.go:204-242 + responses.go:1659/1675 rUsage；anthropic：
//! sniff.go:243-269 + anthropic.go:1519/1570 aUsage），多帧**后到的非零桶
//! 覆盖**（ir.go:652 `Usage.add`）。落账时机在**流尾收口**——reader
//! EOF / 读错误 / drop（客户端断开）都落，已发字节算已服务（Go serve
//! 尽头一口账的口径），millis = 回合起点→流尾。厂商没报的桶就是 None：
//! 显式 0 与缺字段一律不记（Go `add` 只认 >0，同款）。
//!
//! ## 与 Go 的差距清单（有意不做，接端点前先过一遍）
//!
//! - **无 sqlite / 无日志索引**：Go 侧 `Load`（usage.go:416）全量流式扫
//!   usage.jsonl、热路径另有内存/磁盘索引；Rust 的 [`UsageLedger::records_since`]
//!   只扫文件尾部 4MB——更早的窗口查不到，轮转掉进 .1 的历史也不回看。
//! - **无 RouteID 关联**：Go `Record.RouteID` 把一次请求的多次 try 串起来
//!   （Tries/HandedOn 语义，usage.go Record 首字段）；Rust 一次调用一行，
//!   没有 handed-on 行，失败 try 的中间账不在本台账。
//! - **无 tier / ttft 侧字段**：TierAsked/TierSent/TierServed、TTFT/
//!   FirstText/Flow/Sent/Own/Retried/Queued 等 Record 的服务档与时间分解
//!   侧字段全没有——本台账只记任务口径的 13 个字段。流断掉也只记它的
//!   HTTP 状态（200）——Go 的 Error/ErrType 侧字段（sniff.go:26-31
//!   `failed`）没有对应物，断流与正常收尾在账面上不可分。
//! - **无价格折算与字节数**：CostAt、UpBytes/DownBytes、CacheWrite1h/
//!   CacheUnknown、Credits、Reasoning 一概不记；四桶 token 用 `Option`
//! ——「厂商没报就是 None，不编 0」。
//! - **轮转只有单代**：Go 靠 backup_log.go / cold_split.go 冷热分档；
//!   Rust 只在启动时 >8MB 把 usage.jsonl 换名成 .1（旧 .1 删），没有
//!   冷热分档，也没有多代。
//! - **会话字段口径不同**：Go 的 Session 是 agent 自报的会话头
//!   （lan.go:867-869 `sessionOf(r.Header)` 兜底）；Rust 网关按任务口径
//!   存 affinity 种子同源的 `prompt_cache_key` / `metadata.user_id`
//!   （affinity.rs `stick_key` 的提取序，affinity.rs:91-99）。
//!   responses/messages 端点的路由层（routes.rs）不传请求头，这两方言
//!   只取 body 侧的种子（`stick_key(&[], …)`）——头携带的会话
//!   （x-opencode-session 等）在网关记账里缺失，等路由层接线后补。
//! - **提前拒答行不落账**：Go turnedAway（gateway.go:2672-2690）给未出站
//!   就回绝的请求落 `Rejected:true` 行；Rust 目前只在 2xx 应答路径记
//!   （chat 直通 + responses/messages 直通与翻译），4xx/5xx 透传、候选
//!   循环的中间失败 try 都不记——与「无 RouteID」条目同根。
//! - **落盘是异步的**：`record()` 只入内存 ring + 排队，写线程逐行 append
//!   ——进程被杀最多丢尾部几行（与 Go「Append 在请求路径上同步写」不同，
//!   换请求路径零等待；丢尾可接受，见 `append_line` 注释）。

use std::collections::{HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// 台账文件名：`config_dir()/usage.jsonl`（与 Go usage.go:315 `Path()`
/// 同名同位——providers.json 旁边）。
const USAGE_FILE: &str = "usage.jsonl";
/// 轮转后的名字（单代：旧 .1 删）。
const USAGE_ROTATED: &str = "usage.jsonl.1";
/// 内存 ring 上限：最近 2000 条（任务口径）。
const RING_CAP: usize = 2000;
/// 启动轮转阈值：>8MB 换名成 .1。
const ROTATE_BYTES: u64 = 8 * 1024 * 1024;
/// `records_since` 读文件时的尾部扫描上限（4MB）：文件更大时只看尾部，
/// 更早的窗口查不到（Go 有全量 Load + 索引，见模块头差距清单第一条）。
const TAIL_SCAN_BYTES: u64 = 4 * 1024 * 1024;

/// 一通调用（Go `usage.Record` 的任务口径子集）。JSON 字段 camelCase
/// （ts/agent/provider/model/millis/status/rejected/session/kind/
/// inTokens/outTokens/cacheRead/cacheWrite）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UsageRecord {
    /// 毫秒墙钟。0 = `record()` 落账时补 now（usage.go:326-328 的
    /// `if r.Time.IsZero()` 同款）。**反序列化必填**——空壳 `{}`、异构
    /// JSON 行当坏行跳过（deny_unknown_fields + 必填共同把关）。
    pub ts: u64,
    /// 入站方言（chat / responses / anthropic）。Go `Record.Agent` 记的
    /// 是 magpie agent id 或客户端产品名；Rust 网关按任务口径记方言。
    pub agent: String,
    /// 来源 id。提前拒答时可能为空（turnedAway 没到任何来源；
    /// Go usage.go:404 `IsRejected` 对「provider 空且 status>=400」的
    /// 旧行兜底认账，我们显式记 `rejected`，不需要这个推断）。
    #[serde(default)]
    pub provider: String,
    /// 剥掉 `provider/`、`marionette::` 等前缀后的模型 id（调用方剥好
    /// 再传，台账只存裸名——Go `unprefixed(modelOf(body))` 同口径）。
    pub model: String,
    /// 往返耗时（毫秒）。
    #[serde(default)]
    pub millis: u64,
    /// HTTP 状态码（拒答行是回给客户端的那个码，如 429）。
    pub status: u16,
    /// 未出站就拒答（gateway.go:2674 `Rejected: true` 的那路）。
    #[serde(default)]
    pub rejected: bool,
    /// 会话归属：affinity 种子同源（请求的 `prompt_cache_key` /
    /// `metadata.user_id`，affinity.rs:91-99 的提取序），可空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// 这通调用的用途：turn / compact / rejected（Go `Record.Kind` 的
    /// 子集；compact 是 agent 压缩对话自己发起的调用，gateway.go:2610 一带）。
    #[serde(default)]
    pub kind: String,
    /// usage 四桶（厂商应答可得时才有；不可得就是 None，不编数）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<u64>,
}

/// 写线程的消息：一行台账，或一道屏障（关停/测试等排队的行落完）。
enum Msg {
    Line(String),
    Sync(Sender<()>),
}

/// usage 台账：进程内 ring（最近 [`RING_CAP`] 条）+ 异步 JSONL 落盘。
///
/// `record()` 绝不阻塞在磁盘上（Go 把 Append 放在请求路径上同步写，
/// usage.go:325——Rust 版按任务口径改成排队、写线程逐行 append）。
/// 落盘失败安静计数（[`UsageLedger::write_failures`]），不 panic 不刷屏，
/// dev 也不打印——记账失败不该弄断调用，也不该淹没日志。
///
/// **Clone 是句柄不是副本**：内芯在 `Arc` 里，克隆共享同一 ring / 写线程
/// ——流式应答的 reader（`'static` + `Send`，拿不住 `&Gateway`）揣一份
/// 克隆就能在流尾落账（tap 捕获层，见 [`TapReader`]）。
#[derive(Clone)]
pub(crate) struct UsageLedger {
    inner: Arc<Inner>,
}

struct Inner {
    ring: Mutex<VecDeque<UsageRecord>>,
    /// 写线程的投递口；None = 没有可用 config_dir（纯内存，不算失败）。
    tx: Option<Mutex<Sender<Msg>>>,
    /// 台账文件路径（open 时定死，查询/写盘共用，不再重读 env）。
    path: Option<PathBuf>,
    write_failures: Arc<AtomicU64>,
}

impl UsageLedger {
    /// 打开台账：启动轮转（>8MB → .1）+ 起写线程。没有 config_dir 时
    /// 退化为纯内存台账（record 照常、查文件侧永远空）。
    pub(crate) fn open() -> UsageLedger {
        let path = super::config_dir().map(|d| d.join(USAGE_FILE));
        let write_failures = Arc::new(AtomicU64::new(0));
        let tx = path.as_ref().map(|path| {
            rotate_if_huge(path);
            let (tx, rx) = std::sync::mpsc::channel::<Msg>();
            let failures = Arc::clone(&write_failures);
            let path = path.clone();
            // 写线程：按序逐行 append。进程退出不 Drop 台账时它随进程一起
            // 消失——最多丢尾部几行排队中的记录（与 append 的原子性口径
            // 一致，见 append_line）。
            let _ = std::thread::Builder::new()
                .name("engine-usage-ledger".into())
                .spawn(move || {
                    while let Ok(msg) = rx.recv() {
                        match msg {
                            Msg::Line(line) => append_line(&path, &line, &failures),
                            // 屏障到点 = 它之前的行都已 append（通道 FIFO）
                            Msg::Sync(reply) => {
                                let _ = reply.send(());
                            }
                        }
                    }
                });
            Mutex::new(tx)
        });
        UsageLedger {
            inner: Arc::new(Inner {
                ring: Mutex::new(VecDeque::new()),
                tx,
                path,
                write_failures,
            }),
        }
    }

    /// 落一行账（Go usage.go:325 `Append` + lan.go:866 `appendUsage` 的
    /// 合并入口——补 session/钥匙那些事在网关层做，这里只管记）。
    /// ts=0 补墙钟；内存 ring 同步入列，磁盘行排队异步写。
    pub(crate) fn record(&self, r: UsageRecord) {
        let mut r = r;
        if r.ts == 0 {
            r.ts = now_ms();
        }
        // 序列化只可能因 struct 本身失败（不会发生）——失败就只留内存。
        let line = serde_json::to_string(&r).ok();
        {
            let mut ring = self.inner.ring.lock().unwrap_or_else(|p| p.into_inner());
            ring.push_back(r);
            while ring.len() > RING_CAP {
                ring.pop_front();
            }
        }
        if let (Some(tx), Some(line)) = (&self.inner.tx, line) {
            let tx = tx.lock().unwrap_or_else(|p| p.into_inner());
            // 接收端死了（线程起不来/进程正在退）算一次写失败，安静计数
            if tx.send(Msg::Line(line)).is_err() {
                self.inner.write_failures.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// 查询：ts >= since_ms 的记录，按 ts 升序。先内存；内存的窗口不够
    /// （ring 空、或最早一条都晚于 since——可能有被逐出的旧行）时补读
    /// 文件尾部 [`TAIL_SCAN_BYTES`]，按序列化形态与内存去重（写线程还没
    /// 落盘的最新几条只在内存里，文件里的又会与 ring 重叠）。
    ///
    /// 「内存够」的判据是 ring 最早一条 ts <= since：墙钟毫秒单调，比它
    /// 更早的行不可能再出现——文件侧不用看（Go Load 没有这层捷径，
    /// 它每次全量扫，usage.go:416）。
    pub(crate) fn records_since(&self, since_ms: u64) -> Vec<UsageRecord> {
        let (memory, covers) = {
            let ring = self.inner.ring.lock().unwrap_or_else(|p| p.into_inner());
            let memory: Vec<UsageRecord> =
                ring.iter().filter(|r| r.ts >= since_ms).cloned().collect();
            let covers = ring.front().is_some_and(|r| r.ts <= since_ms);
            (memory, covers)
        };
        if covers {
            return memory;
        }
        let Some(path) = &self.inner.path else {
            return memory; // 纯内存台账
        };
        let mut seen: HashSet<String> = memory
            .iter()
            .filter_map(|r| serde_json::to_string(r).ok())
            .collect();
        let mut out = memory;
        for rec in read_tail_records(path) {
            if rec.ts < since_ms {
                continue;
            }
            // 同一条记录两处出现时序列化形态相同（同一 derive、同一字段序）
            if serde_json::to_string(&rec)
                .map(|k| seen.insert(k))
                .unwrap_or(false)
            {
                out.push(rec);
            }
        }
        out.sort_by_key(|r| r.ts); // 稳定排序：同 ts 保持到达序
        out
    }

    /// 屏障：等写线程把已排队的行全部落盘（通道 FIFO，回执到 = 之前的
    /// 行都已 append）。优雅关停与测试用；写线程不在时立即返回。
    pub(crate) fn flush(&self) {
        let Some(tx) = &self.inner.tx else { return };
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let tx = tx.lock().unwrap_or_else(|p| p.into_inner());
        if tx.send(Msg::Sync(reply_tx)).is_ok() {
            // 写线程异常卡死时别跟着挂死：5s 放弃（屏障尽力而为）
            let _ = reply_rx.recv_timeout(std::time::Duration::from_secs(5));
        }
    }

    /// 落盘失败计数（安静，不打印）。
    pub(crate) fn write_failures(&self) -> u64 {
        self.inner.write_failures.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// 捕获层（Go sniff.go usageSniffer + response_id.go usageEncoder 的最小组合）
// ---------------------------------------------------------------------------

/// 客户端面应答流的方言——决定从哪类事件里提哪个桶（Go sniff.go:160 的
/// `switch s.proto`：按协议挑 usage 的落点）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Dialect {
    /// openai chat 线协议：SSE 尾帧 / 整块的 `usage` 对象（sniff.go:187-203）。
    Chat,
    /// openai responses 线协议：response.completed 事件的
    /// `response.usage`（事件流）或顶层 `usage`（整块）（sniff.go:204-242）。
    Responses,
    /// anthropic messages 线协议：message_start 的 `message.usage`
    /// （input + 缓存两桶）+ message_delta 的顶层 `usage`（output 桶）
    /// （sniff.go:243-269）。
    Anthropic,
}

/// 四桶的原生形态（Go `Usage` 的任务口径子集）。0 = 没说——入账时经
/// [`UsageTap::add`] 的 >0 过滤转成 `Option`，不编数。
#[derive(Default, Clone, Copy)]
struct Buckets {
    in_tokens: u64,
    out_tokens: u64,
    cache_read: u64,
    cache_write: u64,
}

/// chat 方言的桶（Go chat.go:1142 cUsage / :1183 usage()，与 chattidy.rs
/// `mend_usage` 同一读取序）：`prompt_tokens` 含缓存（OpenAI 口径，
/// `whole=true`）→ in = prompt − read − write；缓存读优先
/// `prompt_tokens_details.cached_tokens`，回落 DeepSeek/Moonshot/Mistral
/// 的别名，最后 Anthropic 的名字（那对名字里 prompt 只数非缓存——
/// `prompt >= read+write` 不成立时 prompt 原样当净额）。
fn chat_buckets(u: &serde_json::Value) -> Buckets {
    let g = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let prompt = g("prompt_tokens");
    let details = u.get("prompt_tokens_details").and_then(|d| d.as_object());
    let dn = |k: &str| details.and_then(|d| d.get(k)).and_then(|v| v.as_u64()).unwrap_or(0);
    let mut read = dn("cached_tokens");
    let mut write = dn("cache_write_tokens");
    if read == 0 {
        for k in ["prompt_cache_hit_tokens", "cached_tokens", "num_cached_tokens"] {
            let n = g(k);
            if n != 0 {
                read = n;
                break;
            }
        }
    }
    let mut whole = true; // prompt_tokens 含缓存（OpenAI 口径）
    if read == 0 && write == 0 {
        read = g("cache_read_input_tokens");
        write = g("cache_creation_input_tokens");
        whole = prompt >= read + write;
    }
    let input = if whole { prompt.saturating_sub(read + write) } else { prompt };
    Buckets { in_tokens: input, out_tokens: g("completion_tokens"), cache_read: read, cache_write: write }
}

/// responses 方言的桶（Go responses.go:1659 rUsage / :1675 usage()）：
/// `input_tokens` 是整个 prompt（含缓存读与写在内）→ in = input −
/// cached − write（夹 0）；`cache_write_tokens` 是 Codex 也读的缓存写桶。
fn responses_buckets(u: &serde_json::Value) -> Buckets {
    let g = |p: &str| u.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0);
    let read = g("/input_tokens_details/cached_tokens");
    let write = g("/input_tokens_details/cache_write_tokens");
    Buckets {
        in_tokens: g("/input_tokens").saturating_sub(read + write),
        out_tokens: g("/output_tokens"),
        cache_read: read,
        cache_write: write,
    }
}

/// anthropic 方言的桶（Go anthropic.go:1519 aUsage / :1570 usage()）：
/// `input_tokens` 本就只数非缓存（Go ir.go:647 `prompt()` 注释），原样进
/// in；缓存写取 `cache_creation_input_tokens` 与 `cache_creation`
/// ephemeral 两档之和的较大者（Anthropic 按 5m/1h 分开计费，台账不分档）。
fn anthropic_buckets(u: &serde_json::Value) -> Buckets {
    let g = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let mut write = g("cache_creation_input_tokens");
    if let Some(c) = u.get("cache_creation").and_then(|c| c.as_object()) {
        let e = |k: &str| c.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        write = write.max(e("ephemeral_5m_input_tokens") + e("ephemeral_1h_input_tokens"));
    }
    Buckets {
        in_tokens: g("input_tokens"),
        out_tokens: g("output_tokens"),
        cache_read: g("cache_read_input_tokens"),
        cache_write: write,
    }
}

fn has_usage(bytes: &[u8]) -> bool {
    bytes.windows(5).any(|w| w == b"usage")
}

fn as_obj(x: Option<&serde_json::Value>) -> Option<&serde_json::Value> {
    x.filter(|x| x.is_object())
}

/// 四桶的入账形态：None = 厂商没说（不编 0）。
#[derive(Default, Clone, Copy)]
struct TapBuckets {
    in_tokens: Option<u64>,
    out_tokens: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
}

/// 一次回合的流式 usage 捕获 + 落账器（Go sniff.go usageSniffer 的桶半边
/// + response_id.go 在 finish 处收口的形状）。流式由 [`TapReader`] 喂行，
/// 非流式整块走 [`UsageTap::whole`]；[`UsageTap::finish`] 幂等，EOF / 读
/// 错误 / drop 都只落一行（millis = 回合起点→收口，status = 客户端看到的
/// HTTP 状态——流中途死掉已发字节算已服务，照实记）。
pub(crate) struct UsageTap {
    ledger: UsageLedger,
    dialect: Dialect,
    agent: &'static str,
    provider: String,
    model: String,
    session: Option<String>,
    kind: String,
    status: u16,
    began: Instant,
    buckets: TapBuckets,
    done: bool,
}

impl UsageTap {
    /// 一条 SSE data 行（或整块 JSON 的根对象）——含 `"usage"` 关键字才
    /// 进解析（chattidy.rs `line_carries` 同款预筛），坏行/非 data 行安静
    /// 跳过，绝不因捕获断流（Go sniffer 的 `json.Unmarshal == nil` 守卫）。
    pub(crate) fn note_line(&mut self, line: &[u8]) {
        if !has_usage(line) {
            return;
        }
        let Ok(text) = std::str::from_utf8(line) else { return };
        let Some(data) = text.trim_start().strip_prefix("data:") else { return };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data.trim()) else { return };
        self.note_event(&v);
    }

    /// 一个已解析的事件/整块对象，按方言挑 usage 落点（Go sniff.go:160）。
    fn note_event(&mut self, v: &serde_json::Value) {
        match self.dialect {
            Dialect::Chat => {
                if let Some(u) = as_obj(v.get("usage")) {
                    self.add(chat_buckets(u));
                }
            }
            Dialect::Responses => {
                // 事件流的 completed 在 response.usage，整块应答在顶层；
                // 事件优先（sniff.go:237-241 的 else-if 同序）。
                let u = as_obj(v.pointer("/response/usage")).or_else(|| as_obj(v.get("usage")));
                if let Some(u) = u {
                    self.add(responses_buckets(u));
                }
            }
            Dialect::Anthropic => {
                // message_start 的 message.usage（input + 缓存）与
                // message_delta 的顶层 usage（output）两处都提——后到的
                // 非零桶覆盖（sniff.go:263-268 两个 if 同款）。
                if let Some(u) = as_obj(v.pointer("/message/usage")) {
                    self.add(anthropic_buckets(u));
                }
                if let Some(u) = as_obj(v.get("usage")) {
                    self.add(anthropic_buckets(u));
                }
            }
        }
    }

    /// 非流式整块应答：一次提取 + 落账（Go sniff.go:321 `usage()` 对非 SSE
    /// 体整块 parse 的口径）。体里没有 `"usage"` 关键字时只落骨架行。
    pub(crate) fn whole(&mut self, body: &[u8]) {
        if has_usage(body) {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
                self.note_event(&v);
            }
        }
        self.finish();
    }

    /// Go ir.go:652 `Usage.add`：后到的**非零**桶覆盖，零不清洗已见非零值
    /// ——显式 0 与缺字段在账面上都是 None，不编数。
    fn add(&mut self, b: Buckets) {
        let Buckets { in_tokens, out_tokens, cache_read, cache_write } = b;
        if in_tokens > 0 {
            self.buckets.in_tokens = Some(in_tokens);
        }
        if out_tokens > 0 {
            self.buckets.out_tokens = Some(out_tokens);
        }
        if cache_read > 0 {
            self.buckets.cache_read = Some(cache_read);
        }
        if cache_write > 0 {
            self.buckets.cache_write = Some(cache_write);
        }
    }

    /// 收口落账（幂等）：millis = 回合起点→此刻，四桶按 Option 如实。
    pub(crate) fn finish(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        let b = self.buckets;
        self.ledger.record(UsageRecord {
            ts: 0,
            agent: self.agent.into(),
            provider: std::mem::take(&mut self.provider),
            model: std::mem::take(&mut self.model),
            millis: self.began.elapsed().as_millis() as u64,
            status: self.status,
            rejected: false,
            session: std::mem::take(&mut self.session),
            kind: std::mem::take(&mut self.kind),
            in_tokens: b.in_tokens,
            out_tokens: b.out_tokens,
            cache_read: b.cache_read,
            cache_write: b.cache_write,
        });
    }
}

/// 一次回合的记账上下文（端点在请求入口建一次，候选循环里按胜出成员
/// 开 tap / 整块扫描——provider 是每次尝试才知道的，按点传）。
pub(crate) struct TurnLog {
    pub agent: &'static str,
    /// 客户端问的模型串（asked，含组名/provider 前缀——与 chat 直通
    /// note 系口径一致）。
    pub model: String,
    pub session: Option<String>,
    pub began: Instant,
}

impl TurnLog {
    /// `began` 是**请求入口**的钟（端点在 handle 一进来就取）——millis 覆盖
    /// 候选失败转移的耗时（Go `Millis: call.Millis` 从 serve 起算的同口径），
    /// 不是 tap 创建那一刻。
    pub(crate) fn new(
        agent: &'static str,
        model: &str,
        session: Option<String>,
        began: Instant,
    ) -> TurnLog {
        TurnLog { agent, model: model.to_string(), session, began }
    }

    /// 开一个流式 tap（配合 [`TapReader`] 包在 relay 链上）。
    pub(crate) fn tap(
        &self,
        ledger: UsageLedger,
        dialect: Dialect,
        provider: &str,
        status: u16,
    ) -> UsageTap {
        UsageTap {
            ledger,
            dialect,
            agent: self.agent,
            provider: provider.to_string(),
            model: self.model.clone(),
            session: self.session.clone(),
            kind: "turn".into(),
            status,
            began: self.began,
            buckets: TapBuckets::default(),
            done: false,
        }
    }

    /// 非流式整块应答：一次提取 + 落账。
    pub(crate) fn whole(
        &self,
        ledger: &UsageLedger,
        dialect: Dialect,
        provider: &str,
        status: u16,
        body: &[u8],
    ) {
        let mut tap = self.tap(ledger.clone(), dialect, provider, status);
        tap.whole(body);
    }
}

/// 捕获行缓冲上限：跨 chunk 断开且超过它的行不再缓存（Go sniff.go:73
/// 「a runaway line is not one we can use」——超长行只会是工具参数分片，
/// usage 帧是尾帧、很小）。
const TAP_LINE_MAX: usize = 256 * 1024;

/// 流捕获 reader（Go 把 sniffer 包在 relay writer 上；Rust 写层只认
/// reader，对应地包在链尾）：字节照转，含 `"usage"` 的行解析提桶，EOF /
/// 读错误 / drop 时 [`UsageTap::finish`] 落账——**流中途死掉也落**
/// （已发字节算已服务，status 照实）。
///
/// 开销红线：载荷不整段复制——chunk 内的完整行按子串零拷贝过，只有跨
/// chunk 断开的行尾暂存（上限 [`TAP_LINE_MAX`]）；JSON 解析只在含
/// `"usage"` 关键字的行上做。
pub(crate) struct TapReader<R: Read> {
    inner: R,
    tap: UsageTap,
    /// 跨 chunk 未完结行的残段（行完结即清空）。
    partial: Vec<u8>,
    /// 当前跨 chunk 行是否已判死（超限）——判死后整行放过直到行尾。
    line_dead: bool,
    /// 当前是否处于一行中间（partial 属于它）。
    in_line: bool,
}

impl<R: Read> TapReader<R> {
    pub(crate) fn new(tap: UsageTap, inner: R) -> TapReader<R> {
        TapReader { inner, tap, partial: Vec::new(), line_dead: false, in_line: false }
    }

    /// 喂一段刚读到的字节（零拷贝逐行；跨 chunk 行经 partial 缓冲）。
    fn scan(&mut self, chunk: &[u8]) {
        let mut rest = chunk;
        if self.in_line {
            match rest.iter().position(|&b| b == b'\n') {
                None => {
                    // 整段仍是残行的一部分。
                    if !self.line_dead {
                        if self.partial.len() + rest.len() > TAP_LINE_MAX {
                            self.partial.clear();
                            self.line_dead = true;
                        } else {
                            self.partial.extend_from_slice(rest);
                        }
                    }
                    return;
                }
                Some(i) => {
                    if !self.line_dead {
                        if self.partial.len() + i <= TAP_LINE_MAX {
                            self.partial.extend_from_slice(&rest[..i]);
                            let line = std::mem::take(&mut self.partial);
                            self.tap.note_line(&line);
                        }
                    }
                    self.partial.clear();
                    self.line_dead = false;
                    self.in_line = false;
                    rest = &rest[i + 1..];
                }
            }
        }
        // chunk 内的完整行：子串直扫，零拷贝。
        let mut start = 0;
        for (i, &b) in rest.iter().enumerate() {
            if b == b'\n' {
                self.tap.note_line(&rest[start..i]);
                start = i + 1;
            }
        }
        // 尾部未完结段：暂存等下一个 chunk（EOF 冲刷在 read 的 Ok(0) 分支）。
        let tail = &rest[start..];
        if !tail.is_empty() {
            self.in_line = true;
            if tail.len() > TAP_LINE_MAX {
                self.line_dead = true;
            } else {
                self.partial.extend_from_slice(tail);
            }
        }
    }

    /// EOF 冲刷：最后一行可能没有行尾符（截断流），照提再落账。
    fn flush_line(&mut self) {
        if self.in_line && !self.line_dead && !self.partial.is_empty() {
            let line = std::mem::take(&mut self.partial);
            self.tap.note_line(&line);
        }
        self.partial.clear();
        self.in_line = false;
        self.line_dead = false;
    }
}

impl<R: Read> Read for TapReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match self.inner.read(buf) {
                Ok(0) => {
                    self.flush_line();
                    self.tap.finish();
                    return Ok(0);
                }
                Ok(n) => {
                    self.scan(&buf[..n]);
                    return Ok(n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    // 断流：已发字节算已服务，落账后再上抛错误。
                    self.tap.finish();
                    return Err(e);
                }
            }
        }
    }
}

impl<R: Read> Drop for TapReader<R> {
    fn drop(&mut self) {
        // 没读到 EOF 就被丢（客户端断开、上层提前收口）：照实落一行
        //（Go 在 serve 尽头无条件落账的口径）。
        self.tap.finish();
    }
}

/// append 一行到 usage.jsonl：**打开-写-关**，一次一行（任务钉的口径）。
/// 原子性说明：单次 `write_all` 一整行 + `\n`，OS 对本地文件的短写基本
/// 原子；**进程被杀最多丢尾部几行**（排队未写的 + 极端下写到一半的半行，
/// 半行由读侧的坏行跳过兜底）——记账不是账本真相，丢尾可接受。
/// Go 在非 Windows 上为吞吐持有句柄（usage.go:340-372 `heldLog`，#3725），
/// Windows 本就每次开关；Rust 引擎的调用频率下不值得一枚常开句柄。
fn append_line(path: &Path, line: &str, failures: &AtomicU64) {
    let open = || std::fs::OpenOptions::new().create(true).append(true).open(path);
    let mut file = match open() {
        Ok(f) => f,
        Err(_) => {
            // 目录缺失补建一次再试（usage.go:374 `openLog` 的 MkdirAll 同款）
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match open() {
                Ok(f) => f,
                Err(_) => {
                    failures.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        }
    };
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    if file.write_all(buf.as_bytes()).is_err() {
        failures.fetch_add(1, Ordering::Relaxed);
    }
}

/// 读文件尾部的记录（最多 [`TAIL_SCAN_BYTES`]）：
/// - 从文件中间起读时，第一段是半行——丢到首个换行再开始解析；
/// - 解析失败的行安静跳过（尾部半行、异构 JSON、`{}` 空壳——与 Go
///   `Load` 的 bufio 逐行容错同口径，usage.go:416 一带）；
/// - 文件不存在/打不开 = 空（首次运行 / 纯内存）。
fn read_tail_records(path: &Path) -> Vec<UsageRecord> {
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TAIL_SCAN_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return Vec::new();
    }
    let mut lines = buf.split('\n');
    if start > 0 {
        lines.next(); // 从中间起读：首段是半行，丢
    }
    lines.filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// 启动轮转：usage.jsonl >[`ROTATE_BYTES`] 时换名成 .1，旧 .1 先删
/// （Windows rename 不覆盖目标）。删/换名失败（文件被占）就这次不轮转，
/// 下次启动再试——台账仍可 append，只是继续变大。
fn rotate_if_huge(path: &Path) {
    let Ok(md) = std::fs::metadata(path) else { return };
    if md.len() <= ROTATE_BYTES {
        return;
    }
    let rotated = path.with_file_name(USAGE_ROTATED);
    let _ = std::fs::remove_file(&rotated);
    let _ = std::fs::rename(path, &rotated);
}

/// 毫秒墙钟（affinity.rs `now_ms` 同款；0 只在 epoch 之前的时间才出现，
/// 视作「没表」）。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    static DIR_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// 沙箱（presets.rs/quotas.rs 同款）：TEST_SANDBOX_LOCK 串行 +
    /// XDG_CONFIG_HOME 重定向 config_dir，Drop 恢复。台账路径在
    /// UsageLedger::open() 时定死，之后的 env 恢复不影响已开的台账。
    struct Sandbox {
        _lock: std::sync::MutexGuard<'static, ()>,
        prior_xdg: Option<String>,
    }

    fn sandbox() -> Sandbox {
        let lock = crate::engine_gateway::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-usage-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        // config_dir() = XDG_CONFIG_HOME/magpie（不是 XDG 根本身）
        std::fs::create_dir_all(dir.join("magpie")).expect("temp config dir");
        let prior_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        Sandbox {
            _lock: lock,
            prior_xdg,
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            match &self.prior_xdg {
                Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    fn usage_path() -> PathBuf {
        crate::engine_gateway::config_dir()
            .expect("config dir")
            .join(USAGE_FILE)
    }

    fn rec(ts: u64) -> UsageRecord {
        UsageRecord {
            ts,
            agent: "responses".into(),
            provider: "deepseek".into(),
            model: "deepseek-chat".into(),
            millis: 1200 + ts,
            status: 200,
            rejected: false,
            session: Some("pck-abc".into()),
            kind: "turn".into(),
            in_tokens: Some(1000),
            out_tokens: Some(200),
            cache_read: Some(50),
            cache_write: None,
        }
    }

    #[test]
    fn record_roundtrip_and_since_filters() {
        let _sb = sandbox();
        let ledger = UsageLedger::open();
        ledger.record(rec(1000));
        ledger.record(rec(2000));
        ledger.record(rec(3000));
        ledger.flush();
        // 落盘三行
        let body = std::fs::read_to_string(usage_path()).expect("usage.jsonl");
        assert_eq!(body.lines().count(), 3);
        // since 过滤：含边界（>=）
        let got = ledger.records_since(2000);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].model, "deepseek-chat");
        assert_eq!(got[0].session.as_deref(), Some("pck-abc"));
        assert_eq!(got[0].in_tokens, Some(1000));
        assert!(got.iter().all(|r| r.ts >= 2000));
        // since=0：ring 最早一条（1000）> 0 → 内存不覆盖边界 → 读文件合并
        // 去重，不得翻倍
        let all = ledger.records_since(0);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].ts, 1000);
        assert_eq!(all[2].ts, 3000);
        // ts=0 落账时补墙钟毫秒（usage.go:326 同款）
        ledger.record(UsageRecord {
            status: 500,
            ..Default::default()
        });
        let ring = ledger.inner.ring.lock().unwrap_or_else(|p| p.into_inner());
        assert!(
            ring.back().is_some_and(|r| r.ts > 1_700_000_000_000),
            "ts=0 应补成 21 世纪的墙钟毫秒，got {:?}",
            ring.back().map(|r| r.ts)
        );
    }

    #[test]
    fn rejected_row_round_trips() {
        let _sb = sandbox();
        let ledger = UsageLedger::open();
        // turnedAway 的那路（gateway.go:2674）：没出站、没有 usage 四桶
        ledger.record(UsageRecord {
            ts: 500,
            agent: "anthropic".into(),
            provider: String::new(),
            model: "claude-sonnet-4".into(),
            millis: 3,
            status: 429,
            rejected: true,
            session: None,
            kind: "rejected".into(),
            ..Default::default()
        });
        ledger.flush();
        assert_eq!(std::fs::read_to_string(usage_path()).unwrap().lines().count(), 1);
        let got = ledger.records_since(0);
        assert_eq!(got.len(), 1);
        assert!(got[0].rejected);
        assert_eq!(got[0].status, 429);
        assert_eq!(got[0].kind, "rejected");
        assert_eq!(got[0].provider, "");
        assert!(got[0].session.is_none(), "拒答行 session 可空");
        assert!(got[0].in_tokens.is_none(), "拒答行没有 usage 四桶");
        // 旧格式行（缺 ts/agent/model/status 的空壳）当坏行跳过
        std::fs::write(usage_path(), "{}\n{\"ts\":9,\"agent\":\"chat\"}\n").expect("seed junk");
        assert_eq!(
            ledger.records_since(0).len(),
            1,
            "{{}} 与缺必填字段的行都不是台账行"
        );
    }

    #[test]
    fn corrupt_tail_lines_are_skipped() {
        let _sb = sandbox();
        let path = usage_path();
        // 完整行 + 尾部半行（进程被杀的截断）+ 不是 JSON 的行
        std::fs::write(
            &path,
            concat!(
                "{\"ts\":10,\"agent\":\"chat\",\"provider\":\"p\",\"model\":\"m\",\"millis\":1,\"status\":200,\"rejected\":false,\"kind\":\"turn\"}\n",
                "{\"ts\":11,\"agent\":\"chat\"\n",
                "{\"ts\":12 not json at all\n",
            ),
        )
        .expect("seed");
        // 新开台账（ring 空）→ records_since 只能靠文件
        let ledger = UsageLedger::open();
        let got = ledger.records_since(0);
        assert_eq!(got.len(), 1, "只认第一条完整行");
        assert_eq!(got[0].ts, 10);
    }

    #[test]
    fn huge_log_rotates_on_open() {
        let _sb = sandbox();
        let path = usage_path();
        // 旧 .1 先在（上一代轮转残留）
        std::fs::write(path.with_file_name(USAGE_ROTATED), "stale").expect("old .1");
        // 造 >8MB 的 usage.jsonl
        let line = "{\"ts\":1,\"agent\":\"chat\",\"model\":\"m\",\"status\":200}\n";
        let big = line.repeat((ROTATE_BYTES as usize) / line.len() + 2);
        assert!(big.len() as u64 > ROTATE_BYTES);
        std::fs::write(&path, &big).expect("big usage.jsonl");
        let ledger = UsageLedger::open();
        assert!(!path.exists(), "超限的 usage.jsonl 让位");
        let rotated = std::fs::read_to_string(path.with_file_name(USAGE_ROTATED)).unwrap();
        assert_eq!(rotated.len(), big.len(), "整档换名成 .1");
        assert!(!rotated.contains("stale"), "旧 .1 被删");
        // 轮转后新账从空文件开始，.1 不回看
        ledger.record(rec(42));
        ledger.flush();
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        assert_eq!(ledger.records_since(0).len(), 1);
    }

    #[test]
    fn ring_caps_at_2000_and_file_fills_the_rest() {
        let _sb = sandbox();
        let ledger = UsageLedger::open();
        for i in 0..2050u64 {
            let mut r = rec(i + 1);
            r.in_tokens = Some(i);
            ledger.record(r);
        }
        ledger.flush();
        {
            let ring = ledger.inner.ring.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(ring.len(), RING_CAP);
            assert_eq!(ring.front().unwrap().ts, 51, "最老的 50 条被逐出");
        }
        // since=0：ring 最早一条（51）> 0 → 读文件补齐被逐出的 50 条，去重不翻倍
        let all = ledger.records_since(0);
        assert_eq!(all.len(), 2050);
        assert_eq!(all[0].ts, 1);
        assert_eq!(all[0].in_tokens, Some(0));
        assert_eq!(all[2049].ts, 2050);
        // since=1900：ring 最早一条（51）<= 1900 → 内存覆盖边界，纯内存返回
        assert_eq!(ledger.records_since(1900).len(), 151);
    }

    #[test]
    fn fresh_ledger_reads_history_from_disk() {
        let _sb = sandbox();
        {
            let ledger = UsageLedger::open();
            ledger.record(rec(10));
            ledger.record(rec(20));
            ledger.flush();
        }
        // 模拟重启：新台账 ring 是空的 → records_since 只能靠文件
        let ledger = UsageLedger::open();
        let got = ledger.records_since(0);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].ts, 10);
        assert_eq!(got[1].ts, 20);
        assert_eq!(got[1].in_tokens, Some(1000));
    }

    #[test]
    fn write_failures_are_counted_quietly() {
        let _sb = sandbox();
        let path = usage_path();
        // 占住 usage.jsonl 这个名字（目录）：append 打不开、读也读不了
        std::fs::create_dir_all(&path).expect("occupy path");
        let ledger = UsageLedger::open();
        ledger.record(rec(1));
        ledger.record(rec(2));
        ledger.flush();
        assert!(ledger.write_failures() >= 2, "每条失败各计一次，不 panic");
        // 查询侧照常：内存里有，文件侧读目录失败按空处理
        assert_eq!(ledger.records_since(0).len(), 2);
    }

    // ------------------------------------------------------------------
    // 捕获层（tap / TapReader / 三方言桶提取）
    // ------------------------------------------------------------------

    /// 纯内存台账（不落盘、不碰 env）：tap 测试用。
    fn memory_ledger() -> UsageLedger {
        UsageLedger {
            inner: Arc::new(Inner {
                ring: Mutex::new(VecDeque::new()),
                tx: None,
                path: None,
                write_failures: Arc::new(AtomicU64::new(0)),
            }),
        }
    }

    fn tap_record_of(ledger: &UsageLedger) -> UsageRecord {
        let ring = ledger.inner.ring.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(ring.len(), 1, "tap 恰好落一行");
        ring[0].clone()
    }

    fn turn(agent: &'static str) -> TurnLog {
        TurnLog::new(agent, "pv/m", Some("pck-1".into()), Instant::now())
    }

    /// chat 方言整块：OpenAI 口径（prompt 含缓存 → in = prompt − read − write）。
    #[test]
    fn chat_whole_extraction_openai_names() {
        let ledger = memory_ledger();
        let body = br#"{"id":"c","choices":[],"usage":{"prompt_tokens":1000,"completion_tokens":40,"total_tokens":1040,"prompt_tokens_details":{"cached_tokens":300,"cache_write_tokens":100}}}"#;
        turn("chat").whole(&ledger, Dialect::Chat, "pv", 200, body);
        let r = tap_record_of(&ledger);
        assert_eq!(r.agent, "chat");
        assert_eq!(r.provider, "pv");
        assert_eq!(r.model, "pv/m");
        assert_eq!(r.session.as_deref(), Some("pck-1"));
        assert_eq!(r.kind, "turn");
        assert_eq!(r.status, 200);
        assert_eq!(r.in_tokens, Some(600), "1000 − 300 − 100");
        assert_eq!(r.out_tokens, Some(40));
        assert_eq!(r.cache_read, Some(300));
        assert_eq!(r.cache_write, Some(100));
    }

    /// chat 方言的别家名字（DeepSeek 的 prompt_cache_hit_tokens；Anthropic 的
    /// cache_read/creation——后者的 prompt 只数非缓存时净额原样）。
    #[test]
    fn chat_whole_extraction_alien_names() {
        let ledger = memory_ledger();
        turn("chat").whole(
            &ledger,
            Dialect::Chat,
            "pv",
            200,
            br#"{"usage":{"prompt_tokens":1000,"completion_tokens":10,"prompt_cache_hit_tokens":800}}"#,
        );
        let r = tap_record_of(&ledger);
        assert_eq!(r.cache_read, Some(800), "DeepSeek 的 hits 名");
        assert_eq!(r.in_tokens, Some(200));
        assert!(r.cache_write.is_none(), "没说缓存写就是 None");

        let ledger = memory_ledger();
        // Anthropic 名字：prompt(400) < read+write(500+100) → 净额原样。
        turn("chat").whole(
            &ledger,
            Dialect::Chat,
            "pv",
            200,
            br#"{"usage":{"prompt_tokens":400,"completion_tokens":50,"cache_read_input_tokens":500,"cache_creation_input_tokens":100}}"#,
        );
        let r = tap_record_of(&ledger);
        assert_eq!(r.in_tokens, Some(400), "prompt 不含缓存时原样当净额");
        assert_eq!(r.cache_read, Some(500));
        assert_eq!(r.cache_write, Some(100));
    }

    /// responses 方言：input_tokens 是整个 prompt → in 减去缓存两桶；
    /// 事件流的 completed 在 response.usage（顶层 usage 是整块形态）。
    #[test]
    fn responses_extraction_and_event_nesting() {
        let ledger = memory_ledger();
        turn("responses").whole(
            &ledger,
            Dialect::Responses,
            "pv",
            200,
            br#"{"id":"resp_1","object":"response","status":"completed","usage":{"input_tokens":1000,"output_tokens":60,"total_tokens":1060,"input_tokens_details":{"cached_tokens":400,"cache_write_tokens":100},"output_tokens_details":{"reasoning_tokens":20}}}"#,
        );
        let r = tap_record_of(&ledger);
        assert_eq!(r.agent, "responses");
        assert_eq!(r.in_tokens, Some(500), "responses 的 input_tokens 含缓存");
        assert_eq!(r.out_tokens, Some(60));
        assert_eq!(r.cache_read, Some(400));
        assert_eq!(r.cache_write, Some(100));

        // 事件形态：response.completed 的 usage 挂在 response 里。
        let ledger = memory_ledger();
        let mut tap = turn("responses").tap(ledger.clone(), Dialect::Responses, "pv", 200);
        tap.note_line(
            br#"data: {"type":"response.completed","response":{"id":"r","status":"completed","usage":{"input_tokens":100,"output_tokens":5,"input_tokens_details":{"cached_tokens":40}}}}"#,
        );
        tap.finish();
        let r = tap_record_of(&ledger);
        assert_eq!(r.in_tokens, Some(60));
        assert_eq!(r.cache_read, Some(40));
        assert!(r.cache_write.is_none());
    }

    /// anthropic 方言：message_start 的 message.usage（input+缓存）+
    /// message_delta 的顶层 usage（output）分头到、后到非零桶覆盖。
    #[test]
    fn anthropic_two_events_merge_buckets() {
        let ledger = memory_ledger();
        let mut tap = turn("anthropic").tap(ledger.clone(), Dialect::Anthropic, "pv", 200);
        tap.note_line(
            br#"data: {"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":700,"output_tokens":1,"cache_read_input_tokens":500,"cache_creation_input_tokens":120,"cache_creation":{"ephemeral_5m_input_tokens":80,"ephemeral_1h_input_tokens":60}}}}"#,
        );
        tap.note_line(
            br#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#,
        );
        tap.finish();
        let r = tap_record_of(&ledger);
        assert_eq!(r.agent, "anthropic");
        assert_eq!(r.in_tokens, Some(700), "anthropic 的 input_tokens 本就只数非缓存");
        assert_eq!(r.out_tokens, Some(42), "delta 的 output 覆盖 start 的占位 1");
        assert_eq!(r.cache_read, Some(500));
        assert_eq!(r.cache_write, Some(140), "cache_creation 两档之和（120 vs 80+60 取大）");
    }

    /// 不编数：全零 usage（翻译层补的占位、空回合）与没提 usage 的体，
    /// 四桶全是 None，但行照落（骨架真数据）。
    #[test]
    fn zeros_and_absent_usage_are_not_fabricated() {
        let ledger = memory_ledger();
        turn("responses").whole(
            &ledger,
            Dialect::Responses,
            "pv",
            200,
            br#"{"status":"completed","usage":{"input_tokens":0,"output_tokens":0,"input_tokens_details":{"cached_tokens":0}}}"#,
        );
        let r = tap_record_of(&ledger);
        assert_eq!(r.in_tokens, None);
        assert_eq!(r.out_tokens, None);
        assert_eq!(r.cache_read, None);

        let ledger = memory_ledger();
        turn("anthropic").whole(&ledger, Dialect::Anthropic, "pv", 200, br#"{"content":[]}"#);
        let r = tap_record_of(&ledger);
        assert!(r.in_tokens.is_none() && r.out_tokens.is_none());
        assert_eq!(r.status, 200, "行照落（millis/status 是真数据）");
    }

    /// 流式收口：TapReader 把 SSE 尾帧 usage 提走，EOF 落一行带真实
    /// millis；字节一个不少地转发。
    #[test]
    fn tap_reader_records_stream_turn() {
        let ledger = memory_ledger();
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"id\":\"c\",\"choices\":[],\"usage\":{\"prompt_tokens\":900,\"completion_tokens\":30,\"total_tokens\":930,\"prompt_tokens_details\":{\"cached_tokens\":250}}}\n\n",
            "data: [DONE]\n\n",
        );
        let log = turn("chat");
        // 内存流太快会 0ms：起点后垫 2ms 再流，验证 millis 真从回合起点起算。
        std::thread::sleep(std::time::Duration::from_millis(2));
        let tap = log.tap(ledger.clone(), Dialect::Chat, "pv", 200);
        let mut reader = TapReader::new(tap, std::io::Cursor::new(sse.as_bytes().to_vec()));
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out, sse.as_bytes(), "字节照转，一个不少");
        let r = tap_record_of(&ledger);
        assert_eq!(r.in_tokens, Some(650));
        assert_eq!(r.out_tokens, Some(30));
        assert_eq!(r.cache_read, Some(250));
        assert!(r.millis > 0, "真实 millis（回合起点→流尾），got {}", r.millis);
    }

    /// 任意切割不变量：一字节一段与整段喂入，落账行一致（Chattidy 同款
    /// 流不变量）。
    #[test]
    fn tap_reader_chunk_cut_invariance() {
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n",
        );
        let whole = {
            let ledger = memory_ledger();
            let tap = turn("chat").tap(ledger.clone(), Dialect::Chat, "pv", 200);
            let mut r = TapReader::new(tap, std::io::Cursor::new(sse.as_bytes().to_vec()));
            let mut sink = Vec::new();
            r.read_to_end(&mut sink).unwrap();
            let mut rec = tap_record_of(&ledger);
            rec.millis = 0; // millis 与切片节奏无关，归零再比
            rec.ts = 0;
            rec
        };
        for cut in [1usize, 3, 7, 40, 61, 130] {
            let ledger = memory_ledger();
            let pieces = [&sse[..cut], &sse[cut..]];
            // Dribble 逐字节版
            let tap = turn("chat").tap(ledger.clone(), Dialect::Chat, "pv", 200);
            let mut r = TapReader::new(
                tap,
                Dribble(pieces[0].as_bytes().to_vec(), pieces[1].as_bytes().to_vec()),
            );
            let mut sink = Vec::new();
            r.read_to_end(&mut sink).unwrap();
            let mut rec = tap_record_of(&ledger);
            rec.millis = 0;
            rec.ts = 0;
            assert_eq!(rec, whole, "cut at {cut} differs");
            assert_eq!(sink, sse.as_bytes(), "cut at {cut} lost bytes");
        }
    }

    /// 断流也落账：读错误上抛前收口，已见桶照记；drop（客户端断开、
    /// 上层提前收口）同样落——两路各恰一行。
    #[test]
    fn tap_reader_records_on_error_and_drop() {
        let head = "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":3}}\n\n";
        // 头是正常字节、读完即抛错（厂商半途断流的最小模拟）。
        struct HeadThenErr(Vec<u8>, bool);
        impl Read for HeadThenErr {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if !self.1 && !self.0.is_empty() {
                    let n = self.0.len().min(buf.len());
                    buf[..n].copy_from_slice(&self.0[..n]);
                    self.0.drain(..n);
                    if self.0.is_empty() {
                        self.1 = true;
                    }
                    return Ok(n);
                }
                Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "cut"))
            }
        }
        let ledger = memory_ledger();
        let tap = turn("chat").tap(ledger.clone(), Dialect::Chat, "pv", 200);
        let mut reader = TapReader::new(tap, HeadThenErr(head.as_bytes().to_vec(), false));
        let mut out = Vec::new();
        let err = reader.read_to_end(&mut out).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted, "错误原样上抛");
        assert_eq!(out, head.as_bytes());
        let r = tap_record_of(&ledger);
        assert_eq!(r.in_tokens, Some(11), "断流前已见的桶照记");
        assert_eq!(r.out_tokens, Some(3));

        // drop 路径（没读到 EOF 就被丢）：usage 帧到了但流没收尾——照落。
        // 一次读走全部字节（EOF 要下一次 read 才报），随后直接 drop。
        let ledger = memory_ledger();
        let tap = turn("chat").tap(ledger.clone(), Dialect::Chat, "pv", 200);
        let mut reader = TapReader::new(tap, std::io::Cursor::new(head.as_bytes().to_vec()));
        let mut all = vec![0u8; head.len()];
        let _ = reader.read(&mut all); // 拿到全部字节，还没碰 EOF
        drop(reader);
        let r = tap_record_of(&ledger);
        assert_eq!(r.in_tokens, Some(11), "drop 收口也落账");
    }

    /// 超长行不缓存（runaway line）：判死后行内的 "usage" 不解析，
    /// 流照转、账照落（None 桶）。
    #[test]
    fn runaway_line_is_skipped_but_recorded() {
        let giant = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"function\":{{\"arguments\":\"{}\"}}}}]}}}}]}}\n\n",
            "A".repeat(TAP_LINE_MAX + 1024)
        );
        let sse = format!("{giant}data: [DONE]\n\n");
        let ledger = memory_ledger();
        let tap = turn("chat").tap(ledger.clone(), Dialect::Chat, "pv", 200);
        let mut reader = TapReader::new(tap, std::io::Cursor::new(sse.into_bytes()));
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        let r = tap_record_of(&ledger);
        assert!(r.in_tokens.is_none(), "超限行不解析");
    }

    /// 克隆是句柄不是副本：两个克隆写同一个 ring（流式 reader 揣克隆
    /// 落账的机制前提）。
    #[test]
    fn ledger_clone_shares_the_ring() {
        let ledger = memory_ledger();
        let handle = ledger.clone();
        handle.record(rec(7));
        assert_eq!(ledger.records_since(0).len(), 1);
        assert_eq!(ledger.records_since(0)[0].ts, 7);
    }

    /// 一字节一段吐出的 reader（tap 切割不变量用）。
    struct Dribble(Vec<u8>, Vec<u8>);

    impl Read for Dribble {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let take = |from: &mut Vec<u8>, buf: &mut [u8]| -> Option<usize> {
                if from.is_empty() || buf.is_empty() {
                    return None;
                }
                let n = from.len().min(buf.len());
                buf[..n].copy_from_slice(&from[..n]);
                from.drain(..n);
                Some(n)
            };
            match take(&mut self.0, buf).or_else(|| take(&mut self.1, buf)) {
                Some(n) => Ok(n),
                None => Ok(0),
            }
        }
    }
}
