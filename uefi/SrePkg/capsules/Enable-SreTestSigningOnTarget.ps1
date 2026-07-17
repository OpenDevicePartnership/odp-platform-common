<#
.SYNOPSIS
    Prepare a TEST target machine to accept the test-signed SRE firmware-update driver
    package (installs the test cert and, optionally, turns on test signing).

.DESCRIPTION
    Windows will only deliver a firmware-update package (INF + signed .cat) whose catalog
    chains to a trusted publisher. For a self-signed TEST cert (e.g. SreFwTest.cer minted
    by New-SreTestSigningCert.ps1) that means, on the TARGET:

      1. Import the cert into LocalMachine\Root          (trusted root)
      2. Import the cert into LocalMachine\TrustedPublisher (so the driver installs silently)
      3. Enable test signing                              (bcdedit -set TESTSIGNING ON) + reboot

    Run this ELEVATED (Administrator) on the test device. This script ONLY touches a test
    machine; never run it on a production system.

.PARAMETER CertPath
    Path to the test .cer (public cert) to trust. Required.

.PARAMETER EnableTestSigning
    Also run "bcdedit /set TESTSIGNING ON". A reboot is required for it to take effect.

.PARAMETER SkipInstall
    Skip importing the cert (only toggle test signing).

.EXAMPLE
    # On the test device, elevated:
    .\Enable-SreTestSigningOnTarget.ps1 -CertPath C:\sre\SreFwTest.cer -EnableTestSigning
    # reboot, then deliver the package (pnputil /add-driver SreRecovery.inf /install)
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $CertPath,

    [switch] $EnableTestSigning,

    [switch] $SkipInstall
)

$ErrorActionPreference = "Stop"

# Require elevation.
$id = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($id)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "This script must be run elevated (Administrator)."
}

if (-not $SkipInstall) {
    if (-not (Test-Path $CertPath)) {
        throw "CertPath not found: $CertPath"
    }
    Write-Host "Importing test cert into LocalMachine\Root ..."
    Import-Certificate -FilePath $CertPath -CertStoreLocation "Cert:\LocalMachine\Root" | Out-Null

    Write-Host "Importing test cert into LocalMachine\TrustedPublisher ..."
    Import-Certificate -FilePath $CertPath -CertStoreLocation "Cert:\LocalMachine\TrustedPublisher" | Out-Null

    Write-Host "Cert installed."
}

if ($EnableTestSigning) {
    Write-Host "Enabling test signing (bcdedit /set TESTSIGNING ON) ..."
    & bcdedit.exe /set TESTSIGNING ON
    if ($LASTEXITCODE -ne 0) {
        throw "bcdedit failed with exit code $LASTEXITCODE (Secure Boot must be OFF to enable test signing)."
    }
    Write-Host ""
    Write-Host "Test signing enabled. REBOOT the target for it to take effect."
}

Write-Host ""
Write-Host "Done. After reboot, deliver the package on the target with e.g.:"
Write-Host "    pnputil /add-driver <path>\SreRecovery.inf /install"
Write-Host "Then confirm the new firmware shows in Device Manager / 'Get-WindowsUpdateLog' ESRT flow."
