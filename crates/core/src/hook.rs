//! Patch plans: every byte check of a feature runs first, then all writes are applied together. A feature whose
//! bytes differ (other game version, another mod changed them) is skipped as a whole.
//!
//! Mid-function hook ([`Plan::mid`]): the site gets `jmp thunk` (+ optional tail bytes, nops). The thunk saves all
//! registers and flags into a [`Ctx`], calls the Rust handler with it, restores the (possibly changed) registers and
//! continues at `ctx.resume`. Default resume = the replayed instructions (if any), then back to the site.
//!
//! Shared sites: a site that already starts with `jmp rel32` where the original bytes do not (hooked by another mod
//! DLL, e.g. `localization.dll` and `qol.dll` on the same WM_LBUTTONDOWN entry) is chained: only the 5-byte jump is
//! replaced, and the default resume jumps to the previous hook instead of replaying, so that hook (and finally the
//! original instructions) still runs. The last installed handler runs first; setting `ctx.resume` skips the rest.
//!
//! ```text
//! thunk:  push <resume>; pushfd; pushad; cld; mov eax,esp; push <data>; push eax; call handler; add esp,8
//!         popad; popfd; ret                       ; -> ctx.resume
//! replay: <instructions>; jmp <back>
//! ```
use crate::mem::{self, rel5};

/// Registers at the hook site, as stored by `pushad` / `pushfd`. Handlers change them in place.
#[repr(C)]
pub struct Ctx {
    pub edi: u32,
    pub esi: u32,
    pub ebp: u32,
    esp_pushad: u32,
    pub ebx: u32,
    pub edx: u32,
    pub ecx: u32,
    pub eax: u32,
    pub eflags: u32,
    /// Where execution continues after the handler.
    pub resume: u32,
}

impl Ctx {
    /// Stack pointer at the hook site.
    pub fn sp(&self) -> u32 {
        self.esp_pushad + 8
    }

    /// Address of the stack dword `[esp + off]` at the hook site.
    pub fn stack(&self, off: u32) -> *mut u32 {
        (self.sp() + off) as *mut u32
    }

    pub fn set_zf(&mut self, zero: bool) {
        if zero {
            self.eflags |= 0x40;
        } else {
            self.eflags &= !0x40;
        }
    }
}

/// Handler: `(ctx, data)`; `data` is the value given to [`Plan::mid`].
pub type Handler = unsafe extern "C" fn(*mut Ctx, u32);

/// Replayed instruction after the handler: raw bytes (position independent) or `call abs`.
pub enum Ins {
    Bytes(&'static [u8]),
    Call(u32),
}

#[derive(Default)]
pub struct Plan {
    writes: Vec<(u32, Vec<u8>)>,
}

impl Plan {
    pub fn new() -> Plan {
        Plan::default()
    }

    /// Memory at `va` must equal `expect`.
    pub fn check(&self, va: u32, expect: &[u8]) -> Result<(), String> {
        let have = unsafe { mem::bytes(va, expect.len()) };
        if have == expect {
            Ok(())
        } else {
            Err(format!("bytes at {va:#x} differ: {} (expected {})", hexs(have), hexs(expect)))
        }
    }

    /// Dword at `va` must equal `v`.
    pub fn check32(&self, va: u32, v: u32) -> Result<(), String> {
        self.check(va, &v.to_le_bytes())
    }

    pub fn write(&mut self, va: u32, data: Vec<u8>) {
        self.writes.push((va, data));
    }

    /// Check `expect` at `va`, then write `new` there.
    pub fn patch(&mut self, va: u32, expect: &[u8], new: &[u8]) -> Result<(), String> {
        self.check(va, expect)?;
        self.write(va, new.to_vec());
        Ok(())
    }

