//! Council 验证阶段:执行计划声明的 verification 命令并收集证据。
//!
//! 任务书 §4.3:成功的工具调用不是任务成功的证据——独立验证阶段跑
//! build/test/lint 等命令。命令经 shell 执行(cmd /C),带超时与输出
//! 截断;破坏性模式(远端推送/发布/递归删除)拦截为 needs-human,
//! 绝不静默执行(注入防护:命令只能来自 PlanDoc 的 verification 数组,
//! 提示词注入无法在运行期扩这个清单——清单在 Plan Gate 时就已冻结)。

use serde::{Deserialize, Serialize};

/// 单条验证命令的结果。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    pub command: String,
    pub ok: bool,
    pub skipped: bool,
    /// 破坏性命令被拦截且尚无人工裁决(Attention:需要人)。
    #[serde(default)]
    pub needs_human: bool,
    pub exit_code: Option<i32>,
    /// 合并后的 stdout/stderr(截断到 8KB,尾部保留——错误通常在末尾)。
    pub output: String,
}

/// 破坏性模式:命中即不执行,标记需人工。词边界匹配避免误杀
/// `test_push.py` 之类。
const DANGEROUS: &[&str] = &[
    "git push",
    "npm publish",
    "cargo publish",
    "twine upload",
    "rm -rf",
    "rd /s",
    "del /",
    "format ",
    "shutdown",
];

pub fn is_dangerous(command: &str) -> bool {
    let lower = command.to_lowercase();
    DANGEROUS.iter().any(|p| lower.contains(p))
}

const OUTPUT_LIMIT: usize = 8 * 1024;

/// 执行一条验证命令。`cwd` 是项目根;`approved`/`rejected` 是人工对
/// 破坏性命令的裁决留痕(见 state 的 approved/rejected_verifications):
/// 批准 → 真执行;拒绝 → 按拦截失败进修复回路;都没有 → 挂起待裁决。
pub fn run_verification(
    cwd: &std::path::Path,
    command: &str,
    timeout_secs: u64,
    approved: &[String],
    rejected: &[String],
) -> VerifyResult {
    if is_dangerous(command) {
        if approved.iter().any(|c| c == command) {
            // 人工已放行:按原文执行(清单本身仍然冻结,放行只对
            // 被 DANGEROUS 拦下的这条命令生效,模型无法借道)。
        } else {
            let rejected_hit = rejected.iter().any(|c| c == command);
            return VerifyResult {
                command: command.to_string(),
                ok: false,
                skipped: true,
                needs_human: !rejected_hit,
                exit_code: None,
                output: if rejected_hit {
                    "命令包含破坏性操作,用户已拒绝放行;请修改计划改用安全的验证方式。".into()
                } else {
                    "命令包含破坏性操作(推送/发布/递归删除),已拦截:等待人工裁决。".into()
                },
            };
        }
    }
    let mut child = match std::process::Command::new("cmd")
        .args(["/C", command])
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags_no_window()
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return VerifyResult {
                command: command.to_string(),
                ok: false,
                skipped: false,
                needs_human: false,
                exit_code: None,
                output: format!("无法启动命令:{e}"),
            }
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs.max(1));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = child.stdout.take().map(read_capped).unwrap_or_default();
                let err = child.stderr.take().map(read_capped).unwrap_or_default();
                if !err.is_empty() {
                    out.push_str("\n[stderr]\n");
                    out.push_str(&err);
                }
                let code = status.code();
                return VerifyResult {
                    command: command.to_string(),
                    ok: code == Some(0),
                    skipped: false,
                    needs_human: false,
                    exit_code: code,
                    output: truncate_tail(&out, OUTPUT_LIMIT),
                };
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                return VerifyResult {
                    command: command.to_string(),
                    ok: false,
                    skipped: false,
                    needs_human: false,
                    exit_code: None,
                    output: format!("等待命令失败:{e}"),
                };
            }
        }
    }
    // 循环只经 return 或超时 break 退出——到这里一定是超时。
    let _ = child.kill();
    let _ = child.wait();
    VerifyResult {
        command: command.to_string(),
        ok: false,
        skipped: false,
        needs_human: false,
        exit_code: None,
        output: format!("命令超时({timeout_secs}s)被终止。"),
    }
}

/// 全量验证:逐条执行,全部结果返回(顺序与计划声明一致)。
pub fn run_all(
    cwd: &std::path::Path,
    commands: &[String],
    timeout_secs: u64,
    approved: &[String],
    rejected: &[String],
) -> Vec<VerifyResult> {
    commands
        .iter()
        .map(|c| run_verification(cwd, c, timeout_secs, approved, rejected))
        .collect()
}

/// 尚未裁决的破坏性命令(拦截挂起的候选集合)。
pub fn pending_decisions(results: &[VerifyResult]) -> Vec<String> {
    results.iter().filter(|r| r.needs_human).map(|r| r.command.clone()).collect()
}

