use crate::models::{Project, Session};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct StorageService {
    global_dir: PathBuf,
    projects_file: PathBuf,
    /// session id → transcript JSONL path. Avoids `find_session` (full index
    /// scan) on every stream-tick write, and lets the command drop the storage
    /// lock before `fs::write`.
    transcript_paths: Mutex<HashMap<String, PathBuf>>,
}

impl StorageService {
    pub fn new() -> Result<Self, String> {
        Self::from_global_dir(crate::app_paths::global_dir()?)
    }

    fn from_global_dir(global_dir: PathBuf) -> Result<Self, String> {
        let projects_file = global_dir.join("projects.json");
        fs::create_dir_all(&global_dir)
            .map_err(|error| format!("Create storage directory failed: {error}"))?;
        let service = Self {
            global_dir,
            projects_file,
            transcript_paths: Mutex::new(HashMap::new()),
        };
        // Older builds incorrectly persisted Chat as a normal project. Move
        // those rows to the global chat store before the project index is read.
        service.migrate_legacy_chat_project()?;
        Ok(service)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, String> {
        if !self.projects_file.exists() {
            self.write_projects(&[])?;
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&self.projects_file)
            .map_err(|error| format!("Read projects failed: {error}"))?;
        if content.trim().is_empty() {
            return Ok(Vec::new());
        }

        let projects: Vec<Project> = serde_json::from_str(&content)
            .map_err(|error| format!("Parse projects failed: {error}"))?;
        // `project-chat` was used by an interrupted implementation. Keep the
        // index clean even if it was written while the app was already running.
        Ok(projects
            .into_iter()
            .filter(|project| project.id != crate::app_paths::CHAT_PROJECT_ID)
            .collect())
    }

    pub fn add_project(&self, raw_path: String) -> Result<Project, String> {
        let path = fs::canonicalize(Path::new(raw_path.trim()))
            .map_err(|error| format!("Project path is not accessible: {error}"))?;
        if !path.is_dir() {
            return Err("Project path must be a directory".to_string());
        }

        let path_string = path.to_string_lossy().to_string();
        let mut projects = self.list_projects()?;
        if let Some(existing) = projects
            .iter_mut()
            .find(|project| project.root_path == path_string)
        {
            existing.last_opened_at = now_string();
            let result = existing.clone();
            self.write_projects(&projects)?;
            self.ensure_project_dirs(&path)?;
            return Ok(result);
        }

        let project = Project {
            id: project_id(&path_string),
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Project")
                .to_string(),
            root_path: path_string,
            created_at: now_string(),
            last_opened_at: now_string(),
        };
        projects.push(project.clone());
        self.write_projects(&projects)?;
        self.ensure_project_dirs(&path)?;
        Ok(project)
    }

    /// Create a Chat session in the global Chat store.
    ///
    /// Chat is a logical session group only. It must not create a `.marionette`
    /// directory in the user's working folder and must not appear in
    /// `projects.json`.
    pub fn create_chat_session(
        &self,
        agent_id: String,
        label: String,
        cwd: &str,
    ) -> Result<Session, String> {
        let cwd_path = fs::canonicalize(Path::new(cwd.trim()))
            .map_err(|error| format!("Chat folder is not accessible: {error}"))?;
        if !cwd_path.is_dir() {
            return Err("Chat folder must be a directory".to_string());
        }

        self.ensure_chat_dirs()?;
        let id = format!("session-{}", now_string());
        let now = now_string();
        let session = Session {
            id: id.clone(),
            project_id: crate::app_paths::CHAT_PROJECT_ID.to_string(),
            agent_id,
            label: if label.trim().is_empty() {
                "New session".to_string()
            } else {
                label
            },
            label_source: Some("default".to_string()),
            cwd: cwd_path.to_string_lossy().to_string(),
            status: "exited".to_string(),
            process_id: None,
            pty_id: None,
            started_at: String::new(),
            last_active_at: now,
            exited_at: None,
            exit_code: None,
            raw_log_path: self.chat_session_log_path(&id).to_string_lossy().to_string(),
            transcript_path: self
                .chat_dir()
                .join("transcripts")
                .join(format!("{id}.jsonl"))
                .to_string_lossy()
                .to_string(),
            handoff_path: self
                .chat_dir()
                .join("handoff")
                .join(format!("{id}.md"))
                .to_string_lossy()
                .to_string(),
            view_mode: "clean".to_string(),
            preferred_model: None,
            preferred_mode: None,
            preferred_effort: None,
            preferred_effort_id: None,
            preferred_always_approve: None,
            parent_session_id: None,
            origin: Some("user".to_string()),
        };
        let mut sessions = self.read_chat_sessions_all()?;
        sessions.retain(|current| current.id != session.id);
        sessions.insert(0, session.clone());
        self.write_chat_sessions(&sessions)?;
        self.remember_transcript_path(&session);
        Ok(session)
    }

    /// List top-level Chat sessions from the global Chat store.
    pub fn list_chat_sessions(&self) -> Result<Vec<Session>, String> {
        Ok(self
            .read_chat_sessions_all()?
            .into_iter()
            .filter(|s| s.parent_session_id.as_ref().map(|p| p.is_empty()).unwrap_or(true))
            .collect())
    }

    /// Check if a directory contains a real project data directory.
    pub fn has_marionette_dir(path: &str) -> bool {
        fs::canonicalize(Path::new(path))
            .map(|dir| dir.join(crate::app_paths::DIR_NAME).is_dir())
            .unwrap_or(false)
    }

    /// Remove a project from the global list only — never deletes workspace files.
    pub fn delete_project(&self, project_id: &str) -> Result<(), String> {
        let mut projects = self.list_projects()?;
        let before = projects.len();
        projects.retain(|project| project.id != project_id);
        if projects.len() == before {
            return Err(format!("Unknown project: {project_id}"));
        }
        self.write_projects(&projects)
    }

    /// Persist a new project list order. Unknown ids are ignored; any projects
    /// missing from `ordered_ids` are appended in their previous relative order.
    pub fn reorder_projects(&self, ordered_ids: &[String]) -> Result<Vec<Project>, String> {
        let existing = self.list_projects()?;
        if existing.is_empty() {
            return Ok(existing);
        }
        let mut by_id: std::collections::HashMap<String, Project> = existing
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        let mut next = Vec::with_capacity(by_id.len());
        for id in ordered_ids {
            if let Some(project) = by_id.remove(id) {
                next.push(project);
            }
        }
        // Keep leftovers (e.g. concurrent add) after the explicit order.
        let mut rest: Vec<Project> = by_id.into_values().collect();
        rest.sort_by(|a, b| a.name.cmp(&b.name));
        next.extend(rest);
        self.write_projects(&next)?;
        Ok(next)
    }

    pub fn list_sessions(&self, project_id: &str) -> Result<Vec<Session>, String> {
        if project_id == crate::app_paths::CHAT_PROJECT_ID {
            return self.list_chat_sessions();
        }
        let project = self.project_by_id(project_id)?;
        let file = sessions_file(Path::new(&project.root_path));
        if !file.exists() {
            return Ok(Vec::new());
        }

        let content =
            fs::read_to_string(file).map_err(|error| format!("Read sessions failed: {error}"))?;
        if content.trim().is_empty() {
            return Ok(Vec::new());
        }
        let all: Vec<Session> =
            serde_json::from_str(&content).map_err(|error| format!("Parse sessions failed: {error}"))?;
        // Top-level shelf: hide @-delegate children (they live under the parent card).
        Ok(all
            .into_iter()
            .filter(|s| s.parent_session_id.as_ref().map(|p| p.is_empty()).unwrap_or(true))
            .collect())
    }

    /// All sessions including delegate children (for cascade delete / child listing).
    pub fn list_sessions_all(&self, project_id: &str) -> Result<Vec<Session>, String> {
        if project_id == crate::app_paths::CHAT_PROJECT_ID {
            return self.read_chat_sessions_all();
        }
        let project = self.project_by_id(project_id)?;
        let file = sessions_file(Path::new(&project.root_path));
        if !file.exists() {
            return Ok(Vec::new());
        }
        let content =
            fs::read_to_string(file).map_err(|error| format!("Read sessions failed: {error}"))?;
        if content.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str(&content).map_err(|error| format!("Parse sessions failed: {error}"))
    }

    /// Sessions with the stored live status repaired.
    ///
    /// `status` is runtime state: a stored `running` only means the previous run
    /// was closed mid-turn. Nothing is live unless this process says so, and a
    /// dialog restored as "Working" would lie to the user (and lock the composer
    /// into interrupt mode). The repair is written back so the file stops lying.
    pub fn list_sessions_healed<F>(
        &self,
        project_id: &str,
        is_live: F,
    ) -> Result<Vec<Session>, String>
    where
        F: Fn(&str) -> bool,
    {
        if project_id == crate::app_paths::CHAT_PROJECT_ID {
            let mut sessions = self.read_chat_sessions_all()?;
            let mut healed = false;
            for session in sessions.iter_mut() {
                let claims_live =
                    matches!(session.status.as_str(), "starting" | "running" | "waiting");
                if !claims_live || is_live(&session.id) {
                    continue;
                }
                session.status = "exited".to_string();
                session.process_id = None;
                session.pty_id = None;
                if session.exited_at.is_none() {
                    session.exited_at = Some(session.last_active_at.clone());
                }
                healed = true;
            }
            if healed {
                self.write_chat_sessions(&sessions)?;
            }
            return Ok(sessions
                .into_iter()
                .filter(|s| s.parent_session_id.as_ref().map(|p| p.is_empty()).unwrap_or(true))
                .collect());
        }

        // Heal against the full file (including children), then return top-level only.
        let mut sessions = self.list_sessions_all(project_id)?;
        let mut healed = false;
        for session in sessions.iter_mut() {
            let claims_live = matches!(session.status.as_str(), "starting" | "running" | "waiting");
            if !claims_live || is_live(&session.id) {
                continue;
            }
            session.status = "exited".to_string();
            session.process_id = None;
            session.pty_id = None;
            if session.exited_at.is_none() {
                session.exited_at = Some(session.last_active_at.clone());
            }
            healed = true;
        }
        if healed {
            let project = self.project_by_id(project_id)?;
            self.write_sessions(Path::new(&project.root_path), &sessions)?;
        }
        Ok(sessions
            .into_iter()
            .filter(|s| s.parent_session_id.as_ref().map(|p| p.is_empty()).unwrap_or(true))
            .collect())
    }

    pub fn create_session(
        &self,
        project_id: &str,
        agent_id: String,
        label: String,
    ) -> Result<Session, String> {
        if project_id == crate::app_paths::CHAT_PROJECT_ID {
            let cwd = crate::app_paths::current_dir()?;
            return self.create_chat_session(agent_id, label, &cwd);
        }
        let project = self.project_by_id(project_id)?;
        let project_path = Path::new(&project.root_path);
        self.ensure_project_dirs(project_path)?;
        let id = format!("session-{}", now_string());
        let now = now_string();
        let raw_log_path = session_log_path(project_path, &id);
        let session = Session {
            id: id.clone(),
            project_id: project.id,
            agent_id,
            label: if label.trim().is_empty() {
                "New session".to_string()
            } else {
                label
            },
            label_source: Some("default".to_string()),
            cwd: project.root_path.clone(),
            status: "exited".to_string(),
            process_id: None,
            pty_id: None,
            started_at: String::new(),
            last_active_at: now,
            exited_at: None,
            exit_code: None,
            raw_log_path: raw_log_path.to_string_lossy().to_string(),
            transcript_path: crate::app_paths::project_dir(project_path)
                .join("transcripts")
                .join(format!("{id}.jsonl"))
                .to_string_lossy()
                .to_string(),
            handoff_path: crate::app_paths::project_dir(project_path)
                .join("handoff")
                .join(format!("{id}.md"))
                .to_string_lossy()
                .to_string(),
            // Product primary surface is Clean View (Raw is always available as toggle).
            view_mode: "clean".to_string(),
            preferred_model: None,
            preferred_mode: None,
            preferred_effort: None,
            preferred_effort_id: None,
            preferred_always_approve: None,
            parent_session_id: None,
            origin: Some("user".to_string()),
        };
        // Prepend so newest dialogs appear at the top of the project shelf.
        let mut sessions = self.list_sessions_all(project_id)?;
        sessions.retain(|s| s.id != session.id);
        sessions.insert(0, session.clone());
        self.write_sessions(project_path, &sessions)?;
        self.remember_transcript_path(&session);
        Ok(session)
    }

    /// Create a child session for `@` delegate (hidden from list_sessions).
    pub fn create_child_session(
        &self,
        project_id: &str,
        parent_session_id: &str,
        agent_id: String,
        label: String,
    ) -> Result<Session, String> {
        if project_id == crate::app_paths::CHAT_PROJECT_ID {
            let mut sessions = self.read_chat_sessions_all()?;
            let parent = sessions
                .iter()
                .find(|session| session.id == parent_session_id)
                .ok_or_else(|| format!("Unknown parent session: {parent_session_id}"))?;
            let id = format!("session-{}", now_string());
            let now = now_string();
            let session = Session {
                id: id.clone(),
                project_id: crate::app_paths::CHAT_PROJECT_ID.to_string(),
                agent_id,
                label: if label.trim().is_empty() {
                    "Delegate".to_string()
                } else {
                    label
                },
                label_source: Some("default".to_string()),
                cwd: parent.cwd.clone(),
                status: "exited".to_string(),
                process_id: None,
                pty_id: None,
                started_at: String::new(),
                last_active_at: now,
                exited_at: None,
                exit_code: None,
                raw_log_path: self.chat_session_log_path(&id).to_string_lossy().to_string(),
                transcript_path: self
                    .chat_dir()
                    .join("transcripts")
                    .join(format!("{id}.jsonl"))
                    .to_string_lossy()
                    .to_string(),
                handoff_path: self
                    .chat_dir()
                    .join("handoff")
                    .join(format!("{id}.md"))
                    .to_string_lossy()
                    .to_string(),
                view_mode: "clean".to_string(),
                preferred_model: None,
                preferred_mode: None,
                preferred_effort: None,
                preferred_effort_id: None,
                preferred_always_approve: None,
                parent_session_id: Some(parent_session_id.to_string()),
                origin: Some("delegate".to_string()),
            };
            sessions.retain(|current| current.id != session.id);
            sessions.insert(0, session.clone());
            self.write_chat_sessions(&sessions)?;
            self.remember_transcript_path(&session);
            return Ok(session);
        }
        let project = self.project_by_id(project_id)?;
        let project_path = Path::new(&project.root_path);
        self.ensure_project_dirs(project_path)?;
        // Parent must exist (as top-level or anywhere).
        let all = self.list_sessions_all(project_id)?;
        if !all.iter().any(|s| s.id == parent_session_id) {
            return Err(format!("Unknown parent session: {parent_session_id}"));
        }
        let id = format!("session-{}", now_string());
        let now = now_string();
        let raw_log_path = session_log_path(project_path, &id);
        let session = Session {
            id: id.clone(),
            project_id: project.id,
            agent_id,
            label: if label.trim().is_empty() {
                "Delegate".to_string()
            } else {
                label
            },
            label_source: Some("default".to_string()),
            cwd: project.root_path.clone(),
            status: "exited".to_string(),
            process_id: None,
            pty_id: None,
            started_at: String::new(),
            last_active_at: now,
            exited_at: None,
            exit_code: None,
            raw_log_path: raw_log_path.to_string_lossy().to_string(),
            transcript_path: crate::app_paths::project_dir(project_path)
                .join("transcripts")
                .join(format!("{id}.jsonl"))
                .to_string_lossy()
                .to_string(),
            handoff_path: crate::app_paths::project_dir(project_path)
                .join("handoff")
                .join(format!("{id}.md"))
                .to_string_lossy()
                .to_string(),
            view_mode: "clean".to_string(),
            preferred_model: None,
            preferred_mode: None,
            preferred_effort: None,
            preferred_effort_id: None,
            preferred_always_approve: None,
            parent_session_id: Some(parent_session_id.to_string()),
            origin: Some("delegate".to_string()),
        };
        let mut sessions = all;
        sessions.retain(|s| s.id != session.id);
        sessions.insert(0, session.clone());
        self.write_sessions(project_path, &sessions)?;
        self.remember_transcript_path(&session);
        Ok(session)
    }

    pub fn list_child_sessions(&self, parent_session_id: &str) -> Result<Vec<Session>, String> {
        // Scan Chat and all projects — parent id is unique.
        let mut children = Vec::new();
        for session in self.read_chat_sessions_all()? {
            if session.parent_session_id.as_deref() == Some(parent_session_id) {
                children.push(session);
            }
        }
        for project in self.list_projects()? {
            for session in self.list_sessions_all(&project.id)? {
                if session.parent_session_id.as_deref() == Some(parent_session_id) {
                    children.push(session);
                }
            }
        }
        Ok(children)
    }

    pub fn delete_session(&self, project_id: &str, session_id: &str) -> Result<(), String> {
        if project_id == crate::app_paths::CHAT_PROJECT_ID {
            let mut sessions = self.read_chat_sessions_all()?;
            let child_ids: Vec<String> = sessions
                .iter()
                .filter(|s| s.parent_session_id.as_deref() == Some(session_id))
                .map(|s| s.id.clone())
                .collect();
            let drop_ids: std::collections::HashSet<String> = child_ids
                .iter()
                .cloned()
                .chain(std::iter::once(session_id.to_string()))
                .collect();
            for session in sessions.iter().filter(|s| drop_ids.contains(&s.id)) {
                let _ = fs::remove_file(&session.transcript_path);
                let _ = fs::remove_file(&session.raw_log_path);
                let _ = fs::remove_file(&session.handoff_path);
            }
            sessions.retain(|session| !drop_ids.contains(&session.id));
            return self.write_chat_sessions(&sessions);
        }
        let project = self.project_by_id(project_id)?;
        let mut sessions = self.list_sessions_all(project_id)?;
        // Cascade: remove children of this session too (and their transcripts).
        let child_ids: Vec<String> = sessions
            .iter()
            .filter(|s| s.parent_session_id.as_deref() == Some(session_id))
            .map(|s| s.id.clone())
            .collect();
        let drop_ids: std::collections::HashSet<String> = child_ids
            .iter()
            .cloned()
            .chain(std::iter::once(session_id.to_string()))
            .collect();
        for s in sessions.iter().filter(|s| drop_ids.contains(&s.id)) {
            let _ = fs::remove_file(&s.transcript_path);
        }
        sessions.retain(|session| !drop_ids.contains(&session.id));
        self.write_sessions(Path::new(&project.root_path), &sessions)
    }

    pub fn save_session(&self, session: &Session) -> Result<(), String> {
        self.remember_transcript_path(session);
        if session.project_id == crate::app_paths::CHAT_PROJECT_ID {
            let mut sessions = self.read_chat_sessions_all()?;
            if let Some(existing) = sessions.iter_mut().find(|current| current.id == session.id) {
                *existing = session.clone();
            } else {
                sessions.push(session.clone());
            }
            return self.write_chat_sessions(&sessions);
        }
        let project = self.project_by_id(&session.project_id)?;
        let project_path = Path::new(&project.root_path);
        self.ensure_project_dirs(project_path)?;
        let mut sessions = self.list_sessions_all(&session.project_id)?;
        if let Some(existing) = sessions.iter_mut().find(|current| current.id == session.id) {
            *existing = session.clone();
        } else {
            sessions.push(session.clone());
        }
        self.write_sessions(project_path, &sessions)
    }

    pub fn find_session(&self, session_id: &str) -> Result<Option<Session>, String> {
        if let Some(session) = self
            .read_chat_sessions_all()?
            .into_iter()
            .find(|session| session.id == session_id)
        {
            return Ok(Some(session));
        }
        for project in self.list_projects()? {
            if let Some(session) = self
                .list_sessions_all(&project.id)?
                .into_iter()
                .find(|session| session.id == session_id)
            {
                return Ok(Some(session));
            }
        }
        Ok(None)
    }

    pub fn update_session_status(&self, session_id: &str, status: &str) -> Result<(), String> {
        let Some(mut session) = self.find_session(session_id)? else {
            return Ok(());
        };
        session.status = status.to_string();
        session.last_active_at = now_string();
        if status == "running" {
            session.started_at = session.last_active_at.clone();
            session.exited_at = None;
        } else if status == "exited" || status == "error" {
            session.exited_at = Some(session.last_active_at.clone());
        }
        self.save_session(&session)
    }

    /// Persist which agent owns this dialog. Session.agent_id is the single source of truth.
    pub fn update_session_agent(&self, session_id: &str, agent_id: &str) -> Result<(), String> {
        let Some(mut session) = self.find_session(session_id)? else {
            return Err(format!("Session not found: {session_id}"));
        };
        session.agent_id = agent_id.to_string();
        session.last_active_at = now_string();
        // Transport is torn down on agent switch — mark idle until next warm.
        session.status = "exited".to_string();
        session.exited_at = Some(session.last_active_at.clone());
        session.process_id = None;
        session.pty_id = None;
        // Model/mode/effort are agent-specific — wipe so we don't apply Claude effort to OpenCode.
        session.preferred_model = None;
        session.preferred_mode = None;
        session.preferred_effort = None;
        session.preferred_effort_id = None;
        session.preferred_always_approve = None;
        self.save_session(&session)
    }

    /// Persist Composer model / mode / effort / always-approve for this dialog (SSOT on disk).
    pub fn update_session_prefs(
        &self,
        session_id: &str,
        preferred_model: Option<String>,
        preferred_mode: Option<String>,
        preferred_effort: Option<f64>,
        preferred_effort_id: Option<String>,
        preferred_always_approve: Option<bool>,
    ) -> Result<(), String> {
        let Some(mut session) = self.find_session(session_id)? else {
            return Err(format!("Session not found: {session_id}"));
        };
        session.preferred_model = preferred_model
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        session.preferred_mode = preferred_mode
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        session.preferred_effort = preferred_effort.filter(|v| v.is_finite());
        session.preferred_effort_id = preferred_effort_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        session.preferred_always_approve = preferred_always_approve;
        session.last_active_at = now_string();
        self.save_session(&session)
    }

    pub fn update_session_label(
        &self,
        session_id: &str,
        label: &str,
        label_source: Option<&str>,
    ) -> Result<(), String> {
        let Some(mut session) = self.find_session(session_id)? else {
            return Err(format!("Session not found: {session_id}"));
        };
        let trimmed = label.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        // Keep labels short for the left shelf.
        let next = if trimmed.chars().count() > 48 {
            let short: String = trimmed.chars().take(46).collect();
            format!("{short}…")
        } else {
            trimmed.to_string()
        };
        session.label = next;
        let source = match label_source {
            Some("default") | Some("user") | Some("agent") | Some("manual") => {
                label_source.unwrap_or("manual")
            }
            _ => "manual",
        };
        session.label_source = Some(source.to_string());
        session.last_active_at = now_string();
        self.save_session(&session)
    }

    fn remember_transcript_path(&self, session: &Session) {
        if session.transcript_path.is_empty() {
            return;
        }
        if let Ok(mut map) = self.transcript_paths.lock() {
            map.insert(session.id.clone(), PathBuf::from(&session.transcript_path));
        }
    }

    /// Resolve the JSONL path without rewriting the file. Cache hit is a
    /// HashMap lookup; miss scans indexes once and remembers.
    pub fn transcript_path_for(&self, session_id: &str) -> Result<PathBuf, String> {
        if let Ok(map) = self.transcript_paths.lock() {
            if let Some(path) = map.get(session_id) {
                return Ok(path.clone());
            }
        }
        let session = self
            .find_session(session_id)?
            .ok_or_else(|| format!("Session not found: {session_id}"))?;
        self.remember_transcript_path(&session);
        Ok(PathBuf::from(session.transcript_path))
    }

    /// Disk write only — callers should drop the storage lock first.
    pub fn write_transcript_file(
        path: &Path,
        events: &[serde_json::Value],
    ) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Create transcript dir failed: {error}"))?;
        }
        let mut body = String::new();
        for event in events {
            let line = serde_json::to_string(event)
                .map_err(|error| format!("Serialize transcript event failed: {error}"))?;
            body.push_str(&line);
            body.push('\n');
        }
        fs::write(path, body).map_err(|error| format!("Write transcript failed: {error}"))
    }

    /// Rewrite Clean-view transcript as JSONL (one SessionEvent per line).
    pub fn write_transcript(
        &self,
        session_id: &str,
        events: &[serde_json::Value],
    ) -> Result<(), String> {
        let path = self.transcript_path_for(session_id)?;
        Self::write_transcript_file(&path, events)
    }

    pub fn load_transcript(&self, session_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let session = self
            .find_session(session_id)?
            .ok_or_else(|| format!("Session not found: {session_id}"))?;
        let path = PathBuf::from(&session.transcript_path);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("Read transcript failed: {error}"))?;
        let mut events = Vec::new();
        for (index, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(value) => events.push(value),
                Err(error) => {
                    // Skip corrupt lines rather than failing the whole load.
                    let _ = error;
                    let _ = index;
                }
            }
        }
        Ok(events)
    }

    /// Search project/session labels and transcript text. Returns matching session ids.
    pub fn search_sessions(&self, query: &str) -> Result<Vec<String>, String> {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let mut hits = Vec::new();
        for project in self.list_projects()? {
            for session in self.list_sessions_all(&project.id)? {
                // Skip @-delegate children — no independent entry in search.
                if session
                    .parent_session_id
                    .as_ref()
                    .map(|p| !p.is_empty())
                    .unwrap_or(false)
                {
                    continue;
                }
                let mut matched = session.label.to_ascii_lowercase().contains(&q)
                    || session.agent_id.to_ascii_lowercase().contains(&q)
                    || project.name.to_ascii_lowercase().contains(&q);
                if !matched {
                    let path = PathBuf::from(&session.transcript_path);
                    if path.exists() {
                        if let Ok(text) = fs::read_to_string(&path) {
                            matched = text.to_ascii_lowercase().contains(&q);
                        }
                    }
                }
                if matched {
                    hits.push(session.id);
                }
            }
        }
        for session in self.read_chat_sessions_all()? {
            if session
                .parent_session_id
                .as_ref()
                .map(|p| !p.is_empty())
                .unwrap_or(false)
            {
                continue;
            }
            let mut matched = session.label.to_ascii_lowercase().contains(&q)
                || session.agent_id.to_ascii_lowercase().contains(&q)
                || session.cwd.to_ascii_lowercase().contains(&q)
                || "聊天".contains(&q);
            if !matched {
                let path = PathBuf::from(&session.transcript_path);
                if path.exists() {
                    if let Ok(text) = fs::read_to_string(&path) {
                        matched = text.to_ascii_lowercase().contains(&q);
                    }
                }
            }
            if matched {
                hits.push(session.id);
            }
        }
        Ok(hits)
    }

    /// Root directory used for Chat metadata and transcripts. This lives under
    /// the global app directory, never under a user's workspace.
    pub fn chat_data_dir(&self) -> PathBuf {
        self.global_dir.join("chats")
    }

    fn chat_dir(&self) -> PathBuf {
        self.chat_data_dir()
    }

    fn ensure_chat_dirs(&self) -> Result<(), String> {
        for sub in ["sessions", "transcripts", "handoff"] {
            fs::create_dir_all(self.chat_dir().join(sub))
                .map_err(|error| format!("Create global chats/{sub} failed: {error}"))?;
        }
        Ok(())
    }

    fn read_chat_sessions_all(&self) -> Result<Vec<Session>, String> {
        let file = self.chat_dir().join("sessions").join("index.json");
        if !file.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&file)
            .map_err(|error| format!("Read chat sessions failed: {error}"))?;
        if content.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str(&content)
            .map_err(|error| format!("Parse chat sessions failed: {error}"))
    }

    fn write_chat_sessions(&self, sessions: &[Session]) -> Result<(), String> {
        self.ensure_chat_dirs()?;
        let content = serde_json::to_string_pretty(sessions)
            .map_err(|error| format!("Serialize chat sessions failed: {error}"))?;
        fs::write(
            self.chat_dir().join("sessions").join("index.json"),
            format!("{content}\n"),
        )
        .map_err(|error| format!("Write chat sessions failed: {error}"))
    }

    fn chat_session_log_path(&self, session_id: &str) -> PathBuf {
        self.chat_dir()
            .join("sessions")
            .join(format!("{}.raw.log", safe_session_id(session_id)))
    }

    /// Migrate the previous implementation's fake `project-chat` row. That
    /// implementation stored Chat under `<cwd>/.marionette`; preserve its
    /// transcripts and then remove only that generated marker when it contains
    /// Chat data exclusively.
    fn migrate_legacy_chat_project(&self) -> Result<(), String> {
        if !self.projects_file.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(&self.projects_file)
            .map_err(|error| format!("Read projects failed: {error}"))?;
        if content.trim().is_empty() {
            return Ok(());
        }
        let mut projects: Vec<Project> = serde_json::from_str(&content)
            .map_err(|error| format!("Parse projects failed: {error}"))?;
        let Some(legacy) = projects
            .iter()
            .find(|project| project.id == crate::app_paths::CHAT_PROJECT_ID)
            .cloned()
        else {
            return Ok(());
        };

        let legacy_root = PathBuf::from(&legacy.root_path);
        let legacy_index = sessions_file(&legacy_root);
        let legacy_sessions = if legacy_index.exists() {
            let raw = fs::read_to_string(&legacy_index)
                .map_err(|error| format!("Read legacy chat sessions failed: {error}"))?;
            if raw.trim().is_empty() {
                Vec::new()
            } else {
                serde_json::from_str::<Vec<Session>>(&raw)
                    .map_err(|error| format!("Parse legacy chat sessions failed: {error}"))?
            }
        } else {
            Vec::new()
        };

        if !legacy_sessions.is_empty() {
            self.ensure_chat_dirs()?;
            let legacy_latest_handoff = legacy_root
                .join(crate::app_paths::DIR_NAME)
                .join("handoff.md");
            let target_latest_handoff = self.chat_dir().join("handoff.md");
            if !copy_if_present(
                &legacy_latest_handoff.to_string_lossy(),
                &target_latest_handoff,
            ) {
                // Keep the legacy row/marker so a later launch can retry.
                return Ok(());
            }
            let mut chats = self.read_chat_sessions_all()?;
            for mut session in legacy_sessions.iter().cloned() {
                let id = session.id.clone();
                let safe_id = safe_session_id(&id);
                let target_raw = self.chat_session_log_path(&id);
                let target_transcript = self
                    .chat_dir()
                    .join("transcripts")
                    .join(format!("{safe_id}.jsonl"));
                let target_handoff = self
                    .chat_dir()
                    .join("handoff")
                    .join(format!("{safe_id}.md"));
                // A failed copy must leave the legacy project and marker in
                // place so the next launch can retry without losing history.
                if !copy_if_present(&session.raw_log_path, &target_raw)
                    || !copy_if_present(&session.transcript_path, &target_transcript)
                    || !copy_if_present(&session.handoff_path, &target_handoff)
                {
                    return Ok(());
                }
                session.project_id = crate::app_paths::CHAT_PROJECT_ID.to_string();
                if session.cwd.trim().is_empty() {
                    session.cwd = legacy.root_path.clone();
                }
                session.raw_log_path = target_raw.to_string_lossy().to_string();
                session.transcript_path = target_transcript.to_string_lossy().to_string();
                session.handoff_path = target_handoff.to_string_lossy().to_string();
                chats.retain(|current| current.id != id);
                chats.push(session);
            }
            // Preserve the old index's newest-first order after merging.
            chats.sort_by(|a, b| session_recency_key(b).cmp(&session_recency_key(a)));
            self.write_chat_sessions(&chats)?;
        }

        projects.retain(|project| project.id != crate::app_paths::CHAT_PROJECT_ID);
        self.write_projects(&projects)?;
        if legacy_index.exists() {
            remove_legacy_chat_layout(&legacy_root, &legacy_sessions);
        }
        Ok(())
    }

    fn project_by_id(&self, project_id: &str) -> Result<Project, String> {
        self.list_projects()?
            .into_iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| format!("Unknown project: {project_id}"))
    }

    fn write_sessions(&self, project_path: &Path, sessions: &[Session]) -> Result<(), String> {
        let file = sessions_file(project_path);
        let content = serde_json::to_string_pretty(sessions)
            .map_err(|error| format!("Serialize sessions failed: {error}"))?;
        fs::write(file, format!("{content}\n"))
            .map_err(|error| format!("Write sessions failed: {error}"))
    }

    fn write_projects(&self, projects: &[Project]) -> Result<(), String> {
        let content = serde_json::to_string_pretty(projects)
            .map_err(|error| format!("Serialize projects failed: {error}"))?;
        fs::write(&self.projects_file, format!("{content}\n"))
            .map_err(|error| format!("Write projects failed: {error}"))
    }

    fn ensure_project_dirs(&self, project_path: &Path) -> Result<(), String> {
        crate::app_paths::ensure_project_layout(project_path).map(|_| ())
    }

    #[allow(dead_code)]
    pub fn global_dir(&self) -> &Path {
        &self.global_dir
    }
}

