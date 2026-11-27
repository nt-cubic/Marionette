//! User-owned project actions (P1-14).
//!
//! Setup / dev / test / cleanup commands live in Marionette-owned project
//! metadata:
//!
//! ```text
//! <project>/.marionette/project-actions.json
//! ```
//!
//! `.marionette` is git-ignored, so a checked-in repository can never supply
//! these commands. Nothing in this module reads `package.json`, a Makefile, or
//! any other repository script catalogue: only the user's saved entries run.
//!
//! Execution goes through the shared ACP terminal host (the same host agents
//! use for `terminal/create`). That gives the existing output and stop
//! surface for free: `list_acp_terminals`, `list_acp_terminal_outputs`, and
//! `kill_acp_terminal` all see the process. The terminal is owned by a
//! synthetic id (`project-action:<projectId>`), never by an agent session, so
//! no permission mode, always-approve setting, or ACP session config is
//! inherited.
//!
//! The first run of a command requires an explicit confirmation of the exact
//! string. Editing the command invalidates that confirmation. Confirmations
//! are stored per machine in
//! `~/.marionette/project-action-confirmations.json`, **not** in the project
//! tree: a checked-in `.marionette/project-actions.json` can add actions but
//! can never arrive already confirmed.
//!
//! Setup entries may opt in to run after worktree creation; relative cwd
//! values are re-based onto the new worktree, and an unconfirmed command is
//! never run automatically.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::acp::AcpService;
use crate::app_paths;
use crate::AppState;

pub const ACTION_KINDS: [&str; 4] = ["setup", "dev", "test", "cleanup"];

const STORE_FILE: &str = "project-actions.json";
/// Machine-local first-run confirmations. Kept outside the project tree so a
/// checked-in `.marionette/project-actions.json` can never carry a forged
/// "already confirmed" flag into another machine.
const CONFIRM_FILE: &str = "project-action-confirmations.json";
const STORE_VERSION: u32 = 1;
const MAX_ACTIONS: usize = 32;
const MAX_ID: usize = 64;
const MAX_LABEL: usize = 80;
const MAX_COMMAND: usize = 4_000;
const MAX_CWD: usize = 260;
const OUTPUT_TAIL_BYTES: usize = 32 * 1024;
const DEFAULT_WAIT_MS: u64 = 120_000;
const MAX_WAIT_MS: u64 = 600_000;
const WAIT_POLL_MS: u64 = 300;

static STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn store_lock() -> &'static Mutex<()> {
    STORE_LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAction {
    pub id: String,
    /// `setup` / `dev` / `test` / `cleanup`.
    pub kind: String,
    pub label: String,
    /// The exact command line the user typed. It is executed by the platform
    /// shell (`cmd.exe /C` on Windows, `/bin/sh -c` elsewhere).
    pub command: String,
    /// Optional working directory relative to the project root. Absolute
    /// paths and `..` components are rejected so this value also works in a
    /// worktree checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The exact command string the user confirmed before the first run.
    /// `None`, or a different string, means the UI must re-confirm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_command: Option<String>,
    /// Setup only: opt in to running after a new worktree is created. The
    /// command must already have been confirmed manually once.
    #[serde(default)]
    pub run_after_worktree: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionStore {
    #[serde(default = "store_version")]
    version: u32,
    #[serde(default)]
    actions: Vec<ProjectAction>,
}

fn store_version() -> u32 {
    STORE_VERSION
}

/// One first-run acknowledgement. The cwd is part of the fingerprint: changing
/// where a command runs changes its effect, so it must be confirmed again.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredConfirmation {
    #[serde(default)]
    command: String,
    #[serde(default)]
    cwd: Option<String>,
}

