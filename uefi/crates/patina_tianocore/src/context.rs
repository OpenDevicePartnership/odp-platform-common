//! Driver context — the single entry point for accessing platform services.
//!
//! `DriverContext` wraps the platform-specific initialization (TianoCore system
//! table or Patina DXE core) and exposes a uniform Patina `Storage` that
//! drivers use for component registration and service access.

use patina::boot_services::StandardBootServices;
use patina::component::Storage;

use crate::memory::TianocoreMemoryManager;
use crate::platform::{ComponentList, ConfigList, Platform, ServiceList};

/// Result type returned by driver entry points.
pub type DriverResult = patina::error::Result<()>;

/// Platform-independent driver context.
///
/// Created by the entry-point macro from the raw UEFI parameters. Provides
/// two usage modes:
///
/// 1. **`dispatch_platform::<P>()`** — register and dispatch all components
///    defined by a [`crate::Platform`] impl. This is the recommended
///    path that directly mirrors how `patina-dxe-core-<platform>` works.
///
/// 2. **`storage()`** — direct access to the underlying `Storage` for manual
///    component registration when more control is needed.
pub struct DriverContext {
    storage: Storage,
}

impl DriverContext {
    /// Build a context from the raw TianoCore system table pointer.
    ///
    /// This performs all the one-time setup that the existing
    /// `PatinaSmbiosDxe/main.rs` does by hand:
    ///
    /// 1. Initialises `StandardBootServices`.
    /// 2. Registers a `MemoryManager` service backed by UEFI boot services.
    /// 3. Returns a ready-to-use `Storage`.
    ///
    /// # Safety
    ///
    /// - `system_table` must point to a valid `r_efi::system::SystemTable` that
    ///   remains valid for the duration of the DXE phase.
    /// - Must be called before `ExitBootServices`.
    #[cfg(feature = "tianocore")]
    pub unsafe fn from_system_table(
        _image_handle: *const core::ffi::c_void,
        system_table: *const r_efi::system::SystemTable,
    ) -> Self {
        // SAFETY: Caller guarantees system_table is valid.
        let boot_services_ptr = unsafe { (*system_table).boot_services };

        let mut storage = Storage::new();

        // Wire up Patina's StandardBootServices to the TianoCore table.
        storage.set_boot_services(StandardBootServices::new(boot_services_ptr));

        // Register TianoCore-backed MemoryManager so any Patina component
        // that needs memory allocation (SMBIOS, etc.) works out of the box.
        // SAFETY: boot_services_ptr is valid for the DXE phase per caller contract.
        let memory_manager = unsafe { TianocoreMemoryManager::new(boot_services_ptr) };
        storage.add_service(memory_manager);

        Self { storage }
    }

    /// Register and dispatch all components defined by a [`Platform`] impl.
    ///
    /// This is the primary API for the TianoCore bridge. It mirrors what
    /// `patina_dxe_core::Core::start_dispatcher` does:
    ///
    /// 1. Collects configs, services, and components from the platform.
    /// 2. Initialises each component against `Storage`.
    /// 3. Dispatches components (runs them), respecting dependency ordering.
    /// 4. Locks configs and re-dispatches for components waiting on locked state.
    pub fn dispatch_platform<P: Platform>(&mut self) -> DriverResult {
        // Phase 1: Collect platform registrations.
        P::configs(&mut ConfigList {
            storage: &mut self.storage,
        });
        P::services(&mut ServiceList {
            storage: &mut self.storage,
        });

        let mut component_list = ComponentList::new();
        P::components(&mut component_list);

        // Phase 2: Initialise all components.
        let mut ready = alloc::vec::Vec::new();
        for mut component in component_list.components {
            if component.initialize(&mut self.storage) {
                ready.push(component);
            } else {
                log::warn!(
                    "Component {:?} failed to initialize — skipping",
                    component.metadata().name()
                );
            }
        }

        // Phase 3: Dispatch loop (mirrors core_dispatcher).
        loop {
            let mut dispatched = false;
            ready.retain_mut(|component| {
                match component.run(&mut self.storage) {
                    Ok(true) => {
                        dispatched = true;
                        false // remove — successfully dispatched
                    }
                    Ok(false) => true, // retain — deps not yet satisfied
                    Err(e) => {
                        log::error!("Component {:?} failed: {:?}", component.metadata().name(), e);
                        false // remove — failed
                    }
                }
            });

            if !dispatched {
                break;
            }
        }

        // Phase 4: Lock configs and re-dispatch remaining components.
        self.storage.lock_configs();
        loop {
            let mut dispatched = false;
            ready.retain_mut(|component| match component.run(&mut self.storage) {
                Ok(true) => {
                    dispatched = true;
                    false
                }
                Ok(false) => true,
                Err(e) => {
                    log::error!("Component {:?} failed: {:?}", component.metadata().name(), e);
                    false
                }
            });

            if !dispatched {
                break;
            }
        }

        // Log undispatched components.
        for component in &ready {
            log::warn!(
                "Component {:?} was never dispatched (unsatisfied dependencies)",
                component.metadata().name()
            );
        }

        Ok(())
    }

    /// Create a context from an existing `Storage`.
    ///
    /// Useful for testing on host targets where no UEFI system table exists.
    /// The caller is responsible for populating the `Storage` with any
    /// services the platform components require (e.g. `MemoryManager`).
    pub fn from_storage(storage: Storage) -> Self {
        Self { storage }
    }

    /// Returns a mutable reference to the underlying `Storage`.
    ///
    /// Use this for manual component registration when `dispatch_platform`
    /// doesn't provide enough control.
    pub fn storage(&mut self) -> &mut Storage {
        &mut self.storage
    }

    /// Consumes the context and returns the inner `Storage`.
    pub fn into_storage(self) -> Storage {
        self.storage
    }
}
