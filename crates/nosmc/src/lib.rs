//! `mods\nosmc.dll`: removes the self-modifying code of the image decoder `AIM.dll` (both games), in memory.
//!
//! Loaded by `winmm.dll`, `patrician-launcher.exe` or the P3Modding modloader, which call `start`. The blitters of
//! `AIM.dll` patch their own instructions and keep counters inside their code pages on every sprite and row. An x86
//! translator (Rosetta 2 on macOS, Prism on Windows on ARM, FEX / Box86 on Linux) throws its translation of a page
//! away on every such write, which makes drawing slow. [`smc`] moves that data out of the code pages and makes the
//! patched instructions read variables instead. `mods\settings.ini` `[nosmc] enabled=auto|1|0`: `auto` (default)
//! patches when [`host`] finds an x86 translator or a macOS host.

use pcore::{ini::Ini, log, paths};
use std::sync::atomic::{AtomicBool, Ordering};

mod host;
mod smc;

const TAG: &str = "nosmc";

/// Modules with self-modifying blitters (`ddraw_dll.dll` comes from a separate renderer package).
const MODULES: &[&str] = &["AIM.dll"];

/// P3Modding interface: called once after loading; 0 = success.
#[no_mangle]
pub extern "C" fn start() -> u32 {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return 0;
    }
    let h = host::detect();
    log::write(TAG, &format!("host: {}", h.describe()));
    let ini = Ini::load(paths::settings().as_deref());
    let mode = ini.get("nosmc", "enabled").unwrap_or("auto").to_ascii_lowercase();
    let on = match mode.as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        "auto" | "" => h.translated(),
        other => {
            log::write(TAG, &format!("enabled={other} is not auto / 1 / 0, using auto"));
            h.translated()
        }
    };
    if !on {
        log::write(TAG, &format!("off (enabled={mode}{})", if mode == "0" { "" } else { ", native x86 host" }));
        return 0;
    }
    for name in MODULES {
        match smc::patch(name) {
            Ok(msg) => log::write(TAG, &format!("{name}: on ({msg})")),
            Err(e) => log::write(TAG, &format!("{name}: skipped, {e}")),
        }
    }
    0
}
