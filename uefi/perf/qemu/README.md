# Boot and performance measurement under QEMU

Boots Patina firmware under QEMU and measures how long it takes, so boot
numbers come from a reproducible machine rather than from hardware.

- `run-q35-boot.sh` boots the firmware and checks that it reaches BDS.
- `make-fbpt-disk.sh` and `capture-fbpt.sh` capture and parse firmware
  performance data.

## Scope

Q35 only. The scripts hardcode `qemu-system-x86_64`, `-machine q35`, and the
`QEMUQ35_CODE.fd` / `QEMUQ35_VARS.fd` flash pair.

## Requirements

- `qemu-system-x86_64` on `PATH`.
- A Q35 firmware directory containing `QEMUQ35_CODE.fd` and `QEMUQ35_VARS.fd`.

## Building firmware

Build [`patina-qemu`](https://github.com/OpenDevicePartnership/patina-qemu)
following [Building the Firmware][building], adding the performance flag:

```sh
stuart_build -c Platforms/QemuQ35Pkg/PlatformBuild.py 'BLD_*_PERF_TRACE_ENABLE=TRUE'
```

Images land in `Build/QemuQ35Pkg/DEBUG_CLANGPDB/FV/`.

`PERF_TRACE_ENABLE` defaults to `FALSE`. Platform PEI always publishes the
Patina performance configuration HOB, which carries the enable state and the
bitmask selecting which measurements are recorded; the DXE Core reads its
configuration from that HOB. The flag is what PEI writes into it, so the
setting is fixed at build time, and swapping in a different DXE Core binary
will not turn measurement on.

## Running

```sh
./run-q35-boot.sh --firmware-dir <dir> --out-dir <dir>
```

`boot-debugcon.log` in the output directory holds the debug console output and
is the first thing to check on failure. The variable store is copied before
use, so the firmware directory stays reusable. Each script documents its own
exit codes in its header.

## Confirming that measurement is enabled

A firmware built with the tracing flag reports the configuration it published
during PEI:

```text
PublishPatinaPerformanceConfigHob: Patina Performance Config HOB: Enabled=1, EnabledMeasurements=0x9
```

`Enabled=0` means the firmware was built without `PERF_TRACE_ENABLE` and will
produce no performance records, though it still boots normally.

## Capturing firmware performance data

Patina publishes a firmware basic boot performance table (FBPT) during boot.
Reading it needs the UEFI Shell application `FbptDump.efi`, which is not part
of a default build. Add `UefiTestingPkg/PerfTests/FbptDump/FbptDump.inf` to
`QemuQ35Pkg.dsc` before building.

Build a disk that boots to the shell and dumps the table, then capture:

```sh
./make-fbpt-disk.sh --build-dir <build>/X64 --out dump-disk.img
./capture-fbpt.sh --firmware-dir <fw> --disk dump-disk.img --out-dir <results>
```

The guest powers itself off once the dump completes, so the capture ends on its
own rather than on a timeout. The output directory receives `FBPT.bin`, the
boot log, the dump application's output, and the parsed `fbpt.xml` /
`fbpt.txt`.

Each capture prints the boot time in milliseconds, taken from the ACPI basic
boot performance record:

```text
boot time (reset to OS loader handoff): 2642.295 ms
  ResetEnd                         0.000 ms
  OSLoaderLoadImageStart        2629.683 ms
  OSLoaderStartImageStart       2642.295 ms
```

Per-phase records are not all on one time base in this firmware, so the summary
stops there; use `fbpt.txt` or `fbpt.xml` for that detail. The summary can also
be run against an existing report:

```sh
python3 boot_time_summary.py <results>/fbpt.xml
```

### Parsing

Parsing needs `edk2-pytool-extensions`:

```sh
pip install edk2-pytool-extensions
```

`capture-fbpt.sh` runs the parser through `fpdt_parser_any_platform.py`, a
wrapper that lets the Windows-oriented `fpdt_parser` run on Linux, where CI
runs. Its module docstring covers the detail.

For a breakdown by module, feed the parsed XML to the report generator with a
source tree to resolve GUIDs against:

```sh
perf_report_generator -i <results>/fbpt.xml -r report.html -s <patina-qemu>
```

Unmatched start records are expected when the guest powers off from the shell,
since phases that would normally end at boot never complete.

[building]: https://github.com/OpenDevicePartnership/patina-qemu/blob/main/docs/src/building/building.md
