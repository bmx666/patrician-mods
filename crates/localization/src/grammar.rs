//! Grammar of the translations in `Patrician 2.exe` and `Patrician3.exe` (port of patrician2 `tools/exepatch.js`
//! 2b / 2i / 2o / 2p / 2q and patrician3 2b / 2o / 2p / 2q / 2r), installed for every language but `en`:
//! - [`drop_caps`]: `\d` markup draws the first letter as an initial picture (`images\Grossbuchstaben\Gbuchstabe_<A-Z>`);
//!   a letter of the language codepage is mapped to a Latin slot first: Cyrillic to the redrawn / look-alike slots
//!   of the Russian folder, accented capitals to their base letter. Unmapped bytes >= 80h = no initial.
//! - [`plurals`]: count-dependent forms. A string `<11 digits>|form0|form1|...` selects a form by the count: digit d
//!   (0..9) = form index for counts ending in d, the 11th digit = form index for 11..19 (missing index = last form);
//!   any other string is used as is. Auction announcement ship plurals (snaikkas / crayers / cogs / holks; P3 also
//!   the 57 building plurals) and the guild member info ("%i towns", "%i Ships.").
//! - [`adjective_gender`]: the merchant adjective table (P2 64E6F0h, P3 6A0C18h: 15 female then 15 male pointers)
//!   indexed by gender at the readers that always took the female form ([merchant+1Ah] bit 0 = male; P3 statue
//!   text: edx 0 = Lord Mayor, 1 = Lady Mayoress).
//! - [`name_capital`]: merchant names "%s %s %s %s %s" (article / adjective / rank / first / last name) have an empty
//!   article and lowercase adjective / rank in the translations. After `CString::Format` and the positional sprintf,
//!   a format holding "%s %s %s %s" whose first `%` starts that run after a sentence end (start, `_` of `\fN_`, `\c`,
//!   `.` `!` `?`, spaces / newlines skipped) gets the first letter of the run uppercased; spaces are then collapsed.
//!   P3 loan offers ("The" + " " + rank + name, no format): the article becomes "–".
use pcore::game::Base;
use pcore::hook::{Ctx, Ins, Plan};
use pcore::mem::{self, hex, le, rd32, rd8};
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// Merchant adjective table: 0..14 female, 15..29 male.
const ADJ_P2: u32 = 0x64_e6f0;
const ADJ_P3: u32 = 0x6a_0c18;
/// `CString::Format` (cdecl this, fmt, ...), `CString::FormatV` (thiscall this, fmt, va_list) it calls, positional
/// sprintf (cdecl buf, fmt, ...).
struct Formatters {
    format: u32,
    formatv: u32,
    sprintf: u32,
}
const FMT_P2: Formatters = Formatters { format: 0x5f_85f0, formatv: 0x5f_7f80, sprintf: 0x5f_7a80 };
const FMT_P3: Formatters = Formatters { format: 0x64_df80, formatv: 0x64_d910, sprintf: 0x64_d410 };
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
static FORMATV: AtomicU32 = AtomicU32::new(0);
static ADJ: AtomicU32 = AtomicU32::new(0);
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
pub fn drop_caps(p: &mut Plan, base: Base, cyrillic: bool, rev: &HashMap<u16, u8>) -> Result<(), String> {
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
    let site = match base {
        Base::P2 => 0x45a4dd,
        Base::P3 => 0x4626cd,
    };
    p.mid(site, 5, &hex("8a45023c41"), &[Ins::Bytes(&[0x3c, 0x41])], &[], drop_cap, 0)
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

/// Count of a plural site.
#[derive(Clone, Copy)]
enum N {
    Eax,
    Ebp,
    Esi,
    Edi,
    /// `[esp+k]`
    Stack(u32),
    /// byte `[esp+esi+k]`
    StackEsiByte(u32),
}

/// Register the selected string goes to.
#[derive(Clone, Copy)]
enum Out {
    Eax,
    Ecx,
    Edx,
}

/// Plural site: `mov out,[slot]` (or `mov out,[esi*4+slot]`) over `len` bytes; `extra` = the next instruction
/// `mov ecx,[esp+1Ch]` (the count) is part of the site.
struct Site {
    va: u32,
    len: usize,
    ins: &'static str,
    slot: u32,
    indexed: bool,
    n: N,
    out: Out,
    extra: bool,
}

const fn site(va: u32, len: usize, ins: &'static str, slot: u32, n: N, out: Out) -> Site {
    Site { va, len, ins, slot, indexed: false, n, out, extra: false }
}

static SITES_P2: [Site; 6] = [
    // auction announcement: holks, cogs, crayers, snaikkas
    site(0x588ff9, 6, "8b0d2ca16400", 0x64a12c, N::Eax, Out::Ecx),
    Site { extra: true, ..site(0x5890e6, 9, "a128a164008b4c241c", 0x64a128, N::Stack(0x1c), Out::Eax) },
    site(0x58919b, 6, "8b1524a16400", 0x64a124, N::Ebp, Out::Edx),
    site(0x58922d, 6, "8b1520a16400", 0x64a120, N::Eax, Out::Edx),
    // guild member info: "%i towns", "%i Ships."
    site(0x58c1c4, 5, "a1dca16400", 0x64a1dc, N::Eax, Out::Eax),
    site(0x58c234, 5, "a1eca16400", 0x64a1ec, N::Eax, Out::Eax),
];

static SITES_P3: [Site; 7] = [
    // auction announcement: buildings (plural table 6AFD80h, count byte [esp+esi+0C0h]), holks, cogs, crayers,
    // snaikkas
    Site { indexed: true, ..site(0x5c20f3, 7, "8b04b580fd6a00", 0x6afd80, N::StackEsiByte(0xc0), Out::Eax) },
    site(0x5c22ea, 5, "a1ecc76900", 0x69c7ec, N::Stack(0x40), Out::Eax),
    site(0x5c23bd, 6, "8b15e8c76900", 0x69c7e8, N::Stack(0x18), Out::Edx),
    site(0x5c246f, 5, "a1e4c76900", 0x69c7e4, N::Esi, Out::Eax),
    site(0x5c24fc, 5, "a1e0c76900", 0x69c7e0, N::Edi, Out::Eax),
    // guild member info: "%i towns", "%i Ships."
    site(0x5c5d96, 5, "a19cc86900", 0x69c89c, N::Eax, Out::Eax),
    site(0x5c5e06, 5, "a1acc86900", 0x69c8ac, N::Eax, Out::Eax),
];

pub fn plurals(p: &mut Plan, base: Base) -> Result<(), String> {
    let sites: &'static [Site] = match base {
        Base::P2 => &SITES_P2,
        Base::P3 => &SITES_P3,
    };
    for s in sites {
        p.mid(s.va, s.len, &hex(s.ins), &[], &[], plural_site, s as *const Site as u32)?;
    }
    Ok(())
}

