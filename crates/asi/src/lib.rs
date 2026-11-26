//! Shared glue of the Miles Sound System 6 ASI codecs (`Win32\*.asi`).
//!
//! Both games call `AIL_set_redist_directory("./win32")`. Miles then loads every `Win32\*.asi`. For a stream it
//! picks the "ASI codec" provider whose "Input file types" pattern ends with the file's extension, ignoring case
//! (Mss32 `RIB_find_file_provider`). A codec DLL registers its interfaces from `DllMain` ([`attach`] /
//! [`detach`]). The entry names, tokens and meaning copy the original `Mp3dec.asi` 6.0J.
//!
//! Streaming contract (binding):
//! - The fetch callback returning 0 means the Miles buffer is dry for now, not end of file.
//! - Input is pulled one frame at a time, never ahead. The Miles mixer (Mss32 0x21112578) ends a streamed sample
//!   as soon as all of its input bytes have been fetched, and drops PCM still buffered in the decoder. Any
//!   read-ahead cuts that much audio from the end.
//! - A decoder is never given a cut frame.
//! - seek(-2) marks a discontinuity: the decoder history is reset and decoded PCM is kept.

#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, Ordering};

/// Miles input callback: `bytes` bytes into `dest`, from `offset` (-1 = sequential). Returns the bytes read.
pub type Fetch = unsafe extern "system" fn(user: u32, dest: *mut u8, bytes: i32, offset: i32) -> i32;

/// What the codec reports for one decoded frame.
pub struct Frame {
    /// Input bytes consumed.
    pub used: usize,
    /// PCM bytes (interleaved 16-bit) in [`Codec::pcm`]; 0 = no output (e.g. bit reservoir not filled yet).
    pub pcm_bytes: usize,
    pub rate: i32,
    pub channels: i32,
    /// PCM samples per channel.
    pub samples: u32,
    /// Bit rate of the frame header (MPEG), 0 = none.
    pub kbps: i32,
    /// MPEG layer, 0 = none.
    pub layer: i32,
}

/// Values the codec uses to report the input bit rate.
pub struct Stats {
    /// File size passed by Miles.
    pub total: i32,
    /// ID3v2 TLEN (track length, ms), 0 = none.
    pub len_ms: i32,
    /// Sample rate of the stream, 0 = not known yet.
    pub rate: i32,
    /// Bit rate of the last frame header, 0 = none.
    pub kbps: i32,
    /// Frame bytes / PCM samples per channel decoded so far.
    pub frame_bytes: u64,
    pub frame_samples: u64,
}

/// One decoder format.
pub trait Codec: Sized + 'static {
    /// `PROVIDER_query_attribute` "Name", NUL-terminated.
    const NAME: &'static [u8];
    /// "Input file types": description, NUL, pattern, NUL, NUL.
    const INPUT_TYPES: &'static [u8];
    /// Input buffer size; larger than the longest frame.
    const INBUF: usize;
    /// "Maximum frame size" and "Minimum input block size".
    const BLOCK: i32;
    /// Bytes needed to read a frame length.
    const HEADER: usize;
    /// `ASI_error` text when no frame is found at open, NUL-terminated.
    const NO_DATA: &'static [u8];
    /// Extra stream attributes (name, token), answered by [`Codec::attribute`].
    const EXTRA: &'static [(&'static [u8], u32)] = &[];

    fn new() -> Option<Self>;
    /// Frame length incl. header at `h` ([`Codec::HEADER`] bytes), 0 = no frame starts here.
    fn frame_len(h: &[u8]) -> usize;
    /// Decode exactly one frame; `Err(n)` = drop `n` bytes (false sync) and resync.
    fn decode(&mut self, frame: &[u8]) -> Result<Frame, usize>;
    /// PCM of the last decoded frame.
    fn pcm(&self) -> &[u8];
    /// Clear the decoder history (discontinuity).
    fn reset(&mut self);
    /// "Input bit rate" in bit/s; Miles reads it once at open to get the stream length.
    fn bitrate(s: &Stats) -> i32;
    /// Value of a token of [`Codec::EXTRA`].
    fn attribute(_s: &Stream<Self>, _token: u32) -> i32 {
        -1
    }
}

