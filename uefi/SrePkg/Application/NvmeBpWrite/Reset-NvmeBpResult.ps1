# Reset-NvmeBpResult.ps1 — clear the NvmeBpResult UEFI runtime variable on the
# target so the next NvmeBpWrite.efi boot runs WRITE instead of NOOP.
#
# Run elevated. Idempotent: succeeds whether the variable was set or not.
#
# Variable: NvmeBpResult, vendor GUID {7B5A1F3E-2D8C-4A91-B6E3-D8F2C9A4E105},
# attributes NV|BS|RT (0x07).
#
# Why this script has a "fallback" path:
#   Calling SetFirmwareEnvironmentVariableExW needs SeSystemEnvironmentPrivilege
#   *enabled* in the token. In an interactive elevated PowerShell session,
#   UAC issues a token where the privilege is listed-but-Disabled and
#   AdjustTokenPrivileges silently can't enable it (returns
#   ERROR_NOT_ALL_ASSIGNED=1300). The same P/Invoke succeeds in a WinRM-session
#   token because network logons get an unfiltered token with the privilege
#   already Enabled.
#
#   So if the direct call fails with the UAC-filter signature, this script
#   self-relaunches via `Invoke-Command -ComputerName localhost` to acquire a
#   WinRM-style token and retries. enable-remote.ps1 must have been run once
#   per Windows image (it enables PSRemoting + sets
#   LocalAccountTokenFilterPolicy=1) before the fallback path works.
#
# Usage:
#   <USB>:\Reset-NvmeBpResult.ps1                          # elevated PS on target
#   <USB>:\Reset-NvmeBpResult.ps1 -Credential (Get-Credential)
#   <USB>:\Reset-NvmeBpResult.ps1 -NoFallback              # disable WinRM fallback

[CmdletBinding()]
param(
    [PSCredential]$Credential,
    [string]$ComputerName = $env:COMPUTERNAME,
    [switch]$NoFallback,
    [switch]$Quiet
)

function Note($msg) { if (-not $Quiet) { Write-Host $msg } }

# --- Shared P/Invoke definitions (used by both direct and remote paths) ---
$pinvokeDef = @'
using System;
using System.Runtime.InteropServices;
public static class Uefi {
    public const uint TOKEN_QUERY             = 0x0008;
    public const uint TOKEN_ADJUST_PRIVILEGES = 0x0020;
    public const uint SE_PRIVILEGE_ENABLED    = 0x00000002;
    public const int  ERROR_NOT_ALL_ASSIGNED  = 1300;
    public const int  ERROR_PRIVILEGE_NOT_HELD = 1314;
    public const int  ERROR_ENVVAR_NOT_FOUND  = 203;

