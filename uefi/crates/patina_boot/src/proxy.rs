//! C FFI proxy bindings for UefiBootManagerLib primitives.
//!
//! `PatinaBootMgrLibProxy` is a thin C DXE driver that publishes a
//! vtable of `EfiBootManager*` and EndOfDxe primitives as an EFI
//! protocol. This module dispatches through that protocol so Rust
//! callers don't have to re-implement (or statically link against)
//! the C `UefiBootManagerLib` set.
//!
//! Each wrapper here is a one-shot `LocateProtocol` + function pointer
//! call. Cache the protocol pointer at the call site if invoking
//! repeatedly in a hot loop.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
extern crate alloc;

use core::ffi::c_void;

use patina::{error::EfiError, uefi::boot_services::BootServices};
use r_efi::efi;

/// `gPatinaBootMgrLibProxyProtocolGuid` — matches the C header.
pub const PROTOCOL_GUID: efi::Guid = efi::Guid::from_fields(
    0x9E5B1A40,
    0x7C42,
    0x4E91,
    0xB8,
    0xD6,
    &[0x3F, 0x2A, 0x8E, 0x5D, 0x7C, 0x13],
);

/// Current revision. Bump in lockstep with the C header when the
/// vtable layout changes; consumers should refuse to dispatch if the
/// major revision doesn't match.
pub const REVISION: u32 = 0x0001_0002;

/// Mirrors EDK2's `CONSOLE_TYPE` enum (UefiBootManagerLib).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleType {
    ConIn = 0,
    ConOut = 1,
    ErrOut = 2,
}

/// vtable as published by the C `PatinaBootMgrLibProxy` DXE driver.
///
/// Field order MUST match `_PATINA_BOOT_MGR_LIB_PROXY_PROTOCOL` in
/// the platform's `PatinaBootMgrLibProxy.h`.
#[repr(C)]
pub struct Protocol {
    pub revision: u32,
    pub process_capsules: unsafe extern "efiapi" fn() -> efi::Status,
    pub connect_all_default_consoles: unsafe extern "efiapi" fn() -> efi::Status,
    pub connect_console_variable: unsafe extern "efiapi" fn(console_type: ConsoleType) -> efi::Status,
    pub update_console_variable: unsafe extern "efiapi" fn(
        console_type: ConsoleType,
        customized_dp: *mut efi::protocols::device_path::Protocol,
        exclusive_dp: *mut efi::protocols::device_path::Protocol,
    ) -> efi::Status,
    pub start_hotkey_service: unsafe extern "efiapi" fn(hotkey_triggered: *mut efi::Event) -> efi::Status,
    pub dispatch_deferred_images: unsafe extern "efiapi" fn() -> efi::Status,
    /// Pass an opaque pointer to `EFI_BOOT_MANAGER_LOAD_OPTION`. Layout
    /// is complex; for the first cut we don't model it in Rust — callers
    /// receive a pointer from the C side via another path and pass it
    /// through. Returning to Rust-native modeling is future work.
    pub process_load_option: unsafe extern "efiapi" fn(load_option: *mut c_void) -> efi::Status,
    pub install_dxe_smm_ready_to_lock: unsafe extern "efiapi" fn() -> efi::Status,
    /// Promote present-but-untested system memory (classified `Reserved`
    /// by the DXE Core) to usable system memory via
    /// `EFI_GENERIC_MEMORY_TEST_PROTOCOL`. Added in revision 0x0001_0001.
    pub perform_memory_test: unsafe extern "efiapi" fn() -> efi::Status,
    /// Draw the OEM system boot logo and register its location with the Boot
    /// Logo protocol (`BootGraphicsLib::DisplayBootGraphic(BG_SYSTEM_LOGO)`) so
    /// `DisplayUpdateProgress` can render the firmware-update progress bar.
    /// Requires GOP connected. Added in revision 0x0001_0002.
    pub display_boot_logo: unsafe extern "efiapi" fn() -> efi::Status,
}

