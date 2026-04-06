# ec-test-lib

Rust library providing EC transport traits and implementations.

## Features

At most one transport feature may be enabled:

- **mock** — Mock EC data for development and testing without hardware
- **acpi** — Windows ACPI transport via eclib (requires EWDK + [ec-test-win](../ec-test-win/README.md))
- **serial** — Serial transport for communicating with EC over user-space serial port

With no feature enabled, only the traits are available (no transport implementation).
