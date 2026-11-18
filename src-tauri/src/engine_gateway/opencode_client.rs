//! OpenCode 网关客户端头家族——Go `internal/provider/opencode_client.go` 的
//! 1:1 对照移植（T13 真机对账：opencode-go 源 400 MissingSessionID——Go 对
//! opencode.ai 系 URL 会合成 OpenCode 2.2.5 自己的客户端头，Rust 没有）。
//!
//! ## 挂点语义（Go gateway.go:6582-6591，接线由主会话做）
//!
//! ```text
//! if provider.OpenCodeURL(req.URL.String()) {
//!     ... zenFree 分支（见差距 1）...
//!     provider.OpenCodeClient(req.Header, openCodeSeed(in, body), openCodeDir(factsOf(to, body).dir))
//!     if v := in.Get("x-opencode-project"); v != "" { req.Header.Set("x-opencode-project", v) }
//! }
//! ```
//!
//! 本模块提供 [`opencode_url`]（判 opencode.ai 系主机）、[`opencode_client`]
//! （组装 2.2.5 的全部头）、[`opencode_session_or_new`]（对齐
//! `OpenCodeSession` :192-199，对 provider/account.go:230 的 Sign 路径与
//! planquota.go:660 的调用形态——无状态调用自得一个会话，400
//! MissingSessionID 的直接修复点）。`openCodeSeed`（opencode_session.go:19）与
//! `openCodeDir`（zenfree.go:129）是 gateway 侧取值，不在这里；挂点第二半
//! 的 `x-opencode-project` 转发也在接线方（Go :6589-6591）。
//!
//! ## 移植面（Go 文件:行号）
//!
//! - [`opencode_url`] ← OpenCodeURL :171-174 + openCodeHost :177-180
//! - [`opencode_real_session`] / [`opencode_new_id`] / [`opencode_session`]
//!   ← openCodeRealSession :596-602 / openCodeNewID :569-590 /
//!   openCodeSession :609-630（形状正则 :547、base62 :549、LRU 1000 :563）
//! - [`opencode_trace`] ← openCodeTrace :521-545（一会话一 trace、一请求一
//!   span；b3 = trace-span-1-parent、traceparent = 00-trace-span-01）
//! - [`opencode_project`] ← OpenCodeProject :377-394 + openCodeLocalDir
//!   :398-407 + openCodeRepository :422-479 + openCodeRemotePath :484-509
//! - [`opencode_user_agent`] ← OpenCodeUserAgent :41-43 + openCodeLatest
//!   :65-84 + openCodeAskVersion :91-116 + openCodeAtLeast :119-129
//! - [`opencode_client`] ← OpenCodeClient :150-163
//! - [`opencode_session_or_new`] ← OpenCodeSession :192-199
//!
//! 测试向量对齐 Go `opencode_client_test.go`（真/假 id、计数、LRU、头）。
//!
//! ## 与 Go 的差距清单（没搬/换了实现的逐条，改这里前先过一遍）
//!
//! 1. **zenFree 全家不移植**：`OpenCodeFree` / `OpenCodeTools` /
//!    `OpenCodeToolSchemas` / `OpenCodeEnvironment` / `OpenCodeHasEnvironment`
//!    / `OpenCodeFreeURL` / `OpenCodeStub` / `zenFreeProbe`（zenfree.go 全家）
//!    ——本机 opencode-go 源走 `/inference/go`，不在 free 面；剥头+核心工具+
//!    环境块+URL 重写都不做。`openCodeFreeIDs` / `catalog.PriceOf` 同弃。
//! 2. **gateway 侧取值不在**：`openCodeSeed`（opencode_session.go:19，头 →
//!    sessionOf → prompt_cache_key → conversationID 四级）与 `openCodeDir`
//!    （zenfree.go:129，绝对路径/`~/` 判定）由接线方算好传入；本模块的
//!    `dir` 语义与 Go OpenCodeProject 相同（"" = home）。
//! 3. **随机源**：Go `crypto/rand`（CSPRNG）→ 本模块 `RandomState` 哈希熵
//!    （ws.rs 同款，std-only 禁新依赖）。id/trace 只用于路由缓存键与「形状
//!    像 OpenCode」，不承载秘密；撞键概率相当，但不是密码学强度。
//!    Go 的 `randomUUID` 兜底路径不适用（熵源不会失败）。
//! 4. **URL 解析手写**（无 url crate，依赖红线）：`scheme://` + authority 的
//!    最小子集（userinfo/端口/IPv6 括号/尾点），覆盖远端 URL 的现实形状
//!    （https/ssh/file/scp）；Go url.Parse 的其余错误分类未逐一对齐。
//! 5. **lazyre → 手写解析**（无正则 crate）：git config 的 `[remote "…"]` /
//!    `url =` / `gitdir:` 行、SCP 形 `^([^@/:]+@)?([^/:]+):(.+)$`（含回溯
//!    的两个起点）、版本形状 `^\d{1,4}\.\d{1,4}\.\d{1,6}$`，测试向量与
//!    Go 对齐。
//! 6. **EvalSymlinks → fs::canonicalize + 剥 `\\?\` 前缀**：Windows 的
//!    canonicalize 返回 verbatim 路径，剥掉才与 Go 的形态一致（`\\?\UNC\`
//!    折回 `\\server\share`）。相对路径输入会被绝对化（Go EvalSymlinks 保留
//!    相对——挂点传入恒为绝对/`~/` 开头，openCodeDir 保证）；路径大小写跟随
//!    磁盘真实形态（Go 多数时候保留输入形态）——同一目录两种大小写会得到
//!    不同 project id，Go 也一样（host/path 之外它不归一）。
//! 7. **版本更新器**：Go 后台 goroutine → std::thread（3s 超时、redirects(0)
//!    拒重定向、6h 一问、测试二进制 + 生产 URL 恒不真发——cfg!(test) 对齐
//!    Go `testing.Testing()`）。spawn 失败把 asking 复位（Go 的 goroutine
//!    不会失败，这是 Rust 侧的防御）。
//! 8. **64KB 截断的极端形状**：Go `json.Decoder.Decode` 读截断流里的首个
//!    JSON 值（后面还有数据也算成功）；serde `from_slice` 对尾随数据报错。
//!    更新器响应 ~30B，永不到 64KB——只记差异不改。
//! 9. **盘根形态**：filepath.Dir("C:/x") = "C:\" 而 Path::parent = "C:"——
//!    仓库正好落在盘符根时的 root 字符串形态可能差一个分隔符（project id
//!    随之），现实里 .git 不在盘根。
//!
//! ## 全局态与锁纪律
//!
//! 三张表（id 时钟+会话 LRU、trace LRU、版本更新器状态）各是
//! `OnceLock<Mutex<…>>`（mod.rs 风格），**锁内禁 IO**：HTTP 只在
//! [`open_code_ask_version`]（无锁）里做，后台线程存结果时短持版本锁；
//! id/trace 的随机段与 `now` 都在锁外取。测试共享一把模块内 `STATE_LOCK`
//! 串行化（这些测试都动共享表）。
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

// ---------------------------------------------------------------------------
// 常量（Go :35 / :49 / :52 / :133 / :549 / :563）
// ---------------------------------------------------------------------------

/// OpenCodeVersion：对 OpenCode 网关自报的最老版本（2.2.5，2026-11-01 npm 的
/// @opencode/cli latest）。更新器说不出更新时兜底（Go OpenCodeVersion :35）。
pub(crate) const OPENCODE_VERSION: &str = "2.2.5";

/// openCodeUpdater：OpenCode 自己的更新器，npm 渠道答 {"version":"x.y.z"}
/// （Go :49）。
const UPDATER: &str = "https://opencode.ai/update/api/latest/cli/npm";

/// openCodeVersionEvery：多久再问一次更新器（Go :52）。
const VERSION_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// base62：id 尾段随机字符表（Go :549）。
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// openCodeKeep：多少场对话的会话 id / trace 保留（Go :563，两张表共用）。
const OPENCODE_KEEP: usize = 1000;

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Go RE2 的 `\s` = [\t\n\f\r ]（不含 \v）——config 行解析的空白口径。
fn is_go_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

