# ODP Platform — Common

[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[![Workflow: cargo-vet](https://github.com/OpenDevicePartnership/odp-platform-common/actions/workflows/cargo-vet.yml/badge.svg)](https://github.com/OpenDevicePartnership/odp-platform-common/actions/workflows/cargo-vet.yml)
[![Workflow: check](https://github.com/OpenDevicePartnership/odp-platform-common/actions/workflows/check.yml/badge.svg)](https://github.com/OpenDevicePartnership/odp-platform-common/actions/workflows/check.yml)
[![Workflow: uefi-check](https://github.com/OpenDevicePartnership/odp-platform-common/actions/workflows/uefi-check.yml/badge.svg)](https://github.com/OpenDevicePartnership/odp-platform-common/actions/workflows/uefi-check.yml)

This repository contains a collection of common tools, components, and documentation provided by [Open Device Partnership](https://opendevicepartnership.github.io/documentation/guide/overview.html).

Modules in this repo are consumed in one of two ways:

- **Git submodule** — most modules. The parent platform repo adds this repo as a submodule and references modules by path (drivers via `.fdf` includes, libraries via `.dsc` `[LibraryClasses]`).
- **Crates.io** — Rust crates under `uefi/crates/`. The parent platform repo (or any external OEM tree) adds a single dependency line to its `Cargo.toml` and consumes the crate by name — no submodule required.

See the README inside each module for the consumption model that applies.

## Folder Structure and Content

The top-level directories represent broad segments of a platform such as a firmware stage or a hardware target.

Each segment directory contains one or more module folders that are intended to be stand-alone items with their own infrastructure such as a `README.md` file and build sub-system.  Examples of module usage can be found in one or more of the `odp-platform-*` repositories.

```text
<repo root>
├── uefi/               Platform segment — UEFI firmware
│   └── OdpPkg/             Standard UEFI package containing drivers and libraries for integration into an EDK II firmware build.
├── ec/                 Platform segment — Embedded controller firmware
│   ├── test-lib/           EC transport traits and implementations
│   ├── test-tui/           Terminal UI for EC feature demonstration
│   ├── test-cli/           Command-line interface for EC testing and validation
│   └── test-win/           Windows-native EC driver, library, and CLI
├── common/             Cross-platform and cross-segment shared items
│   └── supply-chain/       Cargo-vet audit configuration for Rust dependencies
├── .vscode/            Optional VS Code workspace settings (editor, formatter, rust-analyzer config)
├── LICENSE             License information covering this repository
├── CODE_OF_CONDUCT.md  Community interaction and behavior guidelines
├── CONTRIBUTING.md     How to submit issues, pull requests, and contribution licensing terms
├── CODEOWNERS          GitHub CODEOWNERS file defining required reviewers for pull requests
├── SECURITY.md         Vulnerability disclosure and embargo policy
```