/// Locate the proxy protocol and return a borrowed reference. Returns
/// [`EfiError::NotFound`] if the C proxy DXE driver isn't in the FV
/// (platforms that don't ship it are expected to fall back to native
/// Rust paths or to skip the call gracefully).
fn locate<B: BootServices>(boot_services: &B) -> Result<&'static Protocol, EfiError> {
    use core::ptr;
    // SAFETY: PROTOCOL_GUID is 'static; the returned pointer is owned
    // by the proxy driver and lives for the lifetime of the firmware
    // boot phase, which outlives any single helper call.
    let ptr =
        unsafe { boot_services.locate_protocol_unchecked(&PROTOCOL_GUID, ptr::null_mut()) }.map_err(EfiError::from)?;
    if ptr.is_null() {
        return Err(EfiError::NotFound);
    }
    // SAFETY: `ptr` is non-null (checked above) and points to a `Protocol` installed by the proxy driver, live for the boot phase.
    let protocol = unsafe { &*(ptr as *const Protocol) };
    if (protocol.revision >> 16) != (REVISION >> 16) {
        log::error!(
            "PatinaBootMgrLibProxy revision mismatch: expected {:#x}, got {:#x}",
            REVISION >> 16,
            protocol.revision >> 16
        );
        return Err(EfiError::Unsupported);
    }
    Ok(protocol)
}

fn check<B: BootServices>(boot_services: &B, status: efi::Status) -> Result<(), EfiError> {
    let _ = boot_services;
    if status == efi::Status::SUCCESS {
        Ok(())
    } else {
        Err(EfiError::from(status))
    }
}

/// `EfiBootManagerProcessCapsules` — process any capsules staged for
/// "process on next boot". Required for firmware-update capsules to
/// actually apply.
pub fn process_capsules<B: BootServices>(boot_services: &B) -> Result<(), EfiError> {
    let proxy = locate(boot_services)?;
    // SAFETY: `proxy` is validated by `locate()` (non-null, matching major revision); the field is a valid C `efiapi` function pointer installed by the proxy driver.
    let status = unsafe { (proxy.process_capsules)() };
    check(boot_services, status)
}

/// `EfiBootManagerConnectAllDefaultConsoles` — connects every console
/// (ConIn, ConOut, ErrOut) per its NVRAM variable. Drives the
/// `ConSplitter` invocation that publishes `gST->ConsoleOutHandle`,
/// which in turn lets the platform's Simple Window Manager install.
pub fn connect_all_default_consoles<B: BootServices>(boot_services: &B) -> Result<(), EfiError> {
    let proxy = locate(boot_services)?;
    // SAFETY: `proxy` is validated by `locate()` (non-null, matching major revision); the field is a valid C `efiapi` function pointer installed by the proxy driver.
    let status = unsafe { (proxy.connect_all_default_consoles)() };
    check(boot_services, status)
}

/// Promote present-but-untested system memory to usable system memory.
/// The DXE Core classifies system-memory HOBs that carry only
/// `PRESENT`/`INITIALIZED` (not `TESTED`) attributes as `Reserved`, which the
/// OS cannot use. This drives `EFI_GENERIC_MEMORY_TEST_PROTOCOL` (from
/// `NullMemoryTestDxe`) to convert those regions to `SystemMemory`.
///
/// Requires proxy [`REVISION`] >= 0x0001_0001.
pub fn perform_memory_test<B: BootServices>(boot_services: &B) -> Result<(), EfiError> {
    let proxy = locate(boot_services)?;
    if proxy.revision < 0x0001_0001 {
        // Older proxy driver without the memory-test entry; skip rather
        // than dispatch through an out-of-bounds vtable slot.
        return Err(EfiError::Unsupported);
    }
    // SAFETY: `proxy` is validated by `locate()` (non-null, matching major revision); the field is a valid C `efiapi` function pointer installed by the proxy driver.
    let status = unsafe { (proxy.perform_memory_test)() };
    check(boot_services, status)
}

/// Draw the OEM system boot logo and register its location with the Boot Logo
/// protocol (`BootGraphicsLib::DisplayBootGraphic(BG_SYSTEM_LOGO)`), so that
/// `DisplayUpdateProgress` can render the firmware-update progress bar during
/// capsule processing. Call after controllers are connected (GOP present).
///
/// Requires proxy [`REVISION`] >= 0x0001_0002.
pub fn display_boot_logo<B: BootServices>(boot_services: &B) -> Result<(), EfiError> {
    let proxy = locate(boot_services)?;
    if proxy.revision < 0x0001_0002 {
        // Older proxy driver without the boot-logo entry; skip rather than
        // dispatch through an out-of-bounds vtable slot.
        return Err(EfiError::Unsupported);
    }
    // SAFETY: `proxy` is validated by `locate()` (non-null, matching major revision); the field is a valid C `efiapi` function pointer installed by the proxy driver.
    let status = unsafe { (proxy.display_boot_logo)() };
    check(boot_services, status)
}

