# SRE Boot on Surface OEM (Msft900Maa) — Onboarding

End-to-end notes for getting a new engineer productive on the System Recovery
Environment (SRE) boot path. Covers repo setup, building firmware, writing the
SRE WIM into the NVMe Boot Partition, and exercising the boot.

## Repos

```
git clone <devices-internal-url> sre/Devices
cd sre/Devices
git checkout -b personal/<your-alias>/sre origin/main
```

Work in your personal branch. Submit PRs from a personal fork; do not push
feature branches to upstream.

## Build environment

Windows + VS2022 only. The WSL build path is non-functional for this platform.

```
python -m venv .venv
.venv\Scripts\Activate.ps1
pip install -r pip-requirements.txt
stuart_setup  -c Platform/Surface/SurfPtl/Msft900MaaPkg/PlatformBuild.py TOOL_CHAIN_TAG=VS2022
stuart_update -c Platform/Surface/SurfPtl/Msft900MaaPkg/PlatformBuild.py TOOL_CHAIN_TAG=VS2022
```

## Build firmware

```
stuart_build -c Platform/Surface/SurfPtl/Msft900MaaPkg/PlatformBuild.py \
             TOOL_CHAIN_TAG=VS2022 PROFILE=DEV
```

Output ROM:
`Build/Msft900MaaPkg/DEBUG_VS2022/ROM/900_MAA-UEFI-<date>-<branch>-<sha>-<ver>.bin`

Incremental builds typically 3–6 min after the first clean build (~30 min).

## Flash firmware

Use the internal Surface flashing procedure (bench rig + USB → DUT). Ask
the SRE team for the current playbook.

## Write the SRE WIM into NVMe Boot Partition 1

Once per device (or when the WIM changes):

```
pwsh ./MsSurfaceIntelPkg/Application/BpRecoveryLoader/Run-WinVosFromBp.ps1 `
     -WimPath <path-to-validationos.wim> `
     -DutAddress <dut-ip> `
     -DutCredential (Get-Credential)
```

What it does:

1. Wraps the WIM in a bootable FAT32 image (calls `BuildBpFatImage.ps1`).
2. Stages the image + `NvmeBpWrite.efi` on a USB stick attached to the DUT.
3. Reboots the DUT into the `NvmeBpWrite` UEFI shell app via a firmware boot
   entry. That app:
   - Issues `Set Features FID=0x85 BPWPS=001b` to unlock BP write protection.
   - Streams the FAT image into BP1 via `Firmware Image Download` (with
     CDW12=1 vendor route) and commits via `Firmware Commit CA=110b BPID=1`.
   - Verifies via BPMBL/BPRSEL MMIO and Get Log Page LID=0x15.
4. Reboots back to Windows.

After this point, BP1 is populated. BP write protection (lock/unlock) is
otherwise owned by the FMP capsule update flow — neither the SRE boot path
nor any other in-tree code touches BPWPS.

## Test the SRE boot

| Action | Expected |
|---|---|
| Power on, no buttons held | Normal Windows boot |
| Hold Vol+ while pressing Power | SRE flow: BP1 → RAM disk → bootmgfw → WinVOS |
| Hold Vol- while pressing Power, USB plugged in | USB-first alt boot (Windows installer etc.) |
| Hold Vol- while pressing Power, no USB | Surface FrontPage (firmware UI) |

Total SRE boot time on Maa: ~5s end-to-end (~2s for BP read at the current
1 MiB chunk size, plus RAM disk register + FAT mount + bootmgfw load).

## Where the code lives

| Component | Path |
|---|---|
| SRE hotkey routing (PCD-gated, BdsDxe-side) | `MsSurfaceIntelPkg/Library/DeviceBootManagerLib/DeviceBootManagerLib.c` — search for `SrePriorityBoot` |
| SRE app (read BP → RAM disk → chainload bootx64.efi) | `MsSurfaceIntelPkg/Application/BpRecoveryLoader/BpRecoveryLoader.c` |
| SRE app INF (FILE_GUID dispatched by priority-boot) | `MsSurfaceIntelPkg/Application/BpRecoveryLoader/BpRecoveryLoaderApp.inf` |
| BP write tool (dev/bring-up) | `MsSurfaceIntelPkg/Application/NvmeBpWrite/NvmeBpWrite.c` |
| ESP-based counterpart to BpRecoveryLoader | `MsSurfaceIntelPkg/Application/SreRecoveryLoader/SreRecoveryLoader.c` |
| Feature flag + SRE app GUID PCD declarations | `MsSurfaceIntelPkg/MsSurfaceIntelPkg.dec` |
| Platform opt-in (PCDs + FDF entry) | `Platform/Surface/SurfPtl/Msft900MaaPkg/Msft900MaaPkg.dsc`, `.fdf` |
| Host orchestration script | `MsSurfaceIntelPkg/Application/BpRecoveryLoader/Run-WinVosFromBp.ps1` |
| Host WIM-to-FAT script | `MsSurfaceIntelPkg/Application/BpRecoveryLoader/BuildBpFatImage.ps1` |

## Enabling on a new Intel Surface platform

The SRE hotkey routing is FeaturePcd-gated, default off. To opt a new
platform in:

```
# In <YourPlatformPkg>.dsc:
[PcdsFeatureFlag]
  gSrePkgTokenSpaceGuid.PcdSreHotkeyRoutingEnabled|TRUE

[PcdsFixedAtBuild.common]
  # Set to your SRE app's INF FILE_GUID, little-endian byte order
  gSrePkgTokenSpaceGuid.PcdSreBootAppFileGuid|{ ... 16 bytes ... }
```

And in `<YourPlatformPkg>.fdf [FV.DXE]`, include `RamDiskDxe` + your SRE app
(or `BpRecoveryLoaderApp.inf` if you're reusing the same SRE app).

Other Intel Surface platforms share the same `DeviceBootManagerLib` and will
keep their legacy hotkey behavior because the PCD defaults to FALSE.

## Serial capture (debugging)

UART hook to the bench rig. Useful log lines from a healthy SRE boot:

```
[SrePriorityBoot] [Bds] Vol+ -> SRE app dispatch
[Bds] Expand Fv(...)/FvFile(<PcdSreBootAppFileGuid>) -> ...
INFO - Loaded image at ... BpRecoveryLoaderApp.efi
[BpRecoveryLoader] enter RunSreFlow (BPID=1)
[BpRecoveryLoader] ConnectAll: N handles
[BpRecoveryLoader] step 1 OK (PassThru=...)
[BpRecoveryLoader] step 2 - reading 1024 MiB from BP1 via LID 0x15 (LPOL base = 16)
[BpRecoveryLoader] 16 MiB read
...
[BpRecoveryLoader] step 2 OK (1073741824 bytes -> phys 0x...)
[BpRecoveryLoader] step 3 OK (1024 MiB at phys ..., RamDp=...)
[BpRecoveryLoader] step 4 FAT volume on handle ...
[BpRecoveryLoader] step 4 LoadImage OK, StartImage...
```

If something fails, the `step N FAIL` line tells you which step bailed and
the EFI/NVMe status. The app also writes a `BpLoaderResult` NVRAM variable
that survives the reboot — readable from Windows for post-mortem.

## Project board

https://github.com/orgs/OpenDevicePartnership/projects/40/views/1

C-track items proven on Maa are closed (see "(C-side, OEM)" suffix). The
Patina-Rust port lives under the "Patina Port (Investigation)" milestone —
parallel research track, not blocking the C-side ship.
