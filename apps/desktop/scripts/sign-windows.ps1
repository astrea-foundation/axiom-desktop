param([Parameter(Mandatory)][string] $FilePath)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') {
    throw 'Signing requires x64 Windows PowerShell 7; the payload can be x64 or ARM64.'
}
foreach ($key in 'ENDPOINT', 'ACCOUNT', 'PROFILE', 'PUBLISHER') {
    if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable("AXIOM_SIGNING_$key"))) {
        throw "AXIOM_SIGNING_$key is required"
    }
}
$FilePath = (Resolve-Path -LiteralPath $FilePath).Path
if ($FilePath -match '[,\r\n]') { throw 'Signing paths cannot contain list separators' }
Import-Module ArtifactSigning -RequiredVersion 0.1.20 -ErrorAction Stop
# Only the Azure CLI login obtained through GitHub OIDC may authenticate. Never
# fall back to a developer's cached identity, managed identity or client secret.
Invoke-ArtifactSigning -Endpoint $env:AXIOM_SIGNING_ENDPOINT `
    -CodeSigningAccountName $env:AXIOM_SIGNING_ACCOUNT `
    -CertificateProfileName $env:AXIOM_SIGNING_PROFILE -Files $FilePath `
    -FileDigest SHA256 -TimestampRfc3161 'http://timestamp.acs.microsoft.com' -TimestampDigest SHA256 `
    -ExcludeEnvironmentCredential -ExcludeWorkloadIdentityCredential -ExcludeManagedIdentityCredential `
    -ExcludeSharedTokenCacheCredential -ExcludeVisualStudioCredential -ExcludeVisualStudioCodeCredential `
    -ExcludeAzurePowerShellCredential -ExcludeAzureDeveloperCliCredential -ExcludeInteractiveBrowserCredential
. "$PSScriptRoot/windows-signature.ps1"
Assert-AxiomSignature -FilePath $FilePath -Publisher $env:AXIOM_SIGNING_PUBLISHER
