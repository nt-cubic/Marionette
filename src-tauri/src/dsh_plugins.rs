//! DSH 插件面板的后端 —— `dsh plugin --profile acp …` 的薄封装。
//!
//! profile `acp` 是 spawn 侧（acp.rs 的 DSH 分支）与 dsh web 共享的组合层：
//! 插件装进它，ACP 会话原生带上（tools / commands / skills 由 openma 适配器
//! 自动投影到会话面）。装完需要**重启会话**才生效——运行中的进程持有内存里
//! 的旧组合。
//!
//! `dsh plugin` 本身是 pnpm 转发器（`add <pkg>` / `remove <pkg>` /
//! `list`），首次使用会把 profile 初始化为 `$DSH_HOME/profiles/acp`。

use serde::Serialize;
use std::process::Command;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshPlugin {
    pub name: String,
    pub version: String,
}

fn dsh_command(args: &[&str]) -> Result<Command, String> {
    let resolved = crate::process_util::resolve_spawn_command("dsh")
        .map_err(|e| crate::ts!("`dsh` 不在 PATH 上（先安装 @deepseek-ai/dsh）: {e}", "`dsh` is not on PATH (install @deepseek-ai/dsh first): {e}"))?;
    let mut command = Command::new(&resolved.program);
    resolved.apply_to(&mut command);
    command.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(command)
}

fn run_dsh_plugin(args: &[&str], timeout_secs: u64) -> Result<String, String> {
    let mut command = dsh_command(args)?;
    let child = command
        .output()
        .map_err(|e| format!("run dsh plugin failed: {e}"))?;
    let _ = timeout_secs; // std Command 无超时；pnpm 自身有网络超时，够用
    let stdout = crate::process_util::decode_process_bytes(&child.stdout);
    let stderr = crate::process_util::decode_process_bytes(&child.stderr);
    if !child.status.success() {
        let tail: String = stderr.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        return Err(crate::ts!("dsh plugin 失败（{}）: {}", "dsh plugin failed ({}): {}", child.status, tail));
    }
    Ok(format!("{stdout}{stderr}"))
}

/// `dsh plugin --profile acp list` → 插件清单。首次调用会顺带初始化 profile。
pub fn list() -> Result<Vec<DshPlugin>, String> {
    let output = run_dsh_plugin(&["plugin", "--profile", "acp", "list"], 120)?;
    Ok(parse_list(&output))
}

/// `dsh plugin --profile acp add <spec>`（spec：npm 名 / github:owner/repo /
/// 文件路径）。返回 pnpm 的输出摘要给面板展示。
pub fn add(spec: &str) -> Result<String, String> {
    let output = run_dsh_plugin(&["plugin", "--profile", "acp", "add", spec], 600)?;
    let tail: Vec<&str> = output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(8)
        .collect();
    let mut summary: Vec<&str> = tail.into_iter().rev().collect();
    summary.reverse();
    Ok(summary.join("\n"))
}

/// `dsh plugin --profile acp remove <name>`。
pub fn remove(name: &str) -> Result<String, String> {
    run_dsh_plugin(&["plugin", "--profile", "acp", "remove", name], 300).map(|_| String::new())
}

/// 解析 pnpm list 的树形输出：抓 `└── name@version` 行（含 `│ ├──` 嵌套层）。
pub fn parse_list(output: &str) -> Vec<DshPlugin> {
    let mut plugins = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim_start();
        // 必须是树形依赖行（顶层或嵌套），纯文本行一概不碰。
        if !trimmed.starts_with(['│', '├', '└']) {
            continue;
        }
        let cleaned = trimmed.trim_start_matches(['│', '├', '└', '─', '┬', ' ']);
        if cleaned.is_empty() || cleaned.ends_with(':') {
            continue;
        }
        let Some((name, version)) = cleaned.split_once('@') else {
            continue;
        };
        if name.is_empty() || version.is_empty() {
            continue;
        }
        plugins.push(DshPlugin {
            name: name.to_string(),
            version: version.to_string(),
        });
    }
    plugins
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pnpm_tree() {
        let output = "Legend: production dependency\n\
                      dsh-profile-acp C:\\Users\\x\\.dsh\\profiles\\acp (PRIVATE)\n\
                      ├─┬ dsh-web-all@1.2.3\n\
                      │ └── dsh-pet@0.3.7\n\
                      └── billion-context@2.0.1\n\
                      2 packages\n";
        let plugins = parse_list(output);
        // 顶层 └──/├── 行抓 name@version；嵌套行（│ └──）同样匹配。
        let names: Vec<&str> = plugins.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"billion-context"));
        assert!(names.contains(&"dsh-pet"));
        let bc = plugins.iter().find(|p| p.name == "billion-context").unwrap();
        assert_eq!(bc.version, "2.0.1");
    }

    #[test]
    fn ignores_headings_and_garbage() {
        let output = "dependencies:\n└── \nrandom line without glyph\n1 package";
        assert!(parse_list(output).is_empty());
    }
}
