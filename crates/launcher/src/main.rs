//! `patrician-launcher.exe`: starts the game with `mods\*.dll` loaded, without `winmm.dll` and without DLL overrides
//! (Wine / CrossOver / GPTK keep their built-in winmm).
//!
//! 1. finds the game executable next to the launcher (`[loader] exe=` or `Patrician3.exe`, `Patrician 2.exe`)
//! 2. starts it suspended, with [`pcore::LAUNCHER_ENV`] set (a `winmm.dll` proxy, if also installed, then only
//!    forwards)
//! 3. writes a stub into the process and a 5-byte jump to it at the executable's entry point
//! 4. resumes the process; at the entry point the stub restores the 5 bytes, loads every mod DLL and calls its
//!    `start` export, then jumps to the entry point
//!
//! The stub calls `LoadLibraryA` / `GetProcAddress` through the executable's own import table (resolved by the time
//! the entry point runs), so no address of the launcher process is assumed in the game process.
#![windows_subsystem = "windows"]

use core::ffi::c_void;
use pcore::ffi::*;
use pcore::game::{Game, EXE_NAMES};
use pcore::{ini::Ini, log, mem::rel5, mods, paths, pe};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const TAG: &str = "launcher";

fn main() {
    log::reset();
    if let Err(e) = run() {
        log::write(TAG, &e);
        message_box("Patrician launcher", &e);
    }
}

fn run() -> Result<(), String> {
    let dir = paths::game_dir().ok_or("launcher folder unknown")?;
    let ini = Ini::load(paths::settings().as_deref());
    let exe = find_exe(&dir, &ini)?;
    let file = std::fs::read(&exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    let info = pe::parse(&file).ok_or_else(|| format!("{}: not a 32-bit Windows executable", exe.display()))?;
    match Game::identify(&info) {
        Some(g) => log::write(TAG, &format!("{}: {}", exe.display(), g.name())),
        None => log::write(TAG, &format!("{}: unknown executable (mods check their own patch sites)", exe.display())),
    }

    let mut list = Vec::new();
    if !ini.flag("loader", "enabled", true) {
        log::write(TAG, "[loader] enabled=0: game started without mods");
    } else if info.dll_characteristics & pe::DYNAMIC_BASE != 0 {
        log::write(TAG, "relocatable executable: not supported, game started without mods");
    } else {
        list = paths::mods_dir().map(|d| mods::list(&d)).unwrap_or_default();
        log::write(TAG, &format!("{} mod(s) in mods\\", list.len()));
    }
    let imports = if list.is_empty() {
        None
    } else {
        let lla = pe::import_slot(&file, "kernel32.dll", "LoadLibraryA").ok_or("executable does not import LoadLibraryA")?;
        let gpa = pe::import_slot(&file, "kernel32.dll", "GetProcAddress").ok_or("executable does not import GetProcAddress")?;
        Some((info.image_base + lla, info.image_base + gpa))
    };

    std::env::set_var(pcore::LAUNCHER_ENV, "1");
    let mut cmd = pcore::wide(&command_line(&exe));
    let app = pcore::wide(exe.as_os_str());
    let cwd = pcore::wide(dir.as_os_str());
    let mut si: STARTUPINFOW = unsafe { core::mem::zeroed() };
    si.cb = core::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { core::mem::zeroed() };
    let flags = if imports.is_some() { CREATE_SUSPENDED } else { 0 };
    let ok = unsafe {
        CreateProcessW(app.as_ptr(), cmd.as_mut_ptr(), core::ptr::null(), core::ptr::null(), 0, flags,
            core::ptr::null(), cwd.as_ptr(), &si, &mut pi)
    };
    if ok == 0 {
        return Err(format!("{}: CreateProcess failed ({})", exe.display(), unsafe { GetLastError() }));
    }
    let result = match imports {
        Some((lla, gpa)) => unsafe { inject(pi.hProcess, info.image_base + info.entry_rva, lla, gpa, &list) },
        None => Ok(()),
    };
    unsafe {
        match &result {
            Ok(()) => {
                if flags != 0 {
                    ResumeThread(pi.hThread);
                }
            }
            Err(_) => {
                TerminateProcess(pi.hProcess, 1);
            }
        }
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    }
    result.map(|()| log::write(TAG, "game started"))
}

fn find_exe(dir: &Path, ini: &Ini) -> Result<PathBuf, String> {
    if let Some(name) = ini.get("loader", "exe").filter(|n| !n.is_empty()) {
        let p = dir.join(name);
        return if p.is_file() { Ok(p) } else { Err(format!("[loader] exe={name}: file not found")) };
    }
    EXE_NAMES
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("No game executable ({}) next to the launcher.", EXE_NAMES.join(", ")))
}

/// `"<exe>" <launcher arguments>` with Windows quoting.
fn command_line(exe: &Path) -> OsString {
    let mut s = OsString::new();
    s.push("\"");
    s.push(exe.as_os_str());
    s.push("\"");
    for arg in std::env::args_os().skip(1) {
        s.push(" ");
        let a = arg.to_string_lossy();
        if !a.is_empty() && !a.contains([' ', '\t', '"']) {
            s.push(&arg);
            continue;
        }
        let mut q = String::from("\"");
        let mut slashes = 0;
        for ch in a.chars() {
            match ch {
                '\\' => slashes += 1,
                '"' => {
                    q.extend(std::iter::repeat('\\').take(slashes * 2 + 1));
                    q.push('"');
                    slashes = 0;
                    continue;
                }
                _ => {}
            }
            if ch != '\\' {
                q.extend(std::iter::repeat('\\').take(slashes));
                slashes = 0;
                q.push(ch);
            }
        }
        q.extend(std::iter::repeat('\\').take(slashes * 2));
        q.push('"');
        s.push(q);
    }
    s
}

