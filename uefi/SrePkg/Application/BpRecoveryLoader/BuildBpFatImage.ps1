<#
.SYNOPSIS
  Build a raw FAT32 disk image suitable for writing into an NVMe Boot
  Partition, for chainloading by BpRecoveryLoader.

.DESCRIPTION
  Produces a fixed-size raw disk image with a single GPT-wrapped FAT32
  partition. Two modes:

    -BootEfi <file>   Simple: install the given .efi as \EFI\Boot\bootx64.efi.
                      Useful for chainload smoke tests (Shell.efi etc.).

    -WimFile <file>   WinPE/WinVOS: stage bootmgfw.efi + BCD + boot.sdi
                      + the WIM so the firmware boots Windows Boot Manager
                      which then loads the WIM as a ramdisk per BCD config.

  -BootEfi and -WimFile are mutually exclusive. -PayloadDir is supported
  in either mode for staging extra files.

  Requires elevation (mount + format + partition operations need admin).
  Uses Hyper-V cmdlets (New-VHD, Mount-VHD); Hyper-V Management Tools
  feature must be enabled.

.PARAMETER BootEfi
  Path to the .efi file to install as \EFI\Boot\bootx64.efi inside the FAT
  volume. For first-iteration testing, the platform Shell.efi works as a
  visual confirmation that chainload happened (the target reaches the shell
  prompt instead of returning to Windows).

.PARAMETER WimFile
  Path to a Windows boot.wim (e.g. ValidationOS.wim from CI). When set,
  the script assembles a WinPE-style bootable layout:
      \EFI\Boot\bootx64.efi    <- bootmgfw.efi (UEFI fallback path)
      \EFI\Microsoft\Boot\bootmgfw.efi <- bootmgfw.efi
      \boot\boot.sdi           <- System Deployment Image
      \boot\bcd                <- generated BCD store with ramdisk options
      \sources\boot.wim        <- the supplied WIM

.PARAMETER BootmgfwEfi
  Override for bootmgfw.efi source. Defaults to C:\Windows\Boot\EFI\bootmgfw.efi.

.PARAMETER BootSdi
  Override for boot.sdi source. Defaults to C:\Windows\Boot\DVD\EFI\boot.sdi.

.PARAMETER OutImage
  Path for the produced raw .img file.

.PARAMETER SizeBytes
  Final image size in bytes. Defaults to 1 GiB. Must match the target
  controller's boot-partition size and the BP write tool's expected size.

.PARAMETER PayloadDir
  Optional directory whose contents are recursively copied into the FAT
  volume root. Use it to add any payload needed to make the resulting FAT
  image bootable, such as fonts, locale data, drivers, or loader resources.

.EXAMPLE
  # Smoke test: chainload Shell.efi
  .\BuildBpFatImage.ps1 -BootEfi C:\path\to\Shell.efi -OutImage bp1.img

.EXAMPLE
  # WinVOS recovery image
  .\BuildBpFatImage.ps1 -WimFile C:\path\to\ValidationOS.wim -OutImage bp1.img
#>
[CmdletBinding(DefaultParameterSetName = 'BootEfi')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'BootEfi')]  [string] $BootEfi,
    [Parameter(Mandatory = $true, ParameterSetName = 'WimFile')]  [string] $WimFile,
    [Parameter(ParameterSetName = 'WimFile')] [string] $BootmgfwEfi = 'C:\Windows\Boot\EFI\bootmgfw.efi',
    [Parameter(ParameterSetName = 'WimFile')] [string] $BootSdi     = 'C:\Windows\Boot\DVD\EFI\boot.sdi',
    [Parameter(Mandatory = $true)] [string] $OutImage,
    [long]   $SizeBytes  = 1GB,
    [string] $PayloadDir
)

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "This script must be run from an elevated PowerShell session (mount/format need admin)."
}
if ($PSCmdlet.ParameterSetName -eq 'BootEfi') {
    if (-not (Test-Path $BootEfi)) { throw "BootEfi not found: $BootEfi" }
} else {
    if (-not (Test-Path $WimFile))      { throw "WimFile not found: $WimFile" }
    if (-not (Test-Path $BootmgfwEfi))  { throw "BootmgfwEfi not found: $BootmgfwEfi" }
    if (-not (Test-Path $BootSdi))      { throw "BootSdi not found: $BootSdi" }
}
if ($PayloadDir -and -not (Test-Path $PayloadDir)) { throw "PayloadDir not found: $PayloadDir" }
if ($SizeBytes -lt 64MB)          { throw "SizeBytes must be >= 64 MiB for FAT32." }

# Normalize to an absolute path
$OutImage = [IO.Path]::GetFullPath((Join-Path (Get-Location).Path $OutImage))
$workDir  = Split-Path -Parent $OutImage
$vhd      = Join-Path $workDir ([IO.Path]::ChangeExtension([IO.Path]::GetFileName($OutImage), '.vhd'))

