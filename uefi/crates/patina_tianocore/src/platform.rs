//! Write-once platform registration that works on TianoCore and Patina native.
//!
//! The OEM implements [`Platform`] once using the [`ComponentAdder`],
//! [`ServiceAdder`], and [`ConfigAdder`] traits. The same implementation is
//! consumed by:
//!
//! - **TianoCore** — via `patina_tianocore::driver_entry!` which dispatches
//!   components on the TianoCore runtime.
//! - **Patina native** — via `patina_tianocore::impl_component_info!` which
//!   generates a `patina_dxe_core::ComponentInfo` impl that delegates to the
//!   same `Platform` methods.
//!
//! Since `patina_dxe_core::Add<Component>` and our internal adder types expose
//! methods with identical signatures, the adder traits abstract over both
//! and the OEM's registration code compiles against either backend unchanged.

use alloc::boxed::Box;
use alloc::vec::Vec;

use patina::component::service::IntoService;
use patina::component::{self, IntoComponent, Storage};

// ---------------------------------------------------------------------------
// Adder traits — the OEM writes against these
// ---------------------------------------------------------------------------

/// Accepts components for registration. Implemented internally for the
/// TianoCore dispatcher and externally for `patina_dxe_core::Add<Component>`
/// via the [`impl_component_info!`](crate::impl_component_info) macro.
pub trait ComponentAdder {
    /// Register a Patina component.
    fn component<I>(&mut self, component: impl IntoComponent<I>);
}

/// Accepts services for registration. Implemented internally for the
/// TianoCore dispatcher and externally for `patina_dxe_core::Add<Service>`
/// via the [`impl_component_info!`](crate::impl_component_info) macro.
pub trait ServiceAdder {
    /// Register a Patina service.
    fn service(&mut self, service: impl IntoService + 'static);
}

/// Accepts config values for registration. Implemented internally for the
/// TianoCore dispatcher and externally for `patina_dxe_core::Add<Config>`
/// via the [`impl_component_info!`](crate::impl_component_info) macro.
pub trait ConfigAdder {
    /// Register a configuration value.
    fn config<C: Default + 'static>(&mut self, config: C);
}

// ---------------------------------------------------------------------------
// Platform trait — the OEM implements this exactly once
// ---------------------------------------------------------------------------

/// Define your platform's components, services, and configs.
///
/// This is the **single definition** that drives both TianoCore dispatch
/// and native Patina dispatch. You never need to rewrite these methods.
///
/// # Example
///
/// ```rust,ignore
/// use patina_tianocore::prelude::*;
/// use patina_smbios::component::SmbiosProvider;
///
/// pub struct OemPlatform;
///
/// impl Platform for OemPlatform {
///     fn components(add: &mut impl ComponentAdder) {
///         add.component(SmbiosProvider::new(3, 9));
///     }
///
///     fn configs(add: &mut impl ConfigAdder) {
///         add.config(42u32);
///     }
/// }
///
/// // On TianoCore:
/// patina_tianocore::driver_entry!(platform: OemPlatform);
///
/// // On Patina native (in patina-dxe-core-oem):
/// patina_tianocore::impl_component_info!(OemPlatform);
/// ```
pub trait Platform {
    /// Register components to be dispatched.
    fn components(_add: &mut impl ComponentAdder) {}

    /// Register services into storage.
    fn services(_add: &mut impl ServiceAdder) {}

    /// Register configuration values.
    fn configs(_add: &mut impl ConfigAdder) {}
}

// ---------------------------------------------------------------------------
// TianoCore-side adder implementations
// ---------------------------------------------------------------------------

// Internal TianoCore-side adder implementations. These are not part of the
// public API — drivers go through `Platform` + `driver_entry!`, which set up
// these bridges. The Patina-native side uses the `impl_component_info!`
// macro which constructs its own adapter types over `patina_dxe_core::Add`.

pub(crate) struct ComponentList {
    pub(crate) components: Vec<Box<dyn component::Component>>,
}

impl ComponentList {
    pub(crate) fn new() -> Self {
        Self { components: Vec::new() }
    }
}

impl ComponentAdder for ComponentList {
    fn component<I>(&mut self, component: impl IntoComponent<I>) {
        self.components.push(component.into_component());
    }
}

pub(crate) struct ServiceList<'a> {
    pub(crate) storage: &'a mut Storage,
}

