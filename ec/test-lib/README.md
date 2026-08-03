# ec-test-lib

Rust library providing EC transport traits and implementations.

## Sources

All sources are compiled unconditionally (except `acpi` and `hid`, which are only available on Windows).
Binaries using this library select which source to use at runtime, typically via a `--source` flag.

- **mock** — Mock EC data for development and testing without hardware
- **acpi** — Windows ACPI transport via the `ectest.sys` KMDF driver (compiled only on Windows)
- **hid** — Windows HID transport via the HID class driver(s) (e.g. `HIDTime.sys`) (compiled only on Windows)
- **serial** — Serial transport for communicating with EC over user-space serial port
