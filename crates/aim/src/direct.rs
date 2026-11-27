//! Exports for the renderer `ddraw_Dll.dll` of patrician-ddraw, not in the original (plain C names, `__cdecl`). The
//! renderer looks them up with `GetProcAddress` and keeps its old path when they are missing (the original AIM.dll).
//!
//! - `AIM_HOOK_MEMFILE_DECODER` swaps the decoder that `AIM_CONVERT_MEMFILE` calls. The renderer replaces the
//!   fast-forward map (`images/Vollansichtskarte1280.bmp`) with one of the screen size in it. On the original it patches
//!   the call to that decoder (RVA 0x2984) after checking the hash of the code, which fails on this DLL.
//! - `AIM_DECODE_DIRECT` decodes a file straight into a buffer of the caller, already in the layout the renderer
//!   uploads to Direct3D: 8-bit palette rows padded to 4 bytes with the 256 BGRA entries, or 32-bit BGRA rows of
//!   `width * 4` bytes. Every other format (BGR, 16-bit, DXT, 4-bit palette) is converted to BGRA in the same call.
//!   This replaces `AIM_INIT` / `AIM_CONVERT_MEMFILE` / `AIM_CONVERT_RAW` / `AIM_FREE` plus the renderer's copy and
//!   its BGR-to-BGRA pass.

use crate::codec::{self, Data};
use crate::img::*;
use crate::textures;
use core::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The decoder of `AIM_CONVERT_MEMFILE`: block, name, file, size; nonzero on success.
pub type MemfileDecoder = unsafe extern "C" fn(*mut Pair, *const u8, *mut u8, i32) -> i32;

/// The hooked decoder; 0 = [`decode_memfile`].
static MEMFILE_DECODER: AtomicUsize = AtomicUsize::new(0);

/// Decodes a file into the source of the block (a replacement texture first), then converts it when the destination
/// has a format.
pub unsafe extern "C" fn decode_memfile(p: *mut Pair, name: *const u8, data: *mut u8, size: i32) -> i32 {
    if p.is_null() || data.is_null() || size <= 9 {
        return 0;
    }
    let data = Data { base: data as usize as u32, size };
    let ok = match textures::decode(p, name, data) {
        Some(ok) => ok,
        None => codec::decode(p, name, data),
    };
    ok as i32
}

/// The decoder `AIM_CONVERT_MEMFILE` calls now.
pub fn memfile_decoder() -> MemfileDecoder {
    match MEMFILE_DECODER.load(Ordering::Acquire) {
        0 => decode_memfile,
        // Only `aim_hook_memfile_decoder` stores it, from a function pointer.
        f => unsafe { core::mem::transmute::<usize, MemfileDecoder>(f) },
    }
}

/// Makes `AIM_CONVERT_MEMFILE` call `hook` instead of its decoder and returns the decoder it called until now, which
/// the hook calls on (`hook` null = back to the built-in decoder). The hook gets the internal block of the image as
/// an opaque pointer.
#[export_name = "AIM_HOOK_MEMFILE_DECODER"]
pub unsafe extern "C" fn aim_hook_memfile_decoder(hook: Option<MemfileDecoder>) -> MemfileDecoder {
    let previous = MEMFILE_DECODER.swap(hook.map_or(0, |f| f as usize), Ordering::AcqRel);
    match previous {
        0 => decode_memfile,
        f => core::mem::transmute::<usize, MemfileDecoder>(f),
    }
}

/// The picture of `AIM_DECODE_DIRECT`.
#[repr(C)]
pub struct DirectImage {
    pub width: u32,
    pub height: u32,
    /// Bytes from one row to the next: `(width + 3) & !3` for format 1, `width * 4` for format 4.
    pub pitch: u32,
    /// 1 = 8-bit palette, 4 = BGRA (the AIM format numbers).
    pub format: u32,
    /// 1024 bytes of the caller that receive the palette (256 BGRA entries) of a format 1 picture; may be null.
    pub palette: *mut u8,
}

/// Allocator of the caller: `bytes` for the pixels, null when the memory is missing. It is called once per decode.
pub type Allocate = unsafe extern "C" fn(context: *mut c_void, bytes: u32) -> *mut u8;