/// One open stream (`ASI_stream_open` handle).
pub struct Stream<C: Codec> {
    user: u32,
    fetch: Fetch,
    req: [i32; 3],
    /// Input bytes fetched so far.
    pos: i32,
    /// -1 = sequential, else absolute offset for the next fetch.
    seek: i32,
    /// ID3v2 bytes still to drop.
    skip: usize,
    input: Vec<u8>,
    in_len: usize,
    in_pos: usize,
    pub rate: i32,
    pub channels: i32,
    pub layer: i32,
    out_len: usize,
    out_pos: usize,
    pub stats: Stats,
    dec: C,
}

impl<C: Codec> Stream<C> {
    /// Make `n` bytes available at `in_pos`, fetching only the missing ones. false = the callback is dry right
    /// now; what was fetched is kept for the next call.
    fn need(&mut self, n: usize) -> bool {
        if self.in_len - self.in_pos >= n {
            return true;
        }
        if self.in_pos > 0 {
            self.input.copy_within(self.in_pos..self.in_len, 0);
            self.in_len -= self.in_pos;
            self.in_pos = 0;
        }
        while self.in_len < n {
            let dest = self.input[self.in_len..].as_mut_ptr();
            let got = unsafe { (self.fetch)(self.user, dest, (n - self.in_len) as i32, self.seek) };
            if self.seek != -1 {
                self.pos = self.seek;
                self.seek = -1;
            }
            if got < 1 {
                return false;
            }
            let got = (got as usize).min(n - self.in_len);
            self.in_len += got;
            self.pos += got as i32;
        }
        true
    }

    /// Decode the next frame into the codec's PCM; false = no complete frame available now.
    fn next_frame(&mut self) -> bool {
        loop {
            while self.skip > 0 {
                let mut k = self.in_len - self.in_pos;
                if k == 0 {
                    // a partial fetch is still progress: the dry check is on what arrived
                    self.need(self.skip.min(C::INBUF));
                    k = self.in_len - self.in_pos;
                    if k == 0 {
                        return false;
                    }
                }
                let k = k.min(self.skip);
                self.in_pos += k;
                self.skip -= k;
            }
            if !self.need(C::HEADER.max(3)) {
                return false;
            }
            if self.input[self.in_pos..self.in_pos + 3] == *b"ID3" {
                if !self.need(10) {
                    return false;
                }
                let h = &self.input[self.in_pos..self.in_pos + 10];
                let size = 10 + syncsafe(&h[6..10]) as usize + if h[5] & 0x10 != 0 { 10 } else { 0 };
                if size <= C::INBUF && self.need(size) {
                    if let Some(ms) = id3_tlen(&self.input[self.in_pos..self.in_pos + size]) {
                        self.stats.len_ms = ms;
                    }
                    self.in_pos += size;
                } else {
                    self.skip = size;
                }
                continue;
            }
            let len = C::frame_len(&self.input[self.in_pos..self.in_pos + C::HEADER]);
            if len <= C::HEADER || len > C::INBUF {
                self.in_pos += 1; // junk: resync
                continue;
            }
            if !self.need(len) {
                return false;
            }
            let f = match self.dec.decode(&self.input[self.in_pos..self.in_pos + len]) {
                Ok(f) => f,
                Err(n) => {
                    self.in_pos += n.max(1);
                    continue;
                }
            };
            self.in_pos += f.used.min(len);
            if f.pcm_bytes == 0 || f.rate <= 0 || f.channels <= 0 {
                continue;
            }
            if self.rate == 0 {
                self.rate = f.rate;
                self.channels = f.channels;
                self.layer = f.layer;
                self.stats.rate = f.rate;
            } else if f.channels != self.channels {
                continue; // the Miles format is fixed at open
            }
            self.stats.kbps = f.kbps;
            self.stats.frame_bytes += len as u64;
            self.stats.frame_samples += u64::from(f.samples);
            self.out_len = f.pcm_bytes.min(self.dec.pcm().len());
            self.out_pos = 0;
            return true;
        }
    }

