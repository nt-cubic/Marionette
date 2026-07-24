use crate::models::{Project, Session};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct StorageService {
    global_dir: PathBuf,
    projects_file: PathBuf,
}

impl StorageService {
    pub fn new() -> Result<Self, String> {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .ok_or_else(|| "Unable to determine the user home directory".to_string())?;
        Self::from_global_dir(PathBuf::from(home).join(".agentshell"))
    }

    fn from_global_dir(global_dir: PathBuf) -> Result<Self, String> {
        let projects_file = global_dir.join("projects.json");
        fs::create_dir_all(&global_dir)
            .map_err(|error| format!("Create storage directory failed: {error}"))?;
        Ok(Self {
            global_dir,
            projects_file,
        })
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

        serde_json::from_str(&content).map_err(|error| format!("Parse projects failed: {error}"))
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

    pub fn list_sessions(&self, project_id: &str) -> Result<Vec<Session>, String> {
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

    pub fn create_session(
        &self,
        project_id: &str,
        agent_id: String,
        label: String,
    ) -> Result<Session, String> {
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
            cwd: project.root_path.clone(),
            status: "exited".to_string(),
            process_id: None,
            pty_id: None,
            started_at: String::new(),
            last_active_at: now,
            exited_at: None,
            exit_code: None,
            raw_log_path: raw_log_path.to_string_lossy().to_string(),
            transcript_path: project_path
                .join(".agentshell/transcripts")
                .join(format!("{id}.jsonl"))
                .to_string_lossy()
                .to_string(),
            handoff_path: project_path
                .join(".agentshell/handoff")
                .join(format!("{id}.md"))
                .to_string_lossy()
                .to_string(),
            // Product primary surface is Clean View (Raw is always available as toggle).
            view_mode: "clean".to_string(),
            preferred_model: None,
            preferred_mode: None,
            preferred_effort: None,
            preferred_effort_id: None,
        };
        // Prepend so newest dialogs appear at the top of the project shelf.
        let mut sessions = self.list_sessions(project_id)?;
        sessions.retain(|s| s.id != session.id);
        sessions.insert(0, session.clone());
        self.write_sessions(project_path, &sessions)?;
        Ok(session)
    }

    pub fn delete_session(&self, project_id: &str, session_id: &str) -> Result<(), String> {
        let project = self.project_by_id(project_id)?;
        let mut sessions = self.list_sessions(project_id)?;
        sessions.retain(|session| session.id != session_id);
        self.write_sessions(Path::new(&project.root_path), &sessions)
    }

    pub fn save_session(&self, session: &Session) -> Result<(), String> {
        let project = self.project_by_id(&session.project_id)?;
        let project_path = Path::new(&project.root_path);
        self.ensure_project_dirs(project_path)?;
        let mut sessions = self.list_sessions(&session.project_id)?;
        if let Some(existing) = sessions.iter_mut().find(|current| current.id == session.id) {
            *existing = session.clone();
        } else {
            sessions.push(session.clone());
        }
        self.write_sessions(project_path, &sessions)
    }

    pub fn find_session(&self, session_id: &str) -> Result<Option<Session>, String> {
        for project in self.list_projects()? {
            if let Some(session) = self
                .list_sessions(&project.id)?
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
        self.save_session(&session)
    }

    /// Persist Composer model / mode / effort for this dialog (SSOT on disk).
    pub fn update_session_prefs(
        &self,
        session_id: &str,
        preferred_model: Option<String>,
        preferred_mode: Option<String>,
        preferred_effort: Option<f64>,
        preferred_effort_id: Option<String>,
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
        session.last_active_at = now_string();
        self.save_session(&session)
    }

    pub fn update_session_label(&self, session_id: &str, label: &str) -> Result<(), String> {
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
        session.last_active_at = now_string();
        self.save_session(&session)
    }

    /// Rewrite Clean-view transcript as JSONL (one SessionEvent per line).
    pub fn write_transcript(
        &self,
        session_id: &str,
        events: &[serde_json::Value],
    ) -> Result<(), String> {
        let session = self
            .find_session(session_id)?
            .ok_or_else(|| format!("Session not found: {session_id}"))?;
        let path = PathBuf::from(&session.transcript_path);
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
        fs::write(&path, body).map_err(|error| format!("Write transcript failed: {error}"))
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
            for session in self.list_sessions(&project.id)? {
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
        Ok(hits)
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
        let project_dir = project_path.join(".agentshell");
        fs::create_dir_all(project_dir.join("sessions"))
            .and_then(|_| fs::create_dir_all(project_dir.join("transcripts")))
            .map_err(|error| format!("Create project storage failed: {error}"))
    }

    #[allow(dead_code)]
    pub fn global_dir(&self) -> &Path {
        &self.global_dir
    }
}

fn sessions_file(project_path: &Path) -> PathBuf {
    project_path
        .join(".agentshell")
        .join("sessions")
        .join("index.json")
}

fn session_log_path(project_path: &Path, session_id: &str) -> PathBuf {
    let safe_id = session_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    project_path
        .join(".agentshell")
        .join("sessions")
        .join(format!("{safe_id}.raw.log"))
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
    use super::StorageService;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root() -> std::path::PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "agentshell-storage-test-{}-{}-{}",
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
        assert!(project_dir.join(".agentshell/sessions").is_dir());
        assert!(project_dir.join(".agentshell/transcripts").is_dir());

        let restarted = StorageService::from_global_dir(global_dir).unwrap();
        let restored = restarted.list_projects().unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, project.id);
        assert_eq!(restored[0].root_path, project.root_path);

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
        // Workspace folder and .agentshell data stay on disk.
        assert!(project_dir.is_dir());
        assert!(project_dir.join(".agentshell").is_dir());

        fs::remove_dir_all(root).unwrap();
    }
}
