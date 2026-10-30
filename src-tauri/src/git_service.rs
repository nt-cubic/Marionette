use crate::models::ChangedFile;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// Current branch name for the project root.
/// `None` if not a git repo / no HEAD. Detached HEAD returns `HEAD (abcdef)`.
pub fn get_current_branch(project_root: &Path) -> Result<Option<String>, String> {
    if !project_root.is_dir() {
        return Err(format!("Not a directory: {}", project_root.display()));
    }

    let output = run_git(project_root, &["branch", "--show-current"])?;
    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if code == 128 || stderr.to_lowercase().contains("not a git repository") {
        return Ok(None);
    }
    if code != 0 && stdout.is_empty() {
        if stderr.to_lowercase().contains("not recognized")
            || stderr.to_lowercase().contains("not found")
            || stderr.to_lowercase().contains("enoent")
        {
            return Err("git is not available on PATH".to_string());
        }
        return Ok(None);
    }

    if !stdout.is_empty() {
        return Ok(Some(stdout));
    }

    // Detached HEAD: `branch --show-current` is empty — show a short SHA.
    let rev = run_git(project_root, &["rev-parse", "--short", "HEAD"])?;
    let sha = String::from_utf8_lossy(&rev.stdout).trim().to_string();
    if sha.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("HEAD ({sha})")))
}

/// Run `git status --porcelain` in the project root. Empty vec if not a repo.
pub fn get_changed_files(project_root: &Path) -> Result<Vec<ChangedFile>, String> {
    if !project_root.is_dir() {
        return Err(format!("Not a directory: {}", project_root.display()));
    }

    let output = run_git(project_root, &["status", "--porcelain", "-uall"])?;
    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    if code == 128 || stderr.to_lowercase().contains("not a git repository") {
        return Ok(Vec::new());
    }
    if code != 0 && stdout.trim().is_empty() {
        if stderr.to_lowercase().contains("not recognized")
            || stderr.to_lowercase().contains("not found")
            || stderr.to_lowercase().contains("enoent")
        {
            return Err("git is not available on PATH".to_string());
        }
        return Ok(Vec::new());
    }

    Ok(parse_porcelain(&stdout))
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreTurnResult {
    pub restored: Vec<String>,
    pub removed: Vec<String>,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
}

/// Restore this turn's files to HEAD: tracked paths via `git restore`,
/// files that never existed in HEAD are deleted (untracked / staged-new).
/// Paths must sit inside the project. No HEAD → error, nothing is deleted.
pub fn restore_turn_files(project_root: &Path, paths: &[String]) -> Result<RestoreTurnResult, String> {
    if !project_root.is_dir() {
        return Err(format!("Not a directory: {}", project_root.display()));
    }
    if paths.is_empty() {
        return Err(crate::ts!("没有可撤销的文件", "No files to undo"));
    }
    if !has_head(project_root) {
        return Err(crate::ts!("仓库还没有 commit，无法还原到 HEAD", "The repository has no commits yet; cannot restore to HEAD"));
    }

    let mut relative = Vec::new();
    for raw in paths {
        match project_relative_path(project_root, raw) {
            Ok(path) => {
                if !relative.iter().any(|existing: &String| existing == &path) {
                    relative.push(path);
                }
            }
            Err(error) => {
                return Err(error);
            }
        }
    }

    let mut restore = Vec::new();
    let mut remove = Vec::new();
    for path in &relative {
        if exists_in_head(project_root, path) {
            restore.push(path.clone());
        } else {
            remove.push(path.clone());
        }
    }

    let mut result = RestoreTurnResult::default();

    if !restore.is_empty() {
        match restore_from_head(project_root, &restore) {
            Ok(()) => result.restored.extend(restore),
            Err(error) => {
                for path in restore {
                    result.errors.push(format!("{path}: {error}"));
                }
            }
        }
    }

    for path in remove {
        let abs = project_root.join(&path);
        if abs.is_dir() {
            result
                .errors
                .push(crate::ts!("{path}: 是目录，未删除", "{path}: is a directory, not deleted"));
            continue;
        }
        if abs.is_file() {
            if let Err(error) = std::fs::remove_file(&abs) {
                result.errors.push(format!("{path}: {error}"));
                continue;
            }
            result.removed.push(path.clone());
        } else {
            result.skipped.push(path.clone());
        }
        // Drop a leftover index entry for staged-new files. Ignore failure
        // when the path was never indexed.
        let _ = run_git(project_root, &["rm", "--cached", "-f", "--", &path]);
    }

    if result.restored.is_empty() && result.removed.is_empty() && !result.errors.is_empty() {
        return Err(result.errors.join("\n"));
    }
    Ok(result)
}

fn has_head(project_root: &Path) -> bool {
    run_git(project_root, &["rev-parse", "--verify", "HEAD"])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn exists_in_head(project_root: &Path, rel: &str) -> bool {
    run_git(project_root, &["cat-file", "-e", &format!("HEAD:{rel}")])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn restore_from_head(project_root: &Path, paths: &[String]) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["restore", "--source=HEAD", "--staged", "--worktree", "--"];
    args.extend(paths.iter().map(String::as_str));
    let output = run_git(project_root, &args)?;
    if output.status.success() {
        return Ok(());
    }
    // Git < 2.23 has no `restore`.
    let mut fallback: Vec<&str> = vec!["checkout", "HEAD", "--"];
    fallback.extend(paths.iter().map(String::as_str));
    let output = run_git(project_root, &fallback)?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        "git restore failed".to_string()
    } else {
        stderr
    })
}