    /// Input position: fetched bytes minus those still buffered.
    pub fn position(&self) -> i32 {
        self.pos - (self.in_len - self.in_pos) as i32
    }
}

/// 28-bit ID3v2 syncsafe integer.
fn syncsafe(b: &[u8]) -> u32 {
    b.iter().fold(0, |v, &x| v << 7 | u32::from(x & 127))
}

/// ID3v2.3 / 2.4 tag: value of the TLEN text frame (ms).
fn id3_tlen(t: &[u8]) -> Option<i32> {
    let v = t[3];
    if !(3..=4).contains(&v) {
        return None;
    }
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let mut p = 10usize;
    if t[5] & 0x40 != 0 {
        let ext = t.get(10..14)?;
        p += if v == 4 { syncsafe(ext) as usize } else { 4 + be(ext) as usize };
    }
    while p + 10 <= t.len() && t[p] != 0 {
        let f = &t[p..];
        let size = if v == 4 { syncsafe(&f[4..8]) } else { be(&f[4..8]) } as usize;
        if size > t.len() - p - 10 {
            return None;
        }
        if &f[..4] == b"TLEN" && size > 1 && (f[10] == 0 || f[10] == 3) {
            let mut ms = 0i32;
            for &c in f[11..10 + size].iter().take_while(|c| c.is_ascii_digit()) {
                if ms >= 100_000_000 {
                    break;
                }
                ms = ms * 10 + i32::from(c - b'0');
            }
            return Some(ms);
        }
        p += 10 + size;
    }
    None
}

// ---- state of the DLL (one codec per DLL) ----

static ERROR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static STARTED: AtomicI32 = AtomicI32::new(0);
static PROVIDER: AtomicU32 = AtomicU32::new(0);

fn set_error(msg: &'static [u8]) {
    ERROR.store(msg.as_ptr().cast_mut(), Ordering::Relaxed);
}

// ---- codec interface ----

unsafe extern "system" fn query_attribute<C: Codec>(token: i32) -> i32 {
    match token {
        -100 => C::NAME.as_ptr() as i32,
        -101 => 0x100,
        0 => C::INPUT_TYPES.as_ptr() as i32,
        1 => b"Raw PCM files\0*.RAW\0\0".as_ptr() as i32,
        2 => C::BLOCK,
        _ => 0,
    }
}

unsafe extern "system" fn asi_startup() -> i32 {
    if STARTED.fetch_add(1, Ordering::SeqCst) > 0 {
        set_error(b"Already started\0");
        return 2;
    }
    0
}

unsafe extern "system" fn asi_error() -> *const u8 {
    ERROR.load(Ordering::Relaxed).cast_const()
}

unsafe extern "system" fn asi_shutdown() -> i32 {
    if STARTED.load(Ordering::SeqCst) <= 0 {
        set_error(b"Not initialized\0");
        return 8;
    }
    STARTED.fetch_sub(1, Ordering::SeqCst);
    0
}

// ---- stream interface ----

unsafe extern "system" fn stream_open<C: Codec>(user: u32, fetch: Fetch, total: i32) -> *mut Stream<C> {
    let Some(dec) = C::new() else {
        set_error(b"Decoder init failed\0");
        return core::ptr::null_mut();
    };
    let mut s = Box::new(Stream {
        user,
        fetch,
        req: [-1; 3],
        pos: 0,
        seek: 0,
        skip: 0,
        input: vec![0; C::INBUF],
        in_len: 0,
        in_pos: 0,
        rate: 0,
        channels: 0,
        layer: 0,
        out_len: 0,
        out_pos: 0,
        stats: Stats { total, len_ms: 0, rate: 0, kbps: 0, frame_bytes: 0, frame_samples: 0 },
        dec,
    });
    if !s.next_frame() {
        set_error(C::NO_DATA);
        return core::ptr::null_mut();
    }
    Box::into_raw(s)
}

unsafe extern "system" fn stream_close<C: Codec>(s: *mut Stream<C>) -> i32 {
    if !s.is_null() {
        drop(Box::from_raw(s));
    }
    0
}

