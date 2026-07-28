use crate::session_manager::SessionManager;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::fs::{create_dir_all, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter};

const OUTPUT_EVENT: &str = "session-output";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyOutput {
    pub session_id: String,
    pub data: String,
    pub exited: bool,
    pub error: Option<String>,
}

struct PtySession {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send>>,
    log: Mutex<std::fs::File>,
}

#[derive(Clone, Default)]
pub struct PtyService {
    sessions: Arc<Mutex<HashMap<String, Arc<PtySession>>>>,
}

impl PtyService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(
        &self,
        app: AppHandle,
        session_id: String,
        cwd: String,
        command: String,
        args: Vec<String>,
        manager: SessionManager,
    ) -> Result<(), String> {
        if !Path::new(&cwd).is_dir() {
            return Err(format!("Terminal cwd is not a directory: {cwd}"));
        }

        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "PTY lock poisoned".to_string())?;
        if sessions.contains_key(&session_id) {
            return Ok(());
        }

        let log_dir = crate::app_paths::project_dir(Path::new(&cwd)).join("sessions");
        create_dir_all(&log_dir)
            .map_err(|error| format!("Create session log directory failed: {error}"))?;
        let log_name = session_id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join(format!("{log_name}.raw.log")))
            .map_err(|error| format!("Open session raw log failed: {error}"))?;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("Open PTY failed: {error}"))?;

        // Resolve npm shims / PATH (same rules as ACP spawn). Empty command → shell for debug.
        let mut builder = if command.trim().is_empty() {
            #[cfg(target_os = "windows")]
            {
                let mut builder = CommandBuilder::new("powershell.exe");
                builder.arg("-NoLogo");
                builder.arg("-NoProfile");
                builder
            }
            #[cfg(not(target_os = "windows"))]
            {
                CommandBuilder::new("sh")
            }
        } else {
            let resolved = crate::process_util::resolve_spawn_command(&command).map_err(|error| {
                format!("Start agent PTY failed: {error}")
            })?;
            let mut builder = CommandBuilder::new(&resolved.program);
            for arg in &resolved.prefix_args {
                builder.arg(arg);
            }
            for arg in &args {
                builder.arg(arg);
            }
            builder
        };
        builder.cwd(&cwd);

        // Keep npm global bin on PATH for nested tools (Windows).
        #[cfg(target_os = "windows")]
        {
            if let Some(appdata) = std::env::var_os("APPDATA") {
                let npm_bin = Path::new(&appdata).join("npm");
                if npm_bin.is_dir() {
                    let mut path = std::env::var_os("PATH").unwrap_or_default();
                    let npm_str = npm_bin.to_string_lossy();
                    if !path
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .contains(&npm_str.to_ascii_lowercase())
                    {
                        let mut new_path = npm_bin.into_os_string();
                        new_path.push(";");
                        new_path.push(&path);
                        path = new_path;
                    }
                    builder.env("PATH", path);
                }
            }
        }

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|error| {
                format!(
                    "Start agent PTY failed for `{command} {}`: {error}",
                    args.join(" ")
                )
            })?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("Clone PTY reader failed: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("Create PTY writer failed: {error}"))?;

        let session = Arc::new(PtySession {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            log: Mutex::new(log),
        });
        sessions.insert(session_id.clone(), Arc::clone(&session));
        drop(sessions);
        manager.mark_started(&session_id)?;

        let sessions = Arc::clone(&self.sessions);
        thread::spawn(move || read_output(app, session_id, session, reader, sessions, manager));
        Ok(())
    }

    pub fn write(&self, session_id: &str, data: &str) -> Result<(), String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| "PTY lock poisoned".to_string())?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "Terminal session is not running".to_string())?;
        let mut writer = session
            .writer
            .lock()
            .map_err(|_| "PTY writer lock poisoned".to_string())?;
        writer
            .write_all(data.as_bytes())
            .map_err(|error| format!("Write to PTY failed: {error}"))?;
        writer
            .flush()
            .map_err(|error| format!("Flush PTY failed: {error}"))
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| "PTY lock poisoned".to_string())?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "Terminal session is not running".to_string())?;
        let master = session
            .master
            .lock()
            .map_err(|_| "PTY master lock poisoned".to_string())?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("Resize PTY failed: {error}"))
    }

    pub fn stop(&self, session_id: &str) -> Result<(), String> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| "PTY lock poisoned".to_string())?
            .remove(session_id);
        if let Some(session) = session {
            let mut child = session
                .child
                .lock()
                .map_err(|_| "PTY child lock poisoned".to_string())?;
            child
                .kill()
                .map_err(|error| format!("Stop terminal failed: {error}"))?;
        }
        Ok(())
    }

    /// Kill every terminal we own — nothing should outlive the window.
    pub fn stop_all(&self) -> usize {
        let ids: Vec<String> = self
            .sessions
            .lock()
            .map(|sessions| sessions.keys().cloned().collect())
            .unwrap_or_default();
        let count = ids.len();
        for id in ids {
            let _ = self.stop(&id);
        }
        count
    }
}

fn read_output(
    app: AppHandle,
    session_id: String,
    session: Arc<PtySession>,
    mut reader: Box<dyn Read + Send>,
    sessions: Arc<Mutex<HashMap<String, Arc<PtySession>>>>,
    manager: SessionManager,
) {
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                let data = String::from_utf8_lossy(&buffer[..length]).into_owned();
                if let Ok(mut log) = session.log.lock() {
                    let _ = log.write_all(data.as_bytes());
                    let _ = log.flush();
                }
                let _ = app.emit(
                    OUTPUT_EVENT,
                    PtyOutput {
                        session_id: session_id.clone(),
                        data,
                        exited: false,
                        error: None,
                    },
                );
            }
            Err(error) => {
                let _ = app.emit(
                    OUTPUT_EVENT,
                    PtyOutput {
                        session_id: session_id.clone(),
                        data: String::new(),
                        exited: false,
                        error: Some(error.to_string()),
                    },
                );
                break;
            }
        }
    }

    if let Ok(mut child) = session.child.lock() {
        let _ = child.wait();
    }
    if let Ok(mut current) = sessions.lock() {
        current.remove(&session_id);
    }
    let _ = manager.mark_exited(&session_id);
    let _ = app.emit(
        OUTPUT_EVENT,
        PtyOutput {
            session_id,
            data: String::new(),
            exited: true,
            error: None,
        },
    );
}
