//! Reference System Recovery Environment (SRE) boot orchestrator.
//!
//! SPDX-License-Identifier: MIT
//!
//! [`SreBootManager`] implements [`patina_boot::BootOrchestrator`] for platforms shipping a
//! System Recovery Environment alongside the main OS. At boot it runs the common BDS-phase
//! sequencing (controller-connect / dispatch interleave, BDS-phase-entry and `ReadyToBoot` signals,
//! console discovery), then asks a [`HotkeySource`] whether the SRE-entry gesture is active
//! and branches between two boot paths:
//!
//! - **Normal path.** Write-lock the NVMe boot partition, then boot the main OS device.
//! - **SRE path.** Read the SRE WIM from the boot partition, write-lock the partition,
//!   publish the WIM as a RAM-disk virtual block device, and boot the RAM disk.
//!
//! Capsule-update pre-boot hook orchestration is tracked separately and will layer on without
//! changing the constructor surface.
//!
//! ## Adopting for your platform
//!
//! Construct `SreBootManager` with the device paths of the boot partition, the main OS boot
//! device, the partition-relative path of the SRE WIM, and a [`HotkeySource`] implementation
//! that talks to your platform's hotkey hardware. Register the result with the patina
//! [`BootDispatcher`](https://docs.rs/patina_boot/latest/patina_boot/struct.BootDispatcher.html):
//!
//! ```rust,ignore
//! use patina_boot::BootDispatcher;
//! use patina_sre::{NeverSre, SreBootManager};
//!
//! Core::default()
//!     .with_component(BootDispatcher::new(SreBootManager::new(
//!         boot_partition_device_path,
//!         main_os_device_path,
//!         "\\SRE\\winvos.wim",
//!         NeverSre, // replace with your platform hotkey impl
//!     )))
//!     // ... rest of platform components
//! ```
//!
//! For QEMU / unit-test runs that always need to exercise the SRE path, pass [`AlwaysSre`]
//! in place of the platform hotkey.

#![no_std]
#![feature(coverage_attribute)]
#![feature(never_type)]

extern crate alloc;

pub mod hotkey;

pub use hotkey::{AlwaysSre, HotkeySource, NeverSre};

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

    log::warn!("connect-dispatch interleaving did not converge after {MAX_ROUNDS} rounds; proceeding anyway");

    Ok(())
}

/// SRE boot manager implementing [`BootOrchestrator`].
///
/// Generic over the [`HotkeySource`] implementation so the same orchestrator can be wired with
/// a platform-specific hotkey impl in production, or [`AlwaysSre`] / [`NeverSre`] for testing.
pub struct SreBootManager<H: HotkeySource> {
    boot_partition_path: DevicePathBuf,
    main_os_path: DevicePathBuf,
    sre_wim_path: &'static str,
    hotkey: H,
}

impl<H: HotkeySource> SreBootManager<H> {
    /// Construct an `SreBootManager`.
    ///
    /// * `boot_partition_path` — device path of the boot partition (write-locked before OS
    ///   hand-off; also the source partition for the SRE WIM).
    /// * `main_os_path` — device path of the main OS boot device, used by the normal path.
    /// * `sre_wim_path` — partition-relative file path of the SRE WIM (e.g. `\\SRE\\winvos.wim`)
    ///   read by the SRE path.
    /// * `hotkey` — implementation of [`HotkeySource`] used to choose between normal and SRE
    ///   paths at boot.
    pub fn new(
        boot_partition_path: DevicePathBuf,
        main_os_path: DevicePathBuf,
        sre_wim_path: &'static str,
        hotkey: H,
    ) -> Self {
        Self { boot_partition_path, main_os_path, sre_wim_path, hotkey }
    }

    /// Normal boot path: write-lock the boot partition, then boot the main OS device.
    #[coverage(off)]
    fn execute_normal_path<B: BootServices>(
        &self,
        boot_services: &B,
        image_handle: efi::Handle,
    ) -> Result<!, EfiError> {
        // BP write-lock is best-effort: a missing NVMe Pass-Thru protocol (QEMU without BPWPS
        // emulation, USB-stick rescue media, etc.) shouldn't prevent the OS from booting in
        // the current pre-release iteration. Production platforms targeting strict security
        // should distinguish protocol-absent (expected) from protocol-present-but-rejected
        // (anomalous) and fail closed on the latter — tracked as a follow-up before v1.
        if let Err(e) = patina_nvme::lock_partition_write(boot_services, &self.boot_partition_path) {
            log::warn!("BP write-lock failed (continuing): {:?}", e);
        }

        if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
            log::error!("signal_ready_to_boot failed: {:?}", e);
        }