/// `~/.marionette/project-action-confirmations.json`: canonical project root
/// -> action id -> this machine's confirmed command/cwd fingerprint.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmationStore {
    #[serde(default = "store_version")]
    version: u32,
    #[serde(default)]
    projects: HashMap<String, HashMap<String, StoredConfirmation>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectActionRunStatus {
    ConfirmationRequired,
    Started,
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectActionRunResult {
    pub status: ProjectActionRunStatus,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectActionStarted {
    pub action_id: String,
    pub label: String,
    pub terminal_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeSetupFailure {
    pub action_id: String,
    pub label: String,
    pub error: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeSetupReport {
    pub started: Vec<ProjectActionStarted>,
    pub failed: Vec<WorktreeSetupFailure>,
    /// `true` only when the caller explicitly asked to wait for setup.
    pub waited: bool,
    pub ok: bool,
}

impl ProjectActionRunResult {
    fn confirmation_required(command: String) -> Self {
        Self {
            status: ProjectActionRunStatus::ConfirmationRequired,
            command,
            terminal_id: None,
            exit_code: None,
            signal: None,
            output: None,
            truncated: false,
            error: None,
        }
    }

    fn started(command: String, terminal_id: String) -> Self {
        Self {
            status: ProjectActionRunStatus::Started,
            command,
            terminal_id: Some(terminal_id),
            exit_code: None,
            signal: None,
            output: None,
            truncated: false,
            error: None,
        }
    }

    fn finished(command: String, terminal_id: String, outcome: TerminalOutcome) -> Self {
        let status = if !outcome.exited {
            ProjectActionRunStatus::Running
        } else if outcome.exit_code == Some(0) && outcome.signal.is_none() {
            ProjectActionRunStatus::Completed
        } else {
            ProjectActionRunStatus::Failed
        };
        Self {
            status,
            command,
            terminal_id: Some(terminal_id),
            exit_code: outcome.exit_code,
            signal: outcome.signal,
            output: Some(outcome.output),
            truncated: outcome.truncated,
            error: None,
        }
    }
}

struct TerminalOutcome {
    exit_code: Option<i32>,
    signal: Option<String>,
    output: String,
    truncated: bool,
    exited: bool,
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

fn store_path(project_root: &Path) -> PathBuf {
    app_paths::project_dir(project_root).join(STORE_FILE)
}

fn confirmation_path() -> Result<PathBuf, String> {
    Ok(app_paths::global_dir()?.join(CONFIRM_FILE))
}

fn canonical_project_key(project_root: &Path) -> Result<String, String> {
    let canonical = fs::canonicalize(project_root)
        .map_err(|error| format!("project directory is not accessible: {error}"))?;
    Ok(app_paths::plain_path(&canonical.to_string_lossy()))
}

fn write_json_atomic(path: &Path, json: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|error| format!("create directory {} failed: {error}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, json).map_err(|error| format!("write {} failed: {error}", path.display()))?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(first) => {
            // A reader holding the old file can make the replace fail on
            // Windows. Remove + rename is a narrow, honest fallback; if it
            // also fails, report both errors and leave the temp file for
            // inspection rather than pretending the save worked.
            if path.exists() {
                fs::remove_file(path).map_err(|remove_error| {
                    format!(
                        "replace {} failed: {first}; remove old file failed: {remove_error}",
                        path.display()
                    )
                })?;
                fs::rename(&tmp, path).map_err(|rename_error| {
                    format!(
                        "replace {} failed: {first}; second rename failed: {rename_error}",
                        path.display()
                    )
                })
            } else {
                Err(format!("replace {} failed: {first}", path.display()))
            }
        }
    }
}

/// Read the project action list without taking [`STORE_LOCK`]. Callers that
/// already hold the lock must use this to avoid a re-entrant deadlock. The
/// returned actions never carry a confirmation: that lives in
/// [`ConfirmationStore`].
fn read_actions_unlocked(project_root: &Path) -> Result<Vec<ProjectAction>, String> {
    let path = store_path(project_root);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read project actions failed: {error}")),
    };
    match serde_json::from_str::<ActionStore>(&raw) {
        Ok(store) => Ok(store
            .actions
            .into_iter()
            .take(MAX_ACTIONS)
            .map(|mut action| {
                action.confirmed_command = None;
                action
            })
            .collect()),
        Err(error) => {
            // Keep the broken file instead of silently replacing it on the
            // next save. The user can inspect or restore it.
            let corrupt = path.with_extension(format!("json.corrupt-{}", now_millis()));
            let moved = fs::rename(&path, &corrupt);
            crate::debug_log::append(
                "project-actions",
                "warn",
                "",
                &match &moved {
                    Ok(()) => format!(
                        "project-actions.json unreadable; moved to {}",
                        corrupt.display()
                    ),
                    Err(rename_error) => format!(
                        "project-actions.json unreadable; could not move it aside: {rename_error}"
                    ),
                },
                Some(&error.to_string()),
            );
            Ok(Vec::new())
        }
    }
}

fn read_confirmations_unlocked(path: &Path) -> ConfirmationStore {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ConfirmationStore::default();
        }
        Err(error) => {
            crate::debug_log::append(
                "project-actions",
                "warn",
                "",
                "read project action confirmations failed; treating every action as unconfirmed",
                Some(&error.to_string()),
            );
            return ConfirmationStore::default();
        }
    };
    match serde_json::from_str::<ConfirmationStore>(&raw) {
        Ok(store) => store,
        Err(error) => {
            let corrupt = path.with_extension(format!("json.corrupt-{}", now_millis()));
            let _ = fs::rename(path, &corrupt);
            crate::debug_log::append(
                "project-actions",
                "warn",
                "",
                &format!(
                    "project action confirmations unreadable; moved to {}",
                    corrupt.display()
                ),
                Some(&error.to_string()),
            );
            ConfirmationStore::default()
        }
    }
}

fn write_actions(project_root: &Path, actions: &[ProjectAction]) -> Result<(), String> {
    let dir = app_paths::project_dir(project_root);
    let path = dir.join(STORE_FILE);
    let store = ActionStore {
        version: STORE_VERSION,
        actions: actions
            .iter()
            .cloned()
            .map(|mut action| {
                // Confirmations are machine-local; never persist one in the
                // project tree.
                action.confirmed_command = None;
                action
            })
            .collect(),
    };
    let json = serde_json::to_string_pretty(&store)
        .map_err(|error| format!("serialize project actions failed: {error}"))?;
    write_json_atomic(&path, &json)
}

fn write_confirmations(path: &Path, store: &ConfirmationStore) -> Result<(), String> {
    let json = serde_json::to_string_pretty(store)
        .map_err(|error| format!("serialize project action confirmations failed: {error}"))?;
    write_json_atomic(path, &json)
}

fn load_at_with(
    project_root: &Path,
    confirmation_path: &Path,
) -> Result<Vec<ProjectAction>, String> {
    let mut actions = read_actions_unlocked(project_root)?;
    let key = canonical_project_key(project_root)?;
    let confirmations = read_confirmations_unlocked(confirmation_path);
    let by_id = confirmations.projects.get(&key);
    for action in &mut actions {
        action.confirmed_command = by_id
            .and_then(|map| map.get(&action.id))
            .filter(|confirmed| {
                confirmed.command.as_str() == action.command.trim()
                    && confirmed.cwd.as_deref() == action.cwd.as_deref()
            })
            .map(|confirmed| confirmed.command.clone());
    }
    Ok(actions)
}

pub fn load_at(project_root: &Path) -> Result<Vec<ProjectAction>, String> {
    let path = confirmation_path()?;
    let _guard = store_lock()
        .lock()
        .map_err(|_| "project action store lock poisoned".to_string())?;
    load_at_with(project_root, &path)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn validate_cwd_syntax(raw: Option<&str>) -> Result<(), String> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    if raw.len() > MAX_CWD {
        return Err(format!("action cwd is longer than {MAX_CWD} characters"));
    }
    if raw.contains('\0') {
        return Err("action cwd contains a NUL byte".to_string());
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err("action cwd must be a path relative to the project root".to_string());
    }
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err("action cwd cannot contain `..`".to_string());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("action cwd must be a path relative to the project root".to_string());
            }
            _ => {}
        }
    }
    Ok(())
}

