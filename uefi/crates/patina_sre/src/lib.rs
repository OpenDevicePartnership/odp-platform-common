//! System Recovery Environment boot orchestrator for Patina firmware.
//!
//! See the crate [README](https://github.com/OpenDevicePartnership/odp-platform-common/tree/main/uefi/crates/patina_sre)
//! for the full boot-path description and follow-up issues.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
#![cfg_attr(not(test), no_std)]
#![feature(coverage_attribute)]
#![feature(never_type)]

extern crate alloc;

mod bp_recovery;
mod sre_boot_manager;

pub use sre_boot_manager::{SreBootManager, SreHotkey, fv_file_device_path, fv_volume_file_device_path};

// Re-export the patina device-path types SreBootManager::new consumes, so callers
// (e.g. surface_patina_intel/patina_bin) don't need to depend on the same patina
// source patina_sre uses. Constructing DevicePathBuf via these re-exports
// guarantees type identity with SreBootManager's constructor signature.
pub use patina::device_path::node_defs::EndEntire;
pub use patina::device_path::paths::DevicePathBuf;
