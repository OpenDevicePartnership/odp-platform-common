//! OEM platform definition — shared across runtimes.
//!
//! This module is intentionally portable: it has no TianoCore-specific code.
//! Move it into a separate crate when you want to also run on Patina native;
//! both the TianoCore driver binary (this crate, via `driver_entry!`) and
//! the Patina-native binary (via `impl_component_info!`) can depend on the
//! same source.
//!
//! ## What's demonstrated
//!
//! [`BiosInfoSmbiosPublisher`] is the headline example. It depends on
//! `Service<dyn Smbios>` from `patina_smbios`, builds a typed
//! `Type0PlatformFirmwareInformation` record, and hands it to
//! `smbios.add_record(...)`. There is no reference to `EFI_SMBIOS_PROTOCOL`,
//! no raw buffer construction in the publishing component. Serialization,
//! handle allocation, string-pool assembly, and table publication all
//! happen inside `patina_smbios`.

extern crate alloc;

use alloc::string::String;
use alloc::vec;

use patina::component::component;
use patina::component::service::Service;
use patina::error::Result;
use patina_smbios::component::SmbiosProvider;
use patina_smbios::service::{SMBIOS_HANDLE_PI_RESERVED, Smbios, SmbiosExt, SmbiosTableHeader};
use patina_smbios::smbios_record::Type0PlatformFirmwareInformation;
use patina_tianocore::prelude::*;

/// Publishes a Type 0 (BIOS Information) SMBIOS record via the native
/// `Service<dyn Smbios>` interface.
///
/// ## Build-time metadata
///
/// `firmware_version` and `firmware_release_date` are pulled from the build
/// command line — same convention as `patina-dxe-core-oem`:
///
/// - `env!("CARGO_PKG_VERSION")` — version string from this crate's
///   `Cargo.toml`.
/// - `option_env!("BUILD_DATE")` — set by your build system
///   (e.g. `BUILD_DATE=04/30/2026 cargo build ...`); falls back to
///   `01/01/1970` when absent.
pub struct BiosInfoSmbiosPublisher {
    pub vendor: &'static str,
    pub system_bios_major: u8,
    pub system_bios_minor: u8,
}

#[component]
impl BiosInfoSmbiosPublisher {
    pub fn entry_point(self, smbios: Service<dyn Smbios>) -> Result<()> {
        let firmware_version = env!("CARGO_PKG_VERSION");
        let firmware_release_date = option_env!("BUILD_DATE").unwrap_or("01/01/1970");

        let record = Type0PlatformFirmwareInformation {
            header: SmbiosTableHeader::new(0, 0, SMBIOS_HANDLE_PI_RESERVED),
            // String pool is 1-indexed — these reference the vec below.
            vendor: 1,
            firmware_version: 2,
            firmware_release_date: 3,
            bios_starting_address_segment: 0xE000,
            firmware_rom_size: 0x0F,
            characteristics: 0x08,
            characteristics_ext1: 0x03,
            characteristics_ext2: 0x01,
            system_bios_major_release: self.system_bios_major,
            system_bios_minor_release: self.system_bios_minor,
            embedded_controller_major_release: 0xFF,
            embedded_controller_minor_release: 0xFF,
            extended_bios_rom_size: 0x0000,
            string_pool: vec![
                String::from(self.vendor),
                String::from(firmware_version),
                String::from(firmware_release_date),
            ],
        };

        let handle = smbios.add_record(None, &record).map_err(|e| {
            log::error!("BiosInfoSmbiosPublisher: add_record failed: {:?}", e);
            patina::error::EfiError::DeviceError
        })?;

        log::info!(
            "BiosInfoSmbiosPublisher: added Type 0 BIOS info (vendor={}, version={}, date={}) handle=0x{:04X}",
            self.vendor,
            firmware_version,
            firmware_release_date,
            handle,
        );
        Ok(())
    }
}

/// The single `Platform` impl that drives both runtimes.
pub struct OemPlatform;

impl Platform for OemPlatform {
    fn components(add: &mut impl ComponentAdder) {
        // The SMBIOS service provider from patina_smbios. On TianoCore this
        // also installs the EDK II SMBIOS protocol internally for legacy C
        // consumers — but that's `patina_smbios`'s implementation detail,
        // invisible to the publisher below.
        add.component(SmbiosProvider::new(3, 9));

        // Pure Rust SMBIOS record publisher. Zero C protocol references.
        add.component(BiosInfoSmbiosPublisher {
            vendor: "Acme Inc.",
            system_bios_major: 2,
            system_bios_minor: 14,
        });
    }
}
