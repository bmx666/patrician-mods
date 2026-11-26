//! Language files from `mods\localization\<lang>\`. Every game resource (pictures, ini, scripts, fonts, ...) is opened
//! through `archiver.dll`, by the game and by `ddraw_dll.dll`; the archive container of the game reads only the
//! `.cpr` archives (P2) or the game folder first (P3). The entries of `archiver.dll` are detoured (the same 5-byte
//! prologue in the P2 and P3 builds):
//! - `arc_CreateFile(container, name)`, `arc_CreateMemMapped(container, name, &ptr, &len)`
//! - `cpr_OpenFile(name, container)`: own body in the P3 build (prologue checked); in P2 it calls `arc_CreateFile`
//!
//! A relative `name` (`.\` / `./` prefixes dropped, `/` = `\`, case ignored) that exists in the language folder is
//! opened from there (container NULL = file on disk); anything else, or a failed open, goes to the original call.
//! The folder is indexed once at start; a file added later is not seen.
use pcore::ffi::*;
use pcore::hook::Plan;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;

/// `push esi; mov esi,[esp+8]`: arc_CreateFile / arc_CreateMemMapped, P2 and P3.
const PROLOGUE: [u8; 5] = [0x56, 0x8b, 0x74, 0x24, 0x08];
/// `push esi; mov esi,[esp+0Ch]`: cpr_OpenFile of the P3 build (container = 2nd argument).
const PROLOGUE_CPR: [u8; 5] = [0x56, 0x8b, 0x74, 0x24, 0x0c];

/// Longest path handed to the archiver (its own buffers are MAX_PATH).
const MAX_PATH: usize = 260;

struct Index {
    /// ANSI `<game>\mods\localization\<lang>\`.
    dir: Vec<u8>,
    /// Relative paths of the folder, lower case, `\` separators.
    files: HashSet<Vec<u8>>,
}

static INDEX: RwLock<Option<Index>> = RwLock::new(None);

static CREATE_FILE: AtomicU32 = AtomicU32::new(0);
static MEM_MAPPED: AtomicU32 = AtomicU32::new(0);
static CPR_OPEN: AtomicU32 = AtomicU32::new(0);

type CreateFile = unsafe extern "C" fn(u32, *const u8) -> u32;
type MemMapped = unsafe extern "C" fn(u32, *const u8, *mut u32, *mut u32) -> u32;
type CprOpen = unsafe extern "C" fn(*const u8, u32) -> u32;

/// Index the language folder and detour the archiver. Ok = summary for the log.
pub fn install(lang: &str) -> Result<String, String> {
    let files = index(lang)?;
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
    Ok(format!("{files} file(s), {n} archiver entries{}", if cpr_own { " incl. cpr_OpenFile" } else { "" }))
}

/// Fill [`INDEX`] from `mods\localization\<lang>\`; a missing folder = no files. Returns the file count.
fn index(lang: &str) -> Result<usize, String> {
    let mut buf = [0u8; MAX_PATH];
    let len = unsafe { GetModuleFileNameA(core::ptr::null_mut(), buf.as_mut_ptr(), MAX_PATH as u32) } as usize;
    let exe = &buf[..len.min(MAX_PATH)];
    let game = exe.iter().rposition(|&c| c == b'\\').map(|i| &exe[..=i]).ok_or("no game folder")?;
    let mut dir = game.to_vec();
    dir.extend_from_slice(b"mods\\localization\\");
    dir.extend_from_slice(lang.as_bytes());
    dir.push(b'\\');
    let mut files = HashSet::new();
    if let Some(root) = pcore::paths::localization_dir(lang) {
        walk(&root, &mut Vec::new(), &mut files);
    }
    let n = files.len();
    *INDEX.write().map_err(|_| "index lock")? = Some(Index { dir, files });
    Ok(n)
}

fn walk(dir: &Path, rel: &mut Vec<u8>, out: &mut HashSet<Vec<u8>>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        let keep = rel.len();
        if !rel.is_empty() {
            rel.push(b'\\');
        }
        rel.extend_from_slice(name.as_bytes());
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&e.path(), rel, out),
            Ok(_) => {
                out.insert(rel.clone());
            }
            Err(_) => {}
        }
        rel.truncate(keep);
    }
}

/// Full path of `name` in the language folder, NUL-terminated in `out`, when the folder has it.
unsafe fn redirect(name: *const u8, out: &mut [u8; MAX_PATH]) -> bool {
    if name.is_null() {
        return false;
    }
    let mut s = core::ffi::CStr::from_ptr(name.cast()).to_bytes();
    while s.starts_with(b".\\") || s.starts_with(b"./") {
        s = &s[2..];
    }
    if s.is_empty() || s[0] == b'\\' || s[0] == b'/' || s.contains(&b':') {
        return false;
    }
    let key: Vec<u8> = s.iter().map(|&c| if c == b'/' { b'\\' } else { c.to_ascii_lowercase() }).collect();
    let Ok(guard) = INDEX.read() else { return false };
    let Some(ix) = guard.as_ref() else { return false };
    let len = ix.dir.len() + key.len();
    if len >= MAX_PATH || !ix.files.contains(&key) {
        return false;
    }
    out[..ix.dir.len()].copy_from_slice(&ix.dir);
    out[ix.dir.len()..len].copy_from_slice(&key);
    out[len] = 0;
    true
}

unsafe extern "C" fn create_file(container: u32, name: *const u8) -> u32 {
    let orig: CreateFile = core::mem::transmute(CREATE_FILE.load(Ordering::Relaxed) as usize);
    let mut path = [0u8; MAX_PATH];
    if redirect(name, &mut path) {
        let f = orig(0, path.as_ptr());
        if f != 0 {
            return f;
        }
    }
    orig(container, name)
}

unsafe extern "C" fn mem_mapped(container: u32, name: *const u8, ptr: *mut u32, len: *mut u32) -> u32 {
    let orig: MemMapped = core::mem::transmute(MEM_MAPPED.load(Ordering::Relaxed) as usize);
    let mut path = [0u8; MAX_PATH];
    if redirect(name, &mut path) {
        let m = orig(0, path.as_ptr(), ptr, len);
        if m != 0 {
            return m;
        }
    }
    orig(container, name, ptr, len)
}

unsafe extern "C" fn cpr_open(name: *const u8, container: u32) -> u32 {
    let orig: CprOpen = core::mem::transmute(CPR_OPEN.load(Ordering::Relaxed) as usize);
    let mut path = [0u8; MAX_PATH];
    if redirect(name, &mut path) {
        let f = orig(path.as_ptr(), 0);
        if f != 0 {
            return f;
        }
    }
    orig(name, container)
}