/// Copies `rows` rows of `bytes` bytes between buffers that do not overlap.
unsafe fn copy_rows(dst: u32, dst_pitch: u32, src: u32, src_pitch: u32, bytes: u32, rows: u32) {
    for y in 0..rows {
        core::ptr::copy_nonoverlapping(
            (src + y * src_pitch) as usize as *const u8,
            (dst + y * dst_pitch) as usize as *mut u8,
            bytes as usize,
        );
    }
}

/// Size and layout of a picture of `w` x `h` in the renderer's format; `None` for sizes it does not take.
fn layout(w: u32, h: u32, indexed: bool) -> Option<(u32, u32)> {
    if w == 0 || h == 0 || w > 0xffff || h > 0xffff {
        return None;
    }
    let pitch = if indexed { (w + 3) & !3 } else { w * 4 };
    Some((pitch, pitch.checked_mul(h)?))
}

/// Writes the decoded source of `p` into a buffer of `allocate` in the renderer's format.
unsafe fn emit(p: *mut Pair, out: &mut DirectImage, allocate: Allocate, context: *mut c_void) -> i32 {
    let q = &mut *p;
    if !validate(q) {
        return 0;
    }
    let (w, h) = (q[W], q[H]);
    let indexed = q[FMT] == 1 && q[PAL] != 0;
    let Some((pitch, bytes)) = layout(w, h, indexed) else { return 0 };
    let buffer = allocate(context, bytes);
    if buffer.is_null() {
        return -1;
    }
    let dst = buffer as usize as u32;
    if indexed {
        copy_rows(dst, pitch, q[PIX], q[PITCH], w, h);
        if !out.palette.is_null() {
            copy(out.palette as usize as u32, q[PAL], 0x400);
        }
    } else if q[FMT] == 4 {
        copy_rows(dst, pitch, q[PIX], q[PITCH], w * 4, h);
    } else {
        // Same size: a plain conversion into the caller's buffer, which the block does not own.
        q[D..D + 16].fill(0);
        set_format(q, D, w, h, 4);
        q[D + PITCH] = pitch;
        q[D + PIX] = dst;
        let ok = convert(p);
        let q = &mut *p;
        if q[D + PIX] == dst {
            q[D + PIX] = 0;
            q[D + FLAGS] &= !1;
        }
        if !ok {
            return 0;
        }
    }
    *out = DirectImage { width: w, height: h, pitch, format: if indexed { 1 } else { 4 }, palette: out.palette };
    1
}

/// Decodes the file `data` (`size` bytes) named `name` (which picks the format and the replacement texture) into the
/// renderer's format: with `pixels` nonzero the pixels go to a buffer of `allocate` and the palette to
/// `image.palette`, otherwise only the size and format are filled (as `AIM_GETFILEFORMAT_FROMMEM`). 1 = done, 0 = the
/// file cannot be read, -1 = `allocate` returned null. `AIM_CONVERT_MEMFILE`'s hook is not called.
#[export_name = "AIM_DECODE_DIRECT"]
pub unsafe extern "C" fn aim_decode_direct(
    name: *const u8,
    data: *const u8,
    size: i32,
    pixels: i32,
    image: *mut DirectImage,
    allocate: Option<Allocate>,
    context: *mut c_void,
) -> i32 {
    if image.is_null() || data.is_null() || size <= 9 {
        return 0;
    }
    let out = &mut *image;
    let data = Data { base: data as usize as u32, size };
    if pixels == 0 {
        let mut gfx = [0u32; 11];
        let g = gfx.as_mut_ptr();
        if !textures::inspect(g, name, data).unwrap_or_else(|| codec::inspect(g, name, data)) {
            return 0;
        }
        let indexed = gfx[FMT] == 1;
        let Some((pitch, _)) = layout(gfx[W], gfx[H], indexed) else { return 0 };
        *out = DirectImage {
            width: gfx[W],
            height: gfx[H],
            pitch,
            format: if indexed { 1 } else { 4 },
            palette: out.palette,
        };
        return 1;
    }
    let Some(allocate) = allocate else { return 0 };
    let mut block: Box<Pair> = Box::new([0; 33]);
    let p: *mut Pair = &mut *block;
    set_env(&*p);
    let decoded = match textures::decode(p, name, data) {
        Some(ok) => ok,
        None => codec::decode(p, name, data),
    };
    let result = if decoded { emit(p, out, allocate, context) } else { 0 };
    free(&mut *p, true, true);
    result
}
