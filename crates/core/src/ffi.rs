//! Minimal Windows API declarations (no external crates).
#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]

use core::ffi::c_void;

pub type HANDLE = *mut c_void;
pub type HMODULE = *mut c_void;
pub type BOOL = i32;
pub type FARPROC = *const c_void;

pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const CREATE_SUSPENDED: u32 = 0x4;
pub const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
pub const DLL_PROCESS_ATTACH: u32 = 1;
pub const MB_ICONERROR: u32 = 0x10;

#[repr(C)]
pub struct STARTUPINFOW {
    pub cb: u32,
    pub lpReserved: *mut u16,
    pub lpDesktop: *mut u16,
    pub lpTitle: *mut u16,
    pub dwX: u32,
    pub dwY: u32,
    pub dwXSize: u32,
    pub dwYSize: u32,
    pub dwXCountChars: u32,
    pub dwYCountChars: u32,
    pub dwFillAttribute: u32,
    pub dwFlags: u32,
    pub wShowWindow: u16,
    pub cbReserved2: u16,
    pub lpReserved2: *mut u8,
    pub hStdInput: HANDLE,
    pub hStdOutput: HANDLE,
    pub hStdError: HANDLE,
}

#[repr(C)]
pub struct PROCESS_INFORMATION {
    pub hProcess: HANDLE,
    pub hThread: HANDLE,
    pub dwProcessId: u32,
    pub dwThreadId: u32,
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> HMODULE;
    pub fn GetModuleFileNameA(module: HMODULE, buf: *mut u8, size: u32) -> u32;
    pub fn LoadLibraryW(name: *const u16) -> HMODULE;
    pub fn GetProcAddress(module: HMODULE, name: *const u8) -> FARPROC;
    pub fn DisableThreadLibraryCalls(module: HMODULE) -> BOOL;
    pub fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    pub fn VirtualProtect(addr: *mut c_void, size: usize, prot: u32, old: *mut u32) -> BOOL;
    pub fn VirtualAlloc(addr: *mut c_void, size: usize, ty: u32, prot: u32) -> *mut c_void;
    pub fn FlushInstructionCache(process: HANDLE, addr: *const c_void, size: usize) -> BOOL;
    pub fn GetCurrentProcess() -> HANDLE;
    pub fn GetFileAttributesA(name: *const u8) -> u32;
    pub fn WritePrivateProfileStringW(section: *const u16, key: *const u16, value: *const u16, file: *const u16) -> BOOL;
    pub fn GetTickCount() -> u32;
    pub fn GetLastError() -> u32;
    pub fn CreateProcessW(
        app: *const u16,
        cmd: *mut u16,
        process_attrs: *const c_void,
        thread_attrs: *const c_void,
        inherit: BOOL,
        flags: u32,
        env: *const c_void,
        dir: *const u16,
        startup: *const STARTUPINFOW,
        info: *mut PROCESS_INFORMATION,
    ) -> BOOL;
    pub fn VirtualAllocEx(process: HANDLE, addr: *mut c_void, size: usize, ty: u32, prot: u32) -> *mut c_void;
    pub fn VirtualProtectEx(process: HANDLE, addr: *mut c_void, size: usize, prot: u32, old: *mut u32) -> BOOL;
    pub fn WriteProcessMemory(process: HANDLE, addr: *mut c_void, buf: *const c_void, size: usize, done: *mut usize) -> BOOL;
    pub fn ReadProcessMemory(process: HANDLE, addr: *const c_void, buf: *mut c_void, size: usize, done: *mut usize) -> BOOL;
    pub fn ResumeThread(thread: HANDLE) -> u32;
    pub fn TerminateProcess(process: HANDLE, code: u32) -> BOOL;
    pub fn CloseHandle(handle: HANDLE) -> BOOL;
}

#[link(name = "user32")]
extern "system" {
    pub fn MessageBoxW(window: *mut c_void, text: *const u16, caption: *const u16, ty: u32) -> i32;
}

/// Error message box (UTF-16 conversion included).
pub fn message_box(caption: &str, text: &str) {
    let t = crate::wide(text.as_ref());
    let c = crate::wide(caption.as_ref());
    unsafe { MessageBoxW(core::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_ICONERROR) };
}
