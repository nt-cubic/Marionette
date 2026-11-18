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
//!   侧字段全没有——本台账只记任务口径的 13 个字段。
//! - **无价格折算与字节数**：CostAt、UpBytes/DownBytes、CacheWrite1h/
//!   CacheUnknown、Credits 一概不记；四桶 token 用 `Option`——「厂商没报
//!   就是 None，不编 0」。
//! - **轮转只有单代**：Go 靠 backup_log.go / cold_split.go 冷热分档；
//!   Rust 只在启动时 >8MB 把 usage.jsonl 换名成 .1（旧 .1 删），没有
//!   冷热分档，也没有多代。
//! - **会话字段口径不同**：Go 的 Session 是 agent 自报的会话头
//!   （lan.go:867-869 `sessionOf(r.Header)` 兜底）；Rust 网关按任务口径
//!   存 affinity 种子同源的 `prompt_cache_key` / `metadata.user_id`
//!   （affinity.rs `stick_key` 的提取序，affinity.rs:91-99）。
//! - **落盘是异步的**：`record()` 只入内存 ring + 排队，写线程逐行 append
//!   ——进程被杀最多丢尾部几行（与 Go「Append 在请求路径上同步写」不同，
//!   换请求路径零等待；丢尾可接受，见 `append_line` 注释）。

use std::collections::{HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

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
pub(crate) struct UsageLedger {
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
            ring: Mutex::new(VecDeque::new()),
            tx,
            path,
            write_failures,
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
            let mut ring = self.ring.lock().unwrap_or_else(|p| p.into_inner());
            ring.push_back(r);
            while ring.len() > RING_CAP {
                ring.pop_front();
            }
        }
        if let (Some(tx), Some(line)) = (&self.tx, line) {
            let tx = tx.lock().unwrap_or_else(|p| p.into_inner());
            // 接收端死了（线程起不来/进程正在退）算一次写失败，安静计数
            if tx.send(Msg::Line(line)).is_err() {
                self.write_failures.fetch_add(1, Ordering::Relaxed);
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
            let ring = self.ring.lock().unwrap_or_else(|p| p.into_inner());
            let memory: Vec<UsageRecord> =
                ring.iter().filter(|r| r.ts >= since_ms).cloned().collect();
            let covers = ring.front().is_some_and(|r| r.ts <= since_ms);
            (memory, covers)
        };
        if covers {
            return memory;
        }
        let Some(path) = &self.path else {
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
        let Some(tx) = &self.tx else { return };
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let tx = tx.lock().unwrap_or_else(|p| p.into_inner());
        if tx.send(Msg::Sync(reply_tx)).is_ok() {
            // 写线程异常卡死时别跟着挂死：5s 放弃（屏障尽力而为）
            let _ = reply_rx.recv_timeout(std::time::Duration::from_secs(5));
        }
    }

    /// 落盘失败计数（安静，不打印）。
    pub(crate) fn write_failures(&self) -> u64 {
        self.write_failures.load(Ordering::Relaxed)
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
        let ring = ledger.ring.lock().unwrap_or_else(|p| p.into_inner());
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
            let ring = ledger.ring.lock().unwrap_or_else(|p| p.into_inner());
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
}
