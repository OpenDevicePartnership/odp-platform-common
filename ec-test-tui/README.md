# ec-test-tui

## Overview
Ratatui-based TUI application for demoing and testing EC features (thermal, battery, RTC, UCSI).
See [ODP Documentation](https://opendevicepartnership.github.io/documentation/guide/overview.html) for details on EC specification.

## Building

### With mock data (no hardware required)
```
cargo build --release --features mock
```

### With ACPI transport (Windows-only)
```
cargo build-win --release --features acpi
```

Note: building with `--features acpi` only enables the ACPI transport in the binary. To use it at runtime on Windows, you must also have the `ectest.sys` KMDF driver built/installed and the required ACPI entries/device instance present. See [ec-test-win/README.md](../ec-test-win/README.md) for the Windows driver/setup requirements.

### With serial transport
```
cargo build --release --features serial
```

Usage: `ec-test-tui <serial_port_path> <flow_control> [baud_rate=115200]`
- `serial_port_path` — Path to the serial port (e.g., `/dev/ttyUSB0`, `COM3`)
- `flow_control` — `hw` for hardware flow control, `none` to disable
- `baud_rate` — (Optional) Baud rate as a u32. Defaults to `115200` if not specified

Example:
```
ec-test-tui /dev/ttyUSB0 none 115200
```
