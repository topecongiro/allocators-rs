// Copyright 2017 the authors. See the 'Copyright and license' section of the
// README.md file at the top-level directory of this repository.
//
// Licensed under the Apache License, Version 2.0 (the LICENSE file). This file
// may not be copied, modified, or distributed except according to those terms.

// TODO:
// - Figure out how to panic without allocating
// - Support all Unices, not just Linux and Mac
// - Add tests for UntypedObjectAlloc impls

#![cfg_attr(any(not(test), feature = "test-no-std"), no_std)]
#![cfg_attr(all(test, not(feature = "test-no-std")), feature(test))]
#![feature(alloc, allocator_api)]

#[cfg(all(test, not(feature = "test-no-std")))]
extern crate core;

extern crate alloc;
extern crate libc;
extern crate object_alloc;
extern crate sysconf;

#[cfg(any(target_os = "linux", target_os = "macos"))]
extern crate errno;

#[cfg(windows)]
extern crate kernel32;
#[cfg(windows)]
extern crate winapi;

use self::alloc::allocator::{Alloc, Layout, Excess, AllocErr};
use self::object_alloc::{Exhausted, UntypedObjectAlloc};
use core::ptr;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use errno::errno;

/// A builder for `MapAlloc`.
///
/// `MapAllocBuilder` represents the configuration of a `MapAlloc`. New `MapAllocBuilder`s are
/// constructed using `default`, and then various other methods are used to set various
/// configuration options.
///
/// # Memory Permissions
///
/// One aspect that can be configured is the permissions of allocated memory - readable, writable,
/// or executable. By default, memory is readable and writable but not executable. Note that not
/// all combinations of permissions are supported on all platforms, and if a particular combination
/// is not supported, then another combination that is no more restrictive than the requested
/// combination will be used. For example, if execute-only permission is requested, but that is not
/// supported, then read/execute, write/execute, or read/write/execute permissions may be used. The
/// only guarantee that is made is that if the requested combination is supported on the runtime
/// platform, then precisely that configuration will be used.
///
/// Here are the known limitations with respect to permissions. This list is not guaranteed to be
/// exhaustive:
///
/// - Unix: On some platforms, write permission may imply read permission (so write and
///   write/execute are not supported), and read permission may imply execute permission (so read
///   and read/write are not supported).
/// - Windows:
///   - Write permission is not supported; it is implemented as read/write.
///   - Write/execute permission is not supported; it is implemented as read/write/execute.
pub struct MapAllocBuilder {
    read: bool,
    write: bool,
    exec: bool,
    pagesize: usize,
    huge_pagesize: Option<usize>,
    obj_size: Option<usize>,
}

impl MapAllocBuilder {
    pub fn build(&self) -> MapAlloc {
        #[cfg(target_os = "linux")]
        {
            if let Some(huge) = self.huge_pagesize {
                assert!(sysconf::page::hugepage_supported(huge),
                        "unsupported hugepage size: {}",
                        huge);
            }
        }

        let obj_size = if let Some(obj_size) = self.obj_size {
            assert_eq!(obj_size % self.pagesize,
                       0,
                       "object size ({}) is not a multiple of the page size ({})",
                       obj_size,
                       self.pagesize);
            obj_size
        } else {
            self.pagesize
        };
        MapAlloc {
            pagesize: self.pagesize,
            huge_pagesize: self.huge_pagesize,
            perms: perms::get_perm(self.read, self.write, self.exec),
            obj_size: obj_size,
        }
    }

    #[cfg(target_os = "linux")]
    pub fn default_huge_pagesize(mut self) -> MapAllocBuilder {
        let pagesize = sysconf::page::default_hugepage().expect("huge pages not supported");
        self.pagesize = pagesize;
        self.huge_pagesize = Some(pagesize);
        self
    }

    pub fn huge_pagesize(mut self, pagesize: usize) -> MapAllocBuilder {
        self.huge_pagesize = Some(pagesize);
        self
    }

