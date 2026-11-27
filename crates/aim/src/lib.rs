//! `AIM.dll`: replacement of the image decoder of Patrician II / III (Ascaron, 2000), with the same 32 exports
//! (C++ names, `__cdecl`) and results, and without the self-modifying blitters of the original (which make x86
//! translators such as Rosetta 2 slow, see `nosmc`).
//!
//! The renderer `ddraw_dll.dll` loads it and calls `AIM_INIT`, `AIM_CONVERT_MEMFILE` / `AIM_GETFILEFORMAT_FROMMEM`
//! (decode a file from an archive), `AIM_CONVERT_RAW` (convert to the screen format), `AIM_SAVE_BITMAP` (screenshots)
//! and `AIM_FREE`. Formats read: BMP, TGA, SGI RGB, JPEG and AIM (all layer and sub-layer types); written: BMP, TGA,
//! JPEG (`AIM_SAVE_BITMAP`) and AIM (`AIM_OPEN_WRITE`, `AIM_WRITE_*`). Conversions include DXT compression and
//! mipmap generation (`AIM_GEN_MIPMAPLEVEL`).
//!
//! JPEG, the LZ of AIM chunks and DXT compression come from `aimcodec` (pure Rust, tested on any host): the original
//! calls `ijl11.dll` for JPEG and DirectDraw for DXT compression, so neither is needed.
//!
//! Additions over the original (`[aim]` in `mods\settings.ini`, see [`textures`]): PNG files are read; pictures in
//! `mods\textures\` replace the game's of the same name, at any resolution; conversions to another size scale
//! smoothly (bilinear / area averaging) instead of by nearest pixel. Two exports of [`direct`] serve the renderer of
//! patrician-ddraw: decoding straight into its upload format, and a hook of the decoder of `AIM_CONVERT_MEMFILE` in
//! place of its patch of the original's code. The `CAIM` class is kept as
//! the empty shell it is in the original (its constructor and destructor only set up and release DirectDraw).
// 32-bit Windows only: addresses are kept in dwords, as in the original's structures.
#![cfg(all(windows, target_arch = "x86"))]
#![allow(clippy::missing_safety_doc, non_snake_case)]

mod codec;
mod direct;
mod img;
mod mip;
mod pixels;
mod save;
mod squiz;
mod sys;
mod textures;
mod write;

use codec::{Data, Reader};
use img::*;
use write::Writer;

