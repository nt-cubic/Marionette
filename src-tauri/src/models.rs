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
    /// Last Composer model id for this dialog (ACP option value). Restored when caps allow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
    /// Last execution mode id (e.g. build / plan).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_mode: Option<String>,
    /// Numeric effort 0–1 when agent uses a slider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_effort: Option<f64>,
    /// Discrete effort id (e.g. Claude low/high) when agent uses select options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_effort_id: Option<String>,
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
            // Clean View product path = ACP for every first-class agent.
            // PTY remains a fallback for custom CLIs without a protocol.
            Self::new(
                "opencode",
                "OpenCode",
                "opencode",
                vec!["acp".to_string()],
                "acp",
                "stdin",
            ),
            // Global installs: npm i -g @agentclientprotocol/codex-acp
            // process_util rewrites .cmd → node dist/index.js for working stdio pipes.
            Self::new(
                "codex",
                "Codex CLI",
                "codex-acp",
                vec![],
                "acp",
                "stdin",
            ),
            Self::new(
                "claude-code",
                "Claude Code",
                "claude-agent-acp",
                vec![],
                "acp",
                "stdin",
            ),
            // Native ACP: `grok agent stdio` (not the interactive TUI).
            Self::new(
                "grok-build",
                "Grok Build",
                "grok",
                vec!["agent".to_string(), "stdio".to_string()],
                "acp",
                "stdin",
            ),
        ]
    }

    fn new(
        id: &str,
        label: &str,
        command: &str,
        args: Vec<String>,
        transport: &str,
        send_strategy: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            command: command.to_string(),
            args,
            cwd_mode: "project-root".to_string(),
            launch_mode: "pty".to_string(),
            send_strategy: send_strategy.to_string(),
            parser: "ansi-raw".to_string(),
            transport: transport.to_string(),
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffResult {
    pub project_id: String,
    pub target_agent_id: String,
    pub handoff_path: String,
    pub prompt: String,
    pub created_at: String,
    pub summary: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub change_type: String,
}


