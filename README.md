# ODP Platform — Common

[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A collection of common tools, components, and documentation provided by [Open Device Partnership](https://opendevicepartnership.github.io/documentation/guide/overview.html) for use within a platform build infrastructure.

This repository is intended to be consumed as a git submodule within a parent platform repository. It has no central build system — each module manages its own build infrastructure and outputs independently.  See one of the platform-specific `odp-platform-*` repositories for an example of usage.

## Folder Structure and Content

Top-level directories represent broad segments of a platform such as a firmware stage or hardware target. Each segment directory contains one or more module folders, each with its own component-specific information such as a readme and build infrastructure.

```text
<repo root>
├── uefi/               Platform segment — UEFI firmware
│   └── OdpPkg/             Standard UEFI package containing drivers and libraries for integration into an EDK II firmware build.
├── ec/                 Platform segment — Embedded controller firmware
│   ├── test-lib/           EC transport traits and implementations
│   ├── test-tui/           Terminal UI for EC feature demonstration
│   └── test-win/           Windows-native EC driver, library, and CLI
├── common/             Cross-platform and cross-segment shared items
│   └── supply-chain/       Cargo-vet audit configuration for Rust dependencies
├── LICENSE             License information covering this repository
├── CODE_OF_CONDUCT.md  Community interaction and behavior guidelines
├── CONTRIBUTING.md     How to submit issues, pull requests, and contribution licensing terms
├── CODEOWNERS          GitHub CODEOWNERS file defining required reviewers for pull requests
├── SECURITY.md         Vulnerability disclosure and embargo policy
```
