//! Bridge DSH's profile-configured model routes into the flat settings document
//! the ACP adapter reads.
//!
//! ## The skew this repairs
//!
//! DeepSeek Harness 0.2.0-rc.2 moved its settings out of `$DSH_HOME/settings.yaml`
//! and into *profile patch* files — `$DSH_HOME/profiles/<name>/cordis.patch.yml`,
//! an ordered list of loader patch entries. The legacy document is renamed to
//! `settings.yaml.imported` on the first boot after the upgrade, and never read
//! again.
//!
//! Marionette reaches DSH through the community ACP adapter `deepseek-acp`
//! (0.9.0 — the newest published), which still composes `dsh-settings-file`, so
//! its settings document is hard-wired to `<DSH_HOME>/settings.yaml`. Worse, the
//! adapter mounts the `llm-pi-ai` plugin — the one that turns the user's
//! `llm-pi-ai.providers` entries into selectable models — *only* when that
//! document's raw text contains the substring `llm-pi-ai`
//! (`settingsMentionsPiAi`), because that import costs ~5 s.
//!
//! So after the upgrade the file is gone, the gate stays shut, and every routed
//! provider (opencode-go, opencode, xai, …) disappears from `session/new`. All
//! that is left are the two models `dsh-llm-deepseek` bakes in: the user sees
//! "only the two official DeepSeek models", and a saved preference for a routed
//! model comes back as `config option "model" has no value "…"`.
//!
//! ## What this does — and refuses to do
//!
//! It transplants the `llm-pi-ai` section verbatim from the active profile patch
//! (the most recently modified profile that declares one), falling back to
//! `settings.yaml.imported`, into `<DSH_HOME>/settings.yaml`.
//!
//! * DSH's own files are only ever **read** — the profile patch is never
//!   rewritten, so the harness app keeps owning it.
//! * The document written carries an ownership header, so a later run can tell
//!   its own mirror (which it refreshes once the profile is newer) from a
//!   hand-written file (which it never touches).
//! * A non-empty `settings.yaml` without the header and without `llm-pi-ai` is
//!   reported rather than rewritten: its shape is not ours to guess.
//! * Only the one section is written, because it is the only settings namespace
//!   `deepseek-acp` reads: its default provider/model come from
//!   `DEEPSEEK_ACP_PROVIDER` / `DEEPSEEK_ACP_MODEL`, not from
//!   `agent-default-model`.
//!
//! The transplant is textual rather than parse/re-emit on purpose: a patch entry
//! may carry `!!js` tags, block scalars and quoting we must not normalise, and
//! Marionette has no YAML writer (the tree depends on `serde_json` and `toml`).
//!
//! One limitation worth knowing: pi-ai serves a model only if it can describe
//! it, so a model id the installed catalog does not carry is dropped by the
//! harness — a route listing one must also name its `api` (and `baseURL`) for
//! that entry to become serviceable. That is DSH's rule, not Marionette's, so
//! this module mirrors the routes and does not police them.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The settings namespace that carries the user's routed providers.
const PI_AI_NS: &str = "llm-pi-ai";

/// The document the ACP adapter resolves as its settings provider.
const SETTINGS_FILE: &str = "settings.yaml";

/// First line of a document this module wrote. Later runs use it to tell their
/// own mirror from a hand-written `settings.yaml` — only the former may be
/// refreshed or replaced. ASCII only: the file is read by other tools too.
const OWNED_HEADER: &str =
    "# Written by Marionette - mirror of the DSH profile patch, for the ACP adapter.";

/// The `llm-pi-ai` body plus where it came from and when.
struct Route {
    /// Path relative to `$DSH_HOME`, for the log and the mirror's header.
    source: String,
    /// The `providers:` body, already re-indented to two spaces.
    section: String,
    /// Source mtime, so a mirror older than its profile is refreshed.
    modified: Option<SystemTime>,
}

impl Route {
    /// The document to write, header included.
    fn document(&self) -> String {
        format!(
            "{OWNED_HEADER}\n# Source: {} (DSH reads the profile; only the adapter reads this file.)\n{}:\n{}",
            self.source, PI_AI_NS, self.section
        )
    }
}

/// What the bridge did, so the caller can log it without re-deriving anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeAction {
    /// `<DSH_HOME>/settings.yaml` already carries an `llm-pi-ai` section.
    AlreadyPresent,
    /// The section was transplanted from `source`, a path relative to DSH home.
    Bridged { source: String },
    /// No profile patch or legacy document declares `llm-pi-ai`.
    NoRoutes,
    /// A non-empty `settings.yaml` exists without `llm-pi-ai`; left alone.
    ForeignDocument,
    /// No usable DSH home directory.
    NoHome,
    /// The document could not be read or written.
    Failed { detail: String },
}

