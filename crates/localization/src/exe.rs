//! Texts inside `Patrician 2.exe` (GOG) and `Patrician3.exe` (GOG 1.1) (port of patrician2 / patrician3
//! `tools/exepatch.js` steps 1 / 2 / 2n, applied in memory at start): `mods\localization\<lang>\<p2|p3>\exe\`
//! - `charset.bin`: font charset (byte of the language codepage per glyph cell, no NUL), matching the `.aim` fonts of
//!   the language folder. The game keeps the charset pointer next to every static copy of the original charset
//!   string (one per module: P2 128 bytes, P3 118 bytes and 97 copies, then an aligned pointer to it); all of them
//!   are redirected.
//! - `*.tsv`: `<file offset hex>\t<english>\t<translation>` (`\n \t \r \\` escaped, `""` = empty, `#` comment). The
//!   offset is the RVA too (file alignment = section alignment). The English text must match the image. A translation
//!   that fits (the original length + zero padding nothing points into, up to 3 bytes) is written in place; a longer
//!   one is copied to the heap and every reference is redirected: aligned dwords of `.rdata` / `.data`, and in `.text`
//!   the imm32 / disp32 of `push`, `mov`, `lea` (other code refs are left alone). P3 code that uses a string start as
//!   the base of a pointer table keeps the original address: `[reg+disp32]` / `[index*s+disp32]` dword reads and
//!   `lea` (e.g. 73 x `[reg*4+6C0D00h]`, month table based on "Dec") and `mov ecx,69A7A8h` at 49B508 ("low");
//!   byte reads `mov r8,[reg+str]` are character access of the string and follow it.
//!   Count-dependent forms (`<11 digits>|form0|form1|...`) are kept for [`crate::grammar`] when its plural hooks are
//!   on (`%` specifiers checked per form), otherwise the last form is used.
//! - P3 rows `r<id>\t<english>\t<translation>`: RT_STRING resources (tool tips, `ToolTip=<id>` in `scripts\*.ini`),
//!   checked against the resource of the executable and served by [`tooltips`].
//! UTF-8 text is encoded with the language codepage (1251 `ru`, 1257 `lt` `lv`, else 1252); a line with a character
//! outside it or with other `%` specifiers than the English text is skipped (counted in the log).
use pcore::ffi::*;
use pcore::game::Base;
use pcore::hook::{Ctx, Ins, Plan};
use pcore::mem;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

const IB: u32 = 0x40_0000;

/// Executable layout of the text patch (exepatch.js 1 / 2).
struct Layout {
    /// Original charset string and the pointer to it after the string.
    charset: u32,
    charset_ptr: u32,
    charset_len: usize,
    /// Number of module copies of the charset string, when known.
    copies: Option<usize>,
    /// File offset of the guild's "one %s." that may drop its only `%s`.
    one_ship: u32,
    /// Table-base code refs are kept on the original address (P3).
    table_bases: bool,
    /// imm32 of `mov r32,imm32` table bases (address of the imm32).
    kept_imm: &'static [u32],
}

const P2: Layout = Layout {
    charset: 0x65_f9f0,
    charset_ptr: 0x65_fa70,
    charset_len: 128,
    copies: None,
    one_ship: 0x24_9f7c,
    table_bases: false,
    kept_imm: &[],
};

const P3: Layout = Layout {
    charset: 0x6b_f2c8,
    charset_ptr: 0x6b_f340,
    charset_len: 118,
    copies: Some(97),
    one_ship: 0x29_c63c,
    table_bases: true,
    kept_imm: &[0x49_b509],
};

fn layout(base: Base) -> &'static Layout {
    match base {
        Base::P2 => &P2,
        Base::P3 => &P3,
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn MultiByteToWideChar(cp: u32, flags: u32, src: *const u8, len: i32, dst: *mut u16, cap: i32) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn LoadStringW(module: HMODULE, id: u32, buf: *mut u16, cap: i32) -> i32;
}

#[derive(Clone, Copy)]
struct Sec {
    va: u32,
    size: u32,
}

struct Image {
    text: Sec,
    rdata: Sec,
    data: Sec,
}

