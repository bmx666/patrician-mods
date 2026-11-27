//! `AIM.dll`: replacement of the image decoder of Patrician II / III (Ascaron, 2000), with the same 32 exports
//! (C++ names, `__cdecl`) and results, and without the self-modifying blitters of the original (which make x86
//! translators such as Rosetta 2 slow, see `nosmc`).
//!
//! The renderer `ddraw_dll.dll` loads it and calls `AIM_INIT`, `AIM_CONVERT_MEMFILE` / `AIM_GETFILEFORMAT_FROMMEM`
//! (decode a file from an archive), `AIM_CONVERT_RAW` (convert to the screen format), `AIM_SAVE_BITMAP` (screenshots)
//! and `AIM_FREE`. Formats read: BMP, TGA, SGI RGB, JPEG (through the games' `ijl11.dll`, as the original) and AIM.
//!
//! Not implemented, as the games do not use them: writing AIM files (`AIM_OPEN_WRITE` and `AIM_WRITE_*` fail), AIM
//! tiled layers (type 4) and JPEG layers with an alpha mask (0x1f), DXT compression, `AIM_GEN_MIPMAPLEVEL` and the
//! `CAIM` class (kept as an empty shell).
// 32-bit Windows only: addresses are kept in dwords, as in the original's structures.
#![cfg(all(windows, target_arch = "x86"))]
#![allow(clippy::missing_safety_doc, non_snake_case)]

mod codec;
mod img;
mod pixels;
mod save;
mod sys;

use codec::{Data, Reader};
use img::*;

/// Dword of `AIM_IMAGE` holding the address of the internal block.
const BLOCK: usize = 22;

unsafe fn block(image: *mut u32) -> *mut Pair {
    if image.is_null() {
        return core::ptr::null_mut();
    }
    *image.add(BLOCK) as usize as *mut Pair
}

/// Copies the 11 dwords of the source and destination of `AIM_IMAGE` into the block.
unsafe fn load(image: *mut u32, p: *mut Pair) {
    for i in 0..11 {
        (*p)[i] = *image.add(i);
        (*p)[D + i] = *image.add(11 + i);
    }
    set_env(&*p);
}

/// Copies the source and destination of the block back into `AIM_IMAGE`.
unsafe fn store(image: *mut u32, p: *mut Pair) {
    for i in 0..11 {
        *image.add(i) = (*p)[i];
        *image.add(11 + i) = (*p)[D + i];
    }
}

/// Gives the buffers that point into `data` (a file being read) copies of their own.
unsafe fn own_copies(p: *mut Pair, data: &[u8]) {
    let lo = data.as_ptr() as usize as i32;
    let hi = lo.wrapping_add(data.len() as i32);
    for o in [0, D] {
        let q = &mut *p;
        let pix = q[o + PIX] as i32;
        if lo <= pix && pix < hi {
            let n = size_of(q, o);
            let copy_at = sys::alloc(n);
            img::copy(copy_at, q[o + PIX], n as i32);
            q[o + PIX] = copy_at;
            q[o + FLAGS] |= 1;
        }
        let pal = q[o + PAL] as i32;
        if lo <= pal && pal < hi {
            let copy_at = sys::alloc(0x400);
            img::copy(copy_at, q[o + PAL], 0x400);
            q[o + PAL] = copy_at;
            q[o + FLAGS] |= 2;
        }
    }
}

#[export_name = "?AIM_INIT@@YAXPAUAIM_IMAGE@@@Z"]
pub unsafe extern "C" fn aim_init(image: *mut u32) {
    if image.is_null() {
        return;
    }
    for i in 0..23 {
        *image.add(i) = 0;
    }
    *image.add(BLOCK) = sys::alloc(core::mem::size_of::<Pair>() as u32);
}

#[export_name = "?AIM_FREE@@YAXPAUAIM_IMAGE@@@Z"]
pub unsafe extern "C" fn aim_free(image: *mut u32) {
    if image.is_null() {
        return;
    }
    let p = block(image);
    if !p.is_null() {
        free(&mut *p, true, true);
        sys::free(p as usize as u32);
    }
    for i in 0..23 {
        *image.add(i) = 0;
    }
}

#[export_name = "?AIM_CONVERT_MEMFILE@@YAHPAUAIM_IMAGE@@PBDPAEJ@Z"]
pub unsafe extern "C" fn aim_convert_memfile(image: *mut u32, name: *const u8, data: *mut u8, size: i32) -> i32 {
    let p = block(image);
    if p.is_null() || data.is_null() || size <= 9 {
        return 0;
    }
    load(image, p);
    if codec::decode(p, name, Data { base: data as usize as u32, size }) {
        store(image, p);
        return 1;
    }
    0
}

