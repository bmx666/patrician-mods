//! Win32 calls: the heap of the decoded images, files, and the Intel JPEG Library `ijl11.dll` of the games.

use core::ffi::c_void;
use std::sync::OnceLock;

type Handle = *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn GetProcessHeap() -> Handle;
    fn HeapAlloc(heap: Handle, flags: u32, bytes: usize) -> *mut c_void;
    fn HeapFree(heap: Handle, flags: u32, mem: *mut c_void) -> i32;
    fn LoadLibraryA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn CreateFileA(
        name: *const u8,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn ReadFile(file: Handle, buffer: *mut u8, bytes: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
    fn WriteFile(file: Handle, buffer: *const u8, bytes: u32, written: *mut u32, overlapped: *mut c_void) -> i32;
    fn GetFileSize(file: Handle, high: *mut u32) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
}

const HEAP_ZERO_MEMORY: u32 = 8;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const CREATE_ALWAYS: u32 = 2;
const INVALID: Handle = usize::MAX as Handle;

/// `operator new` of the original: a zeroed block of `bytes` (a 32-bit address), 0 when it cannot be had.
pub fn alloc(bytes: u32) -> u32 {
    if bytes as i32 <= 0 && bytes != 0 {
        return 0;
    }
    unsafe { HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, bytes.max(1) as usize) as usize as u32 }
}

/// `operator delete`: frees a block of [`alloc`]; 0 is ignored.
pub fn free(block: u32) {
    if block != 0 {
        unsafe { HeapFree(GetProcessHeap(), 0, block as usize as *mut c_void) };
    }
}

/// Whole contents of a file named by an ANSI path, `None` when it cannot be read or is empty (the original maps the
/// file, which fails for an empty one).
pub fn read_file(name: *const u8) -> Option<Vec<u8>> {
    if name.is_null() {
        return None;
    }
    unsafe {
        let file = CreateFileA(name, GENERIC_READ, 1, core::ptr::null_mut(), OPEN_EXISTING, 0, core::ptr::null_mut());
        if file == INVALID {
            return None;
        }
        let size = GetFileSize(file, core::ptr::null_mut());
        let mut data = Vec::new();
        let mut ok = size != 0 && size != u32::MAX && data.try_reserve_exact(size as usize).is_ok();
        if ok {
            data.resize(size as usize, 0);
            let mut done = 0u32;
            while ok && done < size {
                let mut read = 0u32;
                ok = ReadFile(file, data.as_mut_ptr().add(done as usize), size - done, &mut read, core::ptr::null_mut())
                    != 0
                    && read != 0;
                done += read;
            }
        }
        CloseHandle(file);
        ok.then_some(data)
    }
}

/// A file created (or truncated) for writing, as `_open(name, _O_WRONLY | _O_CREAT | _O_TRUNC | _O_BINARY)`.
pub struct OutFile(Handle);

impl OutFile {
    pub fn create(name: *const u8) -> Option<OutFile> {
        if name.is_null() {
            return None;
        }
        let file =
            unsafe { CreateFileA(name, GENERIC_WRITE, 3, core::ptr::null_mut(), CREATE_ALWAYS, 0x80, core::ptr::null_mut()) };
        (file != INVALID).then_some(OutFile(file))
    }

    /// Writes `bytes` bytes from a 32-bit address.
    pub fn write_at(&mut self, data: u32, bytes: i32) {
        if bytes > 0 && data != 0 {
            let mut written = 0u32;
            unsafe { WriteFile(self.0, data as usize as *const u8, bytes as u32, &mut written, core::ptr::null_mut()) };
        }
    }

    pub fn write(&mut self, data: &[u8]) {
        self.write_at(data.as_ptr() as usize as u32, data.len() as i32);
    }
}

impl Drop for OutFile {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Entry points of `ijl11.dll` (Intel JPEG Library 1.1, `__stdcall`), imported by ordinal as the original does.
pub struct Ijl {
    pub init: unsafe extern "system" fn(*mut u32) -> i32,
    pub free: unsafe extern "system" fn(*mut u32) -> i32,
    pub read: unsafe extern "system" fn(*mut u32, i32) -> i32,
    pub write: unsafe extern "system" fn(*mut u32, i32) -> i32,
}

/// `ijl11.dll` from the game folder, loaded on first use; `None` when it is missing.
pub fn ijl() -> Option<&'static Ijl> {
    static IJL: OnceLock<Option<Ijl>> = OnceLock::new();
    IJL.get_or_init(|| unsafe {
        let module = LoadLibraryA(b"ijl11.dll\0".as_ptr());
        if module.is_null() {
            return None;
        }
        let get = |ordinal: usize| GetProcAddress(module, ordinal as *const u8);
        let (init, free, read, write) = (get(2), get(3), get(4), get(5));
        if init.is_null() || free.is_null() || read.is_null() || write.is_null() {
            return None;
        }
        Some(Ijl {
            init: core::mem::transmute::<*mut c_void, unsafe extern "system" fn(*mut u32) -> i32>(init),
            free: core::mem::transmute::<*mut c_void, unsafe extern "system" fn(*mut u32) -> i32>(free),
            read: core::mem::transmute::<*mut c_void, unsafe extern "system" fn(*mut u32, i32) -> i32>(read),
            write: core::mem::transmute::<*mut c_void, unsafe extern "system" fn(*mut u32, i32) -> i32>(write),
        })
    })
    .as_ref()
}

/// `JPEG_CORE_PROPERTIES` of IJL 1.1 as dwords (0x4e40 bytes, as the original reserves), with the field offsets used.
pub struct JpegProps(pub Vec<u32>);

impl JpegProps {
    pub const DIB_BYTES: usize = 0x04 / 4;
    pub const DIB_WIDTH: usize = 0x08 / 4;
    pub const DIB_HEIGHT: usize = 0x0c / 4;
    pub const DIB_PAD: usize = 0x10 / 4;
    pub const DIB_CHANNELS: usize = 0x14 / 4;
    pub const DIB_COLOR: usize = 0x18 / 4;
    pub const JPG_FILE: usize = 0x20 / 4;
    pub const JPG_BYTES: usize = 0x24 / 4;
    pub const JPG_SIZE: usize = 0x28 / 4;
    pub const JPG_WIDTH: usize = 0x2c / 4;
    pub const JPG_HEIGHT: usize = 0x30 / 4;
    pub const JPG_CHANNELS: usize = 0x34 / 4;
    pub const JPG_COLOR: usize = 0x38 / 4;
    pub const JPG_SUBSAMPLING: usize = 0x3c / 4;
    pub const QUALITY: usize = 0x50 / 4;

    pub fn new() -> JpegProps {
        JpegProps(vec![0; 0x1390])
    }

    pub fn ptr(&mut self) -> *mut u32 {
        self.0.as_mut_ptr()
    }
}
