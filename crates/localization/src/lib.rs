//! `mods\localization.dll`: language of Patrician II and III.
//!
//! Loaded by `winmm.dll`, `patrician-launcher.exe` or the P3Modding modloader, which call `start`. The language is
//! `mods\settings.ini` `[localization] lang=<ISO 639-1 code>` (missing or invalid = `en`); its files ([`pack`]: the
//! archive `mods\localization\<lang>.cpr`, entries `p2\...` / `p3\...`, and the loose folder
//! `mods\localization\<lang>\<p2|p3>\`, which wins) replace the game files of the same name ([`res`]);
//! in P2 GOG and P3 GOG also the font charset, the texts of the executable ([`exe`], in P3 with the tool tips) and
//! their grammar ([`grammar`]). Language selector on the main menu ([`menu`]).

use pcore::game::{Base, Game};
use pcore::hook::Plan;
use pcore::{ini::Ini, log, paths, pe};
use std::sync::atomic::{AtomicBool, Ordering};

mod exe;
mod grammar;
mod menu;
mod pack;
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
    let Some(base) = game.map(|g| g.base()) else {
        log::write(TAG, "skipped, unknown executable");
        return 0;
    };
    // before the archive and the loose files are opened through archiver.dll
    match pcore::archiver::fix_names(base) {
        Ok(msg) => log::write(TAG, &format!("archiver_names: {msg}")),
        Err(e) => log::write(TAG, &format!("archiver_names: skipped, {e}")),
    }
    let pack = pack::Pack::open(&lang, base);
    match res::install(&lang, base, &pack) {
        Ok(msg) => log::write(TAG, &format!("resources: on ({msg})")),
        Err(e) => log::write(TAG, &format!("resources: skipped, {e}")),
    }
    // the Steam executable decrypts its code only after start-up
    if matches!(game, Some(Game::P2Gog | Game::P3Gog)) && lang != DEFAULT_LANG {
        let forms = grammar(&lang, base);
        match exe::install(&lang, &pack, forms, base) {
            Ok(msg) => log::write(TAG, &format!("exe: on ({msg})")),
            Err(e) => log::write(TAG, &format!("exe: skipped, {e}")),
        }
        if base == Base::P3 {
            let mut plan = Plan::new();
            match exe::tooltips(&mut plan).and_then(|n| plan.commit().map(|k| (n, k))) {
                Ok((n, k)) => log::write(TAG, &format!("tooltips: on ({n} strings, {k} patches)")),
                Err(e) => log::write(TAG, &format!("tooltips: skipped, {e}")),
            }
        }
    }
    let mut plan = Plan::new();
    match ui::trade_row(&mut plan, base, &pack).and_then(|()| plan.commit()) {
        Ok(n) => log::write(TAG, &format!("trade_row: on (auto-trade Undo 72 px, toggles moved, {n} patches)")),
        Err(e) => log::write(TAG, &format!("trade_row: skipped, {e}")),
    }
    let mut plan = Plan::new();
    match menu::selector(&mut plan, base, &lang).and_then(|s| plan.commit().map(|n| (s, n))) {
        Ok((s, n)) => log::write(TAG, &format!("menu: on (main menu language selector, {s}, {n} patches)")),
        Err(e) => log::write(TAG, &format!("menu: skipped, {e}")),
    }
    0
}

/// Grammar hooks of the translations, each committed on its own. Returns whether the plural hooks are on.
fn grammar(lang: &str, base: Base) -> bool {
    let rev = exe::reverse_codepage(exe::codepage(lang));
    let parts: [(&str, &str, &dyn Fn(&mut Plan) -> Result<(), String>); 4] = [
        ("drop_caps", "initials of national capitals", &|p| grammar::drop_caps(p, base, lang == "ru", &rev)),
        ("plurals", "count-dependent forms", &|p| grammar::plurals(p, base)),
        ("adjective_gender", "merchant adjective by gender", &|p| grammar::adjective_gender(p, base)),
        ("name_capital", "merchant name at a sentence start", &|p| grammar::name_capital(p, base)),
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
