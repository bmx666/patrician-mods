//! Helpers shared by the features.
pub use pcore::hook::{Ctx, Ins, Plan};
pub use pcore::mem::{cat, hex, le, rd32, rd8, rel5, wr16, wr32, wr8};
use std::sync::OnceLock;

/// Game `sprintf`-like function: `(buffer or CString*, format, ...)`, cdecl (P2 sprintf 5F85F0, P3 CString::Format
/// 64DF80 with the CString as first argument).
pub type Sprintf = unsafe extern "C" fn(u32, *const u8, ...) -> i32;

/// Per-feature addresses, set once at install, read by the handlers.
pub struct Cell<T>(OnceLock<T>);

impl<T> Cell<T> {
    pub const fn new() -> Self {
        Cell(OnceLock::new())
    }

    /// Store (first install wins; `start` runs once).
    pub fn set(&self, v: T) -> &T {
        let _ = self.0.set(v);
        self.0.get().expect("set")
    }

    /// Stored value; handlers are only installed after `set`.
    pub fn get(&self) -> &T {
        self.0.get().expect("feature data")
    }
}

/// `e8 rel32` at `site` to `to` (5 bytes), as bytes for an expectation.
pub fn call_bytes(site: u32, to: u32) -> Vec<u8> {
    rel5(0xe8, site, to).to_vec()
}

/// `0f 8x rel32` (near jcc, 6 bytes) at `site` to `to`.
pub fn jcc_bytes(op: u8, site: u32, to: u32) -> Vec<u8> {
    cat(&[&[0x0f, op], &to.wrapping_sub(site + 6).to_le_bytes()])
}
