//! CLI EC Test tool subcommand handler modules
//!
//! SPDX-License-Identifier: MIT
//!

pub mod battery;
#[cfg(target_os = "windows")]
pub mod eval;
pub mod rtc;
pub mod script;
pub mod thermal;
