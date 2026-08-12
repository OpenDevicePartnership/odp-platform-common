# Flash a ValidationOS WIM into NVMe Boot Partition 1

End-to-end procedure for writing a ValidationOS (WinVOS) WIM into the SSD's
NVMe Boot Partition 1 (BP1) on a supported development platform using the
`NvmeBpWrite.efi` UEFI application. This is the direct, manual flashing path
— intended for bring-up, debugging, and one-off SRE WIM updates outside the
`Run-WinVosFromBp.ps1` host orchestrator.

## What this procedure does

Writes a bootable image into BP1 — a vendor-specific region of the SSD
that sits outside the GPT partition table. On every boot the tool:

1. **Content check** — reads the first 4 KiB of `\ValidationOS.wim` from
   the USB and the first 4 KiB of BP1 (via Get Log Page LID=0x15, skipping
   the 16-byte preamble) and compares them.
2. **If they match** → BP1 already contains this image. Cold-reset
   immediately. No NVMe writes, no NVRAM activity. (~1 second.)
3. **If they differ** → unlock BP write protection, upload the full WIM
   via chunked `Firmware Image Download` (CDW12=1), commit `CA=110b
   BPID=1`. Re-read first 4 KiB of BP1, report PASS/FAIL. Cold-reset.

The tool keeps **no NVRAM state**. Re-flashing is "drop a new WIM on the
USB and boot the tool again" — works identically in dev iteration,
factory bring-up, field repair, and end-user recovery scenarios, none
of which can assume a working Windows is available to clear an NVRAM
state machine.

> ⚠ **Two BP1 content formats — pick the right one for your goal.** This
> matters because `BpRecoveryLoader`'s chainload expects a FAT volume:
>
> | Format | What's in BP1 | Tool content check | Vol-Up SRE flow (BpRecoveryLoader → bootmgfw → WinVOS) |
> |---|---|---|---|
> | **Raw WIM** (`-WimPath <wim>`) | `MSWIM` magic + WIM bytes at offset 0 | ✅ Works (idempotent reflash via hash compare) | ❌ Fails — BP1 has no FAT volume, no `\EFI\Boot\bootx64.efi` to chainload |
> | **FAT-wrapped** (`-WimPath <wim> -WrapWim`) | FAT32 volume (label `SRE_BP`) containing `bootmgfw.efi` + `BCD` + `boot.sdi` + the WIM | ✅ Works (same idempotent compare) | ✅ Works — firmware partition driver mounts the FAT, BpRecoveryLoader chainloads `bootmgfw.efi` |
>
> If your goal is **just to verify BP1 persistence**, raw WIM is sufficient
> and simpler. If your goal is to
> actually **boot WinVOS from BP1 via the SRE recovery flow**, use
> `-WrapWim`. The two flows can't share the same BP1 contents.

## Source artifacts

### `NvmeBpWrite.efi` (the flashing tool)

Source: [`NvmeBpWrite.c`](./NvmeBpWrite.c) in this directory. The binary is
not published as a standalone ADO artifact — build it locally as part of the
platform build. Pass the resulting binary to the staging script:

```powershell
./Stage-SreflashUsb.ps1 -ToolPath <build-output>\NvmeBpWrite.efi `
    -WimPath <path-to-validationos.wim>
```

Use `NvmeBpWrite.efi`, **NOT** `NvmeBpWriteTest.efi` (same directory) — the
`…Test` variant is a smoke test that does not commit.

### `ValidationOS.wim` (the SRE WIM)

Obtain a bootable recovery WIM from your platform's approved build output.

The zip contains:

```
ValidationOS.wim    (typically ~250 MB)
source.txt          (base image + slipstreamed drivers + run metadata)
```

## Helper scripts (this directory)

Four PowerShell helpers ship next to this doc to remove the manual
keystrokes — they make every step of the procedure a single command.
Each is also documented inline in the sections below.

| Script | Where it runs | What it does |
|---|---|---|
| [`Stage-SreflashUsb.ps1`](./Stage-SreflashUsb.ps1) | workstation | Format-free staging: copies the tool, the WIM, and the three target-side helpers onto a removable FAT32 USB. Accepts `-ToolPath`, `SRE_NVMEBPWRITE_EFI`, or a sibling binary. Replaces Section 1. |
| [`enable-remote.ps1`](./enable-remote.ps1) | target (once per Windows image) | Enables PSRemoting, widens the WinRM firewall to `Any` source, sets `LocalAccountTokenFilterPolicy`. Required if you want to drive the rest of the procedure over WinRM instead of at-keyboard. |
| [`Reset-NvmeBpResult.ps1`](./Reset-NvmeBpResult.ps1) | target (local or via WinRM) | Clears the `NvmeBpResult` UEFI runtime variable via Win32 firmware API. Replaces the manual `SetFirmwareEnvironmentVariableExW` block in Section 5. |
| [`Set-NextBootToUsb.ps1`](./Set-NextBootToUsb.ps1) | target (local or via WinRM) | Sets the firmware `BootNext` one-shot to the `SREFLASH` USB entry and reboots. Eliminates the Vol-Down hotkey dance — no on-device button timing required. |

**Quick start (after one-time `enable-remote.ps1`):**

```powershell
# Workstation: stage USB
./Stage-SreflashUsb.ps1 -WimPath <path-to-validationos.wim>

