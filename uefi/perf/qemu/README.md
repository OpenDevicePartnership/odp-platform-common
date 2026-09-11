# Boot and performance measurement under QEMU

Boots Patina firmware under QEMU and reports whether it reaches BDS, so boot
behavior can be checked on a reproducible machine rather than on hardware.
`run-q35-boot.sh` is the entry point; capturing the firmware's own performance
data builds on it.

## Scope

Q35 only. The script hardcodes `qemu-system-x86_64`, `-machine q35`, and the
`QEMUQ35_CODE.fd` / `QEMUQ35_VARS.fd` flash pair.

## Requirements

- `qemu-system-x86_64` on `PATH`.
- A Q35 firmware directory containing `QEMUQ35_CODE.fd` and `QEMUQ35_VARS.fd`.

## Building firmware

Build [`patina-qemu`](https://github.com/OpenDevicePartnership/patina-qemu)
following [Building the Firmware][building]:

```sh
stuart_build -c Platforms/QemuQ35Pkg/PlatformBuild.py
```

Images land in `Build/QemuQ35Pkg/DEBUG_CLANGPDB/FV/`.

## Running

```sh
./run-q35-boot.sh --firmware-dir <dir> --out-dir <dir>
```

`boot-debugcon.log` in the output directory holds the debug console output and
is the first thing to check on failure. The variable store is copied before
use, so the firmware directory stays reusable. The script documents its exit
codes in its header.

[building]: https://github.com/OpenDevicePartnership/patina-qemu/blob/main/docs/src/building/building.md
