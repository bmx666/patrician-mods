//! Self-modifying code of a loaded module's `ASSEG` section removed in memory (the blitters of `AIM.dll`).
//!
//! Under an x86 translator every write into a page that holds translated code throws that translation away. The
//! blitters patch immediates / displacements of their own instructions and keep variables between them, in the same
//! pages, on every sprite and row. Applied once at start, before the game draws anything (the module is a static
//! import of the game, so it is loaded and initialised when the mods start); the image is analysed as it is in
//! memory (already relocated), so any load address and any build of the module works:
//! 1. all data in the 16 KB pages (Apple Silicon page size) touched by `ASSEG` is moved to new memory: every
//!    relocated reference into it is retargeted, pointers stored in it are moved along;
//! 2. instructions whose immediate / displacement is patched at run time read a variable instead, and their writers
//!    write that variable (`op r,imm32` -> `op r,[var]` in place; `cmp eax,imm` / `mov [r],imm` -> trampoline; shift
//!    imm8 / `op eax,imm32` / `[base+disp32]` -> the straight-line block is moved to new code and rewritten);
//! 3. palette table bases patched into `op [idx*4+tbl]` are read from a variable by a trampoline.
//!
//! Jumps patched once at initialisation (CPU features) stay. Afterwards no write into the code pages may be left;
//! otherwise, or when anything is unexpected, nothing is changed. Nothing is written into the image before the
//! analysis is complete.

use iced_x86::{ConstantOffsets, Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind, Register};
use pcore::ffi::*;
use pcore::mem;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Page size of Apple Silicon: the translator works per 16 KB page.
const P16: u32 = 0x4000;
const SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const PAGE_READWRITE: u32 = 0x04;
/// Room for variables after the mirror of the moved pages.
const DATA_EXTRA: u32 = 0x1_0000;
const CODE_SIZE: u32 = 0x1_0000;

/// `81 /n r,imm32` -> `op r,[var]` (same flags): add, or, and, sub, xor, cmp.
fn alu_op(n: u8) -> Option<u8> {
    match n {
        0 => Some(0x03),
        1 => Some(0x0b),
        4 => Some(0x23),
        5 => Some(0x2b),
        6 => Some(0x33),
        7 => Some(0x3b),
        _ => None,
    }
}

/// `op eax,imm32` -> `op eax,[var]`: add, or, and, sub, xor.
fn acc_op(b: u8) -> Option<u8> {
    match b {
        0x05 => Some(0x03),
        0x0d => Some(0x0b),
        0x25 => Some(0x23),
        0x2d => Some(0x2b),
        0x35 => Some(0x33),
        _ => None,
    }
}

const GPR: [Register; 8] =
    [Register::EAX, Register::ECX, Register::EDX, Register::EBX, Register::ESP, Register::EBP, Register::ESI, Register::EDI];

/// GPR numbers (0 = eax .. 7 = edi) of the register operands of `i`, added to `set`.
fn reg_busy(i: &Instruction, mut set: HashSet<u8>) -> HashSet<u8> {
    const LOW: [Register; 4] = [Register::AL, Register::CL, Register::DL, Register::BL];
    const HIGH: [Register; 4] = [Register::AH, Register::CH, Register::DH, Register::BH];
    const WORD: [Register; 8] =
        [Register::AX, Register::CX, Register::DX, Register::BX, Register::SP, Register::BP, Register::SI, Register::DI];
    for n in 0..i.op_count() {
        if i.op_kind(n) != OpKind::Register {
            continue;
        }
        let r = i.op_register(n);
        for list in [&GPR[..], &LOW[..], &HIGH[..], &WORD[..]] {
            if let Some(k) = list.iter().position(|&x| x == r) {
                set.insert(k as u8);
            }
        }
    }
    set
}

/// First free scratch register (ebx, ecx, edx, esi, edi, eax).
fn scratch(busy: &HashSet<u8>) -> Result<u8, String> {
    [3u8, 1, 2, 6, 7, 0].into_iter().find(|x| !busy.contains(x)).ok_or_else(|| "no scratch register".to_string())
}

struct Sec {
    name: String,
    rva: u32,
    vsz: u32,
    ch: u32,
}

#[derive(Clone, Copy)]
struct Writer {
    ip: u32,
    len: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Imm,
    Disp,
}

struct Site {
    a: u32,
    i: Instruction,
    c: ConstantOffsets,
    fld: Field,
    off: u32,
    writers: Vec<Writer>,
}