# Eject + plug into target. Then from workstation over WinRM:
$cred = Get-Credential <target-ip>\<user>
Invoke-Command -ComputerName <target-ip> -Credential $cred -ScriptBlock {
    & "$(([System.IO.DriveInfo]::GetDrives() | ? VolumeLabel -eq 'SREFLASH').Name)Reset-NvmeBpResult.ps1"
    & "$(([System.IO.DriveInfo]::GetDrives() | ? VolumeLabel -eq 'SREFLASH').Name)Set-NextBootToUsb.ps1"
}
# Target reboots → WRITE phase → resets to Windows → reconnect and:
Invoke-Command -ComputerName <target-ip> -Credential $cred -ScriptBlock {
    & "$(([System.IO.DriveInfo]::GetDrives() | ? VolumeLabel -eq 'SREFLASH').Name)Set-NextBootToUsb.ps1"
}
# Target reboots → VERIFY phase → resets to Windows. Done.
```

For step-by-step understanding (or when WinRM isn't available),
follow the detailed sections below.

## Tool internals worth knowing

- WIM filename **must be exactly** `\ValidationOS.wim` at the root of any
  FAT32 volume present at boot. The tool searches all SimpleFileSystem
  volumes for it.
- Target is **BP1** hard-coded (`TARGET_BPID = 1` in source).
- State machine via NVRAM variable `NvmeBpResult`:
  **WRITE → reboot → VERIFY → reboot → NOOP**. The tool auto-detects which
  phase to run on each invocation.
- The tool cold-resets to Windows after each phase. It clears `BootNext` and
  disables the BDS watchdog as safety guards.
- WRITE phase **clobbers existing BP1 contents** on every run.

---

## Section 1 — Prep the tool USB (workstation, ~30 s)

A removable FAT32 USB stick (≥1 GB). The USB must be **labeled `SREFLASH`**
(once, before staging) so the other helper scripts can find it. Don't reuse
a WinPE deployment USB — we want a USB that auto-boots **the flashing
tool**, not Windows.

**One-time per USB** (format + label):

```powershell
Format-Volume -DriveLetter E -FileSystem FAT32 -NewFileSystemLabel SREFLASH -Confirm:$false
```

**Stage the tool + WIM + companion scripts (every re-flash):**

```powershell
# Raw WIM — sufficient for BP1-persistence tests (NvmeBpWrite VERIFY only).
# BpRecoveryLoader SRE flow will NOT boot from this.
./Stage-SreflashUsb.ps1 -WimPath <path-to-validationos.wim>