#[export_name = "?AIM_CONVERT_FILE@@YAHPAUAIM_IMAGE@@PBD@Z"]
pub unsafe extern "C" fn aim_convert_file(image: *mut u32, name: *const u8) -> i32 {
    let p = block(image);
    if p.is_null() {
        return 0;
    }
    let Some(data) = sys::read_file(name) else { return 0 };
    load(image, p);
    if !codec::decode(p, name, Data { base: data.as_ptr() as usize as u32, size: data.len() as i32 }) {
        return 0;
    }
    own_copies(p, &data);
    store(image, p);
    1
}

#[export_name = "?AIM_CONVERT_RAW@@YAHPAUAIM_IMAGE@@@Z"]
pub unsafe extern "C" fn aim_convert_raw(image: *mut u32) -> i32 {
    let p = block(image);
    if p.is_null() {
        return 0;
    }
    load(image, p);
    if convert(p) {
        store(image, p);
        return 1;
    }
    0
}

#[export_name = "?AIM_GETFILEFORMAT_FROMMEM@@YAHPAUAIM_GFXDEF@@PBDPAEJ@Z"]
pub unsafe extern "C" fn aim_getfileformat_frommem(gfx: *mut u32, name: *const u8, data: *mut u8, size: i32) -> i32 {
    codec::inspect(gfx, name, Data { base: data as usize as u32, size }) as i32
}

#[export_name = "?AIM_GETFILEFORMAT_FROMFILE@@YAHPAUAIM_GFXDEF@@PBD@Z"]
pub unsafe extern "C" fn aim_getfileformat_fromfile(gfx: *mut u32, name: *const u8) -> i32 {
    let Some(data) = sys::read_file(name) else { return 0 };
    codec::inspect(gfx, name, Data { base: data.as_ptr() as usize as u32, size: data.len() as i32 }) as i32
}

#[export_name = "?AIM_CONVERT_FILE_FROMARCHIVE@@YAHPAUAIM_IMAGE@@PBDPAX@Z"]
pub unsafe extern "C" fn aim_convert_file_fromarchive(_image: *mut u32, _name: *const u8, _archive: *mut u8) -> i32 {
    0
}

#[export_name = "?AIM_GETFILEFORMAT_FROMARCHIVE@@YAHPAUAIM_GFXDEF@@PBDPAX@Z"]
pub unsafe extern "C" fn aim_getfileformat_fromarchive(_gfx: *mut u32, _name: *const u8, _archive: *mut u8) -> i32 {
    0
}

#[export_name = "?AIM_SAVE_BITMAP@@YAHPAUAIM_IMAGE@@JPBD@Z"]
pub unsafe extern "C" fn aim_save_bitmap(image: *mut u32, mode: i32, name: *const u8) -> i32 {
    let p = block(image);
    if p.is_null() {
        return 0;
    }
    load(image, p);
    save::save(p, mode, name) as i32
}

/// Not implemented (halves the image with box filters in the original); fails.
#[export_name = "?AIM_GEN_MIPMAPLEVEL@@YAHPAUAIM_IMAGE@@@Z"]
pub unsafe extern "C" fn aim_gen_mipmaplevel(image: *mut u32) -> i32 {
    let p = block(image);
    if !p.is_null() {
        load(image, p);
    }
    0
}

/// Hands the destination buffers to the caller: `AIM_FREE` no longer frees them.
#[export_name = "?AIM_DETACH@@YAXPAUAIM_IMAGE@@@Z"]
pub unsafe extern "C" fn aim_detach(image: *mut u32) {
    let p = block(image);
    if !p.is_null() {
        (*p)[D + FLAGS] &= !3;
    }
}

#[export_name = "?AIM_SETENV@@YAXPAUAIM_IMAGE@@K@Z"]
pub unsafe extern "C" fn aim_setenv(image: *mut u32, env: u32) {
    let p = block(image);
    if !p.is_null() {
        (*p)[ENV] = env;
    }
}

#[export_name = "?AIM_SETVENDOR@@YAHHH@Z"]
pub extern "C" fn aim_setvendor(_a: i32, _b: i32) -> i32 {
    1
}

// ---- Layer reader of AIM files in memory.

#[export_name = "?AIM_OPEN_READ@@YAPAXPBEH@Z"]
pub unsafe extern "C" fn aim_open_read(data: *const u8, size: i32) -> *mut Reader {
    match Reader::open(Data { base: data as usize as u32, size }) {
        Some(reader) => Box::into_raw(Box::new(reader)),
        None => core::ptr::null_mut(),
    }
}

#[export_name = "?AIM_CLOSE@@YAXPAX@Z"]
pub unsafe extern "C" fn aim_close(reader: *mut Reader) {
    if !reader.is_null() {
        let mut reader = Box::from_raw(reader);
        reader.close();
    }
}

#[export_name = "?AIM_READ_LAYER@@YAHPAXPAUAIM_LAYER@@@Z"]
pub unsafe extern "C" fn aim_read_layer(reader: *mut Reader, layer: *mut u8) -> i32 {
    if reader.is_null() || layer.is_null() {
        return -1;
    }
    (*reader).read_header(layer)
}

