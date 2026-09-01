//! Event-group signaling helper shared across the SRE boot path.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
use patina::{error::EfiError, uefi::boot_services::BootServices};
use r_efi::efi;

/// Create a one-shot NOTIFY_SIGNAL event tied to `group_guid`, fire it, then
/// close it — broadcasting a notification to whichever DXE drivers registered
/// a callback against the group.
pub(crate) fn signal_event_group<B: BootServices>(
    boot_services: &B,
    group_guid: &'static efi::Guid,
) -> patina::error::Result<()> {
    use patina::uefi::{boot_services::tpl::Tpl, event::EventType};

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
