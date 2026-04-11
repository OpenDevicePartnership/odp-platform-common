# ODP DXE Demonstration Driver using Rust

This driver is a demonstration of how to create a driver written only in Rust and use it in a typical UEFI build.  The code file `./src/main.rs` contains detailed comments on file layout, debug UART initialization, and possible mechanisms to utilize typical UEFI resources.

## Assumptions and Limitations

This sample assumes the reader is new to Rust but experienced with UEFI and the Tianocore build process.

The driver is compiled outside the EDK II build system and linked into the firmware image via the `.fdf` file. This approach was chosen for simplicity, as integrating Rust into the EDK II build system can be done in several ways and is beyond the scope of this demo.

Because the driver is built independently, it uses a 16550 UART crate for supporting the debug output rather than linking against `DebugLib`. If you need `DebugLib` support, you can integrate the Rust build into the EDK II build system to link C libraries.

For this demo to work on your system, check the UART implementation in `./src/main.rs` to confirm the correct UART crate (`uart_16550`, `pl011`, etc.) and I/O port address for your platform.

## Prerequisites

Install the Rust toolchain via [rustup](https://rustup.rs)  and install the UEFI target for your platform:

``` bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add x86_64-unknown-uefi
```

> **Note:** If your target platform is ARM, replace `x86_64` with `aarch64` (i.e., `aarch64-unknown-uefi`).

## Compile

To compile, navigate to this driver folder and build the code using the proper target installed above:

``` bash
cd ./uefi/OdpPkg/Drivers/ODP_RustDxeDemo
cargo build --target x86_64-unknown-uefi
```

The output will be a PE32+ executable file: `target/x86_64-unknown-uefi/debug/ODP_RustDxeDemo.efi`.

## Insert into UEFI build

The `.efi` file can be added to the UEFI `.fdf` file without a `.dsc` file entry. Replace `<path-to>` below with the actual path relative to your UEFI build tree:

``` text
  FILE DRIVER = 56807AE4-B832-45A4-891E-CAB773564B1C {
    SECTION DXE_DEPEX = <path-to>/OdpPkg/Drivers/ODP_RustDxeDemo/ODP_RustDxeDemo.depex
    SECTION PE32 = <path-to>/OdpPkg/Drivers/ODP_RustDxeDemo/target/x86_64-unknown-uefi/debug/ODP_RustDxeDemo.efi
    SECTION UI = "ODP_RustDxeDemo"
  }
```

The FILE DRIVER guid is not specific, it just needs to be unique for the Firmware File System naming convention.

The DXE_DEPEX `ODP_RustDxeDemo.depex` file provided is a minimal dependency expression consisting of a `PUSH` opcode (0x02) followed by the gEfiVariableArchProtocolGuid and ending in an `END` opcode (0x08) to provide a single dependency to support the Variable Services calls.  For a more complex expression, the [Tianocore EDK II Module Writer's Guide](https://tianocore-docs.github.io/) defines how the file is created.

Re-compiling the UEFI with the updated .fdf file should produce a boot log that contains the text "Hello Rust UART DXE Demo!".
