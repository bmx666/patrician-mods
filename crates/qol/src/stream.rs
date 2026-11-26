//! Stream open by the file on disk. Every `AIL_open_stream(dig, name, size)` of the game goes through the import slot
//! (6 call sites, P2 also a jump thunk), which is redirected to [`open_stream`]:
//! - name exists -> unchanged
//! - missing and extension .mp3 / .aac (any case) -> the same name with the other extension when that file exists
//!   (Miles picks Mp3dec.asi / Aacdec.asi by the extension)
//! - neither -> unchanged name: AIL_open_stream returns 0 = silence (the game then skips playback)
use crate::util::*;
use core::cell::UnsafeCell;
use pcore::ffi::{GetFileAttributesA, INVALID_FILE_ATTRIBUTES};
use pcore::game::Base;
use std::sync::atomic::{AtomicU32, Ordering};

static ORIG: AtomicU32 = AtomicU32::new(0);

type OpenStream = unsafe extern "system" fn(u32, *const u8, i32) -> u32;

pub fn stream_open(p: &mut Plan, base: Base) -> Result<(), String> {
    let (iat, sites): (u32, [u32; 6]) = match base {
        Base::P2 => {
            p.check(0x5defd6, &cat(&[&hex("ff25"), &le(0x61a754)]))?; // jmp [61A754h]
            (0x61a754, [0x43c605, 0x43c847, 0x43caaa, 0x43cbec, 0x43cda7, 0x43cf85])
        }
        Base::P3 => (0x66a5cc, [0x442325, 0x44256b, 0x4427cc, 0x442911, 0x442ad7, 0x442cb5]),
    };
    for va in sites {
        p.check(va, &cat(&[&hex("ff15"), &le(iat)]))?; // call [iat]
    }
    let orig = unsafe { rd32(iat) };
    if orig == 0 {
        return Err("AIL_open_stream import not resolved".into());
    }
    ORIG.store(orig, Ordering::Relaxed);
    p.write(iat, le(open_stream as *const () as usize as u32).to_vec());
    Ok(())
}

/// Name buffer for the swapped extension (the game opens streams from one thread; Miles copies the name).
struct NameBuf(UnsafeCell<[u8; 260]>);
unsafe impl Sync for NameBuf {}
static BUF: NameBuf = NameBuf(UnsafeCell::new([0; 260]));

unsafe extern "system" fn open_stream(dig: u32, name: *const u8, size: i32) -> u32 {
    let orig: OpenStream = core::mem::transmute(ORIG.load(Ordering::Relaxed) as usize);
    if name.is_null() || GetFileAttributesA(name) != INVALID_FILE_ATTRIBUTES {
        return orig(dig, name, size);
    }
    let s = core::ffi::CStr::from_ptr(name.cast()).to_bytes();
    // last '.' after the last '\', followed by exactly 3 characters
    let Some(dot) = s.iter().rposition(|&c| c == b'.' || c == b'\\').filter(|&i| s[i] == b'.') else {
        return orig(dig, name, size);
    };
    if s.len() != dot + 4 || s.len() >= 259 {
        return orig(dig, name, size);
    }
    let other: &[u8; 3] = match s[dot + 1..].to_ascii_lowercase().as_slice() {
        b"mp3" => b"aac",
        b"aac" => b"mp3",
        _ => return orig(dig, name, size),
    };
    let buf = &mut *BUF.0.get();
    buf[..=dot].copy_from_slice(&s[..=dot]);
    buf[dot + 1..dot + 4].copy_from_slice(other);
    buf[dot + 4] = 0;
    if GetFileAttributesA(buf.as_ptr()) == INVALID_FILE_ATTRIBUTES {
        return orig(dig, name, size);
    }
    orig(dig, buf.as_ptr(), size)
}
