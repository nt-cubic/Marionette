//! Key whitelist (codeg `keys.rs` rules, Windows column): what a shared
//! window/app/screen may be asked to press. The clipboard is the user's —
//! paste is refused unless it is still exactly what the agent itself wrote
//! (matched by the clipboard sequence number at press time).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Window,
    App,
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    /// Primary + v: refused with a "paste" reason (the exception lives in the
    /// caller, which knows whether the clipboard is still the agent's).
    Paste,
    Denied,
}

/// Default blocklist of process image names (lowercase) whose windows can
/// never be shared: the OS's own password surfaces and the common password
/// managers. A screen capture paints over these too.
pub const DEFAULT_BLOCKLIST: &[&str] = &[
    // Windows' own credential surfaces
    "systemsettings.exe",
    "credentialuibroker.exe",
    "consent.exe",
    // Password managers
    "1password.exe",
    "bitwarden.exe",
    "keepass.exe",
    "keepassxc.exe",
    "enpass.exe",
    "protonpass.exe",
];

/// Split `"Control+Shift+k"` into (modifiers, key). Modifier names are
/// normalized to ctrl/alt/shift/win; the key is lowercased.
pub fn parse_chord(chord: &str) -> (Vec<String>, String) {
    let mut modifiers = Vec::new();
    let mut key = String::new();
    for part in chord.split('+') {
        let lower = part.trim().to_ascii_lowercase();
        match lower.as_str() {
            "" => {}
            "ctrl" | "control" => modifiers.push("ctrl".to_string()),
            "alt" | "option" => modifiers.push("alt".to_string()),
            "shift" => modifiers.push("shift".to_string()),
            "win" | "windows" | "meta" | "super" | "cmd" | "command" => modifiers.push("win".to_string()),
            _ => key = lower,
        }
    }
    (modifiers, key)
}

fn has(list: &[String], name: &str) -> bool {
    list.iter().any(|m| m == name)
}

