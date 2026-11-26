//! Grammar of the translations in `Patrician 2.exe` (port of patrician2 `tools/exepatch.js` 2b / 2i / 2o / 2p / 2q),
//! installed for every language but `en`:
//! - [`drop_caps`]: `\d` markup draws the first letter as an initial picture (`images\Grossbuchstaben\Gbuchstabe_<A-Z>`);
//!   a letter of the language codepage is mapped to a Latin slot first: Cyrillic to the redrawn / look-alike slots
//!   of the Russian folder, accented capitals to their base letter. Unmapped bytes >= 80h = no initial.
//! - [`plurals`]: count-dependent forms. A string `<11 digits>|form0|form1|...` selects a form by the count: digit d
//!   (0..9) = form index for counts ending in d, the 11th digit = form index for 11..19 (missing index = last form);
//!   any other string is used as is. Auction announcement ship plurals (snaikkas / crayers / cogs / holks) and the
//!   guild member info ("%i towns", "%i Ships.").
//! - [`adjective_gender`]: the merchant adjective table (64E6F0h: 15 female then 15 male pointers) indexed by gender at
//!   the four readers that always took the female form ([merchant+1Ah] bit 0 = male).
//! - [`name_capital`]: merchant names "%s %s %s %s %s" (article / adjective / rank / first / last name) have an empty
//!   article and lowercase adjective / rank in the translations. After `CString::Format` and the positional sprintf,
//!   a format holding "%s %s %s %s" whose first `%` starts that run after a sentence end (start, `_` of `\fN_`, `\c`,
//!   `.` `!` `?`, spaces / newlines skipped) gets the first letter of the run uppercased; spaces are then collapsed.
use pcore::hook::{Ctx, Ins, Plan};
use pcore::mem::{self, hex, le, rd32, rd8};
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// Merchant adjective table: 0..14 female, 15..29 male.
const ADJ: u32 = 0x64_e6f0;
/// `CString::FormatV` (thiscall this, fmt, va_list), called by `CString::Format` (cdecl this, fmt, ...).
const FORMATV: u32 = 0x5f_7f80;
const FORMAT: u32 = 0x5f_85f0;
/// Positional sprintf (cdecl buf, fmt, ...).
const SPRINTF: u32 = 0x5f_7a80;
const SPRINTF_PROLOGUE: [u8; 7] = [0x53, 0x55, 0x56, 0x8b, 0x74, 0x24, 0x14];
/// Arguments of the sprintf copy (buf, fmt + 23).
const SPRINTF_ARGS: usize = 25;

/// Single-threaded game state written by the hooks.
struct Cell<T>(UnsafeCell<T>);
unsafe impl<T> Sync for Cell<T> {}

/// Selected plural form (the caller formats it before the next selection).
static FORM: Cell<[u8; 64]> = Cell(UnsafeCell::new([0; 64]));
/// Drop cap byte -> Latin letter (0 = none).
static DROP: Cell<[u8; 256]> = Cell(UnsafeCell::new([0; 256]));
static SPRINTF_ORIG: AtomicU32 = AtomicU32::new(0);
static RET0: AtomicU32 = AtomicU32::new(0);

// ---- drop caps ----

/// Cyrillic capital (cp1251) -> Latin slot of the Russian initials (patrician2 `tools/initials.js`).
const CYRILLIC: &[(char, u8)] = &[
    ('А', b'A'), ('В', b'B'), ('Е', b'E'), ('К', b'K'), ('М', b'M'), ('Н', b'H'), ('О', b'O'), ('Р', b'P'),
    ('С', b'C'), ('Т', b'T'), ('Х', b'X'), ('Ё', b'E'),
    ('Д', b'D'), ('Ф', b'F'), ('Г', b'G'), ('И', b'I'), ('Я', b'J'), ('Л', b'L'), ('Б', b'N'), ('Ж', b'Q'),
    ('З', b'R'), ('Ч', b'S'), ('У', b'U'), ('Ш', b'V'), ('Э', b'W'), ('Ю', b'Y'), ('П', b'Z'),
];
/// Accented capitals -> base letter (Latin and Baltic codepages; letters missing in a codepage are ignored).
const ACCENTED: &[(char, u8)] = &[
    ('Ä', b'A'), ('Å', b'A'), ('À', b'A'), ('Á', b'A'), ('Â', b'A'), ('Æ', b'A'), ('Ą', b'A'), ('Ā', b'A'),
    ('Ö', b'O'), ('Ó', b'O'), ('Ò', b'O'), ('Ô', b'O'), ('Õ', b'O'), ('Ø', b'O'),
    ('É', b'E'), ('È', b'E'), ('Ê', b'E'), ('Ę', b'E'), ('Ė', b'E'), ('Ē', b'E'),
    ('Ü', b'U'), ('Ú', b'U'), ('Ù', b'U'), ('Û', b'U'), ('Ų', b'U'), ('Ū', b'U'),
    ('Š', b'S'), ('Ž', b'Z'), ('Č', b'C'), ('Į', b'I'), ('Ī', b'I'), ('Ģ', b'G'), ('Ķ', b'K'), ('Ļ', b'L'), ('Ņ', b'N'),
];