impl BridgeAction {
    /// Whether this run wrote the document.
    pub fn changed(&self) -> bool {
        matches!(self, Self::Bridged { .. })
    }

    /// A short sentence for the developer log.
    pub fn describe(&self) -> String {
        match self {
            Self::AlreadyPresent => format!("{SETTINGS_FILE} already carries {PI_AI_NS}"),
            Self::Bridged { source } => {
                format!("bridged {PI_AI_NS} from {source} into {SETTINGS_FILE}")
            }
            Self::NoRoutes => format!(
                "no DSH profile patch declares {PI_AI_NS}; routed models stay hidden"
            ),
            Self::ForeignDocument => format!(
                "{SETTINGS_FILE} exists without {PI_AI_NS}; left untouched (routed models stay hidden)"
            ),
            Self::NoHome => "no DSH home directory; skipped the settings bridge".to_string(),
            Self::Failed { detail } => format!("settings bridge failed: {detail}"),
        }
    }
}

/// Ensure the ACP adapter can see the user's routed providers.
///
/// Idempotent, and safe to call before every DSH launch: the harness app renames
/// `<DSH_HOME>/settings.yaml` away whenever *it* boots, so this is what puts the
/// document back for the adapter.
pub fn ensure_settings_bridge() -> BridgeAction {
    let Some(home) = dsh_home() else {
        return BridgeAction::NoHome;
    };
    bridge_in(&home)
}

/// DSH's own resolution: a non-blank `$DSH_HOME` wins, then `~/.dsh`.
fn dsh_home() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("DSH_HOME") {
        let raw = raw.to_string_lossy();
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)?;
    Some(home.join(".dsh"))
}

fn bridge_in(home: &Path) -> BridgeAction {
    let target = home.join(SETTINGS_FILE);
    let existing = match fs::read_to_string(&target) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            return BridgeAction::Failed {
                detail: format!("cannot read {}: {error}", target.display()),
            };
        }
    };

    let route = route_section(home);

    // A document that is already there decides on its own, unless it is our own
    // mirror and the profile it mirrors has moved on since.
    if let Some(text) = existing.as_deref().filter(|text| !text.trim().is_empty()) {
        if !text.contains(PI_AI_NS) {
            return BridgeAction::ForeignDocument;
        }
        if !text.starts_with(OWNED_HEADER) {
            return BridgeAction::AlreadyPresent;
        }
        match &route {
            Some(route) if source_is_newer(route, &target) => {}
            _ => return BridgeAction::AlreadyPresent,
        }
    }

    let Some(route) = route else {
        return BridgeAction::NoRoutes;
    };
    if let Err(error) = write_atomic(&target, &route.document()) {
        return BridgeAction::Failed {
            detail: format!("cannot write {}: {error}", target.display()),
        };
    }
    BridgeAction::Bridged {
        source: route.source,
    }
}

/// Whether the profile a mirror was built from has been written since.
fn source_is_newer(route: &Route, target: &Path) -> bool {
    let Some(source) = route.modified else {
        return false;
    };
    let Ok(mirror) = fs::metadata(target).and_then(|meta| meta.modified()) else {
        return false;
    };
    source > mirror
}

/// The `llm-pi-ai` body to mirror: the active profile patch first, then the
/// pre-upgrade document as a fallback.
fn route_section(home: &Path) -> Option<Route> {
    for (path, profile) in profile_patches(home) {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(section) = patch_entry_section(&text, PI_AI_NS) {
            return Some(Route {
                source: format!("profiles/{profile}/cordis.patch.yml"),
                section,
                modified: fs::metadata(&path).and_then(|meta| meta.modified()).ok(),
            });
        }
    }

    let legacy = home.join("settings.yaml.imported");
    let text = fs::read_to_string(&legacy).ok()?;
    let section = top_level_section(&text, PI_AI_NS)?;
    Some(Route {
        source: "settings.yaml.imported".to_string(),
        section,
        modified: fs::metadata(&legacy).and_then(|meta| meta.modified()).ok(),
    })
}

