# ec-test-tui

## Overview
Ratatui-based TUI application for demoing and testing EC features (thermal, battery, RTC, UCSI).
See [ODP Documentation](https://opendevicepartnership.github.io/documentation/guide/overview.html) for details on EC specification.

## Building

### With mock data (no hardware required)
```
cargo build --release --features mock
```

### With ACPI transport (Windows, requires EWDK + eclib)
First build eclib from `ec-test-win/lib/` (see [ec-test-win compilation docs](../ec-test-win/README.md#compilation)), then:
```
cargo build --release --features acpi --target=aarch64-pc-windows-msvc
```

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
