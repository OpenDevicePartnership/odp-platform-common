# patina_sre

System Recovery Environment boot orchestrator for Patina firmware.

`SreBootManager` implements [`patina_boot::BootOrchestrator`] for platforms that
ship a System Recovery Environment alongside the main OS. The flow:

1. Interleave controller connection with DXE driver dispatch
2. Extra `connect_all` pass before EndOfDxe so platforms whose driver-binding
   runs only in the open window get a chance to bind (e.g. `PartitionDxe`
   creating GPT child handles)
3. Signal `EndOfDxe` (security lockdown)
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

Follow-ups (tracked separately): capsule-update pre-boot hook,
`HotkeyProvider` trait for OEM-specific button mechanisms.

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
