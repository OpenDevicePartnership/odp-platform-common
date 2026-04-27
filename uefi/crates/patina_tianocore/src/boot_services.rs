//! Re-export and helpers for boot services.
//!
//! When compiled with `tianocore`, this module provides a global allocator
//! backed by UEFI `AllocatePool`/`FreePool`, so normal `alloc` crates work
//! inside DXE drivers without extra setup.

use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, Ordering};

use r_efi::efi;

/// A global allocator that forwards to UEFI boot services pool allocation.
///
/// # Usage
///
/// In your driver crate root:
///
/// ```rust,ignore
/// #[global_allocator]
/// static ALLOCATOR: patina_tianocore::boot_services::UefiAllocator =
///     patina_tianocore::boot_services::UefiAllocator::new();
/// ```
///
/// Then call [`init`](UefiAllocator::init) from your entry point (the
/// [`driver_entry!`](crate::driver_entry) macro does this automatically).
pub struct UefiAllocator {
    boot_services: AtomicPtr<efi::BootServices>,
}

impl Default for UefiAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl UefiAllocator {
    /// Create an uninitialised allocator. Safe to use in a `static`.
    pub const fn new() -> Self {
        Self {
            boot_services: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    /// Initialise the allocator with a pointer to the boot services table.
    ///
    /// # Safety
    ///
    /// `bs` must point to a valid `efi::BootServices` that remains valid
    /// until `ExitBootServices` is called.
    pub unsafe fn init(&self, bs: *mut efi::BootServices) {
        self.boot_services.store(bs, Ordering::Release);
    }
}

unsafe impl GlobalAlloc for UefiAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let bs = self.boot_services.load(Ordering::Acquire);
        if bs.is_null() {
            return core::ptr::null_mut();
        }

        // UEFI pool allocations are 8-byte aligned. For larger alignments
        // we over-allocate, store the real pointer before the aligned start,
        // and return the aligned pointer.
        let align = layout.align();
        let size = layout.size();

        if align <= 8 {
            let mut ptr: *mut c_void = core::ptr::null_mut();
            // SAFETY: bs is non-null and valid per init() contract.
            let status = unsafe { ((*bs).allocate_pool)(efi::LOADER_DATA, size, &mut ptr) };
            if status != efi::Status::SUCCESS {
                return core::ptr::null_mut();
            }
            ptr as *mut u8
        } else {
            // Compute `size + align + size_of::<*mut c_void>()` with overflow
            // checks. A pathological caller could pass `usize::MAX`; we'd
            // rather return null than wrap and write out-of-bounds.
            let header_space = core::mem::size_of::<*mut c_void>();
            let Some(total) = size.checked_add(align).and_then(|s| s.checked_add(header_space)) else {
                return core::ptr::null_mut();
            };

            let mut raw: *mut c_void = core::ptr::null_mut();
            // SAFETY: bs is non-null and valid per init() contract.
            let status = unsafe { ((*bs).allocate_pool)(efi::LOADER_DATA, total, &mut raw) };
            if status != efi::Status::SUCCESS {
                return core::ptr::null_mut();
            }

            let raw_addr = raw as usize;
            let aligned = (raw_addr + header_space + align - 1) & !(align - 1);

            // SAFETY: aligned - header_space is within the allocation we just made.
            unsafe {
                let backptr = (aligned - header_space) as *mut *mut c_void;
                *backptr = raw;
            }

            aligned as *mut u8
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let bs = self.boot_services.load(Ordering::Acquire);
        if bs.is_null() {
            return;
        }

        if layout.align() <= 8 {
            // SAFETY: ptr was allocated by alloc() via the same boot services.
            unsafe { ((*bs).free_pool)(ptr as *mut c_void) };
        } else {
            let header_space = core::mem::size_of::<*mut c_void>();
            // SAFETY: The original pointer was stored at ptr - header_space by alloc().
            unsafe {
                let backptr = (ptr as usize - header_space) as *const *mut c_void;
                let original = *backptr;
                ((*bs).free_pool)(original);
            }
        }
    }
}

// SAFETY: The atomic pointer ensures thread-safe access to boot services.
unsafe impl Send for UefiAllocator {}
unsafe impl Sync for UefiAllocator {}
