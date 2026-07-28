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
    /// Grok `/always-approve` (permission auto-approve). Not a session mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_always_approve: Option<bool>,
}

/// A CLI the ACP bridge shells out to (installed separately from the bridge).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDependency {
    pub command: String,
    pub label: String,
    /// npm package that provides `command`, when there is one.
    pub package: Option<String>,
}

/// How Marionette can put this agent's ACP command on the machine.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInstallSpec {
    /// `npm` → one-click install; `manual` → vendor installer only.
    pub manager: String,
    pub package: Option<String>,
    pub requires: Vec<AgentDependency>,
    pub note: Option<String>,
}

impl AgentInstallSpec {
    fn npm(package: &str, requires: Vec<AgentDependency>) -> Self {
        Self {
            manager: "npm".to_string(),
            package: Some(package.to_string()),
            requires,
            note: None,
        }
    }

    fn manual(note: &str) -> Self {
        Self {
            manager: "manual".to_string(),
            package: None,
            requires: Vec::new(),
            note: Some(note.to_string()),
        }
    }
}

fn dependency(command: &str, label: &str, package: Option<&str>) -> AgentDependency {
    AgentDependency {
        command: command.to_string(),
        label: label.to_string(),
        package: package.map(str::to_string),
    }
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
    /// Where `command` comes from — drives the in-app installer.
    pub install: AgentInstallSpec,
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
                AgentInstallSpec::npm("opencode-ai", Vec::new()),
            ),
            // process_util rewrites .cmd → node dist/index.js for working stdio pipes.
            Self::new(
                "codex",
                "Codex CLI",
                "codex-acp",
                vec![],
                "acp",
                "stdin",
                AgentInstallSpec::npm(
                    "@agentclientprotocol/codex-acp",
                    // The bridge drives the Codex CLI; without it the session dies at prompt time.
                    vec![dependency("codex", "Codex CLI", Some("@openai/codex"))],
                ),
            ),
            Self::new(
                "claude-code",
                "Claude Code",
                "claude-agent-acp",
                vec![],
                "acp",
                "stdin",
                AgentInstallSpec::npm(
                    "@agentclientprotocol/claude-agent-acp",
                    vec![dependency(
                        "claude",
                        "Claude Code CLI",
                        Some("@anthropic-ai/claude-code"),
                    )],
                ),
            ),
            // Native ACP: `grok --trust agent stdio` (not the interactive TUI).
            // `--trust` is required so project-scoped MCP (Unity under `.grok/` /
            // `mcps/`) actually attaches — otherwise Grok silently loads only
            // global servers and Marionette's lend path has nothing to talk to.
            Self::new(
                "grok-build",
                "Grok Build",
                "grok",
                vec![
                    "--trust".to_string(),
                    "agent".to_string(),
                    "stdio".to_string(),
                ],
                "acp",
                "stdin",
                AgentInstallSpec::manual(
                    "Grok ships its own installer (no npm package) — install the Grok CLI, then make sure `grok` is on PATH.",
                ),
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
        install: AgentInstallSpec,
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
            install,
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
    /// Marionette knows an npm package for whatever is missing here.
    #[serde(default)]
    pub installable: bool,
    /// Labels of the pieces that are not on PATH (bridge and/or its CLIs).
    #[serde(default)]
    pub missing: Vec<String>,
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


