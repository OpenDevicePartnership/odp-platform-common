//! System Recovery Environment boot manager.
//!
//! [`SreBootManager`] implements [`patina_boot::BootOrchestrator`] for platforms
//! shipping a System Recovery Environment alongside the main OS. The current
//! skeleton implements the **normal** boot path only:
//!
//! 1. Interleave controller connection with DXE driver dispatch
//! 2. Extra `connect_all` pass before EndOfDxe so platforms whose driver-binding
//!    runs only in the open window get a chance to bind (e.g. PartitionDxe
//!    creating GPT child handles)
//! 3. Signal `EndOfDxe` (security lockdown)
//! 4. Discover console devices
//! 5. Write-lock the NVMe boot partition (volatile, until power cycle)
//!    — currently a stub pending `patina_boot::partition` (odp-platform-common#61)
//! 6. Enumerate firmware `Boot####` EFI variables via `discover_boot_options`
//!    and try each in order: `signal_ready_to_boot` then `boot_from_device_path`
//! 7. If discovery yields no entries or fails, fall back to the
//!    constructor-provided `main_os_path` (one final `signal_ready_to_boot` +
//!    `boot_from_device_path`)
//! 8. Return `EfiError::NotFound` if every boot attempt has been exhausted
//!
//! Hotkey detection (Power+Vol-Up → SRE), SRE WIM RAM-disk boot, and capsule
//! update orchestration are tracked separately and will layer onto this skeleton
//! without changing the public constructor surface.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
extern crate alloc;

use patina::{
    boot_services::{BootServices, StandardBootServices},
    component::service::dxe_dispatch::DxeDispatch,
    device_path::paths::DevicePathBuf,
    error::EfiError,
    runtime_services::StandardRuntimeServices,
};
use patina_boot::{BootOrchestrator, helpers};
use r_efi::efi;

fn interleave_connect_and_dispatch<B: BootServices, D: DxeDispatch + ?Sized>(
    boot_services: &B,
    dxe_services: &D,
) -> patina::error::Result<()> {
    const MAX_ROUNDS: usize = 10;

    for _round in 0..MAX_ROUNDS {
        helpers::connect_all(boot_services)?;
        if !dxe_services.dispatch()? {
            return Ok(());
        }
    }

    log::warn!("connect-dispatch interleaving did not converge after {MAX_ROUNDS} rounds");

    Ok(())
}

/// SRE boot manager implementing [`BootOrchestrator`].
///
/// Skeleton — normal boot path only. The SRE-entry hotkey, WIM-to-RAM-disk boot,
/// and capsule-update pre-boot hook will land in subsequent issues and extend this
/// orchestrator without changing the public constructor surface.
pub struct SreBootManager {
    boot_partition_path: DevicePathBuf,
    main_os_path: DevicePathBuf,
}

impl SreBootManager {
    /// Construct an `SreBootManager` from the device paths of the boot partition
    /// (to be write-locked before OS hand-off) and the main OS boot device.
    pub fn new(boot_partition_path: DevicePathBuf, main_os_path: DevicePathBuf) -> Self {
        Self { boot_partition_path, main_os_path }
    }
}

impl BootOrchestrator for SreBootManager {
    #[coverage(off)]
    fn execute(
        &self,
        boot_services: &StandardBootServices,
        runtime_services: &StandardRuntimeServices,
        dxe_dispatch: &dyn DxeDispatch,
        image_handle: efi::Handle,
    ) -> Result<!, EfiError> {
        if let Err(e) = interleave_connect_and_dispatch(boot_services, dxe_dispatch) {
            log::error!("interleave_connect_and_dispatch failed: {:?}", e);
        }

        // One last connect pass before EndOfDxe so PartitionDxe and similar
        // driver bindings can run during the open window.
        if let Err(e) = helpers::connect_all(boot_services) {
            log::error!("connect_all (pre-EndOfDxe) failed: {:?}", e);
        }

        if let Err(e) = helpers::signal_bds_phase_entry(boot_services) {
            log::error!("signal_bds_phase_entry failed: {:?}", e);
        }

        if let Err(e) = helpers::discover_console_devices(boot_services, runtime_services) {
            log::error!("discover_console_devices failed: {:?}", e);
        }

        // TODO(odp-platform-common#61): boot-partition write-lock helper isn't in
        // patina_boot yet (PR #1488 closed; reopening planned). Skipping the lock
        // for now — the SRE integrity guarantee requires this before shipping.
        log::warn!(
            "boot-partition write-lock skipped (issue #61 pending); target path = {:?}",
            self.boot_partition_path
        );

        // Try boot options discovered from the firmware's Boot#### EFI variables.
        // The constructor's `main_os_path` is used as a fallback when discovery
        // either fails OR yields no entries (both leave `tried_any == false`).
        let mut tried_any = false;
        match helpers::discover_boot_options(runtime_services) {
            Ok(boot_config) => {
                for device_path in boot_config.devices() {
                    tried_any = true;
                    if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                        log::error!("signal_ready_to_boot failed: {:?}", e);
                    }
                    match helpers::boot_from_device_path(boot_services, image_handle, device_path) {
                        Ok(()) => log::warn!("Boot option returned control (path={:?}), trying next...", device_path),
                        Err(e) => log::warn!("Boot option failed (path={:?}): {:?}", device_path, e),
                    }
                }
            }
            Err(e) => log::error!("discover_boot_options failed: {:?}", e),
        }