/// Optional read-only single-file diff (truncated).
pub fn get_file_diff(project_root: &Path, path: &str) -> Result<String, String> {
    let path = project_relative_path(project_root, path)?;
    // Compare against HEAD so staged and unstaged edits are both visible.
    // Repositories without a HEAD fall back to the working-tree diff below.
    let output = run_git(project_root, &["diff", "--no-color", "HEAD", "--", &path])?;
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    if text.trim().is_empty() {
        let working_tree = run_git(project_root, &["diff", "--no-color", "--", &path])?;
        text = String::from_utf8_lossy(&working_tree.stdout).to_string();
    }
    if text.trim().is_empty() {
        let untracked = run_git(project_root, &["status", "--porcelain", "--", &path])?;
        let st = String::from_utf8_lossy(&untracked.stdout);
        if st.trim().starts_with("??") {
            // `git diff` does not include untracked files. `--no-index` gives
            // the same unified format for a new file, including its contents.
            let added = run_git(
                project_root,
                &["diff", "--no-color", "--no-index", "--", "/dev/null", &path],
            )?;
            text = String::from_utf8_lossy(&added.stdout).to_string();
            if text.trim().is_empty() {
                return Ok(format!("(untracked) {path}\n"));
            }
        }
    }
    const MAX: usize = 80_000;
    if text.len() > MAX {
        // Byte-cap must land on a char boundary — CJK/emoji diffs make a raw
        // `truncate(80000)` panic mid-codepoint and take the whole app down.
        text.truncate(floor_char_boundary(&text, MAX));
        text.push_str("\n… (diff truncated)\n");
    }
    Ok(text)
}

