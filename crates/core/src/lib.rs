//! Shared core of the Patrician II / III mod loader (`winmm.dll`, `patrician-launcher.exe`) and the mods.
//!
//! - [`game`]: which game executable runs (Patrician II GOG, Patrician III GOG / Steam)
//! - [`ini`]: `mods\settings.ini`
//! - [`log`]: `mods\loader.log`
//! - [`mem`], [`hook`]: memory patches with byte checks, mid-function hooks with Rust handlers
//! - [`mods`]: loading `mods\*.dll` and calling their `start` export (the P3Modding interface)

pub mod ffi;
pub mod game;
pub mod hook;
pub mod ini;
pub mod log;
pub mod mem;
pub mod mods;
pub mod paths;
pub mod pe;

/// NUL-terminated UTF-16 string for Windows API calls.
pub fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Environment variable set by `patrician-launcher.exe` for the game process: the launcher loads the mods itself,
/// so `winmm.dll` (when also installed) only forwards.
pub const LAUNCHER_ENV: &str = "PATRICIAN_MODS_LAUNCHER";