unsafe extern "C" fn plural_site(c: *mut Ctx, site: u32) {
    let c = &mut *c;
    let s = &*(site as *const Site);
    let n = match s.n {
        N::Eax => c.eax,
        N::Ebp => c.ebp,
        N::Esi => c.esi,
        N::Edi => c.edi,
        N::Stack(k) => *c.stack(k),
        N::StackEsiByte(k) => rd8(c.sp().wrapping_add(c.esi).wrapping_add(k)) as u32,
    };
    let at = if s.indexed { s.slot.wrapping_add(c.esi.wrapping_mul(4)) } else { s.slot };
    let v = plural(rd32(at), n);
    if s.extra {
        c.ecx = n;
    }
    match s.out {
        Out::Eax => c.eax = v,
        Out::Ecx => c.ecx = v,
        Out::Edx => c.edx = v,
    }
}

// ---- adjective gender ----

#[derive(Clone, Copy)]
enum R {
    Eax,
    Ecx,
    Edx,
    Ebx,
    Edi,
}

fn get(c: &Ctx, r: R) -> u32 {
    match r {
        R::Eax => c.eax,
        R::Ecx => c.ecx,
        R::Edx => c.edx,
        R::Ebx => c.ebx,
        R::Edi => c.edi,
    }
}

fn set(c: &mut Ctx, r: R, v: u32) {
    match r {
        R::Eax => c.eax = v,
        R::Ecx => c.ecx = v,
        R::Edx => c.edx = v,
        R::Ebx => c.ebx = v,
        R::Edi => c.edi = v,
    }
}