/// Entry stub in the suspended game process (see the module comment).
unsafe fn inject(process: HANDLE, entry: u32, lla: u32, gpa: u32, list: &[PathBuf]) -> Result<(), String> {
    let mut orig = [0u8; 5];
    if ReadProcessMemory(process, entry as *const c_void, orig.as_mut_ptr() as *mut c_void, 5, core::ptr::null_mut()) == 0 {
        return Err(format!("reading the entry point failed ({})", GetLastError()));
    }

    // data
    let mut data: Vec<u8> = Vec::new();
    let put = |d: &mut Vec<u8>, bytes: &[u8]| {
        let at = d.len() as u32;
        d.extend_from_slice(bytes);
        at
    };
    let s_k32 = put(&mut data, b"kernel32.dll\0");
    let s_vp = put(&mut data, b"VirtualProtect\0");
    let s_llw = put(&mut data, b"LoadLibraryW\0");
    let s_start = put(&mut data, b"start\0");
    while data.len() % 4 != 0 {
        data.push(0);
    }
    let old = put(&mut data, &[0; 4]);
    let old2 = put(&mut data, &[0; 4]);
    let paths: Vec<u32> = list
        .iter()
        .map(|p| {
            let w: Vec<u8> = pcore::wide(p.as_os_str()).iter().flat_map(|c| c.to_le_bytes()).collect();
            put(&mut data, &w)
        })
        .collect();
    while data.len() % 16 != 0 {
        data.push(0);
    }

    let size = data.len() + 160 + 32 * list.len();
    let base = VirtualAllocEx(process, core::ptr::null_mut(), size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as u32;
    if base == 0 {
        return Err(format!("VirtualAllocEx failed ({})", GetLastError()));
    }
    let a = |off: u32| (base + off).to_le_bytes();
    let code_at = base + data.len() as u32;

    // code
    let mut c: Vec<u8> = vec![0x60]; // pushad
    let call_mem = |c: &mut Vec<u8>, slot: u32| {
        c.extend_from_slice(&[0xff, 0x15]);
        c.extend_from_slice(&slot.to_le_bytes());
    };
    let push = |c: &mut Vec<u8>, v: [u8; 4]| {
        c.push(0x68);
        c.extend_from_slice(&v);
    };
    push(&mut c, a(s_k32));
    call_mem(&mut c, lla); // LoadLibraryA("kernel32.dll")
    c.extend_from_slice(&[0x89, 0xc3]); // mov ebx,eax
    push(&mut c, a(s_vp));
    c.push(0x53);
    call_mem(&mut c, gpa); // GetProcAddress(ebx, "VirtualProtect")
    c.extend_from_slice(&[0x89, 0xc6]); // mov esi,eax
    push(&mut c, a(old));
    c.extend_from_slice(&[0x6a, 0x40, 0x6a, 0x05]);
    push(&mut c, entry.to_le_bytes());
    c.extend_from_slice(&[0xff, 0xd6]); // VirtualProtect(entry, 5, RWX, &old)
    for (i, b) in orig.iter().enumerate() {
        c.extend_from_slice(&[0xc6, 0x05]);
        c.extend_from_slice(&(entry + i as u32).to_le_bytes());
        c.push(*b); // mov byte [entry+i], orig
    }
    push(&mut c, a(old2));
    c.extend_from_slice(&[0xff, 0x35]);
    c.extend_from_slice(&a(old)); // push dword [old]
    c.push(0x6a);
    c.push(0x05);
    push(&mut c, entry.to_le_bytes());
    c.extend_from_slice(&[0xff, 0xd6]); // VirtualProtect(entry, 5, old, &old2)
    push(&mut c, a(s_llw));
    c.push(0x53);
    call_mem(&mut c, gpa); // GetProcAddress(ebx, "LoadLibraryW")
    c.extend_from_slice(&[0x89, 0xc6]); // mov esi,eax
    for p in &paths {
        push(&mut c, a(*p));
        c.extend_from_slice(&[0xff, 0xd6, 0x85, 0xc0, 0x74, 18]); // LoadLibraryW(path); test eax,eax; jz next
        push(&mut c, a(s_start));
        c.push(0x50);
        call_mem(&mut c, gpa); // GetProcAddress(module, "start")
        c.extend_from_slice(&[0x85, 0xc0, 0x74, 0x02, 0xff, 0xd0]); // test eax,eax; jz next; call eax
    }
    c.push(0x61); // popad
    let at = code_at + c.len() as u32;
    c.extend_from_slice(&rel5(0xe9, at, entry)); // jmp entry
    if data.len() + c.len() > size {
        return Err("stub too large".into());
    }

    let mut block = data;
    block.extend_from_slice(&c);
    if WriteProcessMemory(process, base as *mut c_void, block.as_ptr() as *const c_void, block.len(), core::ptr::null_mut()) == 0 {
        return Err(format!("writing the stub failed ({})", GetLastError()));
    }
    let jump = rel5(0xe9, entry, code_at);
    let mut prot = 0u32;
    if VirtualProtectEx(process, entry as *mut c_void, 5, PAGE_EXECUTE_READWRITE, &mut prot) == 0 {
        return Err(format!("VirtualProtectEx failed ({})", GetLastError()));
    }
    let written = WriteProcessMemory(process, entry as *mut c_void, jump.as_ptr() as *const c_void, 5, core::ptr::null_mut());
    let mut tmp = 0u32;
    VirtualProtectEx(process, entry as *mut c_void, 5, prot, &mut tmp);
    if written == 0 {
        return Err(format!("writing the entry jump failed ({})", GetLastError()));
    }
    log::write(TAG, &format!("entry stub at {code_at:#x}, {} mod(s)", list.len()));
    Ok(())
}
