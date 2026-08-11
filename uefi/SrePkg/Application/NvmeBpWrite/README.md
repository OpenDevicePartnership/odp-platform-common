# sre-bp-tools

Kit for flashing a ValidationOS WIM into NVMe Boot Partition 1 (BP1) on a
supported x64 UEFI target and exercising the SRE recovery boot path.

Pre-1.0. Behavior, file layout, and command syntax may change.

## Current state (tool v0.2.2)

`NvmeBpWrite.efi` runs as a UEFI application launched from a removable
FAT32 USB. On every boot:

1. Reads the first 4 KiB of `\ValidationOS.wim` from the USB.
2. Reads the first 4 KiB of BP1 via Get Log Page LID=0x15 (skipping the
   16-byte preamble).
3. Compares them.
4. If match → cold-reset, no NVMe writes.
5. If differ → unlock BP write protection (Set Features FID=0x85 CDW11=0x09),
   chunked Firmware Image Download (CDW12=1), Firmware Commit (CA=110b,
   BPID=1), re-read 4 KiB and report PASS/FAIL, cold-reset.

No NVRAM state is persisted between boots. Re-flashing = replace the WIM
on the USB and boot the tool again.

### Force-reflash override (v0.2.2+)

To re-WRITE BP1 even when the content check would say MATCH (e.g., suspected
corruption past the first 4 KiB, or you want to exercise the WRITE path for
debugging), drop an empty file at `\force-reflash.flag` on the USB. The
tool checks for it at startup and skips the content check if present. The
flag is not auto-deleted — remove it manually (or re-stage without
`-ForceReflash`) to return to normal idempotent behavior.

```powershell
./Stage-SreflashUsb.ps1 -WimPath <wim> -ForceReflash -Force
```

## Files

| File | Runs on | Purpose |
|---|---|---|
| `NvmeBpWrite.efi`         | UEFI         | Flashing tool. Staged as `\EFI\Boot\BOOTX64.EFI` so firmware auto-loads on USB boot. |
| `Stage-SreflashUsb.ps1`   | Workstation  | Stages the tool + WIM + companion scripts onto a removable FAT32 USB. `-WrapWim` wraps the WIM in a bootable FAT image (required for the SRE recovery flow). |
| `BuildBpFatImage.ps1`     | Workstation  | Wraps a raw WIM in a FAT32 image with `bootmgfw.efi` + `BCD` + `boot.sdi` + the WIM. Called by `Stage-SreflashUsb.ps1 -WrapWim`. |
| `enable-remote.ps1`       | Target (once) | Enables WinRM + sets `LocalAccountTokenFilterPolicy=1`. Needed only if driving operations from a workstation over WinRM. |
| `Reset-NvmeBpResult.ps1`  | Target       | Legacy — clears the `NvmeBpResult` NVRAM variable used by tool v0.1. Not needed for re-flashing under v0.2. |
| `Set-NextBootToUsb.ps1`   | Target       | Sets firmware `BootNext` to USB Storage and reboots. Optional alternative to the Vol-Down hotkey. |
| `Flash-BP1.md`            | reference    | End-to-end procedure including content-format trade-offs and known failure modes. |

## Stage and run

Workstation (elevated PowerShell — `Mount-VHD` and `Format-Volume` need admin):

```powershell
./Stage-SreflashUsb.ps1 -WimPath <path-to>\ValidationOS.wim -WrapWim -Force
```

> ⚠ **Run the `Stage-SreflashUsb.ps1` that's in this kit folder, not the
> one inside your Devices repo.** The script auto-detects
> `NvmeBpWrite.efi` and prefers a sibling next to itself. If you run the
> repo-resident copy of the script from inside a Devices clone, it walks
> up to find a `.git`, then picks up your local `stuart_build` output —
> which may be a different (and possibly stale) version than the kit's
> binary. The script now prints the resolved path + size + mtime +
> SHA-256 prefix prominently, with a yellow warning if walk-up fires —
> always sanity-check the displayed mtime and hash before continuing.
> If unsure, pass `-ToolPath <kit>\NvmeBpWrite.efi` explicitly.

Target:

1. Plug USB
2. Shift-shutdown (full power-off, skips Fast Startup)
3. Hold Vol-Down + Power through POST

The tool reports each step on the device screen.

See `Flash-BP1.md` for the full procedure.

## BP1 content formats

Two formats can be written to BP1; the choice depends on what you're testing.

| Format | Tool content check (v0.2) | Vol-Up SRE flow (BpRecoveryLoader → bootmgfw → WinVOS) |
|---|---|---|
| Raw WIM (`-WimPath <wim>`) | Works (idempotent compare) | Fails — BP1 has no FAT volume to chainload from |
| FAT-wrapped (`-WimPath <wim> -WrapWim`) | Works (idempotent compare) | Works — firmware mounts the FAT volume in BP1, chainloads `\EFI\Boot\bootx64.efi` |
