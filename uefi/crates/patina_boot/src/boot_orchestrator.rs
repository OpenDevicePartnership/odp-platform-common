//! Boot orchestrator trait definition.
//!
//! Defines the [`BootOrchestrator`] trait that platforms implement to customize
//! boot behavior. The [`BootDispatcher`](crate::BootDispatcher) component holds
//! a `Box<dyn BootOrchestrator>` and delegates to it when the DXE core invokes
//! the BDS architectural protocol.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
use core::convert::Infallible;
use patina::{
    component::service::dxe_dispatch::DxeDispatch, error::EfiError, uefi::boot_services::StandardBootServices,
    uefi::runtime_services::StandardRuntimeServices,
};
use r_efi::efi;

/// Trait for boot orchestration.
///
/// Platforms implement this trait to define custom boot flows. The implementation
/// is passed to [`BootDispatcher::new()`](crate::BootDispatcher::new) and invoked
/// when the DXE core calls the BDS architectural protocol entry point.
///
/// ## Built-in Implementation
///
/// [`SimpleBootManager`](crate::SimpleBootManager) provides a default implementation
/// for platforms with straightforward boot topologies (primary/secondary devices,
/// optional hotkey).
///
/// ## Custom Implementation
///
/// ```rust,ignore
/// use patina_boot::BootOrchestrator;
///
/// struct MyCustomBoot { /* ... */ }
///
/// impl BootOrchestrator for MyCustomBoot {
///     fn execute(
///         &self,
///         boot_services: &StandardBootServices,
///         runtime_services: &StandardRuntimeServices,
///         dxe_services: &dyn DxeDispatch,
///         image_handle: efi::Handle,
///     ) -> Result<Infallible, EfiError> {
///         // Custom boot flow...
///         // Return Err if all boot options are exhausted
///         Err(EfiError::NotFound)
///     }
/// }
/// ```
pub trait BootOrchestrator: Send + Sync + 'static {
    /// Execute the boot flow.
    ///
    /// Called by [`BootDispatcher`](crate::BootDispatcher) when the DXE core invokes
    /// the BDS architectural protocol. This method should:
    ///
    /// 1. Enumerate devices (e.g., `connect_all()`)
    /// 2. Complete the fail-closed EndOfDxe and DxeSmmReadyToLock transition
    /// 3. Signal ReadyToBoot before boot attempts
    /// 4. Attempt to boot from configured device paths
    /// 5. Handle boot failures
    ///
    /// A successful boot transfers control to the boot image and never returns.
    /// If all boot options are exhausted, the implementation returns
    /// `Err(EfiError)`. The `Ok` variant is [`Infallible`], a type that has no
    /// values, enforcing at the type level that this method can only "succeed"
    /// by not returning.
    fn execute(
        &self,
        boot_services: &StandardBootServices,
        runtime_services: &StandardRuntimeServices,
        dxe_services: &dyn DxeDispatch,
        image_handle: efi::Handle,
    ) -> Result<Infallible, EfiError>;
}