# OR FAT-wrapped — required for the SRE recovery flow (Vol-Up → WinVOS).
# -WrapWim invokes BuildBpFatImage.ps1 to wrap the WIM in a bootable FAT32
# image (bootmgfw + BCD + boot.sdi + WIM). Run from elevated PowerShell;
# Mount-VHD + Format-Volume need admin. Final BP1 contents will FAIL the
# NvmeBpWrite VERIFY MSWIM scan — that's expected; see Section 4.
./Stage-SreflashUsb.ps1 -WimPath <path-to-validationos.wim> -WrapWim
```

`Stage-SreflashUsb.ps1` auto-detects the single removable FAT32 USB, the
tool binary (via `-ToolPath`, `$env:SRE_NVMEBPWRITE_EFI`, or a sibling file),
and lays out:

```
<USB>:\EFI\Boot\BOOTX64.EFI       ← NvmeBpWrite.efi (firmware auto-executes)
<USB>:\ValidationOS.wim           ← the WIM (filename must be exact)
<USB>:\Reset-NvmeBpResult.ps1     ← target-side helpers
<USB>:\Set-NextBootToUsb.ps1
<USB>:\enable-remote.ps1
```

Hashes of `BOOTX64.EFI` and `ValidationOS.wim` are printed for traceability.

Safely eject and plug into the target.

### Manual fallback (no helper script)

If you can't run the staging script (different workstation, no PowerShell,
etc.), the equivalent manual steps are:

```powershell
New-Item -ItemType Directory -Path "E:\EFI\Boot" -Force | Out-Null
Copy-Item <path-to>\NvmeBpWrite.efi   "E:\EFI\Boot\BOOTX64.EFI"
Copy-Item <path-to>\ValidationOS.wim  "E:\ValidationOS.wim"
```

The WIM filename **must be exactly** `\ValidationOS.wim` at the root of the
FAT32 volume — the tool searches all SimpleFileSystem volumes for that exact
name.

---

## Section 2 — Pre-flight on the target device (~2 min)

1. Confirm the target is a supported x64 platform running a UEFI build that
   includes the `NvmePassThru` protocol.

2. If the target has an existing WinVOS in BP1, this procedure **will
   overwrite it** — confirm this is what you want.

3. (No Windows-side state check is needed.) The tool self-reports its
   NVRAM state on every boot via the `[mode] this run: WRITE | VERIFY | NOOP`
   line in its first ~10 lines of output. The variable that drives this is a
   UEFI runtime variable (`NvmeBpResult` under vendor GUID
   `{7B5A1F3E-2D8C-4A91-B6E3-D8F2C9A4E105}`) — it is **not** a Windows
   environment variable, and standard PowerShell APIs like
   `[System.Environment]::GetEnvironmentVariable` cannot see it. If you'd
   like to read or clear it from Windows, see
   [Section 5](#section-5--reset-state-for-a-future-re-flash).

   What to do based on what the tool reports on its first boot:

   - `[mode] this run: WRITE` — no prior state, proceed normally.
   - `[mode] this run: VERIFY` — a prior WRITE landed but VERIFY hasn't
     run; that boot will execute VERIFY. Skip to
     [Section 4](#section-4--verify-phase-2-min).
   - `[mode] this run: NOOP` — a prior verify already completed and BP1
     is considered clean. To re-flash, clear NVRAM per
     [Section 5](#section-5--reset-state-for-a-future-re-flash), then
     restart from [Section 3](#section-3--write-phase-35-min).

4. Prepare to boot from USB:
   - Power off the target completely (Shift+Shutdown or full power-down).
   - Insert the `SREFLASH` USB into the target's USB-C or USB-A port
     (full-size port preferred over a hub).

---

## Section 3 — WRITE phase (~3–5 min)

1. **Boot the target from USB.** Two options:

   **(a) From Windows on the target (preferred — no hotkey timing):**

   ```powershell
   # Locally (elevated):
   <USB>:\Set-NextBootToUsb.ps1

   # Or remotely via WinRM from the workstation:
   Invoke-Command -ComputerName <target-ip> -Credential (Get-Credential) -ScriptBlock {
       <USB>:\Set-NextBootToUsb.ps1
   }
   ```

   The script sets the firmware `BootNext` one-shot to the `SREFLASH` entry
   and reboots.

   **(b) Hotkey path (no Windows access required):** hold **Volume-Down**
   while pressing the **power button**; keep holding Vol- until the firmware
   boot menu appears. Select **Boot from USB** (the entry showing `SREFLASH`
   / `UEFI USB`). Platform hotkeys vary; consult the platform documentation.

2. The screen blanks briefly, then `NvmeBpWrite` text output begins.
   Expected sequence:

   ```
   NvmeBpWrite — target=BP1, FID=0x85 unlock BP0+BP1 + CDW12=1 download + CA=110b commit, …
   [safety] BootNext cleared: Success
   [safety] BDS watchdog disabled
   [mode] this run: WRITE

   === Unlock: Set Features FID=0x85 CDW11=0x09 (BP0+BP1 = Write Unlocked) ===
     Set Features FID=0x85: Success  …

   === Phase A: upload via Firmware Image Download CDW12=1 ===
     uploaded NNNN MiB (XXX chunks) without failure

   === Phase B: Firmware Commit action=110b BPID=1 ===
     Firmware Commit: Success  …

   [reset to Windows]
   ```

   Total time: ~2–3 minutes for upload + commit. **Do not power-cycle during
   this** — interrupting the upload mid-stream leaves BP1 in an undefined
   state.

3. After Phase B Commit, the tool cold-resets back to Windows automatically.
   Vol+ is **not** held this time, so the device boots into Windows normally.

4. **Stop here and confirm Windows boots** before continuing.

---

## Section 4 — Post-write verification (built-in)

Starting with tool v0.2, the post-write check is **built into the WRITE
phase itself**. After Phase B Commit succeeds, the tool re-reads the first
4 KiB of BP1 via Get Log Page LID=0x15 and compares it to the first 4 KiB
of `\ValidationOS.wim` on the USB:

```
=== Post-write readback ===
  [bp]  post-write first 32 bytes: 4d 53 57 49 4d 1a 00 00  d0 00 00 00 0e …
  Post-write content check (4096 bytes): PASS — BP1 now matches WIM