#[derive(Clone, Copy)]
enum Gender {
    /// Merchant in a register: [merchant+1Ah] bit 0 = male.
    Merchant(R),
    /// P3 statue text: edx 0 = Lord Mayor (male), 1 = Lady Mayoress.
    Mayor,
}

/// 7-byte reader `mov out,[k*4+ADJ]` (`ins` = its first 3 bytes); the next instruction `next` is only checked.
struct Reader {
    va: u32,
    ins: &'static str,
    next: &'static str,
    k: R,
    out: R,
    gender: Gender,
}

static READERS_P2: [Reader; 3] = [
    Reader {
        va: 0x58c008,
        ins: "8b148d",
        next: "8b0daca16400",
        k: R::Ecx,
        out: R::Edx,
        gender: Gender::Merchant(R::Edi),
    },
    Reader {
        va: 0x5913f8,
        ins: "8b148d",
        next: "8b0d60b46400",
        k: R::Ecx,
        out: R::Edx,
        gender: Gender::Merchant(R::Ebx),
    },
    Reader {
        va: 0x59626a,
        ins: "8b148d",
        next: "8b0d60b46400",
        k: R::Ecx,
        out: R::Edx,
        gender: Gender::Merchant(R::Edi),
    },
];

/// P3: "... opened %s first trading office", "%s %s %s %s %s from %s." (2x), statue text (after
/// `xor eax,eax; and edx,0FFFFh; mov ax,[edi+8]`, checked).
static READERS_P3: [Reader; 4] = [
    Reader {
        va: 0x5c5bb8,
        ins: "8b0c95",
        next: "8b1485f80e6c00",
        k: R::Edx,
        out: R::Ecx,
        gender: Gender::Merchant(R::Edi),
    },
    Reader {
        va: 0x5ccd47,
        ins: "8b148d",
        next: "8b0d60d96900",
        k: R::Ecx,
        out: R::Edx,
        gender: Gender::Merchant(R::Ebx),
    },
    Reader {
        va: 0x5d208a,
        ins: "8b148d",
        next: "8b0d60d96900",
        k: R::Ecx,
        out: R::Edx,
        gender: Gender::Merchant(R::Edi),
    },
    Reader {
        va: 0x485fef,
        ins: "8b0c85",
        next: "8b04956c1d6b00",
        k: R::Eax,
        out: R::Ecx,
        gender: Gender::Mayor,
    },
];

pub fn adjective_gender(p: &mut Plan, base: Base) -> Result<(), String> {
    let (adj, first, readers): (u32, u32, &'static [Reader]) = match base {
        Base::P2 => (ADJ_P2, 0x421226, &READERS_P2),
        Base::P3 => {
            p.check(0x485fe3, &hex("33c081e2ffff0000668b4708"))?;
            (ADJ_P3, 0x421d06, &READERS_P3)
        }
    };
    ADJ.store(adj, Ordering::Relaxed);
    let tab = le(adj);
    // mov eax,[eax*4+TAB]; mov edx,[esi+ecx+20h]
    p.mid(first, 11, &mem::cat(&[&hex("8b0485"), &tab, &hex("8b540e20")]), &[], &[], adjective_a, 0)?;
    for r in readers {
        let expect = mem::cat(&[&hex(r.ins), &tab, &hex(r.next)]);
        p.mid(r.va, 7, &expect, &[], &[], adjective_b, r as *const Reader as u32)?;
    }
    Ok(())
}

