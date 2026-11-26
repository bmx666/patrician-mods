//! Is the game running on an x86 translator? Every sign is read and logged:
//! - Wine: `ntdll!wine_get_version` exists (Wine, CrossOver, Game Porting Toolkit, Proton);
//! - macOS host: `ntdll!wine_get_host_version` names the system `Darwin`;
//! - Rosetta 2: the CPUID brand string is `VirtualApple ...`;
//! - ARM64 host: `IsWow64Process2` reports the native machine ARM64 (Windows on ARM, Wine wow64 on arm64).

use core::ffi::{c_char, c_void, CStr};
use pcore::ffi::*;

const IMAGE_FILE_MACHINE_ARM64: u16 = 0xaa64;

pub struct Host {
    /// Wine version.
    pub wine: Option<String>,
    /// Host system name and release under Wine (`Darwin`, `Linux`).
    pub system: Option<(String, String)>,
    /// CPUID brand string.
    pub brand: String,
    /// `IsWow64Process2` (process machine, native machine).
    pub machine: Option<(u16, u16)>,
}

impl Host {
    pub fn darwin(&self) -> bool {
        self.system.as_ref().is_some_and(|(s, _)| s.eq_ignore_ascii_case("Darwin"))
    }

    pub fn rosetta(&self) -> bool {
        self.brand.contains("VirtualApple")
    }

    pub fn arm64(&self) -> bool {
        self.machine.is_some_and(|(_, native)| native == IMAGE_FILE_MACHINE_ARM64)
    }

    /// `auto` patches: a macOS host (Apple Silicon runs it through Rosetta 2; harmless on Intel Macs), Rosetta 2
    /// itself or an ARM64 host.
    pub fn translated(&self) -> bool {
        self.darwin() || self.rosetta() || self.arm64()
    }

    pub fn describe(&self) -> String {
        let wine = match (&self.wine, &self.system) {
            (Some(v), Some((s, r))) => format!("Wine {v} on {s} {r}"),
            (Some(v), None) => format!("Wine {v}"),
            (None, _) => "Windows".to_string(),
        };
        let machine = match self.machine {
            Some((p, n)) => format!("machine {p:#06x} / native {n:#06x}"),
            None => "IsWow64Process2 unavailable".to_string(),
        };
        let mut signs = Vec::new();
        if self.darwin() {
            signs.push("macOS");
        }
        if self.rosetta() {
            signs.push("Rosetta 2");
        }
        if self.arm64() {
            signs.push("ARM64");
        }
        let signs = if signs.is_empty() { "none".to_string() } else { signs.join(", ") };
        format!("{wine}, CPU \"{}\", {machine}; translator signs: {signs}", self.brand)
    }
}

pub fn detect() -> Host {
    Host { wine: wine_version(), system: host_system(), brand: cpu_brand(), machine: machine() }
}

fn proc(module: &str, name: &[u8]) -> *const c_void {
    let m = unsafe { GetModuleHandleW(pcore::wide(module.as_ref()).as_ptr()) };
    if m.is_null() {
        return core::ptr::null();
    }
    unsafe { GetProcAddress(m, name.as_ptr()) }
}

fn text(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().trim().to_string()
}

fn wine_version() -> Option<String> {
    let f = proc("ntdll.dll", b"wine_get_version\0");
    if f.is_null() {
        return None;
    }
    let f: extern "C" fn() -> *const c_char = unsafe { core::mem::transmute(f) };
    Some(text(f()))
}

fn host_system() -> Option<(String, String)> {
    let f = proc("ntdll.dll", b"wine_get_host_version\0");
    if f.is_null() {
        return None;
    }
    let f: extern "C" fn(*mut *const c_char, *mut *const c_char) = unsafe { core::mem::transmute(f) };
    let (mut sys, mut rel) = (core::ptr::null(), core::ptr::null());
    f(&mut sys, &mut rel);
    Some((text(sys), text(rel)))
}

#[allow(unused_unsafe)]
fn cpu_brand() -> String {
    use core::arch::x86::__cpuid;
    let max = unsafe { __cpuid(0x8000_0000) }.eax;
    if max < 0x8000_0004 {
        return String::new();
    }
    let mut b = Vec::with_capacity(48);
    for leaf in 0x8000_0002u32..=0x8000_0004 {
        let r = unsafe { __cpuid(leaf) };
        for v in [r.eax, r.ebx, r.ecx, r.edx] {
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

fn machine() -> Option<(u16, u16)> {
    let f = proc("kernel32.dll", b"IsWow64Process2\0");
    if f.is_null() {
        return None;
    }
    let f: extern "system" fn(HANDLE, *mut u16, *mut u16) -> BOOL = unsafe { core::mem::transmute(f) };
    let (mut p, mut n) = (0u16, 0u16);
    (f(unsafe { GetCurrentProcess() }, &mut p, &mut n) != 0).then_some((p, n))
}
