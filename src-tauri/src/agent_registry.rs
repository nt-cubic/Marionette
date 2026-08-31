//! Built-in ACP agent harness metadata (Codeg-aligned ids, pins, launch policy).
//!
//! Does **not** replace project-level Skill/MCP lending (`context_inventory`).
//! Policy hooks tell `acp` how to advertise capabilities and whether to inject MCP.

use serde::Serialize;
use serde_json::{json, Value};

/// Whether this agent may receive `session/new.mcpServers` entries at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpWirePolicy {
    /// Inject project-lent servers (default).
    Forward,
    /// Agent rejects non-empty mcpServers (OpenClaw).
    Never,
    /// Accepts field but does not use it (Pi) — skip inject to avoid noise.
    SkipForward,
}

/// How the agent binary / adapter is distributed (Codeg pin model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistributionKind {
    /// Global npm / npx package (pin as `name@version` when installing).
    Npx,
    /// Vendor binary or release zip (binary_cache later).
    Binary,
    /// Python via `uvx --from …` (Hermes).
    Uvx,
    /// Vendor installer only (Grok, Cursor).
    Manual,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHarnessMeta {
    pub id: &'static str,
    pub label: &'static str,
    /// Advertise ACP `elicitation.form` on initialize (Codex only in Codeg).
    pub elicitation_form: bool,
    /// Claude Code subagent transcript meta.
    pub subagent_transcript: bool,
    #[serde(skip)]
    pub mcp_wire: McpWirePolicy,
    /// Serialize-friendly MCP policy.
    pub mcp_wire_label: &'static str,
    #[serde(skip)]
    pub distribution: DistributionKind,
    pub distribution_label: &'static str,
    /// Pinned package / release version (Codeg registry). `None` = unpinned manual.
    pub pin_version: Option<&'static str>,
    /// npm / uvx package spec used for install or cold start.
    /// e.g. `@agentclientprotocol/codex-acp@1.1.0` or `hermes-agent[acp,mcp]==0.18.2`.
    pub pin_package: Option<&'static str>,
    /// Primary ACP command on PATH (or console script).
    pub cmd: &'static str,
    /// Default args after `cmd`.
    pub args: &'static [&'static str],
    /// Min Node major.minor.patch when distribution is Npx (`None` = no check).
    pub node_required: Option<&'static str>,
    /// Min `uv` version when distribution is Uvx.
    pub uv_required: Option<&'static str>,
    /// `uvx --python` pin (Hermes).
    pub python_pin: Option<&'static str>,
    /// Extra CLI commands that must exist for the agent to work (e.g. `codex`, `pi`).
    pub requires_commands: &'static [&'static str],
    /// Env vars the launcher may set (name, value). Applied in acp start when present.
    pub launch_env: &'static [(&'static str, &'static str)],
}

impl Serialize for McpWirePolicy {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            McpWirePolicy::Forward => "forward",
            McpWirePolicy::Never => "never",
            McpWirePolicy::SkipForward => "skip",
        })
    }
}

fn mcp_label(p: McpWirePolicy) -> &'static str {
    match p {
        McpWirePolicy::Forward => "forward",
        McpWirePolicy::Never => "never",
        McpWirePolicy::SkipForward => "skip",
    }
}

fn dist_label(d: DistributionKind) -> &'static str {
    match d {
        DistributionKind::Npx => "npx",
        DistributionKind::Binary => "binary",
        DistributionKind::Uvx => "uvx",
        DistributionKind::Manual => "manual",
    }
}

