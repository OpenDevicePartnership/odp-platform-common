<#
.SYNOPSIS
  End-to-end SRE demo: stage a recovery WIM into NVMe BP1 and chainload
  WinVOS (or any bootable WIM) from a RAM disk backed by that BP.

.DESCRIPTION
  One-command orchestrator for the BP -> RAM disk -> bootmgfw -> WinVOS
  chain. Takes a stock CI ValidationOS.wim, wraps it in a bootable FAT32
  layout, stages on a USB stick, deploys the NvmeBpWrite and
  BpRecoveryLoader EFI apps to the DUT ESP, and drives the firmware
  boot-entry dance:

      1) Reboot into NvmeBpWrite (firmware boot entry).
         The tool searches all FAT volumes for \ValidationOS.wim, finds
         it on the USB, and streams it into BP1 via Firmware Image
         Download (CDW12=1) + Firmware Commit (CA=110b).
      2) Reboot into BpRecoveryLoader (firmware boot entry).
         The tool reads BP1 via Get Log Page LID=0x15, locks both BPs
         (FID=0x85 BPWPS=011b lock-until-power-cycle), registers the
         buffer via EFI_RAM_DISK_PROTOCOL, and chainloads
         \EFI\Boot\bootx64.efi (= bootmgfw.efi inside the FAT image).
      3) bootmgfw reads BCD's ramdisk options, loads boot.wim, runs
         winload.efi -> WinVOS comes up.

  Reports pass/fail. On success, the DUT lands at the WinVOS cmd.exe
  prompt and host WinRM goes silent until the operator reboots it.

.PARAMETER WimFile
  Path to ValidationOS.wim (or any bootable Windows WIM from CI).

.PARAMETER DutHost
  DUT host or IP. Default 192.168.1.81.

.PARAMETER DutCredential
  PSCredential for the DUT. If omitted, prompts for testuser password.

.PARAMETER UsbDrive
  Drive letter of the USB stick where \ValidationOS.wim should be staged
  (e.g. "D"). If omitted, auto-selects the first removable FAT32 volume.

.PARAMETER NvmeBpWriteEfi
  Path to NvmeBpWrite.efi. Defaults to the newest matching x64 binary under
  the repository's Build directory.

.PARAMETER BpRecoveryLoaderEfi
  Path to BpRecoveryLoader.efi. Uses the same build-output discovery.

.PARAMETER WorkDir
  Working directory for the intermediate FAT image. Default $env:TEMP.

.PARAMETER ImageSizeBytes
  Final FAT image size. Default 280 MiB. Bump if WIM > ~250 MiB.

.PARAMETER SkipBuild
  Skip building the FAT image (use the existing -OutImage instead).
  Use when iterating on firmware/EFI deploy without rebuilding the image.

.EXAMPLE
  .\Run-WinVosFromBp.ps1 -WimFile "C:\path\to\ValidationOS.wim"
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string] $WimFile,
    [string] $DutHost           = '192.168.1.81',
    [System.Management.Automation.PSCredential] $DutCredential,
    [string] $UsbDrive,
    [string] $NvmeBpWriteEfi,
    [string] $BpRecoveryLoaderEfi,
    [string] $WorkDir           = $env:TEMP,
    [long]   $ImageSizeBytes    = 280MB,
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'

$RESULT_GUID = '{7B5A1F3E-2D8C-4A91-B6E3-D8F2C9A4E105}'
$ESP_GPT_TYPE = '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}'

function Resolve-DefaultEfi {
    param([string] $Hint, [string] $FileName)
    if ($Hint) { return $Hint }

    $repoRoot = $PSScriptRoot
    while ($repoRoot -and -not (Test-Path (Join-Path $repoRoot '.git'))) {
        $parent = Split-Path $repoRoot -Parent
        if ($parent -eq $repoRoot) { $repoRoot = $null; break }
        $repoRoot = $parent
    }
    if (-not $repoRoot) {
        throw "Repository root not found. Pass -$($FileName -replace '\.efi$')Efi explicitly."
    }

    $buildRoot = Join-Path $repoRoot 'Build'
    if (Test-Path $buildRoot) {
        $candidate = Get-ChildItem -Path $buildRoot -Filter $FileName -File -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '[\\/]X64[\\/]' } |
            Sort-Object LastWriteTimeUtc -Descending |
            Select-Object -First 1
        if ($candidate) { return $candidate.FullName }
    }

    return Join-Path $buildRoot $FileName
}

