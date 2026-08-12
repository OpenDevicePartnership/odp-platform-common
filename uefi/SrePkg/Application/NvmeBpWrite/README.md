# NVMe boot-partition development tool

`NvmeBpWrite.efi` is an experimental UEFI application for writing an image
to NVMe Boot Partition 1 (BP1). It is intended for firmware development and
hardware bring-up, not for production deployment or unattended use.

> [!WARNING]
> This tool performs destructive, controller-specific NVMe administrative
> commands. An incompatible controller or interrupted write can leave BP1
> unusable. Review the implementation and validate it on disposable hardware
> before enabling it in a platform build.

## Platform requirements

The current implementation assumes:

- one NVMe controller is present;
- BP1 is the intended destination;
- the controller accepts the FID `0x85` write-enable sequence;
- Firmware Image Download uses `CDW12=1` for boot-partition data;
- Firmware Commit action `110b` targets BP1;
- Get Log Page `0x15` returns a 16-byte preamble followed by BP data; and
- BP capacity is 1 GiB.

These assumptions are not guaranteed by the NVMe specification for every
controller. Platforms should replace hard-coded values with controller
discovery or a platform-specific implementation before production use.

## Staging

`Stage-SreflashUsb.ps1` copies a locally built `NvmeBpWrite.efi` and recovery
image to a removable FAT32 USB. It does not configure firmware boot entries,
remote access, credentials, or target-host policy.

```powershell
./Stage-SreflashUsb.ps1 `
    -ToolPath <build-output>\NvmeBpWrite.efi `
    -WimPath <recovery-image.wim> `
    -UsbDriveLetter E: `
    -Force
```

Use `-WrapWim` only when the recovery flow expects a FAT image containing a
UEFI fallback loader. This option requires elevated PowerShell because it
uses `Mount-VHD` and `Format-Volume`.

To force a write when the initial content check matches, pass
`-ForceReflash`. Remove the generated `\force-reflash.flag` before returning
to normal idempotent behavior.

Boot-entry creation, device reboot, remote administration, and platform
hotkeys are deliberately out of scope. Integrators must use their platform's
documented and reviewed recovery process.
