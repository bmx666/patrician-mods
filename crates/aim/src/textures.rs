//! Settings of `[aim]` in `mods\settings.ini` and replacement textures in `mods\textures\`.
//!
//! A replacement is a `.png`, `.jpg`, `.bmp` or `.tga` file whose path below `mods\textures\`, without the extension,
//! ends the name the game asks for, without its extension (names ignore case; `/` and `\` are the same): the file
//! `mods\textures\images\town.png` replaces `images\town.bmp` and `x:\game\images\town.aim`, `mods\textures\town.png`
//! any `town.*`; the longest match wins. The folder is listed once, at the first picture.
//!
//! A replacement is decoded to 32-bit BGRA (format 4) in place of the game's file, whatever the format of the
//! game's picture (a palette picture comes back as BGRA when the caller asks for no conversion). With `texture_size = original` (default) it
//! is scaled to the size of the game's picture with [`aimcodec::scale`] (area averaging when larger, bilinear when
//! smaller), so pictures drawn more finely (a 2x or 4x master) or simply edited ones fit wherever the game puts them;
//! with `texture_size = native` it keeps its own size, for callers that scale pictures themselves. When the
//! replacement cannot be read, the game's file is decoded as without it.

use crate::codec::{self, Data};
use crate::img::*;
use pcore::{ini::Ini, log, paths};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Scaling filter of the conversions to another size (`[aim] filter`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// As the original: nearest pixel, bilinear when the game sets environment bit 0 (`original`).
    Original,
    /// Always nearest pixel (`nearest`).
    Nearest,
    /// Bilinear enlargement, area-averaging reduction, alpha-weighted; palette images by nearest pixel (`smooth`,
    /// default).
    Smooth,
}

pub struct Config {
    pub filter: Filter,
    /// Replacement textures from `mods\textures\` (`textures`, default on).
    pub textures: bool,
    /// Replacements keep their own size (`texture_size = native`) instead of the game picture's (`original`).
    pub native: bool,
}

pub fn config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let ini = Ini::load(paths::settings().as_deref());
        let filter = match ini.get("aim", "filter").map(|v| v.to_ascii_lowercase()).as_deref() {
            Some("original") => Filter::Original,
            Some("nearest") => Filter::Nearest,
            _ => Filter::Smooth,
        };
        let native = ini.get("aim", "texture_size").is_some_and(|v| v.eq_ignore_ascii_case("native"));
        Config { filter, textures: ini.flag("aim", "textures", true), native }
    })
}

/// Extensions of replacement files.
const EXTENSIONS: [&str; 4] = ["png", "jpg", "bmp", "tga"];

/// A name as a lookup key: lower case, `\` separators, without the extension.
fn key(name: &str) -> String {
    let name = name.to_ascii_lowercase().replace('/', "\\");
    let stem_end = match (name.rfind('.'), name.rfind('\\')) {
        (Some(dot), Some(sep)) if dot > sep => dot,
        (Some(dot), None) => dot,
        _ => name.len(),
    };
    name[..stem_end].to_string()
}

fn scan(dir: &Path, root: &Path, index: &mut HashMap<String, PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan(&path, root, index);
            continue;
        }
        let known = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)));
        if let (true, Ok(relative)) = (known, path.strip_prefix(root)) {
            index.entry(key(&relative.to_string_lossy())).or_insert(path);
        }
    }
}

fn index() -> &'static HashMap<String, PathBuf> {
    static INDEX: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index = HashMap::new();
        if let Some(root) = paths::mods_dir().map(|d| d.join("textures")) {
            scan(&root, &root, &mut index);
        }
        if !index.is_empty() {
            log!("aim", "{} replacement textures in mods\\textures", index.len());
        }
        index
    })
}

/// The replacement file of a picture name, if any.
unsafe fn find(name: *const u8) -> Option<&'static PathBuf> {
    if name.is_null() || !config().textures {
        return None;
    }
    let index = index();
    if index.is_empty() {
        return None;
    }
    let name = std::ffi::CStr::from_ptr(name.cast()).to_string_lossy();
    let full = key(&name);
    let mut rest = full.as_str();
    loop {
        if let Some(path) = index.get(rest) {
            return Some(path);
        }
        rest = &rest[rest.find('\\')? + 1..];
    }
}