    /// Enables read permission for allocated memory.
    ///
    /// `read` makes it so that allocated memory will be readable. The default is readable.
    ///
    /// See the "Memory Permissions" section of the `MapAllocBuilder` documentation for more
    /// details.
    pub fn read(mut self) -> MapAllocBuilder {
        self.read = true;
        self
    }

    /// Enables write permission for allocated memory.
    ///
    /// `write` makes it so that allocated memory will be writable. The default is writable.
    ///
    /// See the "Memory Permissions" section of the `MapAllocBuilder` documentation for more
    /// details.
    pub fn write(mut self) -> MapAllocBuilder {
        self.write = true;
        self
    }

    /// Enables execution permission for allocated memory.
    ///
    /// `exec` makes it so that allocated memory will be executable. The default is non-executable.
    ///
    /// See the "Memory Permissions" section of the `MapAllocBuilder` documentation for more
    /// details.
    pub fn exec(mut self) -> MapAllocBuilder {
        self.exec = true;
        self
    }

    /// Disables read permission for allocated memory.
    ///
    /// `no_read` makes it so that allocated memory will not be readable. The default is readable.
    ///
    /// See the "Memory Permissions" section of the `MapAllocBuilder` documentation for more
    /// details.
    pub fn no_read(mut self) -> MapAllocBuilder {
        self.read = false;
        self
    }

    /// Disables write permission for allocated memory.
    ///
    /// `no_write` makes it so that allocated memory will not be writable. The default is writable.
    ///
    /// See the "Memory Permissions" section of the `MapAllocBuilder` documentation for more
    /// details.
    pub fn no_write(mut self) -> MapAllocBuilder {
        self.write = false;
        self
    }

    /// Disables execution permission for allocated memory.
    ///
    /// `no_exec` makes it so that allocated memory will not be executable. The default is
    /// non-executable.
    ///
    /// See the "Memory Permissions" section of the `MapAllocBuilder` documentation for more
    /// details.
    pub fn no_exec(mut self) -> MapAllocBuilder {
        self.exec = false;
        self
    }

    /// Sets the object size for the `UntypedObjectAlloc` implementation.
    ///
    /// `MapAlloc` implements `UntypedObjectAlloc`. `obj_size` sets the object size that will be
    /// used by that implementation. It defaults to whatever page size is configured for the
    /// allocator.
    pub fn obj_size(mut self, obj_size: usize) -> MapAllocBuilder {
        self.obj_size = Some(obj_size);
        self
    }
}

impl Default for MapAllocBuilder {
    fn default() -> MapAllocBuilder {
        MapAllocBuilder {
            read: true,
            write: true,
            exec: false,
            pagesize: sysconf::page::pagesize(),
            huge_pagesize: None,
            obj_size: None,
        }
    }
}

pub struct MapAlloc {
    pagesize: usize,
    huge_pagesize: Option<usize>,
    perms: perms::Perm,
    obj_size: usize,
}

impl Default for MapAlloc {
    fn default() -> MapAlloc {
        MapAllocBuilder::default().build()
    }
}

