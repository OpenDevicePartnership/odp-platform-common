//! Capsule-queue processing hook.
//!
//! [`process`] signals the MU capsule processor on every call. The processor
//! arms its drain callback only when the capsule queue is non-empty and the boot
//! mode is a flash update, so signaling is a no-op with an empty queue or on a
//! normal boot; on a flash-update boot with a queued capsule it drains the queue
//! and cold-resets from inside the signal, so [`process`] does not return.
//!
//! The OEM boot logo (the update progress-bar backdrop) is drawn only on a
//! flash-update boot, gated on the boot mode the platform binary records — the
//! expensive GOP blt is not paid on a normal boot. Capsule correctness does not
//! depend on that flag: a platform that never records the boot mode still gets
//! the signal, just no progress bar.
//!
//! Call before EndOfDxe, while the flash is still writable, and after the device
//! tree is connected so the firmware-management protocols and GOP are present.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
use patina::uefi::boot_services::BootServices;
use patina_boot::{boot_mode, proxy};
use r_efi::efi;

use crate::events::signal_event_group;

/// `gMuReadyToProcessCapsulesNotifyGuid` from `MsCorePkg.dec`. The MU capsule
/// processor (`SecuredCoreCapsuleProcessorDxe`) arms a callback on it only when
/// its capsule queue is non-empty, drains the queue (applying FMP capsules),
/// and cold-resets. With an empty queue no callback is registered, so the
/// signal is a no-op. `static` (not `const`) so `&MU_READY_TO_PROCESS_CAPSULES_NOTIFY_GUID`
/// is naturally `&'static efi::Guid`.
static MU_READY_TO_PROCESS_CAPSULES_NOTIFY_GUID: efi::Guid = efi::Guid::from_fields(
    0x2ab1c860,
    0xe697,
    0x4ede,
    0x8c,
    0x0f,
    &[0x65, 0xcd, 0x6e, 0x44, 0x44, 0x35],
);

/// Signal the capsule processor; draw the progress-bar logo on a flash-update
/// boot.
///
/// The logo is drawn only when [`boot_mode::is_flash_update_boot`] is set
/// (best-effort — no proxy or no GOP just means no progress bar). The capsule
/// processor is signaled unconditionally: it is a no-op with an empty queue or
/// on a normal boot, and on a flash-update boot with a queued capsule it drains
/// the queue and cold-resets, so this does not return.
pub fn process<B: BootServices>(boot_services: &B) {
    if boot_mode::is_flash_update_boot()
        && let Err(e) = proxy::display_boot_logo(boot_services)
    {
        log::warn!("proxy::display_boot_logo failed (no progress bar): {:?}", e);
    }

    if let Err(e) = signal_event_group(boot_services, &MU_READY_TO_PROCESS_CAPSULES_NOTIFY_GUID) {
        log::error!("signal gMuReadyToProcessCapsulesNotifyGuid failed: {:?}", e);
    }
}
