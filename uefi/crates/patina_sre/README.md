# patina_sre

Reference System Recovery Environment (SRE) boot orchestrator for ODP platforms.

`patina_sre::SreBootManager` implements `patina_boot::BootOrchestrator` with the BDS-phase sequencing typical of devices shipping a recovery image alongside the main OS. OEMs can adopt as-is or use it as the starting point for vendor-specific recovery flows.

## What it does

The orchestrator runs the following sequence under the DXE Boot Device Selection phase:

1. **Interleave controller connection with DXE driver dispatch** until convergence.
2. **Signal BDS phase entry** — fires the event group used by components that need to react at the transition from DXE driver dispatch into Boot Device Selection.
3. **Discover console devices** and connect them.
4. **Write-lock the NVMe boot partition** (via `patina_nvme::lock_partition_write`) so the OS cannot modify the firmware payload region post-handoff. The lock is volatile and clears on power cycle.
5. **Signal `ReadyToBoot`** — last opportunity for components to react before OS load.
6. **Boot the main OS device** from the configured device path.

The current crate is a skeleton — only the normal path is wired. SRE-entry hotkey detection, SRE WIM RAM-disk boot, and capsule-update pre-boot hook are planned and will layer on without changing the constructor surface.

## Usage

```rust,ignore
use patina_boot::BootDispatcher;
use patina_sre::SreBootManager;

Core::default()
    .with_component(BootDispatcher::new(SreBootManager::new(
        boot_partition_device_path,
        main_os_device_path,
    )))
    // ... rest of platform components
```

## Building

This crate currently consumes `patina`, `patina_boot`, and `patina_nvme` from git branches because none of those releases are yet on crates.io. Once releases ship, the dependencies will move to versioned crates.io references and consumers will no longer need any `[patch.crates-io]` glue.

## Status

- `lock_partition_write` is exercised against a mock NVMe Pass-Thru protocol via `patina_nvme`'s tests.
- The interleave / dispatch loop is unit-tested with mock `BootServices` and `DxeDispatch` implementations.
- End-to-end boot has been verified locally on `surface_patina_intel` (Won/Maa/Pue PTL boards) builds. On-device flash validation is in progress.

## License

`SPDX-License-Identifier: MIT`
