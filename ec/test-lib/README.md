# ec-test-lib

Rust library providing EC transport traits and implementations.

## Sources

All sources are compiled unconditionally (except `acpi`, which is only available on Windows).
Binaries using this library select which source to use at runtime, typically via a `--source` flag.

- **mock** — Mock EC data for development and testing without hardware
- **acpi** — Windows ACPI transport (compiled only on Windows)
- **serial** — Serial transport for communicating with EC over user-space serial port