```

`PASS` confirms the write committed and the bytes on the BP match the
source WIM. `FAIL` means the commit succeeded but the readback bytes
differ — investigate the controller, then re-run.

There is **no separate VERIFY boot**. The tool v0.1 VERIFY phase (which
scanned for `MSWIM` magic across 64 KiB and required a second USB boot)
was removed: it didn't work for the `-WrapWim` path (FAT-wrapped BP1
contents don't carry `MSWIM` magic) and required a Windows-side NVRAM
clear to start over. The content-hash check covers both formats
uniformly.

---

## Section 5 — Re-flashing later

**Just replace `\ValidationOS.wim` on the USB and boot the tool again.**
The tool's content-hash check at startup will detect the change and run
WRITE automatically. No state variable to clear, no Windows-side script
to run — works on factory units with no Windows installed and on units
where Windows is corrupted (the original SRE motivation).

If you boot the tool with **the same** WIM that's already in BP1, you'll
see:

```
Content check (4096 bytes): MATCH — BP1 already contains this WIM
  Nothing to do. Cold-resetting to next boot target.
```

…and the tool cold-resets in <1 second without touching the SSD. So you
can safely re-boot the USB without worrying about wear / accidental
re-flash. The Windows-side `Reset-NvmeBpResult.ps1` script in `sre-bp-tools/`
is **no longer needed for re-flashing** under tool v0.2; it's kept as a
legacy helper only for environments still on v0.1 firmware. Common errors:

| `LastError` | Meaning |
|---|---|
| 5 | Access denied — not running elevated |
| 1314 | Privilege not held — run `enable-remote.ps1` first to set `LocalAccountTokenFilterPolicy=1`, or use the built-in `Administrator` account |
| 203 | Variable wasn't present — already clean, treated as success |

After the clear, on the next tool USB boot the `[mode] this run: …` line
will show `WRITE` and the procedure starts fresh from
[Section 3](#section-3--write-phase-35-min).

### Fallback: reset firmware NVRAM

If `SetFirmwareEnvironmentVariableExW` fails (platform variable policy can
reject deletes on some firmware revisions), the last-resort path is the
firmware setup menu's "Reset to defaults". This clears all vendor NVRAM
including `NvmeBpResult`, at the cost of losing other UEFI configuration.

Then re-run [Section 1](#section-1--prep-the-tool-usb-workstation-5-min)
with the new WIM → [Section 3](#section-3--write-phase-35-min) →
[Section 4](#section-4--verify-phase-2-min).

---

## Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| `[wim] \ValidationOS.wim not found on any FS volume` | WIM not at root of USB, or USB not enumerated | Confirm WIM is literally `<USB>:\ValidationOS.wim` (root, exact name) and the USB mounted as a SimpleFileSystem volume |
| `unlock FAILED — aborting before Phase A` | Controller rejected BP unlock (`Set Features FID=0x85`) | Controller in unexpected state; cold-boot to Windows, retry. If persistent, may indicate a non-Kioxia controller or a controller-firmware mismatch |
| `Firmware Commit: …SC=<non-zero>` | Commit step failed (image rejected by controller) | BP1 may be in a partial state — must re-run WRITE. Clear NVRAM (Section 5) and retry from Section 3 |
| Verify V.2 says `MSWIM signature: NOT FOUND` but V.4 says FOUND | BPMBL/BPRSEL MMIO read path didn't work on this controller (expected on some Kioxia firmware revs) | V.4 is the authoritative check — if V.4 finds MSWIM, the write succeeded |
| Both V.2 and V.4 say NOT FOUND | Write didn't land | Clear NVRAM (Section 5), retry Section 3. If second WRITE also fails verify, capture the full tool serial-console log and escalate |
| `NOOP mode` on first boot of tool USB | Stale NVRAM from a prior session | Section 5 to clear, then re-boot to tool USB |

---

## Related procedures

- `../BpRecoveryLoader/Run-WinVosFromBp.ps1` — the host orchestrator that
  wraps this procedure.
- `SrePkg/Application/BpRecoveryLoader/BpRecoveryLoader.c` — the in-firmware
  SRE app that reads BP1 → RAM disk → chainloads `bootmgfw.efi`.
