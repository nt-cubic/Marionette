//! UI language for Rust-side user-visible strings (zh-CN / en).
//!
//! The frontend owns the pref (localStorage `marionette-language`, auto-detected
//! on first run) and pushes it here at launch and on every switch via the
//! `set_ui_locale` command; this module mirrors it to `~/.marionette/language`
//! so a headless path (Telegram bot replies before any window attached, the
//! native startup dialogs) sees the same language across restarts.
//!
//! `ts!` picks one of two inline literals at the call site — the zh string is
//! its own key, so converting a `format!(…)` is a purely local edit, and
//! `#[cfg(test)]` asserts on zh text stay green under the default language.
//! Log lines and comments stay zh; only strings a user can see go through it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Zh => "zh-CN",
            Lang::En => "en",
        }
    }

    /// Accepts exactly what the frontend store can send.
    fn parse(raw: &str) -> Option<Lang> {
        match raw.trim() {
            "zh-CN" => Some(Lang::Zh),
            "en" => Some(Lang::En),
            _ => None,
        }
    }
}

const LANG_ZH: u8 = 0;
const LANG_EN: u8 = 1;

static LANG: AtomicU8 = AtomicU8::new(LANG_ZH);

/// Read the persisted mirror once at startup; missing/unreadable = Zh.
pub fn init_from_disk() {
    let lang = language_file_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| Lang::parse(&raw))
        .unwrap_or(Lang::Zh);
    LANG.store(code(lang), Ordering::Relaxed);
}

pub fn current() -> Lang {
    if LANG.load(Ordering::Relaxed) == LANG_EN {
        Lang::En
    } else {
        Lang::Zh
    }
}

pub fn is_en() -> bool {
    current() == Lang::En
}

fn code(lang: Lang) -> u8 {
    if lang == Lang::En {
        LANG_EN
    } else {
        LANG_ZH
    }
}

fn language_file_path() -> Option<PathBuf> {
    crate::app_paths::global_dir().ok().map(|dir| dir.join("language"))
}

/// Activate + persist (best effort: a read-only dir leaves the in-memory value).
pub fn set(lang: Lang) {
    LANG.store(code(lang), Ordering::Relaxed);
    let Some(path) = language_file_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, lang.as_str());
}

#[tauri::command]
pub fn set_ui_locale(lang: String) -> Result<(), String> {
    let parsed = Lang::parse(&lang).ok_or_else(|| format!("unknown locale: {lang}"))?;
    set(parsed);
    Ok(())
}

/// Pick + format a user-visible string by the current UI language.
///
/// `ts!("无法初始化：{e}", "Failed to init: {e}", e = err)` formats the chosen
/// literal with the trailing args (both literals must share placeholder names).
/// Without trailing args it returns the chosen `&'static str` as a `String`.
/// Never wrap log lines, dev.log payloads or comments with this — user-visible
/// strings only.
#[macro_export]
macro_rules! ts {
    // Both arms format both literals so the compiler checks placeholders in
    // zh AND en — implicit captures (`{error}`) work with no trailing args.
    ($zh:literal, $en:literal) => {
        if $crate::loc::is_en() {
            format!($en)
        } else {
            format!($zh)
        }
    };
    ($zh:literal, $en:literal, $($arg:tt)+) => {
        if $crate::loc::is_en() {
            format!($en, $($arg)+)
        } else {
            format!($zh, $($arg)+)
        }
    };
}
