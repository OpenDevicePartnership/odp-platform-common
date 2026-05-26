# AGENTS.md

Guidance for AI coding agents working in `openDevicePartnership/odp-platform-common`.

This file complements [`.github/copilot-instructions.md`](.github/copilot-instructions.md) — read both. When they overlap, `copilot-instructions.md` is authoritative for PR review rules and commit-message policy; this file is authoritative for repository layout, build/test commands, and per-workspace conventions.

## Repository overview

This repo aggregates platform building blocks consumed by ODP platform repos. Top-level directories are independent **platform segments**, each with its own toolchain, build system, and CI workflow:

- `uefi/` — UEFI firmware. Contains `OdpPkg/` (EDK II package with `.dec`/`.dsc`/`.inf` files and C/Rust DXE drivers) and `uefi/crates/` (publishable Rust crates targeting `*-unknown-uefi`, pinned to **nightly**).
- `ec/` — Embedded controller firmware. A single Cargo **workspace** (`resolver = "3"`, `edition = "2024"`, `rust-version = "1.88"`) with members `test-cli`, `test-lib`, `test-tui`. `test-win/` is a separate Windows-native (KMDF) tree, not a workspace member.
- `common/` — Cross-segment shared configuration. Currently `common/supply-chain/` holds the `cargo-vet` store shared by all Rust workspaces in this repo.
- `.github/workflows/` — CI: `check.yml` (ec/), `uefi-check.yml` (uefi/crates/), `cargo-vet.yml`, `cargo-vet-pr-comment.yml`.

Modules are consumed by parent platform repos either as **git submodules** (most of `OdpPkg/`, `ec/test-*`) or via **crates.io** (`uefi/crates/*`). Treat each segment as independently versioned.

## Working-directory rules (read carefully)

Most commands are **not** run from the repo root. Mismatches here are the most common cause of CI-vs-local divergence.

| Task | Working directory |
| --- | --- |
| EC workspace Cargo commands (`fmt`, `clippy`, `doc`, `hack`, `deny`, `machete`) | `ec/` |
| `cargo vet` | `ec/` (with `--store-path ../common/supply-chain`) |
| UEFI Rust crate commands (`fmt`, `clippy`, `doc`, `build`) | `uefi/crates/<crate>/` (e.g. `uefi/crates/patina_tianocore/`) |
| EDK II package work (`OdpPkg/`) | Driven by the parent platform repo's EDK II build; no commands run from this repo |

There is **no workspace at the repo root** — `cargo` invoked from the root will fail. Always `cd` into the relevant segment first.

## CI commands (run these before pushing)

These mirror the GitHub Actions workflows exactly. Run them in the listed working directory.

### `ec/` workspace (stable toolchain)

```bash
# from ec/
cargo fmt --check
# clippy across the full feature powerset, one crate at a time, matching CI
cargo hack --feature-powerset clippy --locked -p ec-test-cli -- \
    -Dwarnings -D clippy::suspicious -D clippy::correctness -D clippy::perf -D clippy::style
cargo hack --feature-powerset clippy --locked -p ec-test-lib -- \
    -Dwarnings -D clippy::suspicious -D clippy::correctness -D clippy::perf -D clippy::style
cargo hack --feature-powerset clippy --locked -p ec-test-tui -- \
    -Dwarnings -D clippy::suspicious -D clippy::correctness -D clippy::perf -D clippy::style
cargo deny check --manifest-path Cargo.toml   # or: from repo root, --manifest-path ec/Cargo.toml
cargo machete
# nightly only, with RUSTDOCFLAGS=--cfg docsrs
RUSTDOCFLAGS="--cfg docsrs" cargo +nightly doc --all-features --no-deps --locked
```

Required tools: `cargo-hack`, `cargo-deny`, `cargo-machete`, plus a nightly toolchain for `doc`. Install with `cargo install cargo-hack cargo-deny cargo-machete` and `rustup toolchain install nightly`.

The CI `clippy` matrix is `{stable, beta} × {ubuntu, windows} × {ec-test-cli, ec-test-lib, ec-test-tui}`. Locally, stable on your host OS is usually enough; rely on CI for the rest.

### `uefi/crates/patina_tianocore/` (pinned nightly)

The toolchain is pinned in `uefi/crates/patina_tianocore/rust-toolchain.toml`. CI installs the same channel explicitly (currently `nightly-2025-12-12`) with `rust-src`, `clippy`, `rustfmt`, and the `x86_64-unknown-uefi` + `aarch64-unknown-uefi` targets. **When you bump `rust-toolchain.toml`, also bump the `toolchain:` field in `.github/workflows/uefi-check.yml`.**

