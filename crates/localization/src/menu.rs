//! Patrician II main menu: language selector at the top left (no text). `[<]` flag `[>]` and a check button under
//! the flag. The arrows cycle the flags of the installed languages (display only); the check button (grey when the
//! shown flag is the running language) writes `[localization] lang` to `mods\settings.ini` and restarts the game.
//!
//! - languages: `en` (original files) + every two-letter folder of `mods\localization\` with a known flag, in the
//!   order GB DE DK NO SE FI EE LV LT RU
//! - main menu screen: vtable 61C8F8, draw vt+9Ch 42BC20 (absolute 800x600 coordinates), update vt+12Ch 42BE90
//!   (returns -5 = quit, like the "Exit" item at 42C315)
//! - draw hook 42BE4A (before the render target restore): runs {x, y, w, h, color} drawn with 4A7650 (x, y, w, h)
//!   and the colour of 4A7A90 (returns the previous one)
//! - click: polled in the update hook (left button pressed since the last update, game window in front, mouse at
//!   [6747C8h] / [6747C0h]); the WM_LBUTTONDOWN entry 4ACF00 belongs to `qol.dll` (speed plate)
//! - menu entered (update gap >= 300 ms): shown flag = running language, a held button is not a click
//! - restart: the CRT exit calls ExitProcess at 5E523D; when a language was applied, a new instance is started there
//!   first (after the game has shut down normally): `patrician-launcher.exe` when it started this game, else the same
//!   command line
use pcore::ffi::*;
use pcore::game::Base;
use pcore::hook::{Ctx, Ins, Plan};
use pcore::mem::{cat, hex, rd32, rel5};
use pcore::{log, paths};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::Relaxed};
use std::sync::OnceLock;

const FW: i32 = 30;
const FH: i32 = 20;
const OX: i32 = 12;
const OY: i32 = 12;
const AW: i32 = 16;
const AH: i32 = FH + 2;
const FX: i32 = AW + 4;
const RX: i32 = FX + FW + 2 + 4;
const CY: i32 = AH + 4;
const CH: i32 = 20;
const W: i32 = RX + AW;
const H: i32 = CY + CH;
const EDGE: u32 = 0xff3a2a1a;
const BTN: u32 = 0xffc4a66e;
const INK: u32 = 0xff2a1c10;
const OFF: u32 = 0xff9c8c70;

const SET_COLOR: u32 = 0x4a7a90;
const FILL: u32 = 0x4a7650;
const INVALIDATE: u32 = 0x4a5870;
const INV_THIS: u32 = 0x674268;
const MX: u32 = 0x6747c8;
const MY: u32 = 0x6747c0;

/// Flag -> language codes (ISO 639-1), in the cycle order.
const ORDER: &[(&str, &[&str])] = &[
    ("GB", &["en"]),
    ("DE", &["de"]),
    ("DK", &["da"]),
    ("NO", &["nb", "no", "nn"]),
    ("SE", &["sv"]),
    ("FI", &["fi"]),
    ("EE", &["et"]),
    ("LV", &["lv"]),
    ("LT", &["lt"]),
    ("RU", &["ru"]),
];

/// Invalidated rectangle {l, t, r, b}: the widget +-2.
static RC: [i32; 4] = [OX - 2, OY - 2, OX + W + 2, OY + H + 2];

#[derive(Clone, Copy)]
struct Run {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    c: u32,
}

struct Menu {
    codes: Vec<String>,
    flags: Vec<Vec<Run>>,
    base: Vec<Run>,
    on: Vec<Run>,
    off: Vec<Run>,
    /// Index of the running language (None: not in the list).
    cur: Option<usize>,
    /// `ret` (update returns -5).
    ret: u32,
}

static MENU: OnceLock<Menu> = OnceLock::new();
static SEL: AtomicUsize = AtomicUsize::new(0);
static STAMP: AtomicU32 = AtomicU32::new(0);
static DOWN: AtomicBool = AtomicBool::new(false);
static RST: AtomicBool = AtomicBool::new(false);

