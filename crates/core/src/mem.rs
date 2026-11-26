//! Raw memory access in the game process (32-bit addresses), protected writes, executable arena for hook code.
use crate::ffi::*;
use core::ffi::c_void;
use std::sync::Mutex;

#[inline]
pub unsafe fn rd8(va: u32) -> u8 {
    core::ptr::read_volatile(va as *const u8)
}
#[inline]
pub unsafe fn rd16(va: u32) -> u16 {
    core::ptr::read_unaligned(va as *const u16)
}
#[inline]
pub unsafe fn rd32(va: u32) -> u32 {
    core::ptr::read_unaligned(va as *const u32)
}
#[inline]
pub unsafe fn wr8(va: u32, v: u8) {
    core::ptr::write_volatile(va as *mut u8, v)
}
#[inline]
pub unsafe fn wr16(va: u32, v: u16) {
    core::ptr::write_unaligned(va as *mut u16, v)
}
#[inline]
pub unsafe fn wr32(va: u32, v: u32) {
    core::ptr::write_unaligned(va as *mut u32, v)
}

/// Bytes of the running image (read-only view).
pub unsafe fn bytes(va: u32, len: usize) -> &'static [u8] {
    core::slice::from_raw_parts(va as *const u8, len)
}

/// Write `data` at `va` (code or read-only data): page protection lifted for the write, then restored.
pub unsafe fn write(va: u32, data: &[u8]) -> Result<(), String> {
    let mut old = 0u32;
    if VirtualProtect(va as *mut c_void, data.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
        return Err(format!("VirtualProtect {va:#x} failed ({})", GetLastError()));
    }
    core::ptr::copy_nonoverlapping(data.as_ptr(), va as *mut u8, data.len());
    let mut tmp = 0u32;
    VirtualProtect(va as *mut c_void, data.len(), old, &mut tmp);
    FlushInstructionCache(GetCurrentProcess(), va as *const c_void, data.len());
    Ok(())
}

/// Executable memory for hook code (bump allocator over 64 KB RWX blocks, never freed).
pub fn alloc_code(len: usize) -> Result<u32, String> {
    static ARENA: Mutex<(u32, u32)> = Mutex::new((0, 0));
    let len = (len + 15) & !15;
    let mut a = ARENA.lock().map_err(|_| "arena lock".to_string())?;
    if a.0 == 0 || a.0 + len as u32 > a.1 {
        let size = len.max(0x10000);
        let p = unsafe { VirtualAlloc(core::ptr::null_mut(), size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) };
        if p.is_null() {
            return Err(format!("VirtualAlloc failed ({})", unsafe { GetLastError() }));
        }
        *a = (p as u32, p as u32 + size as u32);
    }
    let at = a.0;
    a.0 += len as u32;
    Ok(at)
}

/// Little-endian bytes of a dword.
pub fn le(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// `e8` / `e9` + rel32 from `at` (5-byte instruction) to `to`.
pub fn rel5(op: u8, at: u32, to: u32) -> [u8; 5] {
    let r = to.wrapping_sub(at.wrapping_add(5)).to_le_bytes();
    [op, r[0], r[1], r[2], r[3]]
}

/// Hex string -> bytes (`"8b0d"` -> `[0x8b, 0x0d]`); spaces ignored. Panics on bad input (constants only).
pub fn hex(s: &str) -> Vec<u8> {
    let d: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    assert!(d.len() % 2 == 0, "odd hex string");
    d.chunks(2)
        .map(|p| u8::from_str_radix(core::str::from_utf8(p).expect("hex"), 16).expect("hex"))
        .collect()
}

/// Concatenate byte pieces: `cat(&[&hex("a0"), &le(addr)])`.
pub fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}