```bash
# from uefi/crates/patina_tianocore/
cargo fmt --check
cargo clippy --all-features -- -D warnings
cargo clippy --all-features --target x86_64-unknown-uefi -- -D warnings
cargo clippy --all-features --target aarch64-unknown-uefi -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo build --target x86_64-unknown-uefi
cargo build --target aarch64-unknown-uefi
```

### Supply chain (`cargo-vet`)

```bash
# from ec/
cargo vet --locked --store-path ../common/supply-chain
```

When adding a new dependency to `ec/`, run `cargo vet` and add the needed audits/exemptions under `common/supply-chain/` rather than disabling the check.

## Conventions

- **Formatting**: `rustfmt` with `max_width = 120` (see `ec/rustfmt.toml`). UEFI crates have their own `rustfmt.toml` — respect each.
- **Lints**: The `ec/` workspace denies `clippy::{suspicious, correctness, perf, style}` at the workspace level. New code must not introduce warnings under these groups.
- **Edition / MSRV**: `ec/` uses edition `2024`, MSRV `1.88`. `uefi/crates/patina_tianocore` uses edition `2024`, MSRV `1.89` and requires the pinned nightly for unstable features.
- **Resolver**: `ec/` uses `resolver = "3"`. Do not downgrade.
- **Dependencies**: Add workspace-shared crates to `[workspace.dependencies]` in `ec/Cargo.toml` and reference them from members with `dep = { workspace = true }`. Service-message crates come from `OpenDevicePartnership/embedded-services` on branch `v0.2.0` — keep that pin consistent.
- **`critical-section`**: Included in `[workspace.dependencies]` with `features = ["std"]` purely to satisfy a transitive dependency from `embedded-services`. It's listed in `[workspace.metadata.cargo-machete] ignored` — leave that entry in place if you touch the manifest.
- **Cross-compile to Windows**: `ec/` defines a `cargo build-win` alias (`xwin build --target aarch64-pc-windows-msvc`) and pins `-C target-feature=+crt-static` for that target. Use the alias rather than re-deriving the flags.
- **License headers**: Source files start with an `SPDX-License-Identifier:` comment. New files in this repo default to `MIT`. Files derived from upstream projects (EDK II, ATF, etc.) keep their original SPDX id (often `BSD-2-Clause-Patent`) — see `CONTRIBUTING.md`.
- **EDK II files** (`.dec`, `.dsc`, `.inf`, `.fdf`, `.depex`): `*.depex` is compiled bytecode and marked `binary` in `.gitattributes` — do not hand-edit. Other EDK II text files are edited as part of `OdpPkg/`; keep alphabetical ordering inside `[LibraryClasses]` / `[Components]` blocks when the existing file uses it.

## Commit and PR rules

These come from `.github/copilot-instructions.md` and `CONTRIBUTING.md`. Highlights an agent must follow:

- **Subject**: ≤ 50 chars, capitalized, imperative ("Fix bug", not "Fixed bug"). Blank line, then body wrapped at 72 chars explaining *what* and *why*.
- **AI attribution**: every AI-assisted commit **must** carry an `Assisted-by:` trailer, e.g. `Assisted-by: GitHub Copilot:claude-opus-4.7`. Verify the actual agent name and model version at commit time — do not copy from a previous session.
- **No `Signed-off-by`**: agents must not add DCO sign-offs; only humans can certify the DCO.
- **PR flow**: open a **draft** PR first and wait for `check`, `uefi-check`, and `cargo-vet` workflows to pass before requesting review.
- **Authorship**: set commit author per invocation with `git -c user.name=... -c user.email=...`; do not change global git config.

## Things to be careful with

- **Async drop safety**: code using `select`, `selectN`, `select_array`, `select_slice` (or carrying a drop-safety comment) drops the un-selected futures. When editing such code, verify no message/value is lost on the dropped branch.
- **Panic-prone code**: any function marked with a panic-safety comment is intentionally narrow about its invariants. Preserve those invariants when refactoring.
- **Submodules**: the repo has submodules (`.gitmodules`). Use `git checkout --recurse-submodules` and clone with `--recursive`; CI always passes `submodules: true` to `actions/checkout`.
- **Don't run `cargo` at the repo root** — there is no top-level workspace. Pick the right segment directory first.
- **Don't merge segments**: keep `ec/`, `uefi/crates/*`, and `OdpPkg/` build systems independent; they are intentionally separate so consumers can pull only what they need.
