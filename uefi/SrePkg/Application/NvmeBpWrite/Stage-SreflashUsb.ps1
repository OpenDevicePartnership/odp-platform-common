# Stage-SreflashUsb.ps1 — workstation helper to stage everything onto a SREFLASH USB.
#
# Stages NvmeBpWrite.efi (as \EFI\Boot\BOOTX64.EFI), the WIM, and the
# target-side companion scripts (Reset-NvmeBpResult.ps1 and
# Set-NextBootToUsb.ps1) onto a removable FAT32 USB. Idempotent.
#
# Tool path resolution (first hit wins):
#   1. -ToolPath argument
#   2. $env:SRE_NVMEBPWRITE_EFI
#   3. .\NvmeBpWrite.efi (sibling to this script)
#
# Examples:
#   # Raw-WIM path (NvmeBpWrite VERIFY will FIND MSWIM, but BpRecoveryLoader
#   # SRE flow will fail to chainload — BP1 contains a WIM, not a FAT volume).
#   ./Stage-SreflashUsb.ps1 -WimPath ~\Downloads\ValidationOS.wim
#
#   # FAT-wrapped path — required for the SRE recovery flow (Vol-Up boot).
#   # Wraps the WIM in a bootable FAT32 image with bootmgfw + BCD + boot.sdi
#   # via BuildBpFatImage.ps1. Run from elevated PowerShell. VERIFY will
#   # report "MSWIM: NOT FOUND" — that's expected; BP1 now contains a FAT
#   # volume that BpRecoveryLoader can chainload \EFI\Boot\bootx64.efi from.
#   ./Stage-SreflashUsb.ps1 -WimPath ~\Downloads\ValidationOS.wim -WrapWim
#
#   # Explicit USB drive + force overwrite.
#   ./Stage-SreflashUsb.ps1 -WimPath C:\drops\ValidationOS.wim -UsbDriveLetter F: -Force