fn image() -> Result<Image, String> {
    unsafe {
        let pe = IB + mem::rd32(IB + 0x3c);
        let n = mem::rd16(pe + 6) as u32;
        let table = pe + 24 + mem::rd16(pe + 20) as u32;
        let find = |name: &[u8]| -> Result<Sec, String> {
            for i in 0..n {
                let s = table + 40 * i;
                let nm = mem::bytes(s, 8);
                let len = nm.iter().position(|&c| c == 0).unwrap_or(8);
                if &nm[..len] == name {
                    return Ok(Sec { va: IB + mem::rd32(s + 12), size: mem::rd32(s + 8) });
                }
            }
            Err(format!("no {} section", String::from_utf8_lossy(name)))
        };
        Ok(Image { text: find(b".text")?, rdata: find(b".rdata")?, data: find(b".data")? })
    }
}

/// Section pages made writable for the patch, old protection restored on drop.
struct Unprotect(Vec<(Sec, u32)>);

impl Unprotect {
    fn new(secs: &[Sec]) -> Result<Unprotect, String> {
        let mut v = Vec::new();
        for &s in secs {
            let mut old = 0u32;
            if unsafe { VirtualProtect(s.va as *mut _, s.size as usize, PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
                return Err(format!("VirtualProtect {:#x} failed ({})", s.va, unsafe { GetLastError() }));
            }
            v.push((s, old));
        }
        Ok(Unprotect(v))
    }
}

impl Drop for Unprotect {
    fn drop(&mut self) {
        for &(s, old) in &self.0 {
            let mut tmp = 0u32;
            unsafe {
                VirtualProtect(s.va as *mut _, s.size as usize, old, &mut tmp);
                FlushInstructionCache(GetCurrentProcess(), s.va as *const _, s.size as usize);
            }
        }
    }
}

/// Codepage of the language's charset (patrician2 `tools/langs.js` / `charset.js`).
pub fn codepage(lang: &str) -> u32 {
    match lang {
        "ru" => 1251,
        "lt" | "lv" => 1257,
        _ => 1252,
    }
}

/// UTF-16 unit -> byte of `cp` for 0x80..0xFF.
pub fn reverse_codepage(cp: u32) -> HashMap<u16, u8> {
    let mut m = HashMap::new();
    for b in 0x80u8..=0xff {
        let mut w = 0u16;
        if unsafe { MultiByteToWideChar(cp, 0, &b, 1, &mut w, 1) } == 1 {
            m.entry(w).or_insert(b);
        }
    }
    m
}

fn encode(s: &str, rev: &HashMap<u16, u8>) -> Option<Vec<u8>> {
    s.chars()
        .map(|c| {
            let u = c as u32;
            if u < 0x80 {
                Some(u as u8)
            } else if u <= 0xffff {
                rev.get(&(u as u16)).copied()
            } else {
                None
            }
        })
        .collect()
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// `%[-+#0-9.]*[a-zA-Z%]` matches, in order.
fn specifiers(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let mut v = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let mut j = i + 1;
            while j < b.len() && (b"-+#.".contains(&b[j]) || b[j].is_ascii_digit()) {
                j += 1;
            }
            if j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == b'%') {
                v.push(s[i..=j].to_string());
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    v
}

struct Line {
    va: u32,
    en: Vec<u8>,
    tr: Vec<u8>,
}

#[derive(Default)]
struct Count {
    in_place: usize,
    relocated: usize,
    skipped: usize,
    code_refs_left: usize,
    table_bases_kept: usize,
}

/// RT_STRING resource `id` of the executable.
fn resource_string(id: u32) -> Option<String> {
    let mut buf = [0u16; 1024];
    let n = unsafe { LoadStringW(GetModuleHandleW(core::ptr::null()), id, buf.as_mut_ptr(), buf.len() as i32) };
    (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

/// Translations of `dir\*.tsv` (sorted by name); resource rows (`r<id>`) go to `res`.
fn read_tsv(
    dir: &Path,
    rev: &HashMap<u16, u8>,
    forms: bool,
    lay: &Layout,
    res: &mut HashMap<u32, Vec<u8>>,
    n: &mut Count,
) -> Vec<Line> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("tsv"))).collect())
        .unwrap_or_default();
    files.sort();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        for line in text.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let mut cols = line.split('\t');
            let (Some(off), Some(en), Some(tr)) = (cols.next(), cols.next(), cols.next()) else { continue };
            if tr.is_empty() {
                continue;
            }
            if let Some(id) = off.strip_prefix('r') {
                let tr = if tr == "\"\"" { String::new() } else { unescape(tr) };
                let en = unescape(en);
                let ok = id.parse::<u32>().ok().filter(|&id| resource_string(id).as_deref() == Some(en.as_str()));
                match (ok, encode(&tr, rev)) {
                    (Some(id), Some(trb)) if !res.contains_key(&id) => {
                        res.insert(id, trb);
                    }
                    _ => n.skipped += 1,
                }
                continue;
            }
            let Ok(off) = u32::from_str_radix(off, 16) else {
                n.skipped += 1;
                continue;
            };
            let en = unescape(en);
            let mut tr = if tr == "\"\"" { String::new() } else { unescape(tr) };
            // count-dependent forms "<11 digits>|form0|form1|...": kept for the plural hooks, else the last form
            let plural = tr.len() > 12 && tr.as_bytes()[11] == b'|' && tr.as_bytes()[..11].iter().all(u8::is_ascii_digit);
            if plural && !forms {
                tr = tr.rsplit('|').next().unwrap_or("").to_string();
            }
            let each: Vec<&str> = if plural && forms { tr[12..].split('|').collect() } else { vec![tr.as_str()] };
            // the guild's "one %s." may drop its only %s
            let fmt_ok = each
                .iter()
                .all(|x| specifiers(&en) == specifiers(x) || (off == lay.one_ship && specifiers(x).is_empty()));
            let (Some(enb), Some(trb)) = (encode(&en, rev), encode(&tr, rev)) else {
                n.skipped += 1;
                continue;
            };
            if !fmt_ok || !seen.insert(off) {
                n.skipped += 1;
                continue;
            }
            out.push(Line { va: IB + off, en: enb, tr: trb });
        }
    }
    out
}

