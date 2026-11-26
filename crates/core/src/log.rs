//! `mods\loader.log`: one line per event, `[component] message`. The first component of a session (launcher or
//! winmm.dll) starts a new file; later ones append. Write errors are ignored (the log must never stop the game).
use std::{fs::OpenOptions, io::Write};

/// Start a new log file (session start).
pub fn reset() {
    if let Some(path) = crate::paths::log_file() {
        if path.parent().is_some_and(|d| d.is_dir()) {
            let _ = std::fs::write(path, b"");
        }
    }
}

pub fn write(component: &str, message: &str) {
    let Some(path) = crate::paths::log_file() else { return };
    if !path.parent().is_some_and(|d| d.is_dir()) {
        return;
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{component}] {message}");
    }
}

/// `log!("qol", "{} hooks", n)`
#[macro_export]
macro_rules! log {
    ($component:expr, $($arg:tt)*) => { $crate::log::write($component, &format!($($arg)*)) };
}