impl ServiceAdder for ServiceList<'_> {
    fn service(&mut self, service: impl IntoService + 'static) {
        self.storage.add_service(service);
    }
}

pub(crate) struct ConfigList<'a> {
    pub(crate) storage: &'a mut Storage,
}

impl ConfigAdder for ConfigList<'_> {
    fn config<C: Default + 'static>(&mut self, config: C) {
        self.storage.add_config::<C>(config);
    }
}

// ---------------------------------------------------------------------------
// Bridge macro for Patina native
// ---------------------------------------------------------------------------

/// Generates a `patina_dxe_core::ComponentInfo` impl that delegates to your
/// [`Platform`] impl.
///
/// Call this in your `patina-dxe-core-<platform>` crate to reuse the same
/// component registrations you wrote for TianoCore — zero changes needed.
///
/// # Example
///
/// ```rust,ignore
/// // In patina-dxe-core-oem/src/lib.rs:
/// use patina_dxe_core::*;
///
/// // OemPlatform already implements patina_tianocore::Platform
/// patina_tianocore::impl_component_info!(OemPlatform);
///
/// impl MemoryInfo for OemPlatform { /* ... */ }
/// impl CpuInfo for OemPlatform { /* ... */ }
///
/// impl PlatformInfo for OemPlatform {
///     type MemoryInfo = Self;
///     type CpuInfo = Self;
///     type ComponentInfo = Self;  // ← ComponentInfo is auto-generated above
///     type Extractor = /* ... */;
/// }
///
/// static CORE: Core<OemPlatform> = Core::new(/* ... */);
/// ```
#[macro_export]
macro_rules! impl_component_info {
    ($platform:ty) => {
        // Anonymous const scope keeps the bridge adapter types private to
        // this expansion site. Multiple invocations of the macro in the
        // same module (rare, but possible in tests) won't collide on the
        // bridge type names. The `impl ComponentInfo for $platform` leaks
        // out to the outer scope as intended.
        const _: () = {
            struct PtComponentBridge<'a>(::patina_dxe_core::Add<'a, ::patina_dxe_core::Component>);

            impl $crate::ComponentAdder for PtComponentBridge<'_> {
                fn component<I>(&mut self, component: impl ::patina::component::IntoComponent<I>) {
                    self.0.component(component);
                }
            }

            struct PtServiceBridge<'a>(::patina_dxe_core::Add<'a, ::patina_dxe_core::Service>);

            impl $crate::ServiceAdder for PtServiceBridge<'_> {
                fn service(&mut self, service: impl ::patina::component::service::IntoService + 'static) {
                    self.0.service(service);
                }
            }

            struct PtConfigBridge<'a>(::patina_dxe_core::Add<'a, ::patina_dxe_core::Config>);

            impl $crate::ConfigAdder for PtConfigBridge<'_> {
                fn config<C: Default + 'static>(&mut self, config: C) {
                    self.0.config(config);
                }
            }

            impl ::patina_dxe_core::ComponentInfo for $platform {
                fn components(add: ::patina_dxe_core::Add<'_, ::patina_dxe_core::Component>) {
                    <$platform as $crate::Platform>::components(&mut PtComponentBridge(add));
                }

                fn services(add: ::patina_dxe_core::Add<'_, ::patina_dxe_core::Service>) {
                    <$platform as $crate::Platform>::services(&mut PtServiceBridge(add));
                }

                fn configs(add: ::patina_dxe_core::Add<'_, ::patina_dxe_core::Config>) {
                    <$platform as $crate::Platform>::configs(&mut PtConfigBridge(add));
                }
            }
        };
    };
}
