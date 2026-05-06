//! Glob-importable bridge-crate types.
//!
//! Only types defined by this crate are exposed here. Patina SDK types
//! (`Storage`, `EfiError`, the component prelude, etc.) must be imported
//! explicitly from `patina` so the dependency on the SDK is visible at
//! every use site.

pub use crate::context::{DriverContext, DriverResult};
pub use crate::platform::{ComponentAdder, ConfigAdder, Platform, ServiceAdder};
