//! System Recovery Environment boot manager.
//!
//! [`SreBootManager`] implements [`patina_boot::BootOrchestrator`] for platforms
//! shipping a System Recovery Environment alongside the main OS. The flow:
//!
//! 1. Interleave controller connection with DXE driver dispatch
//! 2. Extra `connect_all` pass before EndOfDxe so platforms whose driver-binding
//!    runs only in the open window get a chance to bind (e.g. PartitionDxe
//!    creating GPT child handles)
//! 3. Signal `EndOfDxe` (security lockdown)
//! 4. Signal start-of-BDS event groups
//! 5. Discover console devices
//! 6. Probe the hotkey provider; on a recovery chord, dispatch the configured
//!    SRE app path or fall back to [`bp_recovery::run_sre_flow`] (NVMe LID read
//!    → RAM disk → chainload); on a frontpage chord, try USB via live
//!    `SimpleFileSystem` enumeration, then fall back to the configured
//!    frontpage app
//! 7. Write-lock the NVMe boot partition (volatile, until power cycle)
//!    — currently a stub pending `patina_boot::partition` (odp-platform-common#61)
//! 8. Enumerate firmware `Boot####` EFI variables via `discover_boot_options`
//!    and try each in order: `signal_ready_to_boot` then `boot_from_device_path`
//! 9. If discovery yields no entries or fails, fall back to the
//!    constructor-provided `main_os_path`
//! 10. Return `EfiError::NotFound` if every boot attempt has been exhausted
//!
//! Capsule-update orchestration is tracked separately and will layer on without
//! changing the public constructor surface.
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

use crate::bp_recovery;

/// Result of probing the platform's button-services protocol for an SRE
/// hotkey at BDS entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SreHotkey {
    /// Power + Vol-Up was latched. Routes to the SRE recovery app (or the
    /// in-Rust `bp_recovery::run_sre_flow` fallback).
    VolumeUp,
    /// Power + Vol-Down was latched. Routes to USB-first alt boot if
    /// removable USB media is present, else a configured fallback app.
    VolumeDown,
    /// No hotkey latched (or the protocol isn't published).
    None,
}

/// FFI bindings for `MS_BUTTON_SERVICES_PROTOCOL`, a vendor button-services
/// protocol. Platforms publishing this protocol latch the physical
/// Vol-Up + Power / Vol-Down + Power combos during early boot via their
/// embedded controller; this protocol surfaces the latched state at BDS
/// entry. Returns `SreHotkey::None` cleanly when the protocol is absent,
/// so platforms without a button-services producer are unaffected.
mod ms_button_services {
    use r_efi::efi;

    /// `gMsButtonServicesProtocolGuid` — `{e0084c50-3efd-43f7-88df-194df2d160f0}`.
    pub const PROTOCOL_GUID: efi::Guid = efi::Guid::from_fields(
        0xe0084c50,
        0x3efd,
        0x43f7,
        0x88,
        0xdf,
        &[0x19, 0x4d, 0xf2, 0xd1, 0x60, 0xf0],
    );

    /// `(this, out_pressed)`. UEFI `BOOLEAN` is an 8-bit value, not a Rust
    /// `bool` — use `u8` and compare to 0 on the Rust side.
    pub type CheckButtonFn = extern "efiapi" fn(this: *mut Protocol, out_pressed: *mut u8) -> efi::Status;

    pub type ClearFn = extern "efiapi" fn(this: *mut Protocol) -> efi::Status;

    /// Layout matches the protocol's C declaration: three function pointers,
    /// in this order. Platforms may add additional entries; we only need
    /// these three.
    #[repr(C)]
    pub struct Protocol {
        pub pre_boot_volume_down_check: CheckButtonFn,
        pub pre_boot_volume_up_check: CheckButtonFn,
        pub pre_boot_clear_volume_state: ClearFn,
    }
}

