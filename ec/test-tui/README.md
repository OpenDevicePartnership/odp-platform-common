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

Note: to use the `local` source on Windows, you must have the `ectest.sys` KMDF driver built/installed and the required ACPI entries/device instance present. See [test-win/README.md](../test-win/README.md) for the Windows driver/setup requirements.

## Usage

```
ec-test-tui --source <mock|serial|local> [OPTIONS]
```

- `--source` — The data source to use. Accepts `mock`, `serial`, or `local` (Windows only). Defaults to `serial` on Linux and `local` on Windows.
- `--log-file` — Optional path to write logs to a file in addition to the in-app log panel.
- `--sensor-instance` — Sensor instance index. Defaults to `0`.
- `--fan-instance` — Fan instance index. Defaults to `0`.

The following options only apply when `--source serial`:
- `--port` — Path to the serial port (e.g., `/dev/ttyUSB0`, `COM3`). Required.
- `--flow-control` — `hw` or `none`. Defaults to `none`.
- `--baud` — Baud rate. Defaults to `115200`.

Example:
```
ec-test-tui --source serial --port /dev/ttyUSB0
```