struct Patcher {
    base: u32,
    size: u32,
    secs: Vec<Sec>,
    /// Image as found (read only).
    orig: Vec<u8>,
    /// Patched copy of the image, written back at the end where it differs.
    out: Vec<u8>,
    /// RVAs of the HIGHLOW relocations.
    relocs: BTreeSet<u32>,
    exports: Vec<u32>,
    /// `ASSEG` range (VA).
    lo: u32,
    hi: u32,
    code_start: BTreeMap<u32, Instruction>,
    /// Start of the instruction covering each `ASSEG` byte (0 = no code).
    owner: Vec<u32>,
    entries: BTreeSet<u32>,
    /// Moved range (VA): the 16 KB pages touched by `ASSEG`.
    mlo: u32,
    mhi: u32,
    dbase: u32,
    dbuf: Vec<u8>,
    dtop: u32,
    cbase: u32,
    cbuf: Vec<u8>,
    ctop: u32,
    /// Instructions replaced by `jmp` + `nop` (start, length).
    replaced: Vec<(u32, u32)>,
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, String> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| format!("header out of range at {o:#x}"))
}

fn u32_at(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("out of range at {o:#x}"))
}

/// Every instruction of a section, decoded linearly from `buf` (the image at `base`).
fn each_insn(buf: &[u8], base: u32, s: &Sec, mut f: impl FnMut(&Instruction)) {
    let start = s.rva as usize;
    let end = (s.rva as usize + s.vsz as usize).min(buf.len());
    if start >= end {
        return;
    }
    let mut d = Decoder::with_ip(32, &buf[start..end], (base + s.rva) as u64, DecoderOptions::NONE);
    let mut i = Instruction::default();
    while d.can_decode() {
        d.decode_out(&mut i);
        f(&i);
    }
}

/// Patch the self-modifying code of the loaded module `name`; returns the summary for the log.
pub fn patch(name: &str) -> Result<String, String> {
    let mut p = unsafe { Patcher::load(name)? };
    p.map_code()?;
    p.layout()?;
    let moved = p.move_data()?;
    p.run(moved)
}

impl Patcher {
    unsafe fn load(name: &str) -> Result<Patcher, String> {
        let m = GetModuleHandleW(pcore::wide(name.as_ref()).as_ptr());
        if m.is_null() {
            return Err("not loaded".into());
        }
        let base = m as u32;
        let head = mem::bytes(base, 0x1000);
        if &head[0..2] != b"MZ" {
            return Err("no MZ header".into());
        }
        let pe = u32_at(head, 0x3c)? as usize;
        if head.get(pe..pe + 4) != Some(&b"PE\0\0"[..]) || u16_at(head, pe + 4)? != 0x14c || u16_at(head, pe + 24)? != 0x10b {
            return Err("not a 32-bit PE image".into());
        }
        let nsec = u16_at(head, pe + 6)? as usize;
        let opt = pe + 24;
        let shdr = opt + u16_at(head, pe + 20)? as usize;
        let size = u32_at(head, opt + 56)?;
        let mut secs = Vec::new();
        for k in 0..nsec {
            let s = shdr + 40 * k;
            let raw = head.get(s..s + 8).ok_or("section table out of range")?;
            let name = String::from_utf8_lossy(raw).trim_end_matches('\0').to_string();
            let (vsz, rva, ch) = (u32_at(head, s + 8)?, u32_at(head, s + 12)?, u32_at(head, s + 36)?);
            if rva.checked_add(vsz).is_none_or(|e| e > size) {
                return Err(format!("section {name} outside the image"));
            }
            secs.push(Sec { name, rva, vsz, ch });
        }
        if secs.iter().any(|s| s.name == ".rosd" || s.name == ".rosc") {
            return Err("the file is already patched on disk (.rosd / .rosc sections)".into());
        }
        let orig = mem::bytes(base, size as usize).to_vec();
        // relocations: HIGHLOW only (32-bit image)
        let (rdir, rsz) = (u32_at(&orig, opt + 136)? as usize, u32_at(&orig, opt + 140)? as usize);
        if rdir == 0 || rsz == 0 {
            return Err("no relocations".into());
        }
        let mut relocs = BTreeSet::new();
        let (mut o, end) = (rdir, rdir + rsz);
        while o + 8 <= end {
            let (page, bs) = (u32_at(&orig, o)?, u32_at(&orig, o + 4)? as usize);
            if bs < 8 {
                break;
            }
            for k in (8..bs).step_by(2) {
                let e = u16_at(&orig, o + k)?;
                match e >> 12 {
                    0 => {}
                    3 => {
                        let r = page + (e & 0xfff) as u32;
                        if r + 4 > size {
                            return Err(format!("relocation {r:#x} outside the image"));
                        }
                        relocs.insert(r);
                    }
                    t => return Err(format!("relocation type {t}")),
                }
            }
            o += bs;
        }
        let mut exports = Vec::new();
        let edir = u32_at(&orig, opt + 96)? as usize;
        if edir != 0 {
            let (nf, af) = (u32_at(&orig, edir + 20)? as usize, u32_at(&orig, edir + 28)? as usize);
            for k in 0..nf {
                exports.push(u32_at(&orig, af + 4 * k)?);
            }
        }
        let out = orig.clone();
        Ok(Patcher {
            base,
            size,
            secs,
            orig,
            out,
            relocs,
            exports,
            lo: 0,
            hi: 0,
            code_start: BTreeMap::new(),
            owner: Vec::new(),
            entries: BTreeSet::new(),
            mlo: 0,
            mhi: 0,
            dbase: 0,
            dbuf: Vec::new(),
            dtop: 0,
            cbase: 0,
            cbuf: Vec::new(),
            ctop: 0,
            replaced: Vec::new(),
        })
    }