/// Probe `MS_BUTTON_SERVICES_PROTOCOL` for an SRE hotkey at BDS entry.
///
/// Mirrors the C-BDS hotkey-handling pattern: locate the protocol, read
/// Vol-Up first then Vol-Down, clear the latched state so other consumers
/// don't double-act on the same press.
///
/// Returns `SreHotkey::None` if the protocol isn't published (graceful
/// fallback on platforms without a button-services producer) or if neither
/// button was latched. Vol-Up takes priority over Vol-Down when somehow
/// both report set.
fn probe_sre_hotkey<B: BootServices>(boot_services: &B) -> SreHotkey {
    use core::ptr;

    // SAFETY: We never alias the returned pointer past this function — it is
    // used purely to fetch a *mut Protocol then dereference the function-pointer
    // table once each. The PROTOCOL_GUID is `'static`.
    let interface_ptr =
        unsafe { boot_services.locate_protocol_unchecked(&ms_button_services::PROTOCOL_GUID, ptr::null_mut()) };

    let protocol = match interface_ptr {
        Ok(p) if !p.is_null() => p as *mut ms_button_services::Protocol,
        Ok(_) => {
            log::info!("SRE hotkey: MS_BUTTON_SERVICES_PROTOCOL had null interface");
            return SreHotkey::None;
        }
        Err(status) => {
            log::info!("SRE hotkey: MS_BUTTON_SERVICES_PROTOCOL not available ({:?})", status);
            return SreHotkey::None;
        }
    };

    let mut vol_up: u8 = 0;
    let mut vol_down: u8 = 0;

    // SAFETY: `protocol` is non-null per the match above; the function-pointer
    // table layout matches the C `MS_BUTTON_SERVICES_PROTOCOL` struct. The
    // function-pointer ABI is `extern "efiapi"` and the out-params are stack
    // locals we own.
    unsafe {
        let up_status = ((*protocol).pre_boot_volume_up_check)(protocol, &mut vol_up);
        let down_status = ((*protocol).pre_boot_volume_down_check)(protocol, &mut vol_down);
        let clear_status = ((*protocol).pre_boot_clear_volume_state)(protocol);
        if up_status.is_error() {
            log::warn!("SRE hotkey: Vol-Up check returned {:?}", up_status);
        }
        if down_status.is_error() {
            log::warn!("SRE hotkey: Vol-Down check returned {:?}", down_status);
        }
        if clear_status.is_error() {
            log::warn!("SRE hotkey: clear returned {:?}", clear_status);
        }
    }

    let result = match (vol_up != 0, vol_down != 0) {
        (true, _) => SreHotkey::VolumeUp,
        (false, true) => SreHotkey::VolumeDown,
        (false, false) => SreHotkey::None,
    };

    log::info!(
        "SRE hotkey probe: vol_up={} vol_down={} -> {:?}",
        vol_up != 0,
        vol_down != 0,
        result,
    );

    result
}

/// `gMsStartOfBdsNotifyGuid` from `PcBdsPkg.dec`. C BDS fires this at the
/// start of the BDS phase; subscribers include Microsoft boot-policy
/// components that key off it for pre-boot work. `static` (not `const`) so
/// `&MS_START_OF_BDS_NOTIFY_GUID` is naturally `&'static efi::Guid` as
/// `create_event_ex_unchecked` requires.
static MS_START_OF_BDS_NOTIFY_GUID: efi::Guid = efi::Guid::from_fields(
    0x056e730a,
    0x2ac9,
    0x4f9c,
    0xa7,
    0x92,
    &[0x1f, 0x3f, 0x1a, 0x48, 0xa2, 0x4d],
);

/// `gDfciStartOfBdsNotifyGuid` from `DfciPkg.dec`. Signaled by `execute()`
/// after the MU start-of-BDS event so the MU `SettingsManagerDxe`
/// publishes `gDfciSettingAccessProtocolGuid`. Safe to signal under
/// Patina because the upstream `DfciManager` is dispatch-order resilient
/// (ProtocolNotify-based + ProcessMailBoxes NULL guards).
static DFCI_START_OF_BDS_NOTIFY_GUID: efi::Guid = efi::Guid::from_fields(
    0xc9341466,
    0x1a6c,
    0x4ded,
    0x89,
    0xc2,
    &[0x78, 0x12, 0xb0, 0x29, 0x9c, 0x45],
);

