//! `mods\qol.dll`: quality-of-life features and fixes of original bugs, one DLL for Patrician II and III.
//!
//! Loaded by `winmm.dll`, `patrician-launcher.exe` or the P3Modding modloader, which call `start`. Every feature
//! checks the original bytes at all its sites first and is skipped as a whole when they differ. Each feature has a
//! key in `mods\settings.ini` `[qol]` (default 1 = on), except the fixes of original bugs, which are always on; the
//! result of each one is written to `mods\loader.log`. The `archiver.dll` name lookup is fixed first
//! ([`pcore::archiver`]): the widescreen files are served from disk, which broke it.

use pcore::game::{Base, Game};
use pcore::hook::Plan;
use pcore::{ini::Ini, log, paths, pe};
use std::sync::atomic::{AtomicBool, Ordering};

mod fixes;
mod keys;
mod plate;
mod stream;
mod ui;
mod util;
mod wide;

const TAG: &str = "qol";

struct Feature {
    key: &'static str,
    /// Has a `[qol]` key in `settings.ini`; false = always on (fixes of original bugs).
    setting: bool,
    what: &'static str,
    games: &'static [Base],
    install: fn(&mut Plan, Base) -> Result<(), String>,
}

const BOTH: &[Base] = &[Base::P2, Base::P3];

const FEATURES: &[Feature] = &[
    Feature { key: "music_fade", setting: false, what: "old music track fades out within 5 s at any volume", games: BOTH, install: fixes::music_fade },
    Feature { key: "tutorial_ambience", setting: false, what: "building ambience no longer stuck in the tutorial", games: BOTH, install: fixes::tutorial_ambience },
    Feature { key: "single_game_title", setting: false, what: "single game screen title", games: BOTH, install: fixes::single_game_title },
    Feature { key: "shipyard_knots", setting: true, what: "shipyard shows the real base speed in knots", games: BOTH, install: fixes::shipyard_knots },
    Feature { key: "campaign_speed", setting: false, what: "campaign starts with the game speed of the settings", games: &[Base::P2], install: fixes::campaign_speed },
    Feature { key: "ship_speed", setting: true, what: "ship panel speed in tenths of a knot", games: BOTH, install: fixes::ship_speed },
    Feature { key: "building_backdrop", setting: true, what: "more opaque backdrop behind building window texts", games: &[Base::P2], install: ui::building_backdrop },
    Feature { key: "trade_route_dialog", setting: false, what: "wider trade route file dialog, large buttons", games: &[Base::P3], install: ui::trade_route_dialog },
    Feature { key: "wasd", setting: true, what: "WASD scrolls the town / sea map", games: BOTH, install: keys::wasd },
    Feature { key: "speed_keys", setting: true, what: "game speed hotkeys + / -", games: BOTH, install: keys::speed_keys },
    Feature { key: "speed_plate", setting: true, what: "game speed buttons below the top bar", games: BOTH, install: plate::speed_plate },
    Feature { key: "stream_open", setting: false, what: "music / voice stream falls back between .mp3 and .aac", games: BOTH, install: stream::stream_open },
    Feature { key: "widescreen", setting: true, what: "third resolution = [display] width x height, any aspect ratio", games: BOTH, install: wide::widescreen },
];

/// P3Modding interface: called once after loading; 0 = success.
#[no_mangle]
pub extern "C" fn start() -> u32 {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return 0;
    }
    let Some(game) = pe::current().and_then(|(_, info)| Game::identify(&info)) else {
        log::write(TAG, "unsupported executable: nothing patched");
        return 1;
    };
    let base = game.base();
    match pcore::archiver::fix_names(base) {
        Ok(msg) => log::write(TAG, &format!("archiver_names: {msg}")),
        Err(e) => log::write(TAG, &format!("archiver_names: skipped, {e}")),
    }
    let ini = Ini::load(paths::settings().as_deref());
    let (mut on, mut failed) = (0, 0);
    for f in FEATURES.iter().filter(|f| f.games.contains(&base)) {
        if f.setting && !ini.flag("qol", f.key, true) {
            log::write(TAG, &format!("{}: off in settings.ini", f.key));
            continue;
        }
        let mut plan = Plan::new();
        match (f.install)(&mut plan, base).and_then(|()| plan.commit()) {
            Ok(n) => {
                on += 1;
                log::write(TAG, &format!("{}: on ({}, {n} patches)", f.key, f.what));
            }
            Err(e) => {
                failed += 1;
                log::write(TAG, &format!("{}: skipped, {e}", f.key));
            }
        }
    }
    log::write(TAG, &format!("{}: {on} feature(s) on, {failed} skipped", game.name()));
    if game == Game::P3Steam && failed > 0 {
        log::write(TAG, "Steam: the game code is decrypted only after start-up; load qol.dll later (P3Modding) for now");
    }
    0
}