#[export_name = "?AIM_GET_LAYER_SIZE@@YAHPAX@Z"]
pub extern "C" fn aim_get_layer_size(_reader: *mut Reader) -> i32 {
    -1
}

#[export_name = "?AIM_SEEK_START@@YAHPAX@Z"]
pub extern "C" fn aim_seek_start(_reader: *mut Reader) -> i32 {
    -1
}

#[export_name = "?AIM_SKIP_LAYER@@YAHPAX@Z"]
pub unsafe extern "C" fn aim_skip_layer(reader: *mut Reader) -> i32 {
    if reader.is_null() {
        return -1;
    }
    (*reader).skip()
}

#[export_name = "?AIM_GET_LAYER_TYPE@@YAHPAXPAH@Z"]
pub unsafe extern "C" fn aim_get_layer_type(reader: *mut Reader, kind: *mut i32) -> i32 {
    if reader.is_null() {
        return -1;
    }
    match (*reader).peek() {
        Some(k) => {
            if !kind.is_null() {
                *kind = k as i32;
            }
            0
        }
        None => -1,
    }
}

#[export_name = "?AIM_DECODE_BITMAP@@YAHPAXPAUAIM_IMAGE@@@Z"]
pub unsafe extern "C" fn aim_decode_bitmap(reader: *mut Reader, image: *mut u32) -> i32 {
    let p = block(image);
    if reader.is_null() || p.is_null() {
        return -1;
    }
    load(image, p);
    free(&mut *p, true, false);
    let result = codec::decode_layer(&mut *reader, p, 3);
    store(image, p);
    result
}

// ---- Writing AIM files: not implemented (the games only read them).

#[export_name = "?AIM_OPEN_WRITE@@YAPAXPBD@Z"]
pub extern "C" fn aim_open_write(_name: *const u8) -> *mut u8 {
    core::ptr::null_mut()
}

#[export_name = "?AIM_WRITE_LAYER@@YAHPAXPAUAIM_LAYER@@@Z"]
pub extern "C" fn aim_write_layer(_writer: *mut u8, _layer: *mut u8) -> i32 {
    -1
}

#[export_name = "?AIM_WRITE_IMAGE@@YAHPAXPADPAUAIM_LAYER@@PAUAIM_GFXDEF@@@Z"]
pub extern "C" fn aim_write_image(_writer: *mut u8, _name: *mut u8, _layer: *mut u8, _gfx: *mut u32) -> i32 {
    -1
}

#[export_name = "?AIM_WRITE_SQUIZIMAGE@@YAHPAXPADPAUAIM_LAYER@@PAUAIM_GFXDEF@@@Z"]
pub extern "C" fn aim_write_squizimage(_writer: *mut u8, _name: *mut u8, _layer: *mut u8, _gfx: *mut u32) -> i32 {
    -1
}

#[export_name = "?AIM_WRITE_MIPMAPIMAGE@@YAHPAXPADHPAUAIM_LAYER@@PAUAIM_GFXDEF@@@Z"]
pub extern "C" fn aim_write_mipmapimage(
    _writer: *mut u8,
    _name: *mut u8,
    _levels: i32,
    _layer: *mut u8,
    _gfx: *mut u32,
) -> i32 {
    -1
}

// ---- The CAIM class of the original: an object holding only its virtual table.

/// Virtual table of `CAIM`: the scalar deleting destructor, which leaves the memory to its owner.
#[export_name = "??_7CAIM@@6B@"]
pub static CAIM_VFTABLE: [unsafe extern "thiscall" fn(*mut u32, u32) -> *mut u32; 1] = [caim_deleting_destructor];

unsafe extern "thiscall" fn caim_deleting_destructor(this: *mut u32, _flags: u32) -> *mut u32 {
    this
}

unsafe fn caim_set_vftable(this: *mut u32) {
    if !this.is_null() {
        *this = CAIM_VFTABLE.as_ptr() as usize as u32;
    }
}

#[export_name = "??0CAIM@@QAE@XZ"]
pub unsafe extern "thiscall" fn caim_construct(this: *mut u32) -> *mut u32 {
    caim_set_vftable(this);
    this
}

#[export_name = "??0CAIM@@QAE@ABV0@@Z"]
pub unsafe extern "thiscall" fn caim_copy_construct(this: *mut u32, _other: *const u32) -> *mut u32 {
    caim_set_vftable(this);
    this
}

#[export_name = "??1CAIM@@UAE@XZ"]
pub unsafe extern "thiscall" fn caim_destruct(this: *mut u32) {
    caim_set_vftable(this);
}

#[export_name = "??4CAIM@@QAEAAV0@ABV0@@Z"]
pub unsafe extern "thiscall" fn caim_assign(this: *mut u32, _other: *const u32) -> *mut u32 {
    this
}