/// Equivalent of the C `EfiEventGroupSignal(&group_guid)` macro: create a
/// one-shot NOTIFY_SIGNAL event tied to `group_guid`, fire it, then close
/// it. Used to broadcast a start-of-BDS-style notification to whichever
/// DXE drivers registered a callback against the group.
fn signal_event_group<B: BootServices>(boot_services: &B, group_guid: &'static efi::Guid) -> patina::error::Result<()> {
    use patina::boot_services::{event::EventType, tpl::Tpl};

    extern "efiapi" fn noop(_event: *mut core::ffi::c_void, _context: *mut ()) {}

    // SAFETY: noop callback + null context is a valid signal-only event;
    // we use it purely to broadcast to consumers of `group_guid`.
    let event = unsafe {
        boot_services.create_event_ex_unchecked::<()>(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(noop),
            core::ptr::null_mut(),
            group_guid,
        )
    }
    .map_err(EfiError::from)?;

    let signal_result = boot_services.signal_event(event);
    let close_result = boot_services.close_event(event);
    signal_result.map_err(EfiError::from)?;
    close_result.map_err(EfiError::from)?;
    Ok(())
}

fn interleave_connect_and_dispatch<B: BootServices, D: DxeDispatch + ?Sized>(
    boot_services: &B,
    dxe_services: &D,
) -> patina::error::Result<()> {
    // Multi-round connect/dispatch interleaving was observed to leave the
    // PTL I2C5 HID controller in a stuck state on Maa: repeated
    // `connect_all` retriggers the I2C driver's Start() which races the
    // controller's disable-poll path. Restricting to one round matches
    // what the default Patina BootDispatcher does and avoids the race;
    // late-binding drivers still get a chance via the post-loop
    // `connect_all` pass before EndOfDxe in `execute`.
    const MAX_ROUNDS: usize = 1;

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
/// Skeleton — normal boot path plus hotkey dispatch when paths are
/// configured via the `with_*_path` builder methods. WIM-to-RAM-disk boot
/// and capsule-update pre-boot hook will land in subsequent issues and
/// extend this orchestrator without changing the public constructor surface.
pub struct SreBootManager {
    boot_partition_path: DevicePathBuf,
    main_os_path: DevicePathBuf,
    /// Optional FwFile device path for the SRE recovery app — dispatched
    /// when [`probe_sre_hotkey`] returns [`SreHotkey::VolumeUp`]. `None`
    /// means "no SRE app configured; Vol-Up just falls through to normal
    /// Boot#### discovery."
    sre_app_path: Option<DevicePathBuf>,
    /// Optional FwFile device path for a fallback boot-menu / settings
    /// app — dispatched when [`SreHotkey::VolumeDown`] is latched and no
    /// USB-bootable media is present. `None` means "no fallback; Vol-Down
    /// without USB falls through to normal Boot#### discovery."
    frontpage_app_path: Option<DevicePathBuf>,
    /// When `true`, `execute()` checks BP1 for a committed SRE WIM and
    /// (a) skips USB `Boot####` entries while one is present, and
    /// (b) dispatches [`bp_recovery::run_sre_flow`] as the final fallback
    ///     before returning [`EfiError::NotFound`].
    /// Default `false`. Platforms opt in via [`Self::with_bp_sre_fallback`].
    bp_sre_fallback: bool,
}

impl SreBootManager {
    /// Construct an `SreBootManager` from the device paths of the boot partition
    /// (to be write-locked before OS hand-off) and the main OS boot device.
    pub fn new(boot_partition_path: DevicePathBuf, main_os_path: DevicePathBuf) -> Self {
        Self {
            boot_partition_path,
            main_os_path,
            sre_app_path: None,
            frontpage_app_path: None,
            bp_sre_fallback: false,
        }
    }

    /// Wire the SRE recovery app's FwFile device path. When set, Vol-Up at
    /// BDS entry dispatches `boot_from_device_path` on this path; when
    /// unset, Vol-Up falls back to the in-Rust `bp_recovery::run_sre_flow`.
    ///
    /// Caller constructs the path via [`fv_volume_file_device_path`] with
    /// the platform's SRE-app `FILE_GUID` + the host FV's `FvNameGuid`.
    pub fn with_sre_app_path(mut self, sre_app_path: DevicePathBuf) -> Self {
        self.sre_app_path = Some(sre_app_path);
        self
    }

    /// Wire the fallback boot-menu / settings app's FwFile device path.
    /// When set, Vol-Down at BDS entry first probes for USB-bootable media
    /// via live `SimpleFileSystem` handle enumeration; if a USB volume is
    /// found, that's booted; otherwise this path is dispatched.
    ///
    /// Caller constructs the path via [`fv_volume_file_device_path`] with
    /// the platform's fallback-app `FILE_GUID` + the host FV's `FvNameGuid`.
    pub fn with_frontpage_app_path(mut self, frontpage_app_path: DevicePathBuf) -> Self {
        self.frontpage_app_path = Some(frontpage_app_path);
        self
    }

    /// Opt into BP1 SRE WIM fallback. When enabled, [`Self::execute`]
    /// performs a one-time LID 0x15 head read of BP1 before iterating
    /// `Boot####` entries:
    ///
    /// - If BP1 contains a valid WIM magic, USB `Boot####` entries are
    ///   skipped (they typically point at the SRE flashing tool whose
    ///   `\EFI\Boot\bootx64.efi` would re-run and re-flash the same WIM,
    ///   creating a reflash loop on Windows-less devices).
    /// - After all non-USB `Boot####` and `main_os_path` fall through
    ///   without booting, [`bp_recovery::run_sre_flow`] is dispatched
    ///   instead of returning `NotFound`, so the system boots into the
    ///   already-committed SRE WIM rather than failing.
    /// - If BP1 has no WIM (fresh device, never flashed), the fallback
    ///   is a no-op and normal boot semantics apply.
    ///
    /// Default is off. Platforms call this when they ship a flow that
    /// commits the SRE WIM via Firmware Image Download to BP1.
    pub fn with_bp_sre_fallback(mut self) -> Self {
        self.bp_sre_fallback = true;
        self
    }
}

/// True if any node in `dp` is a firmware-volume file/volume reference
/// (`MEDIA_PIWG_FW_FILE_DP` / `MEDIA_PIWG_FW_VOL_DP`). Used to filter
/// `Boot####` entries that point at platform-specific BDS dispatchers
/// (e.g. `MsBootPolicy.efi` in Microsoft platforms) when an SRE fallback
/// is the desired recovery path — those dispatchers can crash under
/// alternate BDS implementations (Patina) and would prevent the fallback
/// from running.
fn device_path_has_fw_file_node(dp: &patina::device_path::paths::DevicePath) -> bool {
    use patina::device_path::node_defs::{DevicePathType, MediaSubType};
    dp.iter().any(|node| {
        let t = node.header.r#type;
        let s = node.header.sub_type;
        t == DevicePathType::Media as u8
            && (s == MediaSubType::PiwgFirmwareFile as u8 || s == MediaSubType::PiwgFirmwareVolume as u8)
    })
}

/// True if any node in `dp` is a USB messaging node (Usb, UsbClass, or
/// UsbWwid sub-types). Used by [`find_first_usb_block_io_device_path`] to
/// filter enumerated `SimpleFileSystem` handles.
fn device_path_has_usb_node(dp: &patina::device_path::paths::DevicePath) -> bool {
    use patina::device_path::node_defs::{DevicePathType, MessagingSubType};
    dp.iter().any(|node| {
        let t = node.header.r#type;
        let s = node.header.sub_type;
        t == DevicePathType::Messaging as u8
            && (s == MessagingSubType::Usb as u8
                || s == MessagingSubType::UsbClass as u8
                || s == MessagingSubType::UsbWwid as u8)
    })
}

/// Locate the device path of the first `SimpleFileSystem` handle whose
/// device path contains a USB messaging node.
///
/// Mirrors the C-BDS pattern of iterating live device topology to find a
/// bootable USB. Filters on `SimpleFileSystem` rather than raw `BlockIo`
/// because: (1) SFS handles only exist on mounted FAT filesystems —
/// `PartitionDxe` and `FatDxe` cooperate to install SFS specifically on
/// the partition hosting a recognizable volume; (2) `LoadImage` on a
/// path terminating in an SFS handle auto-resolves `\EFI\Boot\BOOTX64.EFI`.
/// Picking a whole-device `BlockIo` handle instead gives a path that
/// terminates at the USB messaging node, and `LoadImage` returns
/// `NotFound` because there's no filesystem at that level.
fn find_first_usb_block_io_device_path<B: BootServices>(boot_services: &B) -> Option<DevicePathBuf> {
    use patina::boot_services::protocol_handler::HandleSearchType;
    use patina::device_path::paths::DevicePath;
    use r_efi::protocols::{device_path, simple_file_system};

    let handles = boot_services
        .locate_handle_buffer(HandleSearchType::ByProtocol(&simple_file_system::PROTOCOL_GUID))
        .ok()?;

    for &handle in handles.iter() {
        // Get device path on the SFS handle.
        // SAFETY: handle was returned by locate_handle_buffer for the SFS GUID.
        let dp_ptr = match unsafe { boot_services.handle_protocol_unchecked(handle, &device_path::PROTOCOL_GUID) } {
            Ok(p) => p,
            Err(_) => continue,
        };
        if dp_ptr.is_null() {
            continue;
        }
        // SAFETY: dp_ptr is a well-formed EFI_DEVICE_PATH_PROTOCOL byte stream
        // terminated by EndEntire — `try_from_ptr` walks until EndEntire.
        let dp_ref = match unsafe { DevicePath::try_from_ptr(dp_ptr as *const u8) } {
            Ok(d) => d,
            Err(_) => continue,
        };

        if device_path_has_usb_node(dp_ref) {
            // Append `\EFI\Boot\BOOTX64.EFI` as a FilePath node. Patina's
            // `LoadImage` does not apply the UEFI removable-media auto-resolve
            // rule (which would otherwise pick up the default fallback
            // bootloader when handed a bare SFS-handle path), so we must
            // construct the explicit path ourselves or `LoadImage` returns
            // `NotFound`.
            //
            // SAFETY: dp_ptr is a valid device path terminated by END_ENTIRE.
            let base_total = unsafe { bp_recovery::device_path_size(dp_ptr as *const u8) };
            if base_total < 4 {
                continue;
            }
            let prefix_size = base_total - 4; // strip END_ENTIRE
            // SAFETY: dp_ptr is valid for `base_total` bytes; we slice the
            // prefix that excludes the terminating END_ENTIRE node.
            let prefix = unsafe { core::slice::from_raw_parts(dp_ptr as *const u8, prefix_size) };
            let mut bytes = alloc::vec::Vec::<u8>::with_capacity(base_total + 32);
            bytes.extend_from_slice(prefix);
            bytes.extend_from_slice(&bp_recovery::build_file_path_node("\\EFI\\Boot\\BOOTX64.EFI"));
            // SAFETY: `bytes` is well-formed (prefix nodes + FilePath node +
            // END_ENTIRE, in that order).
            let full = match unsafe { DevicePath::try_from_ptr(bytes.as_ptr()) } {
                Ok(d) => d,
                Err(_) => continue,
            };
            return Some(DevicePathBuf::from(full));
        }
    }
    None
}

/// Construct a partial FwFile device path of the shape
/// `FvFile(<file_guid>) / EndEntire`.
///
/// Suitable for `LoadImage` implementations that walk all installed
/// `EFI_FIRMWARE_VOLUME2_PROTOCOL` handles searching for the file. Patina's
/// `patina_dxe_core` requires the full FV+File form instead — for that, use
/// [`fv_volume_file_device_path`].
pub fn fv_file_device_path(file_guid: efi::Guid) -> DevicePathBuf {
    use patina::device_path::fv_types::FvPiWgDevicePath;
    use patina::device_path::paths::DevicePath;

    let fv_dp = FvPiWgDevicePath::new_file(file_guid);
    // SAFETY: `FvPiWgDevicePath` is `#[repr(C)]` containing a 20-byte FwFile
    // node followed by a 4-byte EndEntire node — well-formed by construction.
    let dp = unsafe { DevicePath::try_from_ptr(&fv_dp as *const FvPiWgDevicePath as *const u8) }
        .expect("FvPiWgDevicePath always well-formed");
    DevicePathBuf::from(dp)
}

/// Construct a full `Fv(<fv_guid>)/FvFile(<file_guid>)/EndEntire` device
/// path. Use when you know which FV hosts the file and the consuming
/// `LoadImage` requires the explicit FV node (Patina's `patina_dxe_core`
/// does — without it the call returns `NotFound` because the bare FvFile
/// shape isn't walked across installed FV2 protocols).
///
/// The C-BDS equivalent typically resolves the FV dynamically via
/// `LoadedImage(gImageHandle).DeviceHandle`. This helper accepts the FV
/// GUID as a parameter; callers typically pin a platform-specific DXE FV
/// GUID. Dynamic resolution is a follow-up.
pub fn fv_volume_file_device_path(fv_guid: efi::Guid, file_guid: efi::Guid) -> DevicePathBuf {
    use patina::device_path::fv_types::{MediaFwDevicePathSubtype, MediaFwVolDevicePath};
    use patina::device_path::paths::DevicePath;

    /// On-wire layout: 20-byte FwVol node | 20-byte FwFile node | 4-byte End.
    #[repr(C)]
    struct FvVolFilePath {
        fv: MediaFwVolDevicePath,
        file: MediaFwVolDevicePath,
        end: efi::protocols::device_path::End,
    }

    let path = FvVolFilePath {
        fv: MediaFwVolDevicePath::new(fv_guid, MediaFwDevicePathSubtype::FirmwareVolume),
        file: MediaFwVolDevicePath::new(file_guid, MediaFwDevicePathSubtype::FirmwareFile),
        end: efi::protocols::device_path::End {
            header: efi::protocols::device_path::Protocol {
                r#type: efi::protocols::device_path::TYPE_END,
                sub_type: efi::protocols::device_path::End::SUBTYPE_ENTIRE,
                length: [4, 0],
            },
        },
    };

    // SAFETY: `FvVolFilePath` is `#[repr(C)]` with 3 well-formed nodes
    // totaling 44 bytes; `try_from_ptr` walks until the EndEntire so the
    // returned slice has the correct length. `DevicePathBuf::from(&_)`
    // copies the bytes into an owned Vec before we return.
    let dp = unsafe { DevicePath::try_from_ptr(&path as *const FvVolFilePath as *const u8) }
        .expect("FvVolFilePath always well-formed");
    DevicePathBuf::from(dp)
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

        // Signal the Microsoft start-of-BDS event group that the C BDS
        // path fires. Boot-policy components in the Microsoft UEFI
        // ecosystem (PcBdsPkg, Project MU) key off this for pre-boot
        // work and it has no observed adverse effects in the Patina
        // dispatch path.
        if let Err(e) = signal_event_group(boot_services, &MS_START_OF_BDS_NOTIFY_GUID) {
            log::error!("signal gMsStartOfBdsNotifyGuid failed: {:?}", e);
        }

        // Signal the DFCI start-of-BDS event so the MU SettingsManager DXE
        // driver publishes gDfciSettingAccessProtocolGuid. Doing this here
        // rather than as part of EndOfDxe processing keeps the resulting
        // SettingAccess-install notify dispatch in a clean stack frame -
        // SemmManager's SettingAccessCallback closes its own event from
        // inside the callback, which would corrupt the EDK2 DxeCore notify
        // iterator if fired while EndOfDxe were still iterating.
        //
        // Safe to signal because the upstream DfciManager is dispatch-order
        // resilient: apply-protocol statics are populated via
        // RegisterProtocolNotify (so they're non-NULL whenever
        // ProcessMailBoxes runs), and ProcessMailBoxes has a top-of-function
        // guard against re-entry after FreeManagerData.
        if let Err(e) = signal_event_group(boot_services, &DFCI_START_OF_BDS_NOTIFY_GUID) {
            log::error!("signal gDfciStartOfBdsNotifyGuid failed: {:?}", e);
        }

        if let Err(e) = helpers::discover_console_devices(boot_services, runtime_services) {
            log::error!("discover_console_devices failed: {:?}", e);
        }

        // Unified SRE hotkey dispatch. probe_sre_hotkey reads the latched
        // Vol-Up/Vol-Down + Power state via MS_BUTTON_SERVICES_PROTOCOL
        // and clears it (so we must run it once — both paths below share
        // the result, no double-read).
        //
        // Vol-Up has two dispatch modes:
        //   1. If `sre_app_path` is configured, dispatch that FwFile via
        //      `boot_from_device_path`. Typical when the platform has a
        //      C-side recovery app stored in the firmware volume.
        //   2. Otherwise, run the in-Rust `bp_recovery::run_sre_flow`
        //      (NVMe LID 0x15 read of BP1 -> RAM disk -> chainload). No
        //      external app needed; everything lives in patina_sre.
        // Callers pick by whether they call `.with_sre_app_path(...)`.
        //
        // Vol-Down: USB-first via SimpleFileSystem enumeration, falling
        // back to `frontpage_app_path` if configured, else fall through
        // to normal Boot#### discovery.
        let hotkey = probe_sre_hotkey(boot_services);
        log::info!("SRE hotkey result: {:?}", hotkey);

        match hotkey {
            SreHotkey::VolumeUp => match &self.sre_app_path {
                Some(path) => {
                    log::info!("SRE hotkey: Vol-Up -> dispatching SRE app at {:?}", path);
                    if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                        log::error!("signal_ready_to_boot (SRE dispatch) failed: {:?}", e);
                    }
                    match helpers::boot_from_device_path(boot_services, image_handle, path) {
                        Ok(()) => log::warn!("SRE app returned control; falling through to Boot####"),
                        Err(e) => log::error!("SRE app boot_from_device_path failed: {:?}", e),
                    }
                }
                None => {
                    log::info!("SRE hotkey: Vol-Up -> running in-Rust bp_recovery flow (no sre_app_path set)");
                    if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                        log::error!("signal_ready_to_boot (bp_recovery) failed: {:?}", e);
                    }
                    match bp_recovery::run_sre_flow(boot_services, image_handle) {
                        Ok(()) => {
                            log::warn!("bp_recovery::run_sre_flow returned control; falling through to normal boot")
                        }
                        Err(e) => log::warn!(
                            "bp_recovery::run_sre_flow failed ({:?}); falling through to normal boot",
                            e
                        ),
                    }
                }
            },
            SreHotkey::VolumeDown => {
                if let Some(usb_path) = find_first_usb_block_io_device_path(boot_services) {
                    log::info!(
                        "SRE hotkey: Vol-Down + USB present -> dispatching USB boot at {:?}",
                        usb_path
                    );
                    if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                        log::error!("signal_ready_to_boot (USB dispatch) failed: {:?}", e);
                    }
                    match helpers::boot_from_device_path(boot_services, image_handle, &usb_path) {
                        Ok(()) => log::warn!("USB boot returned control; falling through to Boot####"),
                        Err(e) => log::error!("USB boot_from_device_path failed: {:?}", e),
                    }
                } else if let Some(path) = &self.frontpage_app_path {
                    log::info!(
                        "SRE hotkey: Vol-Down + no USB -> dispatching fallback app at {:?}",
                        path
                    );
                    if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                        log::error!("signal_ready_to_boot (fallback dispatch) failed: {:?}", e);
                    }
                    match helpers::boot_from_device_path(boot_services, image_handle, path) {
                        Ok(()) => log::warn!("fallback app returned control; falling through to Boot####"),
                        Err(e) => log::error!("fallback boot_from_device_path failed: {:?}", e),
                    }
                } else {
                    log::warn!(
                        "SRE hotkey: Vol-Down latched but no USB SimpleFileSystem handle present and no \
                         frontpage_app_path configured; falling through"
                    );
                }
            }
            SreHotkey::None => {
                // Normal path — falls through to the existing Boot#### discovery below.
            }
        }

        // TODO(odp-platform-common#61): boot-partition write-lock helper isn't in
        // patina_boot yet (PR #1488 closed; reopening planned). Skipping the lock
        // for now — the SRE integrity guarantee requires this before shipping.
        log::warn!(
            "boot-partition write-lock skipped (issue #61 pending); target path = {:?}",
            self.boot_partition_path
        );

        // Optional BP1 SRE WIM fallback. Probed once before Boot#### iteration
        // so we can both filter USB entries (which would re-run the SRE
        // flashing tool that committed the WIM) and dispatch run_sre_flow as
        // the final fallback. Cost: one 512-byte LID 0x15 head read of BP1.
        let bp_has_sre = self.bp_sre_fallback && bp_recovery::bp_has_sre_payload(boot_services);

        // Try boot options discovered from the firmware's Boot#### EFI variables.
        // The constructor's `main_os_path` is used as a fallback when discovery
        // either fails OR yields no entries (both leave `tried_any == false`).
        let mut tried_any = false;
        match helpers::discover_boot_options(runtime_services) {
            Ok(boot_config) => {
                for device_path in boot_config.devices() {
                    if bp_has_sre && device_path_has_usb_node(device_path) {
                        log::info!(
                            "Skipping USB Boot#### (BP1 has SRE payload, fallback enabled); path={:?}",
                            device_path
                        );
                        continue;
                    }
                    if bp_has_sre && device_path_has_fw_file_node(device_path) {
                        log::info!(
                            "Skipping FwFile Boot#### (BP1 has SRE payload, fallback enabled — platform dispatcher likely incompatible with Patina); path={:?}",
                            device_path
                        );
                        continue;
                    }
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

        // Last-resort BP1 SRE fallback. Reached only if every Boot#### entry
        // and the main_os_path either failed or were filtered. Boots the
        // committed SRE WIM directly so a Windows-less device doesn't loop
        // back to the USB flashing tool. Like other dispatch attempts in
        // this function, a returning-control result falls through; only
        // hard errors after this point reach the final `NotFound`.
        if bp_has_sre {
            log::info!("Normal boot exhausted; dispatching SRE from BP1");
            match bp_recovery::run_sre_flow(boot_services, image_handle) {
                Ok(()) => log::warn!("bp_recovery::run_sre_flow returned control; nothing left to try"),
                Err(e) => log::error!("bp_recovery::run_sre_flow failed: {:?}", e),
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
            Self {
                results: spin::Mutex::new(results.iter().cloned().collect()),
            }
        }
    }

    impl DxeDispatch for MockDxeDispatcher {
        fn dispatch(&self) -> patina::error::Result<bool> {
            self.results
                .lock()
                .pop_front()
                .expect("MockDxeDispatcher: unexpected dispatch call")
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

        boot_mock
            .expect_locate_handle_buffer()
            .returning(move |_| Ok(mock_handle_buffer(&[1], box_mock)));
        boot_mock.expect_connect_controller().returning(|_, _, _, _| Ok(()));

        let dxe_mock = MockDxeDispatcher::new(&[Ok(false)]);

        let result = interleave_connect_and_dispatch(&boot_mock, &dxe_mock);
        assert!(result.is_ok());
    }

    #[test]
    fn test_interleave_dispatch_failure_propagates() {
        let box_mock = leaked_boot_services_for_box();
        let mut boot_mock = MockBootServices::new();

        boot_mock
            .expect_locate_handle_buffer()
            .returning(move |_| Ok(mock_handle_buffer(&[1], box_mock)));
        boot_mock.expect_connect_controller().returning(|_, _, _, _| Ok(()));

        let dxe_mock = MockDxeDispatcher::new(&[Err(EfiError::DeviceError)]);

        let result = interleave_connect_and_dispatch(&boot_mock, &dxe_mock);
        assert!(result.is_err());
    }

    #[test]
    fn test_interleave_stops_at_max_rounds() {
        let box_mock = leaked_boot_services_for_box();
        let mut boot_mock = MockBootServices::new();

        boot_mock
            .expect_locate_handle_buffer()
            .returning(move |_| Ok(mock_handle_buffer(&[1], box_mock)));
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