if (-not $DutCredential) {
    Write-Host "Enter DUT password for testuser:"
    $DutCredential = Get-Credential -UserName 'testuser' -Message "DUT $DutHost"
}

if (-not (Test-Path $WimFile)) { throw "WimFile not found: $WimFile" }

$NvmeBpWriteEfi = Resolve-DefaultEfi $NvmeBpWriteEfi 'NvmeBpWrite.efi'
$BpRecoveryLoaderEfi = Resolve-DefaultEfi $BpRecoveryLoaderEfi 'BpRecoveryLoader.efi'

foreach ($p in @($NvmeBpWriteEfi, $BpRecoveryLoaderEfi)) {
    if (-not (Test-Path $p)) { throw "Required EFI not found: $p (build the platform first, or pass an override)" }
}

$ImagePath = Join-Path $WorkDir 'bp1-winvos.img'

# --- 1. Build the bootable FAT image ---------------------------------------
if (-not $SkipBuild) {
    $builder = Join-Path $PSScriptRoot 'BuildBpFatImage.ps1'
    if (-not (Test-Path $builder)) { throw "BuildBpFatImage.ps1 not found at $builder" }
    Write-Host "[1/6] Building FAT image -> $ImagePath ..."
    & $builder -WimFile $WimFile -OutImage $ImagePath -SizeBytes $ImageSizeBytes
    if ($LASTEXITCODE -ne 0) { throw "BuildBpFatImage.ps1 failed (exit $LASTEXITCODE)" }
} else {
    if (-not (Test-Path $ImagePath)) { throw "-SkipBuild set but $ImagePath does not exist" }
    Write-Host "[1/6] -SkipBuild: using existing $ImagePath"
}

# --- 2. Stage to USB on host -----------------------------------------------
if (-not $UsbDrive) {
    $vol = Get-Volume | Where-Object { $_.DriveType -eq 'Removable' -and $_.FileSystem -eq 'FAT32' -and $_.DriveLetter } | Select-Object -First 1
    if (-not $vol) { throw "No removable FAT32 volume found on host. Plug in a USB stick or pass -UsbDrive." }
    $UsbDrive = "$($vol.DriveLetter)"
}
if ($UsbDrive.Length -gt 1) { $UsbDrive = $UsbDrive.Substring(0, 1) }
$usbPath = "${UsbDrive}:\ValidationOS.wim"
$imageBytes = (Get-Item $ImagePath).Length
Write-Host "[2/6] Staging $ImagePath ($([Math]::Round($imageBytes/1MB,1)) MiB) -> $usbPath ..."
Copy-Item -Path $ImagePath -Destination $usbPath -Force

# --- 3. Open PSSession + DUT prep ------------------------------------------
Write-Host "[3/6] Connecting to DUT $DutHost ..."
$session = New-PSSession -ComputerName $DutHost -Credential $DutCredential