// ─── Per-turn workspace snapshots (turn start vs turn end, tree to tree) ───
///
/// Why trees and not `git status` + `git diff HEAD` fingerprints:
/// `status` compares worktree stat data while the diff compares content, so a
/// `text=auto` / CRLF repo reports far more "changed" files than the diff can
/// fingerprint (this repo, 2026-10-23: 227 `status` paths, only 41 files with a
/// diff, and a tree comparison finds 52 files whose content actually differs).
/// The turn delta then degraded to changeType-only comparisons, and a *failed*
/// snapshot (silent `catch` in `api.ts`, `index.lock` contention with a second
/// agent) became an empty baseline — every dirty file in the workspace was
/// published as this turn's work ("delivery card shows the whole repo").
///
/// Now both sides of a turn are content trees written through a **private**
/// index and object directory (`GIT_INDEX_FILE` / `GIT_OBJECT_DIRECTORY` plus
/// the repo's own objects as an alternate), so the user's index, worktree and
/// object store are never touched, and `diff-tree` between the two trees is
/// the turn delta *by content*. Same shape as
/// `@deepseek-ai/dsh-workspace-changes`.
pub struct WorkspaceSnapshot {
    dir: PathBuf,
    objects: PathBuf,
    alternates: PathBuf,
    root: PathBuf,
    /// The repo's own `.git/index`, copied (never written) into the private
    /// index so `add --all` can reuse its stat cache.
    seed_index: Option<PathBuf>,
    base_tree: String,
    head_tree: Option<String>,
    /// `rev-parse --show-prefix`: the project's path inside the repository
    /// (`""` at the repo root). Stripped from every reported path so callers
    /// compare against paths that are relative to the project.
    prefix: String,
    created_ms: u128,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSnapshotStart {
    pub snapshot_id: String,
    /// Paths already dirty (vs HEAD) when the turn started. `None` when the
    /// repo has no HEAD — the touched-set rule then cannot be applied.
    pub dirty_paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSnapshotFile {
    pub path: String,
    pub old_path: Option<String>,
    /// `added` / `modified` / `deleted` / `untracked` (mirrors `ChangedFile`).
    pub change_type: String,
    /// Real line counts from `diff-tree --numstat` — a rename reports 0/0.
    pub add: u64,
    pub del: u64,
    pub binary: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSnapshotEnd {
    pub files: Vec<TurnSnapshotFile>,
}

/// Turn-end snapshots that never arrived (cancelled turn, app killed mid
/// stream) would otherwise pin hashed worktree blobs in `%TEMP%` forever.
const SNAPSHOT_TTL_MS: u128 = 10 * 60 * 1000;

/// Ceiling for one git invocation. status/diff-tree on huge repos can take a
/// few seconds, but nothing legitimate takes minutes — and an unbounded wait
/// holds the caller's thread (delivery-card turns, Tauri commands) hostage.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn snapshot_registry() -> &'static std::sync::Mutex<std::collections::HashMap<String, WorkspaceSnapshot>>
{
    static REGISTRY: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, WorkspaceSnapshot>>,
    > = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn prune_snapshots(now: u128) {
    let Ok(mut registry) = snapshot_registry().lock() else {
        return;
    };
    let stale: Vec<String> = registry
        .iter()
        .filter(|(_, snapshot)| now.saturating_sub(snapshot.created_ms) > SNAPSHOT_TTL_MS)
        .map(|(id, _)| id.clone())
        .collect();
    for id in stale {
        if let Some(snapshot) = registry.remove(&id) {
            let _ = std::fs::remove_dir_all(&snapshot.dir);
        }
    }
}

/// `git` configured to write into the snapshot's private index/objects.
struct SnapshotGit<'a> {
    index: &'a Path,
    objects: &'a Path,
    alternates: &'a Path,
}

fn run_snapshot_git(
    cwd: &Path,
    git: &SnapshotGit<'_>,
    args: &[&str],
) -> Result<std::process::Output, String> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_INDEX_FILE", git.index)
        .env("GIT_OBJECT_DIRECTORY", git.objects)
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", git.alternates)
        // Never take the repo's `index.lock`: another agent may be running git
        // in the same worktree, and a lock failure here used to degrade into
        // "the whole workspace changed".
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output().map_err(|error| format!("Failed to run git: {error}"))
}

fn snapshot_git_ok(cwd: &Path, git: &SnapshotGit<'_>, args: &[&str]) -> Result<String, String> {
    let output = run_snapshot_git(cwd, git, args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("git {} failed", args.first().copied().unwrap_or(""))
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Tree oid of the whole worktree (tracked edits + untracked, `.gitignore`
/// respected) as seen through `git`'s private index.
/// Tree oid of the whole worktree (tracked edits + untracked, `.gitignore`
/// respected) as seen through `git`'s private index.
///
/// `seed` is a copy source for the private index — the user's real
/// `.git/index`. Handing `add --all` the user's stat cache means it only reads
/// genuinely dirty files: 0.6 s instead of 9 s, which is what a stat-less
/// `read-tree` baseline costs on this repo (2026-10-23 measured: it rehashes
/// every tracked file). A split-index or otherwise unreadable copy falls back
/// to the stat-less path, still correct, just slower.
fn worktree_tree(
    root: &Path,
    git: &SnapshotGit<'_>,
    head_tree: Option<&str>,
    seed: Option<&Path>,
) -> Result<String, String> {
    let mut seeded = false;
    if let Some(seed) = seed {
        seeded = std::fs::copy(seed, git.index).is_ok()
            && snapshot_git_ok(root, git, &["add", "--all"]).is_ok();
    }
    if !seeded {
        let _ = std::fs::remove_file(git.index);
        match head_tree {
            Some(tree) => snapshot_git_ok(root, git, &["read-tree", tree])?,
            None => snapshot_git_ok(root, git, &["read-tree", "--empty"])?,
        };
        snapshot_git_ok(root, git, &["add", "--all"])?;
    }
    Ok(snapshot_git_ok(root, git, &["write-tree"])?.trim().to_string())
}

/// `<repo>/.git/index` (per-worktree index for linked worktrees). Only ever
/// read from — the private snapshot index is a copy.
fn repo_index_path(project_root: &Path) -> Option<PathBuf> {
    let output = run_git(project_root, &["rev-parse", "--git-path", "index"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    let path = Path::new(&raw);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    })
}

fn nul_tokens(stdout: &str) -> Vec<String> {
    stdout
        .split('\0')
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

/// `git diff-tree --name-status -z` records: `M\0path\0`,
/// `R100\0old\0new\0`, `A\0path\0`, `D\0path\0`.
fn parse_name_status(tokens: &[String]) -> Vec<(String, String, Option<String>)> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let status = tokens[index].clone();
        index += 1;
        if status.starts_with('R') || status.starts_with('C') {
            if index + 1 >= tokens.len() {
                break;
            }
            let old = tokens[index].clone();
            let new = tokens[index + 1].clone();
            index += 2;
            out.push((status, new, Some(old)));
        } else {
            if index >= tokens.len() {
                break;
            }
            let path = tokens[index].clone();
            index += 1;
            out.push((status, path, None));
        }
    }
    out
}

/// `git diff-tree --numstat -z` records: `added\tdeleted\tpath` — a rename
/// carries an empty path and puts old/new in the two following NUL records.
fn parse_numstat(stdout: &str) -> std::collections::HashMap<String, (u64, u64, bool)> {
    let mut map = std::collections::HashMap::new();
    let tokens = nul_tokens(stdout);
    let mut index = 0;
    while index < tokens.len() {
        let record = &tokens[index];
        index += 1;
        let mut parts = record.splitn(3, '\t');
        let add = parts.next().unwrap_or("");
        let del = parts.next().unwrap_or("");
        let mut path = parts.next().unwrap_or("").to_string();
        if path.is_empty() {
            if index + 1 >= tokens.len() {
                break;
            }
            path = tokens[index + 1].clone();
            index += 2;
        }
        // `-` marks a binary file.
        let binary = add == "-" || del == "-";
        map.insert(
            path,
            (add.parse().unwrap_or(0), del.parse().unwrap_or(0), binary),
        );
    }
    map
}

fn snapshot_change_type(
    status: &str,
    path: &str,
    old_path: Option<&str>,
    head_paths: &std::collections::HashSet<String>,
) -> &'static str {
    match status.chars().next().unwrap_or('M') {
        // Present in HEAD but missing at turn start → the turn restored it.
        'A' => {
            if head_paths.contains(path) {
                "added"
            } else {
                "untracked"
            }
        }
        'C' => "untracked",
        'D' => "deleted",
        'R' => match old_path {
            Some(old) if head_paths.contains(old) => "modified",
            _ => "untracked",
        },
        _ => "modified",
    }
}

/// Baseline for one turn. `Ok(None)` = the project is not a git work tree
/// (plain folder / bare repo) — the caller keeps the tool ledger and publishes
/// no git truth. `Err` = git failed: fail closed, never "everything changed".
pub fn begin_workspace_snapshot(project_root: &Path) -> Result<Option<TurnSnapshotStart>, String> {
    if !project_root.is_dir() {
        return Ok(None);
    }
    let probe = run_git(project_root, &["rev-parse", "--is-inside-work-tree"])?;
    if !probe.status.success() {
        let stderr = String::from_utf8_lossy(&probe.stderr).to_lowercase();
        if stderr.contains("not recognized") || stderr.contains("not found") || stderr.contains("enoent")
        {
            return Err("git is not available on PATH".to_string());
        }
        return Ok(None);
    }
    if String::from_utf8_lossy(&probe.stdout).trim() != "true" {
        return Ok(None);
    }

    let common = run_git(project_root, &["rev-parse", "--git-common-dir"])?;
    if !common.status.success() {
        return Ok(None);
    }
    let common_raw = String::from_utf8_lossy(&common.stdout).trim().to_string();
    if common_raw.is_empty() {
        return Ok(None);
    }
    let common_dir = {
        let raw = Path::new(&common_raw);
        if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            project_root.join(raw)
        }
    };
    let alternates = common_dir.join("objects");
    let seed_index = repo_index_path(project_root);
    let prefix = run_git(project_root, &["rev-parse", "--show-prefix"])
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();

    let now = now_ms();
    prune_snapshots(now);
    static SNAPSHOT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SNAPSHOT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let snapshot_id = format!("{now}-{seq}");

    let dir = std::env::temp_dir()
        .join("marionette-wsnap")
        .join(&snapshot_id);
    let objects = dir.join("objects");
    if let Err(error) = std::fs::create_dir_all(&objects) {
        return Err(format!("Failed to create the snapshot directory: {error}"));
    }
    let index = dir.join("index");
    let git = SnapshotGit {
        index: &index,
        objects: &objects,
        alternates: &alternates,
    };

    let head_tree = match snapshot_git_ok(project_root, &git, &["rev-parse", "--verify", "-q", "HEAD^{tree}"]) {
        Ok(tree) => {
            let tree = tree.trim().to_string();
            if tree.is_empty() {
                None
            } else {
                Some(tree)
            }
        }
        Err(_) => None,
    };

    let base_tree = match worktree_tree(project_root, &git, head_tree.as_deref(), seed_index.as_deref()) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(error);
        }
    };

