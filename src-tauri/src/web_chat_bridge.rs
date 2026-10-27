//! Ship the browser bridge inside the executable, including portable builds.
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, sync::Mutex};

const FILES: &[(&str, &str)] = &[
    ("bridge.mjs", include_str!("../../tools/web-chat-bridge/bridge.mjs")),
    ("attachments.mjs", include_str!("../../tools/web-chat-bridge/attachments.mjs")),
    ("rich-reply.mjs", include_str!("../../tools/web-chat-bridge/rich-reply.mjs")),
    ("markdown-reply.mjs", include_str!("../../tools/web-chat-bridge/markdown-reply.mjs")),
];
static EXTRACT_LOCK: Mutex<()> = Mutex::new(());

pub fn site(agent_id: &str) -> Option<&'static str> {
    match agent_id {
        "web-chatgpt" => Some("chatgpt"),
        "web-deepseek" => Some("deepseek"),
        _ => None,
    }
}

/// Content-addressed directories let old and new app versions run side by side.
pub fn ensure_extracted() -> Result<PathBuf, String> {
    let _guard = EXTRACT_LOCK.lock().map_err(|_| "Web bridge extraction lock poisoned")?;
    let mut hash = Sha256::new();
    for (name, content) in FILES {
        hash.update(name.as_bytes());
        hash.update(content.as_bytes());
    }
    let dir = crate::app_paths::global_dir()?
        .join("bundled/web-chat-bridge")
        .join(format!("{:x}", hash.finalize()));
    fs::create_dir_all(&dir).map_err(|e| format!("Prepare web bridge: {e}"))?;
    for (name, content) in FILES {
        let target = dir.join(name);
        if fs::read(&target).ok().as_deref() != Some(content.as_bytes()) {
            let temporary = dir.join(format!("{name}.{}.tmp", std::process::id()));
            fs::write(&temporary, content).map_err(|e| format!("Extract {name}: {e}"))?;
            fs::rename(&temporary, &target).map_err(|e| format!("Install {name}: {e}"))?;
        }
    }
    Ok(dir.join("bridge.mjs"))
}
