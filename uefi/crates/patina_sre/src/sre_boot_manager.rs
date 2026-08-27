//! System Recovery Environment boot manager.
//!
//! [`SreBootManager`] implements [`patina_boot::BootOrchestrator`] for platforms
//! shipping a System Recovery Environment alongside the main OS. The flow:
//!
//! 1. Signal the `gMsStartOfBds` event group and, when enabled, the
//!    `gDfciStartOfBds` event group
//! 2. Dispatch DXE drivers so their driver bindings are installed; the device
//!    tree is not connected up front
//! 3. Optional capsule-queue processing via `crate::capsule::process`, before
//!    EndOfDxe while the flash is still writable; signals the capsule processor
//!    (a no-op unless a capsule is queued on a flash-update boot) and draws the
//!    progress-bar logo only on a flash-update boot
//! 4. Signal `EndOfDxe` and install `DxeSmmReadyToLock`; abort boot if either fails
//! 5. Probe the hotkey provider. On a recovery chord, connect the full device
//!    tree and consoles, then dispatch the configured SRE app path or fall back
//!    to `bp_recovery::run_sre_flow` (NVMe LID read → RAM disk → chainload);
//!    on a frontpage chord, try USB via live `SimpleFileSystem` enumeration,
//!    then a configured frontpage app
//! 6. Normal boot: connect the device tree except USB host controllers (storage,
//!    partitions, filesystems and graphics bind, so short-form `Boot####` paths
//!    resolve; USB port enumeration — not on the boot path — is skipped), then
//!    boot each `Boot####` entry in order; if no `Boot####` entry was attempted,
//!    fall back to the constructor `main_os_path`
//! 7. Safety net: if nothing booted, connect the full tree and retry, then
//!    consider only filesystem sources authorized by [`BootSourcePolicy`]
//! 8. Return `EfiError::NotFound` if every authorized boot attempt is exhausted
//!
//! Optional platform hooks, all default-off and builder-enabled: MU
//! capsule-queue processing before EndOfDxe
//! ([`SreBootManager::with_capsule_processing`]), a DFCI start-of-BDS signal
//! ([`SreBootManager::with_dfci_bds_signal`]), and BP1 SRE recovery fallback
//! ([`SreBootManager::with_bp_sre_fallback`]).
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
use patina_boot::{BootOrchestrator, BootSourcePolicy, helpers};
use r_efi::efi;

use crate::{bp_recovery, events::signal_event_group};

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
/// Locate the protocol, read Vol-Up first then Vol-Down, and clear the latched
/// state so other consumers don't double-act on the same press.
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

/// `gMsStartOfBdsNotifyGuid` from `PcBdsPkg.dec`. Fired at the start of the BDS
/// phase; subscribers include Microsoft boot-policy components that key off it
/// for pre-boot work. `static` (not `const`) so
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

/// `gDfciStartOfBdsNotifyGuid` from `DfciPkg.dec`. Signaled immediately after
/// the MU start-of-BDS event and before EndOfDxe so `SettingsManagerDxe`
/// publishes `gDfciSettingAccessProtocolGuid` while the platform is still in
/// its pre-lock phase.
static DFCI_START_OF_BDS_NOTIFY_GUID: efi::Guid = efi::Guid::from_fields(
    0xc9341466,
    0x1a6c,
    0x4ded,
    0x89,
    0xc2,
    &[0x78, 0x12, 0xb0, 0x29, 0x9c, 0x45],
);

fn run_pre_lock_phase<B, W, L, T>(
    boot_services: &B,
    signal_dfci_start: bool,
    pre_lock_work: W,
    enter_locked_boot: L,
) -> T
where
    B: BootServices,
    W: FnOnce(),
    L: FnOnce() -> T,
{
    if let Err(e) = signal_event_group(boot_services, &MS_START_OF_BDS_NOTIFY_GUID) {
        log::error!("signal gMsStartOfBdsNotifyGuid failed: {:?}", e);
    }

    if signal_dfci_start && let Err(e) = signal_event_group(boot_services, &DFCI_START_OF_BDS_NOTIFY_GUID) {
        log::error!("signal gDfciStartOfBdsNotifyGuid failed: {:?}", e);
    }

    pre_lock_work();
    enter_locked_boot()
}

