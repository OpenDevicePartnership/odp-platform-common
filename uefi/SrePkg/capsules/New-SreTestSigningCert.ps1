<#
.SYNOPSIS
    Create a local TEST signing certificate for SRE capsule bring-up on UNFUSED parts.

.DESCRIPTION
    Mints a single self-signed code-signing certificate and exports it two ways:

      * <OutDir>\SreFwTest.cer  - the public cert. Point the MAA build at it via
                                  SRE_LOCAL_FW_TEST_CERT so it is baked into
                                  PcdFmpDevicePkcs7CertBufferXdr (the FMP trust anchor).
      * <OutDir>\SreFwTest.pfx  - the private key. Hand it to BuildSreCapsule.py
                                  (--builtin-signer signtool) to sign the capsule.

    Because the cert is self-signed, it acts as BOTH the signer and the trust anchor,
    so payload[0] (the SRE descriptor) verifies against the firmware-baked root.

    This certificate is for TEST USE ONLY on parts where production FMP/BootGuard roots
    are not fused/enforced. Do not ship anything signed with it.

    The cert uses the code-signing EKU (1.3.6.1.5.5.7.3.3), so signtool selects it
    without needing an explicit /u <eku> override.

.PARAMETER OutDir
    Directory to write SreFwTest.cer / SreFwTest.pfx into. Defaults to this script's folder.

.PARAMETER Password
    Password for the exported .pfx. Defaults to "sretest".

.PARAMETER Subject
    Certificate subject. Defaults to "CN=SRE FW Test Signer (DO NOT TRUST)".

.EXAMPLE
    .\New-SreTestSigningCert.ps1
    # then build the MAA UEFI to trust it:
    py Platform\Surface\SurfPtl\Msft900MaaPkg\PlatformBuild.py TARGET=DEBUG `
       SRE_LOCAL_FW_TEST_CERT=Common\WSSI_ODP\SrePkg\capsules\SreFwTest.cer
    # then sign the capsule with the matching key:
    py Common\WSSI_ODP\SrePkg\capsules\BuildSreCapsule.py --wim D:\images\sre.wim `
       --version 0x00010000 --builtin-signer signtool `
       -ds key_file=Common\WSSI_ODP\SrePkg\capsules\SreFwTest.pfx -ds key_pass=sretest `
       D:\out\SreRecovery.bin
#>
[CmdletBinding()]
# Generates a throwaway self-signed test-signing certificate; the plaintext
# password is a fixed test value passed to BuildSreCapsule.py and never a
# production secret.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingConvertToSecureStringWithPlainText', '')]
param(
    [string] $OutDir = $PSScriptRoot,
    [string] $Password = "sretest",
    [string] $Subject = "CN=SRE FW Test Signer (DO NOT TRUST)"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $OutDir)) {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
}

$cerPath = Join-Path $OutDir "SreFwTest.cer"
$pfxPath = Join-Path $OutDir "SreFwTest.pfx"

Write-Host "Creating self-signed code-signing certificate: $Subject"
$cert = New-SelfSignedCertificate `
    -Type CodeSigningCert `
    -Subject $Subject `
    -KeyAlgorithm RSA `
    -KeyLength 2048 `
    -HashAlgorithm SHA256 `
    -KeyExportPolicy Exportable `
    -CertStoreLocation "Cert:\CurrentUser\My" `
    -NotAfter (Get-Date).AddYears(5)

try {
    # Public cert (DER) -> trust anchor baked into the firmware.
    Export-Certificate -Cert $cert -FilePath $cerPath -Type CERT | Out-Null

    # Private key (PKCS#12) -> capsule signer.
    $securePw = ConvertTo-SecureString -String $Password -Force -AsPlainText
    Export-PfxCertificate -Cert $cert -FilePath $pfxPath -Password $securePw | Out-Null
}
finally {
    # Don't leave the private key sitting in the user's personal store.
    Remove-Item ("Cert:\CurrentUser\My\" + $cert.Thumbprint) -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "Wrote trust anchor (.cer): $cerPath"
Write-Host "Wrote signer key  (.pfx): $pfxPath   (password: $Password)"
Write-Host "SHA1 thumbprint         : $($cert.Thumbprint)"
Write-Host ""
Write-Host "Next steps:"
Write-Host "  1) Build the MAA UEFI to trust this cert (DEBUG / unfused part):"
Write-Host "       py Platform\Surface\SurfPtl\Msft900MaaPkg\PlatformBuild.py TARGET=DEBUG ``"
Write-Host "          SRE_LOCAL_FW_TEST_CERT=$cerPath"
Write-Host "  2) Sign the capsule with the matching key:"
Write-Host "       py Common\WSSI_ODP\SrePkg\capsules\BuildSreCapsule.py --wim <sre.wim> ``"
Write-Host "          --version 0x00010000 --builtin-signer signtool ``"
Write-Host "          -ds key_file=$pfxPath -ds key_pass=$Password <out.bin>"