fn normalize_cwd(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    validate_cwd_syntax(Some(raw))?;
    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::CurDir => {}
            _ => return Err("action cwd must be a path relative to the project root".to_string()),
        }
    }
    if parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parts.join("/")))
    }
}

fn validate_actions(actions: Vec<ProjectAction>) -> Result<Vec<ProjectAction>, String> {
    if actions.len() > MAX_ACTIONS {
        return Err(format!("at most {MAX_ACTIONS} project actions are allowed"));
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(actions.len());
    for mut action in actions {
        action.id = action.id.trim().to_string();
        if !valid_id(&action.id) {
            return Err(
                "action id must be 1-64 letters, digits, `-` or `_` characters".to_string(),
            );
        }
        if !seen.insert(action.id.clone()) {
            return Err(format!("duplicate action id: {}", action.id));
        }
        action.kind = action.kind.trim().to_ascii_lowercase();
        if !ACTION_KINDS.contains(&action.kind.as_str()) {
            return Err(format!("unknown action kind: {}", action.kind));
        }
        action.label = action.label.trim().to_string();
        if action.label.is_empty() {
            return Err("action label cannot be empty".to_string());
        }
        if action.label.chars().count() > MAX_LABEL {
            return Err(format!(
                "action label is longer than {MAX_LABEL} characters"
            ));
        }
        action.command = action.command.trim().to_string();
        if action.command.is_empty() {
            return Err("action command cannot be empty".to_string());
        }
        if action.command.len() > MAX_COMMAND {
            return Err(format!("action command is longer than {MAX_COMMAND} bytes"));
        }
        if action.command.contains('\0') {
            return Err("action command contains a NUL byte".to_string());
        }
        action.cwd = normalize_cwd(action.cwd.as_deref())?;
        if action.kind != "setup" {
            action.run_after_worktree = false;
        }
        // Client-supplied confirmation is never trusted; `save_at` copies the
        // stored confirmation only when the command is unchanged.
        action.confirmed_command = None;
        out.push(action);
    }
    Ok(out)
}

fn save_at_with(
    project_root: &Path,
    confirmation_path: &Path,
    actions: Vec<ProjectAction>,
) -> Result<Vec<ProjectAction>, String> {
    if !project_root.is_dir() {
        return Err("project directory is not accessible".to_string());
    }
    let _guard = store_lock()
        .lock()
        .map_err(|_| "project action store lock poisoned".to_string())?;
    let key = canonical_project_key(project_root)?;
    let normalized = validate_actions(actions)?;
    let mut confirmations = read_confirmations_unlocked(confirmation_path);
    let mut returned = normalized.clone();
    {
        let project_confirmations = confirmations.projects.entry(key).or_default();
        let current: HashMap<&str, (&str, Option<&str>)> = normalized
            .iter()
            .map(|action| {
                (
                    action.id.as_str(),
                    (action.command.as_str(), action.cwd.as_deref()),
                )
            })
            .collect();
        // Drop confirmations for deleted actions and for commands/cwds the
        // user edited. They can never be reused by a later action with the
        // same id.
        project_confirmations.retain(|id, confirmation| match current.get(id.as_str()) {
            Some((command, cwd)) => {
                *command == confirmation.command.as_str() && *cwd == confirmation.cwd.as_deref()
            }
            None => false,
        });
        for action in &mut returned {
            action.confirmed_command = project_confirmations
                .get(&action.id)
                .filter(|confirmation| {
                    confirmation.command == action.command
                        && confirmation.cwd.as_deref() == action.cwd.as_deref()
                })
                .map(|confirmation| confirmation.command.clone());
        }
    }
    // Confirmations are machine-local and fail closed: write them before the
    // project list so a failed action write cannot leave a new command
    // pre-confirmed.
    write_confirmations(confirmation_path, &confirmations)?;
    write_actions(project_root, &normalized)?;
    Ok(returned)
}

pub fn save_at(
    project_root: &Path,
    actions: Vec<ProjectAction>,
) -> Result<Vec<ProjectAction>, String> {
    let path = confirmation_path()?;
    save_at_with(project_root, &path, actions)
}

fn mark_confirmed_with(
    project_root: &Path,
    confirmation_path: &Path,
    action_id: &str,
    command: &str,
) -> Result<ProjectAction, String> {
    let _guard = store_lock()
        .lock()
        .map_err(|_| "project action store lock poisoned".to_string())?;
    let key = canonical_project_key(project_root)?;
    let mut actions = read_actions_unlocked(project_root)?;
    let action = actions
        .iter_mut()
        .find(|action| action.id == action_id)
        .ok_or_else(|| format!("unknown project action: {action_id}"))?;
    let confirmed_command = action.command.trim().to_string();
    if confirmed_command.as_str() != command {
        return Err("action command changed; confirm the new command first".to_string());
    }
    let cwd = action.cwd.clone();
    let mut confirmations = read_confirmations_unlocked(confirmation_path);
    confirmations.projects.entry(key).or_default().insert(
        action_id.to_string(),
        StoredConfirmation {
            command: confirmed_command.clone(),
            cwd,
        },
    );
    write_confirmations(confirmation_path, &confirmations)?;
    action.confirmed_command = Some(confirmed_command);
    Ok(action.clone())
}

fn mark_confirmed(
    project_root: &Path,
    action_id: &str,
    command: &str,
) -> Result<ProjectAction, String> {
    let path = confirmation_path()?;
    mark_confirmed_with(project_root, &path, action_id, command)
}

fn validate_action_for_run(base: &Path, action: &ProjectAction) -> Result<(), String> {
    validate_cwd_syntax(action.cwd.as_deref())?;
    if !ACTION_KINDS.contains(&action.kind.as_str()) {
        return Err(format!("unknown action kind: {}", action.kind));
    }
    if action.command.trim().is_empty() {
        return Err("action command cannot be empty".to_string());
    }
    if action.command.len() > MAX_COMMAND {
        return Err(format!("action command is longer than {MAX_COMMAND} bytes"));
    }
    if action.command.contains('\0') {
        return Err("action command contains a NUL byte".to_string());
    }
    // Resolve once now so a broken cwd fails before a terminal is created.
    resolve_run_cwd(base, action.cwd.as_deref())?;
    Ok(())
}

fn resolve_run_cwd(base: &Path, cwd: Option<&str>) -> Result<PathBuf, String> {
    let base = fs::canonicalize(base)
        .map_err(|error| format!("project directory is not accessible: {error}"))?;
    let Some(raw) = cwd.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(base);
    };
    validate_cwd_syntax(Some(raw))?;
    let joined = base.join(raw);
    let canonical = fs::canonicalize(&joined)
        .map_err(|error| format!("action working directory `{raw}` is not accessible: {error}"))?;
    if !canonical.starts_with(&base) {
        return Err("action working directory leaves the project root".to_string());
    }
    if !canonical.is_dir() {
        return Err(format!(
            "action working directory `{raw}` is not a directory"
        ));
    }
    Ok(canonical)
}