fn is_go_space_byte(c: u8) -> bool {
    matches!(c, b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

/// RandomState 哈希熵（ws.rs 同款 std-only 形状；差距 3）。
fn random_bytes(n: usize) -> Vec<u8> {
    use std::hash::{BuildHasher, Hasher};
    let mut out = Vec::with_capacity(n + 8);
    let mut counter: u64 = 0;
    let started = std::time::Instant::now();
    while out.len() < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(counter);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_u64(started.elapsed().as_nanos() as u64);
        h.write_usize(&counter as *const u64 as usize);
        counter = counter.wrapping_add(1);
        out.extend_from_slice(&h.finish().to_le_bytes());
    }
    out.truncate(n);
    out
}

/// hexOf（Go :523-528）：n 个随机字节的十六进制（2n 位小写）。
fn hex_of(n: usize) -> String {
    random_bytes(n).iter().map(|b| format!("{b:02x}")).collect()
}

/// sha1 摘要的十六进制——复用 ws.rs 的手写 SHA-1（FIPS 180-1，RFC 3174
/// 公开向量守着）。project id 的哈希是 Go sha1.Sum 的同义（非安全用途）。
fn sha1_hex(data: &[u8]) -> String {
    super::ws::sha1(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

// ---------------------------------------------------------------------------
// 头表工具（engine_gateway 的出站头形态 = Vec<(String, String)>；Go http.Header
// 的 Set/Del/Get 语义：大小写不敏感，Set 全删后追加）
// ---------------------------------------------------------------------------

fn header_set(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    headers.push((name.to_string(), value.to_string()));
}

fn header_del(headers: &mut Vec<(String, String)>, name: &str) {
    headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
}

fn header_get<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

// ---------------------------------------------------------------------------
// 1. URL 判定（Go OpenCodeURL :171-174 + openCodeHost :177-180）
// ---------------------------------------------------------------------------

/// u 是否指向 OpenCode 的网关（Zen、Go 或 inference API）：opencode.ai 或
/// 它名下的主机，端口不管（Go :167-170 注释同义——按 URL 判，不按 provider
/// 的第一个 base 判）。解析手写（差距 4）：Go url.Parse 的最小语义——
/// `scheme://`（或协议相对 `//`）后取 authority，剥 userinfo 与端口。
pub(crate) fn opencode_url(u: &str) -> bool {
    let u = u.trim();
    // 协议相对（url.Parse 对 "//host/x" 也认 host）
    let host = if let Some(rest) = u.strip_prefix("//") {
        Some(authority_hostname(rest))
    } else {
        match u.split_once("://") {
            None => None, // url.Parse 无 scheme → Hostname 为空
            Some((scheme, rest)) => {
                if !scheme_ok(scheme) {
                    None
                } else {
                    Some(authority_hostname(rest))
                }
            }
        }
    };
    host.map(|h| open_code_host(&h)).unwrap_or(false)
}

/// scheme 形状：`[a-zA-Z][a-zA-Z0-9+.-]*`（Go url.Parse getScheme 的接受集；
/// "://x" 空 scheme / "1http://x" 数字开头都是 Parse 的失败面 → false）。
fn scheme_ok(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
}

/// authority（到第一个 / ? # 为止）里的主机名：剥 userinfo（最后一个 @ 之后）、
/// 剥端口（`[v6]` 括号内不算）、小写。hostname 可为空串（无主机 → false）。
fn authority_hostname(rest: &str) -> String {
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let hostport = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    let host = if let Some(stripped) = hostport.strip_prefix('[') {
        // IPv6 字面量：] 之前的才是主机（无 ] 视作解析失败，交空）
        match stripped.find(']') {
            Some(close) => &stripped[..close],
            None => "",
        }
    } else {
        hostport.split(':').next().unwrap_or(hostport)
    };
    host.to_ascii_lowercase()
}

/// openCodeHost（Go :177-180）：剥尾点（一次）、小写后等于 opencode.ai 或以
/// .opencode.ai 结尾。
fn open_code_host(h: &str) -> bool {
    let h = match h.strip_suffix('.') {
        Some(stripped) => stripped,
        None => h,
    }
    .to_ascii_lowercase();
    h == "opencode.ai" || h.ends_with(".opencode.ai")
}

// ---------------------------------------------------------------------------
// 2. 会话 id 形状机（Go :547 / :549 / :553-563 / :569-630）
// ---------------------------------------------------------------------------

/// openCodeIDRe `^[a-z]{3}_[0-9a-f]{12}[0-9A-Za-z]{14}$` 的手写版：30 字节、
/// 3 个小写字母、`_`、12 位小写十六进制、14 位字母数字。
fn id_shape_ok(id: &str) -> bool {
    let b = id.as_bytes();
    if b.len() != 30 {
        return false;
    }
    b[..3].iter().all(|c| c.is_ascii_lowercase())
        && b[3] == b'_'
        && b[4..16]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
        && b[16..30].iter().all(|c| c.is_ascii_alphanumeric())
}

/// openCodeRealSession（Go :596-602）：id 是 OpenCode 可能造出的会话 id——
/// 形状对，且低 12 位（id[13:16] 三个十六进制位）的反转计数很小
/// （0xfff-low < 16）。Zen 的 free 层对不是的这个形状 403，混进来的同形状
/// 哈希也在内（c9cu on Discord）。
pub(crate) fn opencode_real_session(id: &str) -> bool {
    if !id.starts_with("ses_") || !id_shape_ok(id) {
        return false;
    }
    // 形状已验：id[13..16] 必是 3 个 [0-9a-f]，解析不会失败（Go err 分支）
    let low = u32::from_str_radix(&id[13..16], 16).unwrap_or(0x1000);
    low <= 0xfff && 0xfff - low < 16
}

/// id 时钟与计数器（Go openCodeIDs :553-558 的 last/counter 半边）+ 会话
/// LRU（sessions/order 半边）。
struct IdState {
    last: i64,
    counter: u64,
    /// seed → session id
    sessions: HashMap<String, String>,
    /// seed 的近用序（队头最老），与 sessions 同步增删。
    order: VecDeque<String>,
}

fn ids() -> &'static Mutex<IdState> {
    static S: OnceLock<Mutex<IdState>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(IdState {
            last: 0,
            counter: 0,
            sessions: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// openCodeNewID（Go :569-590）：OpenCode 造 id 的方式（packages/schema/src/
/// identifier.ts）：`前缀_` + 12 位十六进制（毫秒 ×0x1000 + 该毫秒内的计数，
/// session 位反转让最新的排前）+ 14 位 base62 随机。`now_ms` 可注入
/// （Go 收 time.Time），时钟/计数在共享锁内，随机段在锁外。
pub(crate) fn opencode_new_id(prefix: &str, descending: bool, now_ms: i64) -> String {
    let current = {
        let mut st = lock(ids());
        if st.last != now_ms {
            st.last = now_ms;
            st.counter = 0;
        }
        st.counter = st.counter.wrapping_add(1);
        (now_ms as u64)
            .wrapping_mul(0x1000)
            .wrapping_add(st.counter)
    };
    let current = if descending { !current } else { current };
    let tail: String = random_bytes(14)
        .iter()
        .map(|b| BASE62[(*b as usize) % BASE62.len()] as char)
        .collect();
    format!("{}_{:012x}{}", prefix, current & 0xffff_ffff_ffff, tail)
}

/// openCodeSession（Go :609-630）：seed 命名的对话要发的会话 id——seed 本身
/// 是 OpenCode 可能造的就直通；空 seed 新造一个（无状态调用自得一个）；
/// 其余 seed 第一次见到时造一个并记进 LRU（最后 1000 场对话的留着的）。
pub(crate) fn opencode_session(seed: &str) -> String {
    if opencode_real_session(seed) {
        return seed.to_string();
    }
    if seed.is_empty() {
        return opencode_new_id("ses", true, now_millis());
    }
    let id = opencode_new_id("ses", true, now_millis()); // 先造：它拿锁（Go :616）
    let mut st = lock(ids());
    if let Some(existing) = st.sessions.get(seed).cloned() {
        // MoveToBack（Go :620）
        if let Some(pos) = st.order.iter().position(|s| s == seed) {
            st.order.remove(pos);
        }
        st.order.push_back(seed.to_string());
        return existing;
    }
    if st.order.len() >= OPENCODE_KEEP {
        if let Some(oldest) = st.order.pop_front() {
            st.sessions.remove(&oldest);
        }
    }
    st.sessions.insert(seed.to_string(), id.clone());
    st.order.push_back(seed.to_string());
    id
}

// ---------------------------------------------------------------------------
// 3. trace（Go :513-545）
// ---------------------------------------------------------------------------

/// 一会话一 trace 的表（Go openCodeTraces :513-517）。
struct TraceState {
    /// session → trace（32 位十六进制）
    of: HashMap<String, String>,
    /// session 的近用序（队头最老）。
    order: VecDeque<String>,
}

fn traces() -> &'static Mutex<TraceState> {
    static S: OnceLock<Mutex<TraceState>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(TraceState {
            of: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// openCodeTrace（Go :521-545）：session 的请求带的 b3 与 traceparent——
/// 一会话一 trace（LRU 1000），一请求一新 span。
/// b3 = trace-span-1-parent（第四段是再抽的一次 8 字节随机，Go :544 原样），
/// traceparent = 00-trace-span-01。
pub(crate) fn opencode_trace(session: &str) -> (String, String) {
    let mut trace = hex_of(16);
    {
        let mut st = lock(traces());
        if let Some(kept) = st.of.get(session).cloned() {
            trace = kept;
            if let Some(pos) = st.order.iter().position(|s| s == session) {
                st.order.remove(pos);
            }
            st.order.push_back(session.to_string());
        } else {
            if st.order.len() >= OPENCODE_KEEP {
                if let Some(oldest) = st.order.pop_front() {
                    st.of.remove(&oldest);
                }
            }
            st.of.insert(session.to_string(), trace.clone());
            st.order.push_back(session.to_string());
        }
    }
    let span = hex_of(8);
    let b3 = format!("{}-{}-1-{}", trace, span, hex_of(8));
    let traceparent = format!("00-{}-{}-01", trace, span);
    (b3, traceparent)
}

// ---------------------------------------------------------------------------
// 4. project id（Go :377-509）
// ---------------------------------------------------------------------------

/// OpenCodeProject（Go :377-394）：OpenCode 给 dir 的 project id（"" = home）。
/// 带 remote 的 git 仓库 = sha1("git-remote:host/path")；其余仓库 =
/// sha1("directory:仓库根")；非仓库 = sha1("directory:dir")。没有 remote 的
/// 仓库按它所在的文件夹命名（zen-free 插件同款），不跑 git。
pub(crate) fn opencode_project(dir: &str) -> String {
    let dir = if dir.is_empty() {
        user_home().unwrap_or_default()
    } else {
        dir.to_string()
    };
    let local = open_code_local_dir(&dir);
    let (root, remote, is_repo) = open_code_repository(&local);
    let key = if !remote.is_empty() {
        format!("git-remote:{remote}")
    } else if is_repo {
        format!("directory:{root}")
    } else {
        format!("directory:{local}")
    };
    sha1_hex(key.as_bytes())
}

/// openCodeLocalDir（Go :398-407）：dir 里的 `~` 换成 home，链接尽量解析。
/// （Go 的 home 读取失败时 dir 变成 "" + 剩余——这里同义镜像。）
fn open_code_local_dir(dir: &str) -> String {
    let mut dir = dir.to_string();
    if dir == "~" || dir.starts_with("~/") {
        dir = format!("{}{}", user_home().unwrap_or_default(), &dir[1..]);
    }
    match eval_symlinks(Path::new(&dir)) {
        Some(real) => real,
        None => dir,
    }
}

/// os.UserHomeDir 同义：Windows 用 %USERPROFILE%，其余 $HOME；空值视作没有
/// （调用方 unwrap_or_default 得 ""，与 Go 的 `home, _ :=` 出 "" 同义）。
fn user_home() -> Option<String> {
    let key = if cfg!(target_os = "windows") { "USERPROFILE" } else { "HOME" };
    std::env::var(key).ok().filter(|h| !h.is_empty())
}

/// filepath.EvalSymlinks 同义：存在的路径解析符号链接，不存在/读不了返回
/// None（调用方保留原样）。Windows 上剥 canonicalize 的 verbatim 前缀
/// （差距 6）。
fn eval_symlinks(p: &Path) -> Option<String> {
    let real = std::fs::canonicalize(p).ok()?;
    let s = path_string(&real);
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        Some(format!(r"\\{}", rest))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        Some(rest.to_string())
    } else {
        Some(s)
    }
}

/// filepath.IsAbs 的 Windows 语义（Go path/filepath path_windows.go）：有卷名
/// （盘符 X:）且卷名后紧跟分隔符 → 真；无卷名以 `\` 开头 → 真；`/x`、`C:x`
/// → 假。非 Windows 同 Unix（`/` 开头）。
fn go_is_abs(p: &str) -> bool {
    let bytes = p.as_bytes();
    if cfg!(target_os = "windows") {
        let vol = if bytes.len() >= 2
            && bytes[1] == b':'
            && bytes[0].is_ascii_alphabetic()
        {
            2
        } else {
            0
        };
        if vol > 0 {
            let rest = &p[vol..];
            if rest.is_empty() {
                return false;
            }
            if rest.starts_with('\\') || rest.starts_with('/') {
                return true;
            }
        }
        p.starts_with('\\')
    } else {
        p.starts_with('/')
    }
}

/// openCodeRepository（Go :422-479）：dir 所在的 git 仓库——从 dir 向上找
/// 最近的 .git（worktree 的 .git 文件命名它的 gitdir，那个 gitdir 的
/// commondir 文件指向公共 .git），返回（根、OpenCode 命名它的 remote =
/// host/path、是否仓库）。注意 Go :433-435：.git 文件里读不出 gitdir 行时
/// **整个上行判定为非仓库**，不再继续向上。
fn open_code_repository(dir: &str) -> (String, String, bool) {
    let mut d = PathBuf::from(dir);
    loop {
        let dot_git = d.join(".git");
        if dot_git.exists() {
            let mut common = dot_git.clone();
            if dot_git.is_file() {
                // worktree：.git 是文件，内容 `gitdir: <路径>`
                let b = match std::fs::read(&dot_git) {
                    Ok(b) => b,
                    Err(_) => return (path_string(&d), String::new(), true), // Go :429-431
                };
                let own = match gitdir_of(&String::from_utf8_lossy(&b)) {
                    Some(own) if !own.is_empty() => own,
                    _ => return (String::new(), String::new(), false), // Go :433-435
                };
                let own = if go_is_abs(&own) {
                    PathBuf::from(&own)
                } else {
                    d.join(&own)
                };
                common = own.clone();
                if let Ok(c) = std::fs::read(own.join("commondir")) {
                    let shared = String::from_utf8_lossy(&c).trim().to_string();
                    common = if go_is_abs(&shared) {
                        PathBuf::from(&shared)
                    } else {
                        own.join(&shared)
                    };
                }
            }
            let config = match std::fs::read(common.join("config")) {
                Ok(c) => c,
                Err(_) => return (path_string(&d), String::new(), true), // Go :449-452
            };
            let text = String::from_utf8_lossy(&config);
            let u = pick_remote_url(&text);
            return (path_string(&d), open_code_remote_path(&u), true);
        }
        let up = match d.parent() {
            Some(p) => p.to_path_buf(),
            None => return (String::new(), String::new(), false),
        };
        if up == d {
            return (String::new(), String::new(), false);
        }
        d = up;
    }
}

/// openCodeGitdirRe `(?m)^gitdir:\s*(.+)$` 的首匹配（Go :432）：以 `gitdir:`
/// 开头的行，值 = 行内剩余（Go 的 `\s*(.+)$` 贪婪 + TrimSpace 同义 → trim）。
fn gitdir_of(content: &str) -> Option<String> {
    for line in content.split('\n') {
        if let Some(rest) = line.strip_prefix("gitdir:") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// 远端选择（Go :453-471）：各 `[remote "…"]` 段的第一个 `url =` 行；
/// first = 第一段的 url，origin = origin 段的 url（见段即停）。u = first，
/// **除非** origin 非空，或者（存在 remote 段且 config 里有 `"origin"`——
/// origin 段没有 url 行的情形）→ u = origin（可能是空串，即「无 remote」，
/// 不会退回 first）。
fn pick_remote_url(config: &str) -> String {
    let mut first: Option<String> = None;
    let mut origin: Option<String> = None;
    let mut origin_found = false;
    let mut any_section = false;
    let mut pos = 0usize;
    while let Some((name, body_start, body_end)) = next_remote_section(config, pos) {
        any_section = true;
        let u = first_url_in(&config[body_start..body_end]).unwrap_or_default();
        if first.is_none() {
            first = Some(u.clone());
        }
        if name == "origin" {
            origin = Some(u);
            origin_found = true;
            break;
        }
        pos = body_end;
    }
    let contains_origin = config.contains("\"origin\"");
    if origin_found && origin.as_deref().is_some_and(|s| !s.is_empty()) {
        origin.unwrap()
    } else if any_section && contains_origin {
        origin.unwrap_or_default()
    } else {
        first.unwrap_or_default()
    }
}

/// openCodeRemoteRe `(?m)^\s*\[remote\s+"([^"]+)"\]([^\[]*)` 的逐个匹配：
/// `[remote` 前到上一个换行之间只能是空白（`^\s*` 的等价判定，`\s` 可跨
/// 行——空白行隔开的段头也算）；段名 = 两个引号之间（非空）；段体 =
/// 闭 `]` 之后到下一个 `[` 字符（Go `[^\[]*` 对值里的 `[` 同样截断）。
/// 返回（段名、体起点、体终点），扫描从 `from` 继续。
fn next_remote_section(config: &str, from: usize) -> Option<(String, usize, usize)> {
    let bytes = config.as_bytes();
    let mut i = from;
    loop {
        i = config[i..].find("[remote").map(|off| i + off)?;
        // (?m)^\s*：从上一个 \n（或串首）到这里全是空白才认
        let line_start = config[..i].rfind('\n').map(|p| p + 1).unwrap_or(0);
        if !config[line_start..i].bytes().all(is_go_space_byte) {
            i += 1;
            continue;
        }
        let mut p = i + "[remote".len();
        let ws_start = p;
        while p < bytes.len() && is_go_space_byte(bytes[p]) {
            p += 1;
        }
        // \s+（至少一个）+ 引号
        if p == ws_start || p >= bytes.len() || bytes[p] != b'"' {
            i += 1;
            continue;
        }
        p += 1;
        let name_start = p;
        while p < bytes.len() && bytes[p] != b'"' {
            p += 1;
        }
        // [^"]+ 非空
        if p >= bytes.len() || p == name_start {
            i += 1;
            continue;
        }
        let name = config[name_start..p].to_string();
        p += 1; // 过闭引号
        if p >= bytes.len() || bytes[p] != b']' {
            i += 1;
            continue;
        }
        p += 1;
        let body_start = p;
        let body_end = config[p..].find('[').map(|off| p + off).unwrap_or(config.len());
        return Some((name, body_start, body_end));
    }
}

/// openCodeURLRe `(?m)^\s*url\s*=\s*(.+)$` 在段体内的首匹配（Go :456）：
/// 行首空白后是 `url`，再空白后 `=`，值 = 行内剩余（可为空——空值与没有
/// url 行经过 openCodeRemotePath 的 TrimSpace 后同义，选择逻辑同样按空串
/// 处理）。
fn first_url_in(body: &str) -> Option<String> {
    for line in body.split('\n') {
        let t = line.trim_start_matches(is_go_space);
        if let Some(rest) = t.strip_prefix("url") {
            let r = rest.trim_start_matches(is_go_space);
            if let Some(v) = r.strip_prefix('=') {
                let val = v.trim_start_matches(is_go_space);
                return Some(val.to_string());
            }
        }
    }
    None
}

/// openCodeRemotePath（Go :484-509）：remote 的 URL 按 OpenCode 命名 project
/// 的样子：host/path（host 小写），去 .git；file: URL 与读不了的 → ""。
/// scheme:// 先认（`(?i)^[a-z][a-z0-9+.-]*://`），否则 SCP 形
/// `^([^@/:]+@)?([^/:]+):(.+)$`（带 Go 回溯：有 user 与无 user 两个起点）。
fn open_code_remote_path(value: &str) -> String {
    let input = value.trim();
    if input.is_empty() {
        return String::new();
    }
    let parts = |host: &str, name: &str| -> String {
        let mut p = name.trim_start_matches('/').to_string();
        // `\.git/?$` 一次（贪 ? 先吃 /）
        if let Some(stripped) = p.strip_suffix(".git/") {
            p = stripped.to_string();
        } else if let Some(stripped) = p.strip_suffix(".git") {
            p = stripped.to_string();
        }
        let p = p.trim_end_matches('/');
        if host.is_empty() || p.is_empty() {
            return String::new();
        }
        format!("{}/{}", host.to_ascii_lowercase(), p)
    };
    if is_scheme_url(input) {
        let Some((host, path, is_file)) = parse_url_host_path(input) else {
            return String::new();
        };
        if is_file {
            return String::new(); // Go :500-502
        }
        return parts(&host, &path);
    }
    if let Some((host, path)) = scp_split(input) {
        return parts(host, path);
    }
    String::new()
}

/// `(?i)^[a-z][a-z0-9+.-]*://`（Go openCodeSchemeRe :413）。
fn is_scheme_url(s: &str) -> bool {
    let Some(idx) = s.find("://") else { return false };
    let scheme = &s[..idx];
    scheme_ok(scheme) && !scheme.is_empty()
}

/// Go url.Parse 对 scheme URL 的最小面：（主机名、转义路径、是否 file）。
/// authority 到第一个 / ? #；路径 = `/` 后到 ? 或 #；userinfo 取最后一个 @
/// 之后；端口剥到第一个 `:`（IPv6 括号内除外）；`[` 没有配对的 `]` 视作
/// 解析失败（None，Go url.Parse 报错 → ""）。
fn parse_url_host_path(input: &str) -> Option<(String, String, bool)> {
    let (scheme, rest) = input.split_once("://")?;
    let is_file = scheme.eq_ignore_ascii_case("file");
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let after = &rest[end..];
    let path = if let Some(p) = after.strip_prefix('/') {
        p.split(['?', '#']).next().unwrap_or("").to_string()
    } else {
        String::new()
    };
    let hostport = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    let host = if let Some(stripped) = hostport.strip_prefix('[') {
        let close = stripped.find(']')?;
        stripped[..close].to_string()
    } else {
        hostport.split(':').next().unwrap_or(hostport).to_string()
    };
    Some((host, path, is_file))
}

/// SCP 形（Go openCodeSCPRe :414 `^([^@/:]+@)?([^/:]+):(.+)$`）的手写版，
/// 带 Go 正则的回溯：先试带 user@（user = 首个 @ 前的 Run，不含 @ / :），
/// 失败退回不带 user——`git@:path` 在 Go 里是 host="git@"、path="path"。
fn scp_split(input: &str) -> Option<(&str, &str)> {
    let mut starts = vec![0usize];
    if let Some(at) = input.find('@') {
        let user = &input[..at];
        if !user.is_empty() && !user.contains(['@', '/', ':']) {
            starts.push(at + 1);
        }
    }
    // 贪婪序：带 user 的先试（Go `([^@/:]+@)?` 的优先）
    for &start in starts.iter().rev() {
        let after = &input[start..];
        let host_end = after.find([':', '/']).unwrap_or(after.len());
        let host = &after[..host_end];
        if host.is_empty() || !after[host_end..].starts_with(':') {
            continue;
        }
        let path = &after[host_end + 1..];
        if path.is_empty() {
            continue;
        }
        return Some((host, path));
    }
    None
}

// ---------------------------------------------------------------------------
// 5. User-Agent + 更新器（Go :41-129）
// ---------------------------------------------------------------------------

/// 版本更新器的状态（Go openCodeVersion :54-59 + openCodeVersionURL :47 的
/// 测试缝收进同一个锁）：已读最新值、上次问的时刻、正在问的标记、问谁
/// （测试换成假服务）。
struct VersionState {
    v: Option<String>,
    at: Option<std::time::Instant>,
    asking: bool,
    url: String,
}

fn version_state() -> &'static Mutex<VersionState> {
    static S: OnceLock<Mutex<VersionState>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(VersionState {
            v: None,
            at: None,
            asking: false,
            url: UPDATER.to_string(),
        })
    })
}

/// OpenCodeUserAgent（Go :41-43）：`opencode/latest/{v}/cli`。
pub(crate) fn opencode_user_agent() -> String {
    format!("opencode/latest/{}/cli", open_code_latest())
}

/// openCodeLatest（Go :65-84）：发哪个版本——更新器说的最新（上一次读到的）
/// 或 OPENCODE_VERSION。更新器后台问、至多每 6h 一次；测试二进制 + 生产
/// URL 恒不问（Go `testing.Testing() && u == openCodeUpdater` 的同义）。
fn open_code_latest() -> String {
    let (v, u, due) = {
        let mut st = lock(version_state());
        let v = st.v.clone().unwrap_or_else(|| OPENCODE_VERSION.to_string());
        let u = st.url.clone();
        let recent = st.at.map(|t| t.elapsed() < VERSION_EVERY).unwrap_or(false);
        let due = !st.asking && !recent && !(cfg!(test) && u == UPDATER);
        if due {
            st.asking = true;
        }
        (v, u, due)
    };
    if !due {
        return v;
    }
    // 后台线程（Go :74-82）：问回就存，问完盖时刻、清 asking。锁外做 IO。
    // （Go 的 string 传值即可；Rust 侧给线程一份克隆，v 留在返回值里。）
    let thread_v = v.clone();
    let spawned = std::thread::Builder::new()
        .name("opencode-version".into())
        .spawn(move || {
            let found = open_code_ask_version(&u, &thread_v);
            let mut st = lock(version_state());
            if let Some(f) = found {
                st.v = Some(f);
            }
            st.at = Some(std::time::Instant::now());
            st.asking = false;
        });
    if spawned.is_err() {
        // 起不了线程：复位 asking，下次再试（Go 的 goroutine 不会失败）
        lock(version_state()).asking = false;
    }
    v
}

/// openCodeAskVersion（Go :91-116）：u 说的最新版本——读不出、不是 release
/// 形状、或比 OPENCODE_VERSION 老（陈旧镜像）→ None。问法同 zen-free 插件：
/// 3s、拒重定向（redirects(0) 时 ureq 原样交回 3xx 应答，状态非 200 → None，
/// 与 Go CheckRedirect 报错的下场一致）。
fn open_code_ask_version(u: &str, current: &str) -> Option<String> {
    let url = format!("{}?current={}", u, query_escape(current));
    let reply = match updater_agent().get(&url).call() {
        Ok(r) => r,
        // ureq 2 把非 2xx 也当 Err(Status)（chat.rs 同款取回）
        Err(_) => return None,
    };
    if reply.status() != 200 {
        return None;
    }
    let mut body = Vec::new();
    use std::io::Read as _;
    let _ = reply
        .into_reader()
        .take(64 * 1024) // Go io.LimitReader(res.Body, 64<<10)
        .read_to_end(&mut body);
    let got: serde_json::Value = serde_json::from_slice(&body).ok()?;
    // Version any → string 断言（数字/null/缺键都算没有）
    let v = got.get("version")?.as_str()?;
    if !version_shape_ok(v) || !open_code_at_least(v, OPENCODE_VERSION) {
        return None;
    }
    Some(v.to_string())
}

/// 问更新器用的 agent：整体 3s、连接 3s、不跟随重定向；TLS/代理与
/// mod.rs::outbound_agent 同源（native-tls 连接器 + Marionette 代理设置）。
fn updater_agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(3))
        .redirects(0);
    if let Ok(connector) = ureq::native_tls::TlsConnector::new() {
        builder = builder.tls_connector(std::sync::Arc::new(connector));
    }
    if let Some(proxy) = super::outbound_proxy().filter(|p| !p.trim().is_empty()) {
        if let Ok(p) = ureq::Proxy::new(proxy.trim()) {
            builder = builder.proxy(p);
        }
    }
    builder.build()
}

/// `^\d{1,4}\.\d{1,4}\.\d{1,6}$` 的手写版（Go openCodeVersionRe :86）：
/// 三段、段长 1–4 / 1–4 / 1–6、全数字。
fn version_shape_ok(v: &str) -> bool {
    fn digits(p: &str, hi: usize) -> bool {
        (1..=hi).contains(&p.len()) && p.bytes().all(|c| c.is_ascii_digit())
    }
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3 && digits(parts[0], 4) && digits(parts[1], 4) && digits(parts[2], 6)
}

/// openCodeAtLeast（Go :119-129）：版本 a 是否 b 或之后（逐段数值比较，
/// 相等算「至少」）。段缺了按 0（Go 只在正则验过形状后调，不会缺段）。
fn open_code_at_least(a: &str, b: &str) -> bool {
    let pa: Vec<u64> = a.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let pb: Vec<u64> = b.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    for i in 0..3 {
        let m = pa.get(i).copied().unwrap_or(0);
        let n = pb.get(i).copied().unwrap_or(0);
        if m != n {
            return m > n;
        }
    }
    true
}

/// url.QueryEscape 的最小版：RFC 3986 非保留字符保留、空格 → `+`、其余
/// %XX 大写。版本串（数字与点）原样通过。
fn query_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 6. 客户端头组装（Go OpenCodeClient :150-163）
// ---------------------------------------------------------------------------

/// OpenCodeClient：让请求带上 OpenCode 2.2.5 自己发给网关的东西（2026-11-01
/// 实拍）：User-Agent（opencode_user_agent）、b3 与 traceparent（一会话一
/// trace、一请求一 span）、x-opencode-client: cli、所在文件夹的 project id
/// （opencode_project）、会话四头（x-opencode-session / x-opencode-session-id /
/// x-session-affinity / x-session-id）与 Accept: */*；1.x 的
/// x-opencode-request 删掉（Zen free 层对 1.x 形状答 429，#3491）。
/// 头的发送顺序不保留（Go 注释 :147-149：HTTP/1 排序、HTTP/2 无固定序）。
pub(crate) fn opencode_client(headers: &mut Vec<(String, String)>, session: &str, dir: &str) {
    let id = opencode_session(session);
    let (b3, traceparent) = opencode_trace(&id);
    header_set(headers, "User-Agent", &opencode_user_agent());
    header_set(headers, "b3", &b3);
    header_set(headers, "traceparent", &traceparent);
    header_set(headers, "x-opencode-client", "cli");
    header_set(headers, "x-opencode-project", &opencode_project(dir));
    for k in [
        "x-opencode-session",
        "x-opencode-session-id",
        "x-session-affinity",
        "x-session-id",
    ] {
        header_set(headers, k, &id);
    }
    header_del(headers, "x-opencode-request");
    header_set(headers, "Accept", "*/*");
}

// ---------------------------------------------------------------------------
// 7. 无状态调用补会话（Go OpenCodeSession :192-199）
// ---------------------------------------------------------------------------

/// OpenCodeSession：对 OpenCode 网关的请求还没带会话时补一个——2026 年 9 月
/// 起 OpenCode 对每个不带 x-opencode-session 的请求答 400 "Request is
/// missing x-opencode-session and cannot be routed efficiently"（luci on
/// Discord）。不是某场对话回合的调用（模型列表、System One 决策、钥匙检查）
/// 自得一个会话，同 OpenCode 给自己的无状态调用那样（packages/core/src/
/// generate.ts："Gateways require session attribution even for a stateless
/// call"）。回合的会话在此之前由 opencode_client 命好并保持；用户在
/// provider 上自己钉的会话也保持。
pub(crate) fn opencode_session_or_new(headers: &mut Vec<(String, String)>, url: &str) {
    if !opencode_url(url) {
        return;
    }
    if header_get(headers, "x-opencode-session").is_some_and(|v| !v.is_empty()) {
        return;
    }
    let id = opencode_session("");
    header_set(headers, "x-opencode-session", &id);
    header_set(headers, "x-opencode-session-id", &id);
}

// ---------------------------------------------------------------------------
// 测试（向量对齐 Go opencode_client_test.go；共享态的用例全在 STATE_LOCK 下）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 动共享表（id 时钟 / trace / 版本）的用例共用一把，串行化。
    static STATE_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(test)]
    fn reset_ids_for_tests() {
        *lock(ids()) = IdState {
            last: 0,
            counter: 0,
            sessions: HashMap::new(),
            order: VecDeque::new(),
        };
    }

    #[cfg(test)]
    fn reset_traces_for_tests() {
        *lock(traces()) = TraceState {
            of: HashMap::new(),
            order: VecDeque::new(),
        };
    }

    #[cfg(test)]
    fn reset_version_state_for_tests() {
        let mut st = lock(version_state());
        *st = VersionState { v: None, at: None, asking: false, url: UPDATER.to_string() };
    }

    /// 测试缝：把更新器指到本地假服务（对齐 Go 换 openCodeVersionURL 包变量）。
    #[cfg(test)]
    fn set_version_url_for_tests(url: &str) {
        lock(version_state()).url = url.to_string();
    }

    /// Go openCodeDecode（test.go :12-22）：把 id 按 identifier.ts 的摆法读出
    /// （毫秒保 36 位、该毫秒内的计数），descending 时先反转。
    fn decode(id: &str, descending: bool) -> (u64, u64) {
        let mut v = u64::from_str_radix(&id[4..16], 16).expect("hex");
        if descending {
            v = !v & 0xffff_ffff_ffff;
        }
        (v / 0x1000, v % 0x1000)
    }

    fn temp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "marionette-ocproj-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    // —— URL 判定 ——

    #[test]
    fn url_judges_gateway_hosts() {
        assert!(opencode_url("https://opencode.ai/zen/v1/chat/completions"));
        assert!(opencode_url("https://api.opencode.ai:8080/v1"));
        assert!(opencode_url("http://opencode.ai.:443/")); // 尾点剥一次
        assert!(opencode_url("HTTPS://OPENCODE.AI/x")); // 大小写不敏感
        assert!(opencode_url("  https://opencode.ai/x  ")); // TrimSpace
        assert!(opencode_url("https://user:pw@opencode.ai/v1")); // userinfo
        assert!(opencode_url("https://zen.opencode.ai/x")); // 名下的子域
        // 协议相对（Go url.Parse 对 "//host/x" 也认 host）
        assert!(opencode_url("//opencode.ai/x"));
        // 不该算的
        assert!(!opencode_url("https://x.opencode.ai.deep/"), "不以 .opencode.ai 结尾");
        assert!(!opencode_url("https://notopencode.ai/"));
        assert!(!opencode_url("https://xopencode.ai/"));
        assert!(!opencode_url("https://opencode.ai.example.com/"));
        assert!(!opencode_url("https://opencode.ai..")); // 尾点只剥一次
        assert!(!opencode_url("opencode.ai/no-scheme")); // 无 scheme → Hostname 空
        assert!(!opencode_url("://opencode.ai/x")); // 空 scheme 是 Parse 失败面
        assert!(!opencode_url("1http://opencode.ai/")); // 数字开头不是 scheme
        assert!(!opencode_url(""));
        assert!(!opencode_url("   "));
    }

    // —— 会话 id 形状 ——

    #[test]
    fn real_session_shape() {
        // Go test :50 的两只：真 OpenCode 1.19.0 的（Zen 200）过；
        // sha256 切成同形状的（Zen 403）不过——低 12 位反转计数不对。
        assert!(opencode_real_session("ses_ecc5d8f28ffe9Cy14V8Em7lI2u"));
        assert!(!opencode_real_session("ses_e9283da055b15JwOAra76bwUx4"));
        // 形状本身不对的
        assert!(!opencode_real_session("ses_short"));
        assert!(!opencode_real_session("msg_ecc5d8f28ffe9Cy14V8Em7lI2u")); // 前缀
        assert!(!opencode_real_session("SES_ecc5d8f28ffe9Cy14V8Em7lI2u")); // 大写前缀
        assert!(!opencode_real_session("ses_ECC5D8F28FFE9Cy14V8Em7lI2u")); // hex 段大写
        assert!(!opencode_real_session("ses_ecc5d8f28ffe9Cy14V8Em7lI2ux")); // 长 1
        // 计数段的边界：id[13:16]（十二位十六进制的最后三位）反转 ≥ 0xff0 才算
        // （0xfff-low < 16）；tail 恒 14 位
        assert!(opencode_real_session("ses_ecc5d8f28fff9Cy14V8Em7lI2u"), "low=fff");
        assert!(opencode_real_session("ses_ecc5d8f28ff09Cy14V8Em7lI2u"), "low=ff0");
        assert!(!opencode_real_session("ses_ecc5d8f28fef9Cy14V8Em7lI2u"), "low=fef");
    }

    // Go TestOpenCodeIDsCountUp（:74-83）
    #[test]
    fn new_id_counts_up() {
        let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ids_for_tests();
        let now = 1_790_000_000_000i64;
        let first = opencode_new_id("msg", false, now);
        let second = opencode_new_id("msg", false, now);
        let want_ms = (1_790_000_000_000u64) % (1u64 << 36);
        let (ms, n) = decode(&first, false);
        assert_eq!(ms, want_ms, "时间保 36 位");
        assert_eq!(n, 1, "同毫秒计数从 1 起");
        let (_, n2) = decode(&second, false);
        assert_eq!(n2, 2, "第二条计数 2");
        assert!(second > first, "同毫秒内后造的字典序更大（Go :80 同口径）");
        // 换毫秒计数归一
        let third = opencode_new_id("msg", false, now + 1);
        let (_, n3) = decode(&third, false);
        assert_eq!(n3, 1);
    }

    // Go TestOpenCodeSessionIsOpenCodes（:32-70）
    #[test]
    fn session_is_opencodes() {
        let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ids_for_tests();
        let before = (now_millis() as u64) % (1u64 << 36);
        let id = opencode_session("conversation-a");
        let after = (now_millis() as u64) % (1u64 << 36);
        assert!(id_shape_ok(&id) && id.starts_with("ses_") && opencode_real_session(&id));
        let (ms, n) = decode(&id, true);
        assert!(
            ms >= before && ms <= after && n >= 1 && n < 16,
            "session {id}: 时间 {ms}（要 {before}..{after}）、计数 {n}"
        );
        // 一场对话保一个会话，另一场另得一个
        let again = opencode_session("conversation-a");
        let other = opencode_session("conversation-b");
        assert_eq!(again, id);
        assert_ne!(other, id);
        // 无 seed：各得各的、都是真的
        let a = opencode_session("");
        let b = opencode_session("");
        assert_ne!(a, b);
        assert!(opencode_real_session(&a) && opencode_real_session(&b));
        // OpenCode 自己的照发；切成同形状的哈希不过审、也换掉
        let real = "ses_ecc5d8f28ffe9Cy14V8Em7lI2u";
        let hashed = "ses_e9283da055b15JwOAra76bwUx4";
        assert_eq!(opencode_session(real), real);
        assert!(!opencode_real_session(hashed));
        let got = opencode_session(hashed);
        assert_ne!(got, hashed);
        assert!(opencode_real_session(&got));
    }

    // Go TestOpenCodeSessionsKeptBounded（:86-100）
    #[test]
    fn sessions_kept_bounded() {
        let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ids_for_tests();
        let first = opencode_session("bounded-0");
        for i in 1..=OPENCODE_KEEP {
            opencode_session(&format!("bounded-{i}"));
        }
        let st = lock(ids());
        assert!(st.order.len() <= OPENCODE_KEEP, "{} kept", st.order.len());
        drop(st);
        assert_ne!(
            opencode_session("bounded-0"),
            first,
            "最老的会话已逐出，重新拿到新 id"
        );
    }

    // —— trace ——

    #[test]
    fn trace_one_per_session_new_span_each_request() {
        let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_traces_for_tests();
        let (b3a, tpa) = opencode_trace("sess-x");
        let (b3b, tpb) = opencode_trace("sess-x");
        // 同会话同 trace、异 span
        assert_eq!(&b3a[..32], &b3b[..32], "b3 的 trace 段复用");
        assert_ne!(b3a, b3b, "span 每请求一新");
        assert_ne!(tpa, tpb);
        // 形状：b3 = trace-span-1-parent；traceparent = 00-trace-span-01
        let b: Vec<&str> = b3a.split('-').collect();
        let t: Vec<&str> = tpa.split('-').collect();
        assert_eq!(b.len(), 4);
        assert_eq!(b[2], "1");
        assert_eq!(b[0].len(), 32);
        assert_eq!(b[1].len(), 16);
        assert_eq!(b[3].len(), 16);
        assert_eq!(t.len(), 4, "traceparent = 00-trace-span-01（四段）");
        assert_eq!(t[0], "00");
        assert_eq!(t[3], "01");
        assert_eq!(t[1], b[0], "traceparent 与 b3 同 trace");
        assert_eq!(t[2], b[1], "traceparent 与 b3 同 span");
        assert!(
            b[0].bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                && b[1].bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "十六进制小写"
        );
        // 另一会话另一 trace
        let (c3, _) = opencode_trace("sess-y");
        assert_ne!(&c3[..32], &b3a[..32]);
    }

    #[test]
    fn traces_kept_bounded() {
        let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_traces_for_tests();
        let (first, _) = opencode_trace("bounded-0");
        for i in 1..=OPENCODE_KEEP {
            opencode_trace(&format!("bounded-{i}"));
        }
        let st = lock(traces());
        assert!(st.order.len() <= OPENCODE_KEEP);
        drop(st);
        let (again, _) = opencode_trace("bounded-0");
        assert_ne!(again, first, "最老的 trace 已逐出");
    }

    // —— remote path（纯函数，Go openCodeRemotePath :484-509）——

    #[test]
    fn remote_path_forms() {
        // SCP 形
        assert_eq!(open_code_remote_path("git@github.com:foo/bar.git"), "github.com/foo/bar");
        assert_eq!(open_code_remote_path("github.com:foo/bar.git"), "github.com/foo/bar");
        assert_eq!(open_code_remote_path("git@github.com:foo/bar.git/"), "github.com/foo/bar");
        assert_eq!(open_code_remote_path("user@gitlab.com:x/y.git"), "gitlab.com/x/y");
        // scheme 形：host 小写、路径原样、.git 大小写敏感不剥（Go `\.git/?$` 无 (?i)）
        assert_eq!(open_code_remote_path("https://GitHub.com/Foo/Bar.GIT"), "github.com/Foo/Bar.GIT");
        assert_eq!(open_code_remote_path("ssh://git@github.com:22/foo/bar.git"), "github.com/foo/bar");
        assert_eq!(open_code_remote_path("https://gitlab.com/x/y.git/"), "gitlab.com/x/y");
        assert_eq!(open_code_remote_path("HTTPS://github.com/a/b.git"), "github.com/a/b");
        // file: 与读不了的
        assert_eq!(open_code_remote_path("file:///tmp/x.git"), "");
        assert_eq!(open_code_remote_path("FILE://host/x.git"), "");
        assert_eq!(open_code_remote_path("https://host"), "");
        assert_eq!(open_code_remote_path("https://host/"), "");
        assert_eq!(open_code_remote_path("not-a-remote"), "");
        assert_eq!(open_code_remote_path("foo/bar.git"), "");
        assert_eq!(open_code_remote_path(""), "");
        assert_eq!(open_code_remote_path("   "), "");
        // 前后空白（Go TrimSpace）
        assert_eq!(open_code_remote_path("  https://github.com/a/b.git  "), "github.com/a/b");
    }

    // —— 远端选择（纯函数，Go :453-471）——

    #[test]
    fn remote_url_selection() {
        // origin 段在后，压过先列的 upstream
        let cfg = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@gitlab.com:up/first.git\n\tfetch = +refs/heads/*:refs/remotes/upstream/*\n[remote \"origin\"]\n\turl = git@github.com:foo/bar.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n";
        assert_eq!(pick_remote_url(cfg), "git@github.com:foo/bar.git");
        // origin 在前
        let cfg = "[remote \"origin\"]\n\turl = https://github.com/oo/aa.git\n[remote \"later\"]\n\turl = https://github.com/ll/zz.git\n";
        assert_eq!(pick_remote_url(cfg), "https://github.com/oo/aa.git");
        // 无 origin → 第一段
        let cfg = "[remote \"a\"]\n\turl = https://github.com/one/one.git\n[remote \"b\"]\n\turl = https://github.com/two/two.git\n";
        assert_eq!(pick_remote_url(cfg), "https://github.com/one/one.git");
        // origin 段存在但没有 url 行 → u = origin = ""（不退回 first，Go :468）
        let cfg = "[remote \"upstream\"]\n\turl = https://github.com/first/one.git\n[remote \"origin\"]\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n";
        assert_eq!(pick_remote_url(cfg), "");
        // 无 remote 段
        assert_eq!(pick_remote_url("[core]\n\tbare = false\n"), "");
        // 值里带 `[` 的段体在 Go `[^\[]*` 处截断——url 行被切断成半个值
        let cfg = "[remote \"origin\"]\n\turl = https://github.com/a[weird/b.git\n";
        assert_eq!(pick_remote_url(cfg), "https://github.com/a", "段体止于下一个 [ 字符");
        // 空格连着的段头（`[remote   "x"]`）与空白行隔开的段头都认
        let cfg = "\n\n[remote   \"origin\"]\n\turl = https://github.com/sp/sp.git\n";
        assert_eq!(pick_remote_url(cfg), "https://github.com/sp/sp.git");
        // urlx / url2 不是 url 行
        let cfg = "[remote \"origin\"]\n\turls = x\n\turlx = y\n\turl = https://github.com/ok/ok.git\n";
        assert_eq!(pick_remote_url(cfg), "https://github.com/ok/ok.git");
    }

    // —— 仓库发现（临时 .git 夹具，Go openCodeRepository :422-479）——

    #[test]
    fn repository_nearest_worktree_and_none() {
        let root = temp_root("repo");
        let main = root.join("main");
        std::fs::create_dir_all(main.join(".git/worktrees/wt")).expect("dirs");
        std::fs::write(
            main.join(".git/config"),
            "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://github.com/foo/bar.git\n",
        )
        .expect("config");
        // worktree：.git 是文件 → gitdir（绝对）→ commondir 相对回主 .git
        let wt = root.join("wt");
        std::fs::create_dir_all(&wt).expect("wt");
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .expect("git file");
        std::fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").expect("commondir");
        let (rootp, remote, ok) = open_code_repository(wt.to_str().unwrap());
        assert!(ok);
        assert_eq!(remote, "github.com/foo/bar", "worktree 经 commondir 找到主仓的 origin");
        assert_eq!(Path::new(&rootp), wt.as_path(), "root = 发现 .git 的那层");
        // 相对 gitdir（不绝对 → Join(d, own)）
        let wt2 = root.join("wt2");
        std::fs::create_dir_all(&wt2).expect("wt2");
        std::fs::write(wt2.join(".git"), "gitdir: ../main/.git/worktrees/wt\n").expect("git file");
        let (_, remote2, ok2) = open_code_repository(wt2.to_str().unwrap());
        assert!(ok2);
        assert_eq!(remote2, "github.com/foo/bar");
        // 嵌套向上找最近的 .git
        let deep = main.join("sub/deep");
        std::fs::create_dir_all(&deep).expect("deep");
        let (rootp3, remote3, ok3) = open_code_repository(deep.to_str().unwrap());
        assert!(ok3);
        assert_eq!(remote3, "github.com/foo/bar");
        assert_eq!(Path::new(&rootp3), main.as_path());
        // 非 repo
        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).expect("plain");
        let (r, rem, ok4) = open_code_repository(plain.to_str().unwrap());
        assert!(!ok4);
        assert!(r.is_empty() && rem.is_empty());
        // .git 文件缺 gitdir 行 → 整个上行判定为非 repo（Go :433-435 不再向上走）
        let outer = root.join("outer");
        std::fs::create_dir_all(outer.join(".git")).expect("outer");
        std::fs::write(
            outer.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://github.com/outer/outer.git\n",
        )
        .expect("config");
        let inner = outer.join("inner");
        std::fs::create_dir_all(&inner).expect("inner");
        std::fs::write(inner.join(".git"), "not a gitdir file\n").expect("git file");
        let (r, rem, ok5) = open_code_repository(inner.to_str().unwrap());
        assert!(!ok5, "坏 .git 文件不是「回到 outer 的 remote」，是直接不是 repo");
        assert!(rem.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    // —— project id 三态（Go OpenCodeProject :377-394）——

    #[test]
    fn project_three_states() {
        let root = temp_root("proj");
        // git-remote 态：键只含 remote，与路径无关——期望值完全独立
        let repo = root.join("r1");
        std::fs::create_dir_all(repo.join(".git")).expect("git");
        std::fs::write(
            repo.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://GitHub.com/Foo/Bar.git\n",
        )
        .expect("config");
        assert_eq!(
            opencode_project(repo.to_str().unwrap()),
            sha1_hex(b"git-remote:github.com/Foo/Bar"),
            "host 小写、路径大小写原样、.git 剥掉"
        );
        // 嵌套子目录同 id（向上找仓库）
        let sub = repo.join("a/b");
        std::fs::create_dir_all(&sub).expect("sub");
        assert_eq!(opencode_project(sub.to_str().unwrap()), sha1_hex(b"git-remote:github.com/Foo/Bar"));
        // file: remote → 无 remote → directory:root
        let repo2 = root.join("r2");
        std::fs::create_dir_all(repo2.join(".git")).expect("git");
        std::fs::write(
            repo2.join(".git/config"),
            "[remote \"origin\"]\n\turl = file:///srv/x.git\n",
        )
        .expect("config");
        let local2 = open_code_local_dir(repo2.to_str().unwrap());
        assert_eq!(
            opencode_project(repo2.to_str().unwrap()),
            sha1_hex(format!("directory:{local2}").as_bytes()),
            "file: 的 remote 不算，仓库按所在文件夹命名"
        );
        // 无 remote 仓库 → directory:root（root = .git 所在层）
        let repo3 = root.join("r3");
        std::fs::create_dir_all(repo3.join(".git")).expect("git");
        std::fs::write(repo3.join(".git/config"), "[core]\n\tbare = false\n").expect("config");
        let local3 = open_code_local_dir(repo3.to_str().unwrap());
        assert_eq!(
            opencode_project(repo3.to_str().unwrap()),
            sha1_hex(format!("directory:{local3}").as_bytes())
        );
        // 非 repo → directory:local
        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).expect("plain");
        let local4 = open_code_local_dir(plain.to_str().unwrap());
        assert_eq!(
            opencode_project(plain.to_str().unwrap()),
            sha1_hex(format!("directory:{local4}").as_bytes())
        );
        // 空 dir = home（不炸即可，home 依赖环境）
        let _ = opencode_project("");
        let _ = std::fs::remove_dir_all(&root);
    }

    // —— User-Agent 与版本 ——

    #[test]
    fn user_agent_shape_and_floor() {
        let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_version_state_for_tests();
        let ua = opencode_user_agent();
        assert!(
            ua.starts_with("opencode/latest/") && ua.ends_with("/cli"),
            "UA {ua}"
        );
        let v = ua
            .strip_prefix("opencode/latest/")
            .unwrap()
            .strip_suffix("/cli")
            .unwrap();
        assert!(version_shape_ok(v), "版本形状 {v}");
        assert!(open_code_at_least(v, OPENCODE_VERSION), "不低于 2.2.5");
        reset_version_state_for_tests();
    }

    #[test]
    fn version_shape_and_floor() {
        assert!(version_shape_ok("2.2.5"));
        assert!(version_shape_ok("12.34.567890"));
        assert!(version_shape_ok("0.0.0"));
        assert!(!version_shape_ok("2.2"));
        assert!(!version_shape_ok("2.2.5.1"));
        assert!(!version_shape_ok("v2.2.5"));
        assert!(!version_shape_ok("12345.1.1"), "段长 1..=4");
        assert!(!version_shape_ok("2.2.1234567"), "段长 1..=6");
        assert!(!version_shape_ok("2..5"));
        assert!(!version_shape_ok("a.b.c"));
        assert!(!version_shape_ok(""));
        assert!(open_code_at_least("2.2.5", "2.2.5"));
        assert!(open_code_at_least("2.3.0", "2.2.5"));
        assert!(!open_code_at_least("2.2.4", "2.2.5"));
        assert!(open_code_at_least("2.10.0", "2.9.9"), "数值比较不是字典序");
        assert!(open_code_at_least("3.0.0", "2.2.5"));
        // query_escape：版本串原样、空格/保留字符转义
        assert_eq!(query_escape("2.2.5"), "2.2.5");
        assert_eq!(query_escape("a b"), "a+b");
        assert_eq!(query_escape("a&b"), "a%26b");
    }

    /// 本地假更新器：答一发指定状态/体（redirect 给 Location 头）。
    fn fake_updater(status: u16, body: &'static str, location: Option<&'static str>) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind");
        let port = server.server_addr().to_ip().expect("ip").port();
        std::thread::spawn(move || {
            if let Ok(req) = server.recv() {
                // 两个分支统一成 Response<Cursor<Vec<u8>>>（empty 的体类型不同）
                let resp = match location {
                    Some(loc) => tiny_http::Response::from_string(String::new())
                        .with_status_code(302)
                        .with_header(
                            tiny_http::Header::from_bytes(&b"Location"[..], loc.as_bytes())
                                .expect("header"),
                        ),
                    None => tiny_http::Response::from_string(body).with_status_code(status),
                };
                let _ = req.respond(resp);
            }
        });
        format!("http://127.0.0.1:{port}/update/api/latest/cli/npm")
    }

    #[test]
    fn ask_version_reads_and_guards() {
        // 只在 query 对时给答案的假服务：证明 ?current= 确实拼上
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind");
        let port = server.server_addr().to_ip().expect("ip").port();
        std::thread::spawn(move || {
            if let Ok(req) = server.recv() {
                let ok = req.url().contains("current=2.2.5");
                let body = if ok { r#"{"version":"2.3.0"}"# } else { "no" };
                let status = if ok { 200 } else { 400 };
                let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(status));
            }
        });
        let u = format!("http://127.0.0.1:{port}/update/api/latest/cli/npm");
        assert_eq!(open_code_ask_version(&u, "2.2.5").as_deref(), Some("2.3.0"));
        // 低于下限的镜像（Go :112 openCodeAtLeast）
        let u = fake_updater(200, r#"{"version":"2.2.4"}"#, None);
        assert_eq!(open_code_ask_version(&u, "2.2.5"), None);
        // 形状不对
        let u = fake_updater(200, r#"{"version":"v2.3.0"}"#, None);
        assert_eq!(open_code_ask_version(&u, "2.2.5"), None);
        // 不是字符串（Go Version any 的断言失败）
        let u = fake_updater(200, r#"{"version":123}"#, None);
        assert_eq!(open_code_ask_version(&u, "2.2.5"), None);
        // 缺键
        let u = fake_updater(200, r#"{"ver":"2.3.0"}"#, None);
        assert_eq!(open_code_ask_version(&u, "2.2.5"), None);
        // 不是 JSON
        let u = fake_updater(200, "not json", None);
        assert_eq!(open_code_ask_version(&u, "2.2.5"), None);
        // 非 200
        let u = fake_updater(404, r#"{"version":"2.3.0"}"#, None);
        assert_eq!(open_code_ask_version(&u, "2.2.5"), None);
        // 拒重定向（Go CheckRedirect err → ""）
        let u = fake_updater(302, "", Some("http://127.0.0.1:9/moved"));
        assert_eq!(open_code_ask_version(&u, "2.2.5"), None);
    }

    #[test]
    fn latest_never_asks_production_in_tests() {
        let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_version_state_for_tests();
        // 测试二进制 + 生产 URL：绝不真发（cfg!(test) 对齐 Go testing.Testing()）
        assert_eq!(open_code_latest(), OPENCODE_VERSION);
        {
            let st = lock(version_state());
            assert!(!st.asking && st.at.is_none() && st.v.is_none(), "没起后台线程");
        }
        // 白盒注入已读版本 → UA 用它
        lock(version_state()).v = Some("9.9.9".to_string());
        assert_eq!(open_code_latest(), "9.9.9");
        assert!(opencode_user_agent().contains("/9.9.9/"));
        reset_version_state_for_tests();
    }

    #[test]
    fn latest_asks_injected_url_in_background() {
        let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_version_state_for_tests();
        let (url, handle) = {
            let server = tiny_http::Server::http("127.0.0.1:0").expect("bind");
            let port = server.server_addr().to_ip().expect("ip").port();
            let handle = std::thread::spawn(move || {
                if let Ok(req) = server.recv() {
                    let ok = req.url().contains("current=2.2.5");
                    let body = if ok { r#"{"version":"2.3.0"}"# } else { "no" };
                    let status = if ok { 200 } else { 400 };
                    let _ =
                        req.respond(tiny_http::Response::from_string(body).with_status_code(status));
                }
            });
            (format!("http://127.0.0.1:{port}/update/api/latest/cli/npm"), handle)
        };
        set_version_url_for_tests(&url);
        assert_eq!(open_code_latest(), OPENCODE_VERSION, "第一发先回兜底值");
        let mut got = String::new();
        for _ in 0..150 {
            got = lock(version_state()).v.clone().unwrap_or_default();
            if got == "2.3.0" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(got, "2.3.0", "后台线程问回最新版并落进状态");
        assert_eq!(open_code_latest(), "2.3.0", "6h 内用已读值不再问");
        let _ = handle.join();
        reset_version_state_for_tests();
    }

    // —— 客户端头组装（Go :150-163 + test :60-69）——

    #[test]
    fn client_assembles_and_strips() {
        let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ids_for_tests();
        reset_traces_for_tests();
        let id = opencode_session("conversation-a");
        let mut h: Vec<(String, String)> = vec![
            ("x-opencode-request".to_string(), "old".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("X-Session-Affinity".to_string(), "from-the-agent".to_string()),
        ];
        opencode_client(&mut h, "conversation-a", "");
        fn get(h: &[(String, String)], n: &str) -> String {
            header_get(h, n).unwrap_or("").to_string()
        }
        assert_eq!(get(&h, "x-opencode-session"), id);
        assert_eq!(get(&h, "x-opencode-session-id"), id);
        assert_eq!(get(&h, "x-session-affinity"), id, "2.2.5 也把会话放 affinity（原值被顶掉）");
        assert_eq!(get(&h, "x-session-id"), id);
        assert_eq!(get(&h, "User-Agent"), opencode_user_agent());
        assert_eq!(get(&h, "x-opencode-client"), "cli");
        assert_eq!(get(&h, "Accept"), "*/*");
        assert_eq!(get(&h, "Content-Type"), "application/json", "别的头不碰");
        assert!(
            header_get(&h, "x-opencode-request").is_none(),
            "1.x 的 x-opencode-request 被删（#3491）"
        );
        assert_eq!(get(&h, "x-opencode-project").len(), 40, "sha1 十六进制");
        // b3 / traceparent 同源同 span
        let b3 = get(&h, "b3");
        let tp = get(&h, "traceparent");
        let b: Vec<&str> = b3.split('-').collect();
        let t: Vec<&str> = tp.split('-').collect();
        assert_eq!(b.len(), 4);
        assert_eq!(b[2], "1");
        assert_eq!(t.len(), 4, "traceparent = 00-trace-span-01（四段）");
        assert_eq!(t[0], "00");
        assert_eq!(t[3], "01");
        assert_eq!(t[1], b[0]);
        assert_eq!(t[2], b[1]);
        // Set 语义：重复设置不叠（大小写不敏感地整删再放）
        opencode_client(&mut h, "conversation-a", "");
        assert_eq!(get(&h, "x-opencode-session"), id, "同对话同 id");
        assert_eq!(
            h.iter().filter(|(k, _)| k.eq_ignore_ascii_case("b3")).count(),
            1,
            "Set 不叠"
        );
    }

    // —— 无状态调用补会话（Go OpenCodeSession :192-199）——

    #[test]
    fn session_or_new_only_on_gateway_urls_without_one() {
        let _g = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ids_for_tests();
        // 非 opencode 主机不动
        let mut h = vec![("Authorization".to_string(), "k".to_string())];
        opencode_session_or_new(&mut h, "https://example.com/v1/chat");
        assert!(header_get(&h, "x-opencode-session").is_none());
        // opencode 主机且没有 → 补 session + session-id，两个一致、形状真
        let mut h = Vec::new();
        opencode_session_or_new(&mut h, "https://opencode.ai/zen/v1/chat/completions");
        let id = header_get(&h, "x-opencode-session").unwrap().to_string();
        assert!(opencode_real_session(&id));
        assert_eq!(header_get(&h, "x-opencode-session-id"), Some(id.as_str()));
        // 已有就不动（用户/上游钉的会话保住）
        let mut h = vec![("x-opencode-session".to_string(), "pinned".to_string())];
        opencode_session_or_new(&mut h, "https://opencode.ai/zen/v1/chat");
        assert_eq!(header_get(&h, "x-opencode-session"), Some("pinned"));
        assert!(header_get(&h, "x-opencode-session-id").is_none());
        // 端口与尾点也算网关主机
        let mut h = Vec::new();
        opencode_session_or_new(&mut h, "https://api.opencode.ai.:443/v1");
        assert!(header_get(&h, "x-opencode-session").is_some());
        // Go h.Get 的口径：空串值视作没有（Get 返回 "" → 补），非空值哪怕
        // 全空白也是「用户钉的」→ 原样保留
        let mut h = vec![("X-OpenCode-Session".to_string(), "  ".to_string())];
        opencode_session_or_new(&mut h, "https://opencode.ai/zen/v1/chat");
        assert_eq!(header_get(&h, "x-opencode-session"), Some("  "), "空白值也是用户钉的，保留");
        let mut h = vec![("x-opencode-session".to_string(), String::new())];
        opencode_session_or_new(&mut h, "https://opencode.ai/zen/v1/chat");
        assert!(
            header_get(&h, "x-opencode-session").is_some_and(|v| !v.is_empty()),
            "空串值视作没有，补上真会话"
        );
    }

    // —— go_is_abs（filepath.IsAbs Windows 语义）——

    #[test]
    fn absolute_path_shapes() {
        if cfg!(target_os = "windows") {
            assert!(go_is_abs("C:\\x"));
            assert!(go_is_abs("C:/x"));
            assert!(go_is_abs("\\\\server\\share"));
            assert!(go_is_abs("\\x"));
            assert!(!go_is_abs("C:x"));
            assert!(!go_is_abs("C:"));
            assert!(!go_is_abs("/x"));
            assert!(!go_is_abs("rel/x"));
        } else {
            assert!(go_is_abs("/x"));
            assert!(!go_is_abs("x"));
        }
    }
}