/// Codeg-aligned pin table (2026-07 registry snapshot). Prefer PATH binary over npx cold start.
pub fn harness_meta(agent_id: &str) -> AgentHarnessMeta {
    match agent_id {
        "codex" | "codex-acp" => AgentHarnessMeta {
            id: "codex",
            label: "Codex CLI",
            elicitation_form: true,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
            pin_version: Some("1.1.0"),
            pin_package: Some("@agentclientprotocol/codex-acp@1.1.0"),
            cmd: "codex-acp",
            args: &[],
            node_required: None,
            uv_required: None,
            python_pin: None,
            requires_commands: &["codex"],
            launch_env: &[],
        },
        "claude-code" | "claude-acp" => AgentHarnessMeta {
            id: "claude-code",
            label: "Claude Code",
            elicitation_form: false,
            subagent_transcript: true,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
            pin_version: Some("0.57.0"),
            pin_package: Some("@agentclientprotocol/claude-agent-acp@0.57.0"),
            cmd: "claude-agent-acp",
            args: &[],
            node_required: None,
            uv_required: None,
            python_pin: None,
            requires_commands: &["claude"],
            launch_env: &[],
        },
        "grok-build" | "grok" => AgentHarnessMeta {
            id: "grok-build",
            label: "Grok Build",
            elicitation_form: false,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Manual,
            distribution_label: dist_label(DistributionKind::Manual),
            pin_version: None,
            pin_package: None,
            cmd: "grok",
            args: &["--trust", "agent", "stdio"],
            node_required: None,
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        "opencode" => AgentHarnessMeta {
            id: "opencode",
            label: "OpenCode",
            elicitation_form: false,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            // Codeg uses Binary cache; we still list npm package for one-click until binary_cache lands.
            distribution: DistributionKind::Binary,
            distribution_label: dist_label(DistributionKind::Binary),
            pin_version: Some("1.17.15"),
            pin_package: Some("opencode-ai"),
            cmd: "opencode",
            args: &["acp"],
            node_required: None,
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        "openclaw" | "openclaw-acp" => AgentHarnessMeta {
            id: "openclaw",
            label: "OpenClaw",
            elicitation_form: false,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Never,
            mcp_wire_label: mcp_label(McpWirePolicy::Never),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
            pin_version: Some("2026.6.11"),
            pin_package: Some("openclaw@2026.6.11"),
            cmd: "openclaw",
            args: &["acp"],
            node_required: Some("22.19.0"),
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        "pi" | "pi-acp" => AgentHarnessMeta {
            id: "pi",
            label: "Pi",
            elicitation_form: false,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::SkipForward,
            mcp_wire_label: mcp_label(McpWirePolicy::SkipForward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
            pin_version: Some("0.0.31"),
            pin_package: Some("pi-acp@0.0.31"),
            cmd: "pi-acp",
            args: &[],
            node_required: Some("22.0.0"),
            uv_required: None,
            python_pin: None,
            requires_commands: &["pi"],
            launch_env: &[("PI_ACP_ENABLE_EMBEDDED_CONTEXT", "true")],
        },
        "cline" => AgentHarnessMeta {
            id: "cline",
            label: "Cline",
            elicitation_form: false,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
            pin_version: Some("3.0.38"),
            pin_package: Some("cline@3.0.38"),
            cmd: "cline",
            args: &["--acp"],
            node_required: None,
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        "gemini" => AgentHarnessMeta {
            id: "gemini",
            label: "Gemini CLI",
            elicitation_form: false,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
            pin_version: Some("0.49.0"),
            pin_package: Some("@google/gemini-cli@0.49.0"),
            cmd: "gemini",
            args: &["--acp", "--skip-trust"],
            node_required: Some("20.0.0"),
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        "kimi-code" | "kimi" => AgentHarnessMeta {
            id: "kimi-code",
            label: "Kimi Code",
            elicitation_form: false,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
            pin_version: Some("0.23.2"),
            pin_package: Some("@moonshot-ai/kimi-code@0.23.2"),
            cmd: "kimi",
            args: &["acp"],
            node_required: Some("22.19.0"),
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        "codebuddy" | "codebuddy-code" => AgentHarnessMeta {
            id: "codebuddy",
            label: "CodeBuddy",
            elicitation_form: false,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
            pin_version: Some("2.117.2"),
            pin_package: Some("@tencent-ai/codebuddy-code@2.117.2"),
            cmd: "codebuddy",
            args: &["--acp"],
            node_required: Some("22.0.0"),
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        "hermes" => AgentHarnessMeta {
            id: "hermes",
            label: "Hermes Agent",
            elicitation_form: false,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Uvx,
            distribution_label: dist_label(DistributionKind::Uvx),
            pin_version: Some("0.18.2"),
            pin_package: Some("hermes-agent[acp,mcp]==0.18.2"),
            // Prefer PATH `hermes acp`; uvx path is install/preflight guidance.
            cmd: "hermes",
            args: &["acp"],
            node_required: None,
            uv_required: Some("0.5.0"),
            python_pin: Some("3.13"),
            requires_commands: &[],
            launch_env: &[],
        },
        "cursor" => AgentHarnessMeta {
            id: "cursor",
            label: "Cursor",
            elicitation_form: false,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Manual,
            distribution_label: dist_label(DistributionKind::Manual),
            pin_version: None,
            pin_package: None,
            // Must not install a global `agent` that collides with Grok.
            cmd: "cursor-agent",
            args: &["acp"],
            node_required: None,
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        // `omp acp` — native ACP server. MCP Forward verified live: omp expects
        // `session/new.mcpServers` as an array of {name, command, args, env}
        // entries (env/headers as {name, value} arrays) — exactly what
        // `mcp_payload_for_agent` emits, and acp.rs always sends the key.
        // Form elicitation on: omp routes generic approval prompts through
        // `elicitation.form` when the client advertises it (Codex path).
        "omp" => AgentHarnessMeta {
            id: "omp",
            label: "OMP",
            elicitation_form: true,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Manual,
            distribution_label: dist_label(DistributionKind::Manual),
            pin_version: None,
            pin_package: None,
            cmd: "omp",
            args: &["acp"],
            node_required: None,
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        _ => AgentHarnessMeta {
            id: "unknown",
            label: "Agent",
            elicitation_form: false,
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Manual,
            distribution_label: dist_label(DistributionKind::Manual),
            pin_version: None,
            pin_package: None,
            cmd: "agent",
            args: &[],
            node_required: None,
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
    }
}

/// JSON clientCapabilities for ACP `initialize` (Codeg per-agent gates).
pub fn build_client_capabilities_json(agent_id: Option<&str>) -> Value {
    let meta = agent_id.map(harness_meta);
    let mut caps = json!({
        "fs": {
            "readTextFile": true,
            "writeTextFile": true
        },
        "terminal": true,
        "session": {
            "configOptions": {
                "boolean": {}
            }
        }
    });
    if meta.as_ref().map(|m| m.elicitation_form).unwrap_or(false) {
        caps.as_object_mut().unwrap().insert(
            "elicitation".into(),
            json!({ "form": {} }),
        );
    }
    if meta.as_ref().map(|m| m.subagent_transcript).unwrap_or(false) {
        caps.as_object_mut().unwrap().insert(
            "_meta".into(),
            json!({ "subagent-transcript": true }),
        );
    }
    caps
}

/// Whether to call `mcp_payload_for_agent` for this agent.
pub fn should_inject_mcp(agent_id: Option<&str>) -> bool {
    match agent_id.map(harness_meta).map(|m| m.mcp_wire) {
        Some(McpWirePolicy::Never) | Some(McpWirePolicy::SkipForward) => false,
        _ => true,
    }
}

/// Launch env pairs for an agent (empty if none).
pub fn launch_env_for(agent_id: &str) -> &'static [(&'static str, &'static str)] {
    harness_meta(agent_id).launch_env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_codex_gets_elicitation_form() {
        assert!(harness_meta("codex").elicitation_form);
        assert!(!harness_meta("claude-code").elicitation_form);
        assert!(!harness_meta("grok-build").elicitation_form);
    }

    #[test]
    fn openclaw_never_pi_skip_mcp() {
        assert_eq!(harness_meta("openclaw").mcp_wire, McpWirePolicy::Never);
        assert_eq!(harness_meta("pi").mcp_wire, McpWirePolicy::SkipForward);
        assert!(!should_inject_mcp(Some("openclaw")));
        assert!(!should_inject_mcp(Some("pi")));
        assert!(should_inject_mcp(Some("codex")));
    }

    #[test]
    fn pins_match_codeg_snapshot() {
        assert_eq!(harness_meta("codex").pin_version, Some("1.1.0"));
        assert_eq!(harness_meta("claude-code").pin_version, Some("0.57.0"));
        assert_eq!(harness_meta("gemini").pin_version, Some("0.49.0"));
        assert_eq!(harness_meta("cline").pin_version, Some("3.0.38"));
        assert_eq!(harness_meta("hermes").pin_version, Some("0.18.2"));
        assert_eq!(harness_meta("hermes").python_pin, Some("3.13"));
        assert_eq!(harness_meta("pi").pin_version, Some("0.0.31"));
        assert_eq!(harness_meta("opencode").pin_version, Some("1.17.15"));
    }

    #[test]
    fn cursor_does_not_claim_agent_binary() {
        assert_eq!(harness_meta("cursor").cmd, "cursor-agent");
    }

    #[test]
    fn omp_uses_acp_launch_with_form_elicitation_and_mcp() {
        let meta = harness_meta("omp");
        assert!(meta.elicitation_form);
        assert_eq!(meta.mcp_wire, McpWirePolicy::Forward);
        assert!(should_inject_mcp(Some("omp")));
        assert_eq!(meta.cmd, "omp");
        assert_eq!(meta.args, &["acp"]);
        assert_eq!(meta.distribution, DistributionKind::Manual);
    }
}