/// The platform shell used for a user-authored command line. Keeping this in
/// one place makes the exact execution shape testable and leaves the terminal
/// host's own PATH resolution untouched.
pub fn shell_invocation(command: &str) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), command.to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        (
            "/bin/sh".to_string(),
            vec!["-c".to_string(), command.to_string()],
        )
    }
}

fn spawn_action(
    acp: &AcpService,
    owner: &str,
    base_dir: &Path,
    action: &ProjectAction,
) -> Result<String, String> {
    let cwd = resolve_run_cwd(base_dir, action.cwd.as_deref())?;
    let (program, args) = shell_invocation(&action.command);
    let cwd = app_paths::plain_path(&cwd.to_string_lossy());
    acp.create_user_terminal(owner, &cwd, &program, &args)
}

fn tail_text(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_string(), false);
    }
    let mut start = text.len() - limit;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_string(), true)
}

fn outcome_from_snapshot(snapshot: Value) -> TerminalOutcome {
    let raw = snapshot
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (output, truncated) = tail_text(raw, OUTPUT_TAIL_BYTES);
    let status = snapshot.get("exitStatus");
    TerminalOutcome {
        exit_code: status
            .and_then(|status| status.get("exitCode"))
            .and_then(Value::as_i64)
            .map(|code| code as i32),
        signal: status
            .and_then(|status| status.get("signal"))
            .and_then(Value::as_str)
            .map(str::to_string),
        output,
        truncated,
        exited: status.is_some(),
    }
}

