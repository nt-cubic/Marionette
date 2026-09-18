//! Public ACP agent catalog (cdn.agentclientprotocol.com).
//!
//! Used by the custom-agent picker. Binary-only entries are listed so the UI
//! can say "put the command on PATH" — Marionette does not download archives.

use serde::Serialize;
use serde_json::Value;

const REGISTRY_URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogChannel {
    pub kind: String,
    pub package: Option<String>,
    pub cmd: Option<String>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAgent {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub channels: Vec<CatalogChannel>,
    /// True when this machine has an npx or uvx recipe (in-app installable).
    pub installable: bool,
}

fn as_obj(v: &Value) -> Option<&serde_json::Map<String, Value>> {
    v.as_object()
}

fn str_field(obj: &serde_json::Map<String, Value>, key: &str) -> String {
    obj.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn string_vec(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn channel_from_npx(npx: &Value) -> Option<CatalogChannel> {
    let obj = as_obj(npx)?;
    let package = str_field(obj, "package");
    if package.is_empty() {
        return None;
    }
    let cmd = str_field(obj, "cmd");
    Some(CatalogChannel {
        kind: "npx".into(),
        package: Some(package),
        cmd: if cmd.is_empty() { None } else { Some(cmd) },
        args: string_vec(obj.get("args")),
    })
}

fn channel_from_uvx(uvx: &Value) -> Option<CatalogChannel> {
    let obj = as_obj(uvx)?;
    let package = str_field(obj, "package");
    if package.is_empty() {
        return None;
    }
    let cmd = str_field(obj, "cmd");
    Some(CatalogChannel {
        kind: "uvx".into(),
        package: Some(package),
        cmd: if cmd.is_empty() { None } else { Some(cmd) },
        args: string_vec(obj.get("args")),
    })
}

fn summarize_agent(entry: &Value) -> Option<CatalogAgent> {
    let obj = as_obj(entry)?;
    let id = str_field(obj, "id");
    let name = str_field(obj, "name");
    if id.is_empty() || name.is_empty() {
        return None;
    }
    let dist = obj.get("distribution")?;
    let dist_obj = as_obj(dist)?;
    let mut channels = Vec::new();
    if let Some(npx) = dist_obj.get("npx").and_then(channel_from_npx) {
        channels.push(npx);
    }
    if let Some(uvx) = dist_obj.get("uvx").and_then(channel_from_uvx) {
        channels.push(uvx);
    }
    if dist_obj.get("binary").is_some() {
        channels.push(CatalogChannel {
            kind: "binary".into(),
            package: None,
            cmd: None,
            args: Vec::new(),
        });
    }
    if channels.is_empty() {
        return None;
    }
    let installable = channels.iter().any(|c| c.kind == "npx" || c.kind == "uvx");
    Some(CatalogAgent {
        id,
        name,
        description: str_field(obj, "description"),
        version: str_field(obj, "version"),
        channels,
        installable,
    })
}

pub fn fetch() -> Result<Vec<CatalogAgent>, String> {
    let raw = crate::http_client::get_json(REGISTRY_URL)?;
    let agents = raw
        .get("agents")
        .and_then(Value::as_array)
        .ok_or_else(|| "ACP registry JSON has no agents[]".to_string())?;
    Ok(agents.iter().filter_map(summarize_agent).collect())
}

/// Strip `@version` / `==version` from a package spec for in-app npm install.
#[allow(dead_code)]
pub fn unpinned_package(spec: &str) -> String {
    let mut s = spec.trim();
    for sep in ["==", ">=", "<=", "~=", "!=", ">", "<"] {
        if let Some(idx) = s.find(sep) {
            s = &s[..idx];
        }
    }
    if let Some(idx) = s.find('[') {
        s = &s[..idx];
    }
    let search_from = if s.starts_with('@') {
        s.find('/').map(|i| i + 1).unwrap_or(0)
    } else {
        0
    };
    if let Some(idx) = s[search_from..].find('@') {
        s = &s[..search_from + idx];
    }
    s.trim().to_string()
}

#[allow(dead_code)]
pub fn derive_command_name(package: &str) -> String {
    let mut s = unpinned_package(package);
    if let Some(idx) = s.rfind('/') {
        s = s[idx + 1..].to_string();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_npm_and_pep_pins() {
        assert_eq!(
            unpinned_package("@scope/agent-cli@1.2.3"),
            "@scope/agent-cli"
        );
        assert_eq!(unpinned_package("deepseek-acp@0.9.0"), "deepseek-acp");
        assert_eq!(
            unpinned_package("hermes-agent[acp,mcp]==0.21.1"),
            "hermes-agent"
        );
        assert_eq!(derive_command_name("@scope/agent-cli@1.2.3"), "agent-cli");
    }
}
