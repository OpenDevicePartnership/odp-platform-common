# ODP DXE Demonstration Driver using Rust

This driver is a demonstration of how to create a driver written only in Rust, compile it as a stand-alone .efi executable, then integrate it into a firmware volume for a UEFI DXE Core to dispatch.

All notes in this sample assume the reader is new to Rust, but is a seasoned UEFI engineer that understands a Tianocore build process.  Please refer to the `./src/main.rs` file for details on what is needed to create a Rust based driver.

And for samples of integrating this driver as defined below, please refer to one of the `odp-platform-???` repositories.

## Assumptions and Limitations

This driver assumes a DEBUG target build that uses a 16550 UART for debug output. Because it is compiled separately from the UEFI build, it has no access to Platform Configuration Database (PCD) values. PCDs are UEFI's mechanism for storing platform-specific configuration such as debug port addresses and baud rates.

To address this, you could rewrite the Rust code to:

- Locate the PCD protocol via the UEFI Boot Services table
- Read the debug port settings from the PCD database
- Configure the `log` crate accordingly

For this demo to work properly on your system, examine the UART implementation in `./src/main.rs` for the proper crate (`uart_16550`, `pl011`, etc.) and I/O port address.

## Prerequisites

Install the Rust toolchain via [rustup](https://rustup.rs) if you haven't already:

``` bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## Compile

Install the UEFI target for your platform:

``` bash
rustup target add x86_64-unknown-uefi
```

> **Note:** If your target platform is ARM, replace `x86_64` with `aarch64` (i.e., `aarch64-unknown-uefi`).

Then switch to this driver folder and build the code using the proper target installed above:

``` bash
cd ./uefi/OdpPkg/Drivers/StandaloneDxeDemo_ru
cargo build --target x86_64-unknown-uefi
```

The output is a `.efi` PE32+ executable located at `target/x86_64-unknown-uefi/debug/StandaloneDxeDemo.efi`.

## Insert into UEFI build

The `.efi` file can be added to the UEFI `.fdf` file without a `.dsc` file entry. Replace `<path-to>` below with the actual path relative to your UEFI build tree:

``` text
  FILE DRIVER = 35AFEBCD-8485-4865-A9EC-447FF8EA47A9 {
    SECTION DXE_DEPEX = <path-to>/OdpPkg/Drivers/StandaloneDxeDemo_ru/true.depex
    SECTION PE32 = <path-to>/OdpPkg/Drivers/StandaloneDxeDemo_ru/target/x86_64-unknown-uefi/debug/StandaloneDxeDemo.efi
    SECTION UI = "StandaloneDxeDemo"
  }
```

The `true.depex` file contains a minimal dependency expression consisting of a `TRUE` opcode followed by an `END` opcode. This tells the DXE dispatcher that the driver has no dependencies and can be loaded unconditionally. For more complex dependency expressions, the [Tianocore EDK II Module Writer's Guide](https://tianocore-docs.github.io/) defines how the file is created.

Re-compiling the UEFI with the updated .fdf file should produce a boot log that contains the text "Hello Rust UART DXE Demo!".