impl MapAlloc {
    // alloc_helper performs the requested allocation, properly handling the case in which mmap
    // returns null.
    fn alloc_helper(&self, size: usize) -> Option<*mut u8> {
        // Since allocators in Rust are not allowed to return null pointers, but it is valid for
        // mmap to return memory starting at null, we have to handle that case. We do this by
        // checking for null, and if we find that mmap has returned null, we unmap all but the
        // first page and try again. Since we leave the first page (the one starting at address 0)
        // mapped, future calls to mmap are guaranteed to not return null. Note that this leaks
        // memory since we never unmap that page, but this isn't a big deal - even if the page is a
        // huge page, since we never write to it, it will remain uncommitted and will thus not
        // consume any physical memory.
        let f = |ptr: *mut u8| if ptr.is_null() {
            let unmap_size = size - self.pagesize;
            if unmap_size > 0 {
                munmap(self.pagesize as *mut u8, unmap_size);
            }
            // a) Make it more likely that the kernel will not keep the page backed by physical
            // memory and, b) make it so that an access to that range will result in a segfault to
            // make other bugs easier to detect.
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            mark_unused(ptr::null_mut(), self.pagesize);
            self.alloc_helper(size)
        } else {
            Some(ptr)
        };
        mmap(size, self.perms, self.huge_pagesize).and_then(f)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn commit(&self, ptr: *mut u8, layout: Layout) {
        // TODO: What to do about sizes that are not multiples of the page size? These are legal
        // allocations, and so they are legal to pass to uncommit.
        let step = if let Some(huge) = self.huge_pagesize {
            debug_assert_eq!(ptr as usize % huge,
                             0,
                             "ptr {:?} not aligned to huge page size {}",
                             ptr,
                             huge);
            debug_assert!(layout.align() <= huge);
            huge
        } else {
            debug_assert_eq!(ptr as usize % self.pagesize,
                             0,
                             "ptr {:?} not aligned to page size {}",
                             ptr,
                             self.pagesize);
            debug_assert!(layout.align() <= self.pagesize);
            self.pagesize
        };
        // TODO: More elegant way to do this?
        // TODO: If the size isn't a multiple of the page size, this math might be wrong.
        let steps = layout.size() / step;
        for i in 0..steps {
            // TODO: How to make this read not optimized out?
            unsafe { ptr::read(((ptr as usize) + (i * step)) as *mut u8) };
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn uncommit(&self, ptr: *mut u8, layout: Layout) {
        // TODO: What to do about sizes that are not multiples of the page size? These are legal
        // allocations, and so they are legal to pass to uncommit, but will madvise handle them
        // properly?
        if let Some(huge) = self.huge_pagesize {
            debug_assert_eq!(ptr as usize % huge,
                             0,
                             "ptr {:?} not aligned to huge page size {}",
                             ptr,
                             huge);
            debug_assert!(layout.align() <= huge);
        } else {
            debug_assert_eq!(ptr as usize % self.pagesize,
                             0,
                             "ptr {:?} not aligned to page size {}",
                             ptr,
                             self.pagesize);
            debug_assert!(layout.align() <= self.pagesize);
        }
        uncommit(ptr, layout.size());
    }
}

unsafe impl<'a> Alloc for &'a MapAlloc {
    unsafe fn alloc(&mut self, layout: Layout) -> Result<*mut u8, AllocErr> {
        self.alloc_excess(layout).map(|Excess(ptr, _)| ptr)
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        munmap(ptr, layout.size());
    }

    unsafe fn alloc_zeroed(&mut self, layout: Layout) -> Result<*mut u8, AllocErr> {
        <&'a MapAlloc as Alloc>::alloc(self, layout)
    }

    unsafe fn alloc_excess(&mut self, layout: Layout) -> Result<Excess, AllocErr> {
        // alignment less than a page is fine because page-aligned objects are also aligned to
        // any alignment less than a page
        if layout.align() > self.pagesize {
            return Err(AllocErr::invalid_input("cannot support alignment greater than a page"));
        }

        let size = next_multiple(layout.size(), self.pagesize);
        match self.alloc_helper(size) {
            Some(ptr) => Ok(Excess(ptr, size)),
            None => Err(AllocErr::Exhausted { request: layout }),
        }
    }
}

unsafe impl<'a> UntypedObjectAlloc for &'a MapAlloc {
    fn layout(&self) -> Layout {
        if cfg!(debug_assertions) {
            Layout::from_size_align(self.obj_size, self.pagesize).unwrap()
        } else {
            unsafe { Layout::from_size_align_unchecked(self.obj_size, self.pagesize) }
        }
    }

    unsafe fn alloc(&mut self) -> Result<*mut u8, Exhausted> {
        // TODO: There's probably a method that does this more cleanly.
        match self.alloc_excess(self.layout()) {
            Ok(Excess(ptr, _)) => Ok(ptr),
            Err(AllocErr::Exhausted { .. }) => Err(Exhausted),
            Err(AllocErr::Unsupported { .. }) => unreachable!(),
        }
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8) {
        munmap(ptr, self.obj_size);
    }
}

unsafe impl Alloc for MapAlloc {
    unsafe fn alloc(&mut self, layout: Layout) -> Result<*mut u8, AllocErr> {
        <&MapAlloc as Alloc>::alloc(&mut (&*self), layout)
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        <&MapAlloc as Alloc>::dealloc(&mut (&*self), ptr, layout)
    }

    unsafe fn alloc_zeroed(&mut self, layout: Layout) -> Result<*mut u8, AllocErr> {
        <&MapAlloc as Alloc>::alloc_zeroed(&mut (&*self), layout)
    }

    unsafe fn alloc_excess(&mut self, layout: Layout) -> Result<Excess, AllocErr> {
        <&MapAlloc as Alloc>::alloc_excess(&mut (&*self), layout)
    }
}

unsafe impl UntypedObjectAlloc for MapAlloc {
    fn layout(&self) -> Layout {
        <&MapAlloc as UntypedObjectAlloc>::layout(&(&*self))
    }

    unsafe fn alloc(&mut self) -> Result<*mut u8, Exhausted> {
        <&MapAlloc as UntypedObjectAlloc>::alloc(&mut (&*self))
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8) {
        <&MapAlloc as UntypedObjectAlloc>::dealloc(&mut (&*self), ptr);
    }
}

fn next_multiple(size: usize, unit: usize) -> usize {
    if size % unit == 0 {
        size
    } else {
        size + (size - (size % unit))
    }
}

#[cfg(target_os = "linux")]
fn mark_unused(ptr: *mut u8, size: usize) {
    use libc::{c_void, MADV_DONTNEED, PROT_NONE};
    unsafe {
        // Let the kernel know we don't need this memory, so it can free physical resources for it
        libc::madvise(ptr as *mut c_void, size, MADV_DONTNEED);
        // Make it so that accesses to this memory result in a segfault
        libc::mprotect(ptr as *mut c_void, size, PROT_NONE);
    }
}

#[cfg(target_os = "macos")]
fn mark_unused(ptr: *mut u8, size: usize) {
    use libc::{c_void, MADV_FREE, PROT_NONE};
    unsafe {
        // Let the kernel know we don't need this memory, so it can free physical resources for it
        libc::madvise(ptr as *mut c_void, size, MADV_FREE);
        // Make it so that accesses to this memory result in a segfault
        libc::mprotect(ptr as *mut c_void, size, PROT_NONE);
    }
}

#[cfg(target_os = "linux")]
fn mmap(size: usize, perms: i32, huge_pagesize: Option<usize>) -> Option<*mut u8> {
    use libc::{MAP_ANONYMOUS, MAP_PRIVATE, MAP_HUGETLB, MAP_FAILED, ENOMEM};

    // TODO: Figure out when it's safe to pass MAP_UNINITIALIZED (it's not defined in all
    // versions of libc). Be careful about not invalidating alloc_zeroed.

    // MAP_HUGE_SHIFT isn't used on all kernel versions, but I assume it must be
    // backwards-compatible. The only way for it to not be backwards-compatible would be for
    // there to be bits in the range [26, 31] (in the 'flags' argument) that used to be
    // meaningful. That would make old programs fail on newer kernels. In theory, old kernels
    // could be checking to make sure that undefined flags aren't set, but that seems unlikely.
    // See:
    // http://elixir.free-electrons.com/linux/latest/source/arch/alpha/include/uapi/asm/mman.h
    // http://man7.org/linux/man-pages/man2/mmap.2.html
    const MAP_HUGE_SHIFT: usize = 26;
    let flags = if let Some(pagesize) = huge_pagesize {
        debug_assert!(pagesize.is_power_of_two()); // implies pagesize > 0
        let log = pagesize.trailing_zeros();
        MAP_HUGETLB | ((log as i32) << MAP_HUGE_SHIFT)
    } else {
        0
    };

    let ptr = unsafe {
        libc::mmap(ptr::null_mut(),
                   size,
                   perms,
                   MAP_ANONYMOUS | MAP_PRIVATE | flags,
                   -1,
                   0)
    };

    if ptr == MAP_FAILED {
        if errno().0 == ENOMEM {
            None
        } else {
            panic!("mmap failed: {}", errno())
        }
    } else {
        Some(ptr as *mut u8)
    }
}

#[cfg(target_os = "macos")]
fn mmap(size: usize, perms: i32, huge_pagesize: Option<usize>) -> Option<*mut u8> {
    use libc::{MAP_ANON, MAP_PRIVATE, MAP_FAILED, ENOMEM};

    // TODO: Support superpages (see MAP_ANON description in mmap manpage)
    debug_assert!(huge_pagesize.is_none());

    let ptr = unsafe { libc::mmap(ptr::null_mut(), size, perms, MAP_ANON | MAP_PRIVATE, -1, 0) };

    if ptr == MAP_FAILED {
        if errno().0 == ENOMEM {
            None
        } else {
            panic!("mmap failed: {}", errno())
        }
    } else {
        Some(ptr as *mut u8)
    }
}

// For a good overview of virtual memory handling on Windows, see
// https://blogs.technet.microsoft.com/markrussinovich/2008/11/17/pushing-the-limits-of-windows-virtual-memory/

#[cfg(windows)]
fn mmap(size: usize, perms: u32, huge_pagesize: Option<usize>) -> Option<*mut u8> {
    use kernel32::VirtualAlloc;
    use winapi::winnt::{MEM_RESERVE, MEM_COMMIT, MEM_LARGE_PAGES};

    let typ = if huge_pagesize.is_none() {
        MEM_RESERVE | MEM_COMMIT
    } else {
        MEM_RESERVE | MEM_COMMIT | MEM_LARGE_PAGES
    };

    unsafe {
        // NOTE: While Windows makes a distinction between allocation granularity and page size
        // (see https://msdn.microsoft.com/en-us/library/windows/desktop/ms724958(v=vs.85).aspx),
        // VirtualAlloc only cares about allocation granularity for the pointer argument, not the
        // size. Since we're passing null for the pointer, this doesn't affect us.
        let ptr = VirtualAlloc(ptr::null_mut(), size as u64, typ, perms) as *mut u8;
        // NOTE: Windows can return many different error codes in different scenarios that all
        // relate to being out of memory. Instead of trying to list them all, we assume that any
        // error is an out-of-memory condition. This is fine so long as our code doesn't have a bug
        // (that would, e.g., result in VirtualAlloc being called with invalid arguments). This
        // isn't ideal, but during debugging, error codes can be printed here, so it's not the end
        // of the world.
        if ptr.is_null() { None } else { Some(ptr) }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn munmap(ptr: *mut u8, size: usize) {
    use libc::{munmap, c_void};
    unsafe {
        // NOTE: Don't inline the call to munmap; then errno might be called before munmap.
        let ret = munmap(ptr as *mut c_void, size);
        assert_eq!(ret, 0, "munmap failed: {}", errno());
    }
}

#[cfg(windows)]
fn munmap(ptr: *mut u8, _size: usize) {
    use kernel32::{VirtualFree, GetLastError};
    use winapi::winnt::MEM_RELEASE;

    unsafe {
        // NOTE: VirtualFree, when unmapping memory (as opposed to decommitting it), can only
        // operate on an entire region previously mapped with VirtualAlloc. As a result, 'ptr' must
        // have been previously returned by VirtualAlloc, and no length is needed since it is known
        // by the kernel (VirtualFree /requires/ that if the third argument is MEM_RELEASE, the
        // second is 0).
        let ret = VirtualFree(ptr as *mut winapi::c_void, 0, MEM_RELEASE);
        if ret == 0 {
            panic!("Call to VirtualFree failed with error code {}.",
                   GetLastError());
        }
    }
}

#[cfg(target_os = "linux")]
fn uncommit(ptr: *mut u8, size: usize) {
    use libc::{c_void, MADV_DONTNEED};
    unsafe {
        // TODO: Other options such as MADV_FREE are available on newer versions of Linux. Is there
        // a way that we can use those when available? Is that even desirable?
        libc::madvise(ptr as *mut c_void, size, MADV_DONTNEED);
    }
}

#[cfg(target_os = "macos")]
fn uncommit(ptr: *mut u8, size: usize) {
    use libc::{c_void, MADV_FREE};
    unsafe {
        libc::madvise(ptr as *mut c_void, size, MADV_FREE);
    }
}

mod perms {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub use self::unix::*;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub type Perm = i32;
    #[cfg(windows)]
    pub use self::windows::*;
    #[cfg(windows)]
    pub type Perm = u32;

    pub fn get_perm(read: bool, write: bool, exec: bool) -> Perm {
        match (read, write, exec) {
            (false, false, false) => PROT_NONE,
            (true, false, false) => PROT_READ,
            (false, true, false) => PROT_WRITE,
            (false, false, true) => PROT_EXEC,
            (true, true, false) => PROT_READ_WRITE,
            (true, false, true) => PROT_READ_EXEC,
            (false, true, true) => PROT_WRITE_EXEC,
            (true, true, true) => PROT_READ_WRITE_EXEC,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    mod unix {
        // NOTE: On some platforms, libc::PROT_WRITE may imply libc::PROT_READ, and libc::PROT_READ
        // may imply libc::PROT_EXEC.
        extern crate libc;
        pub const PROT_NONE: i32 = libc::PROT_NONE;
        pub const PROT_READ: i32 = libc::PROT_READ;
        pub const PROT_WRITE: i32 = libc::PROT_WRITE;
        pub const PROT_EXEC: i32 = libc::PROT_EXEC;
        pub const PROT_READ_WRITE: i32 = libc::PROT_READ | libc::PROT_WRITE;
        pub const PROT_READ_EXEC: i32 = libc::PROT_READ | libc::PROT_EXEC;
        pub const PROT_WRITE_EXEC: i32 = libc::PROT_WRITE | libc::PROT_EXEC;
        pub const PROT_READ_WRITE_EXEC: i32 = libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC;
    }

    #[cfg(windows)]
    mod windows {
        extern crate winapi;
        use self::winapi::winnt;
        pub const PROT_NONE: u32 = winnt::PAGE_NOACCESS;
        pub const PROT_READ: u32 = winnt::PAGE_READONLY;
        // windows doesn't have a write-only permission, so write implies read
        pub const PROT_WRITE: u32 = winnt::PAGE_READWRITE;
        pub const PROT_EXEC: u32 = winnt::PAGE_EXECUTE;
        pub const PROT_READ_WRITE: u32 = winnt::PAGE_READWRITE;
        pub const PROT_READ_EXEC: u32 = winnt::PAGE_EXECUTE_READ;
        // windows doesn't have a write/exec permission, so write/exec implies read/write/exec
        pub const PROT_WRITE_EXEC: u32 = winnt::PAGE_EXECUTE_READWRITE;
        pub const PROT_READ_WRITE_EXEC: u32 = winnt::PAGE_EXECUTE_READWRITE;
    }
}

#[cfg(test)]
mod tests {
    extern crate sysconf;
    use sysconf::page::pagesize;
    use super::*;
    use super::perms::*;


    #[cfg(not(feature = "test-no-std"))]
    extern crate test;

    // allow(unused) because these imports aren't used on windows
    #[allow(unused)]
    #[cfg(not(feature = "test-no-std"))]
    use std::time::{Instant, Duration};
    #[allow(unused)]
    #[cfg(not(feature = "test-no-std"))]
    use self::test::Bencher;

    // NOTE: Technically mmap is allowed to return 0, but (according to our empirical experience)
    // it only does this for very large map sizes (on Linux, at least 2^30 bytes). We never request
    // maps that large, so it's OK to check for null here. Even if it spuriously fails in the
    // future, it will queue us into the fact that our assumptions about when mmap returns null are
    // wrong.
    fn test_valid_map_address(ptr: *mut u8) {
        assert!(ptr as usize > 0, "ptr: {:?}", ptr);
        assert!(ptr as usize % pagesize() == 0, "ptr: {:?}", ptr);
    }

    // Test that the given range is readable and initialized to zero.
    fn test_zero_filled(ptr: *mut u8, size: usize) {
        for i in 0..size {
            unsafe {
                assert_eq!(*ptr.offset(i as isize), 0);
            }
        }
    }

    // Test that the given range is writable.
    fn test_write(ptr: *mut u8, size: usize) {
        for i in 0..size {
            unsafe {
                *ptr.offset(i as isize) = 1;
            }
        }
    }

    // Test that the given range is readable and writable, and that writes can be read back.
    fn test_write_read(ptr: *mut u8, size: usize) {
        for i in 0..size {
            unsafe {
                *ptr.offset(i as isize) = 1;
            }
        }
        for i in 0..size {
            unsafe {
                assert_eq!(*ptr.offset(i as isize), 1);
            }
        }
    }

    #[test]
    fn test_map() {
        // Check that:
        // - Mapping a single page works
        // - The returned pointer is non-null
        // - The returned pointer is page-aligned
        // - The page is zero-filled
        // - Unmapping it after it's already been unmapped is OK (except on windows).
        let mut ptr = mmap(pagesize(), PROT_READ_WRITE, None).unwrap();
        test_valid_map_address(ptr);
        test_zero_filled(ptr, pagesize());
        munmap(ptr, pagesize());
        #[cfg(not(windows))]
        munmap(ptr, pagesize());

        // Check that:
        // - Mapping multiple pages work
        // - The returned pointer is non-null
        // - The returned pointer is page-aligned
        // - The pages are zero-filled
        // - Unmapping it after it's already been unmapped is OK (except on windows).
        ptr = mmap(16 * pagesize(), PROT_READ_WRITE, None).unwrap();
        test_valid_map_address(ptr);
        test_zero_filled(ptr, 16 * pagesize());
        munmap(ptr, 16 * pagesize());
        #[cfg(not(windows))]
        munmap(ptr, 16 * pagesize());
    }

    #[cfg(not(windows))]
    #[test]
    fn test_map_non_windows() {
        // Check that:
        // - Unmapping a subset of a previously-mapped region works
        // - The remaining pages are still mapped
        let mut ptr = mmap(5 * pagesize(), PROT_READ_WRITE, None).unwrap();
        test_valid_map_address(ptr);
        test_zero_filled(ptr, 5 * pagesize());
        munmap(ptr, pagesize());
        munmap(unsafe { ptr.offset(2 * pagesize() as isize) }, pagesize());
        munmap(unsafe { ptr.offset(4 * pagesize() as isize) }, pagesize());
        test_zero_filled(unsafe { ptr.offset(1 * pagesize() as isize) }, pagesize());
        test_zero_filled(unsafe { ptr.offset(3 * pagesize() as isize) }, pagesize());

        // Check that:
        // - Mapping a vast region of memory works and is fast
        // - The returned pointer is non-null
        // - The returned pointer is page-aligned
        // - A read in the middle of mapping succeds and is zero

        // NOTE: Pick 2^29 bytes because, on Linux, 2^30 causes mmap to return null, which breaks
        // test_valid_map_address.
        let size = 1 << 29;
        #[cfg(not(feature = "test-no-std"))]
        let t0 = Instant::now();
        ptr = mmap(size, PROT_READ_WRITE, None).unwrap();
        #[cfg(not(feature = "test-no-std"))]
        {
            // In tests on a 2016 MacBook Pro (see bench_large_mmap), a 2^31 byte map/unmap pair
            // took ~5 usec natively (Mac OS X) and ~350 ns in a Linux VM. Thus, 1 ms is a safe
            // upper bound.
            let diff = Instant::now().duration_since(t0);
            let target = Duration::from_millis(1);
            assert!(diff < target, "duration: {:?}", diff);
        }
        test_valid_map_address(ptr);
        test_zero_filled(unsafe { ptr.offset((size / 2) as isize) }, pagesize());
        munmap(ptr, size);
    }

    #[test]
    fn test_perms() {
        // TODO: Add tests for executable permissions

        // Check that:
        // - Mapping a single read-only page works
        // - The returned pointer is non-null
        // - The returned pointer is page-aligned
        // - We can read the page, and it is zero-filled
        let mut ptr = mmap(pagesize(), PROT_READ, None).unwrap();
        test_valid_map_address(ptr);
        test_zero_filled(ptr, pagesize());
        munmap(ptr, pagesize());

        // Check that:
        // - Mapping a single write-only page works
        // - The returned pointer is non-null
        // - The returned pointer is page-aligned
        // - We can write to the page
        ptr = mmap(pagesize(), PROT_WRITE, None).unwrap();
        test_valid_map_address(ptr);
        test_write(ptr, pagesize());
        munmap(ptr, pagesize());

        // Check that:
        // - Mapping a single read-write page works
        // - The returned pointer is non-null
        // - The returned pointer is page-aligned
        // - We can read the page, and it is zero-filled
        // - We can write to the page, and those writes are properly read back
        ptr = mmap(pagesize(), PROT_READ_WRITE, None).unwrap();
        test_valid_map_address(ptr);
        test_zero_filled(ptr, pagesize());
        test_write_read(ptr, pagesize());
        munmap(ptr, pagesize());
    }

    #[cfg(not(windows))]
    #[test]
    #[should_panic]
    fn test_map_panic_zero() {
        // Check that zero length causes mmap to panic. On Windows, our mmap implementation never
        // panics.
        mmap(0, PROT_READ_WRITE, None);
    }

    #[cfg(all(not(all(target_os = "linux", target_pointer_width = "64")), not(windows)))]
    #[test]
    #[should_panic]
    fn test_map_panic_too_large() {
        // Check that an overly large length causes mmap to panic. On Windows, our mmap
        // implementation never panics. On 64-bit Linux, mmap simply responds to overly large mmaps
        // by returning ENOMEM.
        use core::usize::MAX;
        mmap(MAX, PROT_READ_WRITE, None);
    }

    #[cfg(not(windows))]
    #[test]
    #[should_panic]
    fn test_unmap_panic_zero() {
        // Check that zero length causes munmap to panic. On Windows, the length parameter is
        // ignored, so the page will simply be unmapped normally.

        // NOTE: This test leaks memory, but it's only a page, so it doesn't really matter.
        let ptr = mmap(pagesize(), PROT_READ_WRITE, None).unwrap();
        munmap(ptr, 0);
    }

    #[test]
    #[should_panic]
    fn test_unmap_panic_unaligned() {
        // Check that a non-page-aligned address causes munmap to panic.
        munmap((pagesize() / 2) as *mut u8, pagesize());
    }

    #[cfg(not(windows))]
    #[cfg(not(feature = "test-no-std"))]
    #[bench]
    #[ignore]
    fn bench_large_mmap(b: &mut Bencher) {
        // Determine the speed of mapping a large region of memory so that we can tune the timeout
        // in test_map_non_windows.
        b.iter(|| {
                   let ptr = mmap(1 << 29, PROT_READ_WRITE, None).unwrap();
                   munmap(ptr, 1 << 29);
               })
    }
}