/// Install the selector. Ok = summary for the log.
pub fn selector(p: &mut Plan, base: Base, lang: &str) -> Result<String, String> {
    if base != Base::P2 {
        return Err("Patrician II only".into());
    }
    let mut codes = vec!["en".to_string()];
    let dir = paths::mods_dir().ok_or("no mods folder")?.join("localization");
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if name.len() != 2 || !name.bytes().all(|c| c.is_ascii_alphabetic()) || !e.path().is_dir() {
                continue;
            }
            if flag_of(&name).is_none() {
                log::write(crate::TAG, &format!("menu: no flag for {name}, not in the selector"));
            } else if !codes.contains(&name) {
                codes.push(name);
            }
        }
    }
    codes.sort_by_key(|c| (flag_of(c).unwrap_or(usize::MAX), c.clone()));
    if codes.len() < 2 {
        return Err("only one language installed".into());
    }
    let cur = codes.iter().position(|c| c == lang);
    p.check32(0x61c8f8 + 0x9c, 0x42bc20)?;
    p.check32(0x61c8f8 + 0x12c, 0x42be90)?;
    // mov ecx,[esp+1Ch]; push ecx; call 4A7BD0
    p.mid(0x42be4a, 5, &cat(&[&hex("8b4c241c51"), &rel5(0xe8, 0x42be4f, 0x4a7bd0)]),
        &[Ins::Bytes(&[0x8b, 0x4c, 0x24, 0x1c, 0x51])], &[], draw, 0)?;
    // mov al,[676853h]
    p.mid(0x42be90, 5, &hex("a053686700"), &[Ins::Bytes(&[0xa0, 0x53, 0x68, 0x67, 0x00])], &[], update, 0)?;
    // call [ExitProcess]
    p.mid(0x5e523d, 6, &hex("ff1504a36100"), &[Ins::Bytes(&[0xff, 0x15, 0x04, 0xa3, 0x61, 0x00])], &[], exit, 0)?;
    let ret = Plan::ret_stub(0)?;
    let flags = codes.iter().map(|c| flag_runs(ORDER[flag_of(c).unwrap_or(0)].0)).collect();
    let summary = format!("{} languages: {}", codes.len(), codes.join(" "));
    SEL.store(cur.unwrap_or(0), Relaxed);
    let _ = MENU.set(Menu { codes, flags, base: base_runs(), on: check(INK), off: check(OFF), cur, ret });
    Ok(summary)
}

fn flag_of(code: &str) -> Option<usize> {
    ORDER.iter().position(|(_, codes)| codes.contains(&code))
}

/// Flag pixel (30 x 20), ARGB.
fn pixel(flag: &str, x: i32, y: i32) -> u32 {
    let (xf, yf) = (x as f64 + 0.5, y as f64 + 0.5);
    let grid = |gw: f64, gh: f64| (xf * gw / FW as f64, yf * gh / FH as f64);
    let stripes = |cols: &[u32]| cols[((yf * cols.len() as f64 / FH as f64) as usize).min(cols.len() - 1)];
    // Nordic cross: field, cross (column cx0..cx1, row cy0..cy1 in grid units gw x gh), optional inner cross
    let nordic = |field: u32, cross: u32, gw: f64, gh: f64, c: [f64; 4], inner: Option<(u32, [f64; 4])>| {
        let (u, v) = grid(gw, gh);
        if let Some((col, i)) = inner {
            if (u >= i[0] && u < i[1]) || (v >= i[2] && v < i[3]) {
                return col;
            }
        }
        if (u >= c[0] && u < c[1]) || (v >= c[2] && v < c[3]) { cross } else { field }
    };
    match flag {
        "GB" => {
            let (u, v) = grid(60.0, 30.0);
            if (u - 30.0).abs() < 3.0 || (v - 15.0).abs() < 3.0 {
                return 0xffc8102e;
            }
            if (u - 30.0).abs() < 5.0 || (v - 15.0).abs() < 5.0 {
                return 0xffffffff;
            }
            let s5 = 5f64.sqrt();
            let d = ((u - 2.0 * v).abs() / s5).min((u + 2.0 * v - 60.0).abs() / s5);
            if d < 1.0 {
                0xffc8102e
            } else if d < 3.0 {
                0xffffffff
            } else {
                0xff012169
            }
        }
        "RU" => [0xffffffff, 0xff0039a6, 0xffd52b1e][(y * 3 / FH) as usize],
        "SE" => {
            let (u, v) = grid(16.0, 10.0);
            if (5.0..7.0).contains(&u) || (4.0..6.0).contains(&v) { 0xfffecc02 } else { 0xff006aa7 }
        }
        "DE" => stripes(&[0xff000000, 0xffdd0000, 0xffffce00]),
        "DK" => nordic(0xffc8102e, 0xffffffff, 37.0, 28.0, [12.0, 16.0, 12.0, 16.0], None),
        "NO" => nordic(0xffba0c2f, 0xffffffff, 22.0, 16.0, [6.0, 10.0, 6.0, 10.0], Some((0xff00205b, [7.0, 9.0, 7.0, 9.0]))),
        "FI" => nordic(0xffffffff, 0xff002f6c, 18.0, 11.0, [5.0, 8.0, 4.0, 7.0], None),
        "EE" => stripes(&[0xff0072ce, 0xff000000, 0xffffffff]),
        "LT" => stripes(&[0xfffdb913, 0xff006a44, 0xffc1272d]),
        "LV" => {
            let (_, v) = grid(1.0, 10.0);
            if (4.0..6.0).contains(&v) { 0xffffffff } else { 0xff9e3039 }
        }
        _ => OFF,
    }
}

