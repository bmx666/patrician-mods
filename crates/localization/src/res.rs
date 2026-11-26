//! Language files of the running game ([`Pack`]: the loose folder `mods\localization\<lang>\<p2|p3>\`, else the
//! archive `mods\localization\<lang>.cpr`; P2 and P3 files of the same name differ). Every game resource (pictures,
//! ini, scripts, fonts, ...) is opened through `archiver.dll`, by the game and by `ddraw_dll.dll`; the archive
//! container of the game reads only the `.cpr` archives (P2) or the game folder first (P3). The entries of `archiver.dll` are detoured (the same 5-byte
//! prologue in the P2 and P3 builds):
//! - `arc_CreateFile(container, name)`, `arc_CreateMemMapped(container, name, &ptr, &len)`
//! - `cpr_OpenFile(name, container)`: own body in the P3 build (prologue checked); in P2 it calls `arc_CreateFile`
//!
//! A relative `name` (`.\` / `./` prefixes dropped, `/` = `\`, case ignored) of the language is opened from the loose
//! folder (container NULL = file on disk) or from the archive: `localization.dll` opens it as a container of its own
//! (`arc_CreateContainerEx(path, 0)` = archives only, no disk, + `arc_AppendArchive`) and asks it for the entry
//! `<p2|p3>\<name>`. Anything else, or a failed open, goes to the original call. Both are indexed once at start; a
//! file added later is not seen.
use crate::pack::Pack;
use pcore::ffi::*;
use pcore::game::Base;
use pcore::hook::Plan;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;

/// `push esi; mov esi,[esp+8]`: arc_CreateFile / arc_CreateMemMapped, P2 and P3.
const PROLOGUE: [u8; 5] = [0x56, 0x8b, 0x74, 0x24, 0x08];
/// `push esi; mov esi,[esp+0Ch]`: cpr_OpenFile of the P3 build (container = 2nd argument).
const PROLOGUE_CPR: [u8; 5] = [0x56, 0x8b, 0x74, 0x24, 0x0c];

/// Longest path handed to the archiver (its own buffers are MAX_PATH).
const MAX_PATH: usize = 260;

struct Index {
    /// ANSI `<game>\mods\localization\<lang>\<p2|p3>\`.
    dir: Vec<u8>,
    /// Keys of the loose folder.
    loose: HashSet<String>,
    /// Keys of the archive -> entry name (`<p2|p3>\...`) in [`CONTAINER`].
    packed: HashMap<String, Vec<u8>>,
}

/// Where a language file is opened from; the path or entry name is in the buffer of [`redirect`].
enum Src {
    Disk,
    Archive,
}

static INDEX: RwLock<Option<Index>> = RwLock::new(None);

static CREATE_FILE: AtomicU32 = AtomicU32::new(0);
static MEM_MAPPED: AtomicU32 = AtomicU32::new(0);
static CPR_OPEN: AtomicU32 = AtomicU32::new(0);
/// Archiver container of the language archive.
static CONTAINER: AtomicU32 = AtomicU32::new(0);

type CreateFile = unsafe extern "C" fn(u32, *const u8) -> u32;
type MemMapped = unsafe extern "C" fn(u32, *const u8, *mut u32, *mut u32) -> u32;
type CprOpen = unsafe extern "C" fn(*const u8, u32) -> u32;
type CreateContainerEx = unsafe extern "C" fn(*const u8, u32) -> u32;
type AppendArchive = unsafe extern "C" fn(u32, *const u8) -> u32;
type ReleaseContainer = unsafe extern "C" fn(u32);

/// Index the language files, open the archive and detour the archiver. Ok = summary for the log.
pub fn install(lang: &str, base: Base, pack: &Pack) -> Result<String, String> {
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
    let game = game_folder()?;
    let mut dir = game.clone();
    dir.extend_from_slice(format!("mods\\localization\\{lang}\\{}\\", base.dir()).as_bytes());
    let loose: HashSet<String> = pack.loose().map(str::to_string).collect();
    let mut packed: HashMap<String, Vec<u8>> = pack.packed().map(|(k, n)| (k.to_string(), n.to_vec())).collect();
    let mut archive = match &pack.error {
        Some(e) => format!(", archive skipped ({e})"),
        None => String::new(),
    };
    if pack.archive().is_some() {
        let mut path = game;
        path.extend_from_slice(format!("mods\\localization\\{lang}.cpr\0").as_bytes());
        match open_container(&path, &export) {
            Ok(c) => CONTAINER.store(c, Ordering::Relaxed),
            Err(e) => {
                packed.clear();
                archive = format!(", archive skipped ({e})");
            }
        }
    }
    let (n_loose, n_packed) = (loose.len(), packed.len());
    *INDEX.write().map_err(|_| "index lock")? = Some(Index { dir, loose, packed });
    let mut p = Plan::new();
    let t = p.detour(export("arc_CreateFile")?, &PROLOGUE, create_file as *const () as usize as u32)?;
    CREATE_FILE.store(t, Ordering::Relaxed);
    let t = p.detour(export("arc_CreateMemMapped")?, &PROLOGUE, mem_mapped as *const () as usize as u32)?;
    MEM_MAPPED.store(t, Ordering::Relaxed);
    let cpr = export("cpr_OpenFile")?;
    let cpr_own = p.check(cpr, &PROLOGUE_CPR).is_ok();
    if cpr_own {
        let t = p.detour(cpr, &PROLOGUE_CPR, cpr_open as *const () as usize as u32)?;
        CPR_OPEN.store(t, Ordering::Relaxed);
    }
    let n = p.commit()?;
    Ok(format!(
        "{n_loose} loose file(s), {n_packed} archived file(s){archive}, {n} archiver entries{}",
        if cpr_own { " incl. cpr_OpenFile" } else { "" }
    ))
}