unsafe extern "system" fn stream_process<C: Codec>(s: *mut Stream<C>, buf: *mut u8, size: i32) -> i32 {
    let s = &mut *s;
    let out = core::slice::from_raw_parts_mut(buf, size.max(0) as usize);
    let mut done = 0;
    while done < out.len() {
        if s.out_pos >= s.out_len && !s.next_frame() {
            out[done..].fill(0);
            break;
        }
        let n = (s.out_len - s.out_pos).min(out.len() - done);
        out[done..done + n].copy_from_slice(&s.dec.pcm()[s.out_pos..s.out_pos + n]);
        s.out_pos += n;
        done += n;
    }
    done as i32
}

/// -2 (Miles: the next buffer follows a discontinuity, loop / reposition): reset the decoder, drop the partial
/// frame, keep decoded PCM. offset >= 0: also drop pending PCM and fetch from that byte offset.
unsafe extern "system" fn stream_seek<C: Codec>(s: *mut Stream<C>, offset: i32) -> i32 {
    let s = &mut *s;
    s.dec.reset();
    s.in_len = 0;
    s.in_pos = 0;
    s.skip = 0;
    if offset != -2 {
        s.out_len = 0;
        s.out_pos = 0;
        s.seek = offset;
    }
    0
}

unsafe extern "system" fn stream_attribute<C: Codec>(s: *mut Stream<C>, token: u32) -> i32 {
    let s = &*s;
    let br = C::bitrate(&s.stats).max(1000);
    match token {
        3 => br,
        4 | 8 => s.rate,
        5 => 16 / (s.rate * s.channels * 16 / br).max(1),
        6 | 10 => s.channels,
        7 => s.rate * s.channels * 16,
        9 => 16,
        11 => s.position(),
        12 => {
            let total = s.stats.total;
            let f = if total != 0 { s.pos as f32 * 100.0 / total as f32 } else { 0.0 };
            f.to_bits() as i32
        }
        13 => C::BLOCK,
        16..=18 => s.req[(token - 16) as usize],
        _ if C::EXTRA.iter().any(|&(_, t)| t == token) => C::attribute(s, token),
        _ => -1,
    }
}

unsafe extern "system" fn stream_set_preference<C: Codec>(s: *mut Stream<C>, token: u32, value: *const i32) -> i32 {
    let s = &mut *s;
    match token {
        16..=18 => core::mem::replace(&mut s.req[(token - 16) as usize], *value),
        _ => -1,
    }
}

// ---- RIB registration ----

#[repr(C)]
struct Entry {
    ty: u32,
    name: *const u8,
    token: u32,
    subtype: u32,
}

const FUNCTION: u32 = 0;
const ATTRIBUTE: u32 = 1;
const PREFERENCE: u32 = 2;
const DEC: u32 = 2;
const HEX: u32 = 3;
const PERCENT: u32 = 5;
const STRING: u32 = 7;

fn func(name: &'static [u8], f: usize) -> Entry {
    Entry { ty: FUNCTION, name: name.as_ptr(), token: f as u32, subtype: 0 }
}

fn attr(ty: u32, name: &'static [u8], token: i32, subtype: u32) -> Entry {
    Entry { ty, name: name.as_ptr(), token: token as u32, subtype }
}

type Register = unsafe extern "system" fn(provider: u32, iface: *const u8, n: i32, e: *const Entry) -> i32;
type ProviderHandle = unsafe extern "system" fn() -> u32;

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
}

fn mss32(name: &[u8]) -> Option<*const c_void> {
    let m = unsafe { GetModuleHandleA(b"mss32.dll\0".as_ptr()) };
    if m.is_null() {
        return None;
    }
    let f = unsafe { GetProcAddress(m, name.as_ptr()) };
    (!f.is_null()).then_some(f)
}

