# Set-NextBootToUsb.ps1 — set the firmware boot sequence to a USB entry on the
# next reboot, then reboot. Eliminates the Vol-Down hotkey dance.
#
# Run elevated on the target (locally or via WinRM). Idempotent.
#
# Defaults to matching firmware boot entries whose description is exactly
# "USB Storage" — the reference firmware's generic description for the
# USB-first alt-boot entry (FAT32 volume labels and vendor strings are not
# visible to the firmware boot manager). Override with -MatchPattern on
# targets that use a different description. Use -DryRun to list matches
# without rebooting.
#
# Examples:
#   ./Set-NextBootToUsb.ps1                          # auto-match SREFLASH or UEFI USB, reboot
#   ./Set-NextBootToUsb.ps1 -MatchPattern 'Kingston' # match by description fragment
#   ./Set-NextBootToUsb.ps1 -DryRun                  # show what would be selected, no reboot
#   ./Set-NextBootToUsb.ps1 -DelaySeconds 0          # reboot immediately (default 5)

[CmdletBinding()]
param(
    [string]$MatchPattern = '^USB Storage$',
    [int]$DelaySeconds = 5,
    [switch]$DryRun
)

# Enumerate firmware boot entries via bcdedit. We parse the text output;
# there's no PowerShell cmdlet that exposes firmware bootmgr entries.
$raw = bcdedit /enum firmware /v 2>&1 | Out-String

# Split into per-entry sections. Each section starts with a header line like
# "Firmware Application (101fffff)" followed by a dashes line, then key/value
# pairs.
$entries  = New-Object System.Collections.Generic.List[object]
$current  = $null
foreach ($line in ($raw -split "`r?`n")) {
    if ($line -match '^(Firmware Application|Windows Boot Manager|Windows Boot Loader)') {
        if ($current) { $entries.Add($current) | Out-Null }
        $current = [PSCustomObject]@{
            Header      = $line.Trim()
            Identifier  = ''
            Description = ''
            Device      = ''
            Path        = ''
        }
    } elseif ($line -match '^-+\s*$' -or $line -match '^Firmware Boot Manager' -or $line -match '^Windows Boot Manager') {
        # skip
    } elseif ($current) {
        if     ($line -match '^identifier\s+(.+)$')  { $current.Identifier  = $Matches[1].Trim() }
        elseif ($line -match '^description\s+(.+)$') { $current.Description = $Matches[1].Trim() }
        elseif ($line -match '^device\s+(.+)$')      { $current.Device      = $Matches[1].Trim() }
        elseif ($line -match '^path\s+(.+)$')        { $current.Path        = $Matches[1].Trim() }
    }
}
if ($current) { $entries.Add($current) | Out-Null }

# Filter to entries whose description or device matches the pattern.
# (Using $matchedEntries to avoid stomping the $Matches auto-variable used by -match.)
$matchedEntries = @($entries | Where-Object {
    ($_.Description -match $MatchPattern) -or ($_.Device -match $MatchPattern)
})

if ($matchedEntries.Count -eq 0) {
    Write-Host "No firmware boot entry matched '$MatchPattern'. All entries:" -ForegroundColor Yellow
    $entries | Format-Table Identifier, Description, Device, Path -AutoSize | Out-String -Width 200 | Write-Host
    exit 1
}

if ($matchedEntries.Count -gt 1) {
    Write-Host "Multiple matches for '$MatchPattern' — refine with -MatchPattern:" -ForegroundColor Yellow
    $matchedEntries | Format-Table Identifier, Description, Device, Path -AutoSize | Out-String -Width 200 | Write-Host
    exit 1
}

$pick = $matchedEntries[0]
'Selected firmware boot entry:'
"  description: $($pick.Description)"
"  device:      $($pick.Device)"
"  identifier:  $($pick.Identifier)"

if ($DryRun) {
    '(-DryRun) skipping bootsequence set + reboot.'
    exit 0
}

# Set as one-shot boot sequence on the firmware boot manager
& bcdedit /set '{fwbootmgr}' bootsequence $pick.Identifier 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Error 'bcdedit /set bootsequence failed.'
    exit 1
}

"Bootsequence set. Rebooting in $DelaySeconds second(s) — Ctrl+C to cancel."
if ($DelaySeconds -gt 0) { Start-Sleep -Seconds $DelaySeconds }

# Use `shutdown /g` (graceful shutdown + restart) instead of `/r` (warm reboot).
# `/r` leaves peripherals in their pre-reboot power state, which can trip an
# `ASSERT(FALSE)` in a DEBUG-build I2C driver when a HID peripheral has not
# re-initialized cleanly. `/g` bypasses Fast Startup and does a full
# cold-boot-equivalent power cycle of attached peripherals.
shutdown /g /t 0
