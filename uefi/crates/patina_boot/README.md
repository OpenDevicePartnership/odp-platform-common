# Patina Boot

Boot orchestration component for Patina-based firmware implementing UEFI boot manager functionality.

## Components

- **BootDispatcher**: Installs the BDS architectural protocol and delegates to a `BootOrchestrator` implementation.
- **BootOrchestrator**: Trait for custom boot flows. Platforms implement this to define boot behavior.
- **SimpleBootManager**: Default `BootOrchestrator` for platforms with straightforward boot topologies.

## Usage

```rust
use patina_boot::{BootDispatcher, SimpleBootManager, config::BootConfig};

// Minimal boot:
let orchestrator = SimpleBootManager::new(
    BootConfig::new(nvme_esp_path())
        .with_device(nvme_recovery_path()),
);
add.component(BootDispatcher::new(orchestrator));

// Custom orchestrator:
add.component(BootDispatcher::new(MyCustomOrchestrator::new()));
```

## Helper Functions

For custom boot flows, use the helper functions in the `helpers` module:

- `connect_all()` - Connect all controllers for device enumeration
- `enter_locked_boot()` - Signal EndOfDxe, install DxeSmmReadyToLock, and return a `LockedBoot` capability
- `signal_ready_to_boot()` - Signal ReadyToBoot event
- `discover_console_devices()` - Populate console variables
- `LockedBoot::boot_from_device_path()` - Load and start an image after the security transition

## Fallback boot policy

`BootSourcePolicy` controls filesystem fallback after provisioned boot options
are exhausted. Its defaults are fail closed:

- no internal volume is eligible until its device path is approved;
- removable USB media is disabled until `allow_removable_media()` is set;
- fallback is allowed with Secure Boot enabled;
- disabled Secure Boot requires `allow_when_secure_boot_disabled()`; and
- SetupMode requires `allow_in_setup_mode()`.

Invalid or missing `SecureBoot` and `SetupMode` variables deny fallback.