fn wait_terminal(
    acp: &AcpService,
    terminal_id: &str,
    wait_for_ms: u64,
) -> Result<TerminalOutcome, String> {
    let deadline = Instant::now() + Duration::from_millis(wait_for_ms);
    loop {
        // `list_terminals` only carries a ~2 KB tail, so polling it while the
        // setup runs does not copy the full (up to 1 MB) snapshot every tick.
        let running = acp
            .list_terminals()
            .into_iter()
            .find(|row| row.get("terminalId").and_then(Value::as_str) == Some(terminal_id))
            .ok_or_else(|| format!("terminal {terminal_id} is no longer available"))?
            .get("running")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !running || Instant::now() >= deadline {
            return Ok(outcome_from_snapshot(
                acp.user_terminal_snapshot(terminal_id)?,
            ));
        }
        std::thread::sleep(Duration::from_millis(WAIT_POLL_MS));
    }
}

fn project_root_of(project_id: &str, state: &State<'_, AppState>) -> Result<PathBuf, String> {
    if project_id.trim().is_empty() || project_id == app_paths::CHAT_PROJECT_ID {
        return Err("project actions are only available for a real project".to_string());
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    let project = storage
        .list_projects()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| format!("Unknown project: {project_id}"))?;
    Ok(PathBuf::from(project.root_path))
}

