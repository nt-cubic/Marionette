//! Built-in ACP agent harness metadata (ids, launch policy, capability ads).
//!
//! Does **not** replace project-level Skill/MCP lending (`context_inventory`).
//! Policy hooks tell `acp` how to advertise capabilities and whether to inject MCP.
//! Install/update follows npm **latest** on PATH — this file does not pin versions.

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

/// How the agent binary / adapter is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistributionKind {
    /// Global npm / npx package. Install tracks registry latest.
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
    /// Claude Code subagent transcript meta.
    pub subagent_transcript: bool,
    #[serde(skip)]
    pub mcp_wire: McpWirePolicy,
    /// Serialize-friendly MCP policy.
    pub mcp_wire_label: &'static str,
    #[serde(skip)]
    pub distribution: DistributionKind,
    pub distribution_label: &'static str,
    /// Primary ACP command on PATH (or console script).
    pub cmd: &'static str,
    /// Default args after `cmd`.
    pub args: &'static [&'static str],
    /// Min Node major.minor.patch when distribution is Npx (`None` = no check).
    pub node_required: Option<&'static str>,
    /// Min `uv` version when distribution is Uvx.
    pub uv_required: Option<&'static str>,
    /// `uvx --python` for Hermes (`<3.14`).
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

/// Built-in launch metadata. Prefer PATH binary over npx cold start.
pub fn harness_meta(agent_id: &str) -> AgentHarnessMeta {
    match agent_id {
        "codex" | "codex-acp" => AgentHarnessMeta {
            id: "codex",
            label: "Codex CLI",
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
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
            subagent_transcript: true,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
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
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Manual,
            distribution_label: dist_label(DistributionKind::Manual),
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
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            // Codeg uses Binary cache; we still list npm package for one-click until binary_cache lands.
            distribution: DistributionKind::Binary,
            distribution_label: dist_label(DistributionKind::Binary),
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
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Never,
            mcp_wire_label: mcp_label(McpWirePolicy::Never),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
            cmd: "openclaw",
            args: &["acp"],
            node_required: Some("24.16.0"),
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        "pi" | "pi-acp" => AgentHarnessMeta {
            id: "pi",
            label: "Pi",
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::SkipForward,
            mcp_wire_label: mcp_label(McpWirePolicy::SkipForward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
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
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
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
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
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
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
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
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
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
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Uvx,
            distribution_label: dist_label(DistributionKind::Uvx),
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
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Manual,
            distribution_label: dist_label(DistributionKind::Manual),
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
        // omp routes generic approval prompts through `elicitation.form`.
        "omp" => AgentHarnessMeta {
            id: "omp",
            label: "OMP",
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Manual,
            distribution_label: dist_label(DistributionKind::Manual),
            cmd: "omp",
            args: &["acp"],
            node_required: None,
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        // Community editor bridge (`deepseek-acp`), not the automation-only
        // `@deepseek-ai/dsh-acp`. Speaks full ACP (streaming, tools, MCP over
        // session/new). Auth is `DEEPSEEK_API_KEY` / `~/.dsh/.credentials.yaml`.
        "deepseek" | "deepseek-acp" => AgentHarnessMeta {
            id: "deepseek",
            label: "DeepSeek Harness",
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Npx,
            distribution_label: dist_label(DistributionKind::Npx),
            cmd: "deepseek-acp",
            args: &[],
            node_required: Some("22.0.0"),
            uv_required: None,
            python_pin: None,
            requires_commands: &[],
            launch_env: &[],
        },
        _ => AgentHarnessMeta {
            id: "unknown",
            label: "Agent",
            subagent_transcript: false,
            mcp_wire: McpWirePolicy::Forward,
            mcp_wire_label: mcp_label(McpWirePolicy::Forward),
            distribution: DistributionKind::Manual,
            distribution_label: dist_label(DistributionKind::Manual),
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

/// JSON clientCapabilities for ACP `initialize`.
///
/// Advertise every client feature we actually implement so agents will use it
/// (form questions, compaction summaries, session notices). Per-agent gates
/// stay only where the wire is agent-specific (`subagent-transcript`).
///
/// `fs.readTextFile` is deliberately not advertised. An agent that sees it
/// routes every `read_file` through ACP's `fs/read_text_file`, whose result is
/// a `String` and so can never carry an image; Grok then answers a PNG/JPG read
/// with `stream did not contain valid UTF-8`. Our reader is a plain disk
/// passthrough anyway, so the capability buys nothing and only denies agents
/// their own image-aware reader.
pub fn build_client_capabilities_json(agent_id: Option<&str>) -> Value {
    let meta = agent_id.map(harness_meta);
    let mut caps = json!({
        "fs": {
            "writeTextFile": true
        },
        "terminal": true,
        "elicitation": {
            "form": {}
        },
        "session": {
            "configOptions": {
                "boolean": {}
            },
            "compaction": {},
            "notices": {}
        }
    });
    // Always advertise AIR sessionFailure so Claude (and adapters that
    // follow it) can name connection / auth / limit failures instead of
    // dumping a generic RPC error. subagent-transcript stays Claude-only.
    let mut meta_obj = serde_json::Map::new();
    meta_obj.insert(
        "jetbrains.air".into(),
        json!({
            "version": 1,
            "capabilities": ["sessionFailure"]
        }),
    );
    if meta.as_ref().map(|m| m.subagent_transcript).unwrap_or(false) {
        meta_obj.insert("subagent-transcript".into(), json!(true));
    }
    caps.as_object_mut()
        .unwrap()
        .insert("_meta".into(), Value::Object(meta_obj));
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
    fn every_agent_gets_form_elicitation_compaction_and_notices() {
        for agent in [
            None,
            Some("grok-build"),
            Some("opencode"),
            Some("claude-code"),
            Some("codex"),
            Some("omp"),
        ] {
            let caps = build_client_capabilities_json(agent);
            assert_eq!(
                caps["elicitation"]["form"],
                json!({}),
                "form elicitation (agent: {agent:?})"
            );
            assert_eq!(
                caps["session"]["compaction"],
                json!({}),
                "compaction (agent: {agent:?})"
            );
            assert_eq!(
                caps["session"]["notices"],
                json!({}),
                "notices (agent: {agent:?})"
            );
        }
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
    fn launch_metadata_still_names_the_binaries() {
        assert_eq!(harness_meta("hermes").python_pin, Some("3.13"));
        assert_eq!(harness_meta("deepseek").cmd, "deepseek-acp");
        assert_eq!(harness_meta("openclaw").node_required, Some("24.16.0"));
        assert_eq!(harness_meta("codex").cmd, "codex-acp");
    }

    #[test]
    fn client_capabilities_advertise_air_session_failure() {
        let caps = build_client_capabilities_json(Some("claude-code"));
        assert_eq!(
            caps["_meta"]["jetbrains.air"]["capabilities"][0],
            "sessionFailure"
        );
        assert_eq!(caps["_meta"]["subagent-transcript"], true);
        let grok = build_client_capabilities_json(Some("grok-build"));
        assert!(grok["_meta"]["subagent-transcript"].is_null());
        assert_eq!(
            grok["_meta"]["jetbrains.air"]["capabilities"][0],
            "sessionFailure"
        );
    }

    /// Regression: advertising `fs.readTextFile` makes image-aware agents send
    /// every `read_file` through ACP's text-only `fs/read_text_file`, so a PNG
    /// or JPG read comes back as `stream did not contain valid UTF-8`.
    #[test]
    fn client_capabilities_never_advertise_text_file_reads() {
        for agent in [
            None,
            Some("grok-build"),
            Some("claude-code"),
            Some("codex"),
            Some("gemini"),
        ] {
            let caps = build_client_capabilities_json(agent);
            assert!(
                caps["fs"]["readTextFile"].is_null(),
                "readTextFile must stay off (agent: {agent:?})"
            );
            assert_eq!(caps["fs"]["writeTextFile"], json!(true));
        }
    }

    #[test]
    fn cursor_does_not_claim_agent_binary() {
        assert_eq!(harness_meta("cursor").cmd, "cursor-agent");
    }

    #[test]
    fn omp_uses_acp_launch_with_form_elicitation_and_mcp() {
        let meta = harness_meta("omp");
        assert_eq!(
            build_client_capabilities_json(Some("omp"))["elicitation"]["form"],
            json!({})
        );
        assert_eq!(meta.mcp_wire, McpWirePolicy::Forward);
        assert!(should_inject_mcp(Some("omp")));
        assert_eq!(meta.cmd, "omp");
        assert_eq!(meta.args, &["acp"]);
        assert_eq!(meta.distribution, DistributionKind::Manual);
    }
}
