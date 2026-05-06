//! Re-exports used by the `driver_entry!` macro at expansion sites.
//!
//! Not part of the public API — the module is `#[doc(hidden)]` and may be
//! removed or restructured without a semver bump. Consumers should never
//! reference these paths directly.

pub use r_efi::efi::Status;
pub use r_efi::system::SystemTable;