/// 结果渲染成 Evidence 消息正文。
pub fn render_evidence(results: &[VerifyResult]) -> String {
    if results.is_empty() {
        return "计划未声明验证命令。".to_string();
    }
    let mut out = String::from("验证结果:\n");
    for r in results {
        let verdict = if r.needs_human {
            "⚠ 已拦截(等待人工裁决)"
        } else if r.skipped {
            "⚠ 已拦截(用户拒绝放行)"
        } else if r.ok {
            "✓ 通过"
        } else {
            "✗ 失败"
        };
        out.push_str(&format!("\n### {verdict} `{}`\n", r.command));
        if !r.output.trim().is_empty() {
            out.push_str("```\n");
            out.push_str(r.output.trim());
            out.push_str("\n```\n");
        }
    }
    out
}

fn read_capped<R: std::io::Read>(mut r: R) -> String {
    let mut buf = vec![0u8; OUTPUT_LIMIT + 1];
    let mut filled = 0;
    loop {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => {
                filled += n;
                if filled > OUTPUT_LIMIT {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    buf.truncate(filled.min(OUTPUT_LIMIT));
    String::from_utf8_lossy(&buf).to_string()
}

/// 截断保留尾部(错误信息通常在末尾)。
fn truncate_tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let cut = s.len() - max;
    let mut start = cut;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…(前 {} 字节截断)\n{}", cut, &s[start..])
}

/// CREATE_NO_WINDOW(council 的命令是后台执行的,不能闪控制台窗)。
trait CreationFlags {
    fn creation_flags_no_window(&mut self) -> &mut Self;
}

#[cfg(windows)]
impl CreationFlags for std::process::Command {
    fn creation_flags_no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        self.creation_flags(CREATE_NO_WINDOW);
        self
    }
}

#[cfg(not(windows))]
impl CreationFlags for std::process::Command {
    fn creation_flags_no_window(&mut self) -> &mut Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marionette-council-verify-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn success_and_failure_commands() {
        let dir = tmp("ok");
        let ok = run_verification(&dir, "echo hello", 10, &[], &[]);
        assert!(ok.ok, "{:?}", ok.output);
        assert!(ok.output.contains("hello"));
        let fail = run_verification(&dir, "cmd /C exit 3", 10, &[], &[]);
        assert!(!fail.ok);
        assert_eq!(fail.exit_code, Some(3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timeout_kills_command() {
        let dir = tmp("timeout");
        let res = run_verification(&dir, "ping -n 30 127.0.0.1 > nul", 1, &[], &[]);
        assert!(!res.ok);
        assert!(res.output.contains("超时"), "{:?}", res.output);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dangerous_commands_are_intercepted() {
        let dir = tmp("danger");
        for cmd in ["git push origin main", "npm publish --access public", "rm -rf /tmp/x", "cargo publish"] {
            let res = run_verification(&dir, cmd, 5, &[], &[]);
            assert!(res.skipped, "{cmd} 应被拦截");
            assert!(!res.ok);
            assert!(res.needs_human, "{cmd} 应标记待人工裁决");
        }
        // 正常命令不受影响(词边界:含 push 子串的文件名不误杀)。
        assert!(!is_dangerous("cargo test --test push_repo"));
        assert!(!is_dangerous("npm run build"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dangerous_command_user_verdicts() {
        let dir = tmp("verdict");
        let cmd = "git push origin main";
        // 批准 → 真执行(本机 git 会因非仓库报错,但退出码存在 = 执行了)。
        let approved = vec![cmd.to_string()];
        let res = run_verification(&dir, cmd, 10, &approved, &[]);
        assert!(!res.skipped, "批准后不应拦截:{:?}", res.output);
        assert!(!res.needs_human);
        // 拒绝 → 拦截但不再挂起(进修复回路,防死循环)。
        let rejected = vec![cmd.to_string()];
        let res = run_verification(&dir, cmd, 10, &[], &rejected);
        assert!(res.skipped);
        assert!(!res.needs_human, "已裁决的命令不应再次挂起");
        assert!(res.output.contains("拒绝"));
        // 名单按原文精确匹配:同前缀不同文本不放行。
        let res = run_verification(&dir, cmd, 5, &["git push origin other".to_string()], &[]);
        assert!(res.needs_human);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_decisions_filter() {
        let results = vec![
            VerifyResult { command: "npm run build".into(), ok: true, skipped: false, needs_human: false, exit_code: Some(0), output: String::new() },
            VerifyResult { command: "git push".into(), ok: false, skipped: true, needs_human: true, exit_code: None, output: String::new() },
            VerifyResult { command: "cargo publish".into(), ok: false, skipped: true, needs_human: false, exit_code: None, output: String::new() },
        ];
        assert_eq!(pending_decisions(&results), vec!["git push".to_string()]);
    }

    #[test]
    fn evidence_rendering() {
        let results = vec![
            VerifyResult { command: "npm run build".into(), ok: true, skipped: false, needs_human: false, exit_code: Some(0), output: "built".into() },
            VerifyResult { command: "cargo test".into(), ok: false, skipped: false, needs_human: false, exit_code: Some(101), output: "test failed".into() },
        ];
        let text = render_evidence(&results);
        assert!(text.contains("✓ 通过"));
        assert!(text.contains("✗ 失败"));
        assert!(text.contains("cargo test"));
    }
}