# Embedded P/Invoke source for EFI variable access from PowerShell.
$fwEnvSrc = @'
using System;
using System.Runtime.InteropServices;
public class FwEnv {
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern uint GetFirmwareEnvironmentVariableExW(string n, string g, byte[] b, uint s, out uint a);
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool SetFirmwareEnvironmentVariableExW(string n, string g, IntPtr b, uint s, uint a);
    [DllImport("advapi32.dll", SetLastError=true)] public static extern bool OpenProcessToken(IntPtr h, uint da, out IntPtr ph);
    [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool LookupPrivilegeValue(string s, string n, out long luid);
    [DllImport("advapi32.dll", SetLastError=true)] public static extern bool AdjustTokenPrivileges(IntPtr t, bool d, ref TOKEN_PRIVILEGES np, uint l, IntPtr p, IntPtr r);
    [DllImport("kernel32.dll")] public static extern IntPtr GetCurrentProcess();
    [StructLayout(LayoutKind.Sequential)] public struct TOKEN_PRIVILEGES { public uint Count; public long Luid; public uint Attr; }
}
'@

# Common DUT-side prelude block; loaded into Invoke-Command via -ArgumentList.
$dutPrelude = {
    param($fwEnvSrc)
    Add-Type -TypeDefinition $fwEnvSrc
    $tok = [IntPtr]::Zero
    [void][FwEnv]::OpenProcessToken([FwEnv]::GetCurrentProcess(), 0x28, [ref]$tok)
    $luid = 0L
    [void][FwEnv]::LookupPrivilegeValue($null, 'SeSystemEnvironmentPrivilege', [ref]$luid)
    $tp = New-Object FwEnv+TOKEN_PRIVILEGES; $tp.Count = 1; $tp.Luid = $luid; $tp.Attr = 2
    [void][FwEnv]::AdjustTokenPrivileges($tok, $false, [ref]$tp, 0, [IntPtr]::Zero, [IntPtr]::Zero)
}

# --- 4. ESP mount + deploy + boot-entry discovery --------------------------
Write-Host "[4/6] Preparing ESP on DUT: mount, find boot entries, deploy EFI apps ..."
$prep = Invoke-Command -Session $session -ArgumentList $ESP_GPT_TYPE, $RESULT_GUID, $fwEnvSrc -ScriptBlock {
    param($EspGptType, $ResGuid, $FwEnvSrc)
    Add-Type -TypeDefinition $FwEnvSrc
    $tok = [IntPtr]::Zero; [void][FwEnv]::OpenProcessToken([FwEnv]::GetCurrentProcess(), 0x28, [ref]$tok)
    $luid = 0L; [void][FwEnv]::LookupPrivilegeValue($null, 'SeSystemEnvironmentPrivilege', [ref]$luid)
    $tp = New-Object FwEnv+TOKEN_PRIVILEGES; $tp.Count = 1; $tp.Luid = $luid; $tp.Attr = 2
    [void][FwEnv]::AdjustTokenPrivileges($tok, $false, [ref]$tp, 0, [IntPtr]::Zero, [IntPtr]::Zero)

    # Find ESP partition, mount if no drive letter.
    $esp = Get-Partition | Where-Object { $_.GptType -eq $EspGptType } | Select-Object -First 1
    if (-not $esp) { throw "ESP partition not found on DUT" }
    if (-not $esp.DriveLetter) {
        $used = (Get-PSDrive -PSProvider FileSystem).Name
        $letter = (90..68 | ForEach-Object { [char]$_ } | Where-Object { $used -notcontains "$_" } | Select-Object -First 1)
        Add-PartitionAccessPath -DiskNumber $esp.DiskNumber -PartitionNumber $esp.PartitionNumber -AccessPath "${letter}:"
        Start-Sleep -Seconds 2
        $esp = Get-Partition -DiskNumber $esp.DiskNumber -PartitionNumber $esp.PartitionNumber
    }
    $espLetter = "$($esp.DriveLetter)"

    # Delete stale WIM so multi-volume search falls through to USB.
    $stale = "${espLetter}:\ValidationOS.wim"
    if ([System.IO.File]::Exists($stale)) {
        [System.IO.File]::Delete($stale)
    }
    if (-not (Test-Path "${espLetter}:\EFI\Boot")) {
        New-Item -ItemType Directory -Force -Path "${espLetter}:\EFI\Boot" | Out-Null
    }

    # Find existing firmware boot entries by description.
    $stanzas = ((bcdedit /enum firmware) -join "`n") -split "(?m)^\s*$"
    function FindEntry($needle) {
        $hit = $stanzas | Where-Object { $_ -match ('description\s+' + [regex]::Escape($needle) + '\b') } | Select-Object -First 1
        if (-not $hit) { return $null }
        if ($hit -match 'identifier\s+(\{[0-9a-fA-F-]+\})') { return $matches[1] } else { return $null }
    }
    $writeId  = FindEntry 'NvmeBpWrite'
    $loaderId = FindEntry 'BpRecoveryLoader'

    # Clear NvmeBpResult so the write tool runs in WRITE mode.
    [void][FwEnv]::SetFirmwareEnvironmentVariableExW('NvmeBpResult', $ResGuid, [IntPtr]::Zero, 0, [uint32]7)

    [PSCustomObject]@{
        EspLetter = $espLetter
        WriteId   = $writeId
        LoaderId  = $loaderId
    }
}

$espLetter = $prep.EspLetter
if (-not $prep.WriteId)  { throw "Firmware boot entry 'NvmeBpWrite' not found on DUT (one-time setup needed: bcdedit /copy to clone an existing Firmware Application entry, set device=partition=${espLetter}: and path=\EFI\Boot\NvmeBpWrite.efi)." }
if (-not $prep.LoaderId) { throw "Firmware boot entry 'BpRecoveryLoader' not found on DUT (same setup as above)." }

Copy-Item -Path $NvmeBpWriteEfi  -Destination "${espLetter}:\EFI\Boot\NvmeBpWrite.efi"  -ToSession $session -Force
Copy-Item -Path $BpRecoveryLoaderEfi -Destination "${espLetter}:\EFI\Boot\BpRecoveryLoader.efi" -ToSession $session -Force
Write-Host "    NvmeBpWrite boot id  : $($prep.WriteId)"
Write-Host "    BpRecoveryLoader boot id : $($prep.LoaderId)"
Write-Host "    ESP letter on DUT        : ${espLetter}:"

# --- 5. NvmeBpWrite cycle ---------------------------------------------
function Wait-DutBack {
    param([int] $TimeoutMinutes)
    $deadline = (Get-Date).AddMinutes($TimeoutMinutes)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 5
        try {
            Invoke-Command -ComputerName $DutHost -Credential $DutCredential -ScriptBlock { 1 } -ErrorAction Stop | Out-Null
            return $true
        } catch { }
    }
    return $false
}

