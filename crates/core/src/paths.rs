//! Game folder layout: `<game>\<exe>`, `<game>\mods\settings.ini`, `<game>\mods\loader.log`, `<game>\mods\*.dll`.
use std::path::PathBuf;

/// Folder of the running executable: the game folder for `winmm.dll` and the mods, the launcher's own folder (= game
/// folder) for `patrician-launcher.exe`.
pub fn game_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

pub fn mods_dir() -> Option<PathBuf> {
    game_dir().map(|d| d.join("mods"))
}

pub fn settings() -> Option<PathBuf> {
    mods_dir().map(|d| d.join("settings.ini"))
}

pub fn log_file() -> Option<PathBuf> {
    mods_dir().map(|d| d.join("loader.log"))
}
