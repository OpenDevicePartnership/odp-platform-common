# ec-test-cli

## Overview
Command-line tool for testing EC features (thermal, battery, RTC). Each command maps directly to an EC data source trait method — it executes the request, prints the result, and exits.

See [ODP Documentation](https://opendevicepartnership.github.io/documentation/guide/overview.html) for details on EC specification.

## Building

Exactly one transport feature must be enabled at build time: `mock`, `acpi`, or `serial`.

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

## Usage

```
ec-test-cli <COMMAND>
```

When built with the `serial` feature, transport arguments are available:
```
ec-test-cli --port <SERIAL_PORT> [--flow-control <hw|none>] [--baud <RATE>] <COMMAND>
```
- `--port` — Required. Path to the serial port (e.g., `/dev/ttyUSB0`, `COM3`)
- `--flow-control` — Optional. `hw` or `none`. Defaults to `none`
- `--baud` — Optional. Baud rate. Defaults to `115200`

Use `ec-test-cli --help` and `ec-test-cli <COMMAND> --help` to see available commands and options.

Setter commands print nothing on success — exit code 0 indicates success.