[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$WimPath,
    [string]$UsbDriveLetter,
    [string]$ToolPath,
    [string]$CompanionDir = $PSScriptRoot,
    # -WrapWim wraps the WIM in a bootable FAT32 image (via BuildBpFatImage.ps1)
    # before staging. Required when targeting the BpRecoveryLoader SRE flow,
    # which reads BP1 as a FAT volume and chainloads \EFI\Boot\bootx64.efi.
    # Without this, BP1 contains a raw WIM that NvmeBpWrite VERIFY accepts
    # (MSWIM at offset 0) but the SRE app cannot chainload from. Requires
    # elevated PowerShell because Mount-VHD/Format-Volume need admin.
    [switch]$WrapWim,
    [string]$BuildBpFatImagePath,
    # Default to the kit-bundled x64 bootmgfw if present, otherwise the host's
    # local Windows files. The bundled copy matters when the staging host has
    # a different architecture than the x64 target.
    [string]$BootmgfwEfi,
    [string]$BootSdi,
    # -ForceReflash drops \force-reflash.flag on the USB. The tool (v0.2.1+)
    # treats that as "skip the content check and re-WRITE unconditionally
    # even if BP1 already matches the WIM."
    [switch]$ForceReflash,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

function Fail($msg) { Write-Error $msg; exit 1 }

# --- Resolve tool path (auto-detect if not given) ---
#
# Resolution order:
#   1. -ToolPath argument (explicit, always wins)
#   2. $env:SRE_NVMEBPWRITE_EFI
#   3. Sibling NvmeBpWrite.efi (next to this script — kit layout)
#
# Whichever wins, the selection (path / size / mtime / hash prefix) is
# printed prominently so the operator can sanity-check it.

function Format-ToolSelection {
    param([string]$Source, [string]$Path)
    $f    = Get-Item $Path
    $hash = (Get-FileHash $Path -Algorithm SHA256).Hash.Substring(0, 16)
    Write-Host ""
    Write-Host "NvmeBpWrite.efi resolved via $Source" -ForegroundColor Cyan
    Write-Host "  Path:    $Path"
    Write-Host "  Size:    $($f.Length) bytes"
    Write-Host "  Mtime:   $($f.LastWriteTime)"
    Write-Host "  SHA256:  $hash..."
    Write-Host ""
}

function Resolve-ToolPath {
    param([string]$Explicit)

    if ($Explicit) {
        $p = (Resolve-Path -LiteralPath $Explicit -ErrorAction Stop).Path
        Format-ToolSelection '-ToolPath argument (explicit)' $p
        return $p
    }
    if ($env:SRE_NVMEBPWRITE_EFI -and (Test-Path $env:SRE_NVMEBPWRITE_EFI)) {
        $p = (Resolve-Path $env:SRE_NVMEBPWRITE_EFI).Path
        Format-ToolSelection '$env:SRE_NVMEBPWRITE_EFI' $p
        return $p
    }
    $sibling = Join-Path $PSScriptRoot 'NvmeBpWrite.efi'
    if (Test-Path $sibling) {
        $p = (Resolve-Path $sibling).Path
        Format-ToolSelection "sibling of script ($PSScriptRoot)" $p
        return $p
    }

    return $null
}

$tool = Resolve-ToolPath -Explicit $ToolPath
if (-not $tool) {
    Fail @"
NvmeBpWrite.efi not found. Provide it one of:
  - Pass -ToolPath <path>
  - Set `$env:SRE_NVMEBPWRITE_EFI = '<path>'`
  - Drop NvmeBpWrite.efi next to this script
"@
}

# --- Resolve USB ---
if (-not $UsbDriveLetter) {
    $cand = @(Get-Volume -ErrorAction SilentlyContinue |
        Where-Object { $_.DriveType -eq 'Removable' -and $_.FileSystem -eq 'FAT32' -and $_.DriveLetter })
    if ($cand.Count -eq 0) { Fail "No removable FAT32 volume found. Pass -UsbDriveLetter explicitly." }
    if ($cand.Count -gt 1) { Fail "Multiple removable FAT32 volumes: $($cand.DriveLetter -join ', '). Pass -UsbDriveLetter to pick." }
    $UsbDriveLetter = "$($cand[0].DriveLetter):"
}
$UsbDriveLetter = ($UsbDriveLetter.TrimEnd('\') -replace ':$','') + ':'
if (-not (Test-Path $UsbDriveLetter)) { Fail "USB drive '$UsbDriveLetter' not accessible." }

# --- Validate inputs ---
if (-not (Test-Path $WimPath)) { Fail "WIM not found: $WimPath" }
$wim = Get-Item $WimPath
$toolItem = Get-Item $tool

# --- Wrap WIM in a bootable FAT32 image, if requested ---
if ($WrapWim) {
    $isAdmin = ([Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $isAdmin) { Fail "-WrapWim requires elevated PowerShell (Mount-VHD/Format-Volume need admin)." }

    # Resolve BuildBpFatImage.ps1: explicit > sibling > sister-app directory.
    $bbfi = $BuildBpFatImagePath
    if (-not $bbfi) {
        $candidates = @(
            (Join-Path $PSScriptRoot 'BuildBpFatImage.ps1'),
            (Join-Path $PSScriptRoot '..\BpRecoveryLoader\BuildBpFatImage.ps1')
        )
        foreach ($c in $candidates) {
            if (Test-Path $c) { $bbfi = (Resolve-Path $c).Path; break }
        }
    }
    if (-not $bbfi -or -not (Test-Path $bbfi)) {
        Fail "BuildBpFatImage.ps1 not found. Pass -BuildBpFatImagePath, or place the script next to this one / in the sibling BpRecoveryLoader directory."
    }
    # Resolve bootmgfw.efi + boot.sdi. Prefer the kit-bundled x64 copies
    # before falling back to the host's local Windows files.
    function Resolve-BootFile {
        param($Explicit, $KitRelative, $HostDefault, $Label)
        if ($Explicit) {
            if (-not (Test-Path $Explicit)) { Fail "$Label not found at '$Explicit'. Pass -$Label." }
            return (Resolve-Path $Explicit).Path
        }
        $candidates = @(
            (Join-Path $PSScriptRoot $KitRelative),
            $HostDefault
        )
        foreach ($c in $candidates) {
            if (Test-Path $c) { return (Resolve-Path $c).Path }
        }
        Fail "$Label not found in kit (boot-x64/) or on host ('$HostDefault'). Pass -$Label explicitly."
    }
    $BootmgfwEfi = Resolve-BootFile -Explicit $BootmgfwEfi -KitRelative 'boot-x64\bootmgfw.efi' -HostDefault 'C:\Windows\Boot\EFI\bootmgfw.efi' -Label 'BootmgfwEfi'
    $BootSdi     = Resolve-BootFile -Explicit $BootSdi     -KitRelative 'boot-x64\boot.sdi'    -HostDefault 'C:\Windows\Boot\DVD\EFI\boot.sdi' -Label 'BootSdi'

    # Sanity-check the resolved bootmgfw.efi is actually x64. The target
    # firmware rejects a binary built for a different architecture.
    function Get-PeMachineSafe($path) {
        try {
            $b = [System.IO.File]::ReadAllBytes($path)
            $peOff = [BitConverter]::ToUInt32($b, 0x3C)
            $m = [BitConverter]::ToUInt16($b, $peOff + 4)
            switch ($m) { 0x8664 {'x64'} 0xAA64 {'AARCH64'} 0x014C {'i386'} default {'0x{0:X4}' -f $m} }
        } catch { 'unreadable' }
    }
    $bootmgfwArch = Get-PeMachineSafe $BootmgfwEfi
    Write-Host ""
    Write-Host "bootmgfw.efi resolved" -ForegroundColor Cyan
    Write-Host "  Path:  $BootmgfwEfi"
    Write-Host "  Arch:  $bootmgfwArch"
    Write-Host "  SDI:   $BootSdi"
    Write-Host ""
    if ($bootmgfwArch -ne 'x64') {
        Fail "bootmgfw.efi at '$BootmgfwEfi' is $bootmgfwArch, not x64. Either:`n  - Run from a kit folder that has boot-x64/bootmgfw.efi (preferred), or`n  - Pass -BootmgfwEfi <path> pointing at an x64 bootmgfw.efi."
    }

    $imgOut = Join-Path $env:TEMP ("bp1-$([IO.Path]::GetFileNameWithoutExtension($wim.Name))-{0}.img" -f (Get-Date -Format yyyyMMdd-HHmmss))
    "Wrapping WIM in FAT32 image via BuildBpFatImage.ps1 ..."
    "  script:  $bbfi"
    "  wim:     $($wim.FullName)"
    "  output:  $imgOut"
    & $bbfi -WimFile $wim.FullName -OutImage $imgOut -BootmgfwEfi $BootmgfwEfi -BootSdi $BootSdi
    if (-not (Test-Path $imgOut)) { Fail "BuildBpFatImage produced no image at $imgOut." }
    $wim = Get-Item $imgOut
    "Wrapped image: $($wim.FullName) ($([math]::Round($wim.Length/1MB,1)) MB)"
    ""
}

"Tool:  $($toolItem.FullName) ($($toolItem.Length) bytes)"
"WIM:   $($wim.FullName) ($([math]::Round($wim.Length/1MB,2)) MB)" + $(if ($WrapWim) {' [FAT-wrapped]'} else {''})
"USB:   $UsbDriveLetter"
""

$reset    = Join-Path $CompanionDir 'Reset-NvmeBpResult.ps1'
$setbn    = Join-Path $CompanionDir 'Set-NextBootToUsb.ps1'
$enable   = Join-Path $CompanionDir 'enable-remote.ps1'

# --- Stage ---
$dstEfiDir = Join-Path $UsbDriveLetter 'EFI\Boot'
$dstBoot   = Join-Path $dstEfiDir 'BOOTX64.EFI'
$dstWim    = Join-Path $UsbDriveLetter 'ValidationOS.wim'

if ((Test-Path $dstWim) -and -not $Force) {
    Fail "$dstWim already exists. Pass -Force to overwrite."
}

New-Item -ItemType Directory -Path $dstEfiDir -Force | Out-Null
Copy-Item $toolItem.FullName $dstBoot -Force
Copy-Item $wim.FullName      $dstWim  -Force

# Drop or remove the force-reflash flag depending on -ForceReflash. The
# tool (v0.2.1+) checks for this file at boot; if present, it skips the
# content check and re-WRITEs unconditionally.
$flagPath = Join-Path $UsbDriveLetter 'force-reflash.flag'
if ($ForceReflash) {
    "" | Set-Content $flagPath
    Write-Host "Created $flagPath — tool will re-WRITE BP1 even if it already matches the WIM." -ForegroundColor Yellow
} elseif (Test-Path $flagPath) {
    Remove-Item $flagPath -Force
    Write-Host "Removed pre-existing $flagPath — tool will run normal content-hash check." -ForegroundColor Cyan
}
foreach ($s in @($reset, $setbn, $enable)) {
    if (Test-Path $s) {
        Copy-Item $s (Join-Path $UsbDriveLetter (Split-Path $s -Leaf)) -Force
    } else {
        Write-Host "WARN: companion script not found, skipping: $s" -ForegroundColor Yellow
    }
}

# --- Verify ---
"--- Staged onto $UsbDriveLetter ---"
Get-ChildItem $UsbDriveLetter -Recurse -File |
    Where-Object { $_.Name -notmatch '^(WPSettings|IndexerVolume)' } |
    Select-Object @{n='Path';e={$_.FullName.Substring($UsbDriveLetter.Length)}}, Length |
    Format-Table -AutoSize

"--- Hashes ---"
foreach ($f in @($dstBoot, $dstWim)) {
    if (Test-Path $f) {
        $h = Get-FileHash $f -Algorithm SHA256
        "{0}: {1}" -f $f, $h.Hash
    }
}

""
"Ready. Eject the USB and plug into the target."