    fn ix(&self, va: u32, n: u32) -> Result<usize, String> {
        let o = va.wrapping_sub(self.base);
        if o.checked_add(n).is_none_or(|e| e > self.size) {
            return Err(format!("address {va:#x} outside the image"));
        }
        Ok(o as usize)
    }

    fn b(&self, va: u32) -> Result<u8, String> {
        Ok(self.orig[self.ix(va, 1)?])
    }

    fn rd32(&self, va: u32) -> Result<u32, String> {
        u32_at(&self.orig, self.ix(va, 4)?)
    }

    fn out_rd32(&self, va: u32) -> Result<u32, String> {
        u32_at(&self.out, self.ix(va, 4)?)
    }

    fn out_wr32(&mut self, va: u32, v: u32) -> Result<(), String> {
        let o = self.ix(va, 4)?;
        self.out[o..o + 4].copy_from_slice(&v.to_le_bytes());
        Ok(())
    }

    fn out_bytes(&self, va: u32, n: u32) -> Result<&[u8], String> {
        let o = self.ix(va, n)?;
        Ok(&self.out[o..o + n as usize])
    }

    fn orig_bytes(&self, va: u32, n: u32) -> Result<&[u8], String> {
        let o = self.ix(va, n)?;
        Ok(&self.orig[o..o + n as usize])
    }

    fn decode_at(&self, va: u32) -> Result<(Instruction, ConstantOffsets), String> {
        let o = self.ix(va, 1)?;
        let s = &self.orig[o..(o + 16).min(self.orig.len())];
        let mut d = Decoder::with_ip(32, s, va as u64, DecoderOptions::NONE);
        let i = d.decode();
        if i.is_invalid() {
            return Err(format!("invalid code at {va:#x}"));
        }
        let c = d.get_constant_offsets(&i);
        Ok((i, c))
    }

    fn asseg(&self) -> Result<&Sec, String> {
        self.secs.iter().find(|s| s.name == "ASSEG").ok_or_else(|| "no ASSEG section".to_string())
    }

    /// Sections whose code may write into `ASSEG`: the executable ones and `ASSEG` itself (not flagged executable in
    /// every build).
    fn scan_secs(&self) -> Vec<&Sec> {
        self.secs.iter().filter(|s| s.ch & SCN_MEM_EXECUTE != 0 || s.name == "ASSEG").collect()
    }

    fn is_code(&self, va: u32) -> bool {
        self.owner_of(va).is_some()
    }

    fn owner_of(&self, va: u32) -> Option<u32> {
        if va < self.lo || va >= self.hi {
            return None;
        }
        Some(self.owner[(va - self.lo) as usize]).filter(|&o| o != 0)
    }