/// Profile patch files, most recently modified first — the profile the user
/// actually edits is the one whose routes they mean.
fn profile_patches(home: &Path) -> Vec<(PathBuf, String)> {
    let mut found: Vec<(PathBuf, String, SystemTime)> = Vec::new();
    let Ok(entries) = fs::read_dir(home.join("profiles")) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let patch = entry.path().join("cordis.patch.yml");
        if !patch.is_file() {
            continue;
        }
        let modified = fs::metadata(&patch)
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        found.push((patch, name, modified));
    }
    found.sort_by(|a, b| b.2.cmp(&a.2));
    found
        .into_iter()
        .map(|(path, name, _)| (path, name))
        .collect()
}

/// Write `document` through a sibling temp file so a concurrent launch never
/// reads a half-written settings document.
fn write_atomic(target: &Path, document: &str) -> std::io::Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = target.with_extension("yaml.marionette-tmp");
    fs::write(&temp, document)?;
    match fs::rename(&temp, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// The body of the top-level `key:` map entry, re-indented to two spaces.
fn top_level_section(text: &str, key: &str) -> Option<String> {
    let lines = normalized_lines(text);
    for (index, line) in lines.iter().enumerate() {
        if indent_of(line) != 0 {
            continue;
        }
        // `?` would abandon the whole scan on the first unrelated key, and
        // `llm-pi-ai` is rarely the first section in these documents.
        let Some(after) = line.trim_start().strip_prefix(key) else {
            continue;
        };
        let Some(rest) = after.strip_prefix(':') else {
            continue;
        };
        if rest.trim().is_empty() {
            return reindent(&child_block(&lines, index, 0), 2);
        }
        // `llm-pi-ai: {…}` — a flow collection is already a complete value.
        let inline = rest.trim();
        if inline.starts_with('{') || inline.starts_with('[') {
            return Some(format!("  {inline}\n"));
        }
        return None;
    }
    None
}

/// The `config:` body of the patch entry whose `id:` is `key`, re-indented to
/// two spaces.
fn patch_entry_section(text: &str, key: &str) -> Option<String> {
    let lines = normalized_lines(text);
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        if indent_of(lines[index]) != 0 || !trimmed.starts_with("- ") {
            index += 1;
            continue;
        }
        // One list entry: the `- ` line plus every deeper or blank line after it.
        let mut end = index + 1;
        while end < lines.len()
            && (lines[end].trim().is_empty() || indent_of(lines[end]) > 0)
        {
            end += 1;
        }
        let entry = &lines[index..end];
        if entry_declares(entry, key) {
            if let Some(section) = entry_config(entry) {
                return Some(section);
            }
        }
        index = end;
    }
    None
}

/// Whether a patch entry's `id:` names `key` (quotes optional).
fn entry_declares(entry: &[&str], key: &str) -> bool {
    entry.iter().any(|line| {
        item_body(line).strip_prefix("id:").is_some_and(|value| {
            let value = value.trim();
            !value.is_empty() && value.trim_matches(['"', '\'']) == key
        })
    })
}

/// The `config:` body of one patch entry, re-indented to two spaces.
fn entry_config(entry: &[&str]) -> Option<String> {
    for (index, line) in entry.iter().enumerate() {
        let Some(rest) = item_body(line).strip_prefix("config:") else {
            continue;
        };
        if rest.trim().is_empty() {
            return reindent(&child_block(entry, index, key_indent(line)), 2);
        }
        let inline = rest.trim();
        if inline.starts_with('{') || inline.starts_with('[') {
            return Some(format!("  {inline}\n"));
        }
        return None;
    }
    None
}

/// A list item's mapping body: the line without its `- ` marker, so the first
/// key of `- id: llm-pi-ai` can be read like any other key.
fn item_body(line: &str) -> &str {
    let trimmed = line.trim_start();
    trimmed.strip_prefix("- ").unwrap_or(trimmed)
}

/// The column a line's *key* sits in. A `- ` marker occupies two columns, so the
/// first key of a list item is one level deeper than its own indentation.
fn key_indent(line: &str) -> usize {
    let marker = usize::from(line.trim_start().starts_with("- ")) * 2;
    indent_of(line) + marker
}

/// Lines owned by the block that starts on `header`: every following line that
/// is blank or indented deeper. Trailing blanks are dropped so the caller owns
/// the final newline.
fn child_block<'a>(lines: &[&'a str], header: usize, header_indent: usize) -> Vec<&'a str> {
    let mut out: Vec<&'a str> = Vec::new();
    for line in lines.iter().skip(header + 1) {
        if line.trim().is_empty() {
            out.push(*line);
            continue;
        }
        if indent_of(line) > header_indent {
            out.push(*line);
        } else {
            break;
        }
    }
    while out.last().is_some_and(|line| line.trim().is_empty()) {
        out.pop();
    }
    out
}

