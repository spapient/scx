//! 只读内存映射（P3.1）：零依赖三平台抽象（Windows kernel32 / Unix libc 手写 extern）。
//!
//! 用途：postings.bin 大文件映射后直接解析，避免整文件二次拷贝进堆。
//! 失败/空文件时调用方回退 `fs::read`（本模块只负责映射，不决定 fallback）。
//! 只读、不写回；映射期间保留 File 句柄；Drop 顺序：先 unmap 再关句柄。

use std::fs::File;
use std::io;
use std::path::Path;

pub(crate) struct Mmap {
    ptr: *const u8,
    len: usize,
    #[cfg(windows)]
    map: *mut std::ffi::c_void,
    #[allow(dead_code)]
    _file: File,
}

// 映射生命周期内文件被外部截断会导致越界读。scx 单进程独占 index/ 目录：
// rebuild 先打开新文件再覆盖，旧映射在 drop 前保持有效，满足该约束。
unsafe impl Send for Mmap {}
unsafe impl Sync for Mmap {}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    const PAGE_READONLY: u32 = 0x02;
    const FILE_MAP_READ: u32 = 0x04;

    extern "system" {
        fn CreateFileMappingW(
            file: *mut c_void,
            sa: *mut c_void,
            protect: u32,
            hi: u32,
            lo: u32,
            name: *const u16,
        ) -> *mut c_void;
        fn MapViewOfFile(
            file: *mut c_void,
            access: u32,
            off_hi: u32,
            off_lo: u32,
            nbytes: usize,
        ) -> *mut c_void;
        fn UnmapViewOfFile(addr: *const c_void) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }

    pub(super) fn map(f: File, len: usize) -> io::Result<Mmap> {
        if len == 0 {
            return Err(io::Error::other("mmap: empty file"));
        }
        let h = f.as_raw_handle();
        let m = unsafe {
            CreateFileMappingW(
                h,
                std::ptr::null_mut(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            )
        };
        if m.is_null() {
            return Err(io::Error::last_os_error());
        }
        let p = unsafe { MapViewOfFile(m, FILE_MAP_READ, 0, 0, len) };
        if p.is_null() {
            unsafe { CloseHandle(m) };
            return Err(io::Error::last_os_error());
        }
        Ok(Mmap {
            ptr: p as *const u8,
            len,
            map: m,
            _file: f,
        })
    }

    pub(super) fn unmap(m: &mut Mmap) {
        unsafe {
            UnmapViewOfFile(m.ptr as *const _);
            CloseHandle(m.map);
        }
        m.ptr = std::ptr::null();
        m.len = 0;
        m.map = std::ptr::null_mut();
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::os::fd::AsRawFd;

    const PROT_READ: i32 = 0x1;
    const MAP_PRIVATE: i32 = 0x2;

    extern "C" {
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: i32,
            flags: i32,
            fd: i32,
            off: i64,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> i32;
    }

    pub(super) fn map(f: File, len: usize) -> io::Result<Mmap> {
        if len == 0 {
            return Err(io::Error::other("mmap: empty file"));
        }
        let fd = f.as_raw_fd();
        let p = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ, MAP_PRIVATE, fd, 0) };
        if p == (-1isize as *mut c_void) {
            return Err(io::Error::last_os_error());
        }
        Ok(Mmap {
            ptr: p as *const u8,
            len,
            _file: f,
        })
    }

    pub(super) fn unmap(m: &mut Mmap) {
        unsafe { munmap(m.ptr as *mut c_void, m.len) };
        m.ptr = std::ptr::null();
        m.len = 0;
    }
}

impl Mmap {
    /// 打开并映射一个文件（只读）。
    pub(crate) fn open(p: &Path) -> io::Result<Mmap> {
        let f = File::open(p)?;
        let len = f.metadata()?.len() as usize;
        imp::map(f, len)
    }

    /// 映射内存切片（Drop 前有效）。
    pub(crate) fn as_slice(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            imp::unmap(self);
        }
    }
}