/// `DLL_PROCESS_ATTACH`: register "ASI codec" and "ASI stream" of `C` with the loading Miles. Without `mss32.dll`
/// the DLL refuses to load (false).
pub fn attach<C: Codec>() -> bool {
    let (Some(handle), Some(reg)) = (mss32(b"_RIB_provider_library_handle@0\0"), mss32(b"RIB_register_interface\0"))
    else {
        return false;
    };
    let handle: ProviderHandle = unsafe { core::mem::transmute(handle) };
    let reg: Register = unsafe { core::mem::transmute(reg) };
    let codec = vec![
        func(b"PROVIDER_query_attribute\0", query_attribute::<C> as *const () as usize),
        attr(ATTRIBUTE, b"Name\0", -100, STRING),
        attr(ATTRIBUTE, b"Version\0", -101, HEX),
        attr(ATTRIBUTE, b"Input file types\0", 0, STRING),
        attr(ATTRIBUTE, b"Output file types\0", 1, STRING),
        attr(ATTRIBUTE, b"Maximum frame size\0", 2, DEC),
        func(b"ASI_startup\0", asi_startup as *const () as usize),
        func(b"ASI_error\0", asi_error as *const () as usize),
        func(b"ASI_shutdown\0", asi_shutdown as *const () as usize),
    ];
    let mut stream = vec![
        func(b"ASI_stream_open\0", stream_open::<C> as *const () as usize),
        func(b"ASI_stream_process\0", stream_process::<C> as *const () as usize),
        func(b"ASI_stream_attribute\0", stream_attribute::<C> as *const () as usize),
        func(b"ASI_stream_set_preference\0", stream_set_preference::<C> as *const () as usize),
        func(b"ASI_stream_seek\0", stream_seek::<C> as *const () as usize),
        func(b"ASI_stream_close\0", stream_close::<C> as *const () as usize),
        attr(ATTRIBUTE, b"Input bit rate\0", 3, DEC),
        attr(ATTRIBUTE, b"Input sample rate\0", 4, DEC),
        attr(ATTRIBUTE, b"Input sample width\0", 5, DEC),
        attr(ATTRIBUTE, b"Input channels\0", 6, DEC),
        attr(ATTRIBUTE, b"Output bit rate\0", 7, DEC),
        attr(ATTRIBUTE, b"Output sample rate\0", 8, DEC),
        attr(ATTRIBUTE, b"Output sample width\0", 9, DEC),
        attr(ATTRIBUTE, b"Output channels\0", 10, DEC),
        attr(ATTRIBUTE, b"Position\0", 11, DEC),
        attr(ATTRIBUTE, b"Percent done\0", 12, PERCENT),
        attr(ATTRIBUTE, b"Minimum input block size\0", 13, DEC),
    ];
    stream.extend(C::EXTRA.iter().map(|&(name, token)| attr(ATTRIBUTE, name, token as i32, DEC)));
    stream.extend([
        attr(PREFERENCE, b"Requested sample rate\0", 16, DEC),
        attr(PREFERENCE, b"Requested bit width\0", 17, DEC),
        attr(PREFERENCE, b"Requested # of channels\0", 18, DEC),
    ]);
    // Miles may keep the tables: they live as long as the DLL
    let (codec, stream) = (codec.leak(), stream.leak());
    unsafe {
        let provider = handle();
        PROVIDER.store(provider, Ordering::SeqCst);
        reg(provider, b"ASI codec\0".as_ptr(), codec.len() as i32, codec.as_ptr());
        reg(provider, b"ASI stream\0".as_ptr(), stream.len() as i32, stream.as_ptr());
    }
    true
}

/// `DLL_PROCESS_DETACH`: unregister every interface of the provider.
pub fn detach() {
    if let Some(unreg) = mss32(b"RIB_unregister_interface\0") {
        let unreg: Register = unsafe { core::mem::transmute(unreg) };
        unsafe { unreg(PROVIDER.load(Ordering::SeqCst), core::ptr::null(), 0, core::ptr::null()) };
    }
}

/// `DllMain` of a codec DLL: [`attach`] on process attach, [`detach`] on process detach.
#[macro_export]
macro_rules! dll_main {
    ($codec:ty) => {
        #[no_mangle]
        pub extern "system" fn DllMain(_module: *mut core::ffi::c_void, reason: u32, _reserved: *mut core::ffi::c_void) -> i32 {
            match reason {
                1 => $crate::attach::<$codec>() as i32,
                0 => {
                    $crate::detach();
                    1
                }
                _ => 1,
            }
        }
    };
}