#[tauri::command(async)]
pub fn project_actions_list(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<ProjectAction>, String> {
    let root = project_root_of(&project_id, &state)?;
    load_at(&root)
}

#[tauri::command(async)]
pub fn project_actions_save(
    project_id: String,
    actions: Vec<ProjectAction>,
    state: State<'_, AppState>,
) -> Result<Vec<ProjectAction>, String> {
    let root = project_root_of(&project_id, &state)?;
    save_at(&root, actions)
}

/// Run one saved action. This never touches an agent session: the terminal is
/// owned by a synthetic project id, so the agent's permission mode and
/// always-approve preference cannot widen it.
///
/// `confirm = true` records the first-run acknowledgement for the exact
/// command. Without it, an unconfirmed command returns
/// `confirmation_required` and does not spawn anything.
#[tauri::command(async)]
pub fn project_actions_run(
    project_id: String,
    action_id: String,
    confirm: Option<bool>,
    wait: Option<bool>,
    wait_for_ms: Option<u64>,
    state: State<'_, AppState>,
) -> Result<ProjectActionRunResult, String> {
    let root = project_root_of(&project_id, &state)?;
    let actions = load_at(&root)?;
    let mut action = actions
        .into_iter()
        .find(|action| action.id == action_id)
        .ok_or_else(|| format!("unknown project action: {action_id}"))?;
    validate_action_for_run(&root, &action)?;

    let command = action.command.trim().to_string();
    action.command = command.clone();
    let needs_confirmation = action.confirmed_command.as_deref() != Some(command.as_str());
    if needs_confirmation {
        if !confirm.unwrap_or(false) {
            return Ok(ProjectActionRunResult::confirmation_required(command));
        }
        action = mark_confirmed(&root, &action_id, &command)?;
    }

    let owner = format!("project-action:{project_id}");
    let terminal_id = spawn_action(&state.acp, &owner, &root, &action)?;
    if !wait.unwrap_or(false) {
        return Ok(ProjectActionRunResult::started(command, terminal_id));
    }
    let budget = wait_for_ms
        .unwrap_or(DEFAULT_WAIT_MS)
        .clamp(1_000, MAX_WAIT_MS);
    let outcome = wait_terminal(&state.acp, &terminal_id, budget)?;
    Ok(ProjectActionRunResult::finished(
        command,
        terminal_id,
        outcome,
    ))
}

/// Worktree hook. A future worktree creator calls this after the checkout
/// exists; no product path calls it yet.
///
/// Only setup actions with `runAfterWorktree` run, and only after their exact
/// command was confirmed manually once. Relative `cwd` values are resolved
/// against `worktree_root`, while the action list itself is read from the
/// original project metadata (`.marionette` is not checked into the worktree).
///
/// The hook never panics and never returns `Err` for a failing action: it
/// reports every failed entry. The caller decides whether `wait` should block
/// opening the session. Returning before setup finishes is the default.
pub fn run_setup_after_worktree(
    acp: &AcpService,
    project_id: &str,
    project_root: &Path,
    worktree_root: &Path,
    wait_for_ms: Option<u64>,
) -> WorktreeSetupReport {
    let actions = match load_at(project_root) {
        Ok(actions) => actions,
        Err(error) => {
            return WorktreeSetupReport {
                started: Vec::new(),
                failed: vec![WorktreeSetupFailure {
                    action_id: "store".to_string(),
                    label: "project actions".to_string(),
                    error,
                }],
                waited: wait_for_ms.is_some(),
                ok: false,
            };
        }
    };

    let owner = format!("project-action:{project_id}:worktree");
    let mut started: Vec<ProjectActionStarted> = Vec::new();
    let mut failed: Vec<WorktreeSetupFailure> = Vec::new();

    for action in actions
        .into_iter()
        .filter(|action| action.kind == "setup" && action.run_after_worktree)
    {
        let confirmed = action.confirmed_command.as_deref().map(str::trim);
        if confirmed != Some(action.command.trim()) {
            failed.push(WorktreeSetupFailure {
                action_id: action.id,
                label: action.label,
                error: "needs first manual confirmation".to_string(),
            });
            continue;
        }
        if let Err(error) = validate_action_for_run(worktree_root, &action) {
            failed.push(WorktreeSetupFailure {
                action_id: action.id,
                label: action.label,
                error,
            });
            continue;
        }
        match spawn_action(acp, &owner, worktree_root, &action) {
            Ok(terminal_id) => started.push(ProjectActionStarted {
                action_id: action.id,
                label: action.label,
                terminal_id,
            }),
            Err(error) => failed.push(WorktreeSetupFailure {
                action_id: action.id,
                label: action.label,
                error,
            }),
        }
    }

    if let Some(budget) = wait_for_ms {
        let budget = budget.clamp(1_000, MAX_WAIT_MS);
        for entry in &started {
            match wait_terminal(acp, &entry.terminal_id, budget) {
                Ok(outcome) if outcome.exited && outcome.exit_code == Some(0) => {}
                Ok(outcome) => failed.push(WorktreeSetupFailure {
                    action_id: entry.action_id.clone(),
                    label: entry.label.clone(),
                    error: if outcome.exited {
                        match (&outcome.signal, outcome.exit_code) {
                            (Some(signal), _) => format!("setup stopped ({signal})"),
                            (None, Some(code)) => format!("setup exited with code {code}"),
                            (None, None) => "setup failed".to_string(),
                        }
                    } else {
                        "setup is still running after the wait timeout".to_string()
                    },
                }),
                Err(error) => failed.push(WorktreeSetupFailure {
                    action_id: entry.action_id.clone(),
                    label: entry.label.clone(),
                    error,
                }),
            }
        }
    }

    let ok = failed.is_empty();
    WorktreeSetupReport {
        started,
        failed,
        waited: wait_for_ms.is_some(),
        ok,
    }
}

/// Tauri hook entry point for a future worktree creator. The caller passes an
/// existing absolute directory; the action list still comes from the original
/// project's `.marionette` store.
#[tauri::command(async)]
pub fn project_actions_run_worktree_setup(
    project_id: String,
    worktree_root: String,
    wait: Option<bool>,
    wait_for_ms: Option<u64>,
    state: State<'_, AppState>,
) -> Result<WorktreeSetupReport, String> {
    let project_root = project_root_of(&project_id, &state)?;
    let worktree = PathBuf::from(worktree_root.trim());
    if !worktree.is_absolute() {
        return Err("worktree root must be an absolute path".to_string());
    }
    let worktree = fs::canonicalize(&worktree)
        .map_err(|error| format!("worktree root is not accessible: {error}"))?;
    if !worktree.is_dir() {
        return Err("worktree root must be a directory".to_string());
    }
    let budget = if wait.unwrap_or(false) {
        Some(wait_for_ms.unwrap_or(DEFAULT_WAIT_MS))
    } else {
        None
    };
    Ok(run_setup_after_worktree(
        &state.acp,
        &project_id,
        &project_root,
        &worktree,
        budget,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("marionette-project-actions-{tag}-{}", now_millis()));
        fs::create_dir_all(&dir).expect("create temp project");
        dir
    }

    fn action(id: &str, kind: &str, command: &str) -> ProjectAction {
        ProjectAction {
            id: id.to_string(),
            kind: kind.to_string(),
            label: format!("label {id}"),
            command: command.to_string(),
            cwd: None,
            confirmed_command: None,
            run_after_worktree: false,
        }
    }

    #[test]
    fn save_round_trip_and_command_change_invalidates_confirmation() {
        let root = temp_project("roundtrip");
        let confirm = root.join("project-action-confirmations.json");
        let saved =
            save_at_with(&root, &confirm, vec![action("a1", "dev", "npm run dev")]).unwrap();
        assert_eq!(saved[0].confirmed_command, None);

        mark_confirmed_with(&root, &confirm, "a1", "npm run dev").unwrap();
        let loaded = load_at_with(&root, &confirm).unwrap();
        assert_eq!(loaded[0].confirmed_command.as_deref(), Some("npm run dev"));
        let project_file = app_paths::project_dir(&root).join(STORE_FILE);
        let raw = fs::read_to_string(&project_file).unwrap();
        assert!(!raw.contains("confirmedCommand"));
        assert!(!raw.contains("confirmed_command"));

        // A client cannot smuggle a confirmation for a new command.
        let mut edited = loaded[0].clone();
        edited.command = "npm test".to_string();
        edited.confirmed_command = Some("npm run dev".to_string());
        let saved = save_at_with(&root, &confirm, vec![edited]).unwrap();
        assert_eq!(saved[0].confirmed_command, None);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn save_rejects_absolute_and_parent_cwds() {
        let root = temp_project("cwd");
        let mut absolute = action("a1", "dev", "npm run dev");
        absolute.cwd = Some(root.to_string_lossy().to_string());
        assert!(save_at_with(&root, &root.join("confirmations.json"), vec![absolute]).is_err());

        let mut parent = action("a2", "dev", "npm run dev");
        parent.cwd = Some("../outside".to_string());
        assert!(save_at_with(&root, &root.join("confirmations.json"), vec![parent]).is_err());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn only_setup_actions_can_run_after_worktree() {
        let root = temp_project("worktree-flag");
        let mut dev = action("a1", "dev", "npm run dev");
        dev.run_after_worktree = true;
        let saved = save_at_with(&root, &root.join("confirmations.json"), vec![dev]).unwrap();
        assert!(!saved[0].run_after_worktree);

        let mut setup = action("a2", "setup", "npm install");
        setup.run_after_worktree = true;
        let saved = save_at_with(&root, &root.join("confirmations.json"), vec![setup]).unwrap();
        assert!(saved[0].run_after_worktree);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn shell_invocation_uses_the_platform_shell() {
        #[cfg(windows)]
        {
            let (program, args) = shell_invocation("npm run dev");
            assert_eq!(program, "cmd.exe");
            assert_eq!(args, vec!["/C", "npm run dev"]);
        }
        #[cfg(not(windows))]
        {
            let (program, args) = shell_invocation("npm run dev");
            assert_eq!(program, "/bin/sh");
            assert_eq!(args, vec!["-c", "npm run dev"]);
        }
    }

    #[test]
    fn corrupt_store_is_moved_aside_instead_of_overwritten() {
        let root = temp_project("corrupt");
        let dir = app_paths::project_dir(&root);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(STORE_FILE), "{not json").unwrap();
        assert!(load_at_with(&root, &root.join("confirmations.json"))
            .unwrap()
            .is_empty());
        assert!(!dir.join(STORE_FILE).exists());
        let moved = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains("corrupt"));
        assert!(moved);
        let _ = fs::remove_dir_all(&root);
    }
}