/// `rev`: UTF-16 unit -> byte of the language codepage.
pub fn drop_caps(p: &mut Plan, cyrillic: bool, rev: &HashMap<u16, u8>) -> Result<(), String> {
    let t = unsafe { &mut *DROP.0.get() };
    for (i, v) in t.iter_mut().enumerate() {
        *v = if i < 0x80 { i as u8 } else { 0 };
    }
    for &(ch, l) in if cyrillic { CYRILLIC } else { ACCENTED } {
        if let Some(&b) = rev.get(&(ch as u32 as u16)) {
            t[b as usize] = l;
        }
    }
    // mov al,[ebp+2]; cmp al,41h -> al = table[[ebp+2]]; cmp al,41h
    p.mid(0x45a4dd, 5, &hex("8a45023c41"), &[Ins::Bytes(&[0x3c, 0x41])], &[], drop_cap, 0)
}

unsafe extern "C" fn drop_cap(c: *mut Ctx, _: u32) {
    let c = &mut *c;
    let b = rd8(c.ebp + 2);
    c.eax = (c.eax & !0xff) | (*DROP.0.get())[b as usize] as u32;
}

// ---- plurals ----

/// The form of `s` for count `n`, or `s` itself.
unsafe fn plural(s: u32, n: u32) -> u32 {
    if s == 0 {
        return s;
    }
    let mut h = [0u8; 12];
    for (k, v) in h.iter_mut().enumerate() {
        *v = rd8(s + k as u32);
        if (k < 11 && !v.is_ascii_digit()) || (k == 11 && *v != b'|') {
            return s; // stops at the first other byte (NUL included)
        }
    }
    let r = n % 100;
    let mut idx = if (11..=19).contains(&r) { h[10] - b'0' } else { h[(r % 10) as usize] - b'0' };
    let mut at = s + 12;
    let mut start = at;
    // skip idx forms; running out = the last form
    while idx > 0 {
        match rd8(at) {
            0 => {
                at = start;
                break;
            }
            b'|' => {
                at += 1;
                start = at;
                idx -= 1;
            }
            _ => at += 1,
        }
    }
    let out = &mut *FORM.0.get();
    let mut k = 0;
    while k < out.len() - 1 {
        let ch = rd8(at + k as u32);
        if ch == 0 || ch == b'|' {
            break;
        }
        out[k] = ch;
        k += 1;
    }
    out[k] = 0;
    out.as_ptr() as u32
}

/// Plural sites: (site, length, expected bytes, slot of the string, how the hook reads / writes registers).
#[derive(Clone, Copy)]
enum Site {
    /// `mov ecx,[slot]`, n = eax
    EcxEax,
    /// `mov eax,[slot]; mov ecx,[esp+1Ch]`, n = [esp+1Ch]
    EaxStack,
    /// `mov edx,[slot]`, n = ebp
    EdxEbp,
    /// `mov edx,[slot]`, n = eax
    EdxEax,
    /// `mov eax,[slot]`, n = eax (pushed before)
    EaxEax,
}

