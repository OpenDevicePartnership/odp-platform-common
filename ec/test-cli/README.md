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

Note (Windows only): the `acpi` source requires the `ectest.sys` KMDF driver built/installed with the required ACPI entries/device instance present; the `windows` source requires the corresponding Windows class driver(s) (e.g. `HIDTime.sys`). These drivers live in [odp-windows-drivers](https://github.com/OpenDevicePartnership/odp-windows-drivers).

## Usage

```
ec-test-cli --source <mock|serial|acpi|windows> [OPTIONS] <COMMAND>
```

- `--source` — The data source to use. Accepts `mock`, `serial`, or (Windows only) `acpi` and `windows`. Defaults to `serial` on Linux and `acpi` on Windows.
- `--sensor-instance` — Sensor instance index. Defaults to `0`.
- `--fan-instance` — Fan instance index. Defaults to `0`.

The following options only apply when `--source serial`:
- `--port` — Path to the serial port (e.g., `/dev/ttyUSB0`, `COM3`). Required.
- `--flow-control` — `hw` or `none`. Defaults to `none`.
- `--baud` — Baud rate. Defaults to `115200`.

Use `ec-test-cli --help` and `ec-test-cli <COMMAND> --help` to see available commands and options.

Setter commands print nothing on success — exit code 0 indicates success.

## Raw ACPI evaluation (Windows only)

The `eval` command evaluates an arbitrary ACPI method by name and prints its return
value(s). It is only supported with `--source acpi` and replaces the legacy C++
`ectest.exe -acpi ...` tool.

```
ec-test-cli --source acpi eval <METHOD> [ARGS]...
```

Each argument is parsed by shape:

- Integer — decimal (`1`), hex (`0x3`), or negative decimal (`-1`)
- GUID — brace-wrapped, e.g. `{07ff6382-e29a-47c9-ac87-e79dad71dd82}`
- String — single-quoted, e.g. `'TestString'`

Examples:
```
ec-test-cli --source acpi eval \_SB.ECT0.TFST
ec-test-cli --source acpi eval \_SB.ECT0.TDSM {07ff6382-e29a-47c9-ac87-e79dad71dd82} 1 3 0
```

Which methods are callable depends on your ACPI, since `ectest.sys` only binds to the
`ectest` device it declares. If your ACPI already has fan and battery definitions, add
the `ectest` device and define methods under `\_SB.ECT0` that call the ACPI functions you
want to exercise, then pass the method name to `eval`.