fn sessions_file(project_path: &Path) -> PathBuf {
    crate::app_paths::project_dir(project_path)
        .join("sessions")
        .join("index.json")
}

fn session_log_path(project_path: &Path, session_id: &str) -> PathBuf {
    let safe_id = safe_session_id(session_id);
    crate::app_paths::project_dir(project_path)
        .join("sessions")
        .join(format!("{safe_id}.raw.log"))
}

fn safe_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>()
}

fn copy_if_present(source: &str, target: &Path) -> bool {
    let source = Path::new(source);
    if !source.is_file() {
        return true;
    }
    if let Some(parent) = target.parent() {
        if fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    fs::copy(source, target).is_ok()
}

fn session_recency_key(session: &Session) -> u128 {
    session
        .last_active_at
        .parse::<u128>()
        .unwrap_or_default()
}

fn remove_legacy_chat_layout(root: &Path, sessions: &[Session]) {
    let marker = root.join(crate::app_paths::DIR_NAME);
    let index = marker.join("sessions").join("index.json");
    if !index.is_file() || sessions.is_empty() {
        return;
    }
    if !sessions
        .iter()
        .all(|session| session.project_id == crate::app_paths::CHAT_PROJECT_ID)
    {
        return;
    }

    // Only remove a marker that contains exactly the files the interrupted
    // Chat implementation could have created. A real project may share the
    // same folder and must never lose its metadata during migration.
    let expected_ids: std::collections::HashSet<String> = sessions
        .iter()
        .map(|session| safe_session_id(&session.id))
        .collect();
    let has_only_files = |dir: &Path, allowed: &dyn Fn(&str) -> bool| {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        entries.filter_map(Result::ok).all(|entry| {
            let Ok(file_type) = entry.file_type() else {
                return false;
            };
            file_type.is_file()
                && allowed(entry.file_name().to_string_lossy().as_ref())
        })
    };
    if !has_only_files(&marker.join("sessions"), &|name| {
        name == "index.json"
            || name
                .strip_suffix(".raw.log")
                .map(|id| expected_ids.contains(id))
                .unwrap_or(false)
    }) {
        return;
    }
    if !has_only_files(&marker.join("transcripts"), &|name| {
        name.strip_suffix(".jsonl")
            .map(|id| expected_ids.contains(id))
            .unwrap_or(false)
    }) {
        return;
    }
    if !has_only_files(&marker.join("handoff"), &|name| {
        name.strip_suffix(".md")
            .map(|id| expected_ids.contains(id))
            .unwrap_or(false)
    }) {
        return;
    }
    let Ok(entries) = fs::read_dir(&marker) else {
        return;
    };
    let top_level_is_safe = entries.filter_map(Result::ok).all(|entry| {
        let name = entry.file_name();
        let allowed_dir = matches!(
            name.to_string_lossy().as_ref(),
            "sessions" | "transcripts" | "handoff"
        );
        let allowed_file = name == "handoff.md";
        allowed_dir || allowed_file
    });
    if top_level_is_safe {
        let _ = fs::remove_dir_all(marker);
    }
}

fn project_id(path: &str) -> String {
    let mut hasher = DefaultHasher::new();
    path.to_lowercase().hash(&mut hasher);
    format!("project-{:x}", hasher.finish())
}

fn now_string() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    millis.to_string()
}