/// Flag as horizontal runs inside the frame.
fn flag_runs(flag: &str) -> Vec<Run> {
    let mut r = Vec::new();
    for y in 0..FH {
        let mut x = 0;
        while x < FW {
            let c = pixel(flag, x, y);
            let mut w = 1;
            while x + w < FW && pixel(flag, x + w, y) == c {
                w += 1;
            }
            r.push(Run { x: FX + 1 + x, y: 1 + y, w, h: 1, c });
            x += w;
        }
    }
    r
}

fn button(x: i32, y: i32, w: i32, h: i32) -> [Run; 2] {
    [Run { x, y, w, h, c: EDGE }, Run { x: x + 1, y: y + 1, w: w - 2, h: h - 2, c: BTN }]
}

/// Buttons, flag frame, arrows (triangles of 5 columns).
fn base_runs() -> Vec<Run> {
    let mut r = Vec::new();
    r.extend(button(0, 0, AW, AH));
    r.extend(button(RX, 0, AW, AH));
    r.push(Run { x: FX, y: 0, w: FW + 2, h: AH, c: EDGE });
    r.extend(button(FX, CY, FW + 2, CH));
    for i in 0..5 {
        r.push(Run { x: 5 + i, y: (AH >> 1) - i, w: 1, h: 2 * i + 1, c: INK });
        r.push(Run { x: RX + AW - 6 - i, y: (AH >> 1) - i, w: 1, h: 2 * i + 1, c: INK });
    }
    r
}

/// Check mark, 3 px thick.
fn check(c: u32) -> Vec<Run> {
    let cx = FX + 1 + (FW >> 1);
    (cx - 6..=cx + 6)
        .map(|x| {
            let y = if x <= cx - 2 { CY + 9 + (x - (cx - 6)) } else { CY + 13 - (x - (cx - 2)) };
            Run { x, y: y - 1, w: 1, h: 3, c }
        })
        .collect()
}

unsafe fn set_color(c: u32) -> u32 {
    let f: extern "C" fn(u32) -> u32 = core::mem::transmute(SET_COLOR as usize);
    f(c)
}

unsafe fn runs(rs: &[Run], cur: &mut u32) {
    let fill: extern "C" fn(i32, i32, i32, i32) -> i32 = core::mem::transmute(FILL as usize);
    for r in rs {
        if r.c != *cur {
            *cur = r.c;
            set_color(r.c);
        }
        fill(OX + r.x, OY + r.y, r.w, r.h);
    }
}

unsafe fn invalidate() {
    let f: extern "thiscall" fn(u32, *const i32) = core::mem::transmute(INVALIDATE as usize);
    f(INV_THIS, RC.as_ptr());
}

/// Main menu draw, before the render target restore.
unsafe extern "C" fn draw(_: *mut Ctx, _: u32) {
    let Some(m) = MENU.get() else { return };
    let sel = SEL.load(Relaxed).min(m.flags.len() - 1);
    let old = set_color(EDGE);
    let mut cur = EDGE;
    runs(&m.base, &mut cur);
    runs(&m.flags[sel], &mut cur);
    runs(if m.cur == Some(sel) { &m.off } else { &m.on }, &mut cur);
    set_color(old);
}