    // Everything that was already dirty before the turn: worktree tree vs HEAD
    // tree, so no `git status` stat scan and no disagreement with the diff.
    let dirty_at_start = match head_tree.as_deref() {
        Some(head) => {
            match snapshot_git_ok(
                project_root,
                &git,
                &[
                    "diff-tree",
                    "-r",
                    "--name-only",
                    "--no-renames",
                    "-z",
                    "--no-commit-id",
                    head,
                    &base_tree,
                ],
            ) {
                Ok(stdout) => Some(
                    nul_tokens(&stdout)
                        .iter()
                        .map(|path| strip_path_prefix(path, &prefix))
                        .collect(),
                ),
                Err(_) => None,
            }
        }
        None => None,
    };

    let registry = snapshot_registry();
    {
        let mut map = registry
            .lock()
            .map_err(|_| "Snapshot registry poisoned".to_string())?;
        map.insert(
            snapshot_id.clone(),
            WorkspaceSnapshot {
                dir,
                objects,
                alternates,
                root: project_root.to_path_buf(),
                seed_index,
                base_tree,
                head_tree,
                prefix,
                created_ms: now,
            },
        );
    }

    Ok(Some(TurnSnapshotStart {
        snapshot_id,
        dirty_paths: dirty_at_start,
    }))
}

