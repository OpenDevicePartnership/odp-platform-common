# Size Study Info

This is a baseline for doing size comparisons between UEFI drivers written in Rust and written in C.  There are too
many factors that might affect size, so approach this baseline with skepticism.  But it is here as a folder that can
be easily added to a build and modified for experiments to include things like a C based library, Patina modules, etc.

The baseline code in both drivers is for an AARCH64 platform using a PL011 UART for debug messages:

- CHelloWorld - Compiled using an AARCH64 Tianocore based platform build using TARGET=DEBUG to include the debug strings
- Debug RustHelloWorld - Compiled using 'cargo build' with normal build debug information compiled in
- Release RustHelloWorld - Compiled using 'cargo build --release' to remove the debug code hooks, but still retains the
  UEFI UART message strings that occur with TARGET=DEBUG in a normal UEFI build.

## EFI File Comparison

Performing a normal compilation produced the following sized .efi files.  Note that the C driver is *much* larger than
the Rust based driver due to the compiler having segments aligned to 4K instead of Rust's 512 byte alignment.  So most
most of the C based driver is unused (zeroed) regions:

| Driver | Size (bytes) | Size (KiB) | Difference from C |
| --- | ---: | ---: | ---: |
| C | 16,384 | 16.00 | Baseline |
| Rust debug | 13,312 | 13.00 | -18.75% |
| Rust release | 7,680 | 7.50 | -53.13% |

## Compressed File Comparison

To help represent the amount of space each driver would consume in a compressed firmware volume, the following values
were obtained by compressing the .efi drivers to give a better idea of actual code in each file.

| Driver | Size (bytes) | Size (KiB) | Difference from C |
| --- | ---: | ---: | ---: |
| C | 4,201 | 4.10 | Baseline |
| Rust debug | 4,675 | 4.57 | +11.28% |
| Rust release | 3,585 | 3.50 | -14.66% |
