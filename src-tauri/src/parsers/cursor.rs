//! Cursor ACP sessions: `~/.cursor/acp-sessions/<uuid>/meta.json`.
//! Interactive `chats/<md5>/` protobuf stores are skipped (no protobuf decoder).

use super::path_norm::cwd_matches_project;
use super::util::{
    env_dir, home_join, json_str, read_json_file, read_subdirs, user_event,
};
use super::{AgentParser, ExternalConversation};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct CursorParser;

fn cursor_home() -> Option<PathBuf> {
    env_dir("CURSOR_CONFIG_DIR")
        .or_else(|| {
            env_dir("XDG_CONFIG_HOME").map(|p| p.join("cursor"))
        })
        .or_else(|| home_join(".cursor"))
}

impl AgentParser for CursorParser {
    fn source(&self) -> &'static str {
        "cursor"
    }
    fn agent_id(&self) -> &'static str {
        "cursor"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let Some(home) = cursor_home() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for dir in read_subdirs(&home.join("acp-sessions")) {
            let meta_path = dir.join("meta.json");
            let Some(v) = read_json_file(&meta_path) else {
                continue;
            };
            let cwd = json_str(&v, "cwd").unwrap_or("").to_string();
            if cwd.is_empty() || !cwd_matches_project(&cwd, project_root) {
                continue;
            }
            let native_id = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("cursor")
                .to_string();
            let title = json_str(&v, "title").unwrap_or("Cursor session").to_string();
            out.push(ExternalConversation {
                id: format!("cursor:{native_id}"),
                source: "cursor".into(),
                agent_id: "cursor".into(),
                title,
                cwd,
                started_at: json_str(&v, "createdAt").map(str::to_string),
                last_active_at: json_str(&v, "updatedAt").map(str::to_string),
                native_id,
                locator: dir.to_string_lossy().into_owned(),
            });
        }
        Ok(out)
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        let dir = Path::new(locator);
        let v = read_json_file(&dir.join("meta.json")).unwrap_or(Value::Null);
        let session_id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("cursor");
        let title = json_str(&v, "title").unwrap_or("Cursor session");
        // Protobuf blob store is not decoded here. Surface a single note so the
        // user sees *something* rather than a blank pane.
        Ok(vec![user_event(
            session_id,
            &format!("（Cursor 会话「{title}」的正文在 store.db 的 protobuf 里，当前只读回看还没解开那份格式。）"),
            json_str(&v, "createdAt").unwrap_or("cursor-0"),
        )])
    }
}