/// Turn delta: worktree tree at turn end vs the baseline tree.
pub fn end_workspace_snapshot(snapshot_id: &str) -> Result<TurnSnapshotEnd, String> {
    let snapshot = {
        let mut map = snapshot_registry()
            .lock()
            .map_err(|_| "Snapshot registry poisoned".to_string())?;
        map.remove(snapshot_id)
    }
    .ok_or_else(|| "Unknown or expired workspace snapshot".to_string())?;
    let result = finish_snapshot(&snapshot);
    let _ = std::fs::remove_dir_all(&snapshot.dir);
    result
}

/// Drop a snapshot without comparing (send failed, turn never ran).
pub fn discard_workspace_snapshot(snapshot_id: &str) {
    if let Ok(mut map) = snapshot_registry().lock() {
        if let Some(snapshot) = map.remove(snapshot_id) {
            let _ = std::fs::remove_dir_all(&snapshot.dir);
        }
    }
}

fn finish_snapshot(snapshot: &WorkspaceSnapshot) -> Result<TurnSnapshotEnd, String> {
    let index = snapshot.dir.join("index-end");
    let git = SnapshotGit {
        index: &index,
        objects: &snapshot.objects,
        alternates: &snapshot.alternates,
    };
    let end_tree = worktree_tree(
        &snapshot.root,
        &git,
        snapshot.head_tree.as_deref(),
        snapshot.seed_index.as_deref(),
    )?;
    if end_tree == snapshot.base_tree {
        return Ok(TurnSnapshotEnd { files: Vec::new() });
    }

    let name_status = snapshot_git_ok(
        &snapshot.root,
        &git,
        &[
            "diff-tree",
            "-r",
            "-M",
            "-z",
            "--name-status",
            "--no-commit-id",
            &snapshot.base_tree,
            &end_tree,
        ],
    )?;
    let numstat = snapshot_git_ok(
        &snapshot.root,
        &git,
        &[
            "diff-tree",
            "-r",
            "-M",
            "-z",
            "--numstat",
            "--no-commit-id",
            &snapshot.base_tree,
            &end_tree,
        ],
    )?;
    let head_paths: std::collections::HashSet<String> = match snapshot.head_tree.as_deref() {
        Some(head) => snapshot_git_ok(&snapshot.root, &git, &["ls-tree", "-r", "--name-only", "-z", head])?
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect(),
        None => std::collections::HashSet::new(),
    };

    let stats = parse_numstat(&numstat);
    let mut files = Vec::new();
    for (status, path, old_path) in parse_name_status(&nul_tokens(&name_status)) {
        let (add, del, binary) = stats.get(&path).copied().unwrap_or((0, 0, false));
        files.push(TurnSnapshotFile {
            change_type: snapshot_change_type(&status, &path, old_path.as_deref(), &head_paths)
                .to_string(),
            path: strip_path_prefix(&path, &snapshot.prefix),
            old_path: old_path
                .as_deref()
                .map(|old| strip_path_prefix(old, &snapshot.prefix)),
            add,
            del,
            binary,
        });
    }
    Ok(TurnSnapshotEnd { files })
}