    /// Code map of `ASSEG` by recursive descent from calls / jumps of the other code sections, relocated
    /// `mov r,imm32` / `push imm32` and exports.
    fn map_code(&mut self) -> Result<(), String> {
        let (rva, vsz) = {
            let a = self.asseg()?;
            (a.rva, a.vsz)
        };
        self.lo = self.base + rva;
        self.hi = self.lo + vsz;
        self.owner = vec![0; vsz as usize];
        let (lo, hi) = (self.lo, self.hi);
        let mut entries = BTreeSet::new();
        for s in self.scan_secs().into_iter().filter(|s| s.name != "ASSEG") {
            each_insn(&self.orig, self.base, s, |i| {
                if matches!(i.flow_control(), FlowControl::Call | FlowControl::UnconditionalBranch)
                    && i.op0_kind() == OpKind::NearBranch32
                {
                    let t = i.near_branch_target() as u32;
                    if t >= lo && t < hi {
                        entries.insert(t);
                    }
                }
            });
        }
        for &r in &self.relocs {
            let v = u32_at(&self.orig, r as usize)?;
            if r >= 1 && v >= lo && v < hi {
                let op = self.orig[r as usize - 1];
                if (0xb8..=0xbf).contains(&op) || op == 0x68 {
                    entries.insert(v);
                }
            }
        }
        for &e in &self.exports {
            let v = self.base.wrapping_add(e);
            if v >= lo && v < hi {
                entries.insert(v);
            }
        }
        let mut q: Vec<u32> = entries.iter().copied().collect();
        while let Some(mut a) = q.pop() {
            while a >= lo && a < hi && !self.code_start.contains_key(&a) {
                let (i, _) = self.decode_at(a)?;
                for k in 0..i.len() as u32 {
                    if a + k < hi {
                        self.owner[(a + k - lo) as usize] = a;
                    }
                }
                self.code_start.insert(a, i);
                if matches!(i.op0_kind(), OpKind::NearBranch32 | OpKind::NearBranch16) {
                    q.push(i.near_branch_target() as u32);
                }
                if matches!(
                    i.flow_control(),
                    FlowControl::Return | FlowControl::UnconditionalBranch | FlowControl::IndirectBranch
                ) {
                    break;
                }
                a += i.len() as u32;
            }
        }
        if self.code_start.is_empty() {
            return Err("no code found in ASSEG".into());
        }
        self.entries = entries;
        Ok(())
    }

