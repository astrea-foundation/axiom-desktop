$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
foreach ($file in Get-ChildItem "$PSScriptRoot/../scripts/*windows*.ps1") {
    $tokens = $null
    $parseErrors = $null
    $null = [Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$tokens, [ref]$parseErrors)
    if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
}
. "$PSScriptRoot/../scripts/windows-signature.ps1"
$script:signer = [pscustomobject]@{ Name = 'Astrea Labs, Inc.' }
$script:signer | Add-Member ScriptMethod GetNameInfo { param($type, $issuer) $this.Name }
$script:result = [pscustomobject]@{ Status = 'Valid'; SignerCertificate = $script:signer; TimeStamperCertificate = 'timestamp' }
function Get-AuthenticodeSignature { param($LiteralPath, $ErrorAction) return $script:result }
function Assert-Rejected {
    $rejected = $false
    try { Assert-AxiomSignature -FilePath 'fixture.exe' -Publisher 'Astrea Labs, Inc.' } catch { $rejected = $true }
    if (-not $rejected) { throw 'An invalid signature was accepted' }
}
Assert-AxiomSignature -FilePath 'fixture.exe' -Publisher 'Astrea Labs, Inc.'
foreach ($status in 'NotSigned', 'HashMismatch', 'NotTrusted', 'UnknownError') {
    $script:result.Status = $status
    Assert-Rejected
}
$script:result.Status = 'Valid'
$script:signer.Name = 'Different Publisher'
Assert-Rejected
$script:signer.Name = 'Astrea Labs, Inc.'
$script:result.TimeStamperCertificate = $null
Assert-Rejected