/// Code ref at `x` (imm32 / disp32) that follows a relocated string.
enum CodeRef {
    /// `push`, `mov` or `lea` (exepatch.js codeRefOk): retargeted.
    Ok,
    /// P3 table base: kept on the original address.
    Kept,
    /// Other instruction: left alone.
    Left,
}

unsafe fn code_ref(x: u32, lay: &Layout) -> CodeRef {
    let b = |k: u32| mem::rd8(x - k);
    let modrm_c7 = |m: u8| m & 0x38 == 0; // C7 /0
    if b(1) == 0x68 || (0xb8..=0xbf).contains(&b(1)) {
        // push imm32; mov r32, imm32
        return if lay.kept_imm.contains(&x) { CodeRef::Kept } else { CodeRef::Ok };
    }
    // mov r/m32, imm32 (the imm32 ends the instruction): [reg]; [sib]; [reg+disp8]; [sib+disp8];
    // [reg+disp32] or [disp32]; [sib+disp32] or [index*s+disp32]
    let mov_imm = (b(2) == 0xc7 && modrm_c7(b(1)) && b(1) >> 6 == 0 && !matches!(b(1) & 7, 4 | 5))
        || (b(3) == 0xc7 && modrm_c7(b(2)) && b(2) >> 6 == 0 && b(2) & 7 == 4 && b(1) & 7 != 5)
        || (b(3) == 0xc7 && modrm_c7(b(2)) && b(2) >> 6 == 1 && b(2) & 7 != 4)
        || (b(4) == 0xc7 && modrm_c7(b(3)) && b(3) >> 6 == 1 && b(3) & 7 == 4)
        || (b(6) == 0xc7 && modrm_c7(b(5)) && ((b(5) >> 6 == 2 && b(5) & 7 != 4) || b(5) == 0x05))
        || (b(7) == 0xc7
            && modrm_c7(b(6))
            && ((b(6) >> 6 == 2 && b(6) & 7 == 4) || (b(6) >> 6 == 0 && b(6) & 7 == 4 && b(5) & 7 == 5)));
    if mov_imm {
        return CodeRef::Ok;
    }
    // lea / mov r32 / mov r8 with a disp32 operand: [disp32], [reg+disp32] (modrm), [base+index*s+disp32],
    // [index*s+disp32] (sib)
    let modrm = |op: u8| b(2) == op && (b(1) & 0xc7 == 0x05 || (b(1) >> 6 == 2 && b(1) & 7 != 4));
    let sib = |op: u8| b(3) == op && b(2) & 7 == 4 && (b(2) >> 6 == 2 || (b(2) >> 6 == 0 && b(1) & 7 == 5));
    let direct = matches!(b(2), 0x8d | 0x8b) && b(1) & 0xc7 == 0x05;
    if !lay.table_bases {
        return if modrm(0x8d) || modrm(0x8b) || sib(0x8d) || sib(0x8b) { CodeRef::Ok } else { CodeRef::Left };
    }
    if direct || modrm(0x8a) || sib(0x8a) {
        CodeRef::Ok // [disp32]; byte read = character access of the string
    } else if modrm(0x8d) || modrm(0x8b) || sib(0x8d) || sib(0x8b) {
        CodeRef::Kept // string start used as a table base
    } else {
        CodeRef::Left
    }
}