/// The stream object of `AIM_OPEN_READ` / `AIM_OPEN_WRITE` (one class in the original, in read or write mode).
pub enum Stream {
    Read(Reader),
    Write(Writer),
}

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
pub(crate) unsafe fn own_copies(p: *mut Pair, data: &[u8]) {
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
    // The original calls its decoder here (RVA 0x2984); the renderer may hook it (`AIM_HOOK_MEMFILE_DECODER`).
    if direct::memfile_decoder()(p, name, data, size) != 0 {
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
    let file = Data { base: data.as_ptr() as usize as u32, size: data.len() as i32 };
    let ok = match textures::decode(p, name, file) {
        Some(ok) => ok,
        None => codec::decode(p, name, file),
    };
    if !ok {
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
    let data = Data { base: data as usize as u32, size };
    textures::inspect(gfx, name, data).unwrap_or_else(|| codec::inspect(gfx, name, data)) as i32
}

#[export_name = "?AIM_GETFILEFORMAT_FROMFILE@@YAHPAUAIM_GFXDEF@@PBD@Z"]
pub unsafe extern "C" fn aim_getfileformat_fromfile(gfx: *mut u32, name: *const u8) -> i32 {
    let Some(data) = sys::read_file(name) else { return 0 };
    let data = Data { base: data.as_ptr() as usize as u32, size: data.len() as i32 };
    textures::inspect(gfx, name, data).unwrap_or_else(|| codec::inspect(gfx, name, data)) as i32
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

/// Halves the source into the destination (its size when set, else half the source).
#[export_name = "?AIM_GEN_MIPMAPLEVEL@@YAHPAUAIM_IMAGE@@@Z"]
pub unsafe extern "C" fn aim_gen_mipmaplevel(image: *mut u32) -> i32 {
    let p = block(image);
    if p.is_null() {
        return 0;
    }
    load(image, p);
    if mip::mipmap(p) {
        store(image, p);
        return 1;
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

// ---- Layer streams: reading AIM files in memory, writing AIM files.

#[export_name = "?AIM_OPEN_READ@@YAPAXPBEH@Z"]
pub unsafe extern "C" fn aim_open_read(data: *const u8, size: i32) -> *mut Stream {
    match Reader::open(Data { base: data as usize as u32, size }) {
        Some(reader) => Box::into_raw(Box::new(Stream::Read(reader))),
        None => core::ptr::null_mut(),
    }
}

#[export_name = "?AIM_OPEN_WRITE@@YAPAXPBD@Z"]
pub unsafe extern "C" fn aim_open_write(name: *const u8) -> *mut Stream {
    match Writer::open(name) {
        Some(writer) => Box::into_raw(Box::new(Stream::Write(writer))),
        None => core::ptr::null_mut(),
    }
}

/// Ends a stream (a written file is closed) and frees it.
#[export_name = "?AIM_CLOSE@@YAXPAX@Z"]
pub unsafe extern "C" fn aim_close(stream: *mut Stream) {
    if !stream.is_null() {
        let mut stream = Box::from_raw(stream);
        if let Stream::Read(reader) = &mut *stream {
            reader.close();
        }
    }
}

/// The reader of a stream; a write stream used for reading goes into its error state, as in the original.
unsafe fn reader<'a>(stream: *mut Stream) -> Option<&'a mut Reader> {
    match stream.as_mut()? {
        Stream::Read(reader) => Some(reader),
        Stream::Write(writer) => {
            writer.error = true;
            None
        }
    }
}

/// The writer of a stream; a read stream used for writing goes into its error state.
unsafe fn writer<'a>(stream: *mut Stream) -> Option<&'a mut Writer> {
    match stream.as_mut()? {
        Stream::Write(writer) => Some(writer),
        Stream::Read(reader) => {
            reader.close();
            None
        }
    }
}

#[export_name = "?AIM_READ_LAYER@@YAHPAXPAUAIM_LAYER@@@Z"]
pub unsafe extern "C" fn aim_read_layer(stream: *mut Stream, layer: *mut u8) -> i32 {
    match reader(stream) {
        Some(reader) if !layer.is_null() => reader.read_header(layer),
        _ => -1,
    }
}

/// Not available in the original either (always -1); exported under both names, as there.
#[export_name = "?AIM_GET_LAYER_SIZE@@YAHPAX@Z"]
pub extern "C" fn aim_get_layer_size(_stream: *mut Stream) -> i32 {
    -1
}

#[export_name = "?AIM_SEEK_START@@YAHPAX@Z"]
pub extern "C" fn aim_seek_start(_stream: *mut Stream) -> i32 {
    -1
}

#[export_name = "?AIM_SKIP_LAYER@@YAHPAX@Z"]
pub unsafe extern "C" fn aim_skip_layer(stream: *mut Stream) -> i32 {
    match stream.as_mut() {
        Some(Stream::Read(reader)) => reader.skip(),
        _ => -1,
    }
}

#[export_name = "?AIM_GET_LAYER_TYPE@@YAHPAXPAH@Z"]
pub unsafe extern "C" fn aim_get_layer_type(stream: *mut Stream, kind: *mut i32) -> i32 {
    let Some(Stream::Read(reader)) = stream.as_mut() else { return -1 };
    match reader.peek() {
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
pub unsafe extern "C" fn aim_decode_bitmap(stream: *mut Stream, image: *mut u32) -> i32 {
    let p = block(image);
    let Some(Stream::Read(reader)) = stream.as_mut() else { return -1 };
    if p.is_null() {
        return -1;
    }
    load(image, p);
    free(&mut *p, true, false);
    let result = codec::decode_layer(reader, p, 3);
    store(image, p);
    result
}

#[export_name = "?AIM_WRITE_LAYER@@YAHPAXPAUAIM_LAYER@@@Z"]
pub unsafe extern "C" fn aim_write_layer(stream: *mut Stream, layer: *mut u8) -> i32 {
    match writer(stream) {
        Some(writer) => writer.write_layer(layer),
        None => -1,
    }
}

#[export_name = "?AIM_WRITE_IMAGE@@YAHPAXPADPAUAIM_LAYER@@PAUAIM_GFXDEF@@@Z"]
pub unsafe extern "C" fn aim_write_image(stream: *mut Stream, name: *mut u8, layer: *mut u8, gfx: *mut u32) -> i32 {
    match writer(stream) {
        Some(writer) => writer.image(name, layer, gfx),
        None => -1,
    }
}

#[export_name = "?AIM_WRITE_SQUIZIMAGE@@YAHPAXPADPAUAIM_LAYER@@PAUAIM_GFXDEF@@@Z"]
pub unsafe extern "C" fn aim_write_squizimage(stream: *mut Stream, name: *mut u8, layer: *mut u8, gfx: *mut u32) -> i32 {
    match writer(stream) {
        Some(writer) => writer.squiz(name, layer, gfx, write::SQUIZ_BLOCK),
        None => -1,
    }
}

#[export_name = "?AIM_WRITE_MIPMAPIMAGE@@YAHPAXPADHPAUAIM_LAYER@@PAUAIM_GFXDEF@@@Z"]
pub unsafe extern "C" fn aim_write_mipmapimage(
    stream: *mut Stream,
    name: *mut u8,
    levels: i32,
    layer: *mut u8,
    gfx: *mut u32,
) -> i32 {
    match stream.as_mut() {
        None => -1,
        Some(Stream::Write(writer)) => writer.mipmaps(name, levels, layer, gfx),
        // A read stream goes into its error state (the original fails when it writes the layer header).
        Some(Stream::Read(reader)) => {
            reader.close();
            -1
        }
    }
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