/// ANSI game folder with the trailing `\`.
fn game_folder() -> Result<Vec<u8>, String> {
    let mut buf = [0u8; MAX_PATH];
    let len = unsafe { GetModuleFileNameA(core::ptr::null_mut(), buf.as_mut_ptr(), MAX_PATH as u32) } as usize;
    let exe = &buf[..len.min(MAX_PATH)];
    exe.iter().rposition(|&c| c == b'\\').map(|i| exe[..=i].to_vec()).ok_or_else(|| "no game folder".into())
}

/// Container with only the archive at `path` (ANSI, NUL-terminated).
fn open_container(path: &[u8], export: &dyn Fn(&str) -> Result<u32, String>) -> Result<u32, String> {
    if path.len() > MAX_PATH {
        return Err("path too long".into());
    }
    unsafe {
        let create: CreateContainerEx = core::mem::transmute(export("arc_CreateContainerEx")? as usize);
        let append: AppendArchive = core::mem::transmute(export("arc_AppendArchive")? as usize);
        let release: ReleaseContainer = core::mem::transmute(export("arc_ReleaseContainer")? as usize);
        let c = create(path.as_ptr(), 0);
        if c == 0 {
            return Err("arc_CreateContainerEx failed".into());
        }
        if append(c, path.as_ptr()) == 0 {
            release(c);
            return Err("archiver.dll did not open it".into());
        }
        Ok(c)
    }
}

/// Source of `name` when the language has it; the full path (disk) or the entry name (archive), NUL-terminated,
/// in `out`.
unsafe fn redirect(name: *const u8, out: &mut [u8; MAX_PATH]) -> Option<Src> {
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
    // ANSI bytes, read as Latin-1 (`ü` = 0xFC, as the archive entries and the folder keys).
    let raw: Vec<u8> = s.iter().map(|&c| if c == b'/' { b'\\' } else { c.to_ascii_lowercase() }).collect();
    let key: String = raw.iter().map(|&c| c as char).collect();
    let key = key.as_str();
    let guard = INDEX.read().ok()?;
    let ix = guard.as_ref()?;
    let put = |out: &mut [u8; MAX_PATH], parts: &[&[u8]]| {
        let len: usize = parts.iter().map(|p| p.len()).sum();
        if len >= MAX_PATH {
            return false;
        }
        let mut at = 0;
        for p in parts {
            out[at..at + p.len()].copy_from_slice(p);
            at += p.len();
        }
        out[at] = 0;
        true
    };
    if ix.loose.contains(key) && put(out, &[ix.dir.as_slice(), raw.as_slice()]) {
        return Some(Src::Disk);
    }
    match ix.packed.get(key) {
        Some(entry) if put(out, &[entry.as_slice()]) => Some(Src::Archive),
        _ => None,
    }
}

/// Container to pass for a redirected `src`.
fn container(src: Src) -> u32 {
    match src {
        Src::Disk => 0,
        Src::Archive => CONTAINER.load(Ordering::Relaxed),
    }
}

unsafe extern "C" fn create_file(container: u32, name: *const u8) -> u32 {
    let orig: CreateFile = core::mem::transmute(CREATE_FILE.load(Ordering::Relaxed) as usize);
    let mut path = [0u8; MAX_PATH];
    if let Some(src) = redirect(name, &mut path) {
        let f = orig(self::container(src), path.as_ptr());
        if f != 0 {
            return f;
        }
    }
    orig(container, name)
}

unsafe extern "C" fn mem_mapped(container: u32, name: *const u8, ptr: *mut u32, len: *mut u32) -> u32 {
    let orig: MemMapped = core::mem::transmute(MEM_MAPPED.load(Ordering::Relaxed) as usize);
    let mut path = [0u8; MAX_PATH];
    if let Some(src) = redirect(name, &mut path) {
        let m = orig(self::container(src), path.as_ptr(), ptr, len);
        if m != 0 {
            return m;
        }
    }
    orig(container, name, ptr, len)
}

unsafe extern "C" fn cpr_open(name: *const u8, container: u32) -> u32 {
    let orig: CprOpen = core::mem::transmute(CPR_OPEN.load(Ordering::Relaxed) as usize);
    let mut path = [0u8; MAX_PATH];
    if let Some(src) = redirect(name, &mut path) {
        let f = orig(path.as_ptr(), self::container(src));
        if f != 0 {
            return f;
        }
    }
    orig(name, container)
}