        match helpers::boot_from_device_path(boot_services, image_handle, &self.main_os_path) {
            Ok(()) => log::warn!("Main OS boot returned control unexpectedly"),
            Err(e) => {
                log::warn!("Main OS boot failed: {:?}", e);
                return Err(e);
            }
        }

        log::error!("SRE normal boot exhausted main OS path");
        Err(EfiError::NotFound)
    }

    /// SRE recovery path: read the SRE WIM from the boot partition, lock the partition,
    /// publish the WIM as a RAM disk, and boot the RAM disk.
    #[coverage(off)]
    fn execute_sre_path<B: BootServices>(
        &self,
        boot_services: &B,
        image_handle: efi::Handle,
    ) -> Result<!, EfiError> {
        let wim_bytes =
            match patina_partition::read_partition_file(boot_services, &self.boot_partition_path, self.sre_wim_path) {
                Ok(b) => {
                    log::info!("Read SRE WIM '{}': {} bytes", self.sre_wim_path, b.len());
                    b
                }
                Err(e) => {
                    log::error!("Failed to read SRE WIM '{}': {:?}", self.sre_wim_path, e);
                    return Err(e);
                }
            };

        // BP write-lock is best-effort: a missing NVMe Pass-Thru protocol (QEMU without BPWPS
        // emulation, USB-stick rescue media, etc.) shouldn't block the SRE recovery boot.
        if let Err(e) = patina_nvme::lock_partition_write(boot_services, &self.boot_partition_path) {
            log::warn!("BP write-lock failed (continuing): {:?}", e);
        }

        let ram_disk_path = match patina_ram_disk::install(boot_services, &wim_bytes) {
            Ok(p) => p,
            Err(e) => {
                log::error!("Failed to install WIM as RAM disk: {:?}", e);
                return Err(e);
            }
        };

        if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
            log::error!("signal_ready_to_boot failed: {:?}", e);
        }

        match helpers::boot_from_device_path(boot_services, image_handle, &ram_disk_path) {
            Ok(()) => log::warn!("SRE WIM boot returned control unexpectedly"),
            Err(e) => {
                log::warn!("SRE WIM boot failed: {:?}", e);
                return Err(e);
            }
        }

        log::error!("SRE recovery boot exhausted RAM disk path");
        Err(EfiError::NotFound)
    }
}

impl<H: HotkeySource + Send + Sync + 'static> BootOrchestrator for SreBootManager<H> {
    #[coverage(off)] // Integration point — delegates to helper functions which are individually tested.
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

        if let Err(e) = helpers::signal_bds_phase_entry(boot_services) {
            log::error!("signal_bds_phase_entry failed: {:?}", e);
        }

        if let Err(e) = helpers::discover_console_devices(boot_services, runtime_services) {
            log::error!("discover_console_devices failed: {:?}", e);
        }

        if self.hotkey.sre_requested() {
            log::info!("SRE hotkey detected — taking SRE recovery path");
            self.execute_sre_path(boot_services, image_handle)
        } else {
            log::info!("No SRE hotkey — taking normal boot path");
            self.execute_normal_path(boot_services, image_handle)
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::{boxed::Box, sync::Arc, vec::Vec};

    use patina::{
        boot_services::{MockBootServices, boxed::BootServicesBox},
        device_path::{node_defs::EndEntire, paths::DevicePathBuf},
    };

    use super::*;

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
        let _ = SreBootManager::new(test_device_path(), test_device_path(), "\\SRE\\winvos.wim", NeverSre);
    }

    #[test]
    fn test_new_with_always_sre() {
        let _ = SreBootManager::new(test_device_path(), test_device_path(), "\\SRE\\winvos.wim", AlwaysSre);
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
    // Send + Sync + 'static bounds at compile time, with each HotkeySource impl shipped here.
    #[test]
    fn test_implements_boot_orchestrator() {
        fn assert_orchestrator<T: BootOrchestrator>() {}
        assert_orchestrator::<SreBootManager<NeverSre>>();
        assert_orchestrator::<SreBootManager<AlwaysSre>>();
    }

    // Confirm the manager is constructible behind an Arc<dyn BootOrchestrator>,
    // matching the BootDispatcher consumption path.
    #[test]
    fn test_arc_dyn_construction() {
        let _: Arc<dyn BootOrchestrator> =
            Arc::new(SreBootManager::new(test_device_path(), test_device_path(), "\\SRE\\winvos.wim", NeverSre));
    }
}