Write-Host "[5/6] Writing BP1 from USB-staged WIM (boot into NvmeBpWrite) ..."
Invoke-Command -Session $session -ArgumentList $prep.WriteId -ScriptBlock {
    param($id)
    bcdedit /set "{fwbootmgr}" bootsequence $id | Out-Null
    shutdown /r /t 3 /f
}
Remove-PSSession $session
Start-Sleep -Seconds 30
if (-not (Wait-DutBack -TimeoutMinutes 6)) { throw "DUT did not return from NvmeBpWrite within 6 min" }

# Read NvmeBpResult to verify the BP write succeeded.
$writeResult = Invoke-Command -ComputerName $DutHost -Credential $DutCredential -ArgumentList $RESULT_GUID, $fwEnvSrc -ScriptBlock {
    param($g, $src)
    Add-Type -TypeDefinition $src
    $tok = [IntPtr]::Zero; [void][FwEnv]::OpenProcessToken([FwEnv]::GetCurrentProcess(), 0x28, [ref]$tok)
    $luid = 0L; [void][FwEnv]::LookupPrivilegeValue($null, 'SeSystemEnvironmentPrivilege', [ref]$luid)
    $tp = New-Object FwEnv+TOKEN_PRIVILEGES; $tp.Count = 1; $tp.Luid = $luid; $tp.Attr = 2
    [void][FwEnv]::AdjustTokenPrivileges($tok, $false, [ref]$tp, 0, [IntPtr]::Zero, [IntPtr]::Zero)
    $buf = New-Object byte[] 512; $a = [uint32]0
    $n = [FwEnv]::GetFirmwareEnvironmentVariableExW('NvmeBpResult', $g, $buf, [uint32]$buf.Length, [ref]$a)
    if ($n -eq 0) { return @{ Found = $false } }
    @{
        Found         = $true
        Phase         = [BitConverter]::ToUInt32($buf, 8)
        Mode          = [BitConverter]::ToUInt32($buf, 12)
        EndToEnd      = [BitConverter]::ToUInt32($buf, 16)
        WimFileBytes  = [BitConverter]::ToUInt64($buf, 20)
        BytesUploaded = [BitConverter]::ToUInt64($buf, 36)
    }
}
if (-not $writeResult.Found) { throw "NvmeBpResult NVRAM variable not found after BP write" }
$writeMiB = [Math]::Round($writeResult.BytesUploaded/1MB, 1)
Write-Host "    BP write: Phase=$($writeResult.Phase) E2E=$($writeResult.EndToEnd) BytesUploaded=$writeMiB MiB"
if ($writeResult.EndToEnd -ne 1) { throw "NvmeBpWrite reported failure (E2E=0). BP write did not complete." }
if ([Math]::Abs($writeResult.BytesUploaded - $imageBytes) -gt 1MB) {
    Write-Warning "BytesUploaded ($writeMiB MiB) != staged image ($([Math]::Round($imageBytes/1MB,1)) MiB). USB may not have been picked up; check 'BytesUploaded' was sourced from USB not stale ESP file."
}

