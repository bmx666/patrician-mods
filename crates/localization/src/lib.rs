//! `mods\localization.dll`: language of Patrician II and III.
//!
//! Loaded by `winmm.dll`, `patrician-launcher.exe` or the P3Modding modloader, which call `start`. The language is
//! `mods\settings.ini` `[localization] lang=<ISO 639-1 code>` (missing or invalid = `en`); its files live in
//! `mods\localization\<lang>\` and replace the game files of the same name ([`res`]); in P2 also the font charset and
//! the texts of the executable ([`exe`]) and their grammar ([`grammar`]). Language selector on the main menu
//! ([`menu`]).

use pcore::game::Game;
use pcore::hook::Plan;
use pcore::{ini::Ini, log, paths, pe};
use std::sync::atomic::{AtomicBool, Ordering};

mod exe;
mod grammar;
mod menu;
mod res;
mod ui;

const TAG: &str = "localization";

/// Language when the setting is missing or not a two-letter code.
const DEFAULT_LANG: &str = "en";

/// P3Modding interface: called once after loading; 0 = success.
#[no_mangle]
pub extern "C" fn start() -> u32 {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return 0;
    }
    let game = pe::current().and_then(|(_, info)| Game::identify(&info));
    log::write(TAG, &format!("game: {}", game.map_or("unknown executable", |g| g.name())));
    let ini = Ini::load(paths::settings().as_deref());
    let lang = lang_code(ini.get("localization", "lang"));
    log::write(TAG, &format!("language: {lang}"));
    match res::install(&lang) {
        Ok(msg) => log::write(TAG, &format!("resources: on ({msg})")),
        Err(e) => log::write(TAG, &format!("resources: skipped, {e}")),
    }
    if game == Some(Game::P2Gog) && lang != DEFAULT_LANG {
        let forms = grammar(&lang);
        match paths::localization_dir(&lang)
            .ok_or_else(|| "no game folder".to_string())
            .and_then(|d| exe::install(&lang, &d, forms))
        {
            Ok(msg) => log::write(TAG, &format!("exe: on ({msg})")),
            Err(e) => log::write(TAG, &format!("exe: skipped, {e}")),
        }
    }
    if let Some(base) = game.map(|g| g.base()) {
        let dir = paths::localization_dir(&lang);
        let mut plan = Plan::new();
        match ui::trade_row(&mut plan, base, dir.as_deref()).and_then(|()| plan.commit()) {
            Ok(n) => log::write(TAG, &format!("trade_row: on (auto-trade Undo 72 px, toggles moved, {n} patches)")),
            Err(e) => log::write(TAG, &format!("trade_row: skipped, {e}")),
        }
        let mut plan = Plan::new();
        match menu::selector(&mut plan, base, &lang).and_then(|s| plan.commit().map(|n| (s, n))) {
            Ok((s, n)) => log::write(TAG, &format!("menu: on (main menu language selector, {s}, {n} patches)")),
            Err(e) => log::write(TAG, &format!("menu: skipped, {e}")),
        }
    }
    0
}

/// Grammar hooks of the P2 translations, each committed on its own. Returns whether the plural hooks are on.
fn grammar(lang: &str) -> bool {
    let rev = exe::reverse_codepage(exe::codepage(lang));
    let parts: [(&str, &str, &dyn Fn(&mut Plan) -> Result<(), String>); 4] = [
        ("drop_caps", "initials of national capitals", &|p| grammar::drop_caps(p, lang == "ru", &rev)),
        ("plurals", "count-dependent forms", &grammar::plurals),
        ("adjective_gender", "merchant adjective by gender", &grammar::adjective_gender),
        ("name_capital", "merchant name at a sentence start", &grammar::name_capital),
    ];
    let mut forms = false;
    for (name, what, f) in parts {
        let mut plan = Plan::new();
        match f(&mut plan).and_then(|()| plan.commit()) {
            Ok(n) => {
                forms |= name == "plurals";
                log::write(TAG, &format!("{name}: on ({what}, {n} patches)"));
            }
            Err(e) => log::write(TAG, &format!("{name}: skipped, {e}")),
        }
    }
    forms
}

/// Two ASCII letters, lower case; anything else -> [`DEFAULT_LANG`] (logged).
fn lang_code(v: Option<&str>) -> String {
    match v {
        Some(s) if s.len() == 2 && s.bytes().all(|c| c.is_ascii_alphabetic()) => s.to_ascii_lowercase(),
        None | Some("") => DEFAULT_LANG.to_string(),
        Some(s) => {
            log::write(TAG, &format!("lang={s} is not a two-letter ISO 639-1 code, using {DEFAULT_LANG}"));
            DEFAULT_LANG.to_string()
        }
    }
}
