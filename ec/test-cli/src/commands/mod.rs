//! CLI EC Test tool subcommand handler modules
//!
//! SPDX-License-Identifier: MIT
//!

pub mod battery;
#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
pub mod eval;
pub mod rtc;
pub mod script;
pub mod thermal;