/// `EfiBootManagerConnectConsoleVariable` — connect a single console
/// variable (ConIn/ConOut/ErrOut).
pub fn connect_console_variable<B: BootServices>(boot_services: &B, console_type: ConsoleType) -> Result<(), EfiError> {
    let proxy = locate(boot_services)?;
    // SAFETY: `proxy` is validated by `locate()` (non-null, matching major revision); the field is a valid C `efiapi` function pointer installed by the proxy driver.
    let status = unsafe { (proxy.connect_console_variable)(console_type) };
    check(boot_services, status)
}

/// `EfiBootManagerUpdateConsoleVariable` — append or exclude a device
/// path from a console variable. `customized` is added (if non-null);
/// `exclusive` is removed (if non-null).
///
/// # Safety
///
/// Device path pointers must remain valid for the duration of the call.
pub unsafe fn update_console_variable<B: BootServices>(
    boot_services: &B,
    console_type: ConsoleType,
    customized: *mut efi::protocols::device_path::Protocol,
    exclusive: *mut efi::protocols::device_path::Protocol,
) -> Result<(), EfiError> {
    let proxy = locate(boot_services)?;
    // SAFETY: `proxy` is validated by `locate()` (non-null, matching major revision); the field is a valid C `efiapi` function pointer installed by the proxy driver.
    let status = unsafe { (proxy.update_console_variable)(console_type, customized, exclusive) };
    check(boot_services, status)
}

/// `EfiBootManagerStartHotkeyService` — install the standard BDS
/// hotkey listener. The returned event signals when a registered
/// hotkey is pressed during the boot menu timeout.
pub fn start_hotkey_service<B: BootServices>(boot_services: &B) -> Result<efi::Event, EfiError> {
    let proxy = locate(boot_services)?;
    let mut hotkey: efi::Event = core::ptr::null_mut();
    // SAFETY: `proxy` is validated by `locate()` (non-null, matching major revision); the field is a valid C `efiapi` function pointer installed by the proxy driver.
    let status = unsafe { (proxy.start_hotkey_service)(&mut hotkey) };
    check(boot_services, status)?;
    Ok(hotkey)
}

/// `EfiBootManagerDispatchDeferredImages` — dispatch DXE images that
/// were deferred (FFS sections with deferred-dispatch attribute).
/// Some platforms have deferred drivers for networking, MM,
/// and platform-init paths that won't run without this call.
pub fn dispatch_deferred_images<B: BootServices>(boot_services: &B) -> Result<(), EfiError> {
    let proxy = locate(boot_services)?;
    // SAFETY: `proxy` is validated by `locate()` (non-null, matching major revision); the field is a valid C `efiapi` function pointer installed by the proxy driver.
    let status = unsafe { (proxy.dispatch_deferred_images)() };
    check(boot_services, status)
}

/// `EfiBootManagerProcessLoadOption` — LoadImage + StartImage on a
/// `Boot####`-style load option with full event-signal sequencing
/// (ReadyToBoot, etc.).
///
/// # Safety
///
/// `load_option` must point to a fully-initialized
/// `EFI_BOOT_MANAGER_LOAD_OPTION` struct owned by the caller.
pub unsafe fn process_load_option<B: BootServices>(
    boot_services: &B,
    load_option: *mut c_void,
) -> Result<(), EfiError> {
    let proxy = locate(boot_services)?;
    // SAFETY: `proxy` is validated by `locate()` (non-null, matching major revision); the field is a valid C `efiapi` function pointer installed by the proxy driver.
    let status = unsafe { (proxy.process_load_option)(load_option) };
    check(boot_services, status)
}

/// Install `gEfiDxeSmmReadyToLockProtocolGuid`. C BdsDxe does this in
/// `ExitPmAuth` immediately after signaling EndOfDxe; the
/// install drives the SMM lockdown chain. Without it, SMM is left
/// in an unlocked state — both a security concern and a source of
/// driver state inconsistency downstream.
pub fn install_dxe_smm_ready_to_lock<B: BootServices>(boot_services: &B) -> Result<(), EfiError> {
    let proxy = locate(boot_services)?;
    // SAFETY: `proxy` is validated by `locate()` (non-null, matching major revision); the field is a valid C `efiapi` function pointer installed by the proxy driver.
    let status = unsafe { (proxy.install_dxe_smm_ready_to_lock)() };
    check(boot_services, status)
}