/// SRE boot manager implementing [`BootOrchestrator`].
///
/// Normal boot path plus hotkey dispatch when paths are configured via the
/// `with_*_path` builder methods, with optional builder-enabled hooks:
/// WIM RAM-disk recovery via `bp_recovery::run_sre_flow`
/// ([`Self::with_bp_sre_fallback`]), MU capsule-queue processing
/// ([`Self::with_capsule_processing`]), and a DFCI start-of-BDS signal
/// ([`Self::with_dfci_bds_signal`]).
pub struct SreBootManager {
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
    /// (b) dispatches `bp_recovery::run_sre_flow` as the final fallback
    ///     before returning [`EfiError::NotFound`].
    /// Default `false`. Platforms opt in via [`Self::with_bp_sre_fallback`].
    bp_sre_fallback: bool,
    /// When `true`, `execute()` signals `gDfciStartOfBdsNotifyGuid` so the MU
    /// `SettingsManagerDxe` publishes `gDfciSettingAccessProtocolGuid` and the
    /// DFCI/SEMM mailbox processing runs at BDS entry. Default `false`; platforms
    /// that ship the DFCI/SEMM stack opt in via [`Self::with_dfci_bds_signal`].
    dfci_bds_signal: bool,
    /// When `true`, `execute()` invokes `crate::capsule::process` after
    /// connecting controllers (so FMP protocols are present) and before
    /// EndOfDxe/flash lockdown. That hook signals
    /// `gMuReadyToProcessCapsulesNotifyGuid` on every call (a no-op with an empty
    /// queue; the MU capsule processor drains its queue and cold-resets on a
    /// flash-update boot with a queued capsule) and draws the progress-bar logo
    /// only on a flash-update boot, gated on the boot mode the platform binary
    /// records. Without it, a staged capsule leaves the flash-update boot mode
    /// uncleared and the platform loops. Default `false`. Platforms with the MU
    /// capsule queue opt in via [`Self::with_capsule_processing`].
    capsule_processing: bool,
    /// Explicit policy for filesystem fallback after `Boot####` and the
    /// configured main OS path are exhausted. The default denies fallback.
    boot_source_policy: BootSourcePolicy,
}