unsafe fn adjective(k: u32, male: bool) -> u32 {
    rd32(ADJ.load(Ordering::Relaxed) + 4 * (k + if male { 15 } else { 0 }))
}

unsafe extern "C" fn adjective_a(c: *mut Ctx, _: u32) {
    let c = &mut *c;
    c.edx = rd32(c.esi.wrapping_add(c.ecx).wrapping_add(0x20));
    c.eax = adjective(c.eax, rd8(c.edx + 0x1a) & 1 != 0);
}

unsafe extern "C" fn adjective_b(c: *mut Ctx, reader: u32) {
    let c = &mut *c;
    let r = &*(reader as *const Reader);
    let male = match r.gender {
        Gender::Merchant(m) => rd8(get(c, m) + 0x1a) & 1 != 0,
        Gender::Mayor => c.edx == 0,
    };
    let v = adjective(get(c, r.k), male);
    set(c, r.out, v);
}

// ---- merchant name at a sentence start ----

pub fn name_capital(p: &mut Plan, base: Base) -> Result<(), String> {
    let f = match base {
        Base::P2 => &FMT_P2,
        Base::P3 => &FMT_P3,
    };
    // CString::Format: mov ecx,[esp+8]; lea eax,[esp+0Ch]; push eax; push ecx; mov ecx,[esp+0Ch]; call FormatV; ret
    let call = mem::rel5(0xe8, f.format + 0xe, f.formatv);
    p.check(f.format, &mem::cat(&[&hex("8b4c24088d44240c50518b4c240c"), &call, &[0xc3]]))?;
    p.check(f.sprintf, &SPRINTF_PROLOGUE)?;
    if base == Base::P3 {
        // loan offers: mov ecx,[69C038h] / mov eax,[69C03Ch] (The) -> "–"
        p.mid(0x5b42de, 6, &hex("8b0d38c06900"), &[], &[], loan_article, 0)?;
        p.mid(0x5b4313, 5, &hex("a13cc06900"), &[], &[], loan_article, 1)?;
    }
    let tramp = mem::alloc_code(SPRINTF_PROLOGUE.len() + 5)?;
    let code = mem::cat(&[&SPRINTF_PROLOGUE, &mem::rel5(0xe9, tramp + 7, f.sprintf + 7)]);
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), tramp as *mut u8, code.len()) };
    SPRINTF_ORIG.store(tramp, Ordering::Relaxed);
    FORMATV.store(f.formatv, Ordering::Relaxed);
    RET0.store(Plan::ret_stub(0)?, Ordering::Relaxed);
    p.mid(f.format, 8, &hex("8b4c24088d44240c"), &[], &[], format_hook, 0)?;
    p.mid(f.sprintf, 7, &SPRINTF_PROLOGUE, &[], &[], sprintf_hook, 0)
}

/// "–" (0x96 in cp1251 / 1252 / 1257).
static DASH: [u8; 2] = [0x96, 0];

/// Loan offer article: `data` 0 = ecx, 1 = eax.
unsafe extern "C" fn loan_article(c: *mut Ctx, data: u32) {
    let c = &mut *c;
    let v = DASH.as_ptr() as u32;
    if data == 0 {
        c.ecx = v;
    } else {
        c.eax = v;
    }
}

type FormatV = unsafe extern "thiscall" fn(u32, u32, u32) -> u32;

/// `CString::Format(this, fmt, ...)` entry: format, capitalise, return to the caller.
unsafe extern "C" fn format_hook(c: *mut Ctx, _: u32) {
    let c = &mut *c;
    let (this, fmt) = (*c.stack(4), *c.stack(8));
    let f: FormatV = core::mem::transmute(FORMATV.load(Ordering::Relaxed) as usize);
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
