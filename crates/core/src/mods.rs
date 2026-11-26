//! `mods\*.dll`: loaded in name order; each DLL's `start` export (no arguments, 0 = success) is called. This is the
//! interface of the P3Modding modloader, so its mods load here too and our mods load under P3Modding.
use crate::ffi::*;
use crate::{game::Game, ini::Ini, log, paths, pe};
use std::path::{Path, PathBuf};

/// DLL files of the mods folder, sorted by name (case-insensitive).
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    v
}

/// P3Modding (`p3_modloader.dll`) already loads `mods\*.dll` in this process.
pub fn p3modding_present() -> bool {
    let name = crate::wide("p3_modloader.dll".as_ref());
    !unsafe { GetModuleHandleW(name.as_ptr()) }.is_null()
}

/// Load every mod of `mods\` into this process and call its `start`. `tag` names the caller in the log.
pub fn load_all(tag: &str) {
    let ini = Ini::load(paths::settings().as_deref());
    if !ini.flag("loader", "enabled", true) {
        log::write(tag, "[loader] enabled=0: mods not loaded");
        return;
    }
    if p3modding_present() {
        log::write(tag, "P3Modding modloader present: it loads the mods");
        return;
    }
    match pe::current().and_then(|(_, info)| Game::identify(&info)) {
        Some(g) => log::write(tag, &format!("game: {}", g.name())),
        None => log::write(tag, "game: unknown executable (mods check their own patch sites)"),
    }
    let Some(dir) = paths::mods_dir() else { return };
    let mods = list(&dir);
    let mut ok = 0;
    for path in &mods {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let w = crate::wide(path.as_os_str());
        let module = unsafe { LoadLibraryW(w.as_ptr()) };
        if module.is_null() {
            log::write(tag, &format!("{name}: LoadLibrary failed ({})", unsafe { GetLastError() }));
            continue;
        }
        let start = unsafe { GetProcAddress(module, b"start\0".as_ptr()) };
        if start.is_null() {
            log::write(tag, &format!("{name}: loaded, no start export"));
            continue;
        }
        let start: extern "system" fn() -> u32 = unsafe { core::mem::transmute(start) };
        match start() {
            0 => {
                ok += 1;
                log::write(tag, &format!("{name}: started"));
            }
            r => log::write(tag, &format!("{name}: start returned {r}")),
        }
    }
    log::write(tag, &format!("{ok} of {} mod(s) started", mods.len()));
}
