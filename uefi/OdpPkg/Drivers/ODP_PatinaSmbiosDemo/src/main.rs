//! ODP Patina SMBIOS Demonstration Driver
//!
//! Shows how to publish an SMBIOS record from a UEFI driver that uses ONLY
//! the native Patina component model — no `EFI_SMBIOS_PROTOCOL`, no raw
//! buffer construction, no `unsafe` boilerplate.
//!
//! This file is the entire TianoCore driver shell. The Platform definition
//! lives in `platform.rs` next to it; the bridge crate `patina_tianocore`
//! handles the rest (allocator, panic handler, `efi_main`, dispatcher,
//! memory manager wiring).
//!
//! ## How the same code runs on Patina native
//!
//! `platform.rs` is intentionally portable. To run the same component
//! registrations on a Patina-native DXE core, you'd write a sibling crate
//! that contains:
//!
//! ```rust,ignore
//! patina_tianocore::impl_component_info!(my_platform::OemPlatform);
//! impl MemoryInfo for OemPlatform { /* ... */ }
//! impl CpuInfo  for OemPlatform { /* ... */ }
//! impl PlatformInfo for OemPlatform { /* ... */ }
//! ```
//!
//! The `Platform` impl in `platform.rs` is unchanged.

#![no_std]
#![no_main]
#![feature(allocator_api)]

mod platform;

// One line generates the global allocator, panic handler, `efi_main` entry
// point, boot-services + memory-manager wiring, and dispatches every
// component registered by `OemPlatform`.
patina_tianocore::driver_entry!(platform: platform::OemPlatform);
