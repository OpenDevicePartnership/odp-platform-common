//! TianoCore-backed `MemoryManager` implementation.
//!
//! SPDX-License-Identifier: MIT
//!
//! This module adapts UEFI boot services memory allocation into Patina's
//! `MemoryManager` service trait so that any Patina component requiring
//! memory (SMBIOS, advanced logger, etc.) works without modification.

use core::alloc::Allocator;

use patina::component::service::memory::{
    AccessType, AllocationOptions, CachingType, MemoryError, MemoryManager, PageAllocation, PageAllocationStrategy,
};
use patina::efi_types::EfiMemoryType;
use patina_macro::IntoService;
use r_efi::efi;

const UEFI_PAGE_SIZE: usize = 4096;

/// A `MemoryManager` backed by UEFI boot services `AllocatePages` / `FreePages`.
///
/// Registered automatically by [`DriverContext::from_system_table`](crate::DriverContext::from_system_table)
/// so driver authors never need to create this manually.
#[derive(IntoService)]
#[service(dyn MemoryManager)]
pub struct TianocoreMemoryManager {
    boot_services: *const efi::BootServices,
}

impl TianocoreMemoryManager {
    /// # Safety
    ///
    /// `boot_services` must remain valid for the DXE phase.
    pub unsafe fn new(boot_services: *const efi::BootServices) -> Self {
        Self { boot_services }
    }
}

// SAFETY: Boot services are effectively single-threaded (TPL concurrency).
unsafe impl Send for TianocoreMemoryManager {}
unsafe impl Sync for TianocoreMemoryManager {}

impl MemoryManager for TianocoreMemoryManager {
    fn allocate_pages(&self, page_count: usize, options: AllocationOptions) -> Result<PageAllocation, MemoryError> {
        if page_count == 0 {
            return Err(MemoryError::InvalidPageCount);
        }

        let memory_type: efi::MemoryType = options.memory_type().into();

        let (alloc_type, mut address): (efi::AllocateType, efi::PhysicalAddress) = match options.strategy() {
            PageAllocationStrategy::Any => (efi::ALLOCATE_ANY_PAGES, 0),
            PageAllocationStrategy::MaxAddress(max) => (efi::ALLOCATE_MAX_ADDRESS, max as efi::PhysicalAddress),
            PageAllocationStrategy::Address(addr) => {
                if !addr.is_multiple_of(UEFI_PAGE_SIZE) {
                    return Err(MemoryError::UnalignedAddress);
                }
                (efi::ALLOCATE_ADDRESS, addr as efi::PhysicalAddress)
            }
        };

        let status =
            unsafe { ((*self.boot_services).allocate_pages)(alloc_type, memory_type, page_count, &mut address) };

        if status != efi::Status::SUCCESS {
            log::error!(
                "AllocatePages failed: status={:?}, page_count={}, type={:?}",
                status,
                page_count,
                memory_type
            );
            return Err(MemoryError::NoAvailableMemory);
        }

        // SAFETY: Allocation succeeded; address is valid and page-aligned.
        // Cast to 'static is safe because the service is leaked when registered
        // via IntoService.
        unsafe {
            let static_self: &'static dyn MemoryManager = &*(self as *const dyn MemoryManager);
            PageAllocation::new(address as usize, page_count, static_self).map_err(|_| MemoryError::InternalError)
        }
    }

    unsafe fn free_pages(&self, address: usize, page_count: usize) -> Result<(), MemoryError> {
        if page_count == 0 {
            return Err(MemoryError::InvalidPageCount);
        }
        if !address.is_multiple_of(UEFI_PAGE_SIZE) {
            return Err(MemoryError::UnalignedAddress);
        }

        // SAFETY: Caller guarantees address was previously allocated via allocate_pages.
        let status = unsafe { ((*self.boot_services).free_pages)(address as efi::PhysicalAddress, page_count) };

        if status != efi::Status::SUCCESS {
            return Err(MemoryError::InvalidAddress);
        }

        Ok(())
    }

    unsafe fn set_page_attributes(
        &self,
        _address: usize,
        _page_count: usize,
        _access: AccessType,
        _caching: Option<CachingType>,
    ) -> Result<(), MemoryError> {
        // UEFI Boot Services don't expose page attribute manipulation directly.
        // The Memory Attribute Protocol would be needed. For typical DXE drivers
        // this is a safe no-op.
        Ok(())
    }

    fn get_page_attributes(
        &self,
        _address: usize,
        _page_count: usize,
    ) -> Result<(AccessType, CachingType), MemoryError> {
        Ok((AccessType::ReadWrite, CachingType::WriteBack))
    }

    fn get_allocator(&self, _memory_type: EfiMemoryType) -> Result<&'static dyn Allocator, MemoryError> {
        // TianoCore does not provide a per-type Allocator impl.
        // Drivers needing typed allocators should use allocate_pages directly.
        Err(MemoryError::InternalError)
    }
}