/// Every dword of `.text` / `.rdata` / `.data` equal to one of `wanted`: value -> (address, in .text).
fn references(img: &Image, wanted: &HashSet<u32>) -> HashMap<u32, Vec<(u32, bool)>> {
    let mut refs: HashMap<u32, Vec<(u32, bool)>> = HashMap::new();
    for (s, code) in [(img.text, true), (img.rdata, false), (img.data, false)] {
        let b = unsafe { mem::bytes(s.va, s.size as usize) };
        for i in 0..b.len().saturating_sub(3) {
            let v = u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
            if wanted.contains(&v) {
                refs.entry(v).or_default().push((s.va + i as u32, code));
            }
        }
    }
    refs
}

fn install_charset(img: &Image, cs: &[u8], lay: &Layout) -> Result<usize, String> {
    let (cs_va, cs_ptr, len) = (lay.charset, lay.charset_ptr, lay.charset_len);
    unsafe {
        if mem::rd32(cs_ptr) != cs_va {
            return Err(format!("unexpected charset pointer at {cs_ptr:#x}"));
        }
        let orig = mem::bytes(cs_va, len).to_vec();
        let d = mem::bytes(img.data.va, img.data.size as usize);
        let mut ptrs = Vec::new();
        for i in 0..d.len().saturating_sub(len + 4) {
            if d[i] != 0x20 || d[i..i + len] != orig[..] {
                continue;
            }
            let va = img.data.va + i as u32;
            let p = (va + len as u32 + 3) & !3;
            if mem::rd32(p) == va {
                ptrs.push(p);
            }
        }
        if !ptrs.contains(&cs_ptr) {
            ptrs.push(cs_ptr);
        }
        if lay.copies.is_some_and(|c| c != ptrs.len()) {
            return Err(format!("charset pointers: {} found, {} expected", ptrs.len(), lay.copies.unwrap_or(0)));
        }
        let mut z = cs.to_vec();
        z.push(0);
        let at = Box::leak(z.into_boxed_slice()).as_ptr() as u32;
        for &p in &ptrs {
            mem::wr32(p, at);
        }
        Ok(ptrs.len())
    }
}

