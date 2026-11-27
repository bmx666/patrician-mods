//! Name lookup bug of the Patrician II `archiver.dll`: always fixed by every mod that touches files ([`fix_names`]).
//!
//! The name compare (`10004c10`) and hash (`10004c90`) map each byte through a 256-entry lower-case table, but index
//! it with `movsx`: a byte >= 80h reads the 128 bytes before the table, which are the bucket array of the disk file
//! map (container 0). With no disk file open they are 0 (compare = end of name, hash += 0), so `M\xf6we` (CP1252, as
//! `textures.ini` writes it) found the archive entry `M\x94we` (OEM) by accident; once a mod serves a file from disk
//! (`orig(0, path)`) the buckets hold pointers and such names are no longer found (missing gulls and potters).
//!
//! Fix: `movsx` -> `movzx` at the three index loads, and the table is written here in full (its lazy init flag set):
//! ASCII upper -> lower, `/` -> `\`, the German letters of CP1252 and OEM (CP437/850) to one value each, every
//! other byte unchanged. Runs before any archive or file is opened through the archiver (mod `start` at the game
//! entry), so all hashes are built with the fixed function. The Patrician III `archiver.dll` indexes the table with
//! zero-extended bytes and needs no fix.
use crate::ffi::*;
use crate::game::Base;
use crate::mem::{bytes, rd32, write};

/// Sign-extending index load: RVA and original bytes (`0f be` + operands).
struct Site {
    rva: u32,
    code: &'static [u8],
}

/// `movsx ecx,byte [ebx+eax]` / `movsx edx,byte [eax]` (compare) and `movsx ecx,cl` (hash).
const SITES: [Site; 3] = [
    Site { rva: 0x4c29, code: &[0x0f, 0xbe, 0x0c, 0x03] },
    Site { rva: 0x4c2d, code: &[0x0f, 0xbe, 0x10] },
    Site { rva: 0x4ca1, code: &[0x0f, 0xbe, 0xc9] },
];
/// `mov cl,[ecx+table]` after the compare loads: `8a 89` + table address.
const TABLE_REF: u32 = 0x4c30;
/// `mov eax,[flag]` at the start of the lazy table init: `a1` + flag address (table + 100h).
const FLAG_REF: u32 = 0x4bb0;
const TABLE_RVA: u32 = 0x1f0f0;

/// Case and code page folding: (bytes, value).
const FOLD: [(&[u8], u8); 4] = [
    (&[0xc4, 0xe4, 0x8e, 0x84], 0xe4), // Ä ä (CP1252), Ä ä (OEM)
    (&[0xd6, 0xf6, 0x99, 0x94], 0xf6), // Ö ö
    (&[0xdc, 0xfc, 0x9a, 0x81], 0xfc), // Ü ü
    (&[0xdf, 0xe1], 0xdf),             // ß
];

fn table() -> [u8; 256] {
    let mut t: [u8; 256] = core::array::from_fn(|i| (i as u8).to_ascii_lowercase());
    t[b'/' as usize] = b'\\';
    for (from, to) in FOLD {
        for &c in from {
            t[c as usize] = to;
        }
    }
    t
}

/// Fix the name lookup of the loaded `archiver.dll`. Safe to call from every mod (each one has its own copy of this
/// code): the state is read from the module bytes. Ok = summary for the log.
pub fn fix_names(base: Base) -> Result<&'static str, String> {
    if base == Base::P3 {
        return Ok("not needed (Patrician III archiver.dll)");
    }
    let module = unsafe { GetModuleHandleW(crate::wide("archiver.dll".as_ref()).as_ptr()) } as u32;
    if module == 0 {
        return Err("archiver.dll not loaded".into());
    }
    let table_va = module + TABLE_RVA;
    unsafe {
        if bytes(module + TABLE_REF, 2) != [0x8a, 0x89] || rd32(module + TABLE_REF + 2) != table_va {
            return Err(format!("unknown archiver.dll (no table reference at +{TABLE_REF:#x})"));
        }
        if bytes(module + FLAG_REF, 1) != [0xa1] || rd32(module + FLAG_REF + 1) != table_va + 0x100 {
            return Err(format!("unknown archiver.dll (no table init at +{FLAG_REF:#x})"));
        }
        let state = |movsx: bool| {
            SITES.iter().all(|s| {
                let b = bytes(module + s.rva, s.code.len());
                b[0] == 0x0f && b[1] == if movsx { 0xbe } else { 0xb6 } && b[2..] == s.code[2..]
            })
        };
        let patched = state(false);
        if !patched && !state(true) {
            return Err("unknown archiver.dll (name lookup bytes differ)".into());
        }
        write(table_va, &table())?;
        write(table_va + 0x100, &1u32.to_le_bytes())?;
        if patched {
            return Ok("already fixed");
        }
        for s in &SITES {
            write(module + s.rva + 1, &[0xb6])?;
        }
    }
    Ok("on (unsigned name bytes, CP1252 / OEM umlauts folded)")
}
