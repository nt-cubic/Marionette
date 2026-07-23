use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct SessionManager {
    live: Arc<Mutex<HashMap<String, String>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_started(&self, session_id: &str) -> Result<(), String> {
        let mut live = self
            .live
            .lock()
            .map_err(|_| "Session manager lock poisoned".to_string())?;
        live.insert(session_id.to_string(), "running".to_string());
        Ok(())
    }

    pub fn mark_exited(&self, session_id: &str) -> Result<(), String> {
        let mut live = self
            .live
            .lock()
            .map_err(|_| "Session manager lock poisoned".to_string())?;
        live.remove(session_id);
        Ok(())
    }

    pub fn is_live(&self, session_id: &str) -> Result<bool, String> {
        let live = self
            .live
            .lock()
            .map_err(|_| "Session manager lock poisoned".to_string())?;
        Ok(live.contains_key(session_id))
    }
}