/// A NUL-terminated name with the extension of `path`, for the format choice of [`codec`].
fn decoder_name(path: &Path) -> Vec<u8> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    format!("replacement.{ext}\0").into_bytes()
}

fn data_of(bytes: &[u8]) -> Data {
    Data { base: bytes.as_ptr() as usize as u32, size: bytes.len() as i32 }
}

/// Size of the game's picture.
unsafe fn original_size(name: *const u8, original: Data) -> Option<(u32, u32)> {
    let mut gfx = [0u32; 11];
    (codec::inspect(gfx.as_mut_ptr(), name, original) && gfx[W] as i32 > 0 && gfx[H] as i32 > 0)
        .then_some((gfx[W], gfx[H]))
}

/// Decodes the replacement of `name` (if any) into the block instead of the game's file `original`, then converts
/// it as the destination asks. `None` = no replacement or it cannot be read: decode the game's file.
pub unsafe fn decode(p: *mut Pair, name: *const u8, original: Data) -> Option<bool> {
    let path = find(name)?;
    let file = std::fs::read(path).ok()?;
    let q = &mut *p;
    let mut saved = [0u32; 32];
    saved.copy_from_slice(&q[..32]);
    // Decode into the source only; the destination comes back for the conversion.
    q[D..D + 16].fill(0);
    // BGRA, as `inspect` reports it.
    let mut ok = codec::decode(p, decoder_name(path).as_ptr(), data_of(&file)) && bgra_source(p);
    if ok && !config().native {
        if let Some((w, h)) = original_size(name, original) {
            if ((*p)[W], (*p)[H]) != (w, h) {
                ok = smooth_source(p, w, h);
            }
        }
    }
    if ok {
        crate::own_copies(p, &file);
    }
    let q = &mut *p;
    if !ok {
        log!("aim", "replacement {} cannot be read; the game's picture is used", path.display());
        // Buffers allocated for the replacement go; pointers into the file are not heap blocks.
        let (lo, hi) = (file.as_ptr() as usize as u32, file.as_ptr() as usize as u32 + file.len() as u32);
        let ours = |a: u32, i: usize| a != 0 && a != saved[i] && !(lo..hi).contains(&a);
        if q[FLAGS] & 1 != 0 && ours(q[PIX], PIX) {
            crate::sys::free(q[PIX]);
        }
        if q[FLAGS] & 2 != 0 && ours(q[PAL], PAL) {
            crate::sys::free(q[PAL]);
        }
        q[..32].copy_from_slice(&saved);
        return None;
    }
    q[D..D + 16].copy_from_slice(&saved[D..D + 16]);
    Some(q[D + FMT] == 0 || convert(p))
}

/// Fills `AIM_GFXDEF` for a picture with a replacement: the game's descriptor (other fields), then BGRA at the size
/// [`decode`] gives. `None` = no replacement or it cannot be read.
pub unsafe fn inspect(gfx: *mut u32, name: *const u8, original: Data) -> Option<bool> {
    let path = find(name)?;
    if gfx.is_null() {
        return None;
    }
    let size = if config().native {
        let file = std::fs::read(path).ok()?;
        let mut local = [0u32; 11];
        if !codec::inspect(local.as_mut_ptr(), decoder_name(path).as_ptr(), data_of(&file)) {
            return None;
        }
        (local[W], local[H])
    } else {
        original_size(name, original)?
    };
    // The game's file fills the fields the replacement does not have (an AIM picture's whole descriptor).
    codec::inspect(gfx, name, original);
    let mut local: Pair = [0; 33];
    set_format(&mut local, 0, size.0, size.1, 4);
    for i in [W, H, BPP, FMT, PITCH] {
        *gfx.add(i) = local[i];
    }
    Some(true)
}
