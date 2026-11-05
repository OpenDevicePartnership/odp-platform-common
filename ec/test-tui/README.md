# ec-test-tui

## Overview
Ratatui-based TUI application for demoing and testing EC features (thermal, battery, RTC).
See [ODP Documentation](https://opendevicepartnership.github.io/documentation/guide/overview.html) for details on EC specification.

## Building

```
cargo build --release
```

For Windows on ARM (cross-compile):
```
cargo build-win --release
```

On Windows ARM64, the `acpi` source requires the `ectest.sys` KMDF driver built/installed with the required ACPI entries/device instance present.
On all Windows targets, the `windows` source requires the corresponding Windows class driver(s) (e.g. `HIDTime.sys`).
These drivers live in [odp-windows-drivers](https://github.com/OpenDevicePartnership/odp-windows-drivers).

## Usage

```
ec-test-tui --source <mock|serial|acpi|windows> [OPTIONS]
```

- `--source` — The data source to use. Accepts `mock` and `serial` on all targets, `acpi` on Windows ARM64, and `windows` on all Windows targets. Defaults to `acpi` on Windows ARM64 and `serial` elsewhere.
- `--log-file` — Optional path to write logs to a file in addition to the in-app log panel.
- `--sensor-instance` — Sensor instance index. Defaults to `0`.
- `--fan-instance` — Fan instance index. Defaults to `0`.

The following options apply when using the `serial` source (including when it is the default):
- `--port` — Path to the serial port (e.g., `/dev/ttyUSB0`, `COM3`). Required.
- `--flow-control` — `hw` or `none`. Defaults to `none`.
- `--baud` — Baud rate. Defaults to `115200`.

Example:
```
ec-test-tui --source serial --port /dev/ttyUSB0
```
