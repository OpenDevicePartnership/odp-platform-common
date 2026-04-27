//! # patina_tianocore
//!
//! A bridge crate that lets UEFI drivers target **TianoCore** and **Patina**
//! through a single API surface — write once, run on either.
//!
//! ## The Problem
//!
//! Today the OEM migration path requires two steps:
//!
//! 1. Port a C UEFI driver to Rust targeting raw TianoCore (`r-efi`).
//! 2. Rewrite that Rust driver *again* to use Patina's component/service model.
//!
//! ## The Solution
//!
//! Define your platform **once** via the [`Platform`] trait:
//!
//! ```rust,ignore
//! use patina_tianocore::prelude::*;
//!
//! pub struct OemPlatform;
//!
//! impl Platform for OemPlatform {
//!     fn components(add: &mut impl ComponentAdder) {
//!         add.component(SmbiosProvider::new(3, 9));
//!         add.component(MyCustomDriver::default());
//!     }
//!
//!     fn configs(add: &mut impl ConfigAdder) {
//!         add.config(42u32);
//!     }
//! }
//! ```
//!
//! This single `impl` drives both runtimes:
//!
//! - **TianoCore today:**
//!   ```rust,ignore
//!   patina_tianocore::driver_entry!(platform: OemPlatform);
//!   ```
//!
//! - **Patina native later** (in `patina-dxe-core-oem`):
//!   ```rust,ignore
//!   patina_tianocore::impl_component_info!(OemPlatform);
//!   // That's it — ComponentInfo is auto-generated from Platform.
//!   // Then add the platform-specific traits:
//!   impl PlatformInfo for OemPlatform { /* MemoryInfo, CpuInfo, Extractor */ }
//!   ```
//!
//! The component structs, services, configs — everything inside the `Platform`
//! methods — transfers with **zero changes**.
//!
//! ## Feature Flags
//!
//! | Feature       | Description |
//! |---------------|-------------|
//! | `tianocore`   | *(default)* Back all abstractions with TianoCore / `r-efi`. The only currently implemented runtime. |
//!
//! A `patina-native` feature for the Patina-native runtime will be added when
//! the corresponding code paths land.

#![cfg_attr(not(test), no_std)]
#![feature(allocator_api)]
#![cfg_attr(feature = "tianocore", allow(non_snake_case))]
#![deny(missing_docs)]

extern crate alloc;

pub mod boot_services;
pub mod context;
pub mod entry;
#[cfg(feature = "tianocore")]
pub mod logger;
#[doc(hidden)]
pub mod macro_support;
pub mod memory;
pub mod platform;
pub mod prelude;

pub use context::{DriverContext, DriverResult};
pub use platform::{ComponentAdder, ConfigAdder, Platform, ServiceAdder};
