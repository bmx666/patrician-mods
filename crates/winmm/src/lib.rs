//! `winmm.dll` proxy in the game folder. All three game executables (and `mss32.dll`) import `winmm.dll`, so Windows
//! loads this file before any game code runs.
//!
//! - every export jumps to the same function of `<System32>\winmm.dll` (loaded in `DllMain`)
//! - the executable's entry point gets a 5-byte jump; at the entry point (process fully initialized, outside the
//!   loader lock) the bytes are restored and `mods\*.dll` are loaded ([`pcore::mods::load_all`])
//! - started by `patrician-launcher.exe` (environment variable [`pcore::LAUNCHER_ENV`]): forwarding only, the
//!   launcher loads the mods
//!
//! Wine / CrossOver / GPTK prefer their built-in winmm: there the proxy needs the DLL override `winmm=n,b`, or the
//! game is started with `patrician-launcher.exe`.

use core::ptr::addr_of_mut;
use pcore::ffi::*;
use pcore::{log, mem};

pub mod exports {
    include!(concat!(env!("OUT_DIR"), "/exports.rs"));
}

/// Addresses of the system winmm functions, indexed like `exports::NAMES`; read by the export thunks.
pub static mut TABLE: [usize; exports::COUNT] = [0; exports::COUNT];

static mut ENTRY: u32 = 0;
static mut ENTRY_BYTES: [u8; 5] = [0; 5];

const TAG: &str = "winmm";

#[no_mangle]
pub extern "system" fn DllMain(module: HMODULE, reason: u32, _reserved: *mut core::ffi::c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        unsafe { DisableThreadLibraryCalls(module) };
        let launched = std::env::var_os(pcore::LAUNCHER_ENV).is_some();
        if !launched {
            log::reset();
        }
        if let Err(e) = unsafe { load_system_winmm() } {
            log::write(TAG, &e);
            message_box("winmm.dll", &format!("{e}\nThe game cannot run with this winmm.dll."));
            return 0;
        }
        if launched {
            log::write(TAG, "started by patrician-launcher.exe: forwarding only");
        } else if let Err(e) = unsafe { hook_entry() } {
            log::write(TAG, &format!("entry point hook failed: {e}; mods not loaded"));
        }
    }
    1
}

unsafe fn load_system_winmm() -> Result<(), String> {
    let mut dir = [0u16; 260];
    let n = GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) as usize;
    if n == 0 || n >= dir.len() {
        return Err("GetSystemDirectory failed".into());
    }
    let mut path: Vec<u16> = dir[..n].to_vec();
    path.extend("\\winmm.dll".encode_utf16());
    path.push(0);
    let module = LoadLibraryW(path.as_ptr());
    if module.is_null() {
        return Err(format!("system winmm.dll not loaded ({})", GetLastError()));
    }
    let table = &mut *addr_of_mut!(TABLE);
    let mut missing = Vec::new();
    for (i, name) in exports::NAMES.iter().enumerate() {
        let f = GetProcAddress(module, name.as_ptr());
        table[i] = if f.is_null() {
            missing.push(String::from_utf8_lossy(&name[..name.len() - 1]).into_owned());
            missing_export as *const () as usize
        } else {
            f as usize
        };
    }
    if !missing.is_empty() {
        log::write(TAG, &format!("not in the system winmm.dll: {}", missing.join(", ")));
    }
    Ok(())
}

/// Target of exports the system DLL does not have.
extern "C" fn missing_export() {
    message_box("winmm.dll", "The game called a winmm function that this system's winmm.dll does not provide.");
    std::process::abort();
}

/// Entry point of the executable -> `entry_thunk`.
unsafe fn hook_entry() -> Result<(), String> {
    let (base, info) = pcore::pe::current().ok_or("executable header not readable")?;
    let entry = base + info.entry_rva;
    ENTRY = entry;
    *addr_of_mut!(ENTRY_BYTES) = mem::bytes(entry, 5).try_into().map_err(|_| "entry bytes")?;
    mem::write(entry, &mem::rel5(0xe9, entry, entry_thunk as *const () as usize as u32))
}

#[unsafe(naked)]
unsafe extern "C" fn entry_thunk() {
    core::arch::naked_asm!(
        ".byte 0x60", // pushad
        ".byte 0x9c", // pushfd
        "cld",
        "call {f}",
        ".byte 0x9d", // popfd
        ".byte 0x61", // popad
        "jmp dword ptr [{e}]",
        f = sym on_entry,
        e = sym ENTRY,
    )
}

extern "C" fn on_entry() {
    unsafe {
        let entry = ENTRY;
        let bytes = *addr_of_mut!(ENTRY_BYTES);
        if let Err(e) = mem::write(entry, &bytes) {
            // The jump is still in place: continuing would loop forever.
            message_box("winmm.dll", &format!("Restoring the entry point failed: {e}"));
            std::process::abort();
        }
    }
    pcore::mods::load_all(TAG);
}