const SITES: &[(u32, usize, &str, u32, Site)] = &[
    // auction announcement: holks, cogs, crayers, snaikkas
    (0x588ff9, 6, "8b0d2ca16400", 0x64a12c, Site::EcxEax),
    (0x5890e6, 9, "a128a164008b4c241c", 0x64a128, Site::EaxStack),
    (0x58919b, 6, "8b1524a16400", 0x64a124, Site::EdxEbp),
    (0x58922d, 6, "8b1520a16400", 0x64a120, Site::EdxEax),
    // guild member info: "%i towns", "%i Ships."
    (0x58c1c4, 5, "a1dca16400", 0x64a1dc, Site::EaxEax),
    (0x58c234, 5, "a1eca16400", 0x64a1ec, Site::EaxEax),
];

pub fn plurals(p: &mut Plan) -> Result<(), String> {
    for (i, &(va, len, ins, _, _)) in SITES.iter().enumerate() {
        p.mid(va, len, &hex(ins), &[], &[], plural_site, i as u32)?;
    }
    Ok(())
}

unsafe extern "C" fn plural_site(c: *mut Ctx, i: u32) {
    let c = &mut *c;
    let (_, _, _, slot, kind) = SITES[i as usize];
    let s = rd32(slot);
    match kind {
        Site::EcxEax => c.ecx = plural(s, c.eax),
        Site::EaxStack => {
            c.ecx = *c.stack(0x1c);
            c.eax = plural(s, c.ecx);
        }
        Site::EdxEbp => c.edx = plural(s, c.ebp),
        Site::EdxEax => c.edx = plural(s, c.eax),
        Site::EaxEax => c.eax = plural(s, c.eax),
    }
}

// ---- adjective gender ----

/// Merchant register of a 7-byte reader `mov edx,[ecx*4+64E6F0h]`.
#[derive(Clone, Copy)]
enum Merchant {
    Edi,
    Ebx,
}

const READERS: &[(u32, &str, Merchant)] = &[
    (0x58c008, "8b0daca16400", Merchant::Edi),
    (0x5913f8, "8b0d60b46400", Merchant::Ebx),
    (0x59626a, "8b0d60b46400", Merchant::Edi),
];

pub fn adjective_gender(p: &mut Plan) -> Result<(), String> {
    let tab = le(ADJ);
    // mov eax,[eax*4+TAB]; mov edx,[esi+ecx+20h]
    p.mid(0x421226, 11, &mem::cat(&[&hex("8b0485"), &tab, &hex("8b540e20")]), &[], &[], adjective_a, 0)?;
    for (i, &(va, next, _)) in READERS.iter().enumerate() {
        // mov edx,[ecx*4+TAB] (the next instruction only checked)
        p.mid(va, 7, &mem::cat(&[&hex("8b148d"), &tab, &hex(next)]), &[], &[], adjective_b, i as u32)?;
    }
    Ok(())
}

unsafe fn adjective(k: u32, merchant: u32) -> u32 {
    let male = rd8(merchant + 0x1a) & 1 != 0;
    rd32(ADJ + 4 * (k + if male { 15 } else { 0 }))
}

unsafe extern "C" fn adjective_a(c: *mut Ctx, _: u32) {
    let c = &mut *c;
    c.edx = rd32(c.esi.wrapping_add(c.ecx).wrapping_add(0x20));
    c.eax = adjective(c.eax, c.edx);
}

unsafe extern "C" fn adjective_b(c: *mut Ctx, i: u32) {
    let c = &mut *c;
    let m = match READERS[i as usize].2 {
        Merchant::Edi => c.edi,
        Merchant::Ebx => c.ebx,
    };
    c.edx = adjective(c.ecx, m);
}

// ---- merchant name at a sentence start ----

pub fn name_capital(p: &mut Plan) -> Result<(), String> {
    // CString::Format: mov ecx,[esp+8]; lea eax,[esp+0Ch]; push eax; push ecx; mov ecx,[esp+0Ch]; call FormatV; ret
    let call = mem::rel5(0xe8, FORMAT + 0xe, FORMATV);
    p.check(FORMAT, &mem::cat(&[&hex("8b4c24088d44240c50518b4c240c"), &call, &[0xc3]]))?;
    p.check(SPRINTF, &SPRINTF_PROLOGUE)?;
    let tramp = mem::alloc_code(SPRINTF_PROLOGUE.len() + 5)?;
    let code = mem::cat(&[&SPRINTF_PROLOGUE, &mem::rel5(0xe9, tramp + 7, SPRINTF + 7)]);
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), tramp as *mut u8, code.len()) };
    SPRINTF_ORIG.store(tramp, Ordering::Relaxed);
    RET0.store(Plan::ret_stub(0)?, Ordering::Relaxed);
    p.mid(FORMAT, 8, &hex("8b4c24088d44240c"), &[], &[], format_hook, 0)?;
    p.mid(SPRINTF, 7, &SPRINTF_PROLOGUE, &[], &[], sprintf_hook, 0)
}