/// Re-indent a block so its shallowest line sits at `base`, keeping relative
/// depth. Content is otherwise preserved verbatim — trailing whitespace inside a
/// block scalar and all.
fn reindent(block: &[&str], base: usize) -> Option<String> {
    if block.is_empty() {
        return None;
    }
    let min = block
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| indent_of(line))
        .min()?;
    let mut out = String::new();
    for line in block {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }
        out.push_str(&" ".repeat(base + indent_of(line).saturating_sub(min)));
        out.push_str(line.trim_start());
        out.push('\n');
    }
    Some(out)
}

/// Lines with any CRLF `\r` dropped so indentation maths is exact. Trailing
/// spaces are deliberately kept: inside a block scalar they are content.
fn normalized_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE_PATCH: &str = r#"# Your patch layer for this dsh profile.
- id: ui-theme
  name: "@deepseek-ai/dsh-client-ui-theme"
  config:
    preference: light
- id: llm-pi-ai
  name: "@deepseek-ai/dsh-llm-pi-ai"
  config:
    providers:
      opencode-go:
        models:
          - id: deepseek-v4.1-flash
            name: DeepSeek V4.1 Flash
      xai:
        baseURL: !!js process.env.XAI_BASE ?? 'https://cli-chat-proxy.grok.com/v1'
        headers:
          x-grok-client-mode: headless
- id: ui-chat
  name: "@deepseek-ai/dsh-client-ui-chat"
  config:
    transcriptView: compact
"#;

    const LEGACY: &str = r#"ui-onboarding:
  welcomeNoticeVersion: 2026-08-13.1
agent-default-model:
  provider: deepseek-official
llm-pi-ai:
  providers:
    opencode-go:
      models:
        - id: deepseek-v4.1-flash
          name: DeepSeek V4.1 Flash
ui-theme:
  preference: light
