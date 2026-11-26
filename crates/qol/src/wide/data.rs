//! Widescreen data, made at run time from the game's own files (port of patrician2 / patrician3 `tools/wide.js`):
//! - `images\wide\top_<w>.bmp`: top bar left part, w-284 wide (from `HauptscreenA1280.bmp`, planks repeated)
//! - `images\wide\carv_<h>.bmp`: bottom right carving, 284 x (h-600) (from `HauptscreenE1280.bmp`)
//! - `scripts\textures.ini`: + TEX of them; `scripts\screenGame.ini`: + TEXLIST0 ids, ANIM of the top bars / carvings;
//!   `scripts\accelMap.ini`: + SCREEN / ANIM of the overview map per preset
//!
//! `arc_CreateFile` / `arc_CreateMemMapped` of `archiver.dll` are detoured, and in P3 `cpr_OpenFile` (own body there,
//! in P2 it calls `arc_CreateFile`); chained after `localization.dll`, which loads first by name. The first request
//! of one of these names in a run reads the base file through the previous entry of the same kind (so a language
//! file of `localization.dll` is the base), writes the result to `mods\qol\wide\<name>` and opens that file
//! (container NULL = file on disk); later requests open it directly. Anything else, or a failure, goes to the
//! previous entry unchanged.
use super::{current_layout, Layout};
use pcore::ffi::*;
use pcore::game::Base;
use pcore::hook::Plan;
use pcore::log;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

/// `push esi; mov esi,[esp+8]`: arc_CreateFile / arc_CreateMemMapped (or a jump of an earlier detour).
const PROLOGUE: [u8; 5] = [0x56, 0x8b, 0x74, 0x24, 0x08];
/// `push esi; mov esi,[esp+0Ch]`: cpr_OpenFile of the P3 build (container = 2nd argument).
const PROLOGUE_CPR: [u8; 5] = [0x56, 0x8b, 0x74, 0x24, 0x0c];
const MAX_PATH: usize = 260;

static CREATE_FILE: AtomicU32 = AtomicU32::new(0);
static MEM_MAPPED: AtomicU32 = AtomicU32::new(0);
static CPR_OPEN: AtomicU32 = AtomicU32::new(0);
static FILE_LENGTH: AtomicU32 = AtomicU32::new(0);
static READ: AtomicU32 = AtomicU32::new(0);
static RELEASE_FILE: AtomicU32 = AtomicU32::new(0);

type CreateFile = unsafe extern "C" fn(u32, *const u8) -> u32;
type MemMapped = unsafe extern "C" fn(u32, *const u8, *mut u32, *mut u32) -> u32;
type CprOpen = unsafe extern "C" fn(*const u8, u32) -> u32;
type FileLength = unsafe extern "C" fn(u32) -> i32;
type Read = unsafe extern "C" fn(u32, *mut u8, i32) -> i32;
type ReleaseFile = unsafe extern "C" fn(u32) -> u32;

