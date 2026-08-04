//! Boot-mode flag recorded by the platform binary at core entry.
//!
//! The boot mode is published by the earlier phase in the PHIT HOB. That HOB is
//! a standard (non-GUID) HOB, so it is not reachable through the component
//! [`Hob`](patina::component::hob::Hob) injection path. The platform binary
//! reads it once at core entry and records it here. The flag is an optional
//! optimization for boot-time work that is only useful on a flash-update boot
//! (e.g. drawing the capsule progress-bar logo); it does not gate correctness,
//! so a platform that never records it simply skips that work.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
use core::sync::atomic::{AtomicBool, Ordering};

static FLASH_UPDATE_BOOT: AtomicBool = AtomicBool::new(false);

/// Record whether the current boot is a flash-update (capsule) boot. The
/// platform binary calls this once at core entry with the boot mode read from
/// the PHIT HOB.
///
/// Uses `Release` ordering to publish the value; it pairs with the `Acquire`
/// load in [`is_flash_update_boot`]. `swap` returns the prior value, so a
/// redundant call (the value is meant to be recorded exactly once) trips a
/// debug assertion.
pub fn set_flash_update_boot(is_flash_update: bool) {
    let previous = FLASH_UPDATE_BOOT.swap(is_flash_update, Ordering::Release);
    debug_assert!(!previous, "set_flash_update_boot called more than once");
}

/// Whether a flash-update boot was recorded via [`set_flash_update_boot`].
/// Returns `false` until the platform records a value. `Acquire` ordering
/// pairs with the `Release` store in [`set_flash_update_boot`].
pub fn is_flash_update_boot() -> bool {
    FLASH_UPDATE_BOOT.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_false_then_records_boot_mode() {
        // The flag defaults to false and reads back the recorded value. It is
        // recorded exactly once (set_flash_update_boot debug-asserts on a
        // second call), so this sets it a single time.
        assert!(!is_flash_update_boot());
        set_flash_update_boot(true);
        assert!(is_flash_update_boot());
    }
}