"#;

    #[test]
    fn pulls_the_pi_ai_entry_out_of_a_patch_list() {
        let section = patch_entry_section(PROFILE_PATCH, PI_AI_NS).expect("section");
        assert_eq!(
            section,
            concat!(
                "  providers:\n",
                "    opencode-go:\n",
                "      models:\n",
                "        - id: deepseek-v4.1-flash\n",
                "          name: DeepSeek V4.1 Flash\n",
                "    xai:\n",
                "      baseURL: !!js process.env.XAI_BASE ?? 'https://cli-chat-proxy.grok.com/v1'\n",
                "      headers:\n",
                "        x-grok-client-mode: headless\n",
            )
        );
    }

    #[test]
    fn ignores_other_entries_and_stops_at_the_next_one() {
        let section = patch_entry_section(PROFILE_PATCH, PI_AI_NS).expect("section");
        assert!(!section.contains("transcriptView"));
        assert!(!section.contains("preference"));
    }

    #[test]
    fn a_patch_without_the_entry_yields_nothing() {
        let only_theme = "- id: ui-theme\n  config:\n    preference: light\n";
        assert_eq!(patch_entry_section(only_theme, PI_AI_NS), None);
    }

    #[test]
    fn a_quoted_id_still_matches() {
        let quoted = "- id: \"llm-pi-ai\"\n  config:\n    providers: {}\n";
        assert_eq!(
            patch_entry_section(quoted, PI_AI_NS).as_deref(),
            Some("  providers: {}\n")
        );
    }

    #[test]
    fn reads_the_legacy_document_section() {
        let section = top_level_section(LEGACY, PI_AI_NS).expect("section");
        assert_eq!(
            section,
            concat!(
                "  providers:\n",
                "    opencode-go:\n",
                "      models:\n",
                "        - id: deepseek-v4.1-flash\n",
                "          name: DeepSeek V4.1 Flash\n",
            )
        );
        assert!(!section.contains("ui-theme"));
        assert!(!section.contains("agent-default-model"));
    }

    #[test]
    fn a_blank_line_inside_a_block_is_kept() {
        let text = "llm-pi-ai:\n  providers:\n    a: 1\n\n    b: 2\nui-theme:\n  preference: light\n";
        let section = top_level_section(text, PI_AI_NS).expect("section");
        assert_eq!(section, "  providers:\n    a: 1\n\n    b: 2\n");
    }

    #[test]
    fn a_nested_key_is_not_mistaken_for_the_top_level_one() {
        let text = "other:\n  llm-pi-ai: nested\nllm-pi-ai:\n  providers: {}\n";
        assert_eq!(
            top_level_section(text, PI_AI_NS).as_deref(),
            Some("  providers: {}\n")
        );
    }

    #[test]
    fn crlf_sources_do_not_shift_indentation() {
        let text = "llm-pi-ai:\r\n  providers:\r\n    a: 1\r\nui-theme:\r\n  x: 2\r\n";
        assert_eq!(
            top_level_section(text, PI_AI_NS).as_deref(),
            Some("  providers:\n    a: 1\n")
        );
    }

    /// Trailing spaces are content inside a block scalar, so they must survive.
    #[test]
    fn trailing_spaces_in_a_block_scalar_are_preserved() {
        let text = "llm-pi-ai:\n  prompt: |\n    keep  \n    two spaces\nui-theme:\n  x: 2\n";
        let section = top_level_section(text, PI_AI_NS).expect("section");
        assert_eq!(section, "  prompt: |\n    keep  \n    two spaces\n");
    }

    /// The user's real shape: a stale `settings.yaml.imported` next to a profile
    /// patch that declares more routes. The profile patch must win.
    #[test]
    fn the_profile_patch_wins_over_the_stale_legacy_document() {
        let home = temp_dir("prefers-profile");
        fs::create_dir_all(home.join("profiles").join("web")).unwrap();
        fs::write(
            home.join("profiles").join("web").join("cordis.patch.yml"),
            PROFILE_PATCH,
        )
        .unwrap();
        fs::write(home.join(SETTINGS_FILE), "").unwrap();
        fs::write(home.join("settings.yaml.imported"), LEGACY).unwrap();

        assert_eq!(
            bridge_in(&home),
            BridgeAction::Bridged {
                source: "profiles/web/cordis.patch.yml".to_string()
            }
        );
        let written = fs::read_to_string(home.join(SETTINGS_FILE)).unwrap();
        assert!(written.starts_with(OWNED_HEADER), "missing ownership header");
        assert!(written.contains("\nllm-pi-ai:\n"));
        assert!(written.contains("x-grok-client-mode: headless"));
        // The stale document's single provider must not have been the source.
        assert_eq!(written.matches("opencode-go").count(), 1);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn falls_back_to_the_pre_upgrade_document() {
        let home = temp_dir("falls-back");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("settings.yaml.imported"), LEGACY).unwrap();

        assert_eq!(
            bridge_in(&home),
            BridgeAction::Bridged {
                source: "settings.yaml.imported".to_string()
            }
        );
        assert!(fs::read_to_string(home.join(SETTINGS_FILE))
            .unwrap()
            .contains("opencode-go"));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn an_already_bridged_document_is_never_rewritten() {
        let home = temp_dir("already-bridged");
        fs::create_dir_all(&home).unwrap();
        let mine = "# mine\nllm-pi-ai:\n  providers:\n    hand-written: {}\n";
        fs::write(home.join(SETTINGS_FILE), mine).unwrap();
        fs::write(home.join("settings.yaml.imported"), LEGACY).unwrap();

        assert_eq!(bridge_in(&home), BridgeAction::AlreadyPresent);
        assert_eq!(fs::read_to_string(home.join(SETTINGS_FILE)).unwrap(), mine);
        fs::remove_dir_all(&home).ok();
    }

    /// The mirror tracks the profile: a provider added after the mirror was
    /// written must reach the adapter without restarting the harness app.
    #[test]
    fn our_own_mirror_is_refreshed_when_the_profile_moves_on() {
        let home = temp_dir("refresh");
        fs::create_dir_all(home.join("profiles").join("web")).unwrap();
        let patch = home.join("profiles").join("web").join("cordis.patch.yml");
        fs::write(&patch, PROFILE_PATCH).unwrap();
        assert!(bridge_in(&home).changed());
        assert!(!fs::read_to_string(home.join(SETTINGS_FILE))
            .unwrap()
            .contains("newly-added"));

        // The user adds a model in the harness UI: the patch is rewritten later
        // than the mirror.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(
            &patch,
            PROFILE_PATCH.replace(
                "      xai:",
                "      newly-added:\n        models:\n          - id: fresh\n            name: Fresh\n            contextWindow: 1000\n            maxTokens: 100\n            input: [text]\n      xai:",
            ),
        )
        .unwrap();

        assert!(bridge_in(&home).changed(), "stale mirror was not refreshed");
        assert!(fs::read_to_string(home.join(SETTINGS_FILE))
            .unwrap()
            .contains("newly-added"));
        // A second run has nothing left to do.
        assert_eq!(bridge_in(&home), BridgeAction::AlreadyPresent);
        fs::remove_dir_all(&home).ok();
    }

    /// Only our own document is refreshed; a hand-written one is not ours to
    /// replace even when the profile is newer.
    #[test]
    fn a_hand_written_mirror_is_not_refreshed() {
        let home = temp_dir("not-ours");
        fs::create_dir_all(home.join("profiles").join("web")).unwrap();
        fs::write(
            home.join("profiles").join("web").join("cordis.patch.yml"),
            PROFILE_PATCH,
        )
        .unwrap();
        let mine = "# mine\nllm-pi-ai:\n  providers:\n    hand-written: {}\n";
        fs::write(home.join(SETTINGS_FILE), mine).unwrap();

        assert_eq!(bridge_in(&home), BridgeAction::AlreadyPresent);
        assert_eq!(fs::read_to_string(home.join(SETTINGS_FILE)).unwrap(), mine);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_foreign_document_is_left_alone_and_reported() {
        let home = temp_dir("foreign");
        fs::create_dir_all(&home).unwrap();
        let foreign = "ui-theme:\n  preference: dark\n";
        fs::write(home.join(SETTINGS_FILE), foreign).unwrap();
        fs::write(home.join("settings.yaml.imported"), LEGACY).unwrap();

        assert_eq!(bridge_in(&home), BridgeAction::ForeignDocument);
        assert_eq!(
            fs::read_to_string(home.join(SETTINGS_FILE)).unwrap(),
            foreign
        );
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn nothing_to_bridge_when_no_document_declares_the_namespace() {
        let home = temp_dir("no-routes");
        fs::create_dir_all(home.join("profiles").join("headless")).unwrap();
        fs::write(
            home.join("profiles").join("headless").join("cordis.patch.yml"),
            "[]\n",
        )
        .unwrap();

        assert_eq!(bridge_in(&home), BridgeAction::NoRoutes);
        assert!(!home.join(SETTINGS_FILE).exists());
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_blank_settings_file_counts_as_absent() {
        let home = temp_dir("blank");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join(SETTINGS_FILE), "\n  \n").unwrap();
        fs::write(home.join("settings.yaml.imported"), LEGACY).unwrap();

        assert!(bridge_in(&home).changed());
        fs::remove_dir_all(&home).ok();
    }

    /// A sibling temp file must never be left behind on the happy path.
    #[test]
    fn the_temp_file_is_renamed_away() {
        let home = temp_dir("atomic");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("settings.yaml.imported"), LEGACY).unwrap();
        bridge_in(&home);

        let leftovers: Vec<_> = fs::read_dir(&home)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains("marionette-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
        fs::remove_dir_all(&home).ok();
    }

    /// Only the one namespace is written: a whole-document copy would drag the
    /// retired `ui-*` sections back into the adapter's settings.
    #[test]
    fn the_bridged_document_carries_only_the_one_namespace() {
        let home = temp_dir("one-namespace");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("settings.yaml.imported"), LEGACY).unwrap();
        bridge_in(&home);

        let written = fs::read_to_string(home.join(SETTINGS_FILE)).unwrap();
        let keys: Vec<&str> = written
            .lines()
            .filter(|line| indent_of(line) == 0 && !line.starts_with('#'))
            .collect();
        assert_eq!(keys, vec!["llm-pi-ai:"], "unexpected document:\n{written}");
        assert!(!written.contains("ui-theme"));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn an_unreadable_document_reports_a_failure_rather_than_a_missing_route() {
        // A directory where the document should be: the read fails with
        // something other than NotFound, so this must not be mistaken for
        // "nothing configured" and silently skipped.
        let home = temp_dir("unreadable");
        fs::create_dir_all(home.join(SETTINGS_FILE)).unwrap();
        let action = bridge_in(&home);
        assert!(
            matches!(action, BridgeAction::Failed { .. }),
            "unexpected: {action:?}"
        );
        fs::remove_dir_all(&home).ok();
    }

    /// A unique scratch directory; the tree's other tests keep no temp helper.
    fn temp_dir(label: &str) -> PathBuf {        let unique = format!(
            "marionette-dsh-bridge-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        std::env::temp_dir().join(unique)
    }
}