impl SreBootManager {
    /// Construct an `SreBootManager` from the main OS boot device path.
    pub fn new(main_os_path: DevicePathBuf) -> Self {
        Self {
            main_os_path,
            sre_app_path: None,
            frontpage_app_path: None,
            bp_sre_fallback: false,
            dfci_bds_signal: false,
            capsule_processing: false,
            boot_source_policy: BootSourcePolicy::new(),
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
    /// - If BP1 contains a valid wrapped SRE payload (FAT signature `0x55AA`), USB `Boot####` entries are
    ///   skipped (they typically point at the SRE flashing tool whose
    ///   `bp_recovery::DEFAULT_BOOT_FILE_PATH` would re-run and re-flash the same payload,
    ///   creating a reflash loop on Windows-less devices).
    /// - After all non-USB `Boot####` and `main_os_path` fall through
    ///   without booting, `bp_recovery::run_sre_flow` is dispatched
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

    /// Opt into signaling `gDfciStartOfBdsNotifyGuid` during [`Self::execute`] so
    /// the DFCI/SEMM stack processes a pending device-setting request at BDS
    /// entry. Default off; enable on platforms that ship the DFCI/SEMM drivers.
    pub fn with_dfci_bds_signal(mut self) -> Self {
        self.dfci_bds_signal = true;
        self
    }

    /// Opt into MU capsule-queue processing during [`Self::execute`]. Invokes
    /// `crate::capsule::process` after connecting controllers and before
    /// EndOfDxe; that hook signals `gMuReadyToProcessCapsulesNotifyGuid` on every
    /// call (handing off to `SecuredCoreCapsuleProcessorDxe`, which self-gates:
    /// a no-op with an empty queue, a drain + cold-reset on a flash-update boot)
    /// and draws the progress-bar logo only on a flash-update boot. Required on
    /// platforms using the MU capsule queue so a staged capsule is applied and
    /// the flash-update boot mode is cleared. Default off.
    pub fn with_capsule_processing(mut self) -> Self {
        self.capsule_processing = true;
        self
    }

    /// Configure filesystem fallback sources and allowed Secure Boot states.
    ///
    /// Without this call, exhaustion of `Boot####` and `main_os_path` does not
    /// enumerate arbitrary filesystem volumes.
    pub fn with_boot_source_policy(mut self, policy: BootSourcePolicy) -> Self {
        self.boot_source_policy = policy;
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
/// UsbWwid sub-types). Used by [`find_first_usb_sfs_device_path`] to
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
/// Iterates live device topology to find a bootable USB. Filters on
/// `SimpleFileSystem` rather than raw `BlockIo`
/// because: (1) SFS handles only exist on mounted FAT filesystems —
/// `PartitionDxe` and `FatDxe` cooperate to install SFS specifically on
/// the partition hosting a recognizable volume; (2) `LoadImage` on a
/// path terminating in an SFS handle auto-resolves the arch default
/// bootloader (`bp_recovery::DEFAULT_BOOT_FILE_PATH`). Picking a
/// whole-device `BlockIo` handle instead gives a path that terminates at
/// the USB messaging node, and `LoadImage` returns `NotFound` because
/// there's no filesystem at that level.
fn find_first_usb_sfs_device_path<B: BootServices>(boot_services: &B) -> Option<DevicePathBuf> {
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
            // Append the arch default bootloader path as a FilePath node.
            // Patina's `LoadImage` does not apply the UEFI removable-media
            // auto-resolve rule (which would otherwise pick up the default
            // fallback bootloader when handed a bare SFS-handle path), so we
            // must construct the explicit path ourselves or `LoadImage`
            // returns `NotFound`.
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
            bytes.extend_from_slice(&bp_recovery::build_file_path_node(bp_recovery::DEFAULT_BOOT_FILE_PATH));
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
/// The FV can also be resolved dynamically via
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
        let locked_boot = run_pre_lock_phase(
            boot_services,
            self.dfci_bds_signal,
            || {
                // Dispatch DXE drivers so their driver bindings are installed.
                loop {
                    match dxe_dispatch.dispatch() {
                        Ok(true) => continue,
                        Ok(false) => break,
                        Err(e) => {
                            log::error!("DXE dispatch failed: {:?}", e);
                            break;
                        }
                    }
                }

                // Connect the device tree except USB host controllers (storage,
                // partitions, filesystems and graphics bind — so short-form Boot####
                // paths resolve, and the firmware-management protocols and GOP are
                // present — while USB port enumeration, not on the boot path, is
                // skipped). Done here so it happens in the pre-EndOfDxe open window and
                // the capsule block below has its FMP and a drawable console without a
                // separate connect. A full connect_all second pass in the boot loop
                // guarantees boot if this leaves the boot device unreachable.
                if let Err(e) = helpers::connect_all_skip_usb(boot_services) {
                    log::error!("connect_all_skip_usb failed: {:?}", e);
                }

                // Drive capsule-queue processing before EndOfDxe, while the flash is
                // still writable. crate::capsule::process signals the capsule processor
                // unconditionally (a no-op with an empty queue or a normal boot; a drain
                // + cold-reset on a flash-update boot, so it may not return) and draws
                // the progress-bar logo only on a flash-update boot. Its firmware-
                // management protocols are already present (installed at DXE dispatch,
                // device FMPs bound by the connect above), so no extra device-tree
                // connect is needed.
                if self.capsule_processing {
                    crate::capsule::process(boot_services);
                }
            },
            || {
                // Complete the security transition after capsule processing so
                // the capsule path stays flash-writable. No image-dispatch
                // capability is produced unless both EndOfDxe and ReadyToLock
                // succeed.
                helpers::enter_locked_boot(boot_services).inspect_err(|e| {
                    log::error!("boot security transition failed: {:?}", e);
                })
            },
        )?;

        // Unified SRE hotkey dispatch. probe_sre_hotkey reads the latched
        // Vol-Up/Vol-Down + Power state via MS_BUTTON_SERVICES_PROTOCOL
        // and clears it (so we must run it once — both paths below share
        // the result, no double-read).
        //
        // Vol-Up has two dispatch modes:
        //   1. If `sre_app_path` is configured, dispatch that FwFile via
        //      `boot_from_device_path`. Typical when the platform has a
        //      recovery app stored in the firmware volume.
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

        // Recovery paths (Vol-Up SRE / bp_recovery, Vol-Down USB) enumerate and
        // display devices beyond the boot chain, so connect the full tree and
        // set up consoles before them. A normal boot (no hotkey) skips both and
        // connects only its boot device path below.
        if hotkey != SreHotkey::None {
            if let Err(e) = helpers::connect_all(boot_services) {
                log::error!("connect_all (recovery) failed: {:?}", e);
            }
            if let Err(e) = helpers::discover_console_devices(boot_services, runtime_services) {
                log::error!("discover_console_devices failed: {:?}", e);
            }
        }

        match hotkey {
            SreHotkey::VolumeUp => match &self.sre_app_path {
                Some(path) => {
                    log::info!("SRE hotkey: Vol-Up -> dispatching SRE app at {:?}", path);
                    if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                        log::error!("signal_ready_to_boot (SRE dispatch) failed: {:?}", e);
                    }
                    match locked_boot.boot_from_device_path(image_handle, path) {
                        Ok(()) => log::warn!("SRE app returned control; falling through to Boot####"),
                        Err(e) => log::error!("SRE app boot_from_device_path failed: {:?}", e),
                    }
                }
                None => {
                    log::info!("SRE hotkey: Vol-Up -> running in-Rust bp_recovery flow (no sre_app_path set)");
                    if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                        log::error!("signal_ready_to_boot (bp_recovery) failed: {:?}", e);
                    }
                    match bp_recovery::run_sre_flow(&locked_boot, image_handle) {
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
                if let Some(usb_path) = find_first_usb_sfs_device_path(boot_services) {
                    log::info!(
                        "SRE hotkey: Vol-Down + USB present -> dispatching USB boot at {:?}",
                        usb_path
                    );
                    if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                        log::error!("signal_ready_to_boot (USB dispatch) failed: {:?}", e);
                    }
                    match locked_boot.boot_from_device_path(image_handle, &usb_path) {
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
                    match locked_boot.boot_from_device_path(image_handle, path) {
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

        // Optional BP1 SRE WIM fallback. Probed once before Boot#### iteration
        // so we can both filter USB entries (which would re-run the SRE
        // flashing tool that committed the WIM) and dispatch run_sre_flow as
        // the final fallback. Cost: one 512-byte LID 0x15 head read of BP1.
        let bp_has_sre = self.bp_sre_fallback && bp_recovery::bp_has_sre_payload(boot_services);

        // Try boot options in two passes. Pass 0 runs after the USB-skip
        // connect. If nothing boots (a boot targets a controller USB-skip left
        // unbound), pass 1 connects the full tree, including USB, and retries —
        // so a normal boot always succeeds. The constructor's `main_os_path` is
        // the fallback when discovery yields no entries.
        for pass in 0..2 {
            if pass == 1 {
                log::warn!("Boot exhausted after USB-skip connect; connecting full tree and retrying");
                if let Err(e) = helpers::connect_all(boot_services) {
                    log::error!("connect_all (fallback) failed: {:?}", e);
                }
            }

            let mut tried_any = false;
            match helpers::discover_boot_options(runtime_services) {
                Ok(boot_config) => {
                    for device_path in boot_config.devices() {
                        if bp_has_sre && device_path_has_usb_node(device_path) {
                            log::info!("Skipping USB Boot#### (BP1 has SRE payload); path={:?}", device_path);
                            continue;
                        }
                        if bp_has_sre && device_path_has_fw_file_node(device_path) {
                            log::info!("Skipping FwFile Boot#### (BP1 has SRE payload); path={:?}", device_path);
                            continue;
                        }
                        tried_any = true;
                        if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                            log::error!("signal_ready_to_boot failed: {:?}", e);
                        }
                        match locked_boot.boot_from_device_path(image_handle, device_path) {
                            Ok(()) => {
                                log::warn!("Boot option returned control (path={:?}), trying next...", device_path)
                            }
                            Err(e) => log::warn!("Boot option failed (path={:?}): {:?}", device_path, e),
                        }
                    }
                }
                Err(e) => log::error!("discover_boot_options failed: {:?}", e),
            }

            if !tried_any {
                if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                    log::error!("signal_ready_to_boot failed: {:?}", e);
                }
                match locked_boot.boot_from_device_path(image_handle, &self.main_os_path) {
                    Ok(()) => log::warn!("Main OS fallback returned control (path={:?})", self.main_os_path),
                    Err(e) => log::warn!("Main OS fallback failed (path={:?}): {:?}", self.main_os_path, e),
                }

                // No boot option was attempted (Boot#### discovery found
                // nothing or failed outright, e.g. NVRAM cleared by a full
                // flash) and the configured main OS path did not boot.
                // Enumerate filesystem volumes and try the standard OS loader
                // locations so the device still reaches the OS.
                match helpers::fallback_boot_options(boot_services, runtime_services, &self.boot_source_policy) {
                    Ok(candidates) => {
                        for device_path in candidates {
                            if bp_has_sre && device_path_has_usb_node(&device_path) {
                                log::info!(
                                    "Skipping USB fallback option (BP1 has SRE payload); path={:?}",
                                    device_path
                                );
                                continue;
                            }
                            match locked_boot.boot_from_device_path(image_handle, &device_path) {
                                Ok(()) => log::warn!(
                                    "Fallback option returned control (path={:?}), trying next...",
                                    device_path
                                ),
                                Err(e) => log::warn!("Fallback option failed (path={:?}): {:?}", device_path, e),
                            }
                        }
                    }
                    Err(e) => log::warn!("fallback_boot_options failed: {:?}", e),
                }
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
            match bp_recovery::run_sre_flow(&locked_boot, image_handle) {
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
    use alloc::{sync::Arc, vec, vec::Vec};
    use patina::{
        boot_services::MockBootServices,
        device_path::{node_defs::EndEntire, paths::DevicePathBuf},
    };
    use std::sync::Mutex;

    fn test_device_path() -> DevicePathBuf {
        DevicePathBuf::from_device_path_node_iter(core::iter::once(EndEntire))
    }

    #[test]
    fn test_new_constructs() {
        let _ = SreBootManager::new(test_device_path());
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
        let _: Arc<dyn BootOrchestrator> = Arc::new(SreBootManager::new(test_device_path()));
    }

    #[test]
    fn test_dfci_start_of_bds_precedes_work_and_security_transition() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let event_trace = Arc::clone(&trace);
        let work_trace = Arc::clone(&trace);
        let lock_trace = Arc::clone(&trace);
        let mut mock = MockBootServices::new();

        mock.expect_create_event_ex_unchecked::<()>()
            .times(2)
            .returning(move |_, _, _, _, group| {
                let phase = if group == &MS_START_OF_BDS_NOTIFY_GUID {
                    "ms-start-of-bds"
                } else if group == &DFCI_START_OF_BDS_NOTIFY_GUID {
                    "dfci-start-of-bds"
                } else {
                    panic!("unexpected event group: {group:?}");
                };
                event_trace.lock().unwrap().push(phase);
                Ok(core::ptr::null_mut())
            });
        mock.expect_signal_event().times(2).returning(|_| Ok(()));
        mock.expect_close_event().times(2).returning(|_| Ok(()));

        run_pre_lock_phase(
            &mock,
            true,
            || work_trace.lock().unwrap().push("pre-lock-work"),
            || lock_trace.lock().unwrap().push("security-transition"),
        );

        assert_eq!(
            *trace.lock().unwrap(),
            vec![
                "ms-start-of-bds",
                "dfci-start-of-bds",
                "pre-lock-work",
                "security-transition",
            ]
        );
    }

    // === Device-path classification helper tests ===
    //
    // Raw node byte streams (header = [type, sub_type, len_lo, len_hi]),
    // matching the UEFI spec node layouts.

    /// ACPI PciRoot node: type 2 (ACPI), sub 1, len 12.
    const NODE_PCI_ROOT: [u8; 12] = [0x02, 0x01, 0x0C, 0x00, 0xD0, 0x41, 0x03, 0x0A, 0x00, 0x00, 0x00, 0x00];
    /// Messaging/Usb node: type 3, sub 5, len 6.
    const NODE_USB: [u8; 6] = [0x03, 0x05, 0x06, 0x00, 0x00, 0x00];
    /// Messaging/UsbClass node: type 3, sub 15, len 11.
    const NODE_USB_CLASS: [u8; 11] = [0x03, 0x0F, 0x0B, 0x00, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF];
    /// Messaging/UsbWwid node: type 3, sub 16, len 10.
    const NODE_USB_WWID: [u8; 10] = [0x03, 0x10, 0x0A, 0x00, 0, 0, 0, 0, 0, 0];
    /// Messaging/Sata node: type 3, sub 18, len 10.
    const NODE_SATA: [u8; 10] = [0x03, 0x12, 0x0A, 0x00, 0, 0, 0xFF, 0xFF, 0, 0];
    /// Media/PiwgFirmwareFile node: type 4, sub 6, len 20 (header + GUID).
    const NODE_FW_FILE: [u8; 20] = [0x04, 0x06, 0x14, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    /// Media/PiwgFirmwareVolume node: type 4, sub 7, len 20.
    const NODE_FW_VOL: [u8; 20] = [0x04, 0x07, 0x14, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    /// Media/Vendor node: type 4, sub 3, len 20. Media type with a non-FW
    /// sub-type — exercises the sub-type check in the FW-file classifier.
    const NODE_MEDIA_VENDOR: [u8; 20] = [0x04, 0x03, 0x14, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    /// Media node whose sub-type collides with Messaging/Usb (type 4, sub 5) —
    /// exercises the type check in the USB classifier.
    const NODE_MEDIA_SUB5: [u8; 20] = [0x04, 0x05, 0x14, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    /// EndEntire node.
    const NODE_END: [u8; 4] = [0x7F, 0xFF, 0x04, 0x00];

    /// Concatenate `nodes` and terminate with EndEntire.
    fn synth_path(nodes: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for node in nodes {
            bytes.extend_from_slice(node);
        }
        bytes.extend_from_slice(&NODE_END);
        bytes
    }

    fn classify<F: Fn(&patina::device_path::paths::DevicePath) -> bool>(bytes: &[u8], f: F) -> bool {
        // SAFETY: synth_path produces well-formed, EndEntire-terminated node
        // streams; the reference does not outlive `bytes`.
        f(unsafe { patina::device_path::paths::DevicePath::try_from_ptr(bytes.as_ptr()) }.unwrap())
    }

    #[test]
    fn test_device_path_has_usb_node_classification() {
        let cases: &[(&str, Vec<u8>, bool)] = &[
            ("pci_root + usb", synth_path(&[&NODE_PCI_ROOT, &NODE_USB]), true),
            ("usb_class", synth_path(&[&NODE_PCI_ROOT, &NODE_USB_CLASS]), true),
            ("usb_wwid", synth_path(&[&NODE_USB_WWID]), true),
            ("sata only", synth_path(&[&NODE_PCI_ROOT, &NODE_SATA]), false),
            ("end only", synth_path(&[]), false),
            (
                "media node with usb sub-type value",
                synth_path(&[&NODE_MEDIA_SUB5]),
                false,
            ),
            ("usb after media", synth_path(&[&NODE_MEDIA_VENDOR, &NODE_USB]), true),
        ];
        for (name, bytes, expected) in cases {
            assert_eq!(classify(bytes, device_path_has_usb_node), *expected, "case: {name}");
        }
    }

    #[test]
    fn test_device_path_has_fw_file_node_classification() {
        let cases: &[(&str, Vec<u8>, bool)] = &[
            ("fw_vol + fw_file", synth_path(&[&NODE_FW_VOL, &NODE_FW_FILE]), true),
            ("fw_vol only", synth_path(&[&NODE_FW_VOL]), true),
            ("fw_file only", synth_path(&[&NODE_FW_FILE]), true),
            ("media vendor only", synth_path(&[&NODE_MEDIA_VENDOR]), false),
            ("messaging only", synth_path(&[&NODE_PCI_ROOT, &NODE_USB]), false),
            ("end only", synth_path(&[]), false),
        ];
        for (name, bytes, expected) in cases {
            assert_eq!(classify(bytes, device_path_has_fw_file_node), *expected, "case: {name}");
        }
    }
}