#[cfg(test)]
mod tests {
    use super::{now_string, StorageService};
    use crate::models::Project;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root() -> std::path::PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "marionette-storage-test-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn add_project_initializes_and_persists_across_service_restart() {
        let root = test_root();
        let global_dir = root.join("global");
        let project_dir = root.join("workspace");
        fs::create_dir_all(&project_dir).unwrap();

        let service = StorageService::from_global_dir(global_dir.clone()).unwrap();
        let project = service
            .add_project(project_dir.to_string_lossy().to_string())
            .unwrap();

        assert_eq!(service.list_projects().unwrap().len(), 1);
        assert!(project_dir.join(".marionette/sessions").is_dir());
        assert!(project_dir.join(".marionette/transcripts").is_dir());

        let restarted = StorageService::from_global_dir(global_dir).unwrap();
        let restored = restarted.list_projects().unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, project.id);
        assert_eq!(restored[0].root_path, project.root_path);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn projects_can_be_reordered() {
        let root = test_root();
        let global_dir = root.join("global");
        let a_dir = root.join("workspace-a");
        let b_dir = root.join("workspace-b");
        let c_dir = root.join("workspace-c");
        fs::create_dir_all(&a_dir).unwrap();
        fs::create_dir_all(&b_dir).unwrap();
        fs::create_dir_all(&c_dir).unwrap();

        let service = StorageService::from_global_dir(global_dir.clone()).unwrap();
        let a = service
            .add_project(a_dir.to_string_lossy().to_string())
            .unwrap();
        let b = service
            .add_project(b_dir.to_string_lossy().to_string())
            .unwrap();
        let c = service
            .add_project(c_dir.to_string_lossy().to_string())
            .unwrap();

        let reordered = service
            .reorder_projects(&[c.id.clone(), a.id.clone(), b.id.clone()])
            .unwrap();
        assert_eq!(
            reordered.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec![c.id.as_str(), a.id.as_str(), b.id.as_str()]
        );

        let restarted = StorageService::from_global_dir(global_dir).unwrap();
        let restored = restarted.list_projects().unwrap();
        assert_eq!(
            restored.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec![c.id.as_str(), a.id.as_str(), b.id.as_str()]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sessions_persist_and_can_be_loaded_after_restart() {
        let root = test_root();
        let global_dir = root.join("global");
        let project_dir = root.join("workspace");
        fs::create_dir_all(&project_dir).unwrap();

        let service = StorageService::from_global_dir(global_dir.clone()).unwrap();
        let project = service
            .add_project(project_dir.to_string_lossy().to_string())
            .unwrap();
        let session = service
            .create_session(&project.id, "codex".to_string(), "M4 session".to_string())
            .unwrap();

        let restarted = StorageService::from_global_dir(global_dir).unwrap();
        let restored = restarted.list_sessions(&project.id).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, session.id);
        assert!(Path::new(&restored[0].raw_log_path)
            .parent()
            .unwrap()
            .is_dir());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chat_sessions_are_global_and_do_not_mark_the_working_folder() {
        let root = test_root();
        let global_dir = root.join("global");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();

        let service = StorageService::from_global_dir(global_dir.clone()).unwrap();
        let session = service
            .create_chat_session("opencode".to_string(), "Chat".to_string(), &workspace.to_string_lossy())
            .unwrap();

        assert!(service.list_projects().unwrap().is_empty());
        assert_eq!(service.list_chat_sessions().unwrap()[0].id, session.id);
        assert_eq!(Path::new(&session.cwd), workspace.canonicalize().unwrap());
        assert!(!workspace.join(".marionette").exists());
        assert!(global_dir.join("chats/sessions/index.json").is_file());

        let restarted = StorageService::from_global_dir(global_dir).unwrap();
        assert_eq!(restarted.list_chat_sessions().unwrap().len(), 1);
        assert!(!workspace.join(".marionette").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_fake_chat_project_is_migrated_without_losing_transcript() {
        let root = test_root();
        let global_dir = root.join("global");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();

        let service = StorageService::from_global_dir(global_dir.clone()).unwrap();
        let old_project = service
            .add_project(workspace.to_string_lossy().to_string())
            .unwrap();
        let mut legacy_session = service
            .create_session(&old_project.id, "opencode".to_string(), "Legacy chat".to_string())
            .unwrap();
        legacy_session.project_id = crate::app_paths::CHAT_PROJECT_ID.to_string();
        fs::write(
            &legacy_session.transcript_path,
            "{\"type\":\"user_message\",\"text\":\"kept\"}\n",
        )
        .unwrap();
        let legacy_index = workspace.join(".marionette/sessions/index.json");
        fs::write(
            &legacy_index,
            serde_json::to_string_pretty(&vec![legacy_session.clone()]).unwrap(),
        )
        .unwrap();
        fs::write(workspace.join(".marionette/handoff.md"), "latest").unwrap();
        let fake_project = Project {
            id: crate::app_paths::CHAT_PROJECT_ID.to_string(),
            name: "聊天".to_string(),
            root_path: workspace.to_string_lossy().to_string(),
            created_at: now_string(),
            last_opened_at: now_string(),
        };
        fs::write(
            global_dir.join("projects.json"),
            serde_json::to_string_pretty(&vec![fake_project]).unwrap(),
        )
        .unwrap();

        let restarted = StorageService::from_global_dir(global_dir.clone()).unwrap();
        let chats = restarted.list_chat_sessions().unwrap();
        assert_eq!(chats.len(), 1);
        assert_eq!(chats[0].label, "Legacy chat");
        assert_eq!(restarted.list_projects().unwrap().len(), 0);
        assert!(global_dir.join("chats/transcripts").join(format!("{}.jsonl", chats[0].id)).is_file());
        assert_eq!(
            fs::read_to_string(global_dir.join("chats/handoff.md")).unwrap(),
            "latest"
        );
        assert!(!workspace.join(".marionette").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_running_status_is_healed_on_read_and_on_disk() {
        let root = test_root();
        let global_dir = root.join("global");
        let project_dir = root.join("workspace");
        fs::create_dir_all(&project_dir).unwrap();

        let service = StorageService::from_global_dir(global_dir.clone()).unwrap();
        let project = service
            .add_project(project_dir.to_string_lossy().to_string())
            .unwrap();
        let session = service
            .create_session(&project.id, "opencode".to_string(), "M4".to_string())
            .unwrap();
        service.update_session_status(&session.id, "running").unwrap();

        // Restart: nothing is live, so the dialog must come back idle — and the
        // file must be repaired so the next reader sees the same truth.
        let restarted = StorageService::from_global_dir(global_dir).unwrap();
        let healed = restarted.list_sessions_healed(&project.id, |_| false).unwrap();
        assert_eq!(healed[0].status, "exited");
        assert!(healed[0].exited_at.is_some());
        assert_eq!(
            restarted.list_sessions(&project.id).unwrap()[0].status,
            "exited"
        );

        // A session this process really owns keeps its live status.
        restarted.update_session_status(&session.id, "running").unwrap();
        let live = restarted.list_sessions_healed(&project.id, |_| true).unwrap();
        assert_eq!(live[0].status, "running");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_project_removes_from_list_only() {
        let root = test_root();
        let global_dir = root.join("global");
        let project_dir = root.join("workspace");
        fs::create_dir_all(&project_dir).unwrap();

        let service = StorageService::from_global_dir(global_dir.clone()).unwrap();
        let project = service
            .add_project(project_dir.to_string_lossy().to_string())
            .unwrap();
        assert_eq!(service.list_projects().unwrap().len(), 1);

        service.delete_project(&project.id).unwrap();
        assert!(service.list_projects().unwrap().is_empty());
        // Workspace folder and .marionette data stay on disk.
        assert!(project_dir.is_dir());
        assert!(project_dir.join(".marionette").is_dir());

        fs::remove_dir_all(root).unwrap();
    }
}
