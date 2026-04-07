# Standalone DXE Demo Driver using Rust

This driver is a demonstration of how to create a driver written only in Rust, compile it as a stand-alone .efi executable, then integrate it into a firmware volume for a UEFI DXE Core to dispatch.

This driver assumes a DEBUG target build that uses a 16550 UART for debug output because it is compiled separately from the UEFI build and has no access to Platform Configuration Database (PCD) values. To address this, you could rewrite the Rust code to locate the PCD protocol, read the debug port settings, and configure the `log` crate accordingly.  For this demo to perform properly on your system, please examine the UART implementation in the `./src/main.rs` file for the proper crate (16550, pl011, etc.) and I/O ports.

## Compile

Before building, install the UEFI target for your platform:

``` bash
rustup target add x86_64-unknown-uefi
```

> **Note:** If your host is an ARM platform, replace `x86_64` with `aarch64` (i.e., `aarch64-unknown-uefi`).

Then switch to this driver folder and build:

``` bash
cd ./uefi/OdpPkg/Drivers/StandaloneDxeDemo_ru
cargo build
```

## Insert into UEFI build

The build process will create a file called `./uefi/OdpPkg/Drivers/StandaloneDxeDemo_ru/target/x86_64-unknown-efi/debug/StandaloneDxeDemo.efi`.  This file can be added to the UEFI .fdf file without the .dsc file entry.

``` text
  FILE DRIVER = 35AFEBCD-8485-4865-A9EC-447FF8EA47A9 {
    SECTION DXE_DEPEX = .../OdpPkg/Drivers/StandaloneDxeDemo_ru/true.depex
    SECTION PE32 = .../OdpPkg/Drivers/StandaloneDxeDemo_ru/target/x86_64-unknown-efi/debug/StandaloneDxeDemo.efi
    SECTION UI = "StandaloneDxeDemo"
  }
```

The `true.depex` file contains a minimal dependency expression consisting of a `TRUE` opcode followed by an `END` opcode. This tells the DXE dispatcher that the driver has no dependencies and can be loaded unconditionally. For more complex dependency expressions, the [Tianocore EDK II Module Writer's Guide](https://tianocore-docs.github.io/) defines how the file is created.

Re-compiling the UEFI with the updated .fdf file should produce a boot log that contains the text "Hello Rust UART DXE Demo!".