/// Strip the `--show-prefix` prefix so paths are relative to the project root,
/// which may be a subdirectory of the repository.
fn strip_path_prefix(path: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return path.to_string();
    }
    match path.strip_prefix(prefix) {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => path.to_string(),
    }
}

/// Largest `<= max` that is a UTF-8 character boundary (or 0).
fn floor_char_boundary(text: &str, max: usize) -> usize {
    if max >= text.len() {
        return text.len();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Git pathspecs must be relative to the project. ACP often sends an absolute
/// Windows path, while the Changed Files panel sends a relative one.
fn project_relative_path(project_root: &Path, raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Err("Invalid path".to_string());
    }

    let requested = Path::new(raw);
    let relative = if requested.is_absolute() {
        requested
            .strip_prefix(project_root)
            .map_err(|_| "Path is outside the project".to_string())?
    } else {
        requested
    };

    let mut clean = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => clean.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Invalid path".to_string());
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err("Invalid path".to_string());
    }
    Ok(clean.to_string_lossy().replace('\\', "/"))
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(cwd);
    // A credential prompt or an nfs-slow disk would otherwise wedge the
    // command thread forever; every git here is short-status work.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    crate::process_util::output_with_kill_timeout(cmd, GIT_TIMEOUT)
        .map_err(|e| format!("Failed to run git: {e}"))
}

fn parse_porcelain(stdout: &str) -> Vec<ChangedFile> {
    let mut files = Vec::new();
    for line in stdout.lines() {
        // XY status pair is always ASCII; bail if the bytes after it would
        // split a multi-byte char (defensive — a panic here kills the app).
        if line.len() < 4 || !line.is_char_boundary(2) {
            continue;
        }
        let status = &line[..2];
        let path_part = line[2..].trim_start();
        let path = if let Some((_, new_path)) = path_part.split_once(" -> ") {
            new_path.trim().to_string()
        } else {
            path_part.trim().trim_matches('"').to_string()
        };
        if path.is_empty() {
            continue;
        }
        files.push(ChangedFile {
            path,
            change_type: map_status(status),
        });
    }
    files
}

