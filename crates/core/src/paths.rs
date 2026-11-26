//! Game folder layout: `<game>\<exe>`, `<game>\mods\settings.ini`, `<game>\mods\loader.log`, `<game>\mods\*.dll`.
use crate::game::Base;
use std::path::PathBuf;

/// Folder of the running executable: the game folder for `winmm.dll` and the mods, the launcher's own folder (= game
/// folder) for `patrician-launcher.exe`.
pub fn game_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

pub fn mods_dir() -> Option<PathBuf> {
    game_dir().map(|d| d.join("mods"))
}

/// `<game>\mods\localization\<lang>\<p2|p3>`: files of one language (`lang` = ISO 639-1 code) for one game.
pub fn localization_dir(lang: &str, base: Base) -> Option<PathBuf> {
    mods_dir().map(|d| d.join("localization").join(lang).join(base.dir()))
}

pub fn settings() -> Option<PathBuf> {
    mods_dir().map(|d| d.join("settings.ini"))
}

pub fn log_file() -> Option<PathBuf> {
    mods_dir().map(|d| d.join("loader.log"))
}
