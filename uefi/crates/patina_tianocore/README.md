# patina_tianocore

A bridge crate that lets a UEFI driver target both **TianoCore** and **Patina** through a single API surface. Write your platform once, ship on TianoCore today, swap to Patina native later by changing one line of glue.

> **Status:** pre-1.0. APIs may change. The crate has not yet been published to crates.io — see [Consuming this crate](#consuming-this-crate) below.

## The transition problem

Today, an OEM moving from C UEFI to Patina ends up porting twice:

1. Rewrite a C UEFI driver in Rust against raw TianoCore (`r-efi`).
2. Rewrite that Rust driver *again* against Patina's component/service model.

`patina_tianocore` collapses those two steps. You implement the [`Platform`] trait once. The same impl is consumed by:

- `patina_tianocore::driver_entry!(platform: MyPlatform)` — produces a TianoCore DXE `.efi` driver today.
- `patina_tianocore::impl_component_info!(MyPlatform)` — generates the `patina_dxe_core::ComponentInfo` impl when you're ready to run on Patina natively.

Component structs, services, configs — everything inside the trait methods — transfers with **zero changes**.

## Consuming this crate

Until the first crates.io release lands, depend on the crate via git:

```toml
# Cargo.toml
[dependencies]
patina_tianocore = { git = "https://github.com/OpenDevicePartnership/odp-platform-common", branch = "main" }
patina = "20"
patina_smbios = "20"
```

Cargo automatically discovers `patina_tianocore` inside the repo even though it lives at `uefi/crates/patina_tianocore/`.

After the first crates.io release the dependency simplifies to:

```toml
patina_tianocore = "0.1"
```

## Quick start

```rust
// src/platform.rs (shared crate — works on both runtimes)
use patina_tianocore::prelude::*;
use patina_smbios::component::SmbiosProvider;

pub struct MyPlatform;

impl Platform for MyPlatform {
    fn components(add: &mut impl ComponentAdder) {
        add.component(SmbiosProvider::new(3, 9));
        // add.component(MyOemBiosInfoPublisher { ... });
    }
}
```

```rust
// TianoCore driver binary (today)
#![no_std]
#![no_main]
#![feature(allocator_api)]

patina_tianocore::driver_entry!(platform: my_platform::MyPlatform);
```

```rust
// Patina native binary (when ready)
#![no_std]
#![no_main]
#![feature(allocator_api)]

patina_tianocore::impl_component_info!(my_platform::MyPlatform);
// ... plus the Patina platform-specific impls (MemoryInfo, CpuInfo, Extractor)
```

## What's shared vs runtime-specific

| | Shared (write once) | TianoCore-only | Patina-native-only |
|---|---|---|---|
| `impl Platform` | ✅ | | |
| `#[component]` structs | ✅ | | |
| `Service<dyn T>` impls | ✅ | | |
| `driver_entry!` | | ✅ | |
| `impl_component_info!` | | | ✅ |
| `impl PlatformInfo` (`MemoryInfo`/`CpuInfo`/`Extractor`) | | | ✅ |

## Feature flags

| Feature | Description |
|---|---|
| `tianocore` *(default)* | Back all abstractions with TianoCore / `r-efi`. The only currently implemented runtime. |

A `patina-native` feature for the Patina-native runtime will be added when the corresponding code paths land — exposing one today would be misleading.

## Working example

See `uefi/OdpPkg/Drivers/ODP_PatinaSmbiosDemo` for an example driver that
publishes an SMBIOS record through the native `Service<dyn Smbios>` from
`patina_smbios`.

## A note on `Cargo.lock`

This crate is a library and intentionally does **not** commit a `Cargo.lock`. Consumers (binaries that depend on it) commit their own lockfile and choose their own resolved versions. Standard Rust convention.

## Distribution

This crate is not intended for publication to crates.io or another public
registry. Consume it from this repository using a Git or path dependency.
The manifest sets `publish = false` to prevent accidental publication.

## License

MIT. See [LICENSE](../../../LICENSE) at the repo root.
