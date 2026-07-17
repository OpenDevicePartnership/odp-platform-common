# enable-remote.ps1 — bootstrap WinRM + token-filter policy on a fresh
# Windows install. Run elevated on the target. Idempotent.
#
# What this enables:
#   1. PSRemoting + WinRM listener + firewall (so a workstation can
#      Invoke-Command into this machine).
#   2. LocalAccountTokenFilterPolicy=1 so non-builtin local admin
#      accounts (like `testuser`) keep their admin token under UAC. Without
#      this, SetFirmwareEnvironmentVariableExW fails with LastError=1314.
#
# Reboot/relog is REQUIRED before LocalAccountTokenFilterPolicy takes effect
# on the current session's token. The script will tell you, or you can pass
# -Restart to reboot automatically.
#
# Usage:
#   <USB>:\enable-remote.ps1            # apply policy + remoting, prompt to reboot
#   <USB>:\enable-remote.ps1 -Restart   # apply + auto-reboot in 5s

[CmdletBinding()]
param(
    [switch]$Restart
)

$ErrorActionPreference = 'Continue'

function Section($name) { Write-Host ""; Write-Host "=== $name ===" -ForegroundColor Cyan }
function OK($msg)       { Write-Host "  [OK]  $msg" -ForegroundColor Green }
function WARN($msg)     { Write-Host "  [WARN] $msg" -ForegroundColor Yellow }
function FAIL($msg)     { Write-Host "  [FAIL] $msg" -ForegroundColor Red }

# --- Preflight: elevation + account identity ---
Section 'Preflight'
$id    = [Security.Principal.WindowsIdentity]::GetCurrent()
$elev  = ([Security.Principal.WindowsPrincipal]::new($id)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$isBuiltinAdmin = $id.User.Value.EndsWith('-500')
"  User:               $($id.Name)"
"  SID:                $($id.User.Value)"
"  Built-in Admin:     $isBuiltinAdmin"
"  Elevated:           $elev"
if (-not $elev) {
    FAIL "Not elevated. Re-launch from an elevated PowerShell (Win+X → Terminal (Admin))."
    exit 1
}
OK 'Elevated.'

# --- 1. Network profile: Private (lets Enable-PSRemoting create firewall rule) ---
Section 'Network profile → Private'
$nics = Get-NetConnectionProfile -ErrorAction SilentlyContinue
foreach ($n in $nics) {
    try {
        if ($n.NetworkCategory -ne 'Private') {
            Set-NetConnectionProfile -InterfaceIndex $n.InterfaceIndex -NetworkCategory Private -ErrorAction Stop
            OK "$($n.Name): $($n.NetworkCategory) → Private"
        } else {
            OK "$($n.Name): already Private"
        }
    } catch {
        WARN "$($n.Name): $($_.Exception.Message)"
    }
}

# --- 2. PSRemoting ---
Section 'Enable-PSRemoting'
try {
    Enable-PSRemoting -Force -ErrorAction Stop | Out-Null
    OK 'PSRemoting enabled (or already on).'
} catch {
    FAIL "Enable-PSRemoting failed: $($_.Exception.Message)"
}

# --- 3. Widen WinRM firewall to Any source ---
Section 'WinRM firewall → RemoteAddress Any'
$rules = Get-NetFirewallRule -Name 'WINRM*' -ErrorAction SilentlyContinue
if (-not $rules) {
    WARN 'No WINRM* firewall rules found.'
} else {
    foreach ($r in $rules) {
        try {
            Set-NetFirewallRule -Name $r.Name -RemoteAddress Any -ErrorAction Stop
            OK "$($r.Name) [$($r.Profile)]: RemoteAddress = Any"
        } catch {
            WARN "$($r.Name): $($_.Exception.Message)"
        }
    }
}

# --- 4. LocalAccountTokenFilterPolicy — verify after set ---
Section 'LocalAccountTokenFilterPolicy = 1'
$regPath  = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System'
$regName  = 'LocalAccountTokenFilterPolicy'
$before   = (Get-ItemProperty $regPath -Name $regName -ErrorAction SilentlyContinue).$regName
"  Before: $(if ($null -eq $before) {'(not set)'} else {$before})"

if ($isBuiltinAdmin) {
    OK 'Built-in Administrator (SID ends -500); policy not strictly required for this user. Setting anyway for future logons.'
}

try {
    New-ItemProperty -Path $regPath -Name $regName -PropertyType DWord -Value 1 -Force -ErrorAction Stop | Out-Null
} catch {
    FAIL "Setting policy failed: $($_.Exception.Message)"
}
$after = (Get-ItemProperty $regPath -Name $regName -ErrorAction SilentlyContinue).$regName
"  After:  $after"
if ($after -eq 1) {
    OK 'Policy set in registry.'
} else {
    FAIL "Policy did not take effect in registry (got '$after')."
}

# --- 5. WinRM service auto-start ---
Section 'WinRM service'
try {
    Set-Service WinRM -StartupType Automatic -ErrorAction Stop
    if ((Get-Service WinRM).Status -ne 'Running') { Start-Service WinRM }
    OK "Service: $((Get-Service WinRM).Status) (StartupType: Automatic)"
} catch {
    FAIL "WinRM service setup failed: $($_.Exception.Message)"
}

# --- 6. Self-test: does my current token have SeSystemEnvironmentPrivilege? ---
Section 'Self-test: token has SeSystemEnvironmentPrivilege?'
$priv = (whoami /priv) -join "`n"
$hasPriv = $priv -match 'SeSystemEnvironmentPrivilege'
if ($hasPriv) {
    OK 'Token has SeSystemEnvironmentPrivilege (firmware-variable operations should work right now).'
    "  No reboot strictly needed for THIS session, but persistent policy is in place for future logons."
} else {
    WARN 'Token does NOT have SeSystemEnvironmentPrivilege right now.'
    "  This is expected for non-builtin admin accounts when the token-filter policy was only"
    "  just set this session — UAC issued your token before the registry change. The new"
    "  policy applies on the next interactive logon (sign out + back in, OR reboot)."
}

# --- 7. Next steps / reboot ---
Section 'Next steps'
if ($hasPriv) {
    OK 'You can run Reset-NvmeBpResult.ps1 right now without rebooting.'
} elseif ($Restart) {
    WARN 'Restarting in 5 seconds to pick up the new token-filter policy. Ctrl+C to cancel.'
    Start-Sleep -Seconds 5
    shutdown /g /t 0
} else {
    WARN 'Sign out + back in OR reboot before running Reset-NvmeBpResult.ps1.'
    "  Quick reboot:   shutdown /g /t 0"
    "  Or re-run this script with -Restart to reboot automatically."
}