if (Test-Path $vhd)       { Remove-Item $vhd -Force }
if (Test-Path $OutImage)  { Remove-Item $OutImage -Force }

Write-Host "Creating fixed VHD: $vhd ($($SizeBytes / 1MB) MiB)..."
New-VHD -Path $vhd -Fixed -SizeBytes $SizeBytes | Out-Null

$letter = $null
try {
    Write-Host "Mounting + partitioning + formatting FAT32..."
    $disk = Mount-VHD -Path $vhd -Passthru | Get-Disk
    Initialize-Disk -Number $disk.Number -PartitionStyle GPT | Out-Null
    $part = New-Partition -DiskNumber $disk.Number -UseMaximumSize
    Format-Volume -Partition $part -FileSystem FAT32 -NewFileSystemLabel 'SRE_BP' -Confirm:$false -Force | Out-Null

    # --- SRE descriptor-region guard --------------------------------------------
    # The SRE FMP driver stamps its SRE_IMAGE_INFO descriptor at a fixed byte
    # offset (0x4400 == GPT LBA 34) inside the reserved MSR gap that precedes the
    # FAT data partition (see SRE_IMAGE_INFO_OFFSET in SreFmpDeviceLib.h). That is
    # only safe while (a) the logical sector size is 512 so LBA 34 == 0x4400, and
    # (b) the FAT data partition starts above the descriptor window. If a future
    # layout change (e.g. a missing MSR partition) moved the FAT partition down to
    # 0x4400, the descriptor would corrupt the FAT boot sector, so fail here.
    $SreDescriptorOffset = 0x4400
    $SreDescriptorBytes  = 512   # conservative reserved window for the descriptor
    if ($disk.LogicalSectorSize -ne 512) {
        throw ("Logical sector size is $($disk.LogicalSectorSize); SRE descriptor offset 0x4400 assumes 512-byte sectors (LBA 34).")
    }
    if ($part.Offset -le ($SreDescriptorOffset + $SreDescriptorBytes)) {
        throw ("FAT partition starts at 0x{0:X}, which overlaps the reserved SRE descriptor window [0x{1:X}..0x{2:X}); the MSR gap is missing or too small." -f `
            [long]$part.Offset, $SreDescriptorOffset, ($SreDescriptorOffset + $SreDescriptorBytes))
    }
    Write-Host ("  SRE descriptor window OK: FAT partition starts at 0x{0:X} (reserved 0x4400 window is clear)" -f [long]$part.Offset)

    # Pick a free drive letter and assign it.
    $used = (Get-PSDrive -PSProvider FileSystem).Name
    $letter = (90..68 | ForEach-Object { [char]$_ } | Where-Object { $used -notcontains "$_" } | Select-Object -First 1)
    if (-not $letter) { throw "No free drive letter available." }
    $part | Set-Partition -NewDriveLetter $letter
    Start-Sleep -Seconds 2  # let the letter mount settle

    $root = "${letter}:"

    if ($PSCmdlet.ParameterSetName -eq 'BootEfi') {
        $bootDir = Join-Path $root 'EFI\Boot'
        New-Item -ItemType Directory -Force -Path $bootDir | Out-Null
        Copy-Item -Path $BootEfi -Destination (Join-Path $bootDir 'bootx64.efi') -Force
        Write-Host "  Installed $(Resolve-Path $BootEfi) -> ${letter}:\EFI\Boot\bootx64.efi"
    }
    else {
        # WinPE/WinVOS layout: stage bootmgr + BCD + SDI + WIM
        $v = Get-Volume -DriveLetter $letter
        Write-Host "  FAT volume: $([Math]::Round($v.Size/1MB,1)) MiB total, $([Math]::Round($v.SizeRemaining/1MB,1)) MiB free before staging"
        $contentBytes = (Get-Item $BootmgfwEfi).Length + (Get-Item $BootSdi).Length + (Get-Item $WimFile).Length
        Write-Host "  Content to stage: $([Math]::Round($contentBytes/1MB,1)) MiB"
        if ($contentBytes -gt $v.SizeRemaining) {
            throw "Not enough room in FAT volume: need $([Math]::Round($contentBytes/1MB,1)) MiB, have $([Math]::Round($v.SizeRemaining/1MB,1)) MiB. Bump -SizeBytes."
        }

        New-Item -ItemType Directory -Force -Path (Join-Path $root 'EFI\Boot') | Out-Null
        New-Item -ItemType Directory -Force -Path (Join-Path $root 'EFI\Microsoft\Boot') | Out-Null
        New-Item -ItemType Directory -Force -Path (Join-Path $root 'boot') | Out-Null
        New-Item -ItemType Directory -Force -Path (Join-Path $root 'sources') | Out-Null

        Copy-Item -Path $BootmgfwEfi -Destination (Join-Path $root 'EFI\Boot\bootx64.efi') -Force
        Copy-Item -Path $BootSdi     -Destination (Join-Path $root 'boot\boot.sdi') -Force
        Copy-Item -Path $WimFile     -Destination (Join-Path $root 'sources\boot.wim') -Force
        Write-Host "  Staged bootmgfw.efi -> \EFI\Boot\bootx64.efi, boot.sdi, and $(Split-Path -Leaf $WimFile) -> boot.wim"

        # Build BCD store at the UEFI canonical location (bootmgfw.efi looks
        # for it there by default).
        $bcd = Join-Path $root 'EFI\Microsoft\Boot\BCD'
        Write-Host "  Generating BCD at $bcd ..."

        function bcd($argList) {
            $out = & bcdedit @argList 2>&1
            if ($LASTEXITCODE -ne 0) { throw "bcdedit failed: bcdedit $($argList -join ' ')`n$out" }
            return ,$out
        }
        function Get-Guid([string[]]$out) {
            foreach ($line in $out) {
                if ($line -match '(\{[0-9a-fA-F-]+\})') { return $matches[1] }
            }
            throw "Could not extract GUID from bcdedit output: $($out -join "`n")"
        }

        bcd @('/createstore', $bcd) | Out-Null
        bcd @('/store', $bcd, '/create', '{bootmgr}',          '/d', 'Windows Boot Manager') | Out-Null
        bcd @('/store', $bcd, '/set',    '{bootmgr}', 'device', 'boot') | Out-Null
        bcd @('/store', $bcd, '/set',    '{bootmgr}', 'timeout', '0') | Out-Null

        bcd @('/store', $bcd, '/create', '{ramdiskoptions}',   '/d', 'Ramdisk options') | Out-Null
        bcd @('/store', $bcd, '/set',    '{ramdiskoptions}', 'ramdisksdidevice', 'boot') | Out-Null
        bcd @('/store', $bcd, '/set',    '{ramdiskoptions}', 'ramdisksdipath',   '\boot\boot.sdi') | Out-Null

        $createOut = bcd @('/store', $bcd, '/create', '/d', 'Windows Validation OS', '/application', 'osloader')
        $osLoader  = Get-Guid $createOut
        Write-Host "    osloader entry: $osLoader"

        $ramSpec = 'ramdisk=[boot]\sources\boot.wim,{ramdiskoptions}'
        bcd @('/store', $bcd, '/set', $osLoader, 'device',     $ramSpec) | Out-Null
        bcd @('/store', $bcd, '/set', $osLoader, 'osdevice',   $ramSpec) | Out-Null
        bcd @('/store', $bcd, '/set', $osLoader, 'path',       '\windows\system32\boot\winload.efi') | Out-Null
        bcd @('/store', $bcd, '/set', $osLoader, 'systemroot', '\windows') | Out-Null
        bcd @('/store', $bcd, '/set', $osLoader, 'detecthal',  'yes') | Out-Null
        bcd @('/store', $bcd, '/set', $osLoader, 'winpe',      'yes') | Out-Null
        bcd @('/store', $bcd, '/set', $osLoader, 'ems',        'no') | Out-Null

        bcd @('/store', $bcd, '/displayorder', $osLoader) | Out-Null
        bcd @('/store', $bcd, '/default',      $osLoader) | Out-Null
    }

    if ($PayloadDir) {
        Write-Host "  Copying payload from $PayloadDir..."
        Copy-Item -Path (Join-Path $PayloadDir '*') -Destination $root -Recurse -Force
    }

    Write-Host "  Volume contents:"
    Get-ChildItem $root -Recurse | Select-Object FullName, Length | Format-Table -AutoSize
}
finally {
    if ($letter) {
        try { Dismount-VHD -Path $vhd -ErrorAction Stop } catch { Write-Warning "Dismount-VHD: $_" }
    }
}

# Fixed VHD = raw disk bytes followed by a 512-byte footer. Truncate
# to exactly $SizeBytes to get a raw image consumable by NvmeBpWrite.
Write-Host "Stripping VHD footer -> raw image: $OutImage"
$inStream  = [IO.File]::OpenRead($vhd)
$outStream = [IO.File]::Create($OutImage)
try {
    $buf = New-Object byte[] 4MB
    $remaining = $SizeBytes
    while ($remaining -gt 0) {
        $toRead = [Math]::Min([long]$buf.Length, $remaining)
        $n = $inStream.Read($buf, 0, [int]$toRead)
        if ($n -eq 0) { break }
        $outStream.Write($buf, 0, $n)
        $remaining -= $n
    }
}
finally {
    $inStream.Close()
    $outStream.Close()
}
Remove-Item $vhd -Force

$actualSize = (Get-Item $OutImage).Length
Write-Host ""
Write-Host "Done. Image: $OutImage ($actualSize bytes)"
if ($actualSize -ne $SizeBytes) {
    Write-Warning "Final size $actualSize != requested $SizeBytes - image is malformed; check earlier errors."
}
