//! Integration test: exercises the full dispatch loop on the host target.
//!
//! Uses `DriverContext::from_storage()` to run without a real UEFI system table.
//! This proves that `Platform` → `DriverContext::dispatch_platform` works end-to-end.

#![feature(allocator_api)]

use patina::component::Storage;
use patina_tianocore::prelude::*;

// ---------------------------------------------------------------------------
// Minimal components for testing dispatch
// ---------------------------------------------------------------------------

/// A zero-dependency component that dispatches immediately.
struct SimpleComponent;

#[patina::component::component]
impl SimpleComponent {
    pub fn entry_point(self) -> patina::error::Result<()> {
        Ok(())
    }
}

/// A component that registers a service.
struct ProducerComponent;

#[patina::component::component]
impl ProducerComponent {
    pub fn entry_point(self, storage: &mut Storage) -> patina::error::Result<()> {
        storage.add_service(TestServiceImpl { value: 42 });
        Ok(())
    }
}

/// A component that consumes the service from ProducerComponent.
struct ConsumerComponent;

#[patina::component::component]
impl ConsumerComponent {
    pub fn entry_point(self, svc: patina::component::service::Service<dyn TestService>) -> patina::error::Result<()> {
        assert_eq!(svc.get_value(), 42);
        Ok(())
    }
}

/// A component that reads a config value.
struct ConfigComponent;

#[patina::component::component]
impl ConfigComponent {
    pub fn entry_point(self, cfg: patina::component::params::Config<TestConfig>) -> patina::error::Result<()> {
        assert_eq!(cfg.x, 7);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Service trait + impl
// ---------------------------------------------------------------------------

trait TestService {
    fn get_value(&self) -> u32;
}

#[derive(patina::component::service::IntoService)]
#[service(dyn TestService)]
struct TestServiceImpl {
    value: u32,
}

impl TestService for TestServiceImpl {
    fn get_value(&self) -> u32 {
        self.value
    }
}

// ---------------------------------------------------------------------------
// Config type
// ---------------------------------------------------------------------------

#[derive(Default)]
struct TestConfig {
    x: u32,
}

// ---------------------------------------------------------------------------
// Platform definition
// ---------------------------------------------------------------------------

struct TestPlatform;

impl Platform for TestPlatform {
    fn components(add: &mut impl ComponentAdder) {
        add.component(SimpleComponent);
        add.component(ProducerComponent);
        // Consumer depends on the service from Producer — dispatcher handles ordering.
        add.component(ConsumerComponent);
        add.component(ConfigComponent);
    }

    fn configs(add: &mut impl ConfigAdder) {
        add.config(TestConfig { x: 7 });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_dispatch_platform_full_platform() {
    let storage = Storage::new();
    let mut ctx = DriverContext::from_storage(storage);
    ctx.dispatch_platform::<TestPlatform>()
        .expect("dispatch should succeed");
}

#[test]
fn test_dispatch_platform_empty() {
    struct EmptyPlatform;
    impl Platform for EmptyPlatform {}

    let storage = Storage::new();
    let mut ctx = DriverContext::from_storage(storage);
    ctx.dispatch_platform::<EmptyPlatform>()
        .expect("empty platform should dispatch");
}

#[test]
fn test_dispatch_platform_simple_component() {
    struct SimplePlatform;
    impl Platform for SimplePlatform {
        fn components(add: &mut impl ComponentAdder) {
            add.component(SimpleComponent);
        }
    }

    let storage = Storage::new();
    let mut ctx = DriverContext::from_storage(storage);
    ctx.dispatch_platform::<SimplePlatform>()
        .expect("simple platform should dispatch");
}

#[test]
fn test_dispatch_platform_service_ordering() {
    // Register consumer BEFORE producer — dispatch loop should handle ordering.
    struct ReversedPlatform;
    impl Platform for ReversedPlatform {
        fn components(add: &mut impl ComponentAdder) {
            add.component(ConsumerComponent);
            add.component(ProducerComponent);
        }
    }

    let storage = Storage::new();
    let mut ctx = DriverContext::from_storage(storage);
    ctx.dispatch_platform::<ReversedPlatform>()
        .expect("reversed ordering should still work");
}