type FormatV = unsafe extern "thiscall" fn(u32, u32, u32) -> u32;

/// `CString::Format(this, fmt, ...)` entry: format, capitalise, return to the caller.
unsafe extern "C" fn format_hook(c: *mut Ctx, _: u32) {
    let c = &mut *c;
    let (this, fmt) = (*c.stack(4), *c.stack(8));
    let f: FormatV = core::mem::transmute(FORMATV as usize);
    c.eax = f(this, fmt, c.sp() + 12);
    capitalise(rd32(this), fmt, true);
    c.resume = RET0.load(Ordering::Relaxed);
}

type Sprintf = unsafe extern "C" fn(
    u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32,
    u32, u32,
) -> u32;

/// Positional sprintf(buf, fmt, ...) entry: the original with 25 argument dwords copied, then capitalise.
unsafe extern "C" fn sprintf_hook(c: *mut Ctx, _: u32) {
    let c = &mut *c;
    let a: [u32; SPRINTF_ARGS] = core::array::from_fn(|i| *c.stack(4 + 4 * i as u32));
    let f: Sprintf = core::mem::transmute(SPRINTF_ORIG.load(Ordering::Relaxed) as usize);
    c.eax = f(
        a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7], a[8], a[9], a[10], a[11], a[12], a[13], a[14], a[15], a[16],
        a[17], a[18], a[19], a[20], a[21], a[22], a[23], a[24],
    );
    capitalise(a[0], a[1], false);
    c.resume = RET0.load(Ordering::Relaxed);
}

unsafe fn cstr<'a>(at: u32) -> &'a [u8] {
    core::ffi::CStr::from_ptr(at as *const core::ffi::c_char).to_bytes()
}

/// Uppercase the merchant name at a sentence start in `buf` (formatted from `fmt`), collapse spaces; a CString gets
/// its length ([buf-8]) updated.
unsafe fn capitalise(buf: u32, fmt: u32, cstring: bool) {
    if buf == 0 || fmt == 0 {
        return;
    }
    let f = cstr(fmt);
    let Some(first) = f.iter().position(|&ch| ch == b'%') else { return };
    let Some(run) = f.windows(11).position(|w| w == b"%s %s %s %s") else { return };
    if run == first && sentence_start(&f[..first]) {
        let mut q = buf + first as u32;
        loop {
            match rd8(q) {
                b' ' => q += 1,
                b'\\' if rd8(q + 1) == b'f' => q += 4,
                b'\\' if rd8(q + 1) == b'c' => q += 2,
                b'\\' => break,
                ch @ (b'a'..=b'z' | 0xe0..=0xff) => {
                    mem::wr8(q, ch - 0x20);
                    break;
                }
                0xb8 => {
                    mem::wr8(q, 0xa8);
                    break;
                }
                _ => break,
            }
        }
    }
    // collapse: a space after a space, newline, "_", "(" or at the start is dropped
    let (mut src, mut dst, mut prev) = (buf, buf, b'\n');
    loop {
        let ch = rd8(src);
        src += 1;
        if ch == b' ' && matches!(prev, b' ' | b'\n' | b'_' | b'(') {
            continue;
        }
        mem::wr8(dst, ch);
        dst += 1;
        prev = ch;
        if ch == 0 {
            break;
        }
    }
    if cstring {
        mem::wr32(buf - 8, dst - 1 - buf);
    }
}

/// The literal prefix before the name ends a sentence.
fn sentence_start(prefix: &[u8]) -> bool {
    let mut i = prefix.len();
    loop {
        if i == 0 {
            return true;
        }
        match prefix[i - 1] {
            b' ' | b'\n' => i -= 1,
            b'_' | b'.' | b'!' | b'?' => return true,
            b'c' => return i >= 2 && prefix[i - 2] == b'\\',
            _ => return false,
        }
    }
}
