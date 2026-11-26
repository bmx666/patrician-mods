//! PE header fields of the game executable, from the file (launcher) or the running image (winmm.dll, mods).

#[derive(Clone, Copy, Debug)]
pub struct PeInfo {
    pub timestamp: u32,
    pub entry_rva: u32,
    pub image_base: u32,
    pub size_of_image: u32,
    pub dll_characteristics: u16,
}

/// IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE: the image may be relocated (the games are not).
pub const DYNAMIC_BASE: u16 = 0x40;

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Offset of the PE signature, checked ("MZ", "PE\0\0", 32-bit optional header).
fn nt(b: &[u8]) -> Option<usize> {
    if b.get(0..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(b, 0x3c)? as usize;
    if b.get(pe..pe + 4)? != b"PE\0\0" || u16_at(b, pe + 24)? != 0x10b {
        return None;
    }
    Some(pe)
}

pub fn parse(b: &[u8]) -> Option<PeInfo> {
    let pe = nt(b)?;
    Some(PeInfo {
        timestamp: u32_at(b, pe + 8)?,
        entry_rva: u32_at(b, pe + 24 + 16)?,
        image_base: u32_at(b, pe + 24 + 28)?,
        size_of_image: u32_at(b, pe + 24 + 56)?,
        dll_characteristics: u16_at(b, pe + 24 + 70)?,
    })
}

/// Header of the running executable (its first page).
pub fn current() -> Option<(u32, PeInfo)> {
    let base = unsafe { crate::ffi::GetModuleHandleW(core::ptr::null()) } as usize;
    if base == 0 {
        return None;
    }
    let header = unsafe { core::slice::from_raw_parts(base as *const u8, 0x1000) };
    parse(header).map(|info| (base as u32, info))
}

/// File offset of an RVA (section table of the file).
fn rva_to_offset(b: &[u8], rva: u32) -> Option<usize> {
    let pe = nt(b)?;
    let sections = u16_at(b, pe + 6)? as usize;
    let table = pe + 24 + u16_at(b, pe + 20)? as usize;
    for i in 0..sections {
        let s = table + i * 40;
        let (vsize, va, raw_size, raw) = (u32_at(b, s + 8)?, u32_at(b, s + 12)?, u32_at(b, s + 16)?, u32_at(b, s + 20)?);
        if rva >= va && rva < va + vsize.max(raw_size) {
            return Some((rva - va + raw) as usize);
        }
    }
    None
}

fn c_str(b: &[u8], o: usize) -> Option<&[u8]> {
    let s = b.get(o..)?;
    Some(&s[..s.iter().position(|&c| c == 0)?])
}

/// RVA of the import address table slot of `dll!func` (imported by name), from the executable file.
pub fn import_slot(b: &[u8], dll: &str, func: &str) -> Option<u32> {
    let pe = nt(b)?;
    let dir = u32_at(b, pe + 24 + 104)?;
    let mut d = rva_to_offset(b, dir)?;
    loop {
        let (lookup, name, first) = (u32_at(b, d)?, u32_at(b, d + 12)?, u32_at(b, d + 16)?);
        if name == 0 && first == 0 {
            return None;
        }
        if c_str(b, rva_to_offset(b, name)?)?.eq_ignore_ascii_case(dll.as_bytes()) {
            let mut t = rva_to_offset(b, if lookup != 0 { lookup } else { first })?;
            for i in 0.. {
                let entry = u32_at(b, t)?;
                if entry == 0 {
                    break;
                }
                if entry & 0x8000_0000 == 0 && c_str(b, rva_to_offset(b, entry)? + 2)? == func.as_bytes() {
                    return Some(first + 4 * i);
                }
                t += 4;
            }
        }
        d += 20;
    }
}
