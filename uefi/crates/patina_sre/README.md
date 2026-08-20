# patina_sre

System Recovery Environment boot orchestrator for Patina firmware.

`SreBootManager` implements [`patina_boot::BootOrchestrator`] for platforms that
ship a System Recovery Environment alongside the main OS. The flow:

1. Interleave controller connection with DXE driver dispatch
2. Extra `connect_all` pass before EndOfDxe so platforms whose driver-binding
   runs only in the open window get a chance to bind (e.g. `PartitionDxe`
   creating GPT child handles)
3. Signal `EndOfDxe`, install `DxeSmmReadyToLock`, and abort boot if either fails
4. Signal start-of-BDS event groups
5. Discover console devices
6. Probe the hotkey provider; on a recovery chord, dispatch the configured
   SRE app or fall back to the in-Rust BP recovery flow (NVMe LID read → RAM
   disk → chainload); on a frontpage chord, try USB via live
   `SimpleFileSystem` enumeration, then fall back to the configured frontpage
   app
7. Write-lock the NVMe boot partition *(pending — TODO referencing
   [odp-platform-common#61](https://github.com/OpenDevicePartnership/odp-platform-common/issues/61))*
8. Enumerate firmware `Boot####` EFI variables via `discover_boot_options`
   and try each in order
9. Fall back to the constructor-provided `main_os_path` if discovery yields
   nothing (or fails)
10. If explicitly configured, enumerate policy-approved filesystem fallback
    sources

Opt-in builders add work at BDS entry: `with_capsule_processing` signals the
platform capsule processor before EndOfDxe (drawing the boot logo first so
firmware-update progress can render), and `with_dfci_bds_signal` drives the
DFCI/SEMM device-setting stack.

Follow-ups (tracked separately): a `HotkeyProvider` trait for OEM-specific
button mechanisms, and a storage-backend abstraction so the recovery read works
across transports (NVMe today, UFS next).

## Use

```rust,ignore
use patina_boot::{BootDispatcher, BootSourcePolicy};
use patina_sre::SreBootManager;

add.component(BootDispatcher::new(
    SreBootManager::new(main_os_path).with_boot_source_policy(
        BootSourcePolicy::new()
            .with_approved_internal_device(recovery_volume_path),
    ),
));
```

Without `with_boot_source_policy`, filesystem fallback is disabled. Approved
internal volumes are usable while Secure Boot is enabled. OEMs must opt in
separately to removable media, disabled Secure Boot, or SetupMode through the
corresponding `BootSourcePolicy` builders.

## License

MIT
