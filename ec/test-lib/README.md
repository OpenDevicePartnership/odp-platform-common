# ec-test-lib

Rust library providing EC transport traits and implementations.

## Sources

The `acpi` source is compiled only for Windows ARM64; `windows` is compiled for all Windows targets.
Other sources are compiled unconditionally.
Binaries using this library select which source to use at runtime, typically via a `--source` flag.

- **mock** — Mock EC data for development and testing without hardware
- **acpi** — Windows ACPI transport via the `ectest.sys` KMDF driver (compiled only on Windows ARM64)
- **windows** — Windows class-driver interface via published device-interface IOCTLs (e.g. `HIDTime.sys`) (compiled only on Windows)
- **serial** — Serial transport for communicating with EC over user-space serial port
