# ODP Patina SMBIOS Demonstration Driver

This driver shows how to build a UEFI DXE driver that produces an SMBIOS Type 0 (BIOS Information) record using **only the native Patina component model** — no `EFI_SMBIOS_PROTOCOL`, no raw buffer construction, no `unsafe` boilerplate inside the publishing component.

It uses the [`patina_tianocore`](https://crates.io/crates/patina_tianocore) bridge crate to ship today on a TianoCore DXE core while keeping the same source compatible with a Patina-native DXE core later.

## Assumptions and Limitations

The reader is assumed to be familiar with UEFI driver authoring and the TianoCore build process. Familiarity with [`ODP_RustDxeDemo`](../ODP_RustDxeDemo) — the prior, lower-level Rust demo using raw `r-efi` — is helpful but not required.

This driver is compiled outside the EDK II build system and linked into the firmware image via the `.fdf` file, the same way as `ODP_RustDxeDemo`. It depends on the SMBIOS service exposed by `patina_smbios`'s `SmbiosProvider` component, and on the boot services / memory manager wiring done automatically by `patina_tianocore::driver_entry!`.

## What's where

```
ODP_PatinaSmbiosDemo/
├── Cargo.toml                    Module manifest
├── ODP_PatinaSmbiosDemo.depex    DXE depex (TRUE — no protocol prerequisites)
├── README.md                     This file
└── src/
    ├── main.rs                   3-line driver shell — driver_entry!(...)
    └── platform.rs               Platform impl + BiosInfoSmbiosPublisher
                                  component. This file is portable across
                                  runtimes; lift it into a shared crate
                                  when you also want a Patina-native binary.
```

## Prerequisites

```bash
# Rust nightly toolchain (required by patina dependencies)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add x86_64-unknown-uefi
```

> If your target platform is ARM, replace `x86_64` with `aarch64` (i.e., `aarch64-unknown-uefi`).

## Compile

```bash
cd ./uefi/OdpPkg/Drivers/ODP_PatinaSmbiosDemo
BUILD_DATE=$(date +%m/%d/%Y) cargo build --target x86_64-unknown-uefi --release
```

The output will be a PE32+ executable: `target/x86_64-unknown-uefi/release/ODP_PatinaSmbiosDemo.efi`.

`BUILD_DATE` is consumed by `option_env!("BUILD_DATE")` inside the publisher so the SMBIOS record carries the actual build date. Skipping it is fine — the record falls back to `01/01/1970`.

## Insert into UEFI build

Add to your platform `.fdf` (no `.dsc` entry needed):

```text
FILE DRIVER = C74158C9-87BC-448B-86B1-818071938A4C {
  SECTION DXE_DEPEX = <path-to>/OdpPkg/Drivers/ODP_PatinaSmbiosDemo/ODP_PatinaSmbiosDemo.depex
  SECTION PE32      = <path-to>/OdpPkg/Drivers/ODP_PatinaSmbiosDemo/target/x86_64-unknown-uefi/release/ODP_PatinaSmbiosDemo.efi
  SECTION UI        = "ODP_PatinaSmbiosDemo"
}
```

The `FILE DRIVER` GUID is unique to this driver. The `.depex` is a minimal `TRUE` expression (`0x06 0x08`), unlike `ODP_RustDxeDemo`'s GUID-pinned depex on `gEfiVariableArchProtocolGuid`. This driver only uses boot services and the SMBIOS service it instantiates itself, so it has no protocol prerequisites and dispatches as soon as the DXE phase begins.

After rebuilding and booting your firmware, the SMBIOS table published to the UEFI Configuration Table will contain a Type 0 record whose vendor string is `"Acme Inc."`. From a UEFI shell:

```
Shell> smbiosview -t 0
```

## Migrating to Patina native

When your firmware is ready to host the Patina DXE core, you do **not** rewrite this driver. You write a sibling crate that uses the same `OemPlatform` struct from `platform.rs`:

```rust
use my_platform::OemPlatform;

patina_tianocore::impl_component_info!(OemPlatform);

impl MemoryInfo for OemPlatform { /* ... */ }
impl CpuInfo  for OemPlatform { /* ... */ }
impl PlatformInfo for OemPlatform { /* ... */ }
```

The `BiosInfoSmbiosPublisher` component, the `Platform::components` registration, and the SMBIOS record itself transfer with **zero changes**.

## Customize for your own platform

In your tree, copy the directory, change the vendor string and BIOS version in `platform.rs`, and add additional record-publishing components alongside `BiosInfoSmbiosPublisher` (Type 1 system info, Type 2 baseboard, Type 3 enclosure, etc. — `patina_smbios::smbios_record` ships typed structs for all standard SMBIOS record types).