fn map_status(xy: &str) -> String {
    let bytes = xy.as_bytes();
    let x = bytes.first().copied().unwrap_or(b' ') as char;
    let y = bytes.get(1).copied().unwrap_or(b' ') as char;
    if x == '?' || y == '?' {
        return "untracked".to_string();
    }
    if x == 'D' || y == 'D' {
        return "deleted".to_string();
    }
    if x == 'A' || y == 'A' || x == 'C' || y == 'C' {
        return "added".to_string();
    }
    if x == 'M' || y == 'M' || x == 'R' || y == 'R' || x == 'U' || y == 'U' {
        return "modified".to_string();
    }
    "modified".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_lines() {
        let raw = " M src/a.rs\nA  src/b.rs\n?? notes.txt\n D gone.rs\n";
        let files = parse_porcelain(raw);
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].change_type, "modified");
        assert_eq!(files[1].change_type, "added");
        assert_eq!(files[2].change_type, "untracked");
        assert_eq!(files[3].change_type, "deleted");
    }

    /// Diffs with CJK content are multi-byte; a fixed byte cap often lands
    /// mid-character. That used to panic the process on send/diff refresh.
    #[test]
    fn floor_char_boundary_never_splits_multibyte() {
        let cjk = "战".repeat(30_000); // 90_000 bytes
        assert!(cjk.len() > 80_000);
        assert!(!cjk.is_char_boundary(80_000));

        let end = floor_char_boundary(&cjk, 80_000);
        assert!(end <= 80_000);
        assert!(cjk.is_char_boundary(end));
        assert_eq!(end % 3, 0);

        let mut text = cjk;
        text.truncate(end);
        text.push_str("\n… (diff truncated)\n");
        assert!(text.ends_with("\n… (diff truncated)\n"));
    }

    #[test]
    fn floor_char_boundary_short_and_exact() {
        assert_eq!(floor_char_boundary("abc", 10), 3);
        assert_eq!(floor_char_boundary("abc", 3), 3);
        assert_eq!(floor_char_boundary("战", 1), 0);
        assert_eq!(floor_char_boundary("战", 3), 3);
    }

    #[test]
    fn parses_name_status_and_numstat_records() {
        let records = parse_name_status(&nul_tokens(
            "M\0src/one.rs\0R100\0old.rs\0new.rs\0D\0gone.rs\0",
        ));
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].0, "M");
        assert_eq!(records[0].1, "src/one.rs");
        assert_eq!(records[0].2, None);
        assert_eq!(records[1].1, "new.rs");
        assert_eq!(records[1].2.as_deref(), Some("old.rs"));
        assert_eq!(records[2].1, "gone.rs");

        // A rename record carries an empty path and puts old/new in the two
        // following NUL records; `-` marks binary.
        let stats = parse_numstat("1\t2\tsrc/one.rs\u{0}-\t-\tlogo.png\u{0}0\t0\t\u{0}old.rs\u{0}new.rs\u{0}");
        assert_eq!(stats["src/one.rs"], (1, 2, false));
        assert_eq!(stats["logo.png"], (0, 0, true));
        assert_eq!(stats["new.rs"], (0, 0, false));
    }

    #[test]
    fn snapshot_change_type_separates_added_from_untracked() {
        let mut head = std::collections::HashSet::new();
        head.insert("tracked.rs".to_string());
        assert_eq!(snapshot_change_type("A", "tracked.rs", None, &head), "added");
        assert_eq!(snapshot_change_type("A", "fresh.rs", None, &head), "untracked");
        assert_eq!(snapshot_change_type("D", "gone.rs", None, &head), "deleted");
        assert_eq!(snapshot_change_type("M", "tracked.rs", None, &head), "modified");
        assert_eq!(
            snapshot_change_type("R100", "new.rs", Some("tracked.rs"), &head),
            "modified"
        );
        assert_eq!(snapshot_change_type("R100", "new.rs", Some("old.rs"), &head), "untracked");
    }

    /// The delivery card used to publish every dirty file in the workspace on a
    /// failed/absent baseline. The tree snapshot must report ONLY what changed
    /// inside the turn window — pre-existing dirt the turn never touched must
    /// not appear, whatever its size.
    #[test]
    fn turn_snapshot_reports_only_the_turn_window() {
        if !git_available() {
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("marionette-wsnap-test-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        run_in(&dir, &["init", "-q"]);
        run_in(&dir, &["config", "user.email", "test@example.com"]);
        run_in(&dir, &["config", "user.name", "Test"]);
        run_in(&dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        run_in(&dir, &["add", "-A"]);
        run_in(&dir, &["commit", "-qm", "init"]);

        // Dirt that predates the turn (user edits / another agent / an earlier
        // conversation). None of it may reach this turn's delta.
        std::fs::write(dir.join("a.txt"), "a\npre-existing\n").unwrap();
        std::fs::write(dir.join("b.txt"), "b\npre-existing\n").unwrap();

        let start = begin_workspace_snapshot(&dir)
            .expect("snapshot")
            .expect("git worktree");
        let mut dirty = start.dirty_paths.clone().unwrap_or_default();
        dirty.sort();
        assert_eq!(dirty, vec!["a.txt".to_string(), "b.txt".to_string()]);

        // The turn itself: one edit, one new untracked file.
        std::fs::write(dir.join("b.txt"), "b\npre-existing\nturn\n").unwrap();
        std::fs::write(dir.join("c.txt"), "new\n").unwrap();

        let end = end_workspace_snapshot(&start.snapshot_id).expect("turn delta");
        let mut paths: Vec<&str> = end.files.iter().map(|file| file.path.as_str()).collect();
        paths.sort();
        assert_eq!(
            paths,
            vec!["b.txt", "c.txt"],
            "untouched pre-existing dirt must not be reported"
        );
        let edited = end.files.iter().find(|file| file.path == "b.txt").unwrap();
        assert_eq!(edited.change_type, "modified");
        assert_eq!((edited.add, edited.del), (1, 0));
        let created = end.files.iter().find(|file| file.path == "c.txt").unwrap();
        assert_eq!(created.change_type, "untracked");
        assert_eq!((created.add, created.del), (1, 0));

        // The snapshot must not touch the user's index or worktree.
        let status = run_in(&dir, &["status", "--porcelain"]);
        let text = String::from_utf8_lossy(&status.stdout).to_string();
        assert!(text.contains(" M a.txt"), "index touched: {text}");
        assert!(text.contains("?? c.txt"), "index touched: {text}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn turn_snapshot_reports_paths_relative_to_a_subdir_project() {
        if !git_available() {
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let repo = std::env::temp_dir().join(format!("marionette-wsnap-sub-{stamp}"));
        let project = repo.join("packages/ui");
        std::fs::create_dir_all(&project).unwrap();
        run_in(&repo, &["init", "-q"]);
        run_in(&repo, &["config", "user.email", "test@example.com"]);
        run_in(&repo, &["config", "user.name", "Test"]);
        run_in(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("README.md"), "root\n").unwrap();
        std::fs::write(project.join("app.ts"), "one\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-qm", "init"]);

        // A project root below the repo root must still speak project-relative
        // paths — the delivery card and `getFileDiff` both resolve from there.
        let start = begin_workspace_snapshot(&project)
            .expect("snapshot")
            .expect("git worktree");
        assert_eq!(start.dirty_paths, Some(Vec::new()));
        std::fs::write(project.join("app.ts"), "one\ntwo\n").unwrap();
        let end = end_workspace_snapshot(&start.snapshot_id).expect("turn delta");
        let paths: Vec<&str> = end.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, vec!["app.ts"]);

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn turn_snapshot_skips_non_repo_and_unknown_ids() {
        if !git_available() {
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("marionette-wsnap-norepo-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        // Not a repo → no git truth, no error (the tool ledger still works).
        assert!(begin_workspace_snapshot(&dir).unwrap().is_none());
        assert!(end_workspace_snapshot("does-not-exist").is_err());
        discard_workspace_snapshot("does-not-exist");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn run_in(dir: &Path, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.output().unwrap_or_else(|error| panic!("git {args:?}: {error}"))
    }

    #[test]
    fn restore_turn_files_reverts_modified_and_deletes_untracked() {
        if !git_available() {
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("marionette-restore-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        let tracked = dir.join("keep.txt");
        std::fs::write(&tracked, "original\n").unwrap();
        assert!(run_in(&dir, &["init", "-q"]).status.success());
        assert!(run_in(&dir, &["config", "user.email", "test@example.com"]).status.success());
        assert!(run_in(&dir, &["config", "user.name", "test"]).status.success());
        let _ = run_in(&dir, &["config", "commit.gpgsign", "false"]);
        let _ = run_in(&dir, &["config", "core.autocrlf", "false"]);
        assert!(run_in(&dir, &["add", "keep.txt"]).status.success());
        assert!(run_in(&dir, &["commit", "-q", "-m", "init"]).status.success());

        std::fs::write(&tracked, "edited\n").unwrap();
        let extra = dir.join("new.txt");
        std::fs::write(&extra, "untracked\n").unwrap();

        let result = restore_turn_files(
            &dir,
            &["keep.txt".into(), "new.txt".into()],
        )
        .expect("restore");
        assert_eq!(result.restored, vec!["keep.txt".to_string()]);
        assert_eq!(result.removed, vec!["new.txt".to_string()]);
        assert_eq!(std::fs::read_to_string(&tracked).unwrap(), "original\n");
        assert!(!extra.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_turn_files_rejects_outside_paths() {
        if !git_available() {
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(1);
        let dir = std::env::temp_dir().join(format!("marionette-restore-out-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep.txt"), "x\n").unwrap();
        assert!(run_in(&dir, &["init", "-q"]).status.success());
        assert!(run_in(&dir, &["config", "user.email", "test@example.com"]).status.success());
        assert!(run_in(&dir, &["config", "user.name", "test"]).status.success());
        let _ = run_in(&dir, &["config", "commit.gpgsign", "false"]);
        assert!(run_in(&dir, &["add", "keep.txt"]).status.success());
        assert!(run_in(&dir, &["commit", "-q", "-m", "init"]).status.success());

        let err = restore_turn_files(&dir, &["../secret.txt".into()]).unwrap_err();
        assert!(err.to_lowercase().contains("invalid"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
