//! DeepSeek Harness: `DEEPSEEK_ACP_SESSIONS_ROOT` | `$DSH_HOME/sessions` | `~/.dsh/sessions`.

use super::jsonl_agents;
use super::util::{env_dir, home_join};
use super::{AgentParser, ExternalConversation};
use serde_json::Value;
use std::path::PathBuf;

pub struct DeepSeekParser;

fn sessions_root() -> Option<PathBuf> {
    env_dir("DEEPSEEK_ACP_SESSIONS_ROOT").or_else(|| {
        Some(env_dir("DSH_HOME").or_else(|| home_join(".dsh"))?.join("sessions"))
    })
}

impl AgentParser for DeepSeekParser {
    fn source(&self) -> &'static str {
        "deepseek"
    }
    fn agent_id(&self) -> &'static str {
        "deepseek"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let roots = sessions_root().into_iter().collect::<Vec<_>>();
        Ok(jsonl_agents_list("deepseek", "deepseek", &roots, project_root))
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        jsonl_agents_load(locator, "deepseek")
    }
}

fn jsonl_agents_list(
    source: &str,
    agent_id: &str,
    roots: &[PathBuf],
    project_root: &str,
) -> Vec<ExternalConversation> {
    // Reuse the tree walker via a tiny shim: KimiParser's list_jsonl_tree is crate-private.
    // Call through the same module function by temporarily constructing roots.
    jsonl_agents::list_tree_for(source, agent_id, roots, project_root)
}

fn jsonl_agents_load(locator: &str, agent_id: &str) -> Result<Vec<Value>, String> {
    jsonl_agents::load_tree_locator(locator, agent_id)
}