    /// Moved range, new data (mirror of the moved range + variables) and new code memory.
    fn layout(&mut self) -> Result<(), String> {
        let (rva, vsz) = {
            let a = self.asseg()?;
            (a.rva, a.vsz)
        };
        self.mlo = self.base + (rva & !(P16 - 1));
        self.mhi = self.base + ((rva + vsz + P16 - 1) & !(P16 - 1));
        let mirror = self.mhi - self.mlo;
        let dsize = mirror + DATA_EXTRA;
        let d = unsafe { VirtualAlloc(core::ptr::null_mut(), dsize as usize, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        let c = unsafe {
            VirtualAlloc(core::ptr::null_mut(), CODE_SIZE as usize, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE)
        };
        if d.is_null() || c.is_null() {
            return Err(format!("VirtualAlloc failed ({})", unsafe { GetLastError() }));
        }
        (self.dbase, self.cbase) = (d as u32, c as u32);
        if self.dbase % P16 != 0 || self.cbase % P16 != 0 {
            return Err("new memory not 16 KB aligned".into());
        }
        self.dbuf = vec![0; dsize as usize];
        for va in self.mlo..self.mhi {
            if let Ok(o) = self.ix(va, 1) {
                self.dbuf[(va - self.mlo) as usize] = self.orig[o];
            }
        }
        self.dtop = mirror;
        self.cbuf = vec![0; CODE_SIZE as usize];
        Ok(())
    }

    fn dalloc(&mut self, init: u32) -> Result<u32, String> {
        self.dtop = (self.dtop + 3) & !3;
        if self.dtop + 4 > self.dbuf.len() as u32 {
            return Err("variable space full".into());
        }
        let at = self.dtop as usize;
        self.dbuf[at..at + 4].copy_from_slice(&init.to_le_bytes());
        self.dtop += 4;
        Ok(self.dbase + at as u32)
    }

    /// New code: `code` + `jmp back` (when given), 16-byte aligned; returns its address.
    fn cemit(&mut self, mut code: Vec<u8>, back: Option<u32>) -> Result<u32, String> {
        self.ctop = (self.ctop + 15) & !15;
        let n = code.len() as u32 + if back.is_some() { 5 } else { 0 };
        if self.ctop + n > CODE_SIZE {
            return Err("code space full".into());
        }
        let at = self.cbase + self.ctop;
        if let Some(to) = back {
            code.extend_from_slice(&mem::rel5(0xe9, at + n - 5, to));
        }
        let o = self.ctop as usize;
        self.cbuf[o..o + code.len()].copy_from_slice(&code);
        self.ctop += n;
        Ok(at)
    }

    /// `jmp to` at `va`, the rest of the `len` bytes `nop`.
    fn jump_out(&mut self, va: u32, len: u32, to: u32) -> Result<(), String> {
        let o = self.ix(va, len)?;
        self.out[o..o + 5].copy_from_slice(&mem::rel5(0xe9, va, to));
        self.out[o + 5..o + len as usize].fill(0x90);
        self.replaced.push((va, len));
        Ok(())
    }

    fn remap(&self, v: u32) -> u32 {
        if v >= self.mlo && v < self.mhi && !self.is_code(v) {
            v.wrapping_add(self.dbase.wrapping_sub(self.mlo))
        } else {
            v
        }
    }

    /// Step 1: retarget every relocated reference into the moved data. Returns (references, pointers in moved data).
    fn move_data(&mut self) -> Result<(usize, usize), String> {
        let (mut moved, mut dup) = (0, 0);
        let relocs: Vec<u32> = self.relocs.iter().copied().collect();
        for r in relocs {
            let va = self.base + r;
            if va >= self.mlo && va + 4 <= self.mhi && !self.is_code(va) && !self.is_code(va + 3) {
                let no = (va - self.mlo) as usize;
                let v = u32_at(&self.dbuf, no)?;
                let nv = self.remap(v);
                self.dbuf[no..no + 4].copy_from_slice(&nv.to_le_bytes());
                dup += 1;
            }
            let v = self.out_rd32(va)?;
            let nv = self.remap(v);
            if nv != v {
                self.out_wr32(va, nv)?;
                moved += 1;
            }
        }
        // absolute references from ASSEG code into the moved range must all be relocated
        for (&a, i) in &self.code_start {
            let (_, c) = self.decode_at(a)?;
            if c.has_displacement() && c.displacement_size() == 4 {
                let v = i.memory_displacement32();
                if v >= self.mlo && v < self.mhi && !self.relocs.contains(&(a - self.base + c.displacement_offset() as u32)) {
                    return Err(format!("unrelocated reference into the moved range at {a:#x}"));
                }
            }
        }
        Ok((moved, dup))
    }

    /// `mov [disp32],r32` / `mov [disp32],r8` writer: register number (| 0x10 for a byte register).
    fn src_reg(&self, w: &Writer) -> Result<u8, String> {
        let (b0, b1) = (self.b(w.ip)?, self.b(w.ip + 1).unwrap_or(0));
        let r = match (b0, w.len) {
            (0xa3, 5) => Some(0),
            (0x89, 6) if b1 & 0xc7 == 0x05 => Some((b1 >> 3) & 7),
            (0xa2, 5) => Some(0x10),
            (0x88, 6) if b1 & 0xc7 == 0x05 => Some(0x10 | ((b1 >> 3) & 7)),
            _ => None,
        };
        r.ok_or_else(|| format!("unexpected code writer at {:#x}", w.ip))
    }

    fn opc(&self, s: &Site) -> (u8, u8) {
        (self.b(s.a).unwrap_or(0), self.b(s.a + 1).unwrap_or(0))
    }

    fn is_shift(&self, s: &Site) -> bool {
        let (b0, b1) = self.opc(s);
        s.fld == Field::Imm && s.c.immediate_size() == 1 && s.i.len() == 3 && b0 == 0xc1 && b1 & 0xc0 == 0xc0
    }

    fn is_base_disp(&self, s: &Site) -> bool {
        s.fld == Field::Disp
            && s.i.memory_index() == Register::None
            && s.i.memory_base() != Register::None
            && s.i.memory_base() != Register::ESP
            && self.b(s.a + s.off - 1).is_ok_and(|m| m >> 6 == 2)
    }

    fn is_alu(&self, s: &Site) -> bool {
        let (b0, b1) = self.opc(s);
        s.fld == Field::Imm
            && s.c.immediate_size() == 4
            && s.i.len() == 6
            && b0 == 0x81
            && b1 & 0xc0 == 0xc0
            && alu_op((b1 >> 3) & 7).is_some()
    }

    fn is_acc(&self, s: &Site) -> bool {
        s.fld == Field::Imm && s.c.immediate_size() == 4 && s.i.len() == 5 && acc_op(self.opc(s).0).is_some()
    }

    /// Rewritten form of a run-time patched instruction, placed inside a moved block.
    fn stub_form(&self, s: &Site, v: u32) -> Result<Vec<u8>, String> {
        let (b0, b1) = self.opc(s);
        let vb = v.to_le_bytes();
        if self.is_shift(s) {
            return Ok(if b1 & 7 == 1 {
                // shift ecx: push ebx; mov ebx,ecx; mov cl,[v]; shift ebx,cl; mov ecx,ebx; pop ebx
                mem::cat(&[&[0x53, 0x8b, 0xd9, 0x8a, 0x0d], &vb, &[0xd3, (b1 & 0xf8) | 3, 0x8b, 0xcb, 0x5b]])
            } else {
                // push ecx; mov cl,[v]; shift r,cl; pop ecx
                mem::cat(&[&[0x51, 0x8a, 0x0d], &vb, &[0xd3, b1, 0x59]])
            });
        }
        if self.is_acc(s) {
            return Ok(mem::cat(&[&[acc_op(b0).unwrap_or(0), 0x05], &vb]));
        }
        if self.is_alu(s) {
            return Ok(mem::cat(&[&[alu_op((b1 >> 3) & 7).unwrap_or(0), ((b1 & 7) << 3) | 5], &vb]));
        }
        if self.is_base_disp(s) {
            // push tmp; mov tmp,[v]; op [tmp+base]; pop tmp
            let len = s.i.len() as u32;
            let mo = s.off - 1;
            let modrm = self.b(s.a + mo)?;
            let base = modrm & 7;
            let tmp = scratch(&reg_busy(&s.i, HashSet::from([base, 4, 5])))?;
            let head = self.orig_bytes(s.a, mo)?;
            let tail = self.orig_bytes(s.a + s.off + 4, len - s.off - 4)?;
            return Ok(mem::cat(&[
                &[0x50 + tmp, 0x8b, (tmp << 3) | 5],
                &vb,
                head,
                &[(modrm & 0x38) | 4, (base << 3) | tmp],
                tail,
                &[0x58 + tmp],
            ]));
        }
        Err(format!("no rewritten form for the instruction at {:#x}", s.a))
    }

    /// Steps 2 and 3, the final check and the write-back.
    fn run(mut self, (moved, dup): (usize, usize)) -> Result<String, String> {
        // writers: "op [disp32], ..." into ASSEG code
        let mut writers = Vec::new();
        for s in self.scan_secs() {
            let asseg = s.name == "ASSEG";
            each_insn(&self.orig, self.base, s, |i| {
                let ip = i.ip32();
                if asseg && !self.code_start.contains_key(&ip) {
                    return;
                }
                if i.op0_kind() != OpKind::Memory || i.memory_base() != Register::None || i.memory_index() != Register::None {
                    return;
                }
                let disp = i.memory_displacement32();
                if !self.is_code(disp)
                    || matches!(i.mnemonic(), Mnemonic::Cmp | Mnemonic::Test | Mnemonic::Push | Mnemonic::Call | Mnemonic::Jmp)
                    || i.op1_kind() == OpKind::Memory
                {
                    return;
                }
                writers.push((Writer { ip, len: i.len() as u32 }, disp));
            });
        }
        if writers.is_empty() {
            return Err("no self-modifying code found".into());
        }
        let mut sites: BTreeMap<u32, Site> = BTreeMap::new();
        for (w, disp) in writers {
            let own = self.owner_of(disp).ok_or("writer target outside the code map")?;
            let (i, c) = self.decode_at(own)?;
            let off = disp - own;
            let fld = if c.has_immediate() && off as usize == c.immediate_offset() && matches!(c.immediate_size(), 1 | 4) {
                Field::Imm
            } else if c.has_displacement() && off as usize == c.displacement_offset() && c.displacement_size() == 4 {
                Field::Disp
            } else {
                return Err(format!("unexpected code write at {:#x} into {own:#x}", w.ip));
            };
            sites.entry(own).or_insert(Site { a: own, i, c, fld, off, writers: Vec::new() }).writers.push(w);
        }

        // variables for every patched field (current value), writers redirected to them
        let mut var_of: HashMap<u32, u32> = HashMap::new();
        let (mut pending, mut tables, mut jumps) = (Vec::new(), Vec::new(), 0);
        for s in sites.values() {
            if matches!(s.i.flow_control(), FlowControl::UnconditionalBranch | FlowControl::ConditionalBranch) {
                jumps += 1;
                continue;
            }
            let table = s.fld == Field::Disp
                && s.i.memory_index() != Register::None
                && s.i.memory_base() == Register::None
                && s.i.memory_index_scale() == 4;
            let byte_fld = s.fld == Field::Imm && s.c.immediate_size() == 1;
            for w in &s.writers {
                let r = self.src_reg(w)?;
                if r & 0x10 != 0 && (table || !byte_fld) || r & 0x10 == 0 && byte_fld {
                    return Err(format!("writer at {:#x} does not match the field at {:#x}", w.ip, s.a));
                }
            }
            let init = if byte_fld { self.b(s.a + s.off)? as u32 } else { self.rd32(s.a + s.off)? };
            let v = self.dalloc(init)?;
            var_of.insert(s.a, v);
            for w in s.writers.clone() {
                self.out_wr32(w.ip + w.len - 4, v)?;
            }
            if table {
                tables.push(s.a);
            } else {
                pending.push(s.a);
            }
        }

        let mut branch_targets = self.entries.clone();
        for i in self.code_start.values() {
            if matches!(i.op0_kind(), OpKind::NearBranch32 | OpKind::NearBranch16) {
                branch_targets.insert(i.near_branch_target() as u32);
            }
        }

        let (mut in_place, mut tramps, mut blocks) = (0, 0, 0);
        let mut in_block: HashSet<u32> = HashSet::new();
        for a in pending {
            if in_block.contains(&a) {
                continue;
            }
            let s = &sites[&a];
            let v = var_of[&a];
            let (b0, b1) = self.opc(s);
            if self.is_shift(s) || self.is_base_disp(s) || self.is_acc(s) {
                // move the straight-line block [a, a+covered) (>= 5 bytes) to new code, rewriting patched instructions
                let mut code = Vec::new();
                let mut covered = 0u32;
                while covered < 5 {
                    let at = a + covered;
                    let ii = *self.code_start.get(&at).ok_or_else(|| format!("cannot move the block at {at:#x}"))?;
                    if ii.flow_control() != FlowControl::Next {
                        return Err(format!("cannot move the block at {at:#x}: branch"));
                    }
                    if covered > 0 && branch_targets.contains(&at) {
                        return Err(format!("branch target inside the block at {at:#x}"));
                    }
                    match (sites.get(&at), var_of.get(&at)) {
                        (Some(si), Some(&sv)) => {
                            code.extend(self.stub_form(si, sv)?);
                            in_block.insert(at);
                        }
                        (Some(_), None) => return Err(format!("unexpected patched instruction in the block at {at:#x}")),
                        (None, _) => code.extend_from_slice(self.out_bytes(at, ii.len() as u32)?),
                    }
                    covered += ii.len() as u32;
                }
                let st = self.cemit(code, Some(a + covered))?;
                self.jump_out(a, covered, st)?;
                blocks += 1;
            } else if self.is_alu(s) {
                let o = self.ix(a, 6)?;
                self.out[o] = alu_op((b1 >> 3) & 7).unwrap_or(0);
                self.out[o + 1] = ((b1 & 7) << 3) | 5;
                self.out_wr32(a + 2, v)?;
                in_place += 1;
            } else if s.i.mnemonic() == Mnemonic::Cmp && s.i.len() == 5 && b0 == 0x3d {
                // cmp eax,[v]; jmp back
                let st = self.cemit(mem::cat(&[&[0x3b, 0x05], &v.to_le_bytes()]), Some(a + 5))?;
                self.jump_out(a, 5, st)?;
                tramps += 1;
            } else if s.i.mnemonic() == Mnemonic::Mov
                && s.i.len() == 6
                && b0 == 0xc7
                && b1 & 0xf8 == 0
                && b1 & 7 != 4
                && b1 & 7 != 5
            {
                // mov [rm],imm32 -> push tmp; mov tmp,[v]; mov [rm],tmp; pop tmp; jmp back
                let rm = b1 & 7;
                let tmp = scratch(&HashSet::from([rm, 4, 5]))?;
                let code = mem::cat(&[&[0x50 + tmp, 0x8b, (tmp << 3) | 5], &v.to_le_bytes(), &[0x89, (tmp << 3) | rm, 0x58 + tmp]]);
                let st = self.cemit(code, Some(a + 6))?;
                self.jump_out(a, 6, st)?;
                tramps += 1;
            } else {
                return Err(format!("unhandled patched instruction at {a:#x}"));
            }
        }

        // tables: op [idx*4+tbl] -> push tmp; mov tmp,[v]; op [tmp+idx*4]; pop tmp; jmp back (flags untouched)
        for &a in &tables {
            let s = &sites[&a];
            let len = s.i.len() as u32;
            let d_off = s.c.displacement_offset() as u32;
            if d_off + 4 != len || s.c.has_immediate() || d_off < 2 {
                return Err(format!("unexpected table instruction at {a:#x}"));
            }
            let mo = d_off - 2;
            let (modrm, sib) = (self.b(a + mo)?, self.b(a + mo + 1)?);
            if modrm & 0xc7 != 0x04 || sib & 7 != 5 || sib >> 6 != 2 {
                return Err(format!("unexpected table addressing at {a:#x}"));
            }
            let idx = (sib >> 3) & 7;
            let tmp = scratch(&reg_busy(&s.i, HashSet::from([idx, 4, 5])))?;
            let code = mem::cat(&[
                &[0x50 + tmp, 0x8b, (tmp << 3) | 5],
                &var_of[&a].to_le_bytes(),
                self.orig_bytes(a, mo)?,
                &[(modrm & 0x38) | 4, (sib & 0xf8) | tmp, 0x58 + tmp],
            ]);
            let st = self.cemit(code, Some(a + len))?;
            self.jump_out(a, len, st)?;
        }

        // no write into the code pages may be left except the init-time jump patches
        let mut left = Vec::new();
        for s in self.scan_secs() {
            let asseg = s.name == "ASSEG";
            each_insn(&self.out, self.base, s, |i| {
                let ip = i.ip32();
                if asseg && !self.code_start.contains_key(&ip) {
                    return;
                }
                // bytes inside a replaced instruction (jmp rel32 / nop) are not code any more
                if self.replaced.iter().any(|&(a, n)| ip > a && ip < a + n) {
                    return;
                }
                if i.op0_kind() != OpKind::Memory || i.memory_base() != Register::None || i.memory_index() != Register::None {
                    return;
                }
                let disp = i.memory_displacement32();
                if disp < self.mlo
                    || disp >= self.mhi
                    || matches!(i.mnemonic(), Mnemonic::Cmp | Mnemonic::Test | Mnemonic::Push | Mnemonic::Call | Mnemonic::Jmp)
                    || i.op1_kind() == OpKind::Memory
                {
                    return;
                }
                if let Some(si) = self.owner_of(disp).and_then(|o| sites.get(&o)) {
                    if si.i.flow_control() != FlowControl::Next {
                        return;
                    }
                }
                left.push(ip);
            });
        }
        if let Some(first) = left.first() {
            return Err(format!("{} writes into the code pages remain (first at {first:#x})", left.len()));
        }

        let writes = self.commit()?;
        Ok(format!(
            "{} ASSEG instructions; data of {:#x}..{:#x} moved to {:#x} ({moved} references, {dup} stored pointers); \
             {} variables: {in_place} in place, {tramps} trampolines, {blocks} moved blocks, {} table sites; \
             {jumps} init-time jumps left; code at {:#x}; {writes} image writes",
            self.code_start.len(),
            self.mlo,
            self.mhi,
            self.dbase,
            var_of.len(),
            tables.len(),
            self.cbase,
        ))
    }

    /// Copy the new data and code to their memory, then every changed run of the image. Returns the image writes.
    fn commit(&self) -> Result<usize, String> {
        unsafe {
            core::ptr::copy_nonoverlapping(self.dbuf.as_ptr(), self.dbase as *mut u8, self.dtop as usize);
            core::ptr::copy_nonoverlapping(self.cbuf.as_ptr(), self.cbase as *mut u8, self.ctop as usize);
            FlushInstructionCache(GetCurrentProcess(), self.cbase as *const _, self.ctop as usize);
        }
        let n = self.out.len();
        let (mut i, mut writes) = (0usize, 0usize);
        while i < n {
            if self.out[i] == self.orig[i] {
                i += 1;
                continue;
            }
            // one write per run of changes less than 16 bytes apart
            let (start, mut last, mut j) = (i, i, i + 1);
            while j < n && j <= last + 16 {
                if self.out[j] != self.orig[j] {
                    last = j;
                }
                j += 1;
            }
            unsafe { mem::write(self.base + start as u32, &self.out[start..=last])? };
            writes += 1;
            i = last + 1;
        }
        Ok(writes)
    }
}
