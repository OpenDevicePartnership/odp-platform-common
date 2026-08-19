# Size Study Info

This is a baseline for doing size comparisons between UEFI drivers written in Rust and written in C.  There are too
many factors that might affect size, so approach this baseline with skepticism.  But it is here as a folder that can
be easily added to a build and modified for experiments to include things like a C based library, Patina modules, etc.

The baseline code in both drivers is for an AARCH64 platform, uses a PL011 UART for debug messages, and doesn't have
a CI nor version pinning.  These choices were made so that the Radxa Orion O6 demo repository could be used to compile
the code and provide just a starting point for using in your own infrastructure.  It is not intended to be a final say
on size nor possible optimizations.

- CHelloWorld - Compiled by adding the .INF to the Radxa Orion O6 platform .dsc file and the default UEFI build compiler
  optimizations from the included EDK2.
- RustHelloWorld - Compiled using 'cargo build' relying on the .cargo/config.toml to indicate the target.  It also
  passes `-C force-unwind-tables` to emit a .pdata section for stack tracing, which could be removed for optimization.

## EFI File Comparison

Performing a normal compilation produced the following sized .efi files.  Note that the C driver is *much* larger than
the Rust based driver due to the compiler having segments aligned to 4K instead of Rust's 512 byte alignment.  So most
of the C driver's binary filesize is unused (zeroed) regions:

| Driver | Size (bytes) | Difference from C |
| --- | ---: | ---: |
| C | 16,384 | Baseline |
| Rust | 7,168 | -56.3% |

## Compressed File Comparison

To help represent the amount of space each driver would consume in a compressed firmware volume, the following values
were obtained by compressing the .efi drivers to give a better idea of actual code in each file.

| Driver | Size (bytes) | Difference from C |
| --- | ---: | ---: |
| C | 4,210 | Baseline |
| Rust | 3,536 | -16.0% |

## Going Forward

These results should not be taken as a general comparison of C and Rust code size.  The C driver includes overhead
from the larger UEFI build infrastructure and its predefined library dependencies, while the Rust driver uses a
smaller, more direct build configuration.  This makes it difficult to separate the size of the source code from the
size contributed by each build system.  Instead, use these drivers as baselines: add the same feature to each and
compare size changes to help inform future implementation choices.
