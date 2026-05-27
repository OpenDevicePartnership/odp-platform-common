# patina_sre

System Recovery Environment boot orchestrator for Patina firmware.

`SreBootManager` implements [`patina_boot::BootOrchestrator`] for platforms that
ship a System Recovery Environment alongside the main OS. The current crate is
a **skeleton** implementing the normal boot path only:

1. Interleave controller connection with DXE driver dispatch
2. Extra `connect_all` pass before EndOfDxe so platforms whose driver-binding
   runs only in the open window get a chance to bind (e.g. `PartitionDxe`
   creating GPT child handles)
3. Signal `EndOfDxe` (security lockdown)
4. Discover console devices
5. Write-lock the NVMe boot partition *(pending — TODO referencing
   [odp-platform-common#61](https://github.com/OpenDevicePartnership/odp-platform-common/issues/61))*
6. Enumerate firmware `Boot####` EFI variables via `discover_boot_options`
   and try each in order
7. Fall back to the constructor-provided `main_os_path` if discovery yields
   nothing (or fails)

Follow-ups (tracked separately): Power+Vol-Up hotkey to enter SRE, SRE WIM
RAM-disk boot, capsule-update pre-boot hook.

## Use

```rust,ignore
use patina_boot::BootDispatcher;
use patina_sre::SreBootManager;

add.component(BootDispatcher::new(
    SreBootManager::new(boot_partition_path, main_os_path),
));
```

## License

MIT
