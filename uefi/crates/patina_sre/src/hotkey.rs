//! Hotkey-source abstraction.
//!
//! SPDX-License-Identifier: MIT
//!
//! [`HotkeySource`] lets [`crate::SreBootManager`] decide between the normal boot path and the
//! SRE-recovery path without depending on any specific input device. Platforms wire their
//! hotkey hardware (e.g. Surface's `MsButtonServicesProtocol`) through an implementation of this
//! trait; tests and headless QEMU runs use [`AlwaysSre`] or [`NeverSre`].

/// Source of SRE-entry hotkey signal.
///
/// Implementors poll their underlying hotkey hardware and return whether the SRE-entry
/// gesture (e.g. Power + Volume Up on Surface) is currently active. The poll is invoked at
/// most once per boot, immediately before the orchestrator chooses between the normal boot
/// path and the SRE-recovery path.
pub trait HotkeySource {
    /// Returns `true` if the SRE-entry hotkey is currently active.
    fn sre_requested(&self) -> bool;
}

/// `HotkeySource` impl that always reports the SRE hotkey as pressed.
///
/// Useful for forcing the SRE-recovery path under QEMU / unit tests without a real input
/// device, and for headless integration runs that always need to validate the SRE flow.
pub struct AlwaysSre;

impl HotkeySource for AlwaysSre {
    fn sre_requested(&self) -> bool {
        true
    }
}

/// `HotkeySource` impl that always reports the SRE hotkey as released.
///
/// Use this in production builds that don't yet have hotkey hardware wired up — the
/// orchestrator will always take the normal boot path.
pub struct NeverSre;

impl HotkeySource for NeverSre {
    fn sre_requested(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_sre_returns_true() {
        assert!(AlwaysSre.sre_requested());
    }

    #[test]
    fn never_sre_returns_false() {
        assert!(!NeverSre.sre_requested());
    }
}