    [DllImport("kernel32.dll")] public static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool CloseHandle(IntPtr h);
    [DllImport("advapi32.dll", SetLastError=true)]
    public static extern bool OpenProcessToken(IntPtr h, uint access, out IntPtr tok);
    [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    public static extern bool LookupPrivilegeValueW(string sys, string name, out long luid);

    [StructLayout(LayoutKind.Sequential, Pack=4)]
    public struct LUID_AND_ATTRIBUTES { public long Luid; public uint Attributes; }
    [StructLayout(LayoutKind.Sequential, Pack=4)]
    public struct TOKEN_PRIVILEGES { public uint Count; public LUID_AND_ATTRIBUTES Priv; }

    [DllImport("advapi32.dll", SetLastError=true)]
    public static extern bool AdjustTokenPrivileges(IntPtr tok, bool disall,
        ref TOKEN_PRIVILEGES nstate, uint blen, IntPtr pstate, IntPtr plen);
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    public static extern bool SetFirmwareEnvironmentVariableExW(
        string name, string guid, IntPtr buf, uint size, uint attrs);
}
'@

# Core clear logic — runs in whatever context the caller is in.
# Returns a hashtable: { Ok=bool; LastError=int; AdjustErr=int; Context=string }
$clearScriptBlock = {
    param([string]$PInvokeDef, [string]$Context)

    Add-Type -TypeDefinition $PInvokeDef -ErrorAction SilentlyContinue

    $tok = [IntPtr]::Zero
    $r1  = [Uefi]::OpenProcessToken([Uefi]::GetCurrentProcess(),
                                    ([Uefi]::TOKEN_ADJUST_PRIVILEGES -bor [Uefi]::TOKEN_QUERY),
                                    [ref]$tok)
    if (-not $r1) {
        return @{
            Ok = $false; Stage='OpenProcessToken'; LastError=[Runtime.InteropServices.Marshal]::GetLastWin32Error();
            AdjustErr=$null; Context=$Context
        }
    }

    $luid = 0L
    $r2 = [Uefi]::LookupPrivilegeValueW($null, 'SeSystemEnvironmentPrivilege', [ref]$luid)
    if (-not $r2) {
        return @{
            Ok=$false; Stage='LookupPrivilegeValueW'; LastError=[Runtime.InteropServices.Marshal]::GetLastWin32Error();
            AdjustErr=$null; Context=$Context
        }
    }

    $tp = New-Object Uefi+TOKEN_PRIVILEGES
    $tp.Count          = 1
    $tp.Priv.Luid      = $luid
    $tp.Priv.Attributes = [Uefi]::SE_PRIVILEGE_ENABLED
    [Uefi]::AdjustTokenPrivileges($tok, $false, [ref]$tp, 0, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    $adjustErr = [Runtime.InteropServices.Marshal]::GetLastWin32Error()

    $r4 = [Uefi]::SetFirmwareEnvironmentVariableExW(
        'NvmeBpResult', '{7B5A1F3E-2D8C-4A91-B6E3-D8F2C9A4E105}',
        [IntPtr]::Zero, 0, 7)
    $err4 = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
    [Uefi]::CloseHandle($tok) | Out-Null

    return @{
        Ok        = [bool]$r4
        Stage     = 'SetFirmwareEnvironmentVariableExW'
        LastError = $err4
        AdjustErr = $adjustErr
        Context   = $Context
    }
}

function Report($result) {
    if ($result.Ok) {
        Note "NvmeBpResult cleared.  (path=$($result.Context))"
        return 0
    }
    if ($result.LastError -eq 203) {
        Note "NvmeBpResult was not set (already clean).  (path=$($result.Context))"
        return 0
    }
    Write-Error ("Clear failed (path=$($result.Context)). Stage=$($result.Stage)  LastError=$($result.LastError)  AdjustErr=$($result.AdjustErr)")
    return 1
}

# --- Pass 1: try directly in the current process token ---
Note "[1] Trying direct path (current process token)..."
$r = & $clearScriptBlock $pinvokeDef 'direct'
if ($r.Ok -or $r.LastError -eq 203) {
    exit (Report $r)
}

# If the failure pattern is "AdjustTokenPrivileges returned ERROR_NOT_ALL_ASSIGNED
# and final call got ERROR_PRIVILEGE_NOT_HELD" — that's the UAC-filter signature.
# Fall back to local WinRM, which gets an unfiltered token.
$isUacSignature = ($r.AdjustErr -eq 1300 -and $r.LastError -eq 1314)
if (-not $isUacSignature) {
    Note "  Direct path failed with unexpected error pattern; not a UAC-filter case."
    exit (Report $r)
}

Note "[2] Direct path hit the UAC-filter token issue (AdjustErr=1300, LastError=1314)."
if ($NoFallback) {
    Note "  -NoFallback passed; not attempting WinRM fallback."
    exit (Report $r)
}

Note "[3] Falling back to Invoke-Command -ComputerName $ComputerName (local WinRM)..."
Note "    enable-remote.ps1 must have been run once on this machine for this to work."

# Try without credentials first (works for current-user local WinRM in some configs).
$winrmArgs = @{
    ComputerName = $ComputerName
    ScriptBlock  = $clearScriptBlock
    ArgumentList = $pinvokeDef, 'winrm-loopback'
    ErrorAction  = 'Stop'
}
if ($Credential) { $winrmArgs.Credential = $Credential }

try {
    $r2 = Invoke-Command @winrmArgs
    exit (Report $r2)
} catch {
    Note "  Invoke-Command failed: $($_.Exception.Message)"
}

# Last resort: prompt for credentials and retry.
if (-not $Credential -and -not $Quiet) {
    Note "[4] Retrying with explicit credentials..."
    $Credential = Get-Credential -Message "Local credentials for $ComputerName (`<machine`>\`<user`>):"
    if ($Credential) {
        try {
            $r3 = Invoke-Command @winrmArgs -Credential $Credential
            exit (Report $r3)
        } catch {
            Write-Error "Credentialed WinRM fallback also failed: $($_.Exception.Message)"
            exit 1
        }
    }
}

Write-Error "All paths failed. Last result: Stage=$($r.Stage)  LastError=$($r.LastError)  AdjustErr=$($r.AdjustErr)"
exit 1