/// Apply `<lang dir>\exe\` for `base`. Ok = summary for the log.
pub fn install(lang: &str, dir: &Path, forms: bool, base: Base) -> Result<String, String> {
    let lay = layout(base);
    let exe = dir.join("exe");
    let cs = std::fs::read(exe.join("charset.bin")).map_err(|_| "no exe\\charset.bin".to_string())?;
    if cs.is_empty() || cs.len() > 255 || cs.contains(&0) {
        return Err("exe\\charset.bin: 1..255 non-zero bytes expected".into());
    }
    let img = image()?;
    let rev = reverse_codepage(codepage(lang));
    let mut n = Count::default();
    let mut res = HashMap::new();
    let lines = read_tsv(&exe, &rev, forms, lay, &mut res, &mut n);
    // check against the untouched image first; refs of the start and of the bytes after each text
    let (lo, hi) = (img.rdata.va, img.data.va + img.data.size);
    let lines: Vec<Line> = lines
        .into_iter()
        .filter(|l| {
            let ok = l.va >= lo
                && l.va + l.en.len() as u32 + 1 <= hi
                && unsafe { mem::bytes(l.va, l.en.len() + 1) } == [&l.en[..], &[0u8][..]].concat();
            if !ok {
                n.skipped += 1;
            }
            ok
        })
        .collect();
    let mut wanted = HashSet::new();
    for l in &lines {
        wanted.insert(l.va);
        for k in 0..=4 {
            wanted.insert(l.va + l.en.len() as u32 + k);
        }
    }
    let refs = references(&img, &wanted);
    let has = |v: u32| refs.contains_key(&v);

    let _guard = Unprotect::new(&[img.text, img.rdata, img.data])?;
    let pointers = install_charset(&img, &cs, lay)?;
    for l in &lines {
        let len = l.en.len() as u32;
        let mut cap = len;
        // grow into zero padding only if nothing references the consumed byte (incl. the original terminator)
        while cap < len + 3 && unsafe { mem::rd8(l.va + cap + 1) } == 0 && !has(l.va + cap) && !has(l.va + cap + 1) {
            cap += 1;
        }
        if l.tr.len() as u32 <= cap {
            unsafe {
                core::ptr::write_bytes(l.va as *mut u8, 0, cap as usize + 1);
                core::ptr::copy_nonoverlapping(l.tr.as_ptr(), l.va as *mut u8, l.tr.len());
            }
            n.in_place += 1;
            continue;
        }
        let Some(rs) = refs.get(&l.va) else {
            n.skipped += 1;
            continue;
        };
        if rs.iter().any(|&(at, code)| !code && at % 4 != 0) {
            n.skipped += 1;
            continue;
        }
        let mut ok = Vec::new();
        for &(at, code) in rs {
            match if code { unsafe { code_ref(at, lay) } } else { CodeRef::Ok } {
                CodeRef::Ok => ok.push(at),
                CodeRef::Kept => n.table_bases_kept += 1,
                CodeRef::Left => n.code_refs_left += 1,
            }
        }
        if ok.is_empty() {
            n.skipped += 1;
            continue;
        }
        let mut z = l.tr.clone();
        z.push(0);
        let at = Box::leak(z.into_boxed_slice()).as_ptr() as u32;
        for r in ok {
            unsafe { mem::wr32(r, at) };
        }
        n.relocated += 1;
    }
    let tips = res.len();
    if !res.is_empty() {
        let _ = TIPS.set(res);
    }
    Ok(format!(
        "charset {} cells ({pointers} pointers), strings: {} in place, {} relocated, {} skipped, {} code refs left, \
         {} table bases kept, {tips} resource strings",
        cs.len(),
        n.in_place,
        n.relocated,
        n.skipped,
        n.code_refs_left,
        n.table_bases_kept
    ))
}

// ---- tool tips (P3) ----

/// Translated RT_STRING resources: id -> text in the language codepage.
static TIPS: OnceLock<HashMap<u32, Vec<u8>>> = OnceLock::new();
static RET3: AtomicU32 = AtomicU32::new(0);
/// MFC `AfxLoadString(id, buf, max)` (stdcall, the only `LoadStringA` caller) and the call in its prologue.
const LOAD_STRING: u32 = 0x65_4cdc;
const GET_MODULE: u32 = 0x65_9981;

/// P3 tool tips (exepatch.js 2n): `AfxLoadString` serves the translated resource strings of [`install`]; other ids
/// load the original resource. Ok = number of strings.
pub fn tooltips(p: &mut Plan) -> Result<usize, String> {
    let n = TIPS.get().map_or(0, HashMap::len);
    if n == 0 {
        return Err("no translated resource strings".into());
    }
    // push esi; call 659981; push dword [esp+10h]
    let expect = mem::cat(&[&[0x56], &mem::rel5(0xe8, LOAD_STRING + 1, GET_MODULE), &mem::hex("ff742410")]);
    RET3.store(Plan::ret_stub(3)?, Ordering::Relaxed);
    p.mid(LOAD_STRING, 6, &expect, &[Ins::Bytes(&[0x56]), Ins::Call(GET_MODULE)], &[], load_string, 0)?;
    Ok(n)
}

unsafe extern "C" fn load_string(c: *mut Ctx, _: u32) {
    let c = &mut *c;
    let (id, buf, max) = (*c.stack(4), *c.stack(8), *c.stack(12) as i32);
    let Some(t) = TIPS.get().and_then(|m| m.get(&id)) else { return };
    if max < 1 || buf == 0 {
        return;
    }
    let k = t.len().min(max as usize - 1);
    core::ptr::copy_nonoverlapping(t.as_ptr(), buf as *mut u8, k);
    mem::wr8(buf + k as u32, 0);
    c.eax = k as u32;
    c.resume = RET3.load(Ordering::Relaxed);
}