fn is_function_key(key: &str) -> bool {
    matches!(key.strip_prefix('f'), Some(digits) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
}

fn is_arrow(key: &str) -> bool {
    matches!(key, "up" | "down" | "left" | "right" | "arrowup" | "arrowdown" | "arrowleft" | "arrowright")
}

/// Window-level rule: bare keys go (except F-keys and shift+delete — Windows
/// Explorer's permanent delete); with the menu modifier (Ctrl here) only the
/// editing/navigation chords an app owns.
pub fn classify_window(modifiers: &[String], key: &str) -> Verdict {
    // Paste is checked first at every scope.
    if has(modifiers, "ctrl") && key == "v" {
        return Verdict::Paste;
    }
    let bare = modifiers.is_empty() || (modifiers.len() == 1 && has(modifiers, "shift"));
    if bare {
        // shift+delete = Explorer's permanent delete; F-keys change hardware
        // and OS state on too many laptops.
        if is_function_key(key) {
            return Verdict::Denied;
        }
        if has(modifiers, "shift") && key == "delete" {
            return Verdict::Denied;
        }
        return Verdict::Allowed;
    }
    if has(modifiers, "ctrl") {
        let plain = modifiers.len() == 1;
        let no_shift = plain || (modifiers.len() == 2 && has(modifiers, "shift") == false);
        let _ = no_shift;
        match key {
            // select/copy/cut/find — no shift variants
            "a" | "c" | "x" | "f" if plain => return Verdict::Allowed,
            // undo/redo in either width
            "z" | "g" | "y" => return Verdict::Allowed,
            // navigation / word-wise editing
            "home" | "end" | "backspace" | "delete" => return Verdict::Allowed,
            _ if is_arrow(key) => return Verdict::Allowed,
            _ => return Verdict::Denied,
        }
    }
    Verdict::Denied
}

/// Desktop-level chords an APP may not own (they belong to the shell/OS):
/// any Win key, Alt+Tab / Alt+Esc, Ctrl+Esc, Ctrl+Alt+anything.
fn desktop_chord(modifiers: &[String], key: &str) -> bool {
    if has(modifiers, "win") {
        return true;
    }
    if has(modifiers, "alt") && !has(modifiers, "ctrl") && (key == "tab" || key == "esc") {
        return true;
    }
    if has(modifiers, "ctrl") && !has(modifiers, "alt") && key == "esc" {
        return true;
    }
    if has(modifiers, "ctrl") && has(modifiers, "alt") {
        return true;
    }
    false
}

/// Chords not even a SCREEN share may press: lock, task view, the secure
/// attention sequence family.
fn never_chord(modifiers: &[String], key: &str) -> bool {
    if has(modifiers, "win") && matches!(key, "l" | "tab") {
        return true;
    }
    if has(modifiers, "ctrl") && has(modifiers, "alt") && matches!(key, "delete" | "backspace" | "tab") {
        return true;
    }
    false
}

pub fn classify(scope: Scope, chord: &str) -> Verdict {
    let (modifiers, key) = parse_chord(chord);
    if key.is_empty() {
        return Verdict::Denied;
    }
    if has(&modifiers, "ctrl") && key == "v" {
        return Verdict::Paste;
    }
    match scope {
        Scope::Window => classify_window(&modifiers, &key),
        Scope::App => {
            if desktop_chord(&modifiers, &key) {
                Verdict::Denied
            } else {
                Verdict::Allowed
            }
        }
        Scope::Screen => {
            if never_chord(&modifiers, &key) {
                Verdict::Denied
            } else {
                Verdict::Allowed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_keys_pass_except_functions_and_shift_delete() {
        assert_eq!(classify(Scope::Window, "a"), Verdict::Allowed);
        assert_eq!(classify(Scope::Window, "Delete"), Verdict::Allowed);
        assert_eq!(classify(Scope::Window, "Shift+Delete"), Verdict::Denied);
        assert_eq!(classify(Scope::Window, "F5"), Verdict::Denied);
        assert_eq!(classify(Scope::Window, "F12"), Verdict::Denied);
    }

    #[test]
    fn window_editing_chords_pass_the_rest_do_not() {
        assert_eq!(classify(Scope::Window, "Control+a"), Verdict::Allowed);
        assert_eq!(classify(Scope::Window, "Control+Shift+a"), Verdict::Denied);
        assert_eq!(classify(Scope::Window, "Control+z"), Verdict::Allowed);
        assert_eq!(classify(Scope::Window, "Control+y"), Verdict::Allowed);
        assert_eq!(classify(Scope::Window, "Control+ArrowLeft"), Verdict::Allowed);
        assert_eq!(classify(Scope::Window, "Control+p"), Verdict::Denied);
        assert_eq!(classify(Scope::Window, "Alt+F4"), Verdict::Denied);
    }

    #[test]
    fn paste_is_refused_everywhere() {
        for scope in [Scope::Window, Scope::App, Scope::Screen] {
            assert_eq!(classify(scope, "Control+v"), Verdict::Paste);
        }
    }

    #[test]
    fn app_scope_blocks_desktop_chords() {
        assert_eq!(classify(Scope::App, "Control+p"), Verdict::Allowed);
        assert_eq!(classify(Scope::App, "Win+e"), Verdict::Denied);
        assert_eq!(classify(Scope::App, "Alt+Tab"), Verdict::Denied);
        assert_eq!(classify(Scope::App, "Control+Esc"), Verdict::Denied);
        assert_eq!(classify(Scope::App, "Control+Alt+Delete"), Verdict::Denied);
    }

    #[test]
    fn screen_scope_blocks_lock_and_secure_attention_only() {
        assert_eq!(classify(Scope::Screen, "Win+l"), Verdict::Denied);
        assert_eq!(classify(Scope::Screen, "Win+Tab"), Verdict::Denied);
        assert_eq!(classify(Scope::Screen, "Control+Alt+Delete"), Verdict::Denied);
        assert_eq!(classify(Scope::Screen, "Control+p"), Verdict::Allowed);
        assert_eq!(classify(Scope::Screen, "Alt+Tab"), Verdict::Allowed);
    }

    #[test]
    fn chord_parsing_normalizes_aliases() {
        let (modifiers, key) = parse_chord("Control+Shift+K");
        assert_eq!(modifiers, vec!["ctrl".to_string(), "shift".to_string()]);
        assert_eq!(key, "k");
    }
}