/// Left (logical) mouse button down while a window of this process is in front.
unsafe fn button_down() -> bool {
    let mut pid = 0u32;
    GetWindowThreadProcessId(GetForegroundWindow(), &mut pid);
    if pid != GetCurrentProcessId() {
        return false;
    }
    let vk = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
    GetAsyncKeyState(vk) as u16 & 0x8000 != 0
}

/// Main menu update entry (thiscall, no arguments; returns the menu state).
unsafe extern "C" fn update(ctx: *mut Ctx, _: u32) {
    let Some(m) = MENU.get() else { return };
    let now = GetTickCount();
    let gap = now.wrapping_sub(STAMP.swap(now, Relaxed));
    let down = button_down();
    let was = DOWN.swap(down, Relaxed);
    if gap >= 300 {
        // menu entered: show the running language; a button held from the previous screen is no click
        if let Some(c) = m.cur {
            if SEL.swap(c, Relaxed) != c {
                invalidate();
            }
        }
        return;
    }
    if !down || was {
        return;
    }
    let (x, y) = (rd32(MX) as i32 - OX, rd32(MY) as i32 - OY);
    let inside = |l: i32, t: i32, w: i32, h: i32| x >= l && x < l + w && y >= t && y < t + h;
    let n = m.codes.len();
    let sel = SEL.load(Relaxed).min(n - 1);
    if inside(0, 0, AW, AH) {
        SEL.store((sel + n - 1) % n, Relaxed);
        invalidate();
    } else if inside(RX, 0, AW, AH) {
        SEL.store((sel + 1) % n, Relaxed);
        invalidate();
    } else if inside(FX, CY, FW + 2, CH) && m.cur != Some(sel) {
        let code = &m.codes[sel];
        if !write_lang(code) {
            log::write(crate::TAG, &format!("menu: lang={code} not written to settings.ini ({})", GetLastError()));
            return;
        }
        log::write(crate::TAG, &format!("menu: lang={code}, restarting"));
        RST.store(true, Relaxed);
        let c = &mut *ctx;
        c.eax = -5i32 as u32;
        c.resume = m.ret;
    }
}

fn write_lang(code: &str) -> bool {
    let Some(file) = paths::settings() else { return false };
    let w = |s: &str| pcore::wide(s.as_ref());
    let (sec, key, val, file) = (w("localization"), w("lang"), w(code), pcore::wide(file.as_os_str()));
    unsafe { WritePrivateProfileStringW(sec.as_ptr(), key.as_ptr(), val.as_ptr(), file.as_ptr()) != 0 }
}

/// CRT exit, before ExitProcess: start the new instance when a language was applied.
unsafe extern "C" fn exit(_: *mut Ctx, _: u32) {
    if !RST.swap(false, Relaxed) {
        return;
    }
    let (app, mut cmd) = restart_command();
    let mut si: STARTUPINFOW = core::mem::zeroed();
    si.cb = core::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = core::mem::zeroed();
    let app_ptr = app.as_ref().map_or(core::ptr::null(), |a| a.as_ptr());
    let ok = CreateProcessW(app_ptr, cmd.as_mut_ptr(), core::ptr::null(), core::ptr::null(), 0, 0,
        core::ptr::null(), core::ptr::null(), &si, &mut pi);
    if ok == 0 {
        log::write(crate::TAG, &format!("menu: restart failed ({})", GetLastError()));
    } else {
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    }
}

/// (application, command line): `patrician-launcher.exe` of the game folder when the launcher started this game
/// ([`pcore::LAUNCHER_ENV`] set), else the game's own command line (`winmm.dll` loads the mods again).
unsafe fn restart_command() -> (Option<Vec<u16>>, Vec<u16>) {
    if std::env::var_os(pcore::LAUNCHER_ENV).is_some() {
        if let Some(launcher) = paths::game_dir().map(|d| d.join("patrician-launcher.exe")).filter(|l| l.is_file()) {
            return (Some(pcore::wide(launcher.as_os_str())), quoted(&launcher));
        }
    }
    let p = GetCommandLineW();
    let mut cmd = Vec::new();
    let mut i = 0;
    while *p.add(i) != 0 {
        cmd.push(*p.add(i));
        i += 1;
    }
    cmd.push(0);
    (None, cmd)
}

fn quoted(p: &Path) -> Vec<u16> {
    let mut s = std::ffi::OsString::from("\"");
    s.push(p.as_os_str());
    s.push("\"");
    pcore::wide(&s)
}