    /// Mid-function hook over `len` bytes at `site` (checked against `expect`, which may be longer than `len`).
    /// `tail` is written right after the 5-byte jump; with a tail the default return goes to it (`site + 5`),
    /// otherwise to `site + len`. The rest of the site is filled with nops.
    #[allow(clippy::too_many_arguments)]
    pub fn mid(
        &mut self,
        site: u32,
        len: usize,
        expect: &[u8],
        replay: &[Ins],
        tail: &[u8],
        handler: Handler,
        data: u32,
    ) -> Result<(), String> {
        if len < 5 + tail.len() || expect.len() < len {
            return Err(format!("hook {site:#x}: bad length"));
        }
        let prev = chained(site, expect);
        match prev {
            None => self.check(site, expect)?,
            Some(_) if !tail.is_empty() => return Err(format!("hook {site:#x}: hooked by another mod, a tail cannot chain")),
            Some(_) => {}
        }
        let back = if tail.is_empty() { site + len as u32 } else { site + 5 };
        let cave = mem::alloc_code(96)?;
        let replay_at = cave + 27;
        let mut c: Vec<u8> = Vec::with_capacity(96);
        c.push(0x68);
        c.extend_from_slice(&replay_at.to_le_bytes()); // push <default resume>
        c.extend_from_slice(&[0x9c, 0x60, 0xfc, 0x89, 0xe0]); // pushfd; pushad; cld; mov eax,esp
        c.push(0x68);
        c.extend_from_slice(&data.to_le_bytes()); // push data
        c.push(0x50); // push eax (ctx)
        c.extend_from_slice(&rel5(0xe8, cave + c.len() as u32, handler as usize as u32)); // call handler
        c.extend_from_slice(&[0x83, 0xc4, 0x08, 0x61, 0x9d, 0xc3]); // add esp,8; popad; popfd; ret
        debug_assert_eq!(c.len(), 27);
        if let Some(prev) = prev {
            // the previous hook replays the original instructions and returns to its own end of the site
            c.extend_from_slice(&rel5(0xe9, cave + c.len() as u32, prev));
        } else {
            for ins in replay {
                match ins {
                    Ins::Bytes(b) => c.extend_from_slice(b),
                    Ins::Call(to) => c.extend_from_slice(&rel5(0xe8, cave + c.len() as u32, *to)),
                }
            }
            c.extend_from_slice(&rel5(0xe9, cave + c.len() as u32, back));
        }
        if c.len() > 96 {
            return Err(format!("hook {site:#x}: replay too long"));
        }
        unsafe { core::ptr::copy_nonoverlapping(c.as_ptr(), cave as *mut u8, c.len()) };
        let mut site_bytes = rel5(0xe9, site, cave).to_vec();
        if prev.is_none() {
            site_bytes.extend_from_slice(tail);
            site_bytes.resize(len, 0x90);
        }
        self.write(site, site_bytes);
        Ok(())
    }

    /// Detour of a function entry (any module): `prologue` (whole position-independent instructions, at least 5 bytes)
    /// is checked at `entry` and replaced by `jmp target` + nops. Returns a trampoline that runs the prologue and
    /// continues in the function: calling it = calling the original.
    ///
    /// An entry that already starts with `jmp rel32` (detoured by another mod, e.g. `localization.dll` before
    /// `qol.dll`) is chained: the jump is replaced and the trampoline jumps to its old target.
    pub fn detour(&mut self, entry: u32, prologue: &[u8], target: u32) -> Result<u32, String> {
        let n = prologue.len();
        if n < 5 {
            return Err(format!("detour {entry:#x}: prologue shorter than 5 bytes"));
        }
        if let Some(prev) = chained(entry, prologue) {
            let cave = mem::alloc_code(5)?;
            let c = rel5(0xe9, cave, prev);
            unsafe { core::ptr::copy_nonoverlapping(c.as_ptr(), cave as *mut u8, c.len()) };
            self.write(entry, rel5(0xe9, entry, target).to_vec());
            return Ok(cave);
        }
        self.check(entry, prologue)?;
        let cave = mem::alloc_code(n + 5)?;
        let mut c = prologue.to_vec();
        c.extend_from_slice(&rel5(0xe9, cave + n as u32, entry + n as u32));
        unsafe { core::ptr::copy_nonoverlapping(c.as_ptr(), cave as *mut u8, c.len()) };
        let mut site = rel5(0xe9, entry, target).to_vec();
        site.resize(n, 0x90);
        self.write(entry, site);
        Ok(cave)
    }

    /// Code address of a stub returning from a stdcall function with `args` dword arguments (`ret 4*args`), as a
    /// handler `resume` target at the function entry.
    pub fn ret_stub(args: u16) -> Result<u32, String> {
        let at = mem::alloc_code(4)?;
        let n = (args * 4).to_le_bytes();
        unsafe { core::ptr::copy_nonoverlapping([0xc2, n[0], n[1]].as_ptr(), at as *mut u8, 3) };
        Ok(at)
    }

    /// Apply all writes.
    pub fn commit(self) -> Result<usize, String> {
        let n = self.writes.len();
        for (va, data) in &self.writes {
            unsafe { mem::write(*va, data)? };
        }
        Ok(n)
    }
}

/// Target of a `jmp rel32` another mod put at `site` (the original bytes `expect` do not start with one).
fn chained(site: u32, expect: &[u8]) -> Option<u32> {
    let head = unsafe { mem::bytes(site, 5) };
    (head[0] == 0xe9 && expect.first() != Some(&0xe9))
        .then(|| site.wrapping_add(5).wrapping_add(u32::from_le_bytes([head[1], head[2], head[3], head[4]])))
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
