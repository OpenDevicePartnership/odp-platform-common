# ec-test-cli

## Overview
Command-line tool for testing EC features (thermal, battery, RTC). Each command maps directly to an EC data source trait method — it executes the request, prints the result, and exits.

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
ec-test-cli --source <mock|serial|local> [OPTIONS] <COMMAND>
```

- `--source` — The data source to use. Accepts `mock`, `serial`, or `local` (Windows only). Defaults to `serial` on Linux and `local` on Windows.
- `--sensor-instance` — Sensor instance index. Defaults to `0`.
- `--fan-instance` — Fan instance index. Defaults to `0`.

The following options only apply when `--source serial`:
- `--port` — Path to the serial port (e.g., `/dev/ttyUSB0`, `COM3`). Required.
- `--flow-control` — `hw` or `none`. Defaults to `none`.
- `--baud` — Baud rate. Defaults to `115200`.

Use `ec-test-cli --help` and `ec-test-cli <COMMAND> --help` to see available commands and options.

Setter commands print nothing on success — exit code 0 indicates success.