struct Cache {
    /// ANSI `<game>\mods\qol\wide\`.
    dir_ansi: Vec<u8>,
    dir: PathBuf,
    /// Lower-case relative name -> NUL-terminated ANSI path (None = making it failed, original file used).
    made: HashMap<Vec<u8>, Option<Vec<u8>>>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

pub fn install(p: &mut Plan, base: Base) -> Result<(), String> {
    let mut buf = [0u8; MAX_PATH];
    let len = unsafe { GetModuleFileNameA(core::ptr::null_mut(), buf.as_mut_ptr(), MAX_PATH as u32) } as usize;
    let exe = &buf[..len.min(MAX_PATH)];
    let game = exe.iter().rposition(|&c| c == b'\\').map(|i| &exe[..=i]).ok_or("no game folder")?;
    let mut dir_ansi = game.to_vec();
    dir_ansi.extend_from_slice(b"mods\\qol\\wide\\");
    let dir = pcore::paths::mods_dir().ok_or("no mods folder")?.join("qol").join("wide");

    let module = unsafe { GetModuleHandleW(pcore::wide("archiver.dll".as_ref()).as_ptr()) };
    if module.is_null() {
        return Err("archiver.dll not loaded".into());
    }
    let export = |name: &str| -> Result<u32, String> {
        let z = format!("{name}\0");
        let at = unsafe { GetProcAddress(module, z.as_ptr()) } as usize as u32;
        if at == 0 {
            Err(format!("archiver.dll has no {name}"))
        } else {
            Ok(at)
        }
    };
    FILE_LENGTH.store(export("arc_FileLength")?, Ordering::Relaxed);
    READ.store(export("arc_Read")?, Ordering::Relaxed);
    RELEASE_FILE.store(export("arc_ReleaseFile")?, Ordering::Relaxed);
    let t = p.detour(export("arc_CreateFile")?, &PROLOGUE, create_file as *const () as usize as u32)?;
    CREATE_FILE.store(t, Ordering::Relaxed);
    let t = p.detour(export("arc_CreateMemMapped")?, &PROLOGUE, mem_mapped as *const () as usize as u32)?;
    MEM_MAPPED.store(t, Ordering::Relaxed);
    let cpr = export("cpr_OpenFile")?;
    if base == Base::P3 {
        let t = p.detour(cpr, &PROLOGUE_CPR, cpr_open as *const () as usize as u32)?;
        CPR_OPEN.store(t, Ordering::Relaxed);
    }
    *CACHE.lock().map_err(|_| "cache lock")? = Some(Cache { dir_ansi, dir, made: HashMap::new() });
    Ok(())
}

/// How a file was asked for: through `arc_CreateFile` / `arc_CreateMemMapped` or `cpr_OpenFile` (container).
#[derive(Clone, Copy)]
enum Via {
    Arc(u32),
    Cpr(u32),
}

unsafe extern "C" fn cpr_open(name: *const u8, container: u32) -> u32 {
    let orig: CprOpen = core::mem::transmute(CPR_OPEN.load(Ordering::Relaxed) as usize);
    if let Some(path) = file_for(Via::Cpr(container), name) {
        let f = orig(path.as_ptr(), 0);
        if f != 0 {
            return f;
        }
    }
    orig(name, container)
}

unsafe extern "C" fn create_file(container: u32, name: *const u8) -> u32 {
    let orig: CreateFile = core::mem::transmute(CREATE_FILE.load(Ordering::Relaxed) as usize);
    if let Some(path) = file_for(Via::Arc(container), name) {
        let f = orig(0, path.as_ptr());
        if f != 0 {
            return f;
        }
    }
    orig(container, name)
}

unsafe extern "C" fn mem_mapped(container: u32, name: *const u8, ptr: *mut u32, len: *mut u32) -> u32 {
    let orig: MemMapped = core::mem::transmute(MEM_MAPPED.load(Ordering::Relaxed) as usize);
    if let Some(path) = file_for(Via::Arc(container), name) {
        let m = orig(0, path.as_ptr(), ptr, len);
        if m != 0 {
            return m;
        }
    }
    orig(container, name, ptr, len)
}

enum Kind {
    Textures,
    ScreenGame,
    AccelMap,
    Top(u32),
    Carv(u32),
}

/// Lower-case relative name with `\` separators (`.\` prefixes dropped); None = absolute or empty.
unsafe fn key_of(name: *const u8) -> Option<Vec<u8>> {
    if name.is_null() {
        return None;
    }
    let mut s = core::ffi::CStr::from_ptr(name.cast()).to_bytes();
    while s.starts_with(b".\\") || s.starts_with(b"./") {
        s = &s[2..];
    }
    if s.is_empty() || s[0] == b'\\' || s[0] == b'/' || s.contains(&b':') {
        return None;
    }
    Some(s.iter().map(|&c| if c == b'/' { b'\\' } else { c.to_ascii_lowercase() }).collect())
}

fn kind(key: &[u8], l: &Layout) -> Option<Kind> {
    match key {
        b"scripts\\textures.ini" => return Some(Kind::Textures),
        b"scripts\\screengame.ini" => return Some(Kind::ScreenGame),
        b"scripts\\accelmap.ini" => return Some(Kind::AccelMap),
        _ => {}
    }
    let s = core::str::from_utf8(key.strip_prefix(b"images\\wide\\")?).ok()?.strip_suffix(".bmp")?;
    if let Some(w) = s.strip_prefix("top_").and_then(|n| n.parse().ok()).filter(|w| l.widths.contains(w)) {
        return Some(Kind::Top(w));
    }
    s.strip_prefix("carv_").and_then(|n| n.parse().ok()).filter(|h| l.heights.contains(h)).map(Kind::Carv)
}

/// Path of the made file for `name`, made on the first request.
unsafe fn file_for(via: Via, name: *const u8) -> Option<Vec<u8>> {
    let key = key_of(name)?;
    let l = current_layout();
    let k = kind(&key, l)?;
    let mut guard = CACHE.lock().ok()?;
    let cache = guard.as_mut()?;
    if let Some(done) = cache.made.get(&key) {
        return done.clone();
    }
    let res = make(via, name, &k, l).and_then(|data| {
        let rel = String::from_utf8_lossy(&key).into_owned();
        let path = cache.dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut ansi = cache.dir_ansi.clone();
        ansi.extend_from_slice(&key);
        if ansi.len() >= MAX_PATH {
            return Err("path too long".into());
        }
        ansi.push(0);
        Ok(ansi)
    });
    let out = match res {
        Ok(p) => Some(p),
        Err(e) => {
            log::write("qol", &format!("widescreen: {} not made ({e}), original file used", String::from_utf8_lossy(&key)));
            None
        }
    };
    cache.made.insert(key, out.clone());
    out
}

/// Whole file through the previous archiver entry of `via`.
unsafe fn read(via: Via, name: *const u8) -> Result<Vec<u8>, String> {
    let length: FileLength = core::mem::transmute(FILE_LENGTH.load(Ordering::Relaxed) as usize);
    let read: Read = core::mem::transmute(READ.load(Ordering::Relaxed) as usize);
    let release: ReleaseFile = core::mem::transmute(RELEASE_FILE.load(Ordering::Relaxed) as usize);
    let what = || String::from_utf8_lossy(core::ffi::CStr::from_ptr(name.cast()).to_bytes()).into_owned();
    let f = match via {
        Via::Arc(c) => {
            let create: CreateFile = core::mem::transmute(CREATE_FILE.load(Ordering::Relaxed) as usize);
            create(c, name)
        }
        Via::Cpr(c) => {
            let open: CprOpen = core::mem::transmute(CPR_OPEN.load(Ordering::Relaxed) as usize);
            open(name, c)
        }
    };
    if f == 0 {
        return Err(format!("{} not found", what()));
    }
    let n = length(f);
    let mut v = vec![0u8; n.max(0) as usize];
    let got = if n > 0 { read(f, v.as_mut_ptr(), n) } else { n };
    release(f);
    if n <= 0 || got != n {
        return Err(format!("{} not read", what()));
    }
    Ok(v)
}

unsafe fn make(via: Via, name: *const u8, k: &Kind, l: &Layout) -> Result<Vec<u8>, String> {
    match *k {
        Kind::Textures => Ok(append(&read(via, name)?, &tex_lines(l))),
        Kind::ScreenGame => {
            let mut ids: Vec<u32> = l.widths.iter().map(|&w| top_of(l, w).tex_top).collect();
            ids.extend(l.heights.iter().map(|&h| carv_of(l, h).tex_carv));
            let base = texlist(&read(via, name)?, &ids).ok_or("screenGame.ini: no [TEXLIST0] Count / IDs")?;
            Ok(append(&base, &screen_lines(l)))
        }
        Kind::AccelMap => Ok(append(&read(via, name)?, &accel_lines(l))),
        Kind::Top(w) => {
            let a = Bmp::parse(&read(via, b"images/HauptscreenA1280.bmp\0".as_ptr())?)?;
            Ok(top_bar(&a, w - 284).encode())
        }
        Kind::Carv(h) => {
            let e = Bmp::parse(&read(via, b"images/HauptscreenE1280.bmp\0".as_ptr())?)?;
            Ok(carving(&e, h - 600).encode())
        }
    }
}

fn top_of(l: &Layout, w: u32) -> &super::Preset {
    l.presets.iter().find(|p| p.w == w).expect("width of a preset")
}

fn carv_of(l: &Layout, h: u32) -> &super::Preset {
    l.presets.iter().find(|p| p.h == h).expect("height of a preset")
}

fn tex_lines(l: &Layout) -> Vec<String> {
    let mut v = Vec::new();
    for &w in &l.widths {
        let t = top_of(l, w).tex_top;
        v.extend([
            format!("[TEX{t}]"),
            format!("TextureFile=images/wide/top_{w}.bmp"),
            "UseAlpha=false".into(),
            "Count=1".into(),
            format!("OffsetNSize0=0 0 {} 42", w - 284),
            String::new(),
        ]);
    }
    for &h in &l.heights {
        let t = carv_of(l, h).tex_carv;
        v.extend([
            format!("[TEX{t}]"),
            format!("TextureFile=images/wide/carv_{h}.bmp"),
            "UseAlpha=false".into(),
            "Count=1".into(),
            format!("OffsetNSize0=0 0 284 {}", h - 600),
            String::new(),
        ]);
    }
    v
}

fn anim(id: u32, frame: String, timers: &str, pos: String) -> [String; 9] {
    [
        format!("[ANIM{id}]"),
        "Count=1".into(),
        "FrameCount0=1".into(),
        format!("Frame0={frame}"),
        "TimerCount=1".into(),
        format!("Timers=./scripts/{timers}"),
        "TimerIDs=0".into(),
        format!("Pos={pos}"),
        String::new(),
    ]
}

fn screen_lines(l: &Layout) -> Vec<String> {
    let mut v = Vec::new();
    for &w in &l.widths {
        let p = top_of(l, w);
        v.extend(anim(p.anim_top, format!("{} 0 0 0 0 {} 42 0", p.tex_top, w - 284), "screenGame.ini", "0 0".into()));
    }
    for p in &l.presets {
        let frame = format!("{} 0 0 0 0 284 {} 0", p.tex_carv, p.h - 600);
        v.extend(anim(p.anim_carv, frame, "screenGame.ini", format!("{} 600", p.w - 284)));
    }
    v
}

fn accel_lines(l: &Layout) -> Vec<String> {
    let mut v = Vec::new();
    for p in &l.presets {
        v.extend([
            format!("[SCREEN{}]", p.accel),
            "Count=0".into(),
            format!("Size={} {}", p.w, p.h),
            "Type=normal".into(),
            format!("AnimBGID={}", p.accel),
            "FontID=2".into(),
            String::new(),
        ]);
        let frame = format!("{} 0 0 0 0 {} {} 0", p.map_tex, p.map_w, p.map_h);
        v.extend(anim(p.accel, frame, "accelMap.ini", format!("{} {}", p.map_x, p.map_y)));
    }
    v
}

/// Lines of an ini text (`\r\n` or `\n`), without line ends.
fn lines(t: &[u8]) -> Vec<&[u8]> {
    let mut v: Vec<&[u8]> = t.split(|&c| c == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l)).collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

fn header(line: &[u8]) -> Option<String> {
    let t = String::from_utf8_lossy(line);
    let t = t.trim();
    t.strip_prefix('[').and_then(|s| s.strip_suffix(']')).map(|s| s.trim().to_ascii_uppercase())
}

/// `base` without the sections that `extra` defines (a base that already has them, e.g. from an older build), then
/// one blank line and `extra`; `\r\n` line ends.
fn append(base: &[u8], extra: &[String]) -> Vec<u8> {
    let ours: HashSet<String> = extra.iter().filter_map(|s| header(s.as_bytes())).collect();
    let mut out = Vec::with_capacity(base.len() + extra.len() * 24);
    let mut skip = false;
    for line in lines(base) {
        if let Some(h) = header(line) {
            skip = ours.contains(&h);
        }
        if !skip {
            out.extend_from_slice(line);
            out.extend_from_slice(b"\r\n");
        }
    }
    while out.ends_with(b"\r\n\r\n") {
        out.truncate(out.len() - 2);
    }
    if !out.is_empty() && !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
    for s in extra {
        out.extend_from_slice(s.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// `[TEXLIST0]` of screenGame.ini: `ids` added to `IDs=` (those not listed yet), `Count=` = number of ids.
fn texlist(base: &[u8], ids: &[u32]) -> Option<Vec<u8>> {
    let ls = lines(base);
    let start = ls.iter().position(|l| header(l).as_deref() == Some("TEXLIST0"))?;
    let end = ls[start + 1..].iter().position(|l| header(l).is_some()).map_or(ls.len(), |i| start + 1 + i);
    let key = |l: &[u8], k: &[u8]| l.len() >= k.len() && l[..k.len()].eq_ignore_ascii_case(k);
    let ci = (start + 1..end).find(|&i| key(ls[i], b"count="))?;
    let ii = (start + 1..end).find(|&i| key(ls[i], b"ids="))?;
    let mut list: Vec<String> =
        String::from_utf8_lossy(&ls[ii][4..]).split_whitespace().map(str::to_string).collect();
    for id in ids {
        let s = id.to_string();
        if !list.contains(&s) {
            list.push(s);
        }
    }
    let count = format!("Count={}", list.len()).into_bytes();
    let idl = format!("IDs={}", list.join(" ")).into_bytes();
    let mut out = Vec::with_capacity(base.len() + ids.len() * 4);
    for (i, l) in ls.iter().enumerate() {
        let s: &[u8] = if i == ci {
            &count
        } else if i == ii {
            &idl
        } else {
            l
        };
        out.extend_from_slice(s);
        out.extend_from_slice(b"\r\n");
    }
    Some(out)
}

/// 8-bit BMP, top-down pixel rows.
struct Bmp {
    w: usize,
    h: usize,
    pal: Vec<u8>,
    px: Vec<u8>,
}

impl Bmp {
    fn parse(b: &[u8]) -> Result<Bmp, String> {
        let u16at = |o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
        let u32at = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
        if b.get(..2) != Some(b"BM") || u16at(28) != Some(8) || u32at(30) != Some(0) {
            return Err("not an 8-bit BMP".into());
        }
        let bad = || "broken BMP".to_string();
        let off = u32at(10).ok_or_else(bad)? as usize;
        let w = u32at(18).ok_or_else(bad)? as i32;
        let h0 = u32at(22).ok_or_else(bad)? as i32;
        let pal_at = 14 + u32at(14).ok_or_else(bad)? as usize;
        if w <= 0 || h0 == 0 || pal_at > off {
            return Err(bad());
        }
        let (w, h) = (w as usize, h0.unsigned_abs() as usize);
        let stride = (w + 3) & !3;
        if b.len() < off + stride * h {
            return Err(bad());
        }
        let mut px = vec![0u8; w * h];
        for y in 0..h {
            let sy = if h0 > 0 { h - 1 - y } else { y };
            px[y * w..y * w + w].copy_from_slice(&b[off + sy * stride..off + sy * stride + w]);
        }
        Ok(Bmp { w, h, pal: b[pal_at..off].to_vec(), px })
    }

    fn encode(&self) -> Vec<u8> {
        let stride = (self.w + 3) & !3;
        let off = 54 + self.pal.len();
        let size = off + stride * self.h;
        let mut b = vec![0u8; size];
        b[..2].copy_from_slice(b"BM");
        b[2..6].copy_from_slice(&(size as u32).to_le_bytes());
        b[10..14].copy_from_slice(&(off as u32).to_le_bytes());
        b[14..18].copy_from_slice(&40u32.to_le_bytes());
        b[18..22].copy_from_slice(&(self.w as i32).to_le_bytes());
        b[22..26].copy_from_slice(&(self.h as i32).to_le_bytes());
        b[26..28].copy_from_slice(&1u16.to_le_bytes());
        b[28..30].copy_from_slice(&8u16.to_le_bytes());
        b[34..38].copy_from_slice(&((stride * self.h) as u32).to_le_bytes());
        b[46..50].copy_from_slice(&((self.pal.len() / 4) as u32).to_le_bytes());
        b[54..off].copy_from_slice(&self.pal);
        for y in 0..self.h {
            let o = off + (self.h - 1 - y) * stride;
            b[o..o + self.w].copy_from_slice(&self.px[y * self.w..y * self.w + self.w]);
        }
        b
    }
}

/// Top bar: both 16 px end caps kept, the inner part repeated.
fn top_bar(a: &Bmp, nw: u32) -> Bmp {
    const CAP: usize = 16;
    let nw = nw as usize;
    let inner = a.w - 2 * CAP;
    let mut px = vec![0u8; nw * a.h];
    for y in 0..a.h {
        for x in 0..nw {
            let sx = if x < CAP {
                x
            } else if x >= nw - CAP {
                a.w - (nw - x)
            } else {
                CAP + (x - CAP) % inner
            };
            px[y * nw + x] = a.px[y * a.w + sx];
        }
    }
    Bmp { w: nw, h: a.h, pal: a.pal.clone(), px }
}

/// Carving: n stacked panels (n = round(h / source height)); each keeps its 30 px top / bottom moldings, the middle
/// is cropped around the centre (shorter) or stretched (taller).
fn carving(e: &Bmp, nh: u32) -> Bmp {
    const T: usize = 30;
    const B: usize = 30;
    let nh = nh as usize;
    let mid = e.h - T - B;
    let n = ((nh as f64 / e.h as f64).round() as usize).max(1);
    let mut px = vec![0u8; e.w * nh];
    let mut y0 = 0;
    for k in 0..n {
        let ph = ((nh * (k + 1)) as f64 / n as f64).round() as usize - y0;
        let pm = ph.saturating_sub(T + B);
        for y in 0..ph {
            let sy = if y < T {
                y
            } else if y >= ph - B {
                e.h - (ph - y)
            } else if pm <= mid {
                T + (mid - pm) / 2 + (y - T)
            } else {
                T + (y - T) * mid / pm
            };
            let d = (y0 + y) * e.w;
            px[d..d + e.w].copy_from_slice(&e.px[sy * e.w..sy * e.w + e.w]);
        }
        y0 += ph;
    }
    Bmp { w: e.w, h: nh, pal: e.pal.clone(), px }
}