# --- 6. BpRecoveryLoader chainload + observe ------------------------------
Write-Host "[6/6] Chainloading WinVOS from BP -> RAM disk (boot into BpRecoveryLoader) ..."
Invoke-Command -ComputerName $DutHost -Credential $DutCredential -ArgumentList $prep.LoaderId -ScriptBlock {
    param($id)
    bcdedit /set "{fwbootmgr}" bootsequence $id | Out-Null
    shutdown /r /t 3 /f
}
Start-Sleep -Seconds 30
$returned = Wait-DutBack -TimeoutMinutes 5
if (-not $returned) {
    Write-Host ""
    Write-Host "SUCCESS - DUT did not return to Windows WinRM within 5 min."
    Write-Host "WinVOS likely booted (cmd.exe on screen). Check the DUT display to confirm."
    Write-Host "When done, reboot the DUT to return to your normal Windows."
    return
}

Write-Host ""
Write-Host "FAIL - DUT returned to Windows. Chainload did not stick. Reading BpLoaderResult ..."
Invoke-Command -ComputerName $DutHost -Credential $DutCredential -ArgumentList $RESULT_GUID, $fwEnvSrc -ScriptBlock {
    param($g, $src)
    Add-Type -TypeDefinition $src
    $tok = [IntPtr]::Zero; [void][FwEnv]::OpenProcessToken([FwEnv]::GetCurrentProcess(), 0x28, [ref]$tok)
    $luid = 0L; [void][FwEnv]::LookupPrivilegeValue($null, 'SeSystemEnvironmentPrivilege', [ref]$luid)
    $tp = New-Object FwEnv+TOKEN_PRIVILEGES; $tp.Count = 1; $tp.Luid = $luid; $tp.Attr = 2
    [void][FwEnv]::AdjustTokenPrivileges($tok, $false, [ref]$tp, 0, [IntPtr]::Zero, [IntPtr]::Zero)
    $buf = New-Object byte[] 256; $a = [uint32]0
    $n = [FwEnv]::GetFirmwareEnvironmentVariableExW('BpLoaderResult', $g, $buf, [uint32]$buf.Length, [ref]$a)
    if ($n -eq 0) { Write-Host "  BpLoaderResult not found"; return }
    $phaseMap = @{0='INIT';1='PASSTHRU_LOCATED';2='BP_LOADED';3='BP_LOCKED';4='RAMDISK_REGISTERED';5='DONE'}
    $phase = [BitConverter]::ToUInt32($buf, 8)
    Write-Host "  Phase=$phase ($($phaseMap[[int]$phase])) RamDisk=$([BitConverter]::ToUInt32($buf,12)) Lock=$([BitConverter]::ToUInt32($buf,16))"
    Write-Host "  ChainloadAttempted=$([BitConverter]::ToUInt32($buf,20)) ChainloadImageLoaded=$([BitConverter]::ToUInt32($buf,24))"
    Write-Host "  BytesRead=$([BitConverter]::ToUInt64($buf,32)) LastEfiStatus=0x$('{0:X8}' -f [BitConverter]::ToInt32($buf,48))"
}
exit 1
