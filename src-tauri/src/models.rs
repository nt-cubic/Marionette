use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub created_at: String,
    pub last_opened_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub label: String,
    pub cwd: String,
    pub status: String,
    pub process_id: Option<u32>,
    pub pty_id: Option<String>,
    pub started_at: String,
    pub last_active_at: String,
    pub exited_at: Option<String>,
    pub exit_code: Option<i32>,
    pub raw_log_path: String,
    pub transcript_path: String,
    pub handoff_path: String,
    pub view_mode: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub id: String,
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd_mode: String,
    pub launch_mode: String,
    pub send_strategy: String,
    pub parser: String,
    pub transport: String,
    pub enabled: bool,
}

impl AgentConfig {
    pub fn defaults() -> Vec<Self> {
        vec![
            Self::new("opencode", "OpenCode", "opencode", vec!["acp".to_string()]),
            Self::new(
                "codex",
                "Codex CLI",
                "npx",
                vec![
                    "-y".to_string(),
                    "@agentclientprotocol/codex-acp".to_string(),
                ],
            ),
            Self::new(
                "claude-code",
                "Claude Code",
                "npx",
                vec![
                    "-y".to_string(),
                    "@agentclientprotocol/claude-agent-acp".to_string(),
                ],
            ),
            Self::new(
                "grok-build",
                "Grok Build",
                "grok",
                vec!["build".to_string()],
            ),
        ]
    }

    fn new(id: &str, label: &str, command: &str, args: Vec<String>) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            command: command.to_string(),
            args,
            cwd_mode: "project-root".to_string(),
            launch_mode: "pty".to_string(),
            send_strategy: "bracketed-paste".to_string(),
            parser: "ansi-raw".to_string(),
            transport: if id == "grok-build" {
                "pty".to_string()
            } else {
                "acp".to_string()
            },
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandStatus {
    pub id: String,
    pub status: String,
    pub path: Option<String>,
    pub message: String,
}