        if !tried_any {
            // Discovery returned no entries or errored — fall back to the
            // constructor-provided path.
            if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                log::error!("signal_ready_to_boot failed: {:?}", e);
            }
            match helpers::boot_from_device_path(boot_services, image_handle, &self.main_os_path) {
                Ok(()) => log::warn!("Main OS fallback returned control (path={:?})", self.main_os_path),
                Err(e) => log::warn!("Main OS fallback failed (path={:?}): {:?}", self.main_os_path, e),
            }
        }

        log::error!("SRE normal boot exhausted all boot options");
        Err(EfiError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use alloc::{boxed::Box, sync::Arc, vec::Vec};
    use patina::{
        boot_services::{MockBootServices, boxed::BootServicesBox},
        device_path::{node_defs::EndEntire, paths::DevicePathBuf},
    };

    fn test_device_path() -> DevicePathBuf {
        DevicePathBuf::from_device_path_node_iter(core::iter::once(EndEntire))
    }

    struct MockDxeDispatcher {
        results: spin::Mutex<alloc::collections::VecDeque<patina::error::Result<bool>>>,
    }

    impl MockDxeDispatcher {
        fn new(results: &[patina::error::Result<bool>]) -> Self {
            Self { results: spin::Mutex::new(results.iter().cloned().collect()) }
        }
    }

    impl DxeDispatch for MockDxeDispatcher {
        fn dispatch(&self) -> patina::error::Result<bool> {
            self.results.lock().pop_front().expect("MockDxeDispatcher: unexpected dispatch call")
        }
    }

    fn leaked_boot_services_for_box() -> &'static MockBootServices {
        Box::leak(Box::new({
            let mut m = MockBootServices::new();
            m.expect_free_pool().returning(|_| Ok(()));
            m
        }))
    }

    fn mock_handle_buffer(
        handle_addrs: &[usize],
        boot_services: &'static MockBootServices,
    ) -> BootServicesBox<'static, [efi::Handle], MockBootServices> {
        let handles: Vec<efi::Handle> = handle_addrs.iter().map(|&a| a as efi::Handle).collect();
        let leaked = handles.leak();
        // SAFETY: leaked is a valid pointer+length from Vec::leak.
        unsafe { BootServicesBox::from_raw_parts_mut(leaked.as_mut_ptr(), leaked.len(), boot_services) }
    }

    #[test]
    fn test_new_constructs() {
        let _ = SreBootManager::new(test_device_path(), test_device_path());
    }

    #[test]
    fn test_interleave_single_round_no_drivers_dispatched() {
        let box_mock = leaked_boot_services_for_box();
        let mut boot_mock = MockBootServices::new();

        boot_mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[1], box_mock)));
        boot_mock.expect_connect_controller().returning(|_, _, _, _| Ok(()));

        let dxe_mock = MockDxeDispatcher::new(&[Ok(false)]);

        let result = interleave_connect_and_dispatch(&boot_mock, &dxe_mock);
        assert!(result.is_ok());
    }

    #[test]
    fn test_interleave_dispatch_failure_propagates() {
        let box_mock = leaked_boot_services_for_box();
        let mut boot_mock = MockBootServices::new();

        boot_mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[1], box_mock)));
        boot_mock.expect_connect_controller().returning(|_, _, _, _| Ok(()));

        let dxe_mock = MockDxeDispatcher::new(&[Err(EfiError::DeviceError)]);

        let result = interleave_connect_and_dispatch(&boot_mock, &dxe_mock);
        assert!(result.is_err());
    }

    #[test]
    fn test_interleave_stops_at_max_rounds() {
        let box_mock = leaked_boot_services_for_box();
        let mut boot_mock = MockBootServices::new();

        boot_mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[1], box_mock)));
        boot_mock.expect_connect_controller().returning(|_, _, _, _| Ok(()));

        let dxe_mock = MockDxeDispatcher::new(&[Ok(true); 10]);

        let result = interleave_connect_and_dispatch(&boot_mock, &dxe_mock);
        assert!(result.is_ok());
    }

    // Type-level confirmation that SreBootManager satisfies BootOrchestrator's
    // Send + Sync + 'static bounds at compile time.
    #[test]
    fn test_implements_boot_orchestrator() {
        fn assert_orchestrator<T: BootOrchestrator>() {}
        assert_orchestrator::<SreBootManager>();
    }

    // Confirm the manager is constructible behind an Arc<dyn BootOrchestrator>,
    // matching the BootDispatcher consumption path.
    #[test]
    fn test_arc_dyn_construction() {
        let _: Arc<dyn BootOrchestrator> = Arc::new(SreBootManager::new(test_device_path(), test_device_path()));
    }
}
