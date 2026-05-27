# patina_sre

System Recovery Environment boot orchestrator for Patina firmware.

`SreBootManager` implements [`patina_boot::BootOrchestrator`] for platforms that
ship a System Recovery Environment alongside the main OS. The current crate is
a **skeleton** implementing the normal boot path only:

1. Interleave controller connection with DXE driver dispatch
2. Signal `EndOfDxe` (security lockdown)
3. Discover console devices
4. Write-lock the NVMe boot partition *(pending — TODO referencing
   [odp-platform-common#61](https://github.com/OpenDevicePartnership/odp-platform-common/issues/61))*
5. Signal `ReadyToBoot`
6. Boot the main OS device

Follow-ups (tracked separately): Power+Vol-Up hotkey to enter SRE, SRE WIM
RAM-disk boot, capsule-update pre-boot hook.

## Use

```rust,ignore
use alloc::sync::Arc;
use patina_boot::BootDispatcher;
use patina_sre::SreBootManager;

add.component(BootDispatcher::new(
    Arc::new(SreBootManager::new(boot_partition_path, main_os_path)),
));
```

## License

MIT
